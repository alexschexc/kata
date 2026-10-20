//! Raw-mode terminal over crossterm, replacing `terminal_posix.zig` and `terminal_windows.zig`.
//!
//! The application consumes a byte stream (see [`crate::input::Decoder`]), so the contract is
//! byte-oriented: [`Terminal::read_byte`] yields one byte, a timeout (80 ms idle, used to tell a
//! lone ESC from a sequence) or an ignorable interruption. Unix reads fd 0 directly with poll;
//! Windows has no byte stream, so key events are translated into the VT bytes a Unix terminal
//! would send.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub columns: usize,
    pub rows: usize,
    /// Text area in pixels from the window size ioctl (0 if unknown).
    pub pixel_width: usize,
    pub pixel_height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Byte(u8),
    Timeout,
    Ignored,
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn is_interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

pub fn interrupt() {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Writes all bytes to stdout and flushes.
pub fn write_all(bytes: &[u8]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)?;
    out.flush()
}

/// Terminal size, defaulting an unreported dimension to 100x30 like the Zig build.
pub fn size() -> io::Result<Size> {
    let (columns, rows) = crossterm::terminal::size()?;
    let (pixel_width, pixel_height) = crossterm::terminal::window_size()
        .map(|w| (w.width as usize, w.height as usize))
        .unwrap_or((0, 0));
    Ok(Size {
        columns: if columns > 0 { columns as usize } else { 100 },
        rows: if rows > 0 { rows as usize } else { 30 },
        pixel_width,
        pixel_height,
    })
}

/// Raw-mode guard: restores the terminal (and signal handlers on Unix) on drop.
pub struct Terminal {
    #[cfg(unix)]
    old_handlers: [libc::sighandler_t; 3],
    #[cfg(windows)]
    console: windows_console::Saved,
}

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

impl Terminal {
    pub fn new() -> io::Result<Terminal> {
        use std::io::IsTerminal;
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other("InteractiveTerminalRequired"));
        }
        crossterm::terminal::enable_raw_mode()?;
        INTERRUPTED.store(false, Ordering::Relaxed);
        #[cfg(unix)]
        {
            let mut old = [0 as libc::sighandler_t; 3];
            for (slot, signal) in old.iter_mut().zip(SIGNALS) {
                // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
                *slot =
                    unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
            }
            Ok(Terminal { old_handlers: old })
        }
        #[cfg(windows)]
        Ok(Terminal {
            console: windows_console::enter(),
        })
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        #[cfg(windows)]
        windows_console::leave(&self.console);
        #[cfg(unix)]
        for (signal, handler) in SIGNALS.into_iter().zip(self.old_handlers) {
            // SAFETY: restoring the handler returned by the earlier `signal` call.
            unsafe { libc::signal(signal, handler) };
        }
    }
}

#[cfg(unix)]
fn poll_stdin(timeout_ms: i32) -> io::Result<Option<i16>> {
    let mut descriptor = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd and an exact count of 1.
    let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if ready < 0 {
        let error = io::Error::last_os_error();
        // A signal interrupts poll; the caller checks `is_interrupted`.
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(error)
        };
    }
    Ok(if ready == 0 {
        Some(0)
    } else {
        Some(descriptor.revents)
    })
}

/// True when input is waiting (non-blocking), so callers can apply several keys before an
/// expensive redraw.
#[cfg(unix)]
pub fn pending() -> bool {
    matches!(poll_stdin(0), Ok(Some(revents)) if revents & libc::POLLIN != 0)
}

/// Waits up to 80 ms for one input byte.
#[cfg(unix)]
pub fn read_byte() -> io::Result<Read> {
    let revents = match poll_stdin(80) {
        Ok(Some(revents)) => revents,
        Ok(None) => {
            return Ok(if is_interrupted() {
                Read::Ignored
            } else {
                Read::Timeout
            });
        }
        Err(error) => return Err(error),
    };
    if revents == 0 {
        return Ok(Read::Timeout);
    }
    if revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        && revents & libc::POLLIN == 0
    {
        return Err(io::Error::other("TerminalRead"));
    }
    let mut value = 0u8;
    // SAFETY: reading one byte into a valid local.
    let count = unsafe { libc::read(0, (&mut value as *mut u8).cast(), 1) };
    if count == 1 {
        Ok(Read::Byte(value))
    } else {
        Err(io::Error::other("TerminalRead"))
    }
}

