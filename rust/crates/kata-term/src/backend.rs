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
        #[cfg(not(unix))]
        Ok(Terminal {})
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
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
}
