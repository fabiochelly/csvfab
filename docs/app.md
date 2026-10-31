# The native app: how it is built, how to add to it

csvfab is being rewritten from a web app (`/home/fabio/Work/csvfab/viewer.htm`, `ui/`,
`bridge/`) into pure Rust: `engine/` (data, no GUI, 211 tests) and `app/` (egui 0.37,
glow). The user's requirements, in their words: **the fastest and most efficient app
possible, the same look as the web app, perfect rendering** — font quality, vertical
centring, every padding, the original SVG icons, silky instant scrolling, no flicker.

## Layout of `app/src`

| file | what | owner |
|---|---|---|
| `main.rs` | entry, module list | lead |
| `app.rs` | `App`: tabs, tools open, save, undo, grid actions, shell dialogs | lead |
| `chrome.rs` | tab bar, toolbar (search, toggles, chips, Save), status bar, main menu, command palette, toasts, welcome | lead |
| `tab.rs` | `Tab`: table, view (filters/sort run as a background `Job`), `restrict`, `marks`, `find`, `ghost`, band toggles | lead |
| `grid.rs` | the grid, drawn by hand; scroll kept as f64 pixels (`GridState::scroll`) | grid agent |
| `spark_ui.rs`, `totals_ui.rs`, `colpanel.rs` | charts band, totals row, column panel (called by grid.rs) | grid agent |
| `widgets.rs` | menu items (`Item`, `head`, `sep`, `width`, `row_buttons`), `centered_edit` | lead (use them) |
| `icons.rs` + `assets/icons/` | the web app's SVG icons, tinted, crisp | icons agent |
| `theme.rs` | `Palette` (every CSS variable, 6 themes), `Metrics`, `apply`, `menu_frame`, `dialog_frame`, `tooltip_frame` | done |
| `fonts.rs` | system fonts as the web picked them; `UI_MEDIUM`, `UI_SEMIBOLD`, `MONO_BOLD` families; system faces for other scripts, on demand | done |
| `bidi.rs` | right-to-left lines in the grid: words in display order | done |
| `job.rs` | `Job::spawn(ctx, |cancel, progress| …)`, `poll()`, `cancel()`; cancelled on drop; a panic in the job is caught: the job returns `Failed::failed()` (implement `job::Failed` for a new result type) and a toast tells it | done |
| `recovery.rs` | unsaved edits kept in `<state>/recovery/` 3 s after the last edit — a journal of the edits (`engine/src/journal.rs`) while the file on disk is the one read, else a copy of the table (big files every 30 s) — offered back after a crash; the panic hook (`crash.log`, and on a UI-thread panic every unsaved table written at once) | lead |
| `tools/mod.rs` | `Tool` trait, `Cx`, `Cmd`, `Command` registry, `dialog()` helper | lead |
| `tools/<name>.rs` | one tool family per file | tool agents |
| `shot.rs` | `CSVFAB_SHOT=menu,col:3,cell,scroll:PX` screenshot hooks | lead |

## Adding a tool

1. `app/src/tools/<name>.rs`: a struct implementing `Tool` (`fn show(&mut self, cx: &mut Cx) -> bool`,
   false once closed), drawn as an `egui::Window`/`egui::Modal` with `theme::dialog_frame`
   (or the `tools::dialog` helper). State lives in the struct.
2. `pub fn commands() -> Vec<Command>`: name, group, keys ("Ctrl+H"), `enabled: fn(&Tab) -> bool`,
   `run: fn(&mut Cx)` — `run` usually does `cx.out.push(Cmd::Tool(Box::new(MyTool::new(cx.tab))))`.
3. In `tools/mod.rs`: add `pub mod <name>;` and `v.extend(<name>::commands());` in `commands()`.
   That file is shared: re-read it right before editing, change only those lines.
4. The main menu looks commands up **by exact name** for its fixed items: `File profile…`,
   `Map`, `Insights`, `Create a schema…`, `Validate against a schema…`, `Review changes…`,
   `Find & replace`; and lists by **group** the submenus `Transform the file`, `Columns`, `Rows`;
   group `Save` goes in the Save button's ▾ menu. Everything is also in the command palette.

