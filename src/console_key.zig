const std = @import("std");

test "console key records ignore key up noncharacters Unicode and raw Alt shortcuts" {
    try std.testing.expectEqual(@as(?u8, 'j'), byte(true, 'j', 0, 74));
    try std.testing.expectEqual(@as(?u8, 13), byte(true, 13, 0, 13));
    try std.testing.expectEqual(@as(?u8, 4), byte(true, 4, 8, 68));
    try std.testing.expect(byte(false, 'q', 0, 81) == null);
    try std.testing.expect(byte(true, 0, 0, 38) == null);
    try std.testing.expect(byte(true, 0x3b1, 0, 0) == null);
    try std.testing.expect(byte(true, 'q', 2, 81) == null);
    try std.testing.expectEqual(@as(?u8, 27), byte(true, 27, 2, 0));
}
