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

def parse_dump(dump, rows=None):
    """Map each source's own chapter:verse label to its text. A pane printed as
    `kjv (c:v): text` (KJV Psalms in Greek/Latin psalm order) keeps its own
    label. `rows`, when given, records the aligned row chapter for each label."""
    found = {source: {} for source in TOOLS}
    key = None
    for record in dump.splitlines():
        header = re.fullmatch(r'.+ (\d+):(\d+)', record)
        if header:
            key = tuple(map(int, header.groups()))
        line = re.fullmatch(r'(kjv|grb|vul)(?: \((\d+):(\d+)\))?: (.*)', record)
        if line and key is not None:
            source, c, v, text = line.groups()
            own = (int(c), int(v)) if c else key
            if text != '[not present under this verse label]':
                found[source][own] = text
                if rows is not None:
                    rows[source][own] = key[0]
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
                row_chapters = {source: {} for source in TOOLS}
                found = parse_dump(dump, row_chapters)
                cache[name] = found
                canonical_books[dump.splitlines()[0]] = (found, row_chapters)
            assert cache[name][tool] == expected, (tool, name, 'verse text or labels differ', len(expected), len(cache[name][tool]))
            entries.append({'tool': tool, 'name': name, 'verses': len(expected), 'chapters': sorted({c for c, _ in expected})})
        print('PASS', tool, totals[tool], 'complete books', flush=True)
    chapters_verified = 0
    for name, (whole, row_chapters) in (canonical_books.items() if '--chapters' in sys.argv else []):
        # Chapter queries use the aligned (Greek/Latin for Psalms) chapter.
        chapters = sorted({c for source in TOOLS for c in row_chapters[source].values()})
        for chapter in chapters:
            expected = {source: {key: text for key, text in whole[source].items() if row_chapters[source][key] == chapter} for source in TOOLS}
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
