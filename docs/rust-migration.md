# Kata: Zig to Rust appraisal and cross-compilation migration path

Status: appraisal only. No Rust code for Kata exists yet. Decision (2026-10-18): proceed with
the migration, because the goal is to consolidate on JVM languages plus Rust only. Work starts
the next day; Phase 0 first. Everything under "Measured"
was checked on 2026-10-18; everything under "Estimate" is judgement.

## Measured

Codebase
- 36 Zig files, 11,706 lines, 140 `test` blocks, plus 1,773 lines of Python PTY/CLI/parity tests.
- 20 MB embedded corpora (kjv/grb/vul TSV) with generated span-index `.zig` files.
- Own implementations of: PNG, JPEG (baseline/progressive), Sixel, Kitty/inline images,
  ZIP/EPUB reader, XHTML-to-document, Unicode width tables (Unicode 16), case folding, search.
- Zig `std` I/O, posix and process APIs appear in ~159 places (the churn-prone surface).
- C interop is small: `wcwidth` (layout.zig), `time.h` localtime (app.zig, cli.zig),
  termios/ioctl/poll/signal (terminal_posix.zig), `windows.h` console (terminal_windows.zig),
  `getenv` (tui.zig).
- Zig is pre-1.0. The repo already pins 0.16.0 via mise.toml while global mise is 0.17.0;
  `std.Io.Writer`, `std.Io` and `std.process.Init` are exactly the APIs that move between releases.

Current cross build (Zig)
- One toolchain, no extra install: `zig build -Dtarget=aarch64-macos|x86_64-windows-gnu|x86_64-linux-musl`.
- macOS binary imports only `libSystem.B.dylib`, minos 13.0, ad-hoc code signature present.
- Never executed on real macOS/Windows by us. The red-screen bug was found by a user, not by a test.

Rust spike (throwaway crate in scratch, not in the repo)
- Host has rustc/cargo 1.98.0. Added targets aarch64-apple-darwin, x86_64-pc-windows-gnu;
  installed cargo-zigbuild 0.23.4 and used the existing Zig 0.16.0 as linker.
- Hello-world using crossterm 0.29, unicode-width 0.2, flate2 (rust backend), serde_json,
  opt-level z + LTO + strip + panic=abort:
    Linux gnu  337 KB ELF (dynamic glibc)
    macOS arm64 320 KB Mach-O, signed, minos 13, links libSystem + libiconv + libcharset
    Windows x64 315 KB PE32+, imports KERNEL32, ntdll and api-ms-win-crt-* (UCRT, Windows 10+)
- Both cross builds succeeded from Linux in about a minute cold. No Apple SDK, no Windows machine.
- Not tested: musl static Linux target, aarch64-pc-windows, running anything on the targets.

## What Rust would and would not fix

Fixes
- Stable language and std: no more per-release rewrites of I/O and build code.
- Mature crates replace hand-rolled parsers (png, zune-jpeg/jpeg-decoder, zip, miniz_oxide,
  unicode-width/unicode-segmentation, time/chrono). That deletes roughly 2,500+ lines
  (png, jpeg, epub zip, flate use, unicode_width, fold_table, terminal_*) but adds dependencies
  to audit, which matters given the project's supply-chain stance.
- One terminal abstraction (crossterm) instead of separate posix and windows backends.
- Better test tooling (cargo test, proptest, insta snapshots, fuzzing for the EPUB/image parsers).

Does not fix
- The v0.3 Mac bug. It was missing terminal-capability handling, not Zig. crossterm does not
  downsample truecolor either; the 256-colour fallback must be ported.
- Lack of a real Mac/Windows test machine. Cross-compiling succeeds either way.

Costs and risks
- Rewrite of ~11.7k lines that currently works and has Linux parity tests against reference tools.
- Binary size: Zig ReleaseSmall musl static is the benchmark (21 MB mostly corpora). Rust will be
  similar since corpora dominate, but needs `include_bytes!` of 20 MB and a replacement for the
  Python-generated index files (build.rs or a runtime index).
- Linux static binary needs the musl target plus a linker; cargo-zigbuild covers it.
- macOS binary links libiconv/libcharset in addition to libSystem (Rust std); still system libs,
  but the "libSystem only" claim in PACKAGING.md would need rewording.
- Zig remains a build-time dependency if cargo-zigbuild is used. It is optional: plain
  `cargo build` works for Linux and Windows-gnu with mingw; macOS cross needs zig or osxcross.
- Interview-project angle: a working Zig TUI is itself the artifact; a half-finished rewrite is worse.

## Estimate

