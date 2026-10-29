//! Ctrl+R: run the ingest check while Kata is open. The work happens on a background thread;
//! screens keep running, a one-line overlay reports progress, and the Library reloads its book
//! list when a run that succeeded has finished.

use kata_core::ingest::{self, Folders};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the result stays on screen after a run finishes.
const RESULT_SHOWN: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done {
        converted: usize,
        failed: usize,
        unchanged: usize,
    },
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Started,
    AlreadyRunning,
    NoFolders,
}

#[derive(Default)]
struct Inner {
    folders: Option<Folders>,
    running: bool,
    /// The last message (a refusal or a result) and when it appeared.
    status: Option<(String, Instant)>,
    /// A successful run finished and the book list has not been reloaded yet.
    refresh: bool,
    /// What the last `changed()` call saw, so screens redraw only when it differs.
    last_seen: Option<String>,
}

impl Inner {
    fn text(&self) -> Option<String> {
        if self.running {
            return Some("Ingest running…".into());
        }
        let (text, at) = self.status.as_ref()?;
        (at.elapsed() <= RESULT_SHOWN).then(|| text.clone())
    }
}

#[derive(Clone, Default)]
pub struct LiveIngest {
    inner: Arc<Mutex<Inner>>,
}

impl LiveIngest {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The folders a run uses; None when they are not chosen yet.
    pub fn set_folders(&self, folders: Option<Folders>) {
        self.lock().folders = folders;
    }

    /// Starts a run with the real ingest, on a background thread.
    pub fn request(&self) -> Request {
        self.request_with(|folders| {
            ingest::run(folders, false)
                .map(|s| Outcome::Done {
                    converted: s.converted,
                    failed: s.failed,
                    unchanged: s.unchanged,
                })
                .unwrap_or_else(|e| Outcome::Failed(e.to_string()))
        })
    }

    pub fn request_with<F>(&self, run: F) -> Request
    where
        F: FnOnce(&Folders) -> Outcome + Send + 'static,
    {
        let folders = {
            let mut inner = self.lock();
            if inner.running {
                // The "Ingest running…" line already tells the user a run is going.
                return Request::AlreadyRunning;
            }
            let Some(folders) = inner.folders.clone() else {
                inner.status = Some((
                    "Choose the ingest folders first (last Library entry).".into(),
                    Instant::now(),
                ));
                return Request::NoFolders;
            };
            inner.running = true;
            inner.status = None;
            folders
        };
        let shared = self.inner.clone();
        std::thread::spawn(move || {
            let outcome = run(&folders);
            let text = match &outcome {
                Outcome::Done {
                    converted: 0,
                    failed: 0,
                    ..
                } => "Ingest: nothing new".to_string(),
                Outcome::Done {
                    converted, failed, ..
                } => format!("Ingest: {converted} new or changed, {failed} failed"),
                Outcome::Failed(error) => format!("Ingest failed: {error}"),
            };
            let mut inner = shared.lock().unwrap_or_else(|e| e.into_inner());
            inner.refresh = matches!(outcome, Outcome::Done { .. });
            inner.status = Some((text, Instant::now()));
            inner.running = false;
        });
        Request::Started
    }

    /// True while a finished run's books are not yet loaded (does not clear it).
    pub fn refresh_pending(&self) -> bool {
        self.lock().refresh
    }

    /// True once after a successful run: the caller reloads the book list.
    pub fn take_refresh(&self) -> bool {
        std::mem::take(&mut self.lock().refresh)
    }

    /// The message to show instead of `base` while a run is going or just finished.
    pub fn overlay_or(&self, base: &str) -> String {
        self.lock().text().unwrap_or_else(|| base.to_string())
    }

    /// True when the overlay text differs from the last call, so a screen should redraw.
    pub fn changed(&self) -> bool {
        let mut inner = self.lock();
        let now = inner.text();
        let differs = now != inner.last_seen;
        inner.last_seen = now;
        differs
    }
}

/// The application-wide instance.
pub fn global() -> &'static LiveIngest {
    static GLOBAL: OnceLock<LiveIngest> = OnceLock::new();
    GLOBAL.get_or_init(LiveIngest::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn folders() -> Folders {
        Folders {
            version: 1,
            ingest: "/i".into(),
            library: "/l".into(),
        }
    }

    fn wait_idle(live: &LiveIngest) {
        for _ in 0..200 {
            if !live.lock().running {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("run never finished");
    }

    #[test]
    fn no_folders_starts_nothing() {
        let live = LiveIngest::default();
        assert_eq!(live.overlay_or("base"), "base");
        assert_eq!(live.request_with(|_| unreachable!()), Request::NoFolders);
        assert_eq!(
            live.overlay_or("base"),
            "Choose the ingest folders first (last Library entry)."
        );
    }

    #[test]
    fn a_second_request_during_a_run_is_refused() {
        let live = LiveIngest::default();
        live.set_folders(Some(folders()));
        let (release, wait) = mpsc::channel::<()>();
        assert_eq!(
            live.request_with(move |_| {
                let _ = wait.recv();
                Outcome::Done {
                    converted: 1,
                    failed: 0,
                    unchanged: 0,
                }
            }),
            Request::Started
        );
        assert_eq!(live.overlay_or("base"), "Ingest running…");
        assert!(live.changed());
        assert!(!live.changed());
        assert_eq!(
            live.request_with(|_| unreachable!()),
            Request::AlreadyRunning
        );
        assert_eq!(live.overlay_or("base"), "Ingest running…");
        release.send(()).unwrap();
        wait_idle(&live);
        assert_eq!(
            live.overlay_or("base"),
            "Ingest: 1 new or changed, 0 failed"
        );
    }

    #[test]
    fn a_successful_run_asks_for_one_refresh() {
        let live = LiveIngest::default();
        live.set_folders(Some(folders()));
        live.request_with(|_| Outcome::Done {
            converted: 0,
            failed: 0,
            unchanged: 3,
        });
        wait_idle(&live);
        assert_eq!(live.overlay_or("base"), "Ingest: nothing new");
        assert!(live.refresh_pending());
        assert!(live.take_refresh());
        assert!(!live.take_refresh());
    }

    #[test]
    fn a_failed_run_keeps_the_library_and_says_why() {
        let live = LiveIngest::default();
        live.set_folders(Some(folders()));
        live.request_with(|_| Outcome::Failed("disk full".into()));
        wait_idle(&live);
        assert!(!live.refresh_pending());
        assert_eq!(live.overlay_or("base"), "Ingest failed: disk full");
        // A new run is allowed again.
        assert_eq!(
            live.request_with(|_| Outcome::Failed("x".into())),
            Request::Started
        );
        wait_idle(&live);
    }
}
