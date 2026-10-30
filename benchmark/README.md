# csvfab benchmark

Measure it yourself: one script generates the files, runs every tool on them, checks every result, and prints the tables.

```sh
git clone https://github.com/fabiochelly/csvfab && cd csvfab
cargo build --release -p csvfab --example bench_ui   # the real app, off screen: csvfab's half of the benchmark
python3 benchmark/run.py                    # 100 k, 1 M and 5 M rows (~1 GB of files), csvfab and VisiData
python3 benchmark/run.py --rows 1000000     # one size
python3 benchmark/run.py --tools csvfab     # csvfab alone
python3 benchmark/run.py --help
```

Needs Rust to build `bench_ui` once, then Python 3.8+ and nothing else. VisiData is used if `import visidata` works, if `vd` is on the `PATH`, or through [`uv`](https://docs.astral.sh/uv/) (`uv run --with visidata`, nothing installed); otherwise it is skipped. Memory is sampled on Linux only. The full run takes about 7 minutes, nearly all of it VisiData, and needs ~7 GB of free RAM at 5 M rows.

## What is measured

**The files** (`benchmark/gen.py`): a typical customer export — `id, first_name, last_name, email, phone, company, street, postal_code, city, country, signup_date, last_order, orders, total_spent, status` — six countries, accents, quoted fields with commas inside, ~160 bytes per row. Same seed everywhere: every machine measures the same bytes. Files go to `benchmark/data/` and are reused. Sizes here are binary, as `run.py` prints them: the 5 M file is 808,516,231 bytes, so 771 MB here and 809 MB in a file manager.

**The operations**, the same for every tool:

| | |
|---|---|
| **Open** | the whole file loaded and ready: every row known and usable, not just the first screen. For csvfab, until the frame that has the whole file is drawn. The first rows on screen come earlier and are given on their own line |
| **Filter** | rows whose `city` contains `lyon`, case ignored |
| **Sort numbers** | by `total_spent`, largest first |
| **Sort text** | by `last_name`, A → Z |
| **Peak memory** | the whole tool, every process of it (PSS), sampled every 50 ms during one more open + filter + sorts |

**Correctness first**: the generator counts what each tool must find (the rows, the rows in Lyon, the largest amount). A tool whose result differs gets no time in the table, only the reason.

**Fair conditions**: the file is read once before measuring, so every tool reads it from the OS cache; each figure is the median of `--repeat` runs (3 by default), each run in a fresh process, so a sort is always the first sort of that column, never one read back from a cache; each tool runs on its own, one after the other.

- **csvfab** runs for real, without a window: `app/examples/bench_ui.rs` drives the app's own code in an [egui_kittest](https://crates.io/crates/egui_kittest) harness at 1920 × 1080, with a new user's settings. Each operation is set up as the interface sets it (the column's filter box, the sort), frames run as an event loop would run them, and the clock stops when the result is drawn and tessellated: the CPU's part of painting. The GPU's is not counted, and there is none here.
- **VisiData** runs in its own Python, a fresh process per run (`benchmark/vd_driver.py`), through what its commands do: open and wait for the load to finish, `|` select-col-regex, `#` type-float then `]` sort-desc, `[` sort-asc. Nothing is drawn (no terminal), and undo is off (it would copy the list of rows before each sort): both favour VisiData slightly, if at all.
- csvfab sorts text like a dictionary (accents folded, numbers in the text understood); VisiData compares code points. The comparison is not to csvfab's advantage.
- **Memory**: csvfab maps the file instead of copying it, and PSS counts the mapped pages it has touched: at 5 M rows that is most of its peak. The system can take those pages back at any time, so the line under it gives the private figure (anonymous memory, the heap). `bench_ui` has no window and no GPU surface: the app in a window adds what those take. Nothing here says what they take.

## Results

Measured on Intel(R) Core(TM) Ultra X7 358H (16 threads, 31 GB RAM), Linux 7.2.8-9-omarchy, csvfab 2.1.1 (release build, `bench_ui`), VisiData in Python 3.13.15, 2026-10-30 — `python3 benchmark/run.py`, median of 3 runs, each in a fresh process.

**100 k rows × 15 columns · 15 MB**

| | csvfab | VisiData 3.5 | csvfab is |
|---|---:|---:|---:|
| Open | **29 ms** | 197 ms | 6.7× faster |
| First rows on screen | **27 ms** | — |  |
| Filter (column contains) | **22 ms** | 100 ms | 4.7× faster |
| Sort numbers | **21 ms** | 213 ms | 10.3× faster |
| Sort text | **20 ms** | 132 ms | 6.6× faster |
| Peak memory (PSS) | **49 MB** | 149 MB | 3.0× less memory |
| of which private (heap) | **18 MB** | 141 MB | 7.7× less memory |

**1 M rows × 15 columns · 153 MB**

| | csvfab | VisiData 3.5 | csvfab is |
|---|---:|---:|---:|
| Open | **35 ms** | 2.1 s | 61.0× faster |
| First rows on screen | **33 ms** | — |  |
| Filter (column contains) | **36 ms** | 1.0 s | 29.5× faster |
| Sort numbers | **45 ms** | 3.7 s | 82.1× faster |
| Sort text | **28 ms** | 2.0 s | 69.2× faster |
| Peak memory (PSS) | **248 MB** | 1.2 GB | 5.0× less memory |
| of which private (heap) | **78 MB** | 1.2 GB | 15.7× less memory |

**5 M rows × 15 columns · 771 MB**

| | csvfab | VisiData 3.5 | csvfab is |
|---|---:|---:|---:|
| Open | **65 ms** | 12 s | 190.4× faster |
| First rows on screen | **44 ms** | — |  |
| Filter (column contains) | **71 ms** | 5.4 s | 76.2× faster |
| Sort numbers | **117 ms** | 22 s | 192.6× faster |
| Sort text | **57 ms** | 10 s | 182.4× faster |
| Peak memory (PSS) | **980 MB** | 5.9 GB | 6.2× less memory |
| of which private (heap) | **193 MB** | 5.9 GB | 31.4× less memory |

**The load during the runs**: the machine was in use (a browser, a desktop). csvfab at all sizes, 20:32–20:34: load average 1.2–2.9, falling after a compilation. VisiData at 100 k and 1 M rows, 20:16–20:17: load 0.6–1.3. VisiData at 5 M rows, 20:36–20:41: load 1.6–2.1. An earlier VisiData run at 5 M rows, beside compilations and tests (load up to 9.6), gave 15 s / 6.4 s / 29 s / 14 s: it is not used here, as it was unfair to VisiData.

**A CPU coming out of idle.** Every csvfab operation starts after a quiet moment, on a laptop in `powersave`, and that costs it: at 5 M rows, with one other core kept busy, the same runs give Open 35 ms and Sort text 47 ms instead of 65 ms and 57 ms. The tables keep the slower figures.

## Modern CSV

Modern CSV 2.4.4, free version, on the same machine, timed by hand on **2026-10-03**. It has no scripting interface, so these times were not taken again; they are on version 1 of the generated files, before a fix to the data (French cities such as *Le Havre* had been cut in two): same sizes, the cities spread a little differently. Each time ran from the click — *Open in Edit Mode* in the dialog it shows for a large file, or Data › Sort › *Sort Selected Column* — to the end of its work (its CPU idle again), every result checked on screen; median of 3 runs (sorts on 5 M rows: one run). Filtering rows is a feature of its Premium version.

| | 1 M rows · csvfab 2.1.1 (2026-10-30) | 1 M rows · Modern CSV (2026-10-03) | 5 M rows · csvfab 2.1.1 (2026-10-30) | 5 M rows · Modern CSV (2026-10-03) |
|---|---:|---:|---:|---:|
| Open | **35 ms** | 1.5 s (read-only: 1.2 s) | **65 ms** | 4.6 s (read-only: 3.1 s) |
| Filter (column contains) | **36 ms** | Premium only | **71 ms** | Premium only |
| Sort numbers | **45 ms** | 9.4 s | **117 ms** | 53 s |
| Sort text | **28 ms** | 4.1 s | **57 ms** | 24 s |
| Peak memory (PSS) | **248 MB** | 1.2 GB | **980 MB** | 5.6 GB |

Read-only, Modern CSV opens faster and keeps little in memory (0.4 GB for 5 M rows), but cannot sort or edit; the edit mode is the one comparable to csvfab, where every file is editable.

## Spreadsheets cannot open these files

| | Rows per sheet | Source |
|---|---|---|
| Microsoft Excel | 1,048,576 | [Excel specifications and limits](https://support.microsoft.com/en-us/office/excel-specifications-and-limits-1672b34d-7043-467e-8e27-269d656771c3) |
| LibreOffice Calc | 1,048,576 | [Calc FAQ](https://wiki.documentfoundation.org/Faq/Calc/022) |
| Apple Numbers | 1,000,000 | Apple, quoted by [Michael Tsai, 2020](https://mjtsai.com/blog/2020/04/02/opening-large-csv-files-in-numbers-10-0) |
| Google Sheets | 20 million cells or 100 MB per spreadsheet | [Google Drive help](https://support.google.com/drive/answer/37603) |

The 5 M-row file does not fit in any of them: Excel and Calc load the first 1,048,576 rows and drop the rest (with a warning). The 1 M-row file (153 MB) is already over Google Sheets' 100 MB.

## Published figures for other tools — not measured by us

For an order of magnitude only: different files, hardware and years, and "open" does not mean the same thing everywhere (end of a progress bar, first screen, fully editable). These tools are not ranked here, and none of these figures can be set against the tables above. Collected from their sources on 2026-10-03, except where the row says otherwise. **Vendor** = the tool's own publisher; **competitor** = the publisher of another tool.

| Tool | File | Operation | Time | Who measured | Source |
|---|---|---|---|---|---|
| Excel 2307 (Windows, i9-11900K) | 123 MB, 700 k lines | open · sort | 7.3 s · 2.4 s | competitor (Emurasoft) | [PDF, 2023](https://download.emeditor.info/doc/working-with-csv.pdf) |
| Excel (Mac, 2019 MacBook Pro) | 230 MB, 450 k rows | open | ~33 s | competitor (Tad) | [DuckDB docs, archived 2022](https://web.archive.org/web/20220807160349/https://duckdb.org/docs/guides/data_viewers/tad) |
| Excel 16.36 (Mac) | 34 MB | open | 5 s | independent | [Michael Tsai, 2020](https://mjtsai.com/blog/2020/04/02/opening-large-csv-files-in-numbers-10-0) |
| Excel | > 4 M rows | open (first 1,048,576 rows only) | > 45 s | competitor (Modern CSV) | [moderncsv.com, 2020](https://moderncsv.com/why-excel-sucks-and-modern-csv-is-awesome-at-least-for-csvs/) |
| LibreOffice Calc 7.4 (i7-10510U) | ~170 MB, > 1 M rows | open | 1 min 10 s | independent (QA) | [bug 94677, 2022](https://bugs.documentfoundation.org/show_bug.cgi?id=94677) |
| LibreOffice Calc 7.5, experimental "jumbo" sheets | 4.27 GB, 33.7 M lines (16.7 M kept) | open | ~3–4 min | independent | [bug 150141, 2022](https://bugs.documentfoundation.org/show_bug.cgi?id=150141) |
| Apple Numbers 10 | 34 MB | open | 47 s, 2.2 GB RAM | independent | [Michael Tsai, 2020](https://mjtsai.com/blog/2020/04/02/opening-large-csv-files-in-numbers-10-0) |
| Modern CSV | > 4 M rows | open, read-only · editable | 7.5 s · 28 s | vendor | [moderncsv.com, 2020](https://moderncsv.com/why-excel-sucks-and-modern-csv-is-awesome-at-least-for-csvs/) |
| Tablecruncher 1.8 (Mac mini M2) | 2 GB, 16 M rows | open | 32 s | vendor | [README, 2025, read again 2026-10-30](https://github.com/Tablecruncher/tablecruncher) |
| Tad (2019 MacBook Pro) | 230 MB, 450 k rows | open | < 5 s | vendor | [DuckDB docs, archived 2022](https://web.archive.org/web/20220807160349/https://duckdb.org/docs/guides/data_viewers/tad) |
| EmEditor Professional 25.2.906 (Ryzen 9 9950X, 64 GB RAM, SSD, Windows 11) | 10 GB of random ASCII, 128,000,000 lines | open · sort A to Z · delete duplicate lines | 1.067 s · 7.271 s · 4.285 s | vendor | [emeditor.com, "Fast File Operations", undated, read 2026-10-30](https://emeditor.com/?p=390) |
| EmEditor Professional 25.2.906 (same machine) | 1 GB, 12,783,832 lines | finish loading · sort A to Z | 0.113 s · 0.619 s (average of 3) | vendor | [emeditor.com, "Fast Processing of Large Files", undated, read 2026-10-30](https://www.emeditor.com/text-editor-features/large-file-support/optimized-sort/) |
| EmEditor Professional 25.2.906 (same machine) | 50 GB, 639,132,990 lines | finish loading | 20.825 s (average of 3) | vendor | same page |
| EmEditor Professional 25.2.906 (same machine) | 15.2 GB CSV from the CDC, 106,219,502 records | sort on the date column `cdc_case_earliest_dt` | 7.343 s (average of 3) | vendor | same page |
| SmoothCSV | "a million-row file" | open | "about 3 seconds"; no hardware or file given | vendor (its blog, which says it is "not a full benchmark run under identical conditions") | [smoothcsv blog, 2026-05-18](https://smoothcsv-website.pages.dev/blog/best-csv-editors-in-2026) |
| Caxton (macOS) | 2 M rows | parse | 2.6 s; no hardware given | vendor (its maker, on Product Hunt) | [launch post, 2026-07-21, read on hunted.space](https://hunted.space/dashboard/caxton) |

As of 2026-10-30, no published import time was found for Google Sheets, nor any figure for Ron's Data Edit, CSVed, Delimit, Rainbow CSV, Gigasheet or Row Zero. Pull requests adding a tool to `run.py` are welcome — it only needs a way to drive it from a script.

## The video

[Watch it on YouTube](https://youtu.be/h-6QeIcTGmo). `benchmark/video.py` puts together a title card, the film of the app, the results above as bars, and a closing card. `app/examples/film.rs` films the real app on the 5 M-row file, off screen, at 1920 × 1080 — open, scroll, sort, sort again, filter:

- The app's clock is the wall clock. Each frame is rendered through wgpu and kept with the moment it shows, and the film is built from those moments at 30 frames a second: an operation of 40 ms lasts 40 ms on screen, to within a frame (33 ms).
- A timed operation runs as in `bench_ui`, and no frame is rendered during it, so capturing costs it nothing: the frame before it holds for the time measured, and the result is stamped with the moment the clock stopped.
- Captions show the times measured in that very take. They differ from the medians above: one take, a CPU already awake (the table below), and the film's sort by `total_spent` starts with the first click, smallest first; the second click, largest first, finds the column already read. Pauses for reading are between operations, never inside one.
- The bars all advance at the same speed, so their relative lengths are the true ones; the film says by how much that speed is above real time.

```sh
cargo build --release -p csvfab --example bench_ui --example film
python3 benchmark/run.py --json results.json
python3 benchmark/video.py --results results.json --check   # needs ffmpeg and a Chromium → benchmark/data/csvfab-demo.mp4
```

`--check` reads the film back and compares the frame where the screen really changes with the time measured. `demo.gif`, on the project's home page, is cut from the same film, in the pauses between operations, never inside one; the video is not cut.

The film published here: 34.8 s of app, 1,036 frames rendered (9.6 ms each, median; 21 ms at most) and 1,045 written, one frame every 33 ms except while an operation runs. Read back from the final mp4, every operation that changes the picture changes it on the very frame its measured time falls on (`sort_text`: 44 ms, the first frame at or after that). The times of the film and of `bench_ui`, at 5 M rows:

| | film (the published take, then 2 more) | `bench_ui`, from idle (Results) | `bench_ui`, one other core kept busy |
|---|---:|---:|---:|
| Open | 34 / 35 / 34 ms | 65 ms | 35 ms |
| Sort numbers, first | 106 / 109 / 110 ms, smallest first | 117 ms, largest first | |
| Sort text, first | 44 / 45 / 45 ms | 57 ms | 47 ms |
| Filter, "lyon" in one go | 57 / 55 ms (the 2 other takes, under the sort by name) | 71 ms | |

Rendering keeps the film's CPU awake: that, not the film, is the gap, as the last column shows. Capturing slows nothing. In the published take the filter is typed letter by letter, so its caption gives each letter's time.

## History: csvfab 1.x

csvfab 1.x was a web app (a Python server and a page in Chromium), measured on 2026-10-03 with these same files and definitions, by scripts now in the [v1.25.1](https://github.com/fabiochelly/csvfab/tree/v1.25.1) sources. There, Open ended at the first frame of the grid, and memory included Chromium's own, about 0.55 GB before any file was opened. VisiData 3.4 and Modern CSV were then on version 1 of the generated files. csvfab 2, the native app, against 1.x, operation by operation: [native-vs-web.md](native-vs-web.md).

**100 k rows × 15 columns · 15 MB**

| | csvfab | VisiData 3.4 | csvfab is |
|---|---:|---:|---:|
| Open | **168 ms** | 196 ms | 1.2× faster |
| Filter (column contains) | **20 ms** | 103 ms | 5.0× faster |
| Sort numbers | **52 ms** | 218 ms | 4.2× faster |
| Sort text | **37 ms** | 131 ms | 3.5× faster |
| Peak memory | **743 MB** | 206 MB | 3.6× more memory |

**1 M rows × 15 columns · 153 MB**

| | csvfab | VisiData 3.4 | csvfab is |
|---|---:|---:|---:|
| Open | **614 ms** | 2.2 s | 3.6× faster |
| Filter (column contains) | **157 ms** | 1.0 s | 6.6× faster |
| Sort numbers | **405 ms** | 4.0 s | 9.8× faster |
| Sort text | **291 ms** | 2.1 s | 7.4× faster |
| Peak memory | **1.1 GB** | 1.3 GB | 1.2× less memory |

**5 M rows × 15 columns · 771 MB**

| | csvfab | VisiData 3.4 | csvfab is |
|---|---:|---:|---:|
| Open | **2.7 s** | 13 s | 5.0× faster |
| Filter (column contains) | **913 ms** | 5.6 s | 6.1× faster |
| Sort numbers | **2.5 s** | 25 s | 10.3× faster |
| Sort text | **1.5 s** | 11 s | 7.4× faster |
| Peak memory | **2.4 GB** | 6.0 GB | 2.5× less memory |

| | 1 M rows · csvfab | 1 M rows · Modern CSV | 5 M rows · csvfab | 5 M rows · Modern CSV |
|---|---:|---:|---:|---:|
| Open | **614 ms** | 1.5 s (read-only: 1.2 s) | **2.7 s** | 4.6 s (read-only: 3.1 s) |
| Filter (column contains) | **157 ms** | Premium only | **913 ms** | Premium only |
| Sort numbers | **405 ms** | 9.4 s | **2.5 s** | 53 s |
| Sort text | **291 ms** | 4.1 s | **1.5 s** | 24 s |
| Peak memory | 1.1 GB | 1.2 GB | **2.4 GB** | 5.6 GB |
