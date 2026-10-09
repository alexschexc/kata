"""Real PTY check of EPUB ingest: first-run folder prompt (absolute-path
validation), startup conversion, library listing, the book reader with
chapter picker, search panel and highlighting, saved position, and the
`kata ingest` cache. Uses the two target samples in examplePubs/."""
from pathlib import Path
import fcntl, json, os, pty, re, select, shutil, signal, struct, subprocess, tempfile, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'zig-out/bin/kata')))
SAMPLES = ['The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub',
           'buildingmicroservices2ndedition.epub']
MARK = b'\x1b[48;2;210;178;116m\x1b[38;2;21;25;34m'


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

with tempfile.TemporaryDirectory(prefix='kata-ingest-') as folder:
    tmp = Path(folder)
    home = tmp / 'home'; home.mkdir()
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(tmp / 'config'), XDG_STATE_HOME=str(tmp / 'state'),
               KATA_TEST_STATE=str(tmp / 'state.json'))
    # Fake image viewer: records the path it was asked to open.
    shim = tmp / 'bin'; shim.mkdir()
    (shim / 'xdg-open').write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$KATA_OPENED"\n'); (shim / 'xdg-open').chmod(0o755)
    env['PATH'] = f"{shim}:{env['PATH']}"; env['KATA_OPENED'] = str(tmp / 'opened.txt')
    ui = Screen(env)
    try:
        f = ui.send(b'', 'Ingest folder (kataIngest)')
        assert str(home / 'kataIngest') in text(f), 'suggests ~/kataIngest'
        # A relative path is rejected with a reason, not guessed.
        ui.send(b'\x15relative/path\r', 'path is relative; enter an absolute path')
        ui.send(b'\x15~/kataIngest\r', 'Library folder (kataLibrary)')
        (home / 'kataIngest').mkdir()
        for sample in SAMPLES:  # appear before the library folder is confirmed
            shutil.copy(ROOT / 'examplePubs' / sample, home / 'kataIngest' / sample)
        f = ui.send(b'\r', 'Ingest: 2 converted, 0 failed')
        t = text(f)
        assert 'The Psalter According to the Seventy' in t and 'Building Microservices' in t and 'Ingest folders' in t, t[-1500:]
        saved = json.loads((tmp / 'config/kata/folders.json').read_text())
        assert saved == {'version': 1, 'ingest': str(home / 'kataIngest'), 'library': str(home / 'kataLibrary')}, saved
        assert (home / 'kataLibrary' / SAMPLES[0][:-5]).is_file() and (home / 'kataLibrary' / SAMPLES[1][:-5]).is_file()

        # Psalter: chapter picker → The Second Kathisma; verse gutter and page rule.
        ui.send(b'2\r', 'The Psalter According to the Seventy · cover')
        f = ui.send(b't', 'Choose chapter')
        f = ui.send(b'8\r', 'The Second Kathisma  (8/33)')
        assert '    1 \x1b[38;2;74;87;101m│ ' in text(f), 'verse number gutter'
        ui.send(b'\x1b[1;2C', 'The Third Kathisma  (9/33)')   # Shift+Right
        ui.send(b'H', 'The Second Kathisma  (8/33)')           # H
        f = ui.send(b'\x1b[B', 'line 2/')                      # Down arrow scrolls
        f = ui.send(b'\x1b[A', 'line 1/')                      # Up arrow scrolls
        f = ui.send(b'G', 'line ')
        assert '── page ' in text(f) or 'page ' in text(f)
        # Search, live highlight, panel, cross-chapter jump with panel kept.
        f = ui.send(b'/mercy', '/mercy█')
        f = ui.send(b'\r', 'passages contain “mercy”')
        assert 'Search · “mercy”' in text(f)
        f = ui.send(b'G\r', 'Search · “mercy”')
        assert MARK in f, 'matches highlighted after jump'
        ui.send(b'\x1b', 'j/k or ↑/↓ scroll')
        f = ui.send(b'm', 'Library')

        # Building Microservices: code keeps indentation; reopen restores position.
        ui.send(b'3\r', 'Building Microservices · cover')
        f = ui.send(b'/eurToGbp\r', 'passages contain')
        f = ui.send(b'\r', ' = new Promise((resolve, reject)')
        assert '    //code to fetch latest exchange rate' in text(f), text(f)[-3000:]
        ui.send(b'\x1b', 'j/k or ↑/↓ scroll')
        ui.send(b'x', 'Search closed.')
        # Figures: image lines in the text; i opens the nearest in the viewer.
        f = ui.send(b'/figure 4-9\r', 'passages contain')
        f = ui.send(b'\r', '▣ image · i opens it')
        ui.send(b'\x1b', 'j/k or ↑/↓ scroll')
        ui.send(b'i', 'in your image viewer.')
        for _ in range(50):
            if (tmp / 'opened.txt').exists(): break
            time.sleep(.1)
        opened = (tmp / 'opened.txt').read_text().split()
        assert opened and opened[0].endswith('.png') and '.assets/' in opened[0] and Path(opened[0]).read_bytes()[:4] == b'\x89PNG', opened
        ui.send(b'm', 'Library')
        f = ui.send(b'3\r', 'Chapter 4. Microservice Communication Styles')
        ui.send(b'q', '\x1b[?1049l')
        ui.p.wait(timeout=10)
        assert ui.p.returncode == 0
    finally:
        ui.close()

    # CLI: cache means a second run converts nothing.
    run = lambda *a: subprocess.run([str(BIN), 'ingest', *a], env=env, capture_output=True, text=True, timeout=120)
    second = run()
    assert second.returncode == 0 and '0 converted, 2 unchanged' in second.stdout and '(index current)' in second.stdout, second.stdout + second.stderr
    forced = run('--force')
    assert forced.returncode == 0 and '2 converted' in forced.stdout, forced.stdout
    (home / 'kataIngest' / SAMPLES[1]).unlink()
    orphan = run()
    assert (home / 'kataLibrary' / (SAMPLES[1][:-5] + '.assets')).is_dir()
    assert '1 orphaned' in orphan.stdout and (home / 'kataLibrary' / SAMPLES[1][:-5]).exists(), orphan.stdout
print('PASS first-run folders, startup ingest, library listing, book reader (chapters, verses, pages, code), search panel, saved position, ingest cache/force/orphans')
