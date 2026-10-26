# ADR 0001: Chapter index for the embedded corpora (Rust port)

Status: accepted (NAD-18)

## Context
The Zig build embeds three TSV corpora (kjv, grb, vul; 20.4 MB) and a generated `*_index.zig`
per corpus that lists one byte span per (book, chapter). Reads slice the span instead of
scanning the whole corpus. The Rust port needs an equivalent.

## Options
1. `build.rs` generates the spans from the embedded TSV at compile time.
2. Lazy index: scan the TSV at first use (tried first, in NAD-14).
3. Commit generated Rust index files, as the Zig build does.

## Decision
Option 1. `rust/crates/kata-core/build.rs` reads `src/data/*.tsv`, applies the same rules as
`tests/bundle_sources.py`, and writes `spans.rs` into `OUT_DIR`; `bundled.rs` includes it.

## Measured (release, musl, this machine, best of 5)
| | Zig V0.3.1 | Rust lazy scan | Rust build.rs |
| --- | ---: | ---: | ---: |
| `--passage John:1 --dump` | 0.9 ms | 47 ms | 1.1 ms |
| `--passage Psalms --dump` | 43.4 ms | | 40.1 ms |
| `--passage Genesis --dump` | 18.8 ms | | 18.8 ms |
| search "shepherd" (kjv, 79 hits) | 145 ms | | 102 ms |
| search "λογος" (grb, 320 hits) | 192 ms | | 190 ms |
| search "verbum" (vul, 481 hits) | 109 ms | | 79 ms |

The lazy scan costs about 46 ms on every start, which is why it was rejected. The build.rs
index matches Zig on startup and search and is faster on three of the four timed searches.

## Consequences
- No generated index is committed; the TSV is the single source of truth.
- A test compares the build-time spans with a runtime scan and with Zig's committed
  `*_index.zig`, so a change to the corpus or to either generator that makes them disagree
  fails the build.
- Building needs `src/data/*.tsv` in the repo, as `include_str!` already does.
