"""Real PTY regression/stress checks for inert terminal input (stdlib only)."""
from pathlib import Path
import fcntl, json, os, pty, select, signal, struct, subprocess, sys, tempfile, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'zig-out/bin/kata')))

class UI:
    def __init__(self, state, env, *args):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.resize(30, 150)
        self.p = subprocess.Popen([str(BIN), '--state', str(state), *args], cwd='/tmp', env=env,
                                 stdin=self.slave, stdout=self.slave, stderr=self.slave, start_new_session=True)
        self.data = bytearray()
    def resize(self, rows, columns):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
    def drain(self, duration=.25):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], min(.05, max(0, end-time.monotonic())))
            if ready:
                self.data.extend(os.read(self.master, 65536))
    def expect(self, text, start=0):
        end = time.monotonic() + 20
        while text.encode() not in self.data[start:]:
            assert time.monotonic() < end, (text, self.data[start:][-2000:])
            self.drain(.05)
            if text.encode() not in self.data[start:]:
                assert self.p.poll() is None, (self.p.returncode, self.data[-2000:])
    def send(self, keys, text):
        start = len(self.data)
        os.write(self.master, keys)
        self.expect(text, start)
    def inert(self, keys):
        start = len(self.data)
        os.write(self.master, keys)
        self.drain(1.0)
        assert self.p.poll() is None, ('input unexpectedly exited', keys[:100], self.data[start:][-1000:])
        assert self.data[start:] == b'', ('input dispatched or redrew', keys[:100], self.data[start:][:500])
    def close(self):
        failing = sys.exc_info()[0] is not None
        try:
            if self.p.poll() is None:
                self.p.send_signal(signal.SIGTERM)
            self.p.wait(timeout=10)
            self.drain(.05)
            if not failing:
                assert self.p.returncode == 0, (self.p.returncode, self.data[-2000:])
                assert self.data.count(b'\x1b[?1049h') == 1
                assert b'\x1b[?1049l' in self.data
                assert b'\x1b[?2004l' in self.data
                assert termios.tcgetattr(self.slave) == self.original
                assert b'panic' not in self.data and b'Invalid selection' not in self.data and b'Invalid day' not in self.data
        finally:
            if self.p.poll() is None:
                self.p.kill(); self.p.wait()
            os.close(self.master); os.close(self.slave)

NOISE = (b'\x1b[A\x1b[B\x1b[C\x1b[D\x1bOq\x1bOP\x1b[1;5q\x1b[27;5;121~'
         b'\x1b[200~qmyc123\r\njj[]\x1bOq\x1b[201~'
         b'\x1b]0;qmy\x07\x1bPqmy\x1b\\'
         b'\x00\x01\x02\x05\x06\x7f' # Backspace is tested separately in menus.
         b'\xff\xfe\x80\xc3\xa9\xf0\x9f\x98\x80@!?\x1b[12;\x1b[q\xc3q\xf0cyq')
READER_NOISE = NOISE.replace(b'\x7f', b'')

with tempfile.TemporaryDirectory(prefix='kata-input-') as folder:
    tmp = Path(folder)
    env = dict(os.environ, HOME=str(tmp), XDG_CONFIG_HOME=str(tmp/'config'), XDG_STATE_HOME=str(tmp/'state'))
    state = tmp/'progress.json'
    ui = UI(state, env, '--passage', 'Genesis:2')
    try:
        ui.expect('Genesis:2 · free reading')
        assert b'\x1b[?2004h' in ui.data, 'terminal must request bracketed paste framing'
        ui.inert(b'\x1b[A')  # RED on old reader: '[' opens the previous chapter.
        ui.inert(b'\x1bOq')  # Old SS3 keypad payload quits.
        ui.inert(READER_NOISE * 100)
        ui.inert(b'\x1b[' + b'1;' * 10000 + b'q')
        ui.inert(b'\x1b['); ui.inert(b'@')  # Truncated CSI times out safely.
        for part in (b'\x1b', b'[', b'1;', b'5', b'q'):
            os.write(ui.master, part); ui.drain(.01)
        ui.drain(.2)
        assert ui.p.poll() is None
        for rows, cols in ((0, 0), (1, 1), (2, 2)):
            ui.resize(rows, cols); ui.drain(.2)
            assert ui.p.poll() is None
        ui.expect('enlarge the terminal')
        ui.inert(READER_NOISE)
        ui.resize(30, 150); ui.expect('Genesis:2 · free reading', len(ui.data))
        ui.send(b']', 'Genesis:3 · free reading')
        ui.send(b'[', 'Genesis:2 · free reading')
        ui.send(b'm', 'Library')
        ui.resize(2, 2); ui.expect('Enlarge terminal')
        ui.inert(READER_NOISE)
        start = len(ui.data)
        ui.resize(30, 150); ui.expect('Library', start)
        ui.inert(READER_NOISE)
        ui.send(b'9'*100 + b'\r', 'Selection:')
        ui.drain(); assert b'Invalid selection' not in ui.data
        ui.send(b'\x7f'*20 + b'1\r', 'Reading mode')
        ui.inert(READER_NOISE)
        ui.send(b'j\r', 'Choose plan')
        ui.inert(READER_NOISE)
        ui.send(b'\r', 'Optina · day 1/89')
        ui.send(b'd', 'Choose plan day')
        ui.inert(READER_NOISE)
        ui.send(b'\r', 'Confirm reading position')
        ui.inert(READER_NOISE)
        ui.send(b'n', 'Choose plan day')
        ui.send(b'\x1b', 'Optina · day 1/89')
        baseline = state.read_bytes() if state.exists() else None
        ui.send(b'c', "Mark today's assignment complete?")
        ui.inert(READER_NOISE)
        assert (state.read_bytes() if state.exists() else None) == baseline
        ui.send(b'n', 'c complete day')
        ui.send(b'c', "Mark today's assignment complete?")
        ui.inert(READER_NOISE)
        ui.send(b'y', 'Completed and saved.')
        ui.send(b'q', '\x1b[?1049l'); ui.p.wait(timeout=10)
    finally:
        ui.close()
    saved = json.loads(state.read_text())
    assert saved['next_day'] == 1 and saved['completed_on'] > 0
    for exit_kind in ('ctrl-c', 'signal-in-unclosed-paste'):
        ui = UI(state, env, '--passage', 'Genesis:1')
        try:
            ui.expect('Genesis:1 · free reading')
            if exit_kind == 'ctrl-c':
                ui.send(b'\x03', '\x1b[?1049l')
            else:
                ui.inert(b'\x1b[200~qcy\r\n')
                ui.inert(b'q')  # Idle timeout must not release a truncated paste.
                ui.p.send_signal(signal.SIGTERM)
                ui.expect('\x1b[?1049l')
            ui.p.wait(timeout=10)
        finally:
            ui.close()
print('PASS inert CSI/SS3/paste/control/Unicode, fragmented/overlong input, resize, free/plan/menu/confirmation, progress and termios cleanup')
