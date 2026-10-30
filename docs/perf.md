# Performance: the rules

The user's goal: **the lightest and most powerful CSV editor in the world** — giant
files handled as if they were small, start-up at the minimum, every search, sort and
job as fast as the hardware allows, scrolling at the screen's refresh rate, nothing
drawn when nothing changes. RAM may be spent when it buys real speed (there is plenty
on most machines), never wasted. **Nothing may get slower, heavier or uglier than
before** unless the gain elsewhere is decisive — and then say so explicitly. Test
everything: the user's words, "everything must be lighter, faster, more responsive
than before", and just as beautiful or more.

## Baseline (2026-10-28, 16 cores, NVMe, `./perf.sh`)

| measure | value |
|---|---|
| engine open 5M (809 MB), cold (evicted from page cache) | 199 ms |
| engine open 5M, warm | 47 ms |
| filter global "lyon" / "émile" / city=lyon / total>1000 | 105 / 106 / 44 / 216 ms |
| sort total_spent / last_name / signup_date | 142 / 163 / 110 ms |
| save 5M (to /dev/null) | 114 ms |
| start-up: grid shown, 1M / 5M warm | 66 / 91 ms (window+GL ready at ~28 ms) |
| start-up: grid shown, 5M cold | 247 ms |
| grid frame, scrolling 9000 px/frame on 5M | 0.88 ms |
| memory with 5M open: anonymous (private) | 48 MB (+ the file's page cache) |
| frames drawn while idle | 0 |
| the web app (README): open / filter / sort 5M | 2.7 s / 0.9 s / 1.5–2.5 s; 2.4 GB |

Other engine benches: `cargo run --release -p csvfab-engine --example aggregate_bench`,
`insights_bench`; `cargo test --release -p csvfab-engine -- --ignored --nocapture perf`
(stats, edit, convert, split… each module has its `#[ignore]` perf test).

## Check after the torture fixes (2026-10-29, machine in use, medians of 3)

Engine on 5M warm: open 32 ms; filter global "lyon" 33, "émile" 50, city=lyon 56, total>1000 77 ms;
sort total_spent 160, last_name 47, signup_date 106 ms; an untouched save is one copied block
(0 ms to /dev/null). Off-screen audit (`examples/audit`, every scenario and command on 5M): worst
frame ≤ 10 ms CPU (first frames of a load or a dialog: first reads of the mapped rows and text
layout), median ≤ 2 ms, idle frames 0. Nothing regressed; the remaining first-frame costs vary by
±50 % between runs and were not worth chasing.

## Edits off the UI thread (2026-10-30, 5M rows, `bench_ui --ops edits,formula,totals`)

Whole-selection and whole-column edits no longer build a `(RowId, col, String)` per cell
on the UI thread: `table::Patch` holds each column's layers ready (sorted ids, one arena
or one text for all), built in a job, linked in by `Table::apply` (O(columns)). A cleared
or filled selection is one *uniform* layer per column (4 bytes a cell, the id list shared
by the columns where every row changes). Past 12 layers only the run of the smallest is
merged (a linear k-way merge, id ranges in parallel): a computed column is not merged
again for every few cells typed after it. Totals keep, per column, what they counted
(rows + the column's layers, held as `Weak`): an edit recounts only its columns, a few
typed cells are told to the totals (`Totals::edit`), dictionary columns count codes. The
column panel searches, ticks "(Select all)" and finds "go to" rows in jobs.

| op (UI thread's longest frame in brackets) | before 4 thr | after 4 thr | before 16 thr | after 16 thr |
|---|---|---|---|---|
| Ctrl+A, Delete (73M cells) | 4.45 s [4.45 s] | 0.38 s [3 ms] | 4.94 s [4.94 s] | 0.14 s [3 ms] |
| Ctrl+A, paste one value (75M) | 7.4 s [7.4 s] | 0.37 s [2 ms] | — | 0.14 s [2 ms] |
| computed column | 590 ms [190 ms] | 365 ms [3 ms] | 460 ms [220 ms] | 136 ms [3 ms] |
| 13 cells typed after it (layer merge) | 205 ms [200 ms] | 64 ms [12 ms] | 270 ms [247 ms] | 27 ms [2 ms] |
| totals row turned on | 233 ms | 188 ms | 129 ms | 70 ms |
| one cell typed under the totals | 250 ms | 5 ms | — | 5 ms |
| column panel on `id` (5M values): open / idle frame / search / "(Select all)" | 22 / 40 / 179 / 815 ms frames | 2 / 1.6 / 1 / 3 ms frames | 26 / 40 / — / 915 | 1.3 / 2 / 2 / 3 |
| insert / duplicate a row under a sort | [15 ms] / [14 ms] | [5 ms] / [7 ms] | | |
| paste a 2×2 block | [7.5 ms] | [3 ms] | | |
| move a column | 17 ms [8.5 ms] | 10 ms [9 ms, the redraw] | | |

Engine (`cargo test --profile fast -p csvfab-engine -- --ignored --nocapture formula_bench
perf_layer_merge`, 16 threads): a computed column as a patch 64–284 ms vs 359–754 ms as
changes; 5M cells as `Patch::fill` 42 ms vs 186 ms as changes; applying either < 0.1 ms.
Not done: totals do not read the non-dictionary number cache (`colcache` num is
`parse_number`, totals count `num_cell` — dates and text-led values would count
differently); inserting a row still writes a new order (one parallel pass, ~2 ms at 5M)
rather than editing it in place, since every cache keys the order by its address.

## Writing files in parallel (2026-10-30, the user's 2 GB file: 5.3M rows × 38 columns, 16 threads)

`engine/examples/save_bench FILE CASE DEST` (A/B against the binary from before, back to back;
outputs to tmpfs; the machine was busy, so a quieter run in brackets), `bench_ui --ops export_xlsx,save_csv`.

| op | before | after |
|---|---|---|
| Excel export (1,048,576 rows), app | 12.9–14.1 s, one core, 499 MB | 0.50–0.64 s, 353 MB |
| Excel export, customers-5m (1M rows) | — | 0.21 s, 121 MB |
| save after a computed column | 3.68 s [2.84] | 1.64 s [1.03] |
| save after a column moved | 3.90 s [2.86] | 1.55 s [1.00] |
| save re-encoded to Windows-1252 | 1.97 s [1.25] | 1.23 s [0.76] |
| save re-encoded to UTF-16 | 9.16 s [5.95] | 2.29 s [2.13] |
| save with CRLF chosen | 0.73 s | 0.56 s |
| save untouched (kernel copy) | 0.47 s | 0.47 s |
| crash-recovery copy, computed column | a full copy, ~2 GB, ~1 s every 30 s | journal 82 MB, 51 ms, 3 s after the edit |

- **Workbook** (`convert/xlsx_write.rs`): the sheet XML is made by hand in chunks of 1–8K rows
  on every core (style indices read from a workbook rust_xlsxwriter makes without the rows,
  numbers by itoa/zmij, cells without `r` where they follow on), each chunk deflated on its own
  ending in a sync flush (pigz's way: the chunks end to end are one deflate stream), CRC32s
  combined, the zip written here (Zip64 only when a bound on the XML, counted in the typing
  pass, passes 4 GB) — a wave of chunks made while the previous one is written. The other parts
  are rust_xlsxwriter's, copied compressed. A test compares every cell (XML, styles resolved,
  calamine's reading, the parts) with rust_xlsxwriter's own output, kept as the fallback
  (`write_slow`) for a workbook layout the splice cannot read. rust_xlsxwriter repeats an empty
  header row (`<row r="1"/>` twice) when there is no data row: not here. LibreOffice is not
  installed on this machine: checked with Python's zipfile (CRCs) and expat (well-formed XML).
- **Saves** (`save.rs`): rows made here are encoded on every core, the line breaks between them
  and the read-back check of the reused index included; a thread of its own writes while the
  next rows are made (short pieces gathered into 1 MB hand-overs); UTF-16 transcoded in
  parallel. Fields are written straight from the record (`State::visit`), no String per field;
  byte-exact with the old path (`rows_written_as_their_fields`). Just before the rename the file
  is looked at again: another program's write during a long save is refused (`SaveError::Changed`).
- **One scan per record after a column insert/delete/move** (`State::visit`, `visit_cols`): the
  record split once, its fields mapped to the columns through the inverse column map, only
  columns with edit layers looked up — used by saves, exports, stats, edits, kinds, dupes, group,
  formulas, insights, schema, find. The global search still reads raw blocks when columns were
  only moved or added (and the pattern finds nothing in an empty cell). Fields of files checked
  UTF-8 at open are no longer re-checked (`Base::text`).
- **Recovery** (`engine/src/journal.rs`, `app/src/recovery.rs`): a table on its file as still on
  disk keeps a journal of its edits (layers, column map, order, header, line break, the file's
  stamp), replayed over the file on restore (refused if the file changed since); otherwise
  (no file, file changed) the whole copy as before.
- **After a save that kept every row in place** the column caches of columns no edit touched are
  carried to the new file (`colcache::carry`) and the rows shown are kept, not filtered and
  sorted again; charts and totals still recount (on the carried caches).
- Not done: the CRLF-forced save still copies records one at a time on one thread (0.56 s);
  pwrite of encoded blocks from several threads (the single writer is now the limit on tmpfs).

## Parity batch (2026-10-30, customers-5m, 16 threads, machine busy: medians of 3, A/B back to back)

| measure | before | after |
|---|---|---|
| text sort, first (rank cache built): email / street / company | 410 / 163 / 99 ms | 445 / 153 / 99 ms (natural keys: digits as numbers) |
| text sort, cached ranks: email | 25 ms | 20 ms |
| bench_ui sort_text (first run) | 53 ms (audit) | 46 ms |

- Text sort keys (`text::sort_key_into`) are made once per column with the rank cache (one more
  pass over the folded text, in parallel): no cost once cached.
- Per frame: the search highlights (`Hl`, a regex compiled) made once per change of the searches,
  not every frame; a cell whose text, look and searches are the last frame's keeps its marks (no
  search through it); `tools::commands()` made once (a static), their shortcut keys parsed once;
  the column titles kept per table version (`Tab::heads`); busy indicators (spinner, progress bars,
  the loading dot) at 30 frames a second, not every frame; the welcome screen's recent files sized
  in a job (never a stat on a frame); the selection's sum not restarted at every frame of a drag
  (counted once the button is up; the rows read in the job); every row's id of an unfiltered tab
  kept per table (`shown_ids`) for the tools.
- Off the UI thread: "Fill empty cells from above" (a patch in a job), a row moved or a column
  renamed no longer analyse every column again; Job threads and the compute pool run at nice 4.
- Start-up: the settings read once (they were read twice), written on a thread of their own
  (opening the files given wrote them before the window existed), the icons rasterised on their
  own thread beside the fonts, the paths made absolute once.
- Not done: the rayon pool is still built on the main thread (`compute_pool`, < 1 ms): built on
  another thread, a file read starting meanwhile would build rayon's default pool (not niced).

## Column reads, search, numbers (2026-10-30, 16 threads, `bench_ui` A/B back to back, medians of 3 processes)

The user's 2 GB file (5.3M rows × 38 columns, French, accents and a quoted `"35,61 €"` in every
row) and customers-5m; "first" is the first run in a process (caches cold), "again" a second one.
Engine passes: `engine/examples/col_bench FILE [runs] [columns…]` (new), `dupes_bench` (`COLS=` for
another file's columns). The machine was busy during the final run: absolute times are higher than
in the sections above, the A/B is fair.

| op (bench_ui) | 2 GB before | 2 GB after | 5M before | 5M after |
|---|---|---|---|---|
| filter "lyon" (global), again | 98 ms | 66 ms | 52 ms | 36 ms |
| filter total>1000, first (cache built) | 355 ms | 104 ms | 114 ms | 90 ms |
| filter `num({total}) > 1000` | 120 ms | 96 ms | 101 ms | 95 ms |
| sort by date, first | 210 ms | 156 ms | 103 ms | 104 ms |
| sort country + total, first | 120 ms | 115 ms | 109 ms | 108 ms |
| find "Lyon" (Ctrl+F) | 108 ms | 95 ms | 117 ms | 52 ms |
| totals row turned on | 360 ms | 248 ms | 90 ms | 77 ms |
| group by city (sum) | 260 ms | 223 ms | 95 ms | 89 ms |
| pivot | 123 ms | 102 ms | 115 ms | 85 ms |
| duplicates (suggested keys), again | 537 ms | 479 ms | 761 ms | 696 ms |
| file profile | 557 ms | 471 ms | 181 ms | 167 ms |
| computed column | 154 ms | 137 ms | 137 ms | 129 ms |
| file profile again after one cell typed (`profile_edit`) | 507 ms | 58 ms | | |
| a duplicate row hovered, view off and sorted (`dupes_hover`, frame) | 46 ms | 0.9 ms | | |
| a duplicate row hovered, duplicates view (frame) | 4.4 ms | 0.8 ms | | |

Open, sorts again, save, Excel export, insights, column values: unchanged (within ±5 %, checked
again with 12 runs where one looked worse: sort country + total again +3 %, sort by date again −7 %).
Engine alone, 2 GB: every field of every record visited 171 → 118 ms; field 28 read 142 → 84 ms;
global "lyon" 151 → 85 ms (the bytes searched alone: 30 ms); find's row test 141 → 53 ms; totals of
the 16 number columns 940 → 620 ms; a number column's first cache 320 → 80 ms, a date column's
(birth dates) 272 → 50 ms.

- **Fields by bit masks** (`scan::visit_fields`): the delimiters of 64 bytes at a time as a mask
  (SSE2 on x86_64, NEON on aarch64, a byte loop elsewhere), the next field's end a `trailing_zeros`,
  not a `memchr` per field (fields are ~10 bytes). Every column read goes through it (`with_field`,
  `State::visit`, `visit_cols`): column caches, totals, profile, group, pivot, formulas, dupes, saves.
  Quoted fields as before. Tested against the old visitor on random records (`fields_by_masks_as_by_bytes`).
- **NEON**: the scanner's four masks (`scan::masks`) and the delimiter mask have an aarch64 version
  (`scan::neon`, simdjson's pairwise-add movemask); the bit-mask scanner (`blocks`) now runs on
  aarch64 too (it fell back to the byte scanner there). Not compiled here: the aarch64 target is
  not installed on this machine — CI's macOS build is the check. x86 tests the same logic via the
  scalar reference.
- **No dictionary tried for a number or date cache in doubt** (`colcache::build_dict`, `sure`): a
  column of amounts (86 % distinct in the sample) passed the sample, built most of a dictionary
  and gave up (250 ms), then parsed its numbers (80 ms). For `Need::Num`/`Date` a sample more than
  ¾ distinct goes straight to the numbers; the dictionary stays possible for value counts and
  charts (not marked "no dictionary").
- **Global search: accents told by a table** (`view::block_search`): with a needle trusted in a
  cell, only characters folding into one of its letters matter; the two-byte ones are looked up in
  a 1920-bit table made with the pattern (`fold2`), three-byte ones (€, narrow no-break space)
  remembered per thread (`fold3`), and in checked UTF-8 only lead bytes are visited, 8 at a time
  (`text::find_lead`). The rest (record found, window folded) only for those few characters.
- **Find's row test by SIMD** (`edit::replace`, `Raw::Ascii`): an ASCII search without k or s (the
  only letters a case-ignoring regex matches beyond ASCII: K, ſ — a test checks it against the regex
  crate) uses `CiFinder` (AVX2) instead of a regex per record.
- **Numbers parsed once** (`stats::num_cell`, `cell_read` in the profile): `cell_kind` parsed a
  number, then the caller parsed it again; `parse_number`'s non-plain path builds its digits on the
  stack (two `String`s a cell before).
- **Duplicates**: the hash pairs sorted by a parallel bucket pass on the top bits (`radix::sort_pairs`,
  same order as `sort_unstable`): −5 % on the whole find (2 GB 573 → 547 ms, 5M 818 → 773 ms, `dupes_bench`).
- **Per-column versions, app side**: the file profile keeps each column's line with what it read
  (`ColKey`: the file, the order, the column's layers and field — as the totals' `Basis`); a cell
  typed reads that column again, not the 38. The duplicate bar's "others shown" (every frame of a
  hover) went through the whole view — a HashSet of it (view off) or `contains` per row (duplicates
  view); now a bit per row of the view, made once per view (`DupMarks::shown`), and only counted.
  Engine caches never were per table version (keyed by file and field); the sparkline band is
  sampled (≤ 50 000 cells), not worth a per-column key.

### Tried, not kept / not done
- Single pass for several duplicate keys: not done after timing the phases — hashing is bound by
  cleaning the values, not by reading the records (fields 1–9 on the 2 GB file): ≤ 50 ms of 480 to
  gain, for three keys' hashes held at once (+170 MB).
- Search: the remaining 55 ms above the byte search on the 2 GB file are the 23M accented characters
  visited; a SIMD lead-byte mask would take some more.
- Group by reads each row twice (a hash pass, then the aggregation); insights, save (a kernel copy
  when untouched, 430 ms to tmpfs) and the Excel export were not changed.

### PGO (evaluated, not applied)
Manual `-Cprofile-generate`/`-Cprofile-use` (rustc 1.99, LLVM 23; `llvm-profdata` 23.1.3 reads its
profiles), trained by `bench_ui` on customers-5m and the 2 GB file (every op once), A/B on the 2 GB file:

| | totals | group by | find | dupes first | insights | profile | export | filter_col / sort first |
|---|---|---|---|---|---|---|---|---|
| PGO vs release | −17 % | −15 % | −13 % | −14 % | −7 % | −5 % | −6 % | +8 … +16 % (worse) |

The passes bound by CPU gain 5–20 %; first sorts and column caches (bound by reading memory) lose
8–16 %. Mixed: worth it only with a training set that is not the benchmark itself. The steps:

    D=/tmp/pgo; rm -rf $D; mkdir -p $D
    RUSTFLAGS="-Cprofile-generate=$D" cargo build --release -p csvfab --example bench_ui
    target/release/examples/bench_ui customers-5m.csv --repeat 1 --out /tmp/pgo-out
    llvm-profdata merge -o $D/merged.profdata $D/*.profraw
    RUSTFLAGS="-Cprofile-use=$D/merged.profdata -Cllvm-args=-pgo-warn-missing-function=false" \
        cargo build --release -p csvfab

(`llvm-profdata` must be the LLVM of rustc: `rustup component add llvm-tools` gives it as
`~/.rustup/toolchains/*/lib/rustlib/*/bin/llvm-profdata`.) Each RUSTFLAGS change rebuilds every
crate (5–6 min here, 20 min for the instrumented build plus its runs).

## Parity batch E (2026-10-30, customers-5m, 16 threads, machine busy and swapping: medians of 8, A/B back to back)

| measure (`engine/examples/bench`, `view_bench`, fast profile) | before | after |
|---|---|---|
| text sort, first (rank cache built): email / street / company / city | 500 / 176 / 115 / 23 ms | 490 / 167 / 120 / 22 ms |
| sort last_name (first) | 61 ms | 60 ms |
| filter, ordinary regexes (`^Lyo`, `lyon\|paris`, `\d{3}-\d{4}`, city `^Ly`) | same results and times (±noise) | same |
| filter `(?<=@)gmail` (look-behind, refused before) | — | 81 ms (4.65 s without the pre-check) |
| filter email `(\w)\1` (back-reference) | — | 680 ms |

- **Ties of the text sort** (`radix::text_order_ties`): only the runs of equal keys are looked at,
  found in the pass that ranks them; a run whose cells are all their own fold (a bit per record
  kept with the fold cache, `Folded::plain`) is skipped, and a column whose cells all are (most
  e-mail columns) skips the pass. A first attempt that read every tied cell again cost email
  +25 % (384 → 482 ms): not kept. Dictionary columns break ties on their distinct values.
- **Look-around and back-references** (`engine/src/re.rs`): the `regex` crate first (nothing
  changed for what it takes); fancy-regex only for what it refuses, behind the pattern relaxed
  for the `regex` crate, whose literals also feed the record prefilter — without it a
  look-behind ran the backtracking engine on every cell (4.65 s on 5M rows).
- Row moved by its number: the two positions found in parallel and the new order made in one
  copy (was: two scans, a copy, a remove and an insert, each O(n), on the UI thread).
- Marked rows (review, compare, the marked-rows chip) put in table order by a sort of the
  marked ids when the table has its file's order, else one parallel pass (was one pass with a
  hash lookup per row, on the UI thread).

## Parquet (2026-10-30, the user's 2 GB file: 5.3M rows × 38 columns)

`cargo test --profile fast -p csvfab-engine --lib bench_parquet -- --ignored --nocapture`,
with `CSVFAB_BENCH` (the CSV) and `CSVFAB_BENCH_OUT` (a folder), the export first. Outputs to tmpfs;
the machine in use.

| op | 4 threads | 16 threads |
|---|---|---|
| export: 1,910 MiB of CSV → 609 MiB (32 %), 15 of 38 columns typed | 2.97 s, 450 MB private | 1.98 s, 1.8 GB private |
| open: back to the same CSV (`cmp`), but for its 11 `-0.00…` cells, now plain zeros | 1.55 s, 359 MB private | 1.13 s, 1.5 GB private |
| open, the text thrown away | | 1.00 s: the writer is not what bounds it |
| half the file: export / open | | 0.72 s, 1.3 GB / 0.53 s, 1.4 GB private |

The open's private memory follows the threads, not the table: half the rows, the same memory
(not broken down further). The bound on queued
text went from 1 GB to 512 MB with no loss of time; counted by capacity, not length (a String
doubling hid half of it: 2.1 GB private before). This CPU's cores are of two kinds, so
16 threads are not 4 × 4.

With another build running on this machine, the same day: export 2.63 s, open 1.60 s; the table's
times are from a quiet moment, before the four decimal columns (`-0.00` below), and the 4-thread
column from before the bound on queued text came down.

Compared at 4 threads, one run each, export then open of what it wrote:

| codec | export | size | open |
|---|---|---|---|
| zstd 1 (kept) | 2.97 s | 614 MiB | 1.55 s |
| snappy | 2.70 s | 904 MiB | 1.44 s |
| zstd 3 | 3.50 s | 622 MiB | 1.63 s |
| zstd 1, 524,288 rows a group | 3.07 s | 594 MiB | 1.65 s |

zstd 1 costs 10 % over snappy and saves 32 %. zstd 3 was no smaller here. Groups four times longer
save 3 % and hold more in memory: 122,880 rows kept.

- **Open**: the file is mapped; a thread per core takes the next row group, decodes it in batches
  (about 16 MB, by the group's own byte count) and queues its text; the caller writes the queues
  in file order. The text waiting is bounded (`Gate`, 512 MB): past it only the group next in the
  file goes on, which rules out a deadlock whatever the sizes. Private memory does not grow with
  the file: the map is page cache.
- **Export**: a sample settles most columns (a text column dies at its first value), the others are
  read whole; row groups are filled and encoded in waves of as many groups as threads, each
  group's columns in parallel, one thread writing the wave before.
- A negative zero fits a number column, as a plain zero (the user's choice): this file's 1 to 5
  `-0.00` cells in `montant`, `solde`, `prix_unitaire` and `longitude` no longer make them strings.
- UTF-8 checks: parquet takes simdutf8 without `std`, and without it no x86_64 build picks AVX2 at
  run time; `engine/Cargo.toml` names the crate for that feature. A/B on one file, machine busy:
  with the text thrown away 1.40 / 1.51 s → 1.38 / 1.33 s; to a file no change (1.60 / 1.66 → 1.61 /
  1.62 s). Small, and it costs nothing.
- Not done: a file of one huge row group decodes on one thread (a group is the unit of work):
  parallel by column would need readers kept in step. A group whose text passes the bound
  makes the threads ahead of it wait: slower, never more memory.
- Binary (stripped, fat LTO): 26.66 → 35.37 MB, +8.7 MB, brotli (1.2 MB) included. By symbols:
  parquet 2.5, the arrow crates 2.7 (arrow-select 1.2, arrow-cmp 0.6, arrow-array 0.5), brotli 1.1,
  zstd's C 0.6, the rest generics inlined into the callers. The user's call: speed first, size
  does not matter. `opt-level` stays as it is.

## Duplicates: filter, direct reads, exact comparison (2026-10-31, 16 threads, 1 GiB file, A/B, one run)

`g1.csv`: 12.6M unique rows, 2 columns; `g1dup.csv`: the same + 10 % of them again (13.9M rows).
Whole row, `bench_ui --ops open,dupes` (from the command to the count shown), then the engine alone
(`dupes_bench`, median of 3):

| | before | filter + direct read | exact comparison |
|---|---|---|---|
| `dupes`, no duplicates | 929 ms, HWM 1 599 MB, anon 290 MB | 673 ms, 1 231 MB, 173 MB | 677 ms, 1 231 MB, 171 MB |
| `dupes`, 10 % duplicates | 1 183 ms, 1 765 MB, 467 MB | 986 ms, 1 579 MB, 433 MB | 1 004 ms, 1 584 MB, 417 MB |
| engine alone, no duplicates | 377 ms | 119 ms | 104 ms |
| engine alone, 10 % duplicates | 583 ms | 409 ms | 380 ms |
| `dupes_delete` (1.26M rows) | 36 ms, frame 36 ms | 22 ms, frame 3 ms | |

The rest of the UI figure is the dialog, not the count (its 150 ms delay, the wait for the suggested
keys). Groups, scores and reasons identical on customers-5m for every spec and every suggestion of
`dupes_bench` (its `digest`). A key that puts most rows in groups pays for the table: the e-mail of
customers-5m (98 % of the rows) 371–470 → 473–497 ms on a noisy machine; a whole row or a
"sounds like" key, the same or better.

- **Bit tables** (`dupes::repeated`): 2 bytes a row of "seen", ½ of "seen twice", 4 of prints (the
  hash's top bits, per row). Only a repeated hash makes a pair; the first rows are found by the
  prints in a second look, the only rows hashed twice. Rows without a duplicate: no sort, no
  union-find, no group table. For `Whole`, the plain keys and the sound keys; the cross-checks,
  several keys a row, keep their pairs.
- **Direct reads** (`loose_direct`): a record in ASCII without quotes gives the cells' cleaned text
  in one go, by blocks; any other row reads its cells. Proved against the cells on random rows.
- **Exact comparison** (`find_exact`): the record itself is hashed (`bare_record`) when it holds no
  quote nor \u{1} and has all its fields; any other row writes the same text from its cells.
- **Delete duplicates**: the rows left made in a job (`State::without`), the table only takes them.
  "`dupes_delete` not done after 100 s" was the benchmark: pressed the frame the count appeared, its
  click fell where the button had been, beside a box about to be re-centred, which closed it.

## How to measure

- `./perf.sh` (whole suite) or `./perf.sh engine`. It takes a lock: only one
  measurement runs at a time on this machine. Other agents build in parallel, so
  **always compare A/B**: keep a copy of the binary from before your change, run old
  and new back to back under the lock (`flock ~/.cache/csvfab-bench/lock <cmd>`),
  several times, report medians.
- The start-up, frame-time and memory parts of `perf.sh` open a csvfab window for a few
  seconds on the user's screen: run them only when you need those numbers.
- Cold numbers: evict the file first (`perf.sh` has `evict`). Warm: run twice.
- Start-up: `CSVFAB_PROFILE=1` prints each step's time since the process started.
- Frame time: `CSVFAB_GRID_TIMING=1 CSVFAB_SHOT=autoscroll:9000`.
- Idle: `CSVFAB_FPS=1` prints frames per second (must be silent when idle).
- Memory: `/proc/<pid>/smaps_rollup` (Anonymous = private heap; Pss_File = mapped file).
- Files: `~/.cache/csvfab-bench/customers-{1m,5m}.csv` (real disk). Make a bigger one if
  useful (`python3 /home/fabio/Work/csvfab/benchmark/gen.py 20000000 …` into
  `~/.cache/csvfab-bench/`, ~3.2 GB) — delete it when done.
- Correctness first: `cargo test --release` must pass; byte-exact round trips stay exact.
- Look: the off-screen renderer (`docs/app.md`, "Checking your work") — never screenshots
  of the screen.

## Report

A before/after table (A/B, medians), what you changed and why it is faster, memory
before/after, anything you tried that did not pay (so nobody tries it again).
