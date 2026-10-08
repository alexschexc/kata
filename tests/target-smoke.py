"""Native CLI smoke test for a copied Kata binary on Linux, Windows, or macOS.
Python is test tooling only; the copied executable must work without it.
Usage: python tests/target-smoke.py /absolute/path/to/kata[.exe]
Interactive console input/restoration must also be reviewed on the target OS.
"""
from pathlib import Path
import json, os, shutil, subprocess, sys, tempfile

binary = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix='kata-native-smoke-') as directory:
    root = Path(directory)
    copy = root / binary.name
    shutil.copy2(binary, copy)
    empty = root / 'empty-path'
    empty.mkdir()
    env = dict(os.environ, PATH=str(empty), HOME=str(root), USERPROFILE=str(root),
               LOCALAPPDATA=str(root/'local-app-data'), XDG_CONFIG_HOME=str(root/'config'),
               XDG_STATE_HOME=str(root/'state'))
    state = root/'progress.json'
    def run(*args, ok=True):
        result = subprocess.run([str(copy), '--state', str(state), *args],
                                cwd=root, env=env, capture_output=True, timeout=30)
        assert (result.returncode == 0) == ok, (args, result.returncode, result.stderr)
        return result.stdout.decode('utf-8'), result.stderr.decode('utf-8')
    for passage in ('Genesis:1', 'John:1:1-3', 'Sirach:0', 'Odes:1', 'Judges (Vaticanus):1'):
        output, errors = run('--passage', passage, '--dump')
        assert passage in output and not errors, (passage, errors)
    notices, _ = run('--licenses')
    assert 'SBL' in notices and 'tsv_sha256' in notices
    cycle, _ = run('--check-plan')
    assert '89' in cycle
    assert not state.exists(), 'Read-only operations wrote progress'
    preview, _ = run('--start-day', '88')
    assert 'John:20' in preview and 'Revelation:21' in preview
    assert not state.exists(), 'Preview wrote progress'
    run('--start-day', '88', '--confirm')
    saved = state.read_bytes()
    assert json.loads(saved)['next_day'] == 87
    output, _ = run('--dump')
    assert 'day 88/89' in output and 'John 20:1' in output and 'Revelation 21:1' in output
    run('--passage', 'John::1', '--dump', ok=False)
    assert state.read_bytes() == saved, 'Read-only/invalid operation changed progress'
    run('--start-day', '1', '--confirm')
    assert json.loads(state.read_text())['next_day'] == 0, 'Replacing existing state failed'
print('PASS copied native target: empty PATH, Unicode dumps, embedded notices, built-in plan, read-only preview, state create/replace, invalid-reference protection')
