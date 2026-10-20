# Dependency audit

Review of the Rust crates Kata ships (KAT-35). Checked on 2026-10-19 against `rust/Cargo.lock`.

## Method

- `cargo audit` against the RustSec advisory database (1,338 advisories loaded): no advisories and no warnings for all 66 locked crates.
- Every build uses `--locked`, so CI and local builds resolve exactly the versions below; a dependency change shows up as a `Cargo.lock` diff.
- Licences read from each crate's metadata. All are permissive (MIT, Apache-2.0, Zlib, Unlicense, 0BSD, Unicode-3.0). The project itself is MIT-0.
- `cargo audit` is not part of CI. Run it before each release: `cargo install cargo-audit --locked && cd rust && cargo audit`.

## Direct dependencies and why

| Crate | Purpose |
| --- | --- |
| crossterm | raw mode, terminal size and (Windows only) key events |
| unicode-width | display width of text, replacing the Zig width table |
| zip, flate2 (pure-Rust backend) | reading EPUB archives |
| png | PNG decoding (JPEG is our own port of the Zig decoder, for pixel-identical output) |
| sha2 | content hashes for ingest manifests |
| serde, serde_json | state, plan and manifest JSON |
| libc (Unix) | termios, signals, local time |

None of these opens a network connection, and the build script uses no network.

## Notes

- `miniz_oxide` appears twice (0.8 through `flate2`, 0.9 through `png`); both are pure Rust.
- Not checked: the contents of each crate's source. This review is advisory-database, licence and purpose only. Fuzzing of the EPUB, XHTML and image parsers is in `rust/fuzz` (manual; see its README).

## All crates in the shipped dependency tree

| Crate | Version | Licence |
| --- | --- | --- |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 |
| crossterm * | 0.29.0 | MIT |
| crossterm_winapi | 0.9.1 | MIT |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| document-features | 0.2.12 | MIT OR Apache-2.0 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 |
| flate2 * | 1.1.10 | MIT OR Apache-2.0 |
| generic-array | 0.14.7 | MIT |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| libc * | 0.2.191 | MIT OR Apache-2.0 |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| litrs | 1.0.1 | MIT OR Apache-2.0 |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.34 | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Unlicense OR MIT |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 |
| mio | 1.2.4 | MIT |
| nix | 0.31.3 | MIT |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| png * | 0.18.1 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| redox_syscall | 0.5.18 | MIT |
| rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde * | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json * | 1.0.154 | MIT OR Apache-2.0 |
| sha2 * | 0.10.9 | MIT OR Apache-2.0 |
| signal-hook | 0.3.18 | Apache-2.0/MIT |
| signal-hook-mio | 0.2.5 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | MIT |
| smallvec | 1.16.3 | MIT OR Apache-2.0 |
| syn | 3.0.9 | MIT OR Apache-2.0 |
| typed-path | 0.12.3 | MIT OR Apache-2.0 |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-width * | 0.2.2 | MIT OR Apache-2.0 |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| winapi | 0.3.9 | MIT/Apache-2.0 |
| winapi-i686-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | MIT/Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| zip * | 9.0.5 | MIT |
| zlib-rs | 0.6.8 | Zlib |
| zmij | 1.0.23 | MIT |

`*` direct dependency of a Kata crate.
