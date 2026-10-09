"""Real-source integration checks; Python is test tooling, not application runtime."""
from pathlib import Path
import fcntl, json, os, pty, select, signal, struct, subprocess, sys, tempfile, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'zig-out/bin/kata')))

class UI:
    def __init__(self, state, env, *args):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 150, 0, 0))
        self.p = subprocess.Popen([str(BIN), '--state', str(state), *args], cwd='/tmp', env=env,
                                  stdin=self.slave, stdout=self.slave, stderr=self.slave, start_new_session=True)
        self.data = bytearray()
    def expect(self, text, start=0):
        deadline = time.monotonic() + 20
        while text.encode() not in self.data[start:]:
            assert time.monotonic() < deadline, (text, self.data[start:].decode(errors='replace')[-2500:])
            ready, _, _ = select.select([self.master], [], [], .1)
            if ready:
                self.data.extend(os.read(self.master, 65536))
    def send(self, keys, text):
        start = len(self.data)
        os.write(self.master, keys)
        self.expect(text, start)
    def close(self):
        failing = sys.exc_info()[0] is not None
        if self.p.poll() is None:
            self.p.send_signal(signal.SIGTERM)
        self.p.wait(timeout=20)
        if not failing:
            assert self.p.returncode == 0, (self.p.returncode, self.data.decode(errors='replace')[-2500:])
            assert self.data.count(b'\x1b[?1049h') == 1
            assert termios.tcgetattr(self.slave) == self.original
        os.close(self.master)
        os.close(self.slave)

def choose_folders(config, root):
    """Pre-answer Kata's first-run ingest folder prompt for this test."""
    (config/'kata').mkdir(parents=True, exist_ok=True)
    (config/'kata'/'folders.json').write_text(json.dumps({'version': 1, 'ingest': str(root/'kataIngest'), 'library': str(root/'kataLibrary')}))

with tempfile.TemporaryDirectory(prefix='kata-full-library-') as folder:
    tmp = Path(folder)
    env = dict(os.environ, XDG_CONFIG_HOME=str(tmp/'config'), XDG_STATE_HOME=str(tmp/'state'))
    choose_folders(tmp/'config', tmp)
    state = tmp/'progress.json'
    subprocess.run([str(BIN), '--state', str(state), '--start-day', '88', '--confirm'], env=env,
                   check=True, capture_output=True, cwd='/tmp')
    original = state.read_bytes()
    for book in ('Genesis', 'Judges (Vaticanus)', '3 Maccabees', 'Odes', '2 Kings'):
        result = subprocess.run([str(BIN), '--state', str(state), '--passage', book+':1', '--dump'],
                                env=env, capture_output=True, text=True, cwd='/tmp')
        assert result.returncode == 0, (book, result.stderr)
        assert book+' 1:1' in result.stdout, (book, result.stdout[:400])
    prologue = subprocess.run([str(BIN), '--state', str(state), '--passage', 'Sirach:0', '--dump'],
                              env=env, capture_output=True, text=True, cwd='/tmp')
    assert prologue.returncode == 0 and 'Sirach 0:' in prologue.stdout, prologue.stderr
    ui = UI(state, env)
    try:
        ui.expect('Library')
        ui.expect('Bible')
        assert b'New Testament' not in ui.data
        ui.send(b'1\r', 'Reading mode')
        ui.send(b'\r', 'Choose book')
        ui.send(b'1\r', 'Choose chapter')
        ui.send(b'1\r', 'Choose starting verse')
        ui.send(b'1\r', 'Genesis:1 · free reading')
        ui.expect('In the beginning')
        ui.send(b'L', 'Genesis:2 · free reading')
        ui.send(b'\x1b[1;2D', 'Genesis:1 · free reading')
        ui.send(b'm', 'Library')
        assert state.read_bytes() == original
        ui.send(b'\r', 'Reading mode')
        ui.send(b'j\r', 'Choose plan')
        ui.send(b'\r', 'Optina · day 88/89')
        ui.send(b'm', 'Library')
        after_plan = state.read_bytes()
        assert json.loads(after_plan)['next_day'] == json.loads(original)['next_day']
        assert json.loads(after_plan)['completed_on'] == json.loads(original)['completed_on']
        ui.send(b'q', '\x1b[?1049l')
        ui.p.wait(timeout=20)
    finally:
        ui.close()
    bookmark = json.loads(Path(str(state)+'.free-selection.json').read_text())
    assert bookmark['section'] == 'Genesis'
    ui = UI(state, env)
    try:
        ui.expect('Library')
        ui.send(b'\r', 'Reading mode')
        ui.send(b'\r', 'Choose book')
        ui.send(b'\r', 'Choose chapter')
        ui.send(b'\r', 'Choose starting verse')
        ui.send(b'0\r', 'Genesis:1 · free reading')
    finally:
        ui.close()
    for book in ('Judges (Vaticanus)', '3 Maccabees', 'Odes', '2 Kings'):
        ui = UI(state, env, '--passage', book+':1')
        try:
            ui.expect(book+':1 · free reading')
            ui.send(b'o', 'Choose book')
            ui.send(b'\r', 'Choose chapter')
            ui.send(b'1\r', 'Choose starting verse')
            ui.send(b'0\r', book+':1 · free reading')
        finally:
            ui.close()
    ui = UI(state, env, '--passage', 'Sirach:0')
    try:
        ui.expect('Sirach:0 · free reading')
        ui.send(b'o', 'Choose book')
        ui.send(b'\r', 'Choose chapter')
        ui.expect('Chapter 0')
        ui.send(b'1\r', 'Choose starting verse')
        ui.send(b'0\r', 'Sirach:0 · free reading')
    finally:
        ui.close()
    assert state.read_bytes() == after_plan
print('PASS full-library startup, Old Testament, source-only books, starting verses, chapter navigation, bookmark restoration, plan preservation, terminal cleanup; binary outside repository')
