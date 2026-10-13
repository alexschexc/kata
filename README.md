# Kata

Kata is a terminal reader written in Zig. It shows parallel texts side by side, keeps daily reading plans, and reads your own EPUB books. It is one self-contained executable: no network, Python, or other runtime is needed.

Status: beta (v0.3.1). Linux is the tested platform. The macOS (Apple silicon) and Windows (x64) builds are cross-compiled and have not been verified on real machines.

## What it does

- **Parallel Bible reading.** The complete installed `kjv`, `grb` (Greek) and `vul` (Latin) texts are embedded in the binary and shown in three columns, with linked or independent scrolling. 89 canonical book entries, including the additional books and Greek textual variants.
- **Psalms in Greek/Latin numbering.** The KJV column (Hebrew numbering) is placed under the matching Greek/Latin psalm so all three columns line up.
- **Reading plans.** Built-in plans (Optina, one Gospel chapter a day) plus custom JSON plans. Every plan, for any book, is listed under Library → Reading plans with what it covers; you can also reach plans from the Bible and from each book. Each plan keeps its own progress, you can import progress from a print reading plan by choosing a day, and `c` marks the day complete from inside any plan reading. Plans you no longer want can be removed with `x`; their saved progress is kept.
- **Plan builder.** Build a plan from the Bible, from any converted book, or from both at once, with presets or a custom length. Plans can run two streams in parallel.
- **Your own books.** Point Kata at a folder of EPUBs; it converts them to a readable format and lists them in the Library. Verse numbers, page labels, code blocks, tables, footnotes and figures are kept.
- **Menu filter.** In any menu, such as the Library, `/` filters the list as you type. On the Library it matches title and author.
- **Search.** `/` searches the focused translation, or the open book, with highlighted matches and a results panel. Library → "Search all books" searches every converted book.
- **Inline images.** Book figures display with the Kitty graphics protocol or Sixel when the terminal supports them; otherwise a placeholder is shown, and `i` opens the image in your system viewer. `KATA_IMAGES=off` disables the terminal query.
- **Colour.** Kata uses 24-bit colour. macOS Terminal.app does not support it, so Kata detects it (`TERM_PROGRAM=Apple_Terminal`) and switches to the 256-colour palette. `KATA_COLORS=256` or `KATA_COLORS=truecolor` forces either mode.
- **Psalter edition switch.** In the Psalms, `e` swaps column 1 between the KJV and an ingested Psalter (the first Library title containing "psalter"). The two editions divide verses differently in many psalms, so verses can drift within a psalm; there is no per-psalm verse map yet.

## Install and run

Download the build for your system from the GitLab release page.

- **Linux (x86-64):** the static `kata` binary. Put it on your `PATH` and run `kata`.
- **macOS (Apple silicon):** unpack `kata-v0.3.1-macos-aarch64.tar.gz` and double-click `Run kata.command`. The binary is not Developer ID signed or notarized, so macOS download policies may block it until you allow it; this has not been tested on a Mac.
- **Windows (x64):** run `kata.exe` in a terminal.

From source (Zig 0.16.0, pinned in `mise.toml`):

```sh
zig build                      # debug build to zig-out/bin/kata
zig build test --summary all   # unit tests
./zig-out/bin/kata
```

Shipping builds (static, stripped):

```sh
zig build -Dtarget=x86_64-linux-musl -Doptimize=ReleaseSmall -Dstrip=true
zig build -Dtarget=aarch64-macos     -Doptimize=ReleaseSmall -Dstrip=true --prefix zig-out/macos-arm64
zig build -Dtarget=x86_64-windows    -Doptimize=ReleaseSmall -Dstrip=true --prefix zig-out/windows-x64
```

`zig-out/bin/kata` is tracked in git and is meant to hold the release Linux build; a plain `zig build` overwrites it with a debug build. `scripts/install-local.sh` builds the static release into a scratch directory, smoke-tests it, and replaces `~/.local/bin/kata` without touching `zig-out/`.

