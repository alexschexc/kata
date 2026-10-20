#!/usr/bin/env python3
"""First-run folder prompt: Tab opens the folder browser; navigate, create, pick."""
import fcntl, json, os, pty, select, struct, tempfile, termios, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', ROOT / 'rust/target/release/kata'))

with tempfile.TemporaryDirectory(prefix='kata-browse-') as folder:
    tmp = Path(folder)
    (tmp / 'books/epubs').mkdir(parents=True); (tmp / 'books/.hidden').mkdir()
    pid, m = pty.fork()
    if pid == 0:
        os.environ.update(HOME=str(tmp), XDG_CONFIG_HOME=str(tmp / 'cfg'), XDG_STATE_HOME=str(tmp / 'st'), KATA_IMAGES='off')
        os.execv(str(BIN), [str(BIN), '--state', str(tmp / 's.json')])
    fcntl.ioctl(m, termios.TIOCSWINSZ, struct.pack('HHHH', 36, 120, 0, 0))
    data = bytearray()

    def send(keys, until, t=10):
        start = len(data)
        os.write(m, keys)
        end = time.monotonic() + t
        while until.encode() not in data[start:]:
            assert time.monotonic() < end, (until, bytes(data[-500:]))
            if select.select([m], [], [], .05)[0]:
                data.extend(os.read(m, 1 << 20))
        return bytes(data[start:])

    send(b'', 'Tab browses folders')
    # Ingest: clear, type ~/books, Tab → browser at <home>/books; hidden folder not listed.
    frame = send(b'\x15~/books\t', 'Use this folder: ' + str(tmp / 'books'))
    assert b'epubs/' in frame and b'.hidden/' not in frame
    send(b'5\r', 'Use this folder: ' + str(tmp / 'books/epubs'))   # open epubs/
    send(b'1\r', 'Library folder')                                   # use it
    # Library: browse from home, create "kataLibrary" via + New folder, use it.
    send(b'\x15~/\t', 'Use this folder: ' + str(tmp))
    send(b'3\r', 'Name of the new folder')
    send(b'myLibrary\r', 'Use this folder: ' + str(tmp / 'myLibrary'))
    send(b'1\r', 'Library')
    time.sleep(.5)
    found = list(tmp.rglob('folders.json'))
    assert found, sorted(str(x) for x in tmp.rglob('*'))[:20]
    saved = json.loads(found[0].read_text())
    assert saved['ingest'] == str(tmp / 'books/epubs') and saved['library'] == str(tmp / 'myLibrary'), saved
    assert (tmp / 'myLibrary').is_dir()
    os.write(m, b'q'); os.waitpid(pid, 0)
print('PASS folder browser: Tab opens it, hidden folders hidden, navigate, create, pick, saved')
