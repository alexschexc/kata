//! Key bytes the application recognises. The full input decoder lands with the terminal layer;
//! the picker only needs to know which bytes are meaningful.

/// True for bytes that dispatch an action; everything else is inert.
pub fn recognized(byte: u8) -> bool {
    matches!(
        byte,
        3 | 4 | 8 | 9 | 10 | 13 | 21 | 27 | 127 | b'0'
            ..=b'9'
                | b'j'
                | b'k'
                | b'h'
                | b'l'
                | b'n'
                | b'y'
                | b'c'
                | b'q'
                | b'm'
                | b'o'
                | b'H'
                | b'L'
                | b'f'
                | b'b'
                | b'g'
                | b'G'
                | b's'
                | b'p'
                | b'd'
                | b'/'
                | b'N'
                | b'r'
                | b'x'
                | b't'
                | b'i'
                | b'+'
                | b'='
                | b'-'
                | b'_'
                | b'e'
    )
}
