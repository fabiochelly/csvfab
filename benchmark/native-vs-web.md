# csvfab: the web app against the native app

The old csvfab (Python `server.py` + its page in Chromium) and the new native one (Rust,
egui) run the same operations on the same generated files. Each result is checked, every time
is the first frame showing the result, and memory covers the whole process tree. Measured during the move to 2.0, both apps side by side,
with `benchmark/compare.py` (see [How it was measured](#how-it-was-measured); the script is not in the repository: it needs both apps' sources at once). Raw
data: `benchmark/native-vs-web.json`.

## In short

Median times, web → native, and the native app's speed-up (files: `benchmark/gen.py`, 15 columns,
~160 bytes a row):

| Operation | 100,000 rows | 1,000,000 rows | 5,000,000 rows |
|---|---:|---:|---:|
| Open (file → grid drawn with its rows) | 102 ms → 8.2 ms · **12×** | 322 ms → 23 ms · **14×** | 2.0 s → 38 ms · **53×** |
| Filter: global search “lyon” | 39 ms → 7.7 ms · **5.0×** | 78 ms → 26 ms · **3.0×** | 274 ms → 127 ms · **2.2×** |
| Filter: column city contains “lyon” | 37 ms → 5.0 ms · **7.4×** | 60 ms → 16 ms · **3.7×** | 252 ms → 56 ms · **4.5×** |
| Filter: total_spent > 1000 (formula in the search box) | 63 ms → 5.0 ms · **13×** | 463 ms → 28 ms · **17×** | 2.7 s → 103 ms · **27×** |
| Filter: total_spent > 1000 (column box `>1000`) | — / 9.2 ms | — / 59 ms | — / 238 ms |
| Sort numbers (total_spent, largest first) | 88 ms → 7.3 ms · **12×** | 498 ms → 31 ms · **16×** | 3.2 s → 194 ms · **17×** |
| Sort text (last_name, A → Z) | 53 ms → 7.3 ms · **7.2×** | 287 ms → 36 ms · **7.9×** | 2.0 s → 210 ms · **9.5×** |
| Sort dates (signup_date) | 69 ms → 6.5 ms · **11×** | 451 ms → 28 ms · **16×** | 3.4 s → 303 ms · **11×** |
| Sort on two keys (country, then total_spent ↓) | 98 ms → 10 ms · **9.3×** | 672 ms → 55 ms · **12×** | 5.0 s → 880 ms · **5.7×** |
| Clear filter + sort | 35 ms → 6.3 ms · **5.6×** | 41 ms → 4.2 ms · **9.8×** | 130 ms → 3.4 ms · **38×** |
| Find “Lyon” (count the matching cells) | 78 ms → 3.9 ms · **20×** | 406 ms → 25 ms · **16×** | 2.5 s → 230 ms · **11×** |
| Totals row on (default sums) | 224 ms → 160 ms · **1.4×** | 429 ms → 169 ms · **2.5×** | — / 245 ms |
| Column panel: city's value counts | 48 ms → 3.6 ms · **14×** | 228 ms → 11 ms · **20×** | — / 52 ms |
| Group by city → new tab | 117 ms → 8.3 ms · **14×** | 535 ms → 21 ms · **26×** | — / 83 ms |
| Pivot table (its guess: country × status, sum) | 94 ms → 11 ms · **8.3×** | 770 ms → 27 ms · **29×** | — / 94 ms |
| Duplicates (suggested keys: name+postal OR e-mail OR phone) | 286 ms → 18 ms · **16×** | 3.0 s → 126 ms · **24×** | — / 895 ms |
| Insights | 1.3 s → 198 ms · **6.7×** | 4.1 s → 211 ms · **19×** | — / 416 ms |
| File profile | 931 ms → 49 ms · **19×** | 1.9 s → 224 ms · **8.6×** | — / 429 ms |
| Computed column num({total_spent}) * 2 | 143 ms → 13 ms · **11×** | 1.2 s → 76 ms · **16×** | — / 581 ms |
| Save as CSV (untouched) | 65 ms → 20 ms · **3.2×** | 261 ms → 22 ms · **12×** | — / 27 ms |
| Export to Excel (.xlsx) | 1.8 s → 487 ms · **3.6×** | 15 s → 4.3 s · **3.6×** | — / 7.6 s |
| **Peak memory** (PSS, web → native) | 1021 MB → 128 MB (heap 99 MB) | 3.56 GB → 593 MB (heap 313 MB) | 4.28 GB (open → find only) → 1.71 GB (heap 711 MB) |
| **Start-up, cold** (process → rows shown) | 495 ms → 18 ms | 742 ms → 69 ms | 2.9 s → 262 ms |

- **The native app is faster at every operation and every size measured.** It is usually 10–50×
  faster, and up to 53× for opening 5 M rows (2.0 s → 38 ms).
- **Where it gains least** (said plainly):
  - **The totals row**: 1.4× at 100 k, 2.5× at 1 M. Both apps wait 150 ms of calm before
    computing it, by design, and that wait is most of the time.
  - **Plain-text filters on big files**: the global search is only **2.2×** faster at 5 M
    (274 → 127 ms) and the city filter 4.5×. The web app filters in parallel workers there.
  - **Saving an untouched CSV at 100 k** (3.2×) and the **Excel export** (3.6× at every size;
    7.6 s native for the 1,048,576 rows Excel takes).
- **Inside the native app, worth a look**:
  - The column box's `>1000` is **2.3× slower than the same condition typed as a formula**
    (238 vs 103 ms at 5 M).
  - A **two-key sort costs 4.5× a one-key sort** (880 vs 194 ms at 5 M).
  - Sorting **dates** costs 1.5× sorting numbers (303 vs 194 ms).
  - The global search costs 2.3× the column filter (127 vs 56 ms).
- **Memory**: the native app's own heap is 13 MB with 100 k rows shown and 48 MB with 5 M. The
  file itself is mapped (page cache: 771 MB at 5 M, given back under pressure). At its busiest the
  native app stays at 0.13 / 0.59 / 1.7 GB PSS (100 k / 1 M / 5 M; the last while saving, with the
  old and new files both mapped). The web app holds 0.7 / 1.5 / 2.7 GB once the file is shown and
  peaks at 1.0 / 3.6 / 4.3 GB (at 5 M over open → find only).
- **Limits**: the web app cannot open 20 M rows (3 GB) or 50 M rows (7.6 GB):
  `Array buffer allocation failed`, at once, on open. At 10 M rows (1.5 GB) it had reached 3.5 GB
  when the machine's memory guard (4 GB must stay available) stopped it mid-open. So whether it
  opens 10 M was **not determined** on this shared machine; it would need ~7–8 GB. The native app
  opened, filtered and sorted all of them correctly, 50 M rows included (open 2.9 s, filter 1.8 s,
  sort 5.8 s, 1.4 GB of heap + the 7 GB file mapped). Its limit was not reached.
- **Start-up** (process → rows on screen, file evicted from the page cache): 0.5 / 0.7 / 2.9 s
  for the web app (server + Chromium + page + file), 18 / 69 / 262 ms for the native app
  (without a window here: see the caveats).
- **Not measured: the web app at 5 M from the totals row on.** At 5 M the web app holds ~4 GB, and
  the machine was shared (other agents' builds, the user's IDE). The memory guard stopped it during
  `find`, and four later attempts ended early too (MemAvailable falling to 2–4 GB, mostly because
  of the other work). Those rows show only the native time; compare at 1 M. In one attempt,
  **`server.py` quit in the middle of the computed column on 5 M rows**: it exits once no window has
  polled it for 30 s (`IDLE_LIVE`, `bridge/session.py`). A page whose main thread is busy that
  long looks closed to it, and the save that followed hung. That run was aborted, so its times are
  not in the JSON, but the failure is the web app's own. A long main-thread job on a big file can
  cut the page off from its server.



## The measures, file by file

Measured 2026-10-28 06:50 CET on Intel(R) Core(TM) Ultra X7 358H (16 threads, 30.9 GB RAM), PCB01 SED SK hynix 2048GB  1,9T 0 (/dev/mapper/omarchy_root[/@home]), Linux 7.2.8-9-omarchy; web app 1.25.1 in Chromium 155.0.8059.84 Arch Linux, Python 3.14.8; native app built 2026-10-28 06:24 with rustc 1.99.0 (b940084d7 2026-09-28). Median of 3 runs; `python3 benchmark/compare.py --rows 100000,1000000,5000000 --limits 10000000,20000000,50000000 --repeat 3 --resume --native-bin /home/fabio/.cache/csvfab-agents/bench-ws/target/release/examples --json benchmark/native-vs-web.json --markdown /home/fabio/.cache/csvfab-bench/compare/tables.md`.

### 100,000 rows × 15 columns · 15 MB

| Operation | Web app | Native app | Native is | Native engine alone |
|---|---:|---:|---|---:|
| Open (file → grid drawn with its rows) | 102 ms | 8.2 ms | **12×** faster | 2.0 ms (index built, no grid) |
| Filter: global search “lyon” | 39 ms | 7.7 ms | **5.0×** faster | 6.0 ms |
| Filter: column city contains “lyon” | 37 ms | 5.0 ms | **7.4×** faster | 2.0 ms |
| Filter: total_spent > 1000 (formula in the search box) | 63 ms | 5.0 ms | **13×** faster |  |
| Filter: total_spent > 1000 (column box `>1000`) | — | 9.2 ms |  | 6.0 ms |
| Sort numbers (total_spent, largest first) | 88 ms | 7.3 ms | **12×** faster | 5.0 ms (ascending) |
| Sort text (last_name, A → Z) | 53 ms | 7.3 ms | **7.2×** faster | 5.0 ms |
| Sort dates (signup_date) | 69 ms | 6.5 ms | **11×** faster | 4.0 ms |
| Sort on two keys (country, then total_spent ↓) | 98 ms | 10 ms | **9.3×** faster |  |
| Clear filter + sort | 35 ms | 6.3 ms | **5.6×** faster |  |
| Find “Lyon” (count the matching cells) | 78 ms | 3.9 ms | **20×** faster |  |
| Totals row on (default sums) | 224 ms | 160 ms | **1.4×** faster |  |
| Column panel: city's value counts | 48 ms | 3.6 ms | **14×** faster |  |
| Group by city → new tab | 117 ms | 8.3 ms | **14×** faster | 11 ms (city and status) |
| Pivot table (its guess: country × status, sum) | 94 ms | 11 ms | **8.3×** faster | 9.0 ms (city × status) |
| Duplicates (suggested keys: name+postal OR e-mail OR phone) | 286 ms | 18 ms | **16×** faster | 17 ms |
| Insights | 1.3 s | 198 ms | **6.7×** faster | 252 ms |
| File profile | 931 ms | 49 ms | **19×** faster |  |
| Computed column num({total_spent}) * 2 | 143 ms | 13 ms | **11×** faster |  |
| Save as CSV (untouched) | 65 ms | 20 ms | **3.2×** faster |  |
| Export to Excel (.xlsx) | 1.8 s | 487 ms | **3.6×** faster |  |

Memory, PSS / anonymous (heap) of the whole process tree:

| | file shown, idle | peak (during) | after all operations | the CSV's pages mapped (page cache) | processes |
|---|---:|---:|---:|---:|---:|
| Web app (server.py + Chromium) | 694 MB / 521 MB | 1021 MB / 840 MB (formula) | 700 MB / 530 MB | not mapped (read into the page) | 11 |
| Native app | 40 MB / 13 MB | 128 MB / 99 MB (insights) | 116 MB / 86 MB | 15 MB | 1 |

<details><summary>Peak PSS per operation, web / native</summary>

open 757 MB / 41 MB · filter_global 766 MB / 41 MB · filter_col 696 MB / 42 MB · filter_expr 687 MB / 42 MB · filter_num — / 43 MB · sort_num 632 MB / 44 MB · sort_text 647 MB / 44 MB · sort_date 661 MB / 45 MB · sort_multi 722 MB / 45 MB · clear 757 MB / 43 MB · find 754 MB / 44 MB · totals 732 MB / 44 MB · col_values 721 MB / 44 MB · group_by 798 MB / 45 MB · pivot 764 MB / 45 MB · dupes 829 MB / 51 MB · insights 970 MB / 128 MB · profile 978 MB / 108 MB · formula 1021 MB / 115 MB · save_csv 954 MB / 115 MB · export_xlsx 715 MB / 116 MB

</details>

Start-up, process start → first frame showing the file's rows:

| | cold (file evicted from the page cache) | warm |
|---|---:|---:|
| Web app | 495 ms | 462 ms |
| Native app | 18 ms | 17 ms |

### 1,000,000 rows × 15 columns · 153 MB

| Operation | Web app | Native app | Native is | Native engine alone |
|---|---:|---:|---|---:|
| Open (file → grid drawn with its rows) | 322 ms | 23 ms | **14×** faster | 8.0 ms (index built, no grid) |
| Filter: global search “lyon” | 78 ms | 26 ms | **3.0×** faster | 29 ms |
| Filter: column city contains “lyon” | 60 ms | 16 ms | **3.7×** faster | 13 ms |
| Filter: total_spent > 1000 (formula in the search box) | 463 ms | 28 ms | **17×** faster |  |
| Filter: total_spent > 1000 (column box `>1000`) | — | 59 ms |  | 67 ms |
| Sort numbers (total_spent, largest first) | 498 ms | 31 ms | **16×** faster | 34 ms (ascending) |
| Sort text (last_name, A → Z) | 287 ms | 36 ms | **7.9×** faster | 43 ms |
| Sort dates (signup_date) | 451 ms | 28 ms | **16×** faster | 28 ms |
| Sort on two keys (country, then total_spent ↓) | 672 ms | 55 ms | **12×** faster |  |
| Clear filter + sort | 41 ms | 4.2 ms | **9.8×** faster |  |
| Find “Lyon” (count the matching cells) | 406 ms | 25 ms | **16×** faster |  |
| Totals row on (default sums) | 429 ms | 169 ms | **2.5×** faster |  |
| Column panel: city's value counts | 228 ms | 11 ms | **20×** faster |  |
| Group by city → new tab | 535 ms | 21 ms | **26×** faster | 32 ms (city and status) |
| Pivot table (its guess: country × status, sum) | 770 ms | 27 ms | **29×** faster | 31 ms (city × status) |
| Duplicates (suggested keys: name+postal OR e-mail OR phone) | 3.0 s | 126 ms | **24×** faster | 142 ms |
| Insights | 4.1 s | 211 ms | **19×** faster | 288 ms |
| File profile | 1.9 s | 224 ms | **8.6×** faster |  |
| Computed column num({total_spent}) * 2 | 1.2 s | 76 ms | **16×** faster |  |
| Save as CSV (untouched) | 261 ms | 22 ms | **12×** faster |  |
| Export to Excel (.xlsx) | 15 s | 4.3 s | **3.6×** faster |  |

Memory, PSS / anonymous (heap) of the whole process tree:

| | file shown, idle | peak (during) | after all operations | the CSV's pages mapped (page cache) | processes |
|---|---:|---:|---:|---:|---:|
| Web app (server.py + Chromium) | 1.53 GB / 1.37 GB | 3.56 GB / 3.40 GB (export_xlsx) | 2.24 GB / 2.08 GB | not mapped (read into the page) | 12.0 |
| Native app | 186 MB / 21 MB | 593 MB / 313 MB (save_csv) | 441 MB / 273 MB | 153 MB | 1 |

<details><summary>Peak PSS per operation, web / native</summary>

open 1.77 GB / 187 MB · filter_global 2.09 GB / 188 MB · filter_col 2.15 GB / 188 MB · filter_expr 2.18 GB / 194 MB · filter_num — / 239 MB · sort_num 1.38 GB / 253 MB · sort_text 1.42 GB / 246 MB · sort_date 1.40 GB / 254 MB · sort_multi 1.47 GB / 254 MB · clear 1.49 GB / 235 MB · find 1.45 GB / 241 MB · totals 1.39 GB / 241 MB · col_values 1.38 GB / 241 MB · group_by 1.44 GB / 248 MB · pivot 1.44 GB / 248 MB · dupes 1.72 GB / 277 MB · insights 2.06 GB / 295 MB · profile 1.84 GB / 361 MB · formula 2.94 GB / 480 MB · save_csv 3.42 GB / 593 MB · export_xlsx 3.56 GB / 441 MB

</details>

Start-up, process start → first frame showing the file's rows:

| | cold (file evicted from the page cache) | warm |
|---|---:|---:|
| Web app | 742 ms | 732 ms |
| Native app | 69 ms | 17 ms |

### 5,000,000 rows × 15 columns · 771 MB

| Operation | Web app | Native app | Native is | Native engine alone |
|---|---:|---:|---|---:|
| Open (file → grid drawn with its rows) | 2.0 s | 38 ms | **53×** faster | 45 ms (index built, no grid) |
| Filter: global search “lyon” | 274 ms | 127 ms | **2.2×** faster | 201 ms |
| Filter: column city contains “lyon” | 252 ms | 56 ms | **4.5×** faster | 80 ms |
| Filter: total_spent > 1000 (formula in the search box) | 2.7 s | 103 ms | **27×** faster |  |
| Filter: total_spent > 1000 (column box `>1000`) | — | 238 ms |  | 393 ms |
| Sort numbers (total_spent, largest first) | 3.2 s | 194 ms | **17×** faster | 239 ms (ascending) |
| Sort text (last_name, A → Z) | 2.0 s | 210 ms | **9.5×** faster | 309 ms |
| Sort dates (signup_date) | 3.4 s | 303 ms | **11×** faster | 208 ms |
| Sort on two keys (country, then total_spent ↓) | 5.0 s | 880 ms | **5.7×** faster |  |
| Clear filter + sort | 130 ms | 3.4 ms | **38×** faster |  |
| Find “Lyon” (count the matching cells) | 2.5 s | 230 ms | **11×** faster |  |
| Totals row on (default sums) | — | 245 ms |  |  |
| Column panel: city's value counts | — | 52 ms |  |  |
| Group by city → new tab | — | 83 ms |  | 158 ms (city and status) |
| Pivot table (its guess: country × status, sum) | — | 94 ms |  | 146 ms (city × status) |
| Duplicates (suggested keys: name+postal OR e-mail OR phone) | — | 895 ms |  | 1.0 s |
| Insights | — | 416 ms |  | 503 ms |
| File profile | — | 429 ms |  |  |
| Computed column num({total_spent}) * 2 | — | 581 ms |  |  |
| Save as CSV (untouched) | — | 27 ms |  |  |
| Export to Excel (.xlsx) | — | 7.6 s |  |  |

web: stopped by the memory guard during find (run 2): MemAvailable fell to 4.0 GB while the web app held ~4 GB and other work on the machine the rest. Four later sessions (web alone, 7–12 GB free at their start) ended early too: three by the same guard, the machine's other work taking up to 10 GB meanwhile (down to 2.0 GB available), and one when server.py exited during the computed column (see later_sessions). The operations from totals on were therefore not measured at 5 M for the web app; find has one run.

Memory, PSS / anonymous (heap) of the whole process tree:

| | file shown, idle | peak (during) | after all operations | the CSV's pages mapped (page cache) | processes |
|---|---:|---:|---:|---:|---:|
| Web app (server.py + Chromium) | 2.73 GB / 2.69 GB | 4.28 GB / 4.23 GB (open) | — / — | not mapped (read into the page) | 11 |
| Native app | 818 MB / 48 MB | 1.71 GB / 711 MB (save_csv) | 486 MB / 364 MB | 771 MB | 1 |

<details><summary>Peak PSS per operation, web / native</summary>

open 4.28 GB / 834 MB · filter_global 2.41 GB / 836 MB · filter_col 3.15 GB / 837 MB · filter_expr 3.29 GB / 873 MB · filter_num — / 1.09 GB · sort_num 3.75 GB / 1.16 GB · sort_text 4.01 GB / 1.12 GB · sort_date 4.22 GB / 1.16 GB · sort_multi 4.15 GB / 1.14 GB · clear 4.12 GB / 1.04 GB · find 3.89 GB / 1.03 GB · totals — / 856 MB · col_values — / 857 MB · group_by — / 877 MB · pivot — / 877 MB · dupes — / 1.05 GB · insights — / 1.05 GB · profile — / 1.01 GB · formula — / 1.46 GB · save_csv — / 1.71 GB · export_xlsx — / 1.12 GB

</details>

Start-up, process start → first frame showing the file's rows:

| | cold (file evicted from the page cache) | warm |
|---|---:|---:|
| Web app | 2.9 s | 2.6 s |
| Native app | 262 ms | 52 ms |

### Limits: the big files

| File | App | Open | Filter (city) | Sort numbers | Peak PSS / heap | Result |
|---|---|---:|---:|---:|---:|---|
| 10 M rows · 1.51 GB | web | — | — | — | 3.51 GB / 3.32 GB | MemAvailable fell to 4.0 GB (floor 4 GB): stopped |
| 10 M rows · 1.51 GB | native | 444 ms | 136 ms | 409 ms | 1.76 GB / 249 MB | ok, results checked |
| 20 M rows · 3.02 GB | web | ✗ | ✗ | ✗ | 447 MB / 320 MB | open: JsError: Error: Array buffer allocation failed |
| 20 M rows · 3.02 GB | native | 148 ms | 301 ms | 1.1 s | 3.51 GB / 489 MB | ok, results checked |
| 50 M rows · 7.58 GB | web | ✗ | ✗ | ✗ | 355 MB / 316 MB | open: JsError: Error: Array buffer allocation failed |
| 50 M rows · 7.58 GB | native | 2.9 s | 1.8 s | 5.8 s | 7.58 GB / 1.37 GB | ok, results checked |


## How it was measured

```sh
python3 benchmark/compare.py                                   # 100 k, 1 M, 5 M rows; both apps; start-up; engine alone
python3 benchmark/compare.py --limits 10000000,20000000,50000000   # + the big files (generated, deleted after)
python3 benchmark/compare.py --rows 1000000 --ops open,filter_col,sort_num --repeat 5
python3 benchmark/compare.py --apps native --no-startup        # one app; --help for the rest
python3 benchmark/compare.py --report benchmark/native-vs-web.json   # re-render the tables
```

It builds the native examples first (`cargo build --release` in `native/`, into `native/target`;
`--native-bin DIR` uses binaries already built). Files go to `~/.cache/csvfab-bench/compare/`
(`benchmark/gen.py`, same seed: the same bytes everywhere).

- **The two apps run for real, side by side, operations interleaved.** The web app is `server.py`
  plus its page in a headless Chromium (throwaway profile and state folder), driven over the
  DevTools protocol (`benchmark/web_ops.py`). The native app is `csvfab::app::App` itself, in an
  `egui_kittest` harness off screen (`native/app/examples/bench_ui.rs`, `--serve` mode). For each
  operation, run *k* goes to one app, then the other, the order flipping each run. Each run waits
  first until the machine's CPU is at most 15 % busy (`--cpu-quiet`); the busy share measured is
  kept with every run in the JSON (`cpu_busy_before`). All of it runs under the bench lock
  (`~/.cache/csvfab-bench/lock`, the one `perf.sh` takes). Times are the median of 3 runs.
- **Each operation is set the way the interface sets it.** Web: the function the UI handler calls
  (`applyFilters`, `sortBy`, `applyGroupBy`, `srvWrite`…; see `web_ops.py`). Native: the tab's
  fields and then `Tab::refresh`, as typing in a filter does; a command run as the menu runs it;
  a press and release on a button found by its text on screen ("Open the result in a new tab",
  "Apply", "Save"); the column panel's ▾ found among the frame's widgets and clicked. The clock
  starts at the user's last action (the click's release, the command) and stops at the first
  frame showing the complete result. Web: `requestAnimationFrame` + `setTimeout 0` after the result,
  painted. Native: that frame run and tessellated (the CPU side of painting). Frames are driven as
  an event loop drives them: one when egui or a background job asks (`request_repaint`), none
  otherwise.
