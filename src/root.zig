//! Kata's source, layout, scheduling, persistence, and terminal components.
pub const source = @import("source.zig");
pub const layout = @import("layout.zig");
pub const plan = @import("plan.zig");
pub const state = @import("state.zig");
pub const tui = @import("tui.zig");
pub const cli = @import("cli.zig");
pub const catalog = @import("catalog.zig");
pub const picker = @import("picker.zig");
pub const app = @import("app.zig");

test {
    @import("std").testing.refAllDecls(@This());
}
