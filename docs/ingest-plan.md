# EPUB ingest — development plan (next push)

Status (2026-10-09 evening): ingest WORKS end to end and is installed.
106/106 unit tests; tests/ingest-pty.py + all other suites pass.

Done (files):
- `src/epub.zig` — in-memory zip + deflate, container/OPF/spine/TOC, DRM check
  (font obfuscation allowed; other encryption.xml methods / rights.xml refused).
- `src/xhtml.zig` — tolerant tokenizer (comments, CDATA, PIs, quoted `>`,
  namespaced tags, entities).
- `src/convert.zig` — XHTML → blocks. Generic rules: h1–h6, p, verse /
  superscription / liturgical classes, pre (whitespace kept), li, dt/dd, tr
  (cells joined with " │ "), figure (caption or "[image: alt]"), footnotes,
  sup → [n], blockquote/aside/sidebar/note → quote, br → newline. Skipped:
  head/script/style/svg/math/nav, index & toc sections, ornaments, indexterm
  anchors. Page labels from epub:type=pagebreak/role=doc-pagebreak. Verse
  numbers inferred per heading and corrected by explicit `verse-number`
  spans (Psalter: 0 mismatches, Psalm 1 = 6 verses, Psalm 118 = 176).
- `src/document.zig` — on-disk format `KATA-DOC 1` (TSV, escaped text),
  write + parse, round-trip tested.
- `src/ingest.zig` — folders.json (config/kata), normalizeFolder (~/ expand,
  absolute required), run(): size+mtime → sha256 → converter version; atomic
  writes; manifest `.kata-manifest.json`, index `.kata-index.json` (reuses
  unchanged entries); orphans reported, never deleted.
- `src/book_reader.zig` — single-pane reader: verse gutter, page rules, code
  colour/indent, `[`/`]` chapters, `t` chapter picker, `/` search + panel +
  highlight + n/N, position saved to `<state>.books/<hash>.json`.
- `src/app.zig` — first-run folder prompt (tui.prompt), startup ingest check
  (only new/changed), books listed in Library after Bible, last Library
  entry "Ingest folders" to change them.
- `src/cli.zig` — `kata ingest [--force] [--ingest-dir P --library-dir P]`.
- All 10 examplePubs convert without errors (5.9 s cold, ~0.02 s cached).

Known gaps / next ideas:
- Front matter (dedication, title pages) can render as many one-line
  paragraphs where the source uses a `<p>` per line; could merge short
  consecutive `prose`/`quote` lines.
- No Bible-style parallel alignment of the Psalter (could become a 4th Psalms
  pane later; it is already LXX-numbered).
- Repeated running header/footer stripping not implemented (no sample needs
  it; PDF-converted EPUBs would).
- Tables are flattened to rows. SVG/GIF/WebP images stay placeholders.
- Zip64 not supported. Search in a book is per-document only.
- Folder prompt has no file browser; paths are typed (paste works).

## Inline images — stage 4 DONE (Sixel), 2026-10-09

- `src/png.zig`: PNG decoder (all colour types/depths, 5 filters, tRNS,
  alpha over white), area-average `scale`. Rejects Adam7. All BMS PNGs decode.
- `src/sixel.zig`: per-image adaptive palette (5-bit buckets, top 256 by
  frequency, rest → nearest), RLE bands. Verified against libsixel
  `sixel2png`: Figure 4-9 RMSE ≈ 0.058, 8 ms encode, ~50 KB.
- `src/inline_image.zig`: `fit` (≤ text column, ≤ ¾ body height, never
  enlarge), cache of decoded+scaled bitmaps (48 MB cap, cleared when full),
  `draw` encodes only the visible rows in whole 6-px bands.
- `book_reader.zig`: image blocks reserve rows when sixel is available, are
  painted after the text frame from their first visible row (cropped when
  scrolled), placeholder otherwise. Key bursts coalesce (`tui.inputPending`)
  so held `j` does not queue a frame per key.
- `graphics.zig`: also asks `CSI 14t` (text area px) and derives the cell
  size when `CSI 16t` is unsupported. Detection runs once in app.run (300 ms
  cap); `KATA_IMAGES=off` disables it.
- Tests: png/sixel/inline_image unit tests; `tests/sixel-pty.py` (foot-like
  replies, placement, crop, libsixel decode, no sixel without support,
  coalescing).
- Verified in a real foot window (scale 1.6): cell size must come from the
  window-size ioctl, not CSI 16t; frames use synchronized update (?2026).
- `src/jpeg.zig`: baseline JPEG (Huffman, any sampling, restarts, gray,
  YCbCr, Adobe CMYK/YCCK); matches ImageMagick within 0.3% RMSE. Progressive
  (SOF2, 5 of 583 samples) also decoded (2026-10-10). ICC profiles ignored.
