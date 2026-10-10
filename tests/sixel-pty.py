"""Inline images over a PTY that answers capability queries like foot:
sixel only when DA1 reports attribute 4, image drawn inside the text
column, cropped to the visible rows when scrolled, decodable by libsixel
when available, never emitted for a terminal without sixel, and a burst of
keys produces one frame rather than one per key."""
from pathlib import Path
import fcntl, json, os, pty, re, select, shutil, struct, subprocess, tempfile, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'zig-out/bin/kata')))
EPUB = ROOT / 'examplePubs' / 'buildingmicroservices2ndedition.epub'
CLEAR = b'\x1b[H\x1b[2J'
SIXEL = re.compile(rb'\x1bP0;1;0q"1;1;(\d+);(\d+).*?\x1b\x5c', re.S)
ROWS, COLS = 45, 150


def session(tmp, reply):
    m, s = pty.openpty()
    fcntl.ioctl(s, termios.TIOCSWINSZ, struct.pack('HHHH', ROWS, COLS, 1500, 900))
    env = dict(os.environ, HOME=str(tmp), XDG_CONFIG_HOME=str(tmp / 'cfg'), XDG_STATE_HOME=str(tmp / 'state'))
    p = subprocess.Popen([str(BIN), '--state', str(tmp / 'st.json')], stdin=s, stdout=s, stderr=s,
                         start_new_session=True, env=env, cwd='/tmp')
    data = bytearray()
    answered = [False]

    def pump(t):
        end = time.monotonic() + t
        while time.monotonic() < end:
            if select.select([m], [], [], .05)[0]:
                data.extend(os.read(m, 1 << 20))
                if not answered[0] and b'\x1b[c' in data:
                    os.write(m, reply); answered[0] = True

    def send(keys, until=None, t=15):
        start = len(data)
        os.write(m, keys)
        end = time.monotonic() + t
        while True:
            pump(.1)
            if until is None or until.encode() in data[start:] or time.monotonic() > end:
                break
        pump(.4)
        f = bytes(data)
        return f[f.rfind(CLEAR):], start

    return p, m, s, data, pump, send


def close(p, m, s, send):
    send(b'\x1b'); send(b'q')
    try:
        p.wait(10)
    finally:
        if p.poll() is None:
            p.kill()
        os.close(m); os.close(s)
    assert p.returncode == 0, p.returncode


