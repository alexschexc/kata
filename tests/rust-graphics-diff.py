"""Compare Sixel and Kitty output of the Zig and Rust inline-image renderers byte for byte.

Renders a sample of the sample-book images (whole image, a scrolled slice, and the enlarged
variant) for both protocols with 10x20-pixel cells and requires identical terminal bytes.
Builds the Zig probe from temporary copies of the renderer sources. Skips when examplePubs/ is
absent. Needs `cargo build --release --example render_probe` in rust/.
Usage: python tests/rust-graphics-diff.py [ZIG_EXE]
"""
from pathlib import Path
import os, shutil, subprocess, sys, tempfile, zipfile

ROOT = Path(__file__).resolve().parents[1]
pinned = Path.home()/'.local/share/mise/installs/zig/0.16.0/bin/zig'  # the project pins Zig 0.16.0
ZIG = os.environ.get('ZIG') or (sys.argv[1] if len(sys.argv) > 1 else str(pinned) if pinned.exists() else shutil.which('zig'))
RUST_PROBE = ROOT/'rust/target/release/examples/render_probe'
STRIDE = int(os.environ.get('KATA_GRAPHICS_STRIDE', '5'))
epubs = sorted((ROOT/'examplePubs').glob('*.epub'))
if not epubs:
    print('SKIP examplePubs/ has no EPUBs')
    sys.exit(0)

PROBE = '''const std = @import("std");
const graphics = @import("graphics.zig");
const inline_image = @import("inline_image.zig");
pub fn main(init: std.process.Init) !void {
    const a = init.arena.allocator();
    const args = try init.minimal.args.toSlice(a);
    const protocol: graphics.Protocol = if (std.mem.eql(u8, args[3], "kitty")) .kitty else .sixel;
    for (args[4..], 0..) |name, n| {
        var r: inline_image.Renderer = .{ .allocator = a, .io = init.io, .dir = args[1], .caps = .{ .protocol = protocol, .cell_width = 10, .cell_height = 20, .answered = true } };
        var out: std.Io.Writer.Allocating = .init(a);
        try r.beginFrame(&out.writer);
        const rows = r.rows(name, 80, 30);
        if (rows) |v| try out.writer.print("rows=Some({d})\\n", .{v}) else try out.writer.writeAll("rows=None\\n");
        if (rows != null) {
            const whole = try r.draw(&out.writer, name, 80, 30, 0, 30);
            const part = try r.draw(&out.writer, name, 80, 30, 1, 5);
            r.setEnlarge(true);
            const big = try r.draw(&out.writer, name, 80, 30, 0, 30);
            try out.writer.print("\\n{} {} {}\\n", .{ whole, part, big });
        }
        const path = try std.fmt.allocPrint(a, "{s}/{d}.bin", .{ args[2], n });
        try std.Io.Dir.cwd().writeFile(init.io, .{ .sub_path = path, .data = out.written() });
    }
}
'''

with tempfile.TemporaryDirectory(prefix='kata-graphics-diff-') as folder:
    work = Path(folder)
    build = work/'build'
    build.mkdir()
    for name in ('png.zig', 'jpeg.zig', 'sixel.zig', 'graphics.zig', 'inline_image.zig', 'epub.zig', 'xhtml.zig'):
        shutil.copy(ROOT/'src'/name, build/name)
    (build/'probe.zig').write_text(PROBE)
    built = subprocess.run([ZIG, 'build-exe', 'probe.zig', '-O', 'ReleaseFast', '-femit-bin=' + str(work/'zig-probe')],
                           cwd=build, capture_output=True, text=True)
    assert built.returncode == 0, built.stderr[-1500:]
    assets = work/'assets'
    assets.mkdir()
    names = []
    for epub in epubs:
        with zipfile.ZipFile(epub) as z:
            for info in z.infolist():
                if info.filename.lower().endswith(('.png', '.jpg', '.jpeg')):
                    if len(names) % STRIDE == 0:
                        name = f'{len(names)}{Path(info.filename).suffix.lower()}'
                        (assets/name).write_bytes(z.read(info))
                        names.append(name)
                    else:
                        names.append(None)
    names = [n for n in names if n]
    total = compared = 0
    for protocol in ('sixel', 'kitty'):
        for tool in ('zig', 'rust'):
            (work/protocol/tool).mkdir(parents=True)
        subprocess.run([str(work/'zig-probe'), str(assets), str(work/protocol/'zig'), protocol, *names], check=True)
        subprocess.run([str(RUST_PROBE), str(assets), str(work/protocol/'rust'), protocol, *names], check=True)
        for n, name in enumerate(names):
            z, r = [(work/protocol/t/f'{n}.bin').read_bytes() for t in ('zig', 'rust')]
            total += 1
            assert z == r, f'{protocol} {name}: output differs ({len(z)} vs {len(r)} bytes)'
            compared += b'rows=Some' in z
    assert compared > 50, f'only {compared} drawable images'
    print(f'PASS graphics: {total} sixel/kitty renders ({compared} drawable) are byte-identical in Zig and Rust')