- Kitty graphics (2026-10-10): preferred when the terminal answers the
  `a=q` probe (Ghostty, kitty, WezTerm). Each image is transmitted once
  (RGB, chunked base64, q=2) and placed per frame with a source-rect crop
  (`y=`,`h=`,`r=`); every frame starts with `a=d,d=a` so scrolling leaves
  no stale placements. Ghostty: ED2 (`CSI 2J`) deletes visible
  placements AND images left unused, so kitty frames erase line by line
  (`CSI 2K`) instead; found when images vanished on first scroll in real
  Ghostty (2026-10-10). Fix confirmed by the user in real Ghostty.
- Next ideas: `+`/`-` to change image
  size; skip sixel re-encode of unchanged images on redraw.

## Images (2026-10-09, stages 1–3 done)

Done:
1. Ingest copies images: `<library>/<book>.assets/<zip_path_with_underscores>`
   (written before the document; shared images copied once). Blocks of kind
   `image` hold the asset file name. `img` inside figures or outside any block
   become image blocks; inline icons in text are ignored. Converter v2 →
   existing books re-ingest automatically. BMS: 185 image blocks, 179 files.
2. Book reader shows `▣ image · i opens it (name)`; `i` opens the image on
   screen (else nearest above in the chapter) via `xdg-open` / `open` / `start`,
   detached (pgid 0, stdio ignored). Tested with a fake xdg-open in ingest-pty.
3. `src/graphics.zig`: capability query = kitty APC `a=q` + `CSI 16t` (cell px)
   + DA1 `CSI c` (always answered, terminates the wait). DA1 attribute `4` →
   sixel; kitty `;OK` → kitty (preferred). `tui.detectGraphics(ms)`;
   `kata --detect-graphics` prints the result. Simulated foot reply → sixel
   10×20; silent terminal → gives up in ~0.5 s. NOT yet called at app
   startup and NOT yet verified in the user's real foot window — ask the user
   to run `kata --detect-graphics` in foot first.