## Using it

Run `kata` to open the **Library**: the Bible, Reading plans, your converted books, Search all books, and Ingest folders. The Bible and each book have their own menu for free reading and their plans. The colors follow the Omarchy Ethereal theme.

| Keys | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | Scroll |
| `Ctrl-d` / `Ctrl-u` | Half page |
| `g` / `G` | Start / end |
| `h` / `l`, `←` / `→`, `Tab` | Focus another column |
| `s` | Linked / independent scrolling |
| `1` `2` `3` | Show or hide KJV, Greek, Latin |
| `H` / `L`, `Shift+←` / `Shift+→` | Previous / next chapter |
| `o` | Pick a book, chapter and verse (free reading) |
| `p` / `d` | Pick a plan / pick a plan day |
| `c`, then `y` | Mark today complete (in any plan reading, Bible or book) |
| `e` | Switch column 1 between KJV and the ingested Psalter (Psalms only) |
| `/` | Search; `Enter` runs it, `r` focuses results, `n` / `N` next / previous, `x` closes |
| `/` in a menu | Filter the list; `Enter` keeps it, `Esc` clears it |
| `x` in Reading plans | Remove the selected plan (`y` confirms) |
| `m` | Back to the Library |
| `q`, `Ctrl-c` | Save and quit |

In a book: `j`/`k` scroll, `H`/`L` change chapter, `t` opens the chapter list, `/` searches the book, `i` opens an image, `m` returns to the Library.

## Your own EPUBs

On first launch Kata asks for two folders. Absolute paths are required, `~/` is expanded, and nothing is guessed. Press Tab in the prompt to browse for a folder.

- **Ingest folder:** where Kata reads `.epub` files. It never writes here.
- **Library folder:** where converted books and their images are stored.

Your choice is saved in `<config>/kata/folders.json` (normally `~/.config/kata/folders.json`) and can be changed from the last Library entry. At startup only new or changed EPUBs are converted. A manifest records each file's size, modification time and SHA-256 together with the converter version. Converted books whose EPUB was removed are kept and reported, never deleted. DRM-protected EPUBs are refused.

```sh
kata ingest                  # convert new/changed EPUBs, print a per-book report
kata ingest --force          # reconvert everything
kata ingest --ingest-dir P --library-dir P          # use these folders for this run only
kata ingest --ingest-dir P --library-dir P --save   # ...and remember them
```

## Other commands

```sh
kata --passage 'John:1:1-3' --dump   # plain text, no state written
kata --plan config/optina.json       # use a custom JSON plan
kata --check-plan                    # print the complete plan cycle
kata --start-day N [--confirm]       # preview, or save, importing progress up to day N
kata --state PATH                    # isolated state (separate progress files)
kata --detect-graphics               # report inline-image support
kata --licenses                      # embedded text credits and license notices
```

Custom plans are JSON files found in `config/` of the current directory and in `$XDG_CONFIG_HOME/kata/plans/`.

## Known limits

- Verse numbers are matched by label across KJV, Greek and Latin. Outside the Psalms alignment above, there is no curated map for differently divided verses.
- Running headers and footers are not stripped from converted books (this affects PDF-derived EPUBs).
- SVG, GIF and WebP images, and interlaced (Adam7) PNGs, are not supported.
- Custom plans are JSON files or come from the plan builder; there is no general in-app plan editor.
- Position is saved on normal exit; `SIGKILL` cannot save unsaved navigation.
- Full emoji and grapheme-cluster rendering is not claimed.

## More

- [PROTOTYPE.md](PROTOTYPE.md): detailed behaviour, state files and verification notes
- [docs/ingest-plan.md](docs/ingest-plan.md): EPUB conversion design and status
- [PACKAGING.md](PACKAGING.md): provenance and cross-platform notes

License: [LICENSE](LICENSE) (MIT No Attribution). Embedded source text credits: `kata --licenses`.
