#!/usr/bin/env python3
"""Theme switcher in a PTY: Library -> Theme picks Aetheria, the palette changes, the choice is
saved and applied on the next start; KATA_THEME overrides it."""
import fcntl, os, pty, select, struct, subprocess, sys, tempfile, termios, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = os.environ.get('KATA_BIN', str(ROOT / 'rust/target/release/kata'))
home = tempfile.mkdtemp()
PAPER = {'ethereal': b'48;2;6;11;30m', 'aetheria': b'48;2;14;9;29m', 'sedate': b'48;2;28;29;33m'}


class Session:
    def __init__(self, **extra):
        env = dict(os.environ, HOME=home, XDG_STATE_HOME=home + '/s', XDG_CONFIG_HOME=home + '/c',
                   TERM='xterm-256color', COLORTERM='truecolor', **extra)
        env.pop('TERM_PROGRAM', None)
        env.pop('KATA_THEME', None) if 'KATA_THEME' not in extra else None
        self.m, s = pty.openpty()
        fcntl.ioctl(s, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
        self.p = subprocess.Popen([BIN], stdin=s, stdout=s, stderr=s, env=env, start_new_session=True)
        self.data = b''

    def read(self, seconds=1.0):
        end = time.time() + seconds
        while time.time() < end:
            if select.select([self.m], [], [], .05)[0]:
                self.data += os.read(self.m, 65536)

    def send(self, keys, expect=None):
        start = len(self.data)
        os.write(self.m, keys)
        end = time.time() + 10
        while expect and expect not in self.data[start:]:
            assert time.time() < end, (expect, self.data[start:][-300:])
            self.read(.1)
        self.read(.3)
        return self.data[start:]

    def close(self):
        self.p.kill()
        self.p.wait()


s = Session()
s.read(2)
assert PAPER['ethereal'] in s.data, 'default palette is Ethereal'
for _ in range(2):                               # first run: accept the two default folders
    if b'Esc cancel' in s.data[-400:]:
        s.send(b'\r')
        s.read(1)
s.send(b'/', b'Type to filter')
s.send(b'Theme\r', b'1 of')                      # filter to the Theme row
s.send(b'\r', b'Enter applies')                  # open it
frame = s.send(b'j', b'Aetheria')
s.send(b'\r', 'Theme · Aetheria'.encode())
assert PAPER['aetheria'] in s.data[-6000:], 'palette switched to Aetheria'
s.send(b'q')
s.close()
found = list(Path(home).rglob('*.theme'))
assert len(found) == 1, found
saved = found[0].read_text()
assert saved == 'aetheria', saved

s = Session()                                    # next start applies the saved theme
s.read(2)
assert PAPER['aetheria'] in s.data and PAPER['ethereal'] not in s.data, 'saved theme applied'
s.close()

s = Session(KATA_THEME='sedate')                 # environment wins for one run
s.read(2)
assert PAPER['sedate'] in s.data and PAPER['aetheria'] not in s.data, 'KATA_THEME overrides'
s.close()
print('PASS theme switcher: menu, preview-free apply, saved, reloaded, KATA_THEME override')
