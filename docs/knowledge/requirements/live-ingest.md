# Live ingest

## Goal

A reader who adds an EPUB to the ingest folder while Kata is open can press one key to convert it and see it in the Library, without restarting Kata or losing their place.

## Requirements

- R1: Ctrl+R starts an ingest run from the Library screen and from any reader screen (Bible reading, plan day, ingested book).
- R2: The run does what the startup check does: it converts only new or changed EPUBs, never forces a re-conversion.
- R3: The run happens in the background. The screen keeps responding, and the reader keeps scrolling, searching and saving progress.
- R4: Starting a run shows a notice on the current screen. Finishing shows a second notice with the counts (converted, failed, unchanged).
- R5: When a run finishes, the Library list includes the new books. If the Library is open, it refreshes at once; otherwise it is refreshed the next time the Library opens.
- R6: Pressing Ctrl+R while a run is going starts nothing and says a run is already going.
- R7: Pressing Ctrl+R with no ingest folders chosen starts nothing and says to choose them in the Library.
- R8: A failed run keeps the existing library and shows the error in a notice.

## Edge cases

- Quitting during a run ends Kata at once. A half-written conversion is a `.tmp` file that ingest already ignores, and the next run redoes it.
- The reader of a book being re-converted keeps its open text; the new text shows next time it is opened.
- A run that finishes while a prompt or picker is open shows its notice on the next screen that has a notice line.

## Out of scope

- A forced re-ingest (`--force`) from inside Kata.
- Watching the folder automatically.
- A progress bar.
- Ingest from the plan hub or the folder picker having its own key (the key is ignored there).

## Done when

- With Kata open at the Library, copying an EPUB into the ingest folder and pressing Ctrl+R shows a start notice, then a finish notice, and the book is listed without a restart.
- The same works from inside a Bible reading and inside an ingested book, and the Library lists the book after `m`.
- A second Ctrl+R during a run says one is already going.
- With no folders chosen, Ctrl+R says so.

## Open questions

(none)

## Decisions

- Where: Library and while reading. Asked, user chose.
- Key: Ctrl+R. Asked, user chose.
- Scope of run: new or changed only. Asked, user chose.
- Background, not blocking. Asked, user chose.
- Notice wording and the "ignored in prompts" edge: assistant's choice, not asked.
