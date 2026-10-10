# Kata prototype

Kata is a Zig terminal reader with the original installed `kjv`, `grb`, and `vul` TSV datasets embedded in the executable. Native Zig code selects their actual text; no source command or substitute dataset is used at runtime.

## Build and review

Build requirements: Zig 0.16.0 and libc support supplied by Zig's toolchain. Runtime requirements: a Linux/POSIX terminal and writable state directory. The bundled source files and indexes are included in the project; normal builds need neither Python nor installed source commands. The build uses Zig's LLVM/LLD backend; this avoids the native linker's incompatibility with the host's newer glibc SFrame relocations.

From this directory:

```sh
zig build
zig build test --summary all
./zig-out/bin/kata --passage 'John:1'
```

Open the library and choose a reading mode:

```sh
./zig-out/bin/kata
```

For a review without touching your usual progress, select a separate state file:

```sh
./zig-out/bin/kata --state /tmp/kata-review-state.json
```

## Library and reading modes

Normal startup opens **Library → title → Reading mode**. Choose **Bible** for every available book from the installed `kjv`, `grb`, and `vul` sources: Old Testament, New Testament, additional books, and distinct Greek textual variants. The duplicate **New Testament** listing is hidden. Its internal metadata and existing files are retained for compatibility; the remembered book/chapter maps into Bible's picker. Future unrelated titles require real source wrappers and a registry entry; they are not offered as nonfunctional placeholders.

Book names, source query aliases, and chapter availability were discovered from the original source commands. Translation collections differ: a source without the chosen book/chapter/verse contributes no verses, while the available sources remain readable. Malformed passage references remain errors rather than fabricated or substituted text. Variant editions remain separate books; verse-number differences are not automatically mapped.

Psalms follow the Greek (LXX) and Latin (Vulgate) numbering. The KJV pane, which uses Hebrew (Masoretic) numbering, is placed under the matching Greek/Latin psalm: Psalm 9 holds KJV 9–10, 10–112 hold KJV 11–113, 113 holds KJV 114–115, 114 and 115 hold KJV 116:1–9 and 116:10–19, 116–145 hold KJV 117–146, 146 and 147 hold KJV 147:1–11 and 147:12–20, and 148–150 are identical. Greek Psalm 151 has no KJV text. Each translation keeps its own verse numbers: KJV lines show their KJV chapter:verse (e.g. `23:1` under Psalm 22), and `--dump` prints them as `kjv (23:1): …`. Because the Greek and Latin count superscriptions as verses and the KJV does not, verses within a psalm align by label, not by content.

The installed editions list 79 KJV books, 87 Greek books, and 68 Latin books, forming 89 distinct canonical entries. Greek records with repeated verse labels are displayed together under that label, preserving their text in source order with a space between records. This preserves source content without claiming a corrected verse-number mapping. Sirach's chapter-zero prologue is accessible; chapter choices and navigation use observed chapter availability rather than assuming every number exists in every source.

For a standalone, size-optimized stripped x86-64 Linux executable, build with `zig build -Dtarget=x86_64-linux-musl -Doptimize=ReleaseSmall -Dstrip=true`. This statically links musl and embeds the original text uncompressed. Python scripts under `tests/` are extraction/provenance and verification tools only. `--licenses` displays embedded source credits and licensing notes.

- **Free reading:** choose a book, chapter, and starting verse. Menus accept `j`/`k` or a numbered selection followed by `Enter`. The verse picker uses the actual chapter output: `0` resumes that chapter's saved position (or starts at its beginning if new), while a verse selection anchors the full chapter rather than fetching only one verse. `H`/`L` (or `Shift+←`/`Shift+→`) move to previous/next chapters, including across book boundaries, without wrapping around the title.
- **Reading plans:** choose a plan and enter the existing daily-session reader. Completion, postponed missed readings, day imports, and independent per-plan progress behave as before.

Press `m` in either reader to return to the library, or `o` to choose a free-reading place. `Esc`/`q` backs out of nested startup menus; at Library it quits without writing progress. Explicit `--passage` and `--plan` launches still open directly in their corresponding reading view.

Free-reading positions and pane settings are saved separately for each chapter or explicit passage under `<state-path>.library/<title-id>/`. `<state-path>.free-selection.json` remembers the last free-reading book/chapter for the picker. Free reading does not alter a plan's completed prefix or completion date. Existing plan progress is retained.

Press `p` inside the reader to choose a plan. Use `j`/`k`, then `Enter`; `Esc` or `q` cancels. Kata includes Optina and a one-chapter-a-day Gospels plan. It also discovers valid JSON plans from `config/` in the current working directory and `$XDG_CONFIG_HOME/kata/plans/` (normally `~/.config/kata/plans/`). Invalid files are skipped with a notice; identical configs are deduplicated. Discovery happens at launch, so restart after adding or editing a file.

