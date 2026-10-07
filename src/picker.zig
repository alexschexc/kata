const std = @import("std");

pub const Action = union(enum) { choose_plan: usize, start_day: usize };
pub const Kind = enum { closed, plans, days, confirm_day };
pub const Picker = struct {
    kind: Kind = .closed,
    selected: usize = 0,
    digits: [20]u8 = undefined,
    digit_count: usize = 0,
    invalid_input: bool = false,

    pub fn handle(self: *Picker, key: u8, count: usize) ?Action {
        if (key == 27 or key == 'q') {
            self.kind = .closed;
            self.digit_count = 0;
            return null;
        }
        if (count == 0 or self.kind == .closed) return null;
        if (self.kind == .confirm_day) {
            self.kind = .days;
            if (key == 'y') return .{ .start_day = self.selected + 1 };
            return null;
        }
        if (self.kind == .days and std.ascii.isDigit(key)) {
            if (self.digit_count < self.digits.len) {
                self.digits[self.digit_count] = key;
                self.digit_count += 1;
                self.invalid_input = false;
            } else self.invalid_input = true;
            return null;
        }
        if (key == 127 or key == 8) {
            self.digit_count -|= 1;
            self.invalid_input = false;
            return null;
        }
        switch (key) {
            'j' => self.selected = @min(count - 1, self.selected +| 1),
            'k' => self.selected -|= 1,
            4 => self.selected = @min(count - 1, self.selected +| 10),
            21 => self.selected -|= 10,
            'g' => self.selected = 0,
            'G' => self.selected = count - 1,
            10, 13 => {
                if (self.kind == .plans) return .{ .choose_plan = self.selected };
                if (self.digit_count > 0) {
                    const day = std.fmt.parseInt(usize, self.digits[0..self.digit_count], 10) catch {
                        self.invalid_input = true;
                        return null;
                    };
                    if (self.invalid_input or day == 0 or day > count) {
                        self.invalid_input = true;
                        return null;
                    }
                    self.selected = day - 1;
                    self.digit_count = 0;
                }
                self.kind = .confirm_day;
                return null;
            },
            else => return null,
        }
        self.digit_count = 0;
        self.invalid_input = false;
        return null;
    }
};

test "plan picker navigates and chooses only on Enter" {
    var picker: Picker = .{ .kind = .plans };
    try std.testing.expect(picker.handle('j', 2) == null);
    const action = picker.handle(13, 2) orelse return error.TestExpectedAction;
    try std.testing.expectEqual(@as(usize, 1), action.choose_plan);
}

test "day number entry previews then needs separate confirmation" {
    var picker: Picker = .{ .kind = .days };
    _ = picker.handle('8', 89);
    _ = picker.handle('8', 89);
    try std.testing.expect(picker.handle(13, 89) == null);
    try std.testing.expectEqual(Kind.confirm_day, picker.kind);
    const action = picker.handle('y', 89) orelse return error.TestExpectedAction;
    try std.testing.expectEqual(@as(usize, 88), action.start_day);
}

test "invalid days and cancelled dialogs never emit progress changes" {
    var picker: Picker = .{ .kind = .days };
    _ = picker.handle('9', 89);
    _ = picker.handle('0', 89);
    try std.testing.expect(picker.handle(13, 89) == null);
    try std.testing.expect(picker.invalid_input);
    try std.testing.expectEqual(Kind.days, picker.kind);
    _ = picker.handle(27, 89);
    try std.testing.expectEqual(Kind.closed, picker.kind);
    picker = .{ .kind = .confirm_day, .selected = 87 };
    try std.testing.expect(picker.handle('n', 89) == null);
    try std.testing.expectEqual(Kind.days, picker.kind);
}