Stage 4 plan — inline Sixel (user's terminal: foot; Ghostty → kitty later):
- Call detectGraphics(300) once in app.run right after Terminal.init (before
  the first menu); pass Capabilities into book_reader.run.
- PNG decoder (src/png.zig): zlib via std.compress.flate (container .zlib),
  filters 0–4, color types 0/2/3/4/6, bit depth 8 (+1/2/4 palette), no
  interlace first (fall back to placeholder for Adam7). JPEG: placeholder.
- Scale to text column width in px (cols × cell_width), cap height ≈ ½ screen;
  box/area-average downscale. Quantize: fixed 6×6×6 cube + 16 grays (no
  dithering first) → sixel palette; encode with RLE (`!n`).
- Reader layout: an image block reserves ceil(h_px / cell_height) blank lines;
  draw the sixel at its first visible row after the text frame. Partially
  scrolled images: crop rows to the visible band (re-encode the cropped band;
  cache full decoded+scaled RGB per (asset,width), encode per visible band).
- Cache: decoded/scaled bitmap in memory LRU (~32 MB). Keep `i` fallback.
- Kitty (later, Ghostty): `a=T,f=100` send PNG base64 once per id, place with
  `a=p`; crop with source rect x,y,w,h. No decoder needed.
- Tests: png.zig unit tests on 2 sample PNGs (known dims/pixels), sixel encoder
  golden on a tiny image, PTY test asserting `ESC P q` emitted only when DA1
  reply includes 4.

## Agreed requirements (from the user)

- Source folder of EPUBs → one converted document per EPUB in the library
  folder, same name minus `.epub` (e.g. `kataLibrary/buildingmicroservices2ndedition`).
- Then index every document in the library folder; indexed docs appear in the
  Kata library menu and are readable like the embedded Bible.
- Cache: a manifest so unchanged books are neither re-converted nor re-indexed;
  startup must stay fast.
- Folder names `kataIngest/` and `kataLibrary/`. Locations are ASKED of the
  user on first run (cross-platform: no guessed OS paths), saved in
  `<config>/kata/folders.json`, changeable in-app, overridable by flags.
  For development the user's samples live in repo `examplePubs/`.
- Both an explicit `kata ingest` command and a quick startup check that only
  converts new/changed EPUBs.
- Strip non-meaningful formatting (repeated running headers/footers, author/
  title boilerplate). Keep: chapters, headings, paragraphs, verse numbers,
  print page numbers (non-religious texts), code layout, lists.
- Not embedded in the binary. Refuse DRM (`META-INF/encryption.xml` with
  non-font-obfuscation algorithms).
- Targets for this push ONLY these two:
  1. `examplePubs/The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub`
  2. `examplePubs/buildingmicroservices2ndedition.epub`
  The other examplePubs are later.

## What the samples actually contain (inspected 2026-10-09)

Psalter (EPUB 3.0, 33 spine items, OEBPS/text/*.xhtml, nav.xhtml + toc.ncx):
- One file per kathisma (`kathisma-01.xhtml` …), `<h1>The First Kathisma</h1>`.
- Psalms: `<h2 id="psalm-1">Psalm I. 1</h2>` (Roman + Arabic, LXX numbering).
- `<p class="superscription">` headings; `<p class="verse">` = one verse each.
- Verse numbers are MOSTLY IMPLICIT: only every 5th has
  `<span class="verse-number">5</span>`. Infer by counting `p.verse` per psalm
  and cross-check against explicit numbers (fail loudly on mismatch).
- `<span class="dropcap">B</span>lessed` — join dropcap into the word.
- Decorative `<svg>` ornaments (class ornament/tailpiece): drop.
- Page breaks: `<span epub:type="pagebreak" role="doc-pagebreak" aria-label="26">` (468).
- Other classes: stasis, rubric, refrain, response, chant, day, glossary,
  appendix, note-marker, liturgical, prose. 7 tables, 19 imgs, 1 footnote.
- Bonus later: since it is LXX-numbered, it could align as a 4th Psalms pane.

Building Microservices (EPUB 2 / OPF 1.0, 29 spine items, `.html`, toc.ncx only):
- O'Reilly markup: `<section data-type="chapter">`, `<h1>`–`<h6>`,
  `<pre>` code (15) with pygments span classes (k, kc, kd, kn…) → keep text,
  preserve whitespace exactly; `<figure>` + `<img>` (199) → `[Figure 4-9: caption]`;
  `<aside data-type="sidebar">` → quoted block; footnotes (314,
  `data-type="noteref"`/`footnote`) → numbered notes at chapter end;
  `<a data-type="indexterm">` empty anchors → drop; index file `ix01.html` → skip
  or keep last. Only 44 explicit page markers; `pagebreak-before` CSS class is
  NOT a page number.
- No running headers/footers present in either sample (they are reflowable,
  not PDF conversions), so header/footer stripping is a heuristic to add but
  not the main work.

## Build order for the push

1. Folders: `loadFolders`/save; first-run TUI prompt for both paths (create
   dirs on confirm); `--ingest-dir`/`--library-dir` flags; in-app change.
2. EPUB reading: `std.zip` (0.16: `std.zip.Iterator` over `File.Reader`,
   `Decompress` for deflate) → `META-INF/container.xml` → OPF manifest/spine
   → nav.xhtml or toc.ncx for chapter titles. Encryption check.
3. Minimal tolerant XHTML tokenizer (tags, attrs, entities incl. numeric,
   CDATA, comments; no DTD). Block builder: element → `Block.Kind` rules in
   a small table, with per-book overrides only if unavoidable.
4. Psalter rules: verse inference + validation; superscription; page labels.
5. O'Reilly rules: pre/code, figures, sidebars, footnotes, tables (simple
   pipe rows), lists.
6. Output file format (decide first thing): line-oriented TSV like the Bible
   data — `chapter<TAB>locator<TAB>kind<TAB>text`, plus a header block with
   title/author/source fingerprint/converter version. Keeps reuse of search
   (`search.zig`) and the reader simple.
7. Manifest + `plan`: size+mtime first, sha256 when they differ;
   converter_version bump forces reconvert; orphans reported, never deleted.
8. `kata ingest` command (prints convert/skip/orphan summary) and startup
   check (convert only new/changed; show notice).
9. Library integration: `library.zig` titles are comptime today — add a
   runtime title list for ingested docs; single-pane reader for them
   (chapter = section, `[`/`]` chapters, page/verse label in gutter); `/`
   search over the document.
10. Tests: unit tests per stage; golden-ish checks on both samples (Psalm 1
    has 6 verses, Psalm 118 has 176, page 26 marker present; BMS chapter 4
    has Figure 4-9 caption, code block indentation intact, no indexterm
    noise); PTY test for first-run folder prompt and opening an ingested book.
11. `scripts/install-local.sh`, update README/PROTOTYPE, report to user.

## Housekeeping notes

- Use `mise exec -- zig …` (repo pins Zig 0.16.0; global is 0.17).
- Run all suites: `zig build test`, tests/*.py (search-pty, input-pty,
  full-library-pty, reference-parity, cli/plan-regression, bundle-standalone,
  all-source-books --chapters ~80 s). Restore `zig-out/bin/kata` if a test
  build should not be committed (`git checkout -- zig-out/bin/kata`), or ask.
- Nothing is committed yet: Psalms alignment, word search, install script,
  README cleanup, mise.toml, examplePubs/ are all pending in the work tree.
  Ask the user whether to commit before starting / whether examplePubs and
  kataLibrary output belong in .gitignore.