`Cx` gives the tab (`cx.tab.st()` → `&State` snapshot, `cx.tab.table` to edit,
`tab.apply_changes(&changes, ctx)` = one undo step and the grid updated,
`tab.apply_patch(patch, ctx)` for cells made ready in a job (`table::Patch::new(&changes)`,
`Patch::fill`, `Patch::column`: nothing per cell left for the UI thread),
`tab.table.batch(|t| …)` for several edits as one step, then `tab.after_edit(true, ctx)` after
structural changes; `tab.restrict_to(label, ids, ctx)` shows these rows (a chip lets the user
leave); `tab.marks` marks rows/cells; `tab.hide_values(col, set, ctx)`; `tab.shown_ids()`,
`tab.shown_id(pos)`, `tab.kinds`), `cx.p`/`cx.m` (palette, metrics), `cx.out` (open a file,
a tab from bytes, a toast, another tool).

Heavy work: `Job::spawn` with a `State` clone (cheap: Arcs), poll in `show`, show progress,
cancel on close. Never block the UI thread on anything that grows with the file.

## Look rules

- Read `docs/look.md` (the web app measured, zone by zone) and compare with the reference
  screenshots in `/tmp/claude-1000/-home-fabio-Work-csvfab/265a7873-924b-40c6-bc8e-62b45251ed0e/scratchpad/ref/`.
  Also read the web app's own CSS/JS for what you build (`ui/app.css`, `ui/js/NN-*.js`, `viewer.htm`).
- Colours only from `cx.p` (Palette); sizes from `Metrics` or look.md. Fonts: `FontId::proportional`
  (Inter), `grid::semibold(size)`, `FontFamily::Name(fonts::UI_MEDIUM)`, monospace for data.
- Menus: `widgets::Item` / `head` / `sep` / `width` in a popup with `theme::menu_frame`.
- Text fields: `widgets::centered_edit` (egui otherwise draws the text at the top of a taller box);
  set the hint's font explicitly (egui's default hint size is 14).
- Positions of text and 1 px rules on whole physical pixels (`(x * ppp).round() / ppp`), rules at +0.5.
- Icons: `icons::…` (the original SVGs, being wired by the icons agent — use `icons::paint(painter, Icon::X, rect, color)`
  if it exists when you start, else the wrapper functions).

## Checking your work

- Unit-test the logic you add (engine functions are already tested).
- **Look at it off screen — never open windows on the user's desktop** (they are working on it;
  screenshots of the screen would capture their private windows):
  `cargo build --release -p csvfab --example shot` then
  `./target/release/examples/shot out.png file.csv "<CSVFAB_SHOT spec>" [hover x,y]`
  renders the real app through wgpu into a PNG (1400×860 logical at ppp 2 by default:
  `SHOT_SIZE=WxH`, `SHOT_PPP`, `SHOT_MS` = how long it runs frames before rendering).
  Specs: `menu`, `col:3`, `cell`, `edit`, `search:text`, `scroll:PX`, `sort:N`, `filter:C=text`,
  `select:r.c-r.c`, `freeze:N`, `totals`, `heat:C`, `bars:C`, `xoff:PX`, `cmd:<command name>`,
  `run:<command prefix>`, the tools' own (`tool:split`…). Add `crate::shot::take("mine")` hooks
  for your states. Read the PNG and compare with the web app's references.
- Test files: `~/.cache/csvfab-bench/customers-1m.csv`, `customers-5m.csv`,
  `/home/fabio/Work/csvfab/Companies.csv`, `Contacts.csv`.

## Robustness (keep these true)

- **Files cut short by another program**: the engine maps files (`engine/src/mapguard.rs`); a SIGBUS from
  a read past the new end is caught, zero pages put in place, `Base::cut_short()` set. The app tells the
  user (`Dialog::OnDisk`), and `save` refuses (`SaveError::CutShort`): zeros are never written as data.
- **Changed / removed on disk**: looked at when the window comes back to the front and at most every 2 s
  while it is used (`App::check_disk`) — never by waking an idle window. Our own saves are expected
  (`App::written`), so they are not reported.
- **Panics**: in jobs, caught (`job::guarded`); on the UI thread, the hook writes the unsaved tables and
  `crash.log` before the process ends. The next start offers the copies back (`Dialog::Recover`).
- **Toasts** that follow an edit carry Undo (only while that edit is still the last of its tab).
- **Sync clients** (kDrive, Dropbox…) rewrite a file after our own save (another inode, another
  time): a stamp that differs at the same size is checked on a sample of the content (whole up to
  3 MB, else its first, middle and last MB, `base::file_sample`, in a job) against the bytes read
  or written (`Stamp::holds`, `Expect`); our content is ours, as the web's 2 s / fingerprint rule.
