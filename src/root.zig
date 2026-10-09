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
pub const library = @import("library.zig");
pub const reading = @import("reading.zig");
pub const start_menu = @import("start_menu.zig");
pub const search = @import("search.zig");
pub const ingest = @import("ingest.zig");
pub const epub = @import("epub.zig");

test {
    @import("std").testing.refAllDecls(@This());
}
