"""Byte-for-byte comparison of the Rust binary against the frozen Zig build (V0.3.1 sources).

Runs every book, every aligned chapter and a set of CLI edge cases through both binaries in an
isolated environment and requires identical stdout and exit codes. Needs no installed applets,
so it runs in CI. Usage: python tests/rust-dump-diff.py [ZIG_BIN [RUST_BIN]]
Defaults: zig-out/bin/kata and rust/target/release/kata (or KATA_ZIG_BIN / KATA_BIN).
"""
from pathlib import Path
import json, os, re, subprocess, sys, tempfile

ROOT = Path(__file__).resolve().parents[1]
args = sys.argv[1:]
ZIG = Path(args[0] if args else os.environ.get('KATA_ZIG_BIN', ROOT/'zig-out/bin/kata')).resolve()
RUST = Path(args[1] if len(args) > 1 else os.environ.get('KATA_BIN', ROOT/'rust/target/release/kata')).resolve()
books = sorted({row['canonical'] for row in json.loads((ROOT/'tests/book_discovery.json').read_text())})

with tempfile.TemporaryDirectory(prefix='kata-dump-diff-') as folder:
    env = dict(os.environ, XDG_STATE_HOME=folder, XDG_CONFIG_HOME=folder, HOME=folder, PATH=folder)
    compared = failures = 0

    def run(binary, arguments):
        result = subprocess.run([str(binary), *arguments], capture_output=True, env=env, cwd=folder, timeout=120)
        return result.returncode, result.stdout

    def compare(arguments):
        global compared, failures
        compared += 1
        zig, rust = run(ZIG, arguments), run(RUST, arguments)
        if zig != rust:
            failures += 1
            print('DIFF', arguments, 'exit', zig[0], rust[0], 'bytes', len(zig[1]), len(rust[1]))

    for book in books:
        compare(['--passage', book, '--dump'])
        whole = run(ZIG, ['--passage', book, '--dump'])[1].decode()
        for chapter in sorted({int(m.group(1)) for m in re.finditer(r'^.+ (\d+):\d+$', whole, re.M)}):
            compare(['--passage', f'{book}:{chapter}', '--dump'])
    for arguments in (['--check-plan'], ['--licenses'], ['--help'], ['--passage', 'John:1:1-3', '--dump'],
                      ['--passage', 'Psalms:22', '--dump'], ['--start-day', '88'], ['--start-day', '1'],
                      ['--start-day', '0'], ['--bogus'], ['--passage', 'John::1', '--dump'],
                      ['--complete', '--dump'], ['--passage'], ['--passage', 'Nowhere:1', '--dump']):
        compare(arguments)
    print(f'compared {compared} invocations; differing: {failures}')
    sys.exit(1 if failures else 0)
