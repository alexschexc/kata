# Kata — Initial Design Brief

Status: concept with initial source inspection complete; no Kata prototype exists. Implementation language: Zig. Installed compiler verified: Zig 0.16.0.

This brief records the user's requirements; it is not an implementation plan or a claim about the reference tools' capabilities.

## Purpose

An attractive terminal UI for reading multiple translations of the same text in parallel. The initial use case is reading text obtained from Luke Smith's installed terminal Bible applications: kjv, grb, and vul.

Confirmed executable locations using `which kjv grb vul`:

- kjv: /usr/local/bin/kjv
- grb: /usr/bin/grb
- vul: /usr/bin/vul

The exact command interfaces, output formats, and available texts must still be inspected rather than assumed. The first version targets these three sources. Support for other texts is a later extension and may require a comparable retrieval interface.

## Reading experience

- Let the reader select the text or passage they want to read.
- Display multiple translations in side-by-side panes.
- Support independent pane scrolling, with an option to scroll all panes together.
- For the initial Bible texts, align translations by verse. Linked scrolling must use verse identity rather than equal rendered-line offsets.
- Use shared verse-row heights across panes: the tallest wrapped translation determines each row's height, with padding beneath shorter translations. In linked mode, corresponding verses remain visually beside one another.
- Missing or differently numbered verses require explicit mapping; never align sources merely by the nth verse in their output. The mapping policy and data remain to be established.
- Provide keyboard-driven, Vim-style navigation.
- Provide a long scrolling reading buffer for each day's assigned readings.
- A day's buffer should contain the actual assigned passages, not just references or links that require opening each passage separately.

The side-by-side layout, optional linked scrolling, and verse-based alignment for the initial Bible texts are confirmed. Key bindings, navigation granularity, and handling of missing or differently numbered verses are not yet specified.

## Custom reading plans

Readers should be able to configure how quickly they progress through a text.

Plans must support irregular schedules, not only a fixed number of chapters per day or a uniform division by a target completion date.

### Required example: Rule of the Optina Fathers

As described by the user:

- Normally read one chapter of the Gospels and two chapters of the Epistles each day.
- In the final phase, covering the last seven chapters of John and Revelation, switch to one chapter of John and one chapter of Revelation each day.
- The two reading sets should finish together.

Confirmed streams: Gospels, and Acts → Epistles → Revelation. The plan repeats automatically after both streams finish. The user believes the final assignment is John 21 and Revelation 22; the next cycle starts with Matthew 1 and Acts 1–2. Validate the exact phase boundary against the actual chapter sequences before implementing. Do not independently alter the user's rule.

### Calendar behavior and completion

- Plans are calendar-day based.
- Missing a day shifts its unfinished assignment to the next day and shifts subsequent assignments accordingly. There are no automatic catch-up readings or doubled workloads.
- Completion is explicit: the reader manually marks a day's assignment complete. Reaching the bottom of the buffer does not mark completion.
- Persist plan progress, selected translations, and reading-buffer position across sessions.

### Plan authoring

- Start with an editable configuration file.
- Add in-tool configuration later; it should represent the same plan model rather than a separate scheduling system.

## Daily reading session

Given a plan and a selected plan day:

1. Resolve that day's assignments across all active reading streams.
2. Obtain the corresponding text in the selected translations.
3. Assemble the readings into one continuous scrolling session.
4. Make passage boundaries and translation identities clear in the UI.

Steps 1–3 are required behaviors. The precise presentation of step 4 needs design discussion.

## Useful design distinction

Keep these responsibilities conceptually separate:

- Text source: retrieve requested passages from the reference applications.
- Plan scheduling: determine which passages belong to a plan day.
- Session assembly: combine those passages into a day's reading content.
- TUI: display and navigate that content.

This is a proposed architectural boundary, not a mandate to create particular modules or abstractions before inspecting the sources.

## Questions for the next session

- How should missing verses or differences in verse numbering across translations be represented?
- Validate the precise phase transition and cycle alignment for the Optina schedule using the actual chapter counts and ordered streams.
- Decide the configuration format and storage locations for configuration and persistent state.

These are open questions, not additional agreed requirements.

## Starting point for the next session

Inspect kjv, grb, and vul and their real output interfaces. Then agree on the smallest vertical slice that retrieves a passage and displays it in a navigable parallel reading view. Validate the irregular plan schedule separately before connecting it to the daily buffer.

## Verified source interfaces and schedule

Evidence: Kata-source-inspection.json in the same directory. The probe retrieved all 27 New Testament books from each installed source and compared chapter counts and verse labels.

- All three tools provide `-l` for book listings and `-W` for unwrapped output.
- Example query: `kjv -W John:1:1-3`; the corresponding grb and vul queries work too.
- Captured output starts with a book heading, followed by rows of `chapter:verse`, a tab, and UTF-8 text.
- Invalid references can return exit code 0 and an `Unknown reference:` message on stdout. Output validation is mandatory.
- The advertised abbreviation `Phi` matches both Philippians and Philemon. Use an unambiguous query such as `Philippians` and retain book identity when parsing headings.
- All three sources agree on chapter counts: 89 Gospel chapters and 171 chapters from Acts through Revelation.
- The planned cycle is exactly 89 completed daily assignments: 82 assignments at rates 1 and 2, followed by 7 assignments at rates 1 and 1.
- Assignment 1: Matthew 1; Acts 1–2.
- Assignment 82: John 14; Revelation 14–15.
- Assignment 83: John 15; Revelation 16.
- Assignment 89: John 21; Revelation 22. The next assignment restarts the cycle.
- The generated schedule was checked for complete, ordered coverage of both chapter streams, with no skipped or repeated chapters within a cycle.
- Verse labels differ between sources, including omissions and shifted chapter boundaries. These are label differences, not automatically proof of omitted text; correspondence needs explicit mapping.

Proposed first Zig slice: retrieve a short passage from all three tools, validate and parse it into source-aware verse records, then display a minimal three-pane reading view. Test retrieval/parsing before adding terminal interaction. Keep scheduling and persistent calendar state separate from this first slice.