Switching saves the old plan's position and restores the selected plan's own progress and pane settings. The last selected plan is remembered across launches, including custom files outside the repository. There is no need to quit the application to switch plans. Built-in configurations are embedded at build time; discovered file variants are loaded immediately. You can also launch with an explicit configuration:

```sh
./zig-out/bin/kata --plan config/optina.json --state /tmp/kata-custom-review.json
./zig-out/bin/kata --check-plan
./zig-out/bin/kata --passage 'John:1:1-3' --dump
./zig-out/bin/kata --help
```

`--dump` and `--check-plan` do not write progress and follow the remembered plan unless `--plan` overrides it. An explicit `--plan` still rejects an incompatible base state. In-app switching keeps incompatible schedules in separate progress files automatically; editing a discovered config creates a new schedule identity rather than silently reusing old progress.

## Continue from a print reading plan

Inside the reader, press `d` to browse the selected plan's days and their actual chapter assignments. Use `j`/`k`, `g`/`G`, or type a day number. Press `Enter` to preview the progress change, then `y` to save and open that day's reading. `Esc` or `q` cancels without changing progress.

For Optina, John 20 and Revelation 21 are day 88: press `d`, type `88`, press `Enter`, and confirm with `y`. No command-line import is necessary.

The command-line preview and import remain available as alternatives:

Preview importing your position without changing saved progress:

```sh
./zig-out/bin/kata --start-day 88
```

The preview names the assigned chapters and explains which progress will be replaced. Confirm, then launch the normal reader:

```sh
./zig-out/bin/kata --start-day 88 --confirm
./zig-out/bin/kata
```

Days 1–87 in the current cycle count as completed elsewhere. Day 88 remains pending and available today; importing does not mark it read or consume today's completion. Finish it normally with `c`, then `y`. No fictional completion dates are created for readings done in print.

The day number is 1-based and must lie within the plan's cycle. Forward and backward changes are supported: a confirmed change replaces the completed prefix within the current cycle, makes the selected day pending, and clears today's completion marker and reading positions. Previously finished cycles are retained for repeating plans. Nonrepeating plans can be restarted within their sole cycle. Pane visibility, focus, and linked-scroll preference are retained.

Use the same `--state` for a command-line import and subsequent launches when using a review state. `--plan` can override the remembered selection. Do not reposition progress while another instance is using that state file.

## Keys

| Keys | Action |
| --- | --- |
| `j` / `k`, or `↓` / `↑` | Scroll down/up one rendered line |
| `Ctrl-d` / `Ctrl-u` (also `f` / `b`) | Scroll half a screen |
| `g` / `G` | Beginning/end of the session |
| `h` / `l`, `←` / `→`, or `Tab` | Focus another visible pane |
| `s` | Toggle linked and independent scrolling |
| `1` / `2` / `3` | Show/hide KJV, Greek, and Latin panes |
| `m` | Return to Library and choose a title/mode |
| `o` | Open the free-reading book/chapter/verse picker |
| `H` / `L`, or `Shift+←` / `Shift+→` | Previous/next chapter in free-reading mode |
| `p` | Open the in-app plan picker; `j`/`k`, then `Enter` selects |
| `d` | Open the in-app day picker; browse or type a number, `Enter` previews, `y` confirms |
| `Esc` / `q` in a picker | Cancel and return to the reader |
| `c`, then `y` | Explicitly confirm today's completion in plan mode |
| `q`, or `Ctrl-c` | Save position and leave the TUI |
| `/` | Search the focused translation; type a word or phrase, `Enter` searches, `Esc` cancels |
| `r` | Focus the search results panel |
| `n` / `N` | Open the next/previous search result |
| `x` | Close search and clear highlighting |

## EPUB books (ingest)

First launch asks for two absolute folders: an ingest folder (default suggestion `~/kataIngest`) that Kata only reads EPUBs from, and a library folder (`~/kataLibrary`) where each EPUB is converted to a Kata document with the same name minus `.epub`. The choice is saved in `<config>/kata/folders.json` and can be changed from the last Library entry. `Esc` skips the prompt; Bible reading works without folders.

At startup Kata converts only new or changed EPUBs and then lists every indexed document in the Library. `<library>/.kata-manifest.json` records each source's size, modification time, SHA-256, and converter version; `<library>/.kata-index.json` lists readable documents. Unchanged books are neither reconverted nor reparsed. Documents whose EPUB was removed are kept and reported as orphaned; Kata never deletes them. `kata ingest` prints a per-book report; `--force` reconverts everything.

