"""Decode every PNG/JPEG in the sample EPUBs with the Zig and Rust decoders; require identical pixels.

The Zig probe is built from a temporary copy of src/png.zig and src/jpeg.zig (they import only
std), so nothing is added to the repository tree. Skips (exit 0) when examplePubs/ is absent.
Usage: python tests/rust-image-diff.py [ZIG_EXE]   (needs `cargo build --release --example pixel_probe`)
"""
from pathlib import Path
import hashlib, os, shutil, subprocess, sys, tempfile, zipfile

ROOT = Path(__file__).resolve().parents[1]
pinned = Path.home()/'.local/share/mise/installs/zig/0.16.0/bin/zig'  # the project pins Zig 0.16.0
ZIG = os.environ.get('ZIG') or (sys.argv[1] if len(sys.argv) > 1 else str(pinned) if pinned.exists() else shutil.which('zig'))
RUST_PROBE = ROOT/'rust/target/release/examples/pixel_probe'
epubs = sorted((ROOT/'examplePubs').glob('*.epub'))
if not epubs:
    print('SKIP examplePubs/ has no EPUBs')
    sys.exit(0)

PROBE = '''const std = @import("std");
const png = @import("png.zig");
const jpeg = @import("jpeg.zig");
pub fn main(init: std.process.Init) !void {
    const a = init.arena.allocator();
    const args = try init.minimal.args.toSlice(a);
    for (args[2..], 0..) |path, n| {
        const bytes = std.Io.Dir.cwd().readFileAlloc(init.io, path, a, .limited(128 << 20)) catch continue;
        const result = if (std.mem.startsWith(u8, bytes, "\\x89PNG")) png.decode(a, bytes) else jpeg.decode(a, bytes);
        const image = result catch continue;
        const name = try std.fmt.allocPrint(a, "{s}/{d}.rgb", .{ args[1], n });
        try std.Io.Dir.cwd().writeFile(init.io, .{ .sub_path = name, .data = image.pixels });
    }
}
'''

with tempfile.TemporaryDirectory(prefix='kata-image-diff-') as folder:
    work = Path(folder)
    build = work/'build'
    build.mkdir()
    for name in ('png.zig', 'jpeg.zig', 'epub.zig', 'xhtml.zig'):  # epub/xhtml: imported by the test blocks
        shutil.copy(ROOT/'src'/name, build/name)
    (build/'probe.zig').write_text(PROBE)
    built = subprocess.run([ZIG, 'build-exe', 'probe.zig', '-O', 'ReleaseFast', '-femit-bin=' + str(work/'zig-probe')],
                           cwd=build, capture_output=True, text=True)
    assert built.returncode == 0, built.stderr[-1500:]
    images = []
    for epub in epubs:
        with zipfile.ZipFile(epub) as z:
            for info in z.infolist():
                if info.filename.lower().endswith(('.png', '.jpg', '.jpeg')):
                    path = work/'img'/f'{len(images)}{Path(info.filename).suffix}'
                    path.parent.mkdir(exist_ok=True)
                    path.write_bytes(z.read(info))
                    images.append(path)
    (work/'zig').mkdir()
    (work/'rust').mkdir()
    subprocess.run([str(work/'zig-probe'), str(work/'zig'), *map(str, images)], check=True)
    subprocess.run([str(RUST_PROBE), str(work/'rust'), *map(str, images)], check=True, stdout=subprocess.DEVNULL)
    decoded = differing = 0
    for n, image in enumerate(images):
        zig, rust = work/'zig'/f'{n}.rgb', work/'rust'/f'{n}.rgb'
        assert zig.exists() == rust.exists(), f'{image.name}: only one decoder succeeded'
        if zig.exists():
            decoded += 1
            if zig.read_bytes() != rust.read_bytes():
                differing += 1
    assert decoded > 900, f'only {decoded} images decoded'
    assert differing == 0, f'{differing} of {decoded} images differ'
    print(f'PASS images: {decoded} PNG/JPEG images decode to identical RGB pixels in Zig and Rust')
