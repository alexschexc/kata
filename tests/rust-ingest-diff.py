"""Ingest the example EPUBs with the Zig and Rust binaries and require identical output.

Compares every converted document and image asset byte-for-byte, and the manifest/index JSON
ignoring mtimes. Skips (exit 0) when examplePubs/ is absent: the samples are local, not in git.
Usage: python tests/rust-ingest-diff.py [ZIG_BIN [RUST_BIN]]
"""
from pathlib import Path
import hashlib, json, os, subprocess, sys, tempfile

ROOT = Path(__file__).resolve().parents[1]
args = sys.argv[1:]
ZIG = Path(args[0] if args else os.environ.get('KATA_ZIG_BIN', ROOT/'zig-out/bin/kata')).resolve()
RUST = Path(args[1] if len(args) > 1 else os.environ.get('KATA_BIN', ROOT/'rust/target/release/kata')).resolve()
PUBS = ROOT/'examplePubs'
if not list(PUBS.glob('*.epub')):
    print('SKIP examplePubs/ has no EPUBs')
    sys.exit(0)


def ingest(binary, library):
    env = dict(os.environ, HOME=str(library.parent), XDG_CONFIG_HOME=str(library.parent/'cfg'))
    result = subprocess.run([str(binary), 'ingest', '--ingest-dir', str(PUBS), '--library-dir', str(library)],
                            capture_output=True, text=True, env=env, timeout=600)
    assert result.returncode == 0, (binary, result.stdout[-500:], result.stderr[-500:])
    return result.stdout.splitlines()


def tree(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file() and p.name not in ('.kata-manifest.json', '.kata-index.json')}


def normalised(path):
    data = json.loads(path.read_text())
    for entry in data['entries']:
        entry.pop('mtime_ns', None)
        entry.get('fingerprint', {}).pop('mtime_ns', None)
    return data


with tempfile.TemporaryDirectory(prefix='kata-ingest-diff-') as folder:
    zig_lib, rust_lib = Path(folder)/'zig'/'lib', Path(folder)/'rust'/'lib'
    zig_lib.mkdir(parents=True)
    rust_lib.mkdir(parents=True)
    zig_out, rust_out = ingest(ZIG, zig_lib), ingest(RUST, rust_lib)
    strip = lambda lines: [l.replace(str(zig_lib), 'LIB').replace(str(rust_lib), 'LIB') for l in lines]
    assert strip(zig_out) == strip(rust_out), 'ingest reports differ'
    zig_files, rust_files = tree(zig_lib), tree(rust_lib)
    assert zig_files.keys() == rust_files.keys(), (set(zig_files) ^ set(rust_files))
    differing = [name for name in zig_files if zig_files[name] != rust_files[name]]
    assert not differing, differing[:5]
    for name in ('.kata-manifest.json', '.kata-index.json'):
        assert normalised(zig_lib/name) == normalised(rust_lib/name), name
    # A second run converts nothing.
    again = ingest(RUST, rust_lib)
    assert any('0 converted' in line for line in again), again[-1]
    print(f'PASS ingest: {len(zig_files)} files byte-identical between Zig and Rust; second run is a no-op')
