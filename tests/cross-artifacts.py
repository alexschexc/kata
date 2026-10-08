"""Read cross-build headers and prove exact datasets are embedded (no target execution)."""
from pathlib import Path
import hashlib, json, struct, subprocess

ROOT = Path(__file__).resolve().parents[1]
inputs = {'macos-arm64': ROOT/'zig-out/macos-arm64/bin/kata',
          'windows-x64': ROOT/'zig-out/windows-x64/bin/kata.exe'}
metadata = json.loads((ROOT/'src/data/provenance.json').read_text())
results = {}
for target, path in inputs.items():
    data = path.read_bytes()
    details = {'path': str(path), 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(),
               'executed_on_target': False}
    for source, entry in metadata['sources'].items():
        raw = (ROOT/'src/data'/entry['member']).read_bytes()
        assert hashlib.sha256(raw).hexdigest() == entry['tsv_sha256']
        assert raw in data, (target, source, 'exact corpus missing')
        assert entry['tsv_sha256'].encode() in data, 'embedded provenance missing'
    if target == 'macos-arm64':
        magic, cpu, sub, kind, ncmds, size, flags, reserved = struct.unpack_from('<8I', data)
        assert magic == 0xfeedfacf and cpu == 0x0100000c and kind == 2
        offset = 32
        libraries = []
        signature = False
        for _ in range(ncmds):
            cmd, count = struct.unpack_from('<2I', data, offset)
            assert count >= 8 and offset+count <= 32+size
            if cmd == 0x32:  # LC_BUILD_VERSION
                platform, minimum, sdk, tools = struct.unpack_from('<4I', data, offset+8)
                assert platform == 1
                details['minimum_macos'] = f'{minimum>>16}.{(minimum>>8)&255}.{minimum&255}'
            if cmd == 0xc:  # LC_LOAD_DYLIB
                name_offset = struct.unpack_from('<I', data, offset+8)[0]
                libraries.append(data[offset+name_offset:offset+count].split(b'\0', 1)[0].decode())
            if cmd == 0x1d:  # LC_CODE_SIGNATURE: presence, not signing/notarization validation
                start, length = struct.unpack_from('<2I', data, offset+8)
                assert start+length <= len(data) and length > 0
                signature = True
            offset += count
        assert libraries == ['/usr/lib/libSystem.B.dylib'], libraries
        assert signature
        details.update(format='Mach-O arm64', libraries=libraries, code_signature_present=True,
                       notarization_verified=False)
    else:
        assert data[:2] == b'MZ'
        pe = struct.unpack_from('<I', data, 0x3c)[0]
        assert data[pe:pe+4] == b'PE\0\0'
        assert struct.unpack_from('<H', data, pe+4)[0] == 0x8664
        assert struct.unpack_from('<H', data, pe+24)[0] == 0x20b
        info = subprocess.check_output(['objdump', '-p', str(path)], text=True)
        libraries = [line.split('DLL Name:', 1)[1].strip() for line in info.splitlines() if 'DLL Name:' in line]
        system = {'kernel32.dll', 'msvcrt.dll', 'ntdll.dll', 'advapi32.dll', 'ws2_32.dll',
                  'bcrypt.dll', 'crypt32.dll', 'user32.dll', 'shell32.dll', 'ole32.dll', 'secur32.dll'}
        assert libraries and all(name.lower() in system or name.lower().startswith('api-ms-win-')
                                 for name in libraries), libraries
        details.update(format='PE32+ Windows x86-64', libraries=libraries)
    results[target] = details
print(json.dumps(results, indent=2))
print('PASS cross-build headers, architecture, embedded corpora/provenance, and OS-only library dependencies; target runtime NOT tested')
