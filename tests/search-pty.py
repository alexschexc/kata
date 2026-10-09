"""Real PTY check of word search: typed prompt, live highlight, results
panel, jumping within and across chapters with the panel kept open."""
from pathlib import Path
import os, re, sys, tempfile

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'zig-out/bin/kata')))
MARK = b'\x1b[48;2;210;178;116m\x1b[38;2;21;25;34m'

import fcntl, json, pty, select, signal, struct, subprocess, termios, time


class Screen:
    def __init__(self, state, env, *args):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 180, 0, 0))
        self.p = subprocess.Popen([str(BIN), '--state', str(state), *args], cwd='/tmp', env=env,
                                  stdin=self.slave, stdout=self.slave, stderr=self.slave, start_new_session=True)
        self.data = bytearray()

    def drain(self, duration=.2):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], .05)
            if ready:
                self.data.extend(os.read(self.master, 1 << 16))

    def frame(self):
        """Latest full frame (each redraw clears the screen)."""
        cut = self.data.rfind(b'\x1b[H\x1b[2J')
        return bytes(self.data[cut:])

    def send(self, keys, text, timeout=20):
        start = len(self.data)
        os.write(self.master, keys)
        end = time.monotonic() + timeout
        while text.encode() not in self.data[start:]:
            assert time.monotonic() < end, (keys, text, bytes(self.data[start:])[-3000:])
            assert self.p.poll() is None, bytes(self.data[-2000:])
            self.drain(.05)
        self.drain(.15)
        return self.frame()

    def close(self):
        try:
            if self.p.poll() is None:
                # Esc leaves the results panel (where q only unfocuses); q quits.
                os.write(self.master, b'\x1b'); self.drain(.3)
                os.write(self.master, b'q')
                self.p.wait(timeout=10)
            assert self.p.returncode == 0, self.p.returncode
            assert termios.tcgetattr(self.slave) == self.original
            assert b'panic' not in self.data
        finally:
            if self.p.poll() is None:
                self.p.kill(); self.p.wait()
            os.close(self.master); os.close(self.slave)


def marked(frame):
    """Texts drawn with the highlight colour."""
    return [m.decode() for m in re.findall(re.escape(MARK) + rb'(.*?)\x1b\[48;2;21;25;34m', frame)]


with tempfile.TemporaryDirectory(prefix='kata-search-') as folder:
    tmp = Path(folder)
    env = dict(os.environ, HOME=str(tmp), XDG_CONFIG_HOME=str(tmp / 'config'), XDG_STATE_HOME=str(tmp / 'state'))
    ui = Screen(tmp / 'progress.json', env, '--passage', 'John:1')
    try:
        ui.send(b'', 'John:1 · free reading')
        # KJV pane is focused: typing shows in the bottom bar and highlights live.
        f = ui.send(b'/', 'searching kjv')
        f = ui.send(b'Wor', '/Wor█')
        assert 'Word' in marked(f), marked(f)
        f = ui.send(b'd', '/Word█')
        assert marked(f).count('Word') >= 3, marked(f)
        f = ui.send(b'\r', 'Search kjv')
        assert re.search(rb'\d+ verses \xc2\xb7 \d+ matches', f), f[-2000:]
        assert 'John 1:1' in f.decode(errors='replace')
        # Next result is still John 1 (verse 14): same chapter, panel stays.
        f = ui.send(b'j\r', 'John 1:14')
        assert 'Search kjv' in f.decode(errors='replace')
        # Jump far away: the G key selects the last result (another book).
        f = ui.send(b'G', '▶')
        last = re.findall(r'▶ ([^\x1b]+?) (\d+):(\d+)', f.decode(errors='replace'))
        assert last, f[-2000:]
        f = ui.send(b'\r', 'Search result opened')
        text = f.decode(errors='replace')
        assert 'Search kjv' in text, 'panel must stay open across chapters'
        assert 'Word' in marked(f) or any('word' in m.lower() for m in marked(f)), marked(f)
        # Tab returns to the reader; n steps to the previous/next results.
        ui.send(b'\t', '/ search · r results')
        ui.send(b'N', '▶')
        # Greek: focus pane 2 and search an accented word without accents.
        ui.send(b'x', 'Search closed.')
        ui.send(b'l', '▶ grb')
        f = ui.send('/λογος'.encode(), '/λογος█')
        f = ui.send(b'\r', 'Search grb')
        assert re.search(rb'Search grb', f)
        f = ui.send(b'\x1b', '/ search · r results')
        # Esc in a prompt cancels and keeps the previous results.
        ui.send(b'/', 'searching grb')
        f = ui.send(b'\x1b', '/ search · r results')
        assert 'Search grb' in f.decode(errors='replace')
        # No matches: a notice, no panel.
        ui.send(b'x', 'Search closed.')
        f = ui.send(b'/zzqqxx\r', 'No matches')
        assert 'Search grb' not in f.decode(errors='replace')
        # Psalms: KJV results jump to the Greek/Latin psalm.
        ui.send(b'h', '▶ kjv')
        f = ui.send(b'/shall not want\r', 'Search kjv')
        assert 'Psalms 22:1 (kjv 23:1)' in f.decode(errors='replace'), f.decode(errors='replace')[-3000:]
        f = ui.send(b'\r', 'Psalms:22 · free reading')
        assert 'want' in ' '.join(marked(f)), marked(f)
    finally:
        ui.close()
print('PASS search prompt, live highlight, results panel, in-chapter and cross-book jumps, Greek accents, Psalms mapping')
