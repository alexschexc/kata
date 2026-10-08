# Single-file distribution

Kata now embeds the original installed `kjv`, `grb`, and `vul` TSV text datasets and indexed chapter spans. References are selected directly in Zig. Reading requires no external applets, shell utilities, Python, network, data directory, or runtime extraction.

## Observed installed sources

All three executables are POSIX shell scripts followed by gzip-compressed tar archives containing an AWK program and a TSV text corpus. Their script paths invoke `sed`, `tar`, `awk`, `tput`, and a pager. Bundling the scripts would not remove those dependencies.

Inspection found these TSV sizes:

| Source | TSV bytes |
| --- | ---: |
| kjv | 5,634,305 |
| grb | 10,307,364 |
| vul | 4,500,727 |

The three existing compressed archives total 5,325,926 bytes; raw TSV totals 20,442,396 bytes. These are measured source sizes, not promises of a final executable size.

## Implemented bundling

`src/data/*.tsv` contains the exact archive-member bytes; generated chapter indexes select spans without copying the whole corpus. `src/bundled.zig` implements chapter/verse lists, ranges, and cross-chapter ranges, preserving source availability, duplicate Greek text, and chapter-zero prologues. Data is currently embedded uncompressed, trading executable size for simple, fast, dependency-free reads. Compression is not implemented.

Normal builds need only Zig 0.16.0; sources and generated indexes are included in the project. The optional development command `python tests/bundle_sources.py` refreshes exact TSV/index files from the installed source archives without executing those scripts. `--verify` checks all bytes against those archives. It records applet, archive, and TSV SHA-256 hashes in `src/data/provenance.json`. Refresh the separate book catalog/discovery metadata when changing editions, and rerun all parity checks.

`kata --licenses` prints embedded attribution/license notes and dataset provenance. This does not read companion files or access the network.

## Linux build and verification

Build a stripped static x86-64 Linux executable:

```sh
zig build -Dtarget=x86_64-linux-musl -Doptimize=ReleaseSmall -Dstrip=true
```

`file` reports a statically linked ELF executable; program headers contain no dynamic interpreter. Copying just this executable is sufficient for supported Linux terminals; writable progress files are created separately in the user's state directory. Optional custom plans are still separate user files, not required runtime dependencies.

Verification scripts accept `KATA_BIN=/absolute/path/to/kata`:

- `tests/bundle-standalone.py`: copy just the executable outside the repository, isolate HOME/config/state, and clear PATH of all applets and utilities. Exercise real text dumps, menus, reader navigation, built-in plans, licenses, and terminal cleanup.
- `tests/all-source-books.py --chapters`: compare all 234 source/book combinations, 108,305 distinct verse labels/texts, and 1,479 chapter queries with original source output. Kata runs with no tools on its PATH; original applets run only as verification oracles.
- `tests/reference-parity.py`: compare verse/chapter lists, ranges, and cross-chapter ranges; reject malformed references.
- `tests/input-pty.py`: stress unknown/fragmented/malformed terminal input, paste, numerical selections, reader/menu/confirmation paths, tiny-terminal resize, progress preservation, and terminal restoration.
- `tests/plan-regression.py` and `tests/cli-regression.py`: retain prior in-app and CLI plan/import coverage.

All checks use isolated temporary state; they do not mutate normal reading progress.

## Cross-platform boundaries

A separate executable is needed for each OS/CPU. Windows and Apple Silicon (`aarch64-macos`) are deferred until the bundled Linux executable is reviewed and confirmed. Windows requires a native console/input/terminal backend instead of the current POSIX termios/ioctl/signal implementation. macOS must also be build-checked and exercised on target hardware; Linux verification does not prove macOS behavior.

The older `zig-out/aarch64-linux-musl/bin/kata` artifact predates bundling. It was only cross-built, not executed on ARM hardware, and is not this standalone release.

## Redistribution notices

Installed script headers declare public-domain code. Text licensing must be checked separately. Greek upstream identifies its New Testament as SBLGNT:

- https://github.com/LukeSmithxyz/grb
- https://sblgnt.com/license/

The current SBLGNT license page specifies Creative Commons Attribution 4.0 International; attribution, license links, and change notices must be retained as applicable. Greek upstream's older README description of a noncommercial license is not the current license page. Confirm edition provenance and the Septuagint, Latin, and KJV datasets' applicable redistribution terms before publishing a bundled corpus. Script license declarations alone do not establish all text rights.
