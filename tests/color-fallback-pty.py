"""PTY check for the 256-colour fallback (macOS Terminal.app has no 24-bit colour).

Runs the real TUI in a pseudo-terminal and inspects the bytes it writes.
  - TERM_PROGRAM=Apple_Terminal, no COLORTERM: no 38;2 / 48;2 (truecolor) sequences.
  - Same, but KATA_COLORS=truecolor: truecolor is emitted (proves the probe can see it).
  - Other terminal: truecolor is emitted.
  - KATA_COLORS=256 forces the fallback anywhere.
Usage: python tests/color-fallback-pty.py   (KATA_BIN overrides the binary)
POSIX only (needs pty); Windows has no equivalent fallback.
"""
from pathlib import Path
import fcntl, os, pty, re, select, signal, struct, subprocess, sys, tempfile, termios, time

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'rust/target/release/kata')))
TRUECOLOR = re.compile(rb'\x1b\[[0-9;]*\b[34]8;2;')


def capture(extra_env, seconds=1.5):
    with tempfile.TemporaryDirectory(prefix='kata-color-') as directory:
        env = {k: v for k, v in os.environ.items()
               if k not in ('COLORTERM', 'TERM_PROGRAM', 'KATA_COLORS')}
        env.update(HOME=directory, TERM='xterm-256color', **extra_env)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
        proc = subprocess.Popen([str(BIN), '--state', str(Path(directory) / 'progress.json')],
                                cwd=directory, env=env, stdin=slave, stdout=slave, stderr=slave,
                                start_new_session=True)
        data = bytearray()
        end = time.monotonic() + seconds
        try:
            while time.monotonic() < end:
                ready, _, _ = select.select([master], [], [], .05)
                if ready:
                    try:
                        data.extend(os.read(master, 65536))
                    except OSError:
                        break
            assert proc.poll() is None, ('TUI exited early', proc.returncode, bytes(data[-500:]))
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
            os.close(master)
            os.close(slave)
        return bytes(data)


def has_truecolor(data):
    return TRUECOLOR.search(data) is not None


def main():
    assert BIN.exists(), f'binary not found: {BIN}'
    apple = capture({'TERM_PROGRAM': 'Apple_Terminal'})
    assert apple, 'no output captured'
    assert not has_truecolor(apple), ('Apple_Terminal got truecolor', TRUECOLOR.search(apple))
    assert b'\x1b[38;5;' in apple or b'\x1b[48;5;' in apple, 'no 256-colour sequences seen'
    print('ok  Apple_Terminal: no 38;2/48;2, 256-colour sequences present')

    assert has_truecolor(capture({'TERM_PROGRAM': 'Apple_Terminal', 'KATA_COLORS': 'truecolor'}))
    print('ok  KATA_COLORS=truecolor overrides detection')

    assert has_truecolor(capture({'TERM_PROGRAM': 'Apple_Terminal', 'COLORTERM': 'truecolor'}))
    print('ok  Apple_Terminal with COLORTERM=truecolor keeps truecolor')

    assert has_truecolor(capture({'TERM_PROGRAM': 'ghostty'}))
    print('ok  other terminal keeps truecolor')

    assert not has_truecolor(capture({'KATA_COLORS': '256'}))
    print('ok  KATA_COLORS=256 forces fallback')


if __name__ == '__main__':
    main()
