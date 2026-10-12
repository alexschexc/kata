#!/usr/bin/env python3
"""Library QoL: Ethereal palette, standalone Reading plans menu, `/` filter by
title or author, and removing plans (built-in hidden, user file deleted)."""
import fcntl, json, os, pty, select, struct, tempfile, termios, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', ROOT / 'zig-out/bin/kata'))
SAMPLES = ['thinklikeaprogrammer.epub', 'buildingmicroservices2ndedition.epub']

PLAN = {'name': 'Throwaway', 'repeat': True, 'streams': [{'books': [{'name': 'John', 'chapters': 21}]}],
        'phases': [{'days': 21, 'rates': [1]}]}


def launch(tmp):
    pid, m = pty.fork()
    if pid == 0:
        os.environ.update(HOME=str(tmp), XDG_CONFIG_HOME=str(tmp / 'cfg'), XDG_STATE_HOME=str(tmp / 'st'), KATA_IMAGES='off')
        os.execv(str(BIN), [str(BIN), '--state', str(tmp / 's.json')])
    fcntl.ioctl(m, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 140, 0, 0))
    data = bytearray()

    def send(keys, until, t=15):
        start = len(data)
        os.write(m, keys)
        end = time.monotonic() + t
        while until.encode() not in data[start:]:
            assert time.monotonic() < end, (until, bytes(data[-900:]))
            if select.select([m], [], [], .05)[0]:
                data.extend(os.read(m, 1 << 20))
        return bytes(data[start:])

    def quit_():
        os.write(m, b'\x03')
        time.sleep(.3)
        try:
            os.kill(pid, 9)
        except ProcessLookupError:
            pass
        os.waitpid(pid, 0)
    return send, quit_, data


with tempfile.TemporaryDirectory(prefix='kata-menuqol-') as folder:
    tmp = Path(folder)
    (tmp / 'cfg/kata/plans').mkdir(parents=True)
    (tmp / 'in').mkdir()
    for name in SAMPLES:
        (tmp / 'in' / name).write_bytes((ROOT / 'examplePubs' / name).read_bytes())
    (tmp / 'cfg/kata/folders.json').write_text(json.dumps({'version': 1, 'ingest': str(tmp / 'in'), 'library': str(tmp / 'lib')}))
    user_plan = tmp / 'cfg/kata/plans/throwaway.json'
    user_plan.write_text(json.dumps(PLAN))

    send, quit_, data = launch(tmp)
    try:
        library = send(b'', 'Search all books', 60)
        # Ethereal palette: page background #060B1E, text #ffcead, accent #7d82d9.
        assert b'\x1b[48;2;6;11;30m' in library and b'\x1b[38;2;255;206;173m' in library and b'\x1b[38;2;125;130;217m' in library
        assert b'38;2;210;178;116' not in library and b'48;2;21;25;34' not in library, 'old palette still drawn'
        # Reading plans is its own Library row, directly under the Bible.
        assert library.find(b'2. Reading plans') > library.find(b'1. Bible') > 0, library[-1500:]

        # Bible keeps its own menu: free reading, its plans (with what they cover), build.
        frame = send(b'1\r', 'Build a new plan')
        assert b'Free reading' in frame and b'Plan \xc2\xb7 Optina' in frame and b'Matthew\xe2\x80\x93John' in frame, frame[-1500:]
        send(b'\x1b', 'Search all books')

        # `/` filters by title or author; descriptions are not searched.
        frame = send(b'/sam new\r', 'Filter: sam new')
        assert b'Building Microservices' in frame[-2500:] and b'1 of ' in frame, frame[-1500:]
        assert b'Think Like a Programmer' not in frame[-1500:]
        # Esc clears the filter; the whole list returns.
        frame = send(b'\x1b', 'Think Like a Programmer')
        # A filter with no hits says so.
        send(b'/zzzz', 'No matches')
        send(b'\x1b', 'Think Like a Programmer')
        # Number keys still work while unfiltered, and filtering then opening a row works.
        send(b'/microservices\r', 'Building Microservices')
        frame = send(b'\r', 'Read · resume')
        send(b'\x1b', 'Search all books')

        # Reading plans menu: built-in plans plus the one in the config folder.
        frame = send(b'2\r', 'Throwaway')
        assert b'Optina' in frame and b'Gospels' in frame and b'Build a new plan' in frame and b'Matthew\xe2\x80\x93John' in frame, frame[-1500:]
        # Remove the user plan: confirmation first, any other key cancels.
        send(b'x', 'Remove "')
        send(b'n', 'Throwaway')
        assert user_plan.exists()
        # Select the Throwaway row (last plan, just above Build) and confirm removal.
        # Move to the user plan (last plan row) with G then k (last row is Build).
        send(b'G', 'Build a new plan')
        send(b'k', 'Throwaway')
        send(b'x', 'Remove "Throwaway')
        frame = send(b'y', 'Removed "Throwaway"')
        assert not user_plan.exists(), 'plan file was not deleted'
        assert b'3. Throwaway' not in frame[-1500:]
        # A built-in plan is hidden, not deleted, and stays hidden after a restart.
        send(b'x', 'Remove "Optina')
        send(b'y', 'Removed "Optina"')
        hidden = Path(str(tmp / 's.json') + '.hidden-plans').read_text()
        assert 'builtin:optina' in hidden, hidden
        send(b'\x1b', 'Search all books')
    finally:
        quit_()

    send, quit_, data = launch(tmp)
    try:
        send(b'', 'Search all books', 60)
        frame = send(b'2\r', 'Build a new plan')
        assert b'Gospels' in frame and b'Optina' not in frame[-1500:] and b'Throwaway' not in frame, frame[-1500:]
        # The last remaining plan cannot be removed.
        send(b'x', 'Remove "Gospels')
        send(b'y', 'Keep at least one plan')
    finally:
        quit_()
print('PASS ethereal palette, standalone Reading plans menu, / filter, plan removal (hide built-in, delete user file, keep last)')
