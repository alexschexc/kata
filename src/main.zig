const std = @import("std");

pub fn main(init: std.process.Init) void {
    @import("kata").cli.run(init, @import("defaults").optina, @import("defaults").gospels) catch |err| {
        std.debug.print("kata: {s}\nUse --help for commands. Check source tools, config, and state path.\n", .{@errorName(err)});
        std.process.exit(1);
    };
}
