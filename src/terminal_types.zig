/// `pixel_*`: text area in pixels from the window size ioctl (0 if unknown).
pub const Size = struct { columns: usize, rows: usize, pixel_width: usize = 0, pixel_height: usize = 0 };
pub const Read = union(enum) { byte: u8, timeout, ignored };
