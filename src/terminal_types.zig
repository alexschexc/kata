pub const Size = struct { columns: usize, rows: usize };
pub const Read = union(enum) { byte: u8, timeout, ignored };
