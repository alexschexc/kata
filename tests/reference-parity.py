"""Compare bundled reference selection with the original installed applets."""
from pathlib import Path
import json, os, re, subprocess, tempfile

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT/'zig-out/bin/kata')))
TOOLS = ('kjv', 'grb', 'vul')
metadata = json.loads((ROOT/'tests/book_discovery.json').read_text())
CASES = ('John:1', 'John:1:1', 'John:1:1,3,5', 'John:1:1-3', 'John:1-2',
         'John:1:50-2:3', 'Genesis:49:33-50:3', 'Psalms:150-151',
         'Judges (Vaticanus):1:1-3', 'Odes:1:1-3', 'Philippians:1:1-3',
         'Psalms:9', 'Psalms:22', 'Psalms:113', 'Psalms:114', 'Psalms:115', 'Psalms:146', 'Psalms:147')
# Kata shows KJV Psalms under Greek/Latin psalm numbers; the KJV applet uses
# Hebrew numbering. These are the KJV selections each Greek/Latin psalm holds.
KJV_PSALMS = {('kjv', 'Psalms:9'): '9-10', ('kjv', 'Psalms:22'): '23', ('kjv', 'Psalms:113'): '114-115',
              ('kjv', 'Psalms:114'): '116:1-9', ('kjv', 'Psalms:115'): '116:10-19',
              ('kjv', 'Psalms:146'): '147:1-11', ('kjv', 'Psalms:147'): '147:12-20'}

def original_rows(text, name):
    result = {}
    selected = False
    for line in text.splitlines():
        if line and '\t' not in line:
            selected = line == name
        match = re.fullmatch(r'(\d+):(\d+)\t(.+)', line)
        if selected and match:
            c, v, words = match.groups()
            key = (int(c), int(v))
            result[key] = result[key]+' '+words if key in result else words
    return result

def dumped_rows(text):
    result = {tool: {} for tool in TOOLS}
    key = None
    for line in text.splitlines():
        match = re.fullmatch(r'.+ (\d+):(\d+)', line)
        if match:
            key = tuple(map(int, match.groups()))
        own = re.fullmatch(r'(kjv|grb|vul)(?: \((\d+):(\d+)\))?: (.*)', line)
        if key is not None and own:
            tool, c, v, words = own.groups()
            if words != '[not present under this verse label]':
                result[tool][(int(c), int(v)) if c else key] = words
    return result

with tempfile.TemporaryDirectory(prefix='kata-reference-parity-') as folder:
    env = dict(os.environ, XDG_STATE_HOME=folder, XDG_CONFIG_HOME=folder)
    for passage in CASES:
        book, suffix = passage.split(':', 1)
        expected = {}
        for tool in TOOLS:
            record = next((row for row in metadata if row['tool'] == tool and row['canonical'] == book), None)
            if record is None:
                expected[tool] = {}
                continue
            query = (record['name'] if book == 'Philippians' else record['alias'])+':'+KJV_PSALMS.get((tool, passage), suffix)
            raw = subprocess.run([tool, '-W', query], capture_output=True, text=True, env=env, timeout=30)
            assert raw.returncode == 0 and not raw.stderr, (tool, passage, raw.stderr)
            expected[tool] = original_rows(raw.stdout, record['name'])
        native = subprocess.run([str(BIN), '--passage', passage, '--dump'],
                                env=dict(env, PATH=folder), cwd=folder, capture_output=True, text=True, timeout=30)
        assert native.returncode == 0, (passage, native.stderr)
        assert dumped_rows(native.stdout) == expected, (passage, 'native reference text differs from applet')
        print('PASS', passage)
    for passage in ('John::1', 'John:1,,2', 'John:1-', 'John:9999999999999999999999999999',
                    'John:1:3-1', 'John:2-1', 'John:1:1-2:3:4'):
        result = subprocess.run([str(BIN), '--passage', passage, '--dump'], env=dict(env, PATH=folder),
                                cwd=folder, capture_output=True, text=True, timeout=30)
        assert result.returncode != 0 and 'panic' not in result.stderr.lower(), passage
print('PASS reference lists/ranges/cross-chapter selection and malformed references without applets')
