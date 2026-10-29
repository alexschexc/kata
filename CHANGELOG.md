# Changelog

Kata is a terminal reader. Before 1.0.0 a MINOR release may change things you
use; when it does, the entry says so under "Upgrade steps". A PATCH release
never breaks anything.

## [Unreleased]

### Added

- Ctrl+R checks the ingest folder for new or changed EPUBs while Kata is open,
  from the Library or from any reader. It runs in the background, so you keep
  reading; a line on the screen says when it starts and what it found, and the
  Library lists the new books straight away.

## [0.4.1] - 2026-10-26

### Changed

- Reading a plan day from a converted book now works like a Bible plan day.
  The day's text is one continuous buffer, and reaching the bottom, at the
  "end of today's reading" marker, is the end of that day. `H`, `L` and `t`
  no longer jump between chapters there, and `/` searches only that day.

### Added

- Where you stop in a plan day is remembered for each plan, day and item, so
  reopening it returns you to the same place. Free reading of the same book
  keeps its own position.

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