Conversion keeps chapters (spine order, titled by their first heading), headings, paragraphs, verse numbers (explicit or counted within each heading, cross-checked against printed numbers), superscriptions and rubrics, print page labels, code blocks with exact spacing, lists, glossary terms, simple tables, figure captions, sidebars and footnotes. It drops navigation, index pages, ornaments, scripts/styles, SVG, and index-term anchors. Images are copied to `<library>/<book>.assets/` and shown as `▣ image` lines; `i` opens the nearest one in the desktop image viewer. In terminals with image support, PNG and JPEG figures are drawn inline in the text column (Sixel in foot and xterm; Kitty graphics in Ghostty, kitty and WezTerm), up to three quarters of the screen height, cropped as they scroll. Kata asks the terminal once at startup (`kata --detect-graphics` shows the answer); set `KATA_IMAGES=off` to disable. Terminals without image support keep the `▣ image` line. EPUBs with encrypted content (DRM) are refused; font obfuscation is allowed.

Book reader keys: `j`/`k`, `Ctrl-d`/`Ctrl-u` (or `f`/`b`, Space), `g`/`G`; `H`/`L` (or `Shift+←`/`Shift+→`) previous/next chapter; `↑`/`↓` scroll; `t` chapter list; `/` search the whole book (results panel, `n`/`N`, `r`, `x` as in the Bible reader); `m` Library; `q` quit. Verse numbers appear in the left gutter and page changes as `── page N ──` rules. Position is saved per book under `<state-path>.books/`.

## Word search

Press `/` in any reader. A Vim-style prompt opens in the bottom bar and searches the focused pane's translation only (KJV, Greek, or Latin). As you type, matches in the visible text are highlighted. `Enter` searches that whole source and opens a results panel on the right listing every verse that contains the term, with the verse/match counts and a context snippet. `Ctrl-u` clears the prompt; `Backspace` deletes.

In the panel: `j`/`k` (or `Ctrl-d`/`Ctrl-u`, `g`/`G`) select, `Enter` opens the selected verse, `n`/`N` open the next/previous result, `Tab`/`Esc` return to the reader, and `x` closes the search. Opening a result in the current reading scrolls to it; a result elsewhere opens that chapter in free reading with the panel still open. Every occurrence stays highlighted in the searched pane. On narrow terminals (under 72 columns) the panel takes the whole width while focused.

Matching is case- and accent-insensitive and matches word starts: `love` finds *love*, *loved*, *loveth*, but not *glove*; `λογος` finds *λόγος*, *Λόγος*, *λόγου*. Multi-word phrases match with single spaces between words. Different inflected forms of the same word with different stems are not linked: no lemma or concordance data is used. KJV results in Psalms are listed under the Greek/Latin psalm number with the KJV label in parentheses, for example `Psalms 22:1 (kjv 23:1)`.

Corresponding verse labels share a row height, with padding under shorter translations. Linked scrolling operates over this common layout; independent scrolling moves only the focused pane. Resizing rewraps text while preserving verse-row anchors. The terminal's original input mode and screen are restored on exit and handled termination signals.

Unknown keys and invalid numeric selections do nothing silently. Terminal escape sequences (including arrow/function-key and modified-key events), control strings, malformed Unicode, and bracketed-paste payloads are consumed without dispatching their bytes as commands. Arrow keys act as `h`/`j`/`k`/`l`, and `Shift+←`/`Shift+→` as `H`/`L`; other modified or function keys are ignored. Arrows are inert while typing in a prompt. A lone `Esc` still cancels after a short interbyte timeout. Incomplete bracketed paste remains quarantined until its closing marker, or until the process is stopped with a signal. Paste framing is enabled on entry and disabled on exit.

## Reading plans and state

Configurations specify ordered concurrent streams of books/chapter counts and phases with a duration and one chapter rate per stream. Zero rates can pause a stream. The engine validates that a complete cycle consumes each stream exactly once; it rejects under- and overshooting configurations.

The supplied repeating Optina plan contains 89 assignments:

- Days 1–82: one Gospel chapter and two chapters from Acts through Revelation.
- Days 83–89: one John chapter and one Revelation chapter.
- First assignment: Matthew 1, Acts 1, Acts 2.
- Final assignment: John 21, Revelation 22.

The Gospels plan reads Matthew, Mark, Luke, and John at one chapter per day, repeating after 89 days. Custom nonrepeating plans show a completion screen after their final assignment; `p` selects another plan and `d` can restart at a chosen day.

Progress is explicitly completed at most once per local calendar date. Missed days leave the next assignment unchanged: no skipped readings and no catch-up workload. Completion keeps the just-completed assignment available that day; the next local date exposes the next assignment. At cycle end, the plan repeats.

