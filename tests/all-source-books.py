"""Read every listed source book through Kata and compare actual verse text.
Python is verification tooling only. No user state is written.
"""
from pathlib import Path
import json, os, re, subprocess, sys, tempfile

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT/'zig-out/bin/kata')))
TOOLS = ('kjv', 'grb', 'vul')

def run(args, env):
    result = subprocess.run(args, capture_output=True, text=True, env=env, cwd='/tmp', timeout=90)
    assert result.returncode == 0, (args, result.stderr)
    return result.stdout

def parse_dump(dump):
    found = {source: {} for source in TOOLS}
    key = None
    for record in dump.splitlines():
        header = re.fullmatch(r'.+ (\d+):(\d+)', record)
        if header:
            key = tuple(map(int, header.groups()))
        for source in TOOLS:
            prefix = source+': '
            if record.startswith(prefix) and key is not None:
                text = record[len(prefix):]
                if text != '[not present under this verse label]':
                    found[source][key] = text
    return found

with tempfile.TemporaryDirectory(prefix='kata-all-source-books-') as folder:
    env = dict(os.environ, XDG_STATE_HOME=folder, XDG_CONFIG_HOME=folder)
    native_env = dict(env, PATH=folder)
    entries = []
    cache = {}
    canonical_books = {}
    totals = {}
    for tool in TOOLS:
        listing = run([tool, '-l'], env).splitlines()
        totals[tool] = len(listing)
        for line in listing:
            match = re.fullmatch(r'(.+) \(([^()]+)\)', line)
            assert match, line
            name, alias = match.groups()
            actual = run([tool, '-W', alias], env)
            expected = {}
            current = None
            for record in actual.splitlines():
                if record and '\t' not in record:
                    current = record
                row = re.fullmatch(r'(\d+):(\d+)\t(.+)', record)
                if row and current == name:
                    chapter, verse, text = row.groups()
                    key = (int(chapter), int(verse))
                    expected[key] = expected[key]+' '+text if key in expected else text
            assert expected, (tool, name, actual[:300])
            # Query by exact listed name: Kata resolves source spelling aliases.
            if name not in cache:
                dump = run([str(BIN), '--state', folder+'/state.json', '--passage', name, '--dump'], native_env)
                found = parse_dump(dump)
                cache[name] = found
                canonical_books[dump.splitlines()[0]] = found
            assert cache[name][tool] == expected, (tool, name, 'verse text or labels differ', len(expected), len(cache[name][tool]))
            entries.append({'tool': tool, 'name': name, 'verses': len(expected), 'chapters': sorted({c for c, _ in expected})})
        print('PASS', tool, totals[tool], 'complete books', flush=True)
    chapters_verified = 0
    for name, whole in (canonical_books.items() if '--chapters' in sys.argv else []):
        chapters = sorted({c for source in TOOLS for c, _ in whole[source]})
        for chapter in chapters:
            expected = {source: {key: text for key, text in whole[source].items() if key[0] == chapter} for source in TOOLS}
            dump = run([str(BIN), '--state', folder+'/state.json', '--passage', name+':'+str(chapter), '--dump'], native_env)
            assert parse_dump(dump) == expected, (name, chapter, 'chapter query differs from whole book')
            chapters_verified += 1
            if chapters_verified % 50 == 0:
                print('PASS', chapters_verified, 'chapter queries', flush=True)
    assert not (Path(folder)/'state.json').exists()
    print(json.dumps({'source_book_counts': totals, 'source_books_verified': len(entries),
                      'canonical_books_verified': len(canonical_books), 'chapters_verified': chapters_verified,
                      'verses_verified': sum(e['verses'] for e in entries)}, sort_keys=True))
print('PASS every listed source book: complete verse labels/text match CLI output; no progress written')
