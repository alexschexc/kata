# Changelog

Kata is a terminal reader. Before 1.0.0 a MINOR release may change things you
use; when it does, the entry says so under "Upgrade steps". A PATCH release
never breaks anything.

## [Unreleased]

## [0.4.0] - 2026-10-20

Kata is now written in Rust. Output for every reference, plan and command line
matches the last Zig release (0.3.1) byte for byte, and your saved progress,
plans and converted books carry over without any step.

### Added

- Themes. Library, then `Theme`, switches between Ethereal (the default),
  Aetheria and Sedate. The choice is saved. `KATA_THEME=sedate kata` picks one
  for a single run.

### Changed

- The macOS build links `libiconv` as well as `libSystem`; both ship with
  macOS.
- `kata --help` no longer says "Zig prototype".
- Decoding JPEG images, ingesting large books and similar heavy work can take
  a little longer than before. Nothing changes in the result.
