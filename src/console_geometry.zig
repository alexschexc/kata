const std = @import("std");

pub fn extent(first: i16, last: i16, fallback: usize) usize {
    const length = @as(i32, last) - @as(i32, first) + 1;
    return if (length > 0) @intCast(length) else fallback;
}

test "console visible window uses inclusive coordinates rather than backing buffer" {
    try std.testing.expectEqual(@as(usize, 80), extent(20, 99, 100));
    try std.testing.expectEqual(@as(usize, 1), extent(5, 5, 30));
    try std.testing.expectEqual(@as(usize, 100), extent(10, 9, 100));
    try std.testing.expectEqual(@as(usize, 65536), extent(-32768, 32767, 100));
}
