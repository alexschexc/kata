"""Provenance check for the embedded corpora, runnable in CI without the installed applets.

1. Each src/data/<tool>.tsv hashes to the tsv_sha256 / bytes recorded in src/data/provenance.json.
2. The binary under test embeds every corpus byte-for-byte and the same hashes.
3. `--licenses` prints the provenance manifest exactly as committed.
(tests/bundle_sources.py --verify additionally compares against the installed applets.)
Usage: python tests/provenance-check.py /path/to/kata[.exe] [--no-run]
--no-run skips executing the binary (for cross-built targets).
"""
from pathlib import Path
import hashlib, json, subprocess, sys

ROOT = Path(__file__).resolve().parents[1]
binary = Path(sys.argv[1]).resolve()
run_native = '--no-run' not in sys.argv[2:]  # cross-built binaries cannot run on this host
manifest_bytes = (ROOT/'src/data/provenance.json').read_bytes()
manifest = json.loads(manifest_bytes)
data = binary.read_bytes()
assert set(manifest['sources']) == {'kjv', 'grb', 'vul'}, manifest['sources'].keys()
for tool, entry in manifest['sources'].items():
    raw = (ROOT/'src/data'/f'{tool}.tsv').read_bytes()
    assert len(raw) == entry['bytes'], (tool, 'size differs from manifest')
    assert hashlib.sha256(raw).hexdigest() == entry['tsv_sha256'], (tool, 'TSV hash differs from manifest')
    assert raw in data, (tool, 'corpus not embedded byte-for-byte')
    print('ok  ', tool, entry['bytes'], 'bytes', entry['tsv_sha256'][:16])
if run_native:
    notices = subprocess.run([str(binary), '--licenses'], capture_output=True, check=True, timeout=30).stdout
    assert notices.endswith(manifest_bytes), '--licenses does not end with the committed manifest'
    for entry in manifest['sources'].values():
        assert entry['tsv_sha256'].encode() in notices
    print('ok   --licenses prints the committed provenance manifest')
print('PASS provenance: manifest, source files and binary agree')
