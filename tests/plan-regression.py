"""Temporary real-terminal integration checks for in-app plan controls."""
from pathlib import Path
import fcntl, json, os, pty, select, signal, struct, subprocess, tempfile, termios, time, sys
ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get('KATA_BIN', str(ROOT / 'zig-out/bin/kata')))
class UI:
    def __init__(self, state, env, cwd=ROOT):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 150, 0, 0))
        self.p = subprocess.Popen([str(BIN), '--state', str(state)], cwd=cwd, env=env, stdin=self.slave, stdout=self.slave, stderr=self.slave, start_new_session=True)
        self.data = bytearray()
        self.expect('Library')
        self.send(b'\r', 'Reading mode')
        self.send(b'j\r', 'Choose plan')
        offset = len(self.data)
        os.write(self.master, b'\r')
        deadline = time.monotonic() + 15
        while b'm library' not in self.data[offset:] and b'Plan complete.' not in self.data[offset:]:
            self.read()
            assert self.p.poll() is None
            assert time.monotonic() < deadline, 'Expected plan reader after startup menus'
    def read(self):
        if select.select([self.master], [], [], .05)[0]: self.data.extend(os.read(self.master, 65536))
    def expect(self, text, start=0):
        end = time.monotonic() + 8
        while text.encode() not in self.data[start:]:
            self.read()
            assert self.p.poll() is None, self.data.decode(errors='replace')
            assert time.monotonic() < end, ('missing', text, self.data[start:].decode(errors='replace'))
    def send(self, keys, text=None):
        start = len(self.data)
        os.write(self.master, keys)
        if text: self.expect(text, start)
        else:
            for _ in range(3): self.read()
    def close(self):
        failed = sys.exc_info()[0] is not None
        if self.p.poll() is None:
            self.p.send_signal(signal.SIGTERM)
            self.p.wait(timeout=10)
        if not failed:
            assert self.p.returncode == 0, self.data[-2000:].decode(errors='replace')
            assert self.data.count(b'\x1b[?1049h') == 1, 'Switching sessions must stay on the application screen'
        assert termios.tcgetattr(self.slave) == self.original
        os.close(self.master); os.close(self.slave)
    def resize(self, columns, rows):
        offset = len(self.data)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        self.expect('enlarge the terminal', offset)
def choose_folders(config, root):
    """Pre-answer Kata's first-run ingest folder prompt for this test."""
    (config/'kata').mkdir(parents=True, exist_ok=True)
    (config/'kata'/'folders.json').write_text(json.dumps({'version': 1, 'ingest': str(root/'kataIngest'), 'library': str(root/'kataLibrary')}))

with tempfile.TemporaryDirectory(prefix='.prototype-inapp-test-', dir=ROOT) as folder:
    tmp = Path(folder); state = tmp/'state.json'
    plans = tmp/'config/kata/plans'; plans.mkdir(parents=True)
    short = {'name':'Short custom', 'repeat':False, 'streams':[{'books':[{'name':'John','chapters':2}]}], 'phases':[{'days':2,'rates':[1]}]}
    (plans/'short.json').write_text(json.dumps(short))
    (plans/'invalid.json').write_text('not JSON')
    env = os.environ | {'XDG_CONFIG_HOME':str(tmp/'config')}
    choose_folders(tmp/'config', tmp)
    ui = UI(state, env)
    try:
        ui.send(b'p', 'Choose plan')
        ui.expect('Optina'); ui.expect('Gospels - one chapter a day'); ui.expect('Short custom')
        ui.send(b'\x1b', 'Optina')
        ui.send(b'd88\r', 'Confirm reading position')
        ui.expect('John:20'); ui.expect('Revelation:21'); ui.expect('87 preceding days')
        assert not state.exists(), 'preview must not save progress'
        ui.send(b'n', 'Choose plan day')
        ui.send(b'\x1b', 'day 1/89')
        ui.send(b'd90\r', 'Choose plan day')
        assert b'Invalid day' not in ui.data
        ui.send(b'\x1b', 'day 1/89')
        ui.send(b'd88\r', 'Confirm reading position')
        ui.send(b'y', 'day 88/89')
        assert json.loads(state.read_text())['next_day'] == 87
        ui.send(b'p', 'Choose plan')
        ui.send(b'j\r', 'Gospels - one chapter a day · day 1/89')
        assert json.loads(state.read_text())['next_day'] == 87
        assert json.loads(Path(str(state)+'.selection.json').read_text())['id'] == 'builtin:gospels'
        ui.send(b'd28\r', 'Confirm reading position')
        ui.send(b'y', 'day 28/89')
        ui.send(b'q')
    finally: ui.close()
    print('PASS in-app plan/day menus, cancellation, invalid days, confirmed day88 import, isolated progress')
    ui = UI(state, env)
    try:
        ui.expect('Gospels - one chapter a day'); ui.expect('day 28/89')
        ui.send(b'p', 'Choose plan'); ui.send(b'k\r', 'Optina · day 88/89')
        ui.send(b'p', 'Choose plan'); ui.send(b'G\r', 'Short custom · day 1/2')
        ui.send(b'cy', 'Completed and saved')
        ui.send(b'q')
    finally: ui.close()
    print('PASS remembered plan, restored per-plan day, and custom-plan discovery/opening/completion')
    ui = UI(state, env, cwd=Path('/'))
    try:
        ui.expect('Short custom')
        ui.send(b'd2\r', 'Confirm reading position'); ui.send(b'y', 'day 2/2')
        ui.send(b'q')
    finally: ui.close()
    print('PASS selected external custom plan restores outside repository; day change stays in app')
    # Emulate the next date after finishing the sole cycle using isolated test state.
    sidecars = list(Path(str(state)+'.plans').glob('*.json'))
    selected = json.loads(Path(str(state)+'.selection.json').read_text())['id']
    assert selected.endswith('short.json')
    custom_states = [p for p in sidecars if json.loads(p.read_text())['next_day']==1]
    assert len(custom_states)==1
    data=json.loads(custom_states[0].read_text()); data.update(next_day=2, completed_on=0)
    custom_states[0].write_text(json.dumps(data))
    ui = UI(state, env)
    try:
        ui.expect('Plan complete')
        ui.send(b'd1\r', 'Confirm reading position'); ui.send(b'y', 'day 1/2')
        ui.resize(30, 12)
        ui.send(b'p', 'Choose plan'); ui.send(b'\x1b', 'enlarge the terminal')
        ui.send(b'd', 'Choose plan day'); ui.send(b'\x1b', 'enlarge the terminal')
        ui.send(b'q')
    finally: ui.close()
    print('PASS finished nonrepeating plan can restart via in-app day picker')
print('ALL IN-APP CHECKS PASSED')


