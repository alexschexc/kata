#!/usr/bin/env python3
"""Reading plans for library books: Psalter by stasis from the book's menu,
end-of-reading marker, completing a day; a mixed Bible + book plan."""
import fcntl, json, os, pty, select, shutil, struct, tempfile, termios, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', ROOT / 'zig-out/bin/kata'))
PSALTER = ROOT / 'examplePubs/The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub'
BMS = ROOT / 'examplePubs/buildingmicroservices2ndedition.epub'
assert PSALTER.exists() and BMS.exists(), 'sample EPUBs required'

with tempfile.TemporaryDirectory(prefix='kata-bookplan-') as folder:
    tmp = Path(folder)
    (tmp / 'cfg/kata').mkdir(parents=True); (tmp / 'in').mkdir()
    shutil.copy(PSALTER, tmp / 'in'); shutil.copy(BMS, tmp / 'in')
    (tmp / 'cfg/kata/folders.json').write_text(json.dumps({'version': 1, 'ingest': str(tmp / 'in'), 'library': str(tmp / 'lib')}))
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
            assert time.monotonic() < end, (until, bytes(data[-700:]))
            if select.select([m], [], [], .05)[0]:
                data.extend(os.read(m, 1 << 20))
        return bytes(data[start:])

    menu = send(b'', 'Search all books')
    psalter_row = b'3' if menu.find(b'The Psalter') < menu.find(b'Building Microservices') else b'4'
    # Psalter: book menu → build → defaults to stases, 60 days.
    send(psalter_row + b'\r', 'Build a reading plan for this book')
    send(b'2\r', 'Name the plan')
    frame = send(b'\r', 'Build reading plan')
    assert b'60 stases' in frame and b'Day 1: The First Kathisma' in frame, frame[-1200:]
    send(b'1\r', 'Day 1/60')
    send(b'1\r', 'Stasis 1')
    frame = send(b'\x04\x04\x04', "end of today's reading")
    # c inside the book marks the day complete (any other key cancels first).
    send(b'c', "Mark today's reading complete?"); send(b'n', 'Not marked complete')
    send(b'c', "Mark today's reading complete?"); send(b'y', 'Completed and saved')
    send(b'c', "Mark today's reading complete?"); send(b'y', 'Already complete today')
    send(b'm', 'Completed today')
    send(b'4\r', 'Search all books')
    # The book menu now lists the plan.
    send(psalter_row + b'\r', 'Reading plan · The Psalter')
    send(b'\x1b', 'Search all books')
    # Mixed plan: Gospels stream + Building Microservices by section.
    send(b'2\r', 'Reading plans')
    send(b'4\r', 'Name the plan')   # optina, gospels, the new Psalter plan, then ＋ Build
    send(b'Gospels and microservices\r', 'Add Bible books')
    send(b'1\r', 'Matthew'); send(b'\x1b', 'Stream 1'); send(b'\x1b', 'Build reading plan')
    send(b'6\r', 'Choose library book'); time.sleep(.5)
    while select.select([m], [], [], .1)[0]: data.extend(os.read(m, 1 << 20))
    options = bytes(data[-3000:])
    choice = b'1' if options.find(b'1. Building Microservices') >= 0 else b'2'
    frame = send(choice + b'\r', 'Stream 2: Building Microservices')
    assert b'Day 1: Matthew 1, Chapter 1. What Are Microservices?' in frame, frame[-1500:]
    send(b'1\r', 'Day 1/159')
    plans = sorted(p.name for p in (tmp / 'cfg/kata/plans').glob('*.json'))
    assert plans == ['gospels-and-microservices.json', 'the-psalter-according-to-the-seventy.json'], plans
    mixed = json.loads((tmp / 'cfg/kata/plans/gospels-and-microservices.json').read_text())
    assert mixed['streams'][1]['library'] == 'buildingmicroservices2ndedition' and mixed['streams'][1]['unit'] == 'section'
    send(b'1\r', 'Bible · Matthew 1')
    send(b'm', 'Day 1/159')
    send(b'2\r', 'Plan reading: Chapter 1. What Are Microservices?')
    send(b'm', 'Day 1/159')
    send(b'\x1b', 'Search all books')   # Back to Library
    os.write(m, b'q')
    end = time.monotonic() + 5
    while time.monotonic() < end and os.waitpid(pid, os.WNOHANG) == (0, 0):
        if select.select([m], [], [], .05)[0]:
            try: data.extend(os.read(m, 1 << 20))
            except OSError: break
    else:
        pass
    if os.waitpid(pid, os.WNOHANG) == (0, 0):
        os.kill(pid, 9); os.waitpid(pid, 0); raise AssertionError('kata did not quit from the Library')
print('PASS book plans: Psalter by stasis from the book menu, end marker, completion; mixed Bible + book plan')
