#!/usr/bin/env python3
"""Library-wide search and Bible Psalms → ingested Psalter (P) in a PTY."""
import fcntl, json, os, pty, select, shutil, struct, tempfile, termios, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', ROOT / 'zig-out/bin/kata'))
PSALTER = ROOT / 'examplePubs/The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub'
OTHER = ROOT / 'examplePubs/thinklikeaprogrammer.epub'
assert PSALTER.exists() and OTHER.exists(), 'sample EPUBs required'

with tempfile.TemporaryDirectory(prefix='kata-library-') as folder:
    tmp = Path(folder)
    (tmp / 'cfg/kata').mkdir(parents=True); (tmp / 'in').mkdir()
    shutil.copy(PSALTER, tmp / 'in'); shutil.copy(OTHER, tmp / 'in')
    (tmp / 'cfg/kata/folders.json').write_text(json.dumps({'version': 1, 'ingest': str(tmp / 'in'), 'library': str(tmp / 'lib')}))
    pid, m = pty.fork()
    if pid == 0:
        os.environ.update(HOME=str(tmp), XDG_CONFIG_HOME=str(tmp / 'cfg'), XDG_STATE_HOME=str(tmp / 'st'), KATA_IMAGES='off')
        os.execv(str(BIN), [str(BIN), '--state', str(tmp / 's.json')])
    fcntl.ioctl(m, termios.TIOCSWINSZ, struct.pack('HHHH', 36, 120, 0, 0))
    data = bytearray()

    def send(keys, until, t=15):
        start = len(data)
        os.write(m, keys)
        end = time.monotonic() + t
        while until.encode() not in data[start:]:
            assert time.monotonic() < end, (until, bytes(data[-600:]))
            if select.select([m], [], [], .05)[0]:
                data.extend(os.read(m, 1 << 20))
        return bytes(data[start:])

    send(b'', 'Search all books')
    # Library search: results from the Psalter, open one, come back to the list.
    send(b'4\r', 'Search all books')
    send(b'mercy\r', 'passages in 1 book ')
    send(b'3\r', 'The First Kathisma')
    send(b'm', 'Search results')
    send(b'q', 'Bible ·')
    # Bible Psalm 3: e switches column 1 to the ingested Psalter (htm),
    # search then follows it; the choice holds on the next psalm; e returns.
    send(b'1\r', 'Reading mode'); send(b'1\r', 'Choose book')
    send(b'19\r', 'Choose chapter'); send(b'3\r', 'starting verse'); send(b'0\r', 'Psalms:3')
    frame = send(b'e', 'htm · Psalms 3')
    assert b'O Lord, why are they multiplied' in frame
    send(b'/salvation\r', 'verses contain')
    send(b'x', 'e edition')
    send(b'L', 'htm · Psalms 4')
    send(b'e', 'kjv · Psalms 4')
    send(b'q', '')
    os.waitpid(pid, 0)
print('PASS library search across books, results reopen; e switches column 1 to the Psalter and back')
