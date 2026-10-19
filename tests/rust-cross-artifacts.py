"""Inspect Rust cross-build artifacts in rust/target/<triple>/release/ (no target execution).

Port of tests/cross-artifacts.py. Checks executable headers, architecture and OS-library
dependencies for Linux musl, macOS arm64 and Windows x64. The embedded-corpus/provenance check
is skipped for the skeleton and enabled with --expect-corpora once the TSVs are embedded (KAT-17).
Usage: python tests/rust-cross-artifacts.py [--expect-corpora]   (build first: scripts/rust-cross-build.sh)
"""
from pathlib import Path
import hashlib, json, struct, subprocess, sys

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / 'rust/target'
inputs = {'linux-x64-musl': RELEASE/'x86_64-unknown-linux-musl/release/kata',
          'macos-arm64': RELEASE/'aarch64-apple-darwin/release/kata',
          'windows-x64': RELEASE/'x86_64-pc-windows-gnu/release/kata.exe'}
expect_corpora = '--expect-corpora' in sys.argv[1:]
metadata = json.loads((ROOT/'src/data/provenance.json').read_text())
# Rust std on macOS links libiconv/libcharset in addition to libSystem; all are system libraries.
MAC_ALLOWED = {'/usr/lib/libSystem.B.dylib', '/usr/lib/libiconv.2.dylib', '/usr/lib/libcharset.1.dylib'}
WIN_SYSTEM = {'kernel32.dll', 'msvcrt.dll', 'ntdll.dll', 'advapi32.dll', 'ws2_32.dll', 'bcrypt.dll',
              'crypt32.dll', 'user32.dll', 'shell32.dll', 'ole32.dll', 'secur32.dll'}
results = {}
for target, path in inputs.items():
    assert path.exists(), f'{path} missing: run scripts/rust-cross-build.sh'
    data = path.read_bytes()
    details = {'path': str(path.relative_to(ROOT)), 'bytes': len(data),
               'sha256': hashlib.sha256(data).hexdigest(), 'executed_on_target': False}
    if expect_corpora:
        for source, entry in metadata['sources'].items():
            raw = (ROOT/'src/data'/entry['member']).read_bytes()
            assert hashlib.sha256(raw).hexdigest() == entry['tsv_sha256']
            assert raw in data, (target, source, 'exact corpus missing')
            assert entry['tsv_sha256'].encode() in data, 'embedded provenance missing'
        details['corpora_verified'] = True
    if target == 'linux-x64-musl':
        assert data[:4] == b'\x7fELF' and data[4] == 2 and data[5] == 1  # ELF64, little-endian
        assert struct.unpack_from('<H', data, 18)[0] == 0x3e  # x86-64
        dynamic = subprocess.check_output(['readelf', '-ld', str(path)], text=True)
        assert 'NEEDED' not in dynamic and 'INTERP' not in dynamic, 'not statically linked'
        details.update(format='ELF x86-64 static', libraries=[])
    elif target == 'macos-arm64':
        magic, cpu, sub, kind, ncmds, size, flags, reserved = struct.unpack_from('<8I', data)
        assert magic == 0xfeedfacf and cpu == 0x0100000c and kind == 2
        offset, libraries, signature = 32, [], False
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
            if cmd == 0x1d:  # LC_CODE_SIGNATURE: presence only, not signing/notarization validation
                start, length = struct.unpack_from('<2I', data, offset+8)
                assert start+length <= len(data) and length > 0
                signature = True
            offset += count
        assert libraries and set(libraries) <= MAC_ALLOWED and '/usr/lib/libSystem.B.dylib' in libraries, libraries
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
        assert libraries and all(name.lower() in WIN_SYSTEM or name.lower().startswith('api-ms-win-')
                                 for name in libraries), libraries
        details.update(format='PE32+ Windows x86-64', libraries=libraries)
    results[target] = details
print(json.dumps(results, indent=2))
print('PASS Rust cross-build headers, architecture and OS-only library dependencies'
      + ('; embedded corpora/provenance verified' if expect_corpora else '; corpora check skipped (skeleton)')
      + '; target runtime NOT tested')