/// Output side of the Windows console: UTF-8 code page and VT escape processing (the Zig build
/// set both). Failures are ignored: Windows Terminal already has VT on, and redirected output
/// has no console to configure.
#[cfg(windows)]
mod windows_console {
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const CP_UTF8: u32 = 65001;
    const ENABLE_PROCESSED_OUTPUT: u32 = 0x1;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x4;

    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> isize;
        fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: isize, mode: u32) -> i32;
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
    }

    pub struct Saved {
        mode: Option<u32>,
        code_page: u32,
    }

    pub fn enter() -> Saved {
        // SAFETY: plain Win32 console calls on the process's own standard handle.
        unsafe {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0u32;
            let saved_mode = (GetConsoleMode(handle, &mut mode) != 0).then_some(mode);
            if saved_mode.is_some() {
                SetConsoleMode(
                    handle,
                    mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                );
            }
            let code_page = GetConsoleOutputCP();
            SetConsoleOutputCP(CP_UTF8);
            Saved {
                mode: saved_mode,
                code_page,
            }
        }
    }

    pub fn leave(saved: &Saved) {
        // SAFETY: restores the values read in `enter`.
        unsafe {
            if let Some(mode) = saved.mode {
                SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), mode);
            }
            if saved.code_page != 0 {
                SetConsoleOutputCP(saved.code_page);
            }
        }
    }
}

#[cfg(windows)]
mod windows_input {
    use super::Read;
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::time::Duration;

    thread_local! { static QUEUE: RefCell<VecDeque<u8>> = const { RefCell::new(VecDeque::new()) }; }

    /// The VT bytes a Unix terminal would send for a key press.
    pub fn key_bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match code {
            KeyCode::Char(c)
                if modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_alphabetic() =>
            {
                vec![(c.to_ascii_lowercase() as u8) & 0x1f]
            }
            KeyCode::Char(c) => c.to_string().into_bytes(),
            KeyCode::Enter => vec![b'\r'],
            KeyCode::Tab => vec![b'\t'],
            KeyCode::Backspace => vec![0x7f],
            KeyCode::Esc => vec![0x1b],
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Right if shift => b"\x1b[1;2C".to_vec(),
            KeyCode::Left if shift => b"\x1b[1;2D".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Left => b"\x1b[D".to_vec(),
            _ => Vec::new(),
        }
    }

    fn fill(wait: Duration) -> io::Result<()> {
        if event::poll(wait)?
            && let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            QUEUE.with(|q| q.borrow_mut().extend(key_bytes(key.code, key.modifiers)));
        }
        Ok(())
    }

    pub fn pending() -> bool {
        QUEUE.with(|q| !q.borrow().is_empty()) || event::poll(Duration::ZERO).unwrap_or(false)
    }

    pub fn read_byte() -> io::Result<Read> {
        if QUEUE.with(|q| q.borrow().is_empty()) {
            fill(Duration::from_millis(80))?;
        }
        Ok(QUEUE
            .with(|q| q.borrow_mut().pop_front())
            .map_or(Read::Timeout, Read::Byte))
    }
}

#[cfg(windows)]
pub use windows_input::{pending, read_byte};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_flag_round_trips() {
        INTERRUPTED.store(false, Ordering::Relaxed);
        assert!(!is_interrupted());
        interrupt();
        assert!(is_interrupted());
        INTERRUPTED.store(false, Ordering::Relaxed);
    }

    #[test]
    fn terminal_requires_a_tty() {
        // `cargo test` runs without a controlling terminal on stdin.
        use std::io::IsTerminal;
        if !io::stdin().is_terminal() {
            assert!(Terminal::new().is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_keys_translate_to_the_bytes_a_unix_terminal_sends() {
        use crossterm::event::{KeyCode, KeyModifiers};
        use windows_input::key_bytes;
        let none = KeyModifiers::NONE;
        assert_eq!(key_bytes(KeyCode::Char('j'), none), b"j");
        assert_eq!(key_bytes(KeyCode::Char('ά'), none), "ά".as_bytes());
        assert_eq!(key_bytes(KeyCode::Char('d'), KeyModifiers::CONTROL), [4]);
        assert_eq!(key_bytes(KeyCode::Char('u'), KeyModifiers::CONTROL), [21]);
        assert_eq!(key_bytes(KeyCode::Enter, none), b"\r");
        assert_eq!(key_bytes(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(key_bytes(KeyCode::Esc, none), [0x1b]);
        assert_eq!(key_bytes(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(key_bytes(KeyCode::Left, none), b"\x1b[D");
        assert_eq!(key_bytes(KeyCode::Right, KeyModifiers::SHIFT), b"\x1b[1;2C");
        assert_eq!(key_bytes(KeyCode::Left, KeyModifiers::SHIFT), b"\x1b[1;2D");
        assert!(key_bytes(KeyCode::F(5), none).is_empty());
    }
}