- **Row ids after a save**: a save that wrote the rows in another order (inserted, deleted, moved)
  gives the next table each old id's record (`tab::renumbering`, in the save's job); the rows a
  tool showed (`restrict`) and marked (`marks`) follow them; the duplicate marks are counted again.
- **Duplicate marks** follow every change of the table: `Tab::poll` counts them again 300 ms after
  the last one, one job per tab (a newer one cancels the one under way).

- **Imports from a folder that cannot be written** (a read-only mount, an archive's view):
  the CSV goes to the documents folder (`convert::fallback_dir`: XDG's, else ~/Documents,
  else home) and the toast names both folders.

## Encodings and formats

`enc.rs` holds 33 encodings, `Enc::GROUPS` listing them by region for the pickers (status bar, next
file, Save dialog: `widgets::enc_list`, searchable by label, region or language). `Enc::ALL` is a
journal index: new ones go at its end. Single-byte encodings stay as the file's bytes, decoded by
field. UTF-16, UTF-32 and the multi-byte ones (`Enc::transcoded`) are UTF-8 in memory: their bytes
can hold a delimiter's inside a character. A multi-byte base keeps the file mapped behind it
(`Base::raw`): rows left as they were are written as the file had them (`Plan::exact`,
`Base::raw_run`: line breaks counted from marks taken at the transcoding), so a character two byte
sequences give, or bytes that mean nothing, survive. Not done for ISO-2022-JP when a line break
fell inside a Japanese run (the counts differ, `marks` is empty): rows are encoded anew.

Detection, without BOM and when the file is not UTF-8 (`enc::detect`): `looks_western` first, then
chardetng on two samples of whole lines. A Western file stays Windows-1252: Hungarian in 1250 reads
as Western too, one click on the pill puts it right. ISO-2022-JP is told by its escape (`sniff`).

A character the target cannot hold is never written without a word: count, first few, Cancel, Save
as UTF-8 (with a BOM) or Replace with "?" (`tools::export::Lossy`), in the Save dialog and on Ctrl+S.

Opened through `convert`: `.xls`, `.xlsb`, `.ods` (`book_read`, calamine, a sheet read whole); `.gz`,
`.zst` and a `.zip` of tables (`packed`) are unpacked beside themselves (`data.csv.gz` → `data.csv`,
`dump.gz` → `dump.csv`), and what comes out opens as any file: an `.xlsx` in a zip then goes through
its own import. A zip's members are base names only, whatever the archive says.

## Exports and saves (keep this line clear)

An **export** writes another file and leaves the tab where it is: Excel, SQLite, Parquet, a CSV of
"Only the rows shown", Extract, Split into files, Combine. It leaves out the hidden columns
(`Tab::export_st`, `State::without_columns`) and writes the rows shown, in the view's order. In the
save dialog "Only the rows shown" starts ticked for Excel, SQLite and Parquet.

A **save** rewrites the file, or moves the tab onto a new one: Save, Ctrl+S, and Save as to a CSV
(any delimiter or encoding), with "Only the rows shown" left as it starts, unticked. It writes
the table whole, hidden columns and filtered rows included: the tab goes on from that file, so
anything left out would be lost. Never give a save `export_st`'s table. A tab with hidden
columns says which of the two it is in the dialog's note.

JSON, JSON Lines, TSV, any other delimiter, HTML and Markdown are exports too (the copies under
the CSV choices in the Save dialog; `convert::text_write`, and `save::save` for the delimited ones,
on `export_st`): they leave the tab on its file. "CSV — tab" stays a save-as. JSON gives bare numbers
and booleans only for a column whose every cell comes back the same (no `007`, 15 digits at most),
`null` for its empty cells. Group by and pivot results open as new tabs: they hold the columns chosen
in their dialogs, over the rows shown.

## Parquet (`engine/src/convert/parquet_read.rs`, `parquet_write.rs`)

- **Open**: as the other imports (`.parquet`, `.parq`, `.pqt`): a CSV beside the file or in the
  fallback folder, the same questions and toast, no sheet to pick. The file is mapped, row groups
  are decoded by several threads, their text written in file order; at most 512 MB of text waits
  (`Gate`), past it only the next group in the file goes on, so a file of any size fits.
- **Cells**: null is empty; floats in their shortest digits, an exponent only beyond 10^±21;
  decimals with all their digits; binary as `0x…` (as the SQLite import); dates ISO; lists, maps
  and structs as JSON text. A decimal comma in a `;` CSV, as every import.
- **Codecs**: snappy, gzip, zstd, lz4, brotli.
- **Timestamps**: without a zone, as stored (`2024-03-01 10:30:15.123`); an instant
  (`isAdjustedToUTC`) always in UTC with a `Z` (`2024-03-01T10:30:15Z`), never in the machine's
  zone. Arrow's metadata in the file is not read: a zone name there is a display hint.
- **Export**: a column is typed only if **every** value comes back as the same cell, the decimal
  mark aside; one value that does not fit and the column is a string (the rules: top of
  `parquet_write.rs`). The one exception, the user's: a negative zero (`-0`, `-0.00`) fits any
  number column and becomes a plain zero, so it reopens as `0` or `0.00`, without its sign. The detected kind (`view::detect_kinds`) picks
  the candidates, the whole column is then read. `31/12/2024`, `35,61 €`, `12 %`, `10:15` are
  strings. How booleans and timestamps were written (capitals, `T`, digits of a second) is kept
  in the file's metadata (`csvfab.cells`), so `.000` or `TRUE` reopen as they were; other readers
  see plain types.
- **Refused, with a message**: LZO, encrypted files, INTERVAL columns (Arrow's reading would drop
  their months). A panic in the decoder on a damaged file is caught and told as a damaged file.

## Filters (choices that differ from a plain reading)

- **Value filters** (`FilterSpec::hidden`, `picks`): values hidden from a column's panel or a
  cell's menu, values kept ("Only the rows with …", exact text) and chart parts picked
  (`view::ValuePick`, the part's own test). One per column (a new one replaces it), the other
  columns' and the typed filters kept. Exact text: "Paris " is not "Paris".
- **Invert** flips the text tests (search, column filters, expression) as one test — never the
  value filters, which are ticked by hand (as the web's). Column filters of several words: AND.
- **Text sort**: the web's French numeric collator (`text::sort_key`): accents and case ignored,
  digit runs as numbers ("Lot 2" < "Lot 10"), punctuation before digits before letters. Equal
  keys are then told apart as the collator did, accents first, then case, lower first
  (`text::tie_key_into`: emile < Emile < émile < Émile); only then the file's order. Keys made
  once per column (`colcache` ranks), radix sorted; the cells of equal keys read again only
  where the fold changed them (`Folded::plain`).
- **Regexes** (search, column boxes, find & replace, their highlights: `engine::re::Re`): the
  `regex` crate for every pattern it takes; look-around and back-references, which it
  refuses, run on fancy-regex (backtracking bounded to 1M steps a cell: past it, no match,
  a replace leaves the cell), checked first by the pattern relaxed for the `regex` crate
  (`re::relax`: look-arounds out, a back-reference = any text), whose literals also skip
  the records.
- **Chart picks**: a sampled picture whose counts say none or all of the rows read hold the
  part is checked on every row in a job (`sparkline::holds`) before "No row holds those
  values." / "every row already holds those values" (as the web's, on all the rows).
- **Sort** is a view (Save keeps the file's order; "Write the sorted order to the file" writes it).
  Enter and Tab keep the cell after writing it.

## Process

- **The user shares this machine: never hog the CPU.** Every cargo command and heavy test binary runs as
  `nice -n 19` with `CARGO_BUILD_JOBS=4` (tests: `-- --test-threads=4`, `RAYON_NUM_THREADS=4`), one cargo
  command at a time. Release builds use fat LTO: iterate with debug test runs, release once at the end.

- Work in your own copy only when several agents edit the same files at once: `cp -r
  /home/fabio/Work/csvfab/native ~/.cache/csvfab-agents/<you>-ws` (skip `target/`, never under /tmp: it is
  nearly full). A separate target dir rebuilds everything (fat LTO, minutes of every core): share the
  workspace's `target/` whenever you can, and never run two builds at once.
- Touch in the real tree **only your files** (+ the two lines in `tools/mod.rs`). Engine changes: only
  new files or clearly additive functions; say so in your report.
- At the end copy your files to the real tree, check `cargo build -p csvfab` there (your target dir);
  if it fails in someone else's file, report it, don't fix it. Delete your target dir.
- Code style: concise, idiomatic Rust; comments say *why*; English for code and UI text (as the web app).
