"""Linux PTY smoke test for the kata-term backend (rust/crates/kata-term/examples/term_probe.rs).

Checks, through a real pseudo-terminal: raw mode is entered (keys arrive unechoed, without
Enter), the window size is reported, arrow/Shift-arrow/paste/lone-ESC decode as in input.zig,
SIGTERM is handled (no kill), and the original termios settings are restored on exit.
Usage: python tests/term-pty.py [path/to/term_probe]
"""
from pathlib import Path
import fcntl, os, pty, select, signal, struct, subprocess, sys, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(sys.argv[1] if len(sys.argv) > 1 else ROOT/'rust/target/debug/examples/term_probe')


class Probe:
    def __init__(self):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 31, 97, 0, 0))
        self.before = termios.tcgetattr(self.slave)
        self.p = subprocess.Popen([str(BIN)], stdin=self.slave, stdout=self.slave, stderr=self.slave,
                                  start_new_session=True)
        self.data = bytearray()

    def drain(self, seconds=.3):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([self.master], [], [], .05)[0]:
                self.data.extend(os.read(self.master, 4096))

    def expect(self, text, since=0):
        end = time.monotonic() + 10
        while text.encode() not in self.data[since:]:
            assert time.monotonic() < end, (text, bytes(self.data[since:]))
            self.drain(.05)

    def send(self, keys, expect):
        start = len(self.data)
        os.write(self.master, keys)
        self.expect(expect, start)


probe = Probe()
probe.expect('size 97x31')
raw = termios.tcgetattr(probe.slave)
assert not raw[3] & termios.ICANON and not raw[3] & termios.ECHO, 'raw mode not entered'
probe.send(b'j', 'key j')                      # no Enter needed
assert b'jj' not in probe.data and b'j\r\n' not in probe.data.replace(b'key j\r\n', b''), 'input was echoed'
probe.send(b'\x1b[A', 'key k')                 # Up arrow
probe.send(b'\x1b[1;2C', 'key L')              # Shift+Right
probe.send(b'\x1b[200~qz\x1b[201~j', 'key j')   # paste is quarantined; the trailing j proves it ended
assert b'key q' not in probe.data and b'key z' not in probe.data, 'pasted text leaked as a command'
probe.send(b'\x1b', 'key esc')                 # lone ESC after the idle timeout
fcntl.ioctl(probe.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
probe.send(b'q', 'restored')
probe.p.wait(timeout=5)
after = termios.tcgetattr(probe.slave)
assert after[3] == probe.before[3] and after[0] == probe.before[0], \
    f'termios not restored: iflag {probe.before[0]:#x}->{after[0]:#x}, lflag {probe.before[3]:#x}->{after[3]:#x}'
assert probe.p.returncode == 0, probe.p.returncode

# SIGTERM is caught (flag set, loop ends, terminal restored) rather than killing the process.
probe = Probe()
probe.expect('size 97x31')
os.killpg(probe.p.pid, signal.SIGTERM)
probe.expect('restored')
probe.p.wait(timeout=5)
assert probe.p.returncode == 0, ('SIGTERM exit', probe.p.returncode)
assert termios.tcgetattr(probe.slave)[3] == probe.before[3], 'termios not restored after SIGTERM'
print('PASS kata-term PTY: raw mode, size, key decoding, paste quarantine, lone ESC, SIGTERM, termios restore')
