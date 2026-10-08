const std = @import("std");

/// Single producer/single consumer queue. One reserved cell distinguishes full.
pub fn Queue(comptime capacity: usize) type {
    return struct {
        bytes: [capacity]u8 = undefined,
        head: std.atomic.Value(usize) = .init(0),
        tail: std.atomic.Value(usize) = .init(0),
        pub fn push(self: *@This(), value: u8) bool {
            const at = self.head.load(.monotonic);
            const next = (at + 1) % capacity;
            if (next == self.tail.load(.acquire)) return false;
            self.bytes[at] = value;
            self.head.store(next, .release);
            return true;
        }
        pub fn pop(self: *@This()) ?u8 {
            const at = self.tail.load(.monotonic);
            if (at == self.head.load(.acquire)) return null;
            const value = self.bytes[at];
            self.tail.store((at + 1) % capacity, .release);
            return value;
        }
    };
}

test "console byte queue preserves fragmented VT input and refuses overflow" {
    var queue: Queue(4) = .{};
    try std.testing.expect(queue.pop() == null);
    try std.testing.expect(queue.push(27));
    try std.testing.expect(queue.push('['));
    try std.testing.expect(queue.push('A'));
    try std.testing.expect(!queue.push('q'));
    try std.testing.expectEqual(@as(?u8, 27), queue.pop());
    try std.testing.expect(queue.push('j'));
    try std.testing.expectEqual(@as(?u8, '['), queue.pop());
    try std.testing.expectEqual(@as(?u8, 'A'), queue.pop());
    try std.testing.expectEqual(@as(?u8, 'j'), queue.pop());
    try std.testing.expect(queue.pop() == null);
}