- **Every result is checked.** It is compared with a plain Python `csv` pass over the file (the
  `.ref.json` beside it: rows, matches, first value after each sort, the number of cities, the sum
  of total_spent, the top city's count, the doubled first value…), and the two apps with each other
  (duplicates, insights, pivot shape). Saved CSVs must be byte-identical to the source (SHA-1), and
  workbooks must hold every row (or Excel's 1,048,576). A run whose result is wrong is not given a
  time. **On 100 k, 1 M and 5 M, every result was right in both apps, and the two apps found the
  same thing in every operation** (`cross` is empty in the JSON).
- **Memory** is sampled every 50 ms from `/proc/<pid>/smaps_rollup`, for the whole process tree of
  each app: web = `server.py` + every Chromium process (12), native = its one process. PSS (a
  shared page counts 1/n) and Anonymous (the heap: what the app itself allocated). The native
  app maps the CSV instead of reading it, so those pages are page cache, given back whenever the
  kernel needs them. They are reported apart ("the CSV's pages mapped"). The web app copies the file into
  its page's JS heap, so its figures are all heap. "File shown, idle": after the open, settled.
  "Peak": the highest sample over all operations. "After all operations": what stays.
- **Start-up**: from the process start (`subprocess.Popen`) to the first frame showing the file's
  rows. Web: as `csvfab file.csv` does: `server.py` started, the file queued (`POST /api/open`),
  Chromium started on the page, which picks the file up. The time is read in the page
  (`performance.timeOrigin`). Native: `bench_ui file --startup` (`App::new` with the file, frames
  until it is shown). "Cold": the CSV evicted from the page cache (`posix_fadvise DONTNEED`) first. The
  programs' own files are warm in both cases.
- **The engine alone** (last column): `native/engine/examples/bench`, `aggregate_bench` and
  `insights_bench` on the same file, without any app. They are not always the same operation:
  sort ascending, group by city *and* status, pivot city × status, save to `/dev/null`, durations
  rounded to the ms or the second by those examples. They show how much the app adds on top.
- **Limits**: 10 M, 20 M and 50 M rows (1.6 / 3.2 / 8.1 GB), each app alone (one after the
  other), only open, filter (city) and sort (numbers), once. Before each, MemAvailable must leave
  the floor (4 GB) plus the app's expected peak (scaled from the largest file measured) plus 1 GB.
  Otherwise the run is skipped and says why. During the run, a sampler stops the app if
  MemAvailable falls under 4 GB while the app holds more than 512 MB (or under 2 GB at all). The big
  files are deleted at the end.

## Caveats — read before quoting a number

- **The machine was shared.** Other agents were compiling and benchmarking on it throughout
  (load average 6–11). The CPU gate, the interleaving and the bench lock limit the damage, but
  times under ~20 ms can move by ±30 % between runs. Ratios at 1 M and 5 M are the robust ones.
- **Off screen, both.** Neither app draws to a real window: the web page is headless Chromium (it
  composites but presents nothing), the native app has no window, no GL context and no GPU upload
  (tessellation is counted, the GPU's share of a frame is not: under 1 ms for the grid by
  `docs/perf.md`). The native harness also builds an AccessKit tree every frame, which the real
  app does not: that cost counts against native. The web's stop waits for its next animation frame
  (up to ~16 ms, a cost a real browser pays too). The native harness has no vsync: on a 60 Hz screen
  either app shows a result at the next refresh.
- **Native start-up here is not the windowed start-up.** No headless display server is
  installed (no Xvfb/weston), and windows must not open on the user's screen. So `bench_ui
  --startup` measures process → app built → grid drawn, without creating a window or GL context.
  `docs/perf.md` measured the real binary on screen at 66 / 91 ms (1 M / 5 M, warm), with
  window + GL ready at ~28 ms.
- **Not quite the same operations everywhere:**
  - *Numeric filter*: the web's column box only matches text (`>1000` is searched literally), so
    the like-for-like row is the formula in the search box. Both apps evaluate
    `num({total_spent}) > 1000`. The native column box (`>1000`) is shown on its own row.
  - *Clear*: in the web app the filter is cleared with "Clear all" and the sort with Ctrl+Z (two
    actions, timed together). The native app clears both at once.
  - *Two-key sort*: web = the shift-click adding the second key (it re-sorts on both). Native =
    the two keys set at once.
  - *Totals*: both apps wait 150 ms of calm before computing (a deliberate debounce), included.
  - *File profile*: the web app reads a 200,000-row sample, the native app 1,000,000 rows (all of
    them up to 1 M). The native app does up to 5× more work for the same table. *Insights*: both read
    100,000 rows at most (an even sample).
  - *Duplicates*: web clock from the suggestion applied; native from the command (the dialog's
    creation included).
  - *Save*: both write a temporary file, fsync it and rename it. Native, as Save as, then reads the
    new file again (not timed).
  - *Excel*: the 5 M-row file is truncated to Excel's 1,048,576 rows by both apps (and checked so).
- **Memory is PSS**: Chromium's own ~0.5 GB is in the web figures before any file. Swap was in
  use on the machine (zram). Pages swapped out are not in PSS, and the JSON has `swap` per app.
