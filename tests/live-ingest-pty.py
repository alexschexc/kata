"""Real PTY check of EPUB ingest: first-run folder prompt (absolute-path
validation), startup conversion, library listing, the book reader with
chapter picker, search panel and highlighting, saved position, and the
`kata ingest` cache. Uses the two target samples in examplePubs/."""
from pathlib import Path
import fcntl, json, os, pty, re, select, shutil, signal, struct, subprocess, tempfile, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'rust/target/release/kata')))
SAMPLES = ['The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub',
           'buildingmicroservices2ndedition.epub']
MARK = b'\x1b[48;2;125;130;217m\x1b[38;2;6;11;30m'


class Screen:
    def __init__(self, env):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 160, 0, 0))
        self.p = subprocess.Popen([str(BIN), '--state', env['KATA_TEST_STATE']], cwd='/tmp', env=env,
                                  stdin=self.slave, stdout=self.slave, stderr=self.slave, start_new_session=True)
        self.data = bytearray()

    def drain(self, duration=.2):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], .05)
            if ready:
                self.data.extend(os.read(self.master, 1 << 16))

    def frame(self):
        return bytes(self.data[self.data.rfind(b'\x1b[H\x1b[2J'):])

    def send(self, keys, text, timeout=30):
        start = len(self.data)
        os.write(self.master, keys)
        end = time.monotonic() + timeout
        while text.encode() not in self.data[start:]:
            assert time.monotonic() < end, (keys, text, bytes(self.data[start:])[-2500:])
            assert self.p.poll() is None, bytes(self.data[-2000:])
            self.drain(.05)
        self.drain(.15)
        return self.frame()

    def close(self):
        try:
            if self.p.poll() is None:
                self.p.send_signal(signal.SIGTERM)
            self.p.wait(timeout=10)
            assert termios.tcgetattr(self.slave) == self.original
            assert b'panic' not in self.data
        finally:
            if self.p.poll() is None:
                self.p.kill(); self.p.wait()
            os.close(self.master); os.close(self.slave)


def text(frame):
    return frame.decode(errors='replace')


for sample in SAMPLES:
    assert (ROOT / 'examplePubs' / sample).exists(), 'sample EPUBs are required in examplePubs/'

with tempfile.TemporaryDirectory(prefix='kata-live-') as folder:
    tmp = Path(folder)
    home = tmp / 'home'; home.mkdir()
    ingest, library = home / 'kataIngest', home / 'kataLibrary'
    ingest.mkdir()
    (tmp / 'config/kata').mkdir(parents=True)
    (tmp / 'config/kata/folders.json').write_text(json.dumps({'version': 1, 'ingest': str(ingest), 'library': str(library)}))
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(tmp / 'config'), XDG_STATE_HOME=str(tmp / 'state'),
               KATA_TEST_STATE=str(tmp / 'state.json'))
    shutil.copy(ROOT / 'examplePubs' / SAMPLES[0], ingest / SAMPLES[0])
    ui = Screen(env)
    try:
        f = ui.send(b'', 'Ingest: 1 converted')
        t = text(f)
        assert 'The Psalter According to the Seventy' in t and 'Building Microservices' not in t, t[-1500:]

        # R1/R4/R5: a new EPUB appears while Kata is open; Ctrl+R converts it in the background.
        shutil.copy(ROOT / 'examplePubs' / SAMPLES[1], ingest / SAMPLES[1])
        f = ui.send(b'\x12', 'Ingest: 1 new or changed, 0 failed')
        # The Library reloads by itself once the run is done.
        end = time.monotonic() + 20
        while 'Building Microservices' not in text(ui.frame()):
            assert time.monotonic() < end, text(ui.frame())[-1500:]
            ui.drain(.1)
        assert (library / SAMPLES[1][:-5]).is_file()

        # R2: nothing new the second time.
        f = ui.send(b'\x12', 'Ingest: nothing new')

        # R1: from inside a reader. Open the Psalter, add a copy, press Ctrl+R, scroll meanwhile (R3).
        ui.send(b'3\r', 'Read · resume'); ui.send(b'1\r', 'The Psalter According to the Seventy · cover')
        shutil.copy(ROOT / 'examplePubs' / SAMPLES[1], ingest / 'Copy of microservices.epub')
        ui.send(b'\x12', 'Ingest')
        f = ui.send(b'L', '(2/33)')   # the reader still answers while the run is going
        end = time.monotonic() + 30
        while 'Ingest: 1 new or changed' not in text(ui.frame()):
            assert time.monotonic() < end, text(ui.frame())[-1500:]
            ui.drain(.1)
        # R5: back in the Library the new book is listed.
        f = ui.send(b'm', 'Library')
        end = time.monotonic() + 5
        while text(ui.frame()).count('Building Microservices') < 2:
            assert time.monotonic() < end, text(ui.frame())[-1500:]
            ui.drain(.1)   # the copy carries the same title, so the book now appears twice
        ui.send(b'q', '\x1b[?1049l')
        ui.p.wait(timeout=10)
        assert ui.p.returncode == 0
    finally:
        ui.close()

    # R7: no folders chosen -> a message, nothing started.
    (tmp / 'config/kata/folders.json').unlink()
    ui = Screen(dict(env, KATA_TEST_STATE=str(tmp / 'state2.json')))
    try:
        ui.send(b'', 'Ingest folder (kataIngest)')
        ui.send(b'\x1b', 'Library')
        f = ui.send(b'\x12', 'Choose the ingest folders first')
        ui.send(b'q', '\x1b[?1049l')
    finally:
        ui.close()
print('PASS live ingest: Ctrl+R in Library and reader, background run, reload, nothing-new, no folders')