assert EPUB.exists(), 'sample EPUB required'
with tempfile.TemporaryDirectory(prefix='kata-sixel-') as folder:
    tmp = Path(folder)
    (tmp / 'cfg/kata').mkdir(parents=True); (tmp / 'in').mkdir()
    shutil.copy(EPUB, tmp / 'in')
    (tmp / 'cfg/kata/folders.json').write_text(json.dumps({'version': 1, 'ingest': str(tmp / 'in'), 'library': str(tmp / 'lib')}))

    # foot-like: sixel (DA1 attribute 4) and 10×20 px cells via CSI 16t.
    p, m, s, data, pump, send = session(tmp, b'\x1b[6;20;10t\x1b[?62;4;22c')
    try:
        pump(4)
        send(b'2\r', 'Building Microservices')
        send(b'/figure 4-9\r', 'passages contain')
        frame, _ = send(b'\r', 'Figure 4-9.')
        images = SIXEL.findall(frame)
        assert len(images) == 1, images
        width, height = map(int, images[0])
        assert width <= 100 * 10 and height % 6 == 0, (width, height)
        start = frame.find(b'\x1bP0;1;0q')
        row, col = map(int, re.findall(rb'\x1b\[(\d+);(\d+)H', frame[:start])[-1])
        assert col == 9 and 3 <= row <= ROWS - 2, (row, col)  # text column, inside the body
        assert height <= (ROWS - 2 - row + 1) * 20, 'image must not extend below the body'
        if shutil.which('sixel2png'):
            (tmp / 'f.six').write_bytes(SIXEL.search(frame).group(0))
            subprocess.run(['sixel2png', '-i', str(tmp / 'f.six'), '-o', str(tmp / 'f.png')], check=True)
            assert (tmp / 'f.png').read_bytes()[:4] == b'\x89PNG'
        send(b'\x1b', 'j/k or')
        # Scroll so the image's top leaves the screen: cropped band at row 3.
        frame, _ = send(b'j' * 15)
        cropped = SIXEL.findall(frame)
        assert cropped and int(cropped[0][1]) < height, cropped
        assert re.findall(rb'\x1b\[(\d+);(\d+)H', frame[:frame.find(b'\x1bP0;1;0q')])[-1] == (b'3', b'9')
        # A burst of 30 keys yields far fewer than 30 frames.
        _, start = send(b'k' * 30)
        frames = bytes(data[start:]).count(CLEAR)
        assert 1 <= frames <= 5, frames
    finally:
        close(p, m, s, send)

    # JPEG figures (baseline) are drawn inline too.
    jpg_book = ROOT / 'examplePubs' / 'secretlifeofprograms.epub'
    if jpg_book.exists():
        shutil.copy(jpg_book, tmp / 'in')
        p, m, s, data, pump, send = session(tmp, b'\x1b[6;20;10t\x1b[?62;4;22c')
        try:
            pump(8)
            send(b'3\r', 'Secret Life of Programs')
            send(b'/truth tables for boolean\r', 'passages contain')
            frame, _ = send(b'\r', 'Truth tables')
            assert SIXEL.findall(frame), 'baseline JPEG figure drawn as sixel'
            assert '&#8212;'.encode() not in frame, 'title entities decoded'
        finally:
            close(p, m, s, send)

    # Kitty graphics terminal (Ghostty/kitty): transmit once, place per frame.
    p, m, s, data, pump, send = session(tmp, b'\x1b_Gi=31;OK\x1b\\\x1b[6;20;10t\x1b[?62;22c')
    try:
        pump(4)
        send(b'2\r', 'Building Microservices')
        send(b'/figure 4-9\r', 'passages contain')
        _, at = send(b'\r', 'Figure 4-9.')
        frame = bytes(data[at:])
        assert b'\x1bP' not in bytes(data), 'no sixel when kitty is preferred'
        assert re.search(rb'\x1b_Ga=t,q=2,f=24,i=\d+', bytes(data)), 'image transmitted'
        assert re.search(rb'\x1b_Ga=p,q=2,i=\d+,y=0,', frame), 'placed in the frame'
        send(b'\x1b', 'j/k or')
        sent = bytes(data).count(b'\x1b_Ga=t,')
        _, at = send(b'j' * 15)
        frame = bytes(data[at:])
        # ED2 makes Ghostty delete the transmitted image: never clear that way.
        assert b'\x1b[2J' not in frame, 'kitty frames erase by line, not ED2'
        assert bytes(data).count(b'\x1b_Ga=t,') == sent, 'not re-transmitted when scrolling'
        assert re.search(rb'\x1b_Ga=p,q=2,i=\d+,y=[1-9]\d*,', frame), 'cropped placement after scrolling'
    finally:
        close(p, m, s, send)

    # A terminal without sixel (DA1 lacks 4): placeholder line, no image data.
    p, m, s, data, pump, send = session(tmp, b'\x1b[?62;22c')
    try:
        pump(3)
        send(b'2\r', 'Building Microservices')
        send(b'/figure 4-9\r', 'passages contain')
        frame, _ = send(b'\r', 'Figure 4-9.')
        assert b'\x1bP' not in bytes(data), 'no sixel for a terminal without support'
        assert '▣ image · i opens it'.encode() in frame
    finally:
        close(p, m, s, send)
print('PASS inline sixel + kitty images: placement, crop on scroll, libsixel-decodable, no sixel without support, coalesced key bursts')
