# EPUB ingest — development plan (next push)

Status (2026-10-09, before 3:10 pm reset): 92/92 unit tests pass.
- `src/ingest.zig`: types + stage stubs (`error.NotImplemented`). Not wired in.
- `src/epub.zig` DONE for step 2: in-memory zip central directory + deflate
  (`Archive.init/find/read`), `attribute`/`Tags` scanners, `package()` →
  title, author, spine member paths, toc (nav.xhtml or NCX). Test opens both
  samples (Psalter: 33 spine items, BMS: 29), reads every spine member.
- TODO in epub.zig: `checkDrm` currently rejects ANY encryption.xml; allow
  font obfuscation (`http://www.idpf.org/2008/embedding`,
  `http://ns.adobe.com/pdf/enc#RC`). Zip64 not handled (fine for samples).
- NEXT: step 3 (XHTML tokenizer + block builder), then step 6 (decide output
  format) and 4/5 rules; then 7–9; step 1 (folder prompt) can go any time.
- Installed ~/.local/bin/kata does NOT include epub/ingest (not user-visible).

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
