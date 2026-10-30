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
