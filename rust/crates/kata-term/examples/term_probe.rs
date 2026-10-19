//! PTY probe for tests/term-pty.py: raw mode, size report, decoded keys, clean restore on 'q'.
use kata_term::backend::{self, Read, Terminal};
use kata_term::input::Decoder;

fn main() {
    let terminal = Terminal::new().expect("terminal");
    let size = backend::size().expect("size");
    backend::write_all(format!("size {}x{}\r\n", size.columns, size.rows).as_bytes()).unwrap();
    let mut decoder = Decoder::new();
    loop {
        match backend::read_byte().expect("read") {
            Read::Byte(b) => {
                if let Some(key) = decoder.feed(b) {
                    backend::write_all(format!("key {}\r\n", key as char).as_bytes()).unwrap();
                    if key == b'q' {
                        break;
                    }
                }
            }
            Read::Timeout => {
                if let Some(key) = decoder.timeout() {
                    backend::write_all(format!("key esc{key}\r\n").as_bytes()).unwrap();
                }
            }
            Read::Ignored => break,
        }
        if backend::is_interrupted() {
            break;
        }
    }
    drop(terminal);
    backend::write_all(b"restored\r\n").unwrap();
}
