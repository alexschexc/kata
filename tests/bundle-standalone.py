"""Exercise a copied Kata executable with no applets, shell tools, or repo cwd."""
from pathlib import Path
import json, os, shutil, subprocess, sys, tempfile

ROOT = Path(__file__).resolve().parents[1]
original = Path(os.environ.get('KATA_BIN', str(ROOT/'zig-out/bin/kata')))
with tempfile.TemporaryDirectory(prefix='kata-standalone-') as folder:
    tmp = Path(folder)
    binary = tmp/'kata'
    shutil.copy2(original, binary)
    empty = tmp/'empty-path'
    empty.mkdir()
    env = dict(os.environ, PATH=str(empty), HOME=str(tmp), XDG_CONFIG_HOME=str(tmp/'config'), XDG_STATE_HOME=str(tmp/'state'))
    for name in ('kjv', 'grb', 'vul', 'sh', 'awk', 'sed', 'tar'):
        assert shutil.which(name, path=env['PATH']) is None
    for passage in ('Genesis:1', 'John:1:1-3', 'Judges (Vaticanus):1', 'Sirach:0', '2 Kings:25', 'Odes:1'):
        result = subprocess.run([str(binary), '--passage', passage, '--dump'], env=env, cwd=tmp,
                                text=True, capture_output=True, timeout=30)
        assert result.returncode == 0, (passage, result.stderr)
        assert passage in result.stdout and 'NOTE:' in result.stdout
        assert not result.stderr
    licenses = subprocess.run([str(binary), '--licenses'], env=env, cwd=tmp, text=True, capture_output=True, timeout=30)
    assert licenses.returncode == 0, licenses.stderr
    assert 'SBL' in licenses.stdout and 'Attribution 4.0' in licenses.stdout
    provenance = json.loads((ROOT/'src/data/provenance.json').read_text())
    for source in provenance['sources'].values():
        assert source['tsv_sha256'] in licenses.stdout, 'dataset provenance missing from single executable'
    assert not (tmp/'state').exists(), 'Read-only commands wrote progress'
    # Existing full-library PTY harness is run by the host Python, but the
    # child executable inherits this empty PATH and isolated HOME.
    p = subprocess.run([sys.executable, str(ROOT/'tests/full-library-pty.py')],
                       env=dict(env, KATA_BIN=str(binary)), cwd=tmp, capture_output=True, text=True, timeout=150)
    assert p.returncode == 0, p.stdout+p.stderr
    print(p.stdout, end='')
    p = subprocess.run([sys.executable, str(ROOT/'tests/input-pty.py')],
                       env=dict(env, KATA_BIN=str(binary)), cwd=tmp, capture_output=True, text=True, timeout=150)
    assert p.returncode == 0, p.stdout+p.stderr
    print(p.stdout, end='')
print('PASS copied single executable: empty PATH, no applets/shell utilities, no repository cwd, dump/readers/menus/plans/licenses, isolated progress')
