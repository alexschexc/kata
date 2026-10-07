# Kata prototype

Kata is a Zig terminal reader backed by the installed `kjv`, `grb`, and `vul` commands. It loads their real output, not bundled substitute text.

## Build and review

Requirements: Linux/POSIX terminal, Zig 0.16.0, libc, and all three source commands on `PATH`. The build uses Zig's LLVM/LLD backend; this avoids the native linker's incompatibility with the host's newer glibc SFrame relocations.

From this directory:

```sh
zig build
zig build test --summary all
./zig-out/bin/kata --passage 'John:1'
```

Start the daily reading plan:

```sh
./zig-out/bin/kata
```

For a review without touching your usual progress, select a separate state file:

```sh
./zig-out/bin/kata --state /tmp/kata-review-state.json
```

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
| `j` / `k` | Scroll down/up one rendered line |
| `Ctrl-d` / `Ctrl-u` (also `f` / `b`) | Scroll half a screen |
| `g` / `G` | Beginning/end of the session |
| `h` / `l`, or `Tab` | Focus another visible pane |
| `s` | Toggle linked and independent scrolling |
| `1` / `2` / `3` | Show/hide KJV, Greek, and Latin panes |
| `p` | Open the in-app plan picker; `j`/`k`, then `Enter` selects |
| `d` | Open the in-app day picker; browse or type a number, `Enter` previews, `y` confirms |
| `Esc` / `q` in a picker | Cancel and return to the reader |
| `c`, then `y` | Explicitly confirm today's completion in plan mode |
| `q`, or `Ctrl-c` | Save position and leave the TUI |

Corresponding verse labels share a row height, with padding under shorter translations. Linked scrolling operates over this common layout; independent scrolling moves only the focused pane. Resizing rewraps text while preserving verse-row anchors. The terminal's original input mode and screen are restored on exit and handled termination signals.

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

- The source adapter currently supports the New Testament books and its listed aliases. Other texts and Old Testament books are not implemented yet.
- Alignment currently matches explicit book/chapter/verse labels, never output-row indices. Missing labels in a returned passage appear as placeholders.
- A curated cross-source map for differently numbered or divided verses is not implemented. Equal numbers do not prove equal textual boundaries; both TUI and dump mode warn about this. Do not treat the prototype as a verified full textual concordance.
- Plan/day selection and pane visibility are in-app. Plan editing remains JSON-based; there is no in-app passage picker or configuration editor yet.
- Passage fetching is synchronous and requires all three source tools, even if a pane is hidden.
- Viewport positions are saved on normal exit and handled termination; confirmed completion is saved immediately. Forced termination such as SIGKILL cannot save unsaved navigation.
- Full grapheme-cluster/emoji rendering is not claimed. Greek UTF-8 and combining-mark wrapping have unit coverage; libc supplies display-cell widths in the terminal locale.

## Verification performed

`zig build test` passed 35 Zig tests covering parsing, layout, plan validation and boundaries, allocation cleanup, calendar completion rules, importing/repositioning progress, plan catalog deduplication, isolated state paths, and picker navigation/confirmation.

A temporary Python PTY probe exercised the actual built Zig application against all three real sources. It verified retrieval for the first chapter of all 27 configured books, daily chapter assembly, linked/independent scrolling, position restoration, source toggles, resizing and narrow terminals, SIGTERM cleanup, confirmation/cancellation, duplicate completion blocking, missed-day behavior, cycle repetition, and incompatible-plan protection. Python is verification tooling only; the application is Zig.

A separate temporary CLI probe verified read-only start-day previews, confirmed state readback, real John 20/Revelation 21 retrieval after importing day 88, normal completion and next-day progression, invalid/conflicting argument protection, retained repeat cycles and pane preferences, backward repositioning, and restarting a completed nonrepeating plan.

A temporary in-app PTY probe verified plan/day menus, cancellation, invalid-day protection, confirmed day-88 import, independent plan progress and restored positions, remembered selection, custom-plan discovery/completion, restoring a custom plan from outside the repository, restarting a finished nonrepeating plan, menus in narrow terminals, and terminal cleanup. Session changes keep one application screen active instead of returning briefly to the shell.
