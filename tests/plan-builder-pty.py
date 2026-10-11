#!/usr/bin/env python3
"""In-app plan builder: two streams, 30 days, saved, selected, day 1 opens."""
import fcntl, json, os, pty, select, struct, tempfile, termios, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', ROOT / 'zig-out/bin/kata'))

with tempfile.TemporaryDirectory(prefix='kata-planbuild-') as folder:
    tmp = Path(folder)
    (tmp / 'cfg/kata').mkdir(parents=True)
    (tmp / 'cfg/kata/folders.json').write_text(json.dumps({'version': 1, 'ingest': str(tmp / 'in'), 'library': str(tmp / 'lib')}))
    pid, m = pty.fork()
    if pid == 0:
        os.environ.update(HOME=str(tmp), XDG_CONFIG_HOME=str(tmp / 'cfg'), XDG_STATE_HOME=str(tmp / 'st'), KATA_IMAGES='off')
        os.execv(str(BIN), [str(BIN), '--state', str(tmp / 's.json')])
    fcntl.ioctl(m, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 140, 0, 0))
    data = bytearray()

    def send(keys, until, t=10):
        start = len(data)
        os.write(m, keys)
        end = time.monotonic() + t
        while until.encode() not in data[start:]:
            assert time.monotonic() < end, (until, bytes(data[-700:]))
            if select.select([m], [], [], .05)[0]:
                data.extend(os.read(m, 1 << 20))
        return bytes(data[start:])

    send(b'', 'Library')
    send(b'1\r', 'Reading mode'); send(b'2\r', 'Choose plan')
    send(b'3\r', 'Name the plan')                       # 2 built-in plans, then ＋ Build
    send(b'Gospels and Psalms\r', 'Add Bible books')
    send(b'1\r', 'Matthew'); send(b'\x1b', 'Stream 1')  # preset: Gospels
    send(b'\x1b', 'Build reading plan')
    send(b'5\r', 'Add Bible books')                           # ＋ Add a reading stream
    send(b'6\r', 'Psalms'); send(b'\x1b', 'Stream 2'); send(b'\x1b', 'Build reading plan')
    frame = send(b'3\r', 'Number of days')
    frame = send(b'30\r', 'Length: 30 days')
    assert b'Day 1: Matthew 1, Matthew 2, Matthew 3, Psalms 1' in frame, frame[-1500:]
    send(b'1\r', 'and selected')
    saved = list((tmp / 'cfg/kata/plans').glob('*.json'))
    assert [p.name for p in saved] == ['gospels-and-psalms.json'], saved
    plan = json.loads(saved[0].read_text())
    assert plan['name'] == 'Gospels and Psalms' and sum(p['days'] for p in plan['phases']) == 30
    assert b'Gospels and Psalms' in bytes(data[-4000:])
    os.write(m, b'q'); os.waitpid(pid, 0)
print('PASS plan builder: presets, two streams, custom length, saved, selected')
