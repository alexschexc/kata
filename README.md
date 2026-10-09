# kata

Zig terminal reader with the complete installed `kjv`, `grb`, and `vul` text datasets embedded in one executable, optional linked scrolling, and configurable daily reading plans. No source applets, shell utilities, Python, or network are needed at runtime.

Build: `zig build`

Read a passage: `./zig-out/bin/kata --passage 'John:1'`

Open the library: `./zig-out/bin/kata`, then choose a title and **Free reading** or **Reading plans**.

Inside the application: `p` chooses a plan; `d` chooses a reading day and previews importing progress. Use `j`/`k` and `Enter` in pickers, then `y` to confirm a day change. Each plan keeps its own progress, and the selected plan is remembered.

`/` searches the focused translation for a word or phrase (case- and accent-insensitive), highlighting matches and listing every verse in a results panel on the right; `Enter` jumps to a result and the panel stays open. See PROTOTYPE.md › Word search.

EPUB books: on first launch Kata asks where your `kataIngest` (EPUBs to read from) and `kataLibrary` (converted books) folders should live — absolute paths, `~/` expanded, nothing guessed. Each EPUB in the ingest folder becomes a readable document of the same name (without `.epub`) in the library folder; converted books appear in the Library after Bible. Startup only converts new or changed EPUBs (a manifest tracks size/time/SHA-256 and the converter version). `kata ingest` runs the same pipeline from a shell (`--force` reconverts, `--ingest-dir`/`--library-dir` set the folders); the last Library entry changes them. DRM-encrypted EPUBs are refused. In a book: `j`/`k` (or `↑`/`↓`) scroll, `H`/`L` (or `Shift+←`/`Shift+→`) chapters, `t` chapter list, `/` search with the same results panel, `m` library. See docs/ingest-plan.md.

Free reading has book/chapter/verse selection, `H`/`L` (or `Shift+←`/`Shift+→`) chapter navigation; arrow keys work like `h`/`j`/`k`/`l`, and positions saved separately from plan progress. `m` returns to the library; `o` chooses another free-reading place. Choose **Bible** for the full combined book collection of the installed `kjv`, `grb`, and `vul` sources, including Old Testament, additional books, and Greek textual variants. The duplicate New Testament listing is hidden; legacy metadata remains compatible, and old book/chapter selections map into Bible. Sources without a selected book/chapter leave their pane empty; numbering variants are not inferred.

Standalone x86-64 Linux executable: `zig build -Dtarget=x86_64-linux-musl -Doptimize=ReleaseSmall -Dstrip=true`. This is statically linked, with the original source text embedded uncompressed and queried directly in Zig.

Unknown keys, unsupported terminal sequences, bracketed paste, and invalid numeric selections are ignored silently. `kata --licenses` shows embedded source credits and redistribution notes. See [PACKAGING.md](PACKAGING.md) for provenance, standalone verification, and cross-platform boundaries.

Run unit tests: `zig build test --summary all`

Install for the app launcher: `scripts/install-local.sh` builds the standalone release, smoke-tests it, and atomically replaces `~/.local/bin/kata` (the path `kata.desktop` runs). The tracked `zig-out/` binary is not touched.

See [PROTOTYPE.md](PROTOTYPE.md) for controls, configuration, verification, and known limitations—including incomplete cross-source verse-number mapping.

License: see [LICENSE](LICENSE); embedded source text credits via `kata --licenses`.