By default, state is saved to `$XDG_STATE_HOME/kata/state.json`, or `~/.local/state/kata/state.json` if the XDG variable is absent. It stores progress, the last completion date, source visibility, focus, linked mode, and viewport positions. State writes use a temporary file followed by rename. Only one Kata instance should use a given state file at a time; concurrent writers are not locked yet.

Existing base progress is preserved. Additional plans use `<state-path>.plans/<config-hash>.json`; `<state-path>.selection.json` stores the last plan choice. `--state` isolates this entire family of files, not just the base state. Selecting a plan validates and retrieves its assignment before committing its selection; a failed switch reports an error and returns to the previous plan.

## Prototype boundaries

- The source catalog covers the bundled editions' complete book lists, including source-specific variants. Updating an edition requires regenerating source data/indexes and the catalog, then rerunning source parity verification.
- Alignment currently matches explicit book/chapter/verse labels, never output-row indices. Missing labels in a returned passage appear as placeholders.
- A curated cross-source map for differently numbered or divided verses is not implemented. Equal numbers do not prove equal textual boundaries; both TUI and dump mode warn about this. Do not treat the prototype as a verified full textual concordance.
- Library, mode, passage, plan/day selection, and pane visibility are in-app. Plan editing remains JSON-based; there is no in-app configuration editor yet.
- Passage selection is synchronous and scans indexed spans of the embedded text for each available translation, even if its pane is hidden. No external sources or extraction directories are required.
- Viewport positions are saved on normal exit and handled termination; confirmed completion is saved immediately. Forced termination such as SIGKILL cannot save unsaved navigation.
- Full grapheme-cluster/emoji rendering is not claimed. Greek UTF-8 and combining-mark wrapping have unit coverage; libc supplies display-cell widths in the terminal locale.

## Verification performed

`zig build test -Dtarget=x86_64-linux-musl` passed 70 Zig tests in both ReleaseSafe and ReleaseSmall, covering parsing, bundled retrieval and reference selection, input framing, bounded numeric/stale selections, layout, plan validation and boundaries, calendar completion rules, importing/repositioning progress, catalogs, isolated state paths, picker navigation, supported-title metadata, adjacent chapters, verse anchoring, the single visible Bible listing, and mapping legacy book/chapter selections.

`python tests/all-source-books.py --chapters` verified every complete book listed by the original sources: 234 source/book combinations across 89 canonical books, comparing 108,305 distinct source verse labels/texts and 1,479 chapter queries. Kata runs with no applets on PATH; original tools are verification oracles only. Repeated Greek labels preserve all record text in order. No progress is written. `tests/reference-parity.py` separately compares lists, ranges, cross-chapter selection, and malformed-reference rejection.

`tests/bundle-standalone.py` copies only the executable outside the repository, isolates HOME/config/state, and clears PATH. It verifies dump mode, full-library terminal workflows, built-in reading plans, embedded licenses/provenance, and input stress with no source tools. `tests/bundle_sources.py --verify` checks exact original archive-member bytes and generated indexes. The Linux release is stripped and statically linked with musl; raw text is embedded uncompressed.

`python tests/full-library-pty.py` verified the size-optimized executable from outside the repository: full-library startup, Genesis reading and chapter navigation, Greek-only books and variants, source-specific names, Sirach's chapter-zero prologue, bookmark restoration, preserved plan counters, and terminal cleanup. `tests/plan-regression.py` and `tests/cli-regression.py` retain the earlier plan/import checks. These checks use isolated temporary state/config, not normal user progress.

A temporary Python PTY probe exercised the actual built Zig application against all three real sources. It verified retrieval for the first chapter of all 27 configured books, daily chapter assembly, linked/independent scrolling, position restoration, source toggles, resizing and narrow terminals, SIGTERM cleanup, confirmation/cancellation, duplicate completion blocking, missed-day behavior, cycle repetition, and incompatible-plan protection. Python is verification tooling only; the application is Zig.

A separate temporary CLI probe verified read-only start-day previews, confirmed state readback, real John 20/Revelation 21 retrieval after importing day 88, normal completion and next-day progression, invalid/conflicting argument protection, retained repeat cycles and pane preferences, backward repositioning, and restarting a completed nonrepeating plan.

A temporary in-app PTY probe verified plan/day menus, cancellation, invalid-day protection, confirmed day-88 import, independent plan progress and restored positions, remembered selection, custom-plan discovery/completion, restoring a custom plan from outside the repository, restarting a finished nonrepeating plan, menus in narrow terminals, and terminal cleanup. Session changes keep one application screen active instead of returning briefly to the shell.

A library PTY probe verified startup title/mode choices, free chapter/verse selection using actual source text, invalid chapters, starting-verse anchors, previous/next chapter navigation, remembered free-reading location, switching back to the existing plan view, and separate free/plan state. Quitting or terminating on the start menu restores the terminal without creating progress. All probes use isolated state/config files.