Roughly 9k to 12k lines of Rust. About 2 to 4 weeks of focused human work, less if the Python
black-box tests are reused as the acceptance gate. Highest risk modules: terminal input/resize
parity (input.zig, tui.zig, book_reader.zig) and image decode/Sixel/Kitty output. Lowest risk:
state, plan, picker, reading_units, catalog, search, document (pure logic with tests).

## Recommendation

The macOS bug alone does not justify a rewrite (it was missing terminal-capability handling,
not a Zig problem). The language-consolidation goal (JVM languages plus Rust only) does, since
Zig would otherwise remain a language kept only for Kata. Migrate in the strangler-by-parity
order below and keep Zig as the shipped build until cutover.
The cross-compile story is already equivalent; the real gap is verification on native OSes,
and that is fixable first, in either language (Phase 0).

## Zig as linker vs the consolidation goal

The Mac cross-build in the spike worked only because cargo-zigbuild borrows Zig's linker and
macOS system stubs, so Zig stays a tool dependency even after no Zig is written. Options:
1. Keep Zig as a build tool only. Cheapest, verified in the spike. Use during the migration.
2. Native macOS CI runner builds the Mac binary (also provides the missing real-Mac
   verification). Linux and Windows still cross-compile locally with cargo or cargo-xwin.
   Cleanest way to drop Zig entirely. Target end state.
3. osxcross with an Apple SDK from Xcode. Licensing and fragility problems; avoid.

Plan: option 1 now, move to option 2 at cutover (Phase 7).

Since Zig code is being retired, prefer mature crates (png, zune-jpeg, zip, unicode-width,
crossterm) over line-by-line ports, and keep the Python PTY/parity tests unchanged as the gate.

## Migration path

Phase 0 (do now, language independent): close the verification gap
- Add CI with a native macOS arm64 and Windows runner that executes `tests/target-smoke.py`
  plus a PTY colour test (assert no `38;2`/`48;2` when TERM_PROGRAM=Apple_Terminal).
- Add `tests/color-fallback-pty.py` to the repo (the probe used for v0.3.1).
- Tag the Zig release (v0.3.1) so Rust can be diffed against it.

Phase 1: skeleton and toolchain
- `rust/` workspace in the same repo: crates `kata-core` (pure logic), `kata-term` (terminal),
  `kata` (binary). `rust-toolchain.toml` pins 1.98.0 with targets
  x86_64-unknown-linux-musl, aarch64-apple-darwin, x86_64-pc-windows-gnu.
- mise.toml gains `rust` plus tasks; install `cargo-zigbuild` locked. Pin zig 0.16.0 only for linking.
- Release profile: opt-level z, lto, strip, panic abort, codegen-units 1.
- Build script (replaces PACKAGING commands):
    cargo zigbuild --release --target aarch64-apple-darwin
    cargo zigbuild --release --target x86_64-pc-windows-gnu
    cargo zigbuild --release --target x86_64-unknown-linux-musl
- Port `tests/cross-artifacts.py` to look at `rust/target/<triple>/release/`.

Phase 2: pure logic with golden tests (no terminal)
- Port unicode width (use the crate, cross-check against the existing table for the Greek,
  combining, CJK and emoji cases in the Zig tests), state.json, plan, picker, reading_units,
  catalog, reference parsing, search/fold, document, layout.
- Gate: same inputs as the Zig tests; byte-identical `--dump` output for all 234
  source/book combinations via `tests/all-source-books.py` and `tests/reference-parity.py`.

Phase 3: corpora and data
- `include_bytes!` for the TSVs; replace generated `*_index.zig` with a build.rs or lazy index
  built from the TSV. Keep provenance.json checks.

Phase 4: terminal layer
- crossterm (raw mode, size, events, bracketed paste) behind a small trait; keep the Zig
  input decoder semantics by porting `input.zig` tests. Port `detectColorDepth` and the
  256-colour downsample first; add `KATA_COLORS`.
- Windows: rely on crossterm's Windows backend; re-run the Windows-specific checks in
  PACKAGING.md (no HOME set, UTF-8 output).

Phase 5: EPUB, images, ingest
- zip + miniz_oxide, png, zune-jpeg, own Sixel/Kitty encoders; fuzz the EPUB and image parsers.

Phase 6: app, TUI, book reader
- Port tui.rs, book_reader.rs, app.rs, start_menu.rs. Gate: all existing PTY tests
  (`tests/*-pty.py`, `cli-regression.py`, `plan-regression.py`) pass unchanged against the Rust binary.

Phase 7: cutover
- Release Rust build for all three targets, run Phase 0 CI on real macOS and Windows,
  ship as a minor version with Zig tag retained. Remove Zig sources only after one release
  cycle with no regressions.

Decision gates: stop after Phase 2 if the parity suite shows divergence you do not want to
chase; each phase leaves the Zig build shipping.
