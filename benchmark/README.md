# csvfab benchmark

Measure it yourself: one script generates the files, runs every tool on them, checks every result, and prints the tables.

```sh
git clone https://github.com/fabiochelly/csvfab && cd csvfab
python3 benchmark/run.py                    # 100 k, 1 M and 5 M rows (~1 GB of files), csvfab and VisiData
python3 benchmark/run.py --rows 1000000     # one size
python3 benchmark/run.py --tools csvfab     # csvfab alone
python3 benchmark/run.py --help
```

Needs Python 3.8+ and a Chromium-based browser (Chrome, Chromium, Brave, Edge), nothing else. VisiData is used if `import visidata` works, if `vd` is on the `PATH`, or through [`uv`](https://docs.astral.sh/uv/) (`uv run --with visidata`, nothing installed); otherwise it is skipped. Memory is sampled on Linux only. The full run takes about 10 minutes and needs ~9 GB of free RAM at 5 M rows (VisiData alone takes 6 GB).

## What is measured

**The files** (`benchmark/gen.py`): a typical customer export — `id, first_name, last_name, email, phone, company, street, postal_code, city, country, signup_date, last_order, orders, total_spent, status` — six countries, accents, quoted fields with commas inside, ~160 bytes per row. Same seed everywhere: every machine measures the same bytes. Files go to `benchmark/data/` and are reused.

**The operations**, the same for every tool:

| | |
|---|---|
| **Open** | the whole file loaded and ready — for csvfab, until the first frame of the grid is painted (1920 × 1080 window) |
| **Filter** | rows whose `city` contains `lyon`, case ignored |
| **Sort numbers** | by `total_spent`, largest first |
| **Sort text** | by `last_name`, A → Z |
| **Peak memory** | the whole tool, every process of it (PSS), sampled every 50 ms during one more open + filter + sorts |

**Correctness first**: the generator counts what each tool must find (the rows, the rows in Lyon, the largest amount). A tool whose result differs gets no time in the table, only the reason.

**Fair conditions**: the file is read once before measuring, so every tool reads it from the OS cache; each figure is the median of `--repeat` runs (3 by default); each tool runs in its own processes, one after the other.

- **csvfab** runs for real: `server.py` and the page in a headless Chromium with throwaway profile and state folder, driven over the DevTools protocol. Each operation calls what the interface calls (the column filter box, a click on a column title) and waits for the next painted frame. Every open is cold: the tab closed and the reopen cache emptied first (a file seen before reopens about twice as fast).
- **VisiData** runs in its own Python, a fresh process per run (`benchmark/vd_driver.py`), through what its commands do: open and wait for the load to finish, `|` select-col-regex, `#` type-float then `]` sort-desc, `[` sort-asc. Nothing is drawn (no terminal), and undo is off (it would copy the list of rows before each sort): both favour VisiData slightly, if at all.
- csvfab sorts text like a dictionary (`Intl.Collator`, accents and numbers in the text understood); VisiData compares code points. The comparison is not to csvfab's advantage.
- csvfab's memory includes Chromium's own, about 0.55 GB before any file is opened.

## Results

Measured on Intel(R) Core(TM) Ultra X7 358H (16 threads, 31 GB RAM), Linux 7.2.5-3-omarchy, Chromium 152.0.7977.82 Arch Linux, Python 3.14.7 — `python3 benchmark/run.py`, median of 3 runs.

**100 k rows × 15 columns · 15 MB**

| | csvfab | VisiData 3.4 | csvfab is |
|---|---:|---:|---:|
| Open | **146 ms** | 196 ms | 1.3× faster |
| Filter (column contains) | **21 ms** | 103 ms | 5.0× faster |
| Sort numbers | **52 ms** | 218 ms | 4.2× faster |
| Sort text | **39 ms** | 131 ms | 3.4× faster |
| Peak memory | **719 MB** | 206 MB | 3.5× more memory |

**1 M rows × 15 columns · 153 MB**

| | csvfab | VisiData 3.4 | csvfab is |
|---|---:|---:|---:|
| Open | **599 ms** | 2.2 s | 3.7× faster |
| Filter (column contains) | **204 ms** | 1.0 s | 5.1× faster |
| Sort numbers | **410 ms** | 4.0 s | 9.7× faster |
| Sort text | **287 ms** | 2.1 s | 7.5× faster |
| Peak memory | **1.1 GB** | 1.3 GB | 1.2× less memory |

**5 M rows × 15 columns · 770 MB**

| | csvfab | VisiData 3.4 | csvfab is |
|---|---:|---:|---:|
| Open | **2.7 s** | 13 s | 4.9× faster |
| Filter (column contains) | **1.1 s** | 5.6 s | 5.1× faster |
| Sort numbers | **2.4 s** | 25 s | 10.6× faster |
| Sort text | **1.8 s** | 11 s | 6.3× faster |
| Peak memory | **2.4 GB** | 6.0 GB | 2.5× less memory |

## Spreadsheets cannot open these files

| | Rows per sheet | Source |
|---|---|---|
| Microsoft Excel | 1,048,576 | [Excel specifications and limits](https://support.microsoft.com/en-us/office/excel-specifications-and-limits-1672b34d-7043-467e-8e27-269d656771c3) |
| LibreOffice Calc | 1,048,576 | [Calc FAQ](https://wiki.documentfoundation.org/Faq/Calc/022) |
| Apple Numbers | 1,000,000 | Apple, quoted by [Michael Tsai, 2020](https://mjtsai.com/blog/2020/04/02/opening-large-csv-files-in-numbers-10-0) |
| Google Sheets | 20 million cells or 100 MB per spreadsheet | [Google Drive help](https://support.google.com/drive/answer/37603) |

The 5 M-row file does not fit in any of them: Excel and Calc load the first 1,048,576 rows and drop the rest (with a warning). The 1 M-row file (153 MB) is already over Google Sheets' 100 MB.

## Published figures for other tools — not measured by us

Collected from their sources on 2026-10-03, for an order of magnitude only: different files, hardware and years, and "open" does not mean the same thing everywhere (end of a progress bar, first screen, fully editable). **Vendor** = the tool's own publisher; **competitor** = the publisher of another tool.

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
| Tablecruncher 1.8 (Mac mini M2) | 2 GB, 16 M rows | open | 32 s | vendor | [README, 2025](https://github.com/Tablecruncher/tablecruncher) |
| Tad (2019 MacBook Pro) | 230 MB, 450 k rows | open | < 5 s | vendor | [DuckDB docs, archived 2022](https://web.archive.org/web/20220807160349/https://duckdb.org/docs/guides/data_viewers/tad) |
| EmEditor 22.5 (i9-11900K) | 123 MB, 700 k lines | open · sort | 0.19 s · 0.17 s | vendor | [PDF, 2023](https://download.emeditor.info/doc/working-with-csv.pdf) |

EmEditor is a text editor with a CSV mode, and Windows-only; its open time is to the end of loading while the file can already be scrolled. No published import time was found for Google Sheets, nor any figure for Ron's Data Edit, CSVed, Delimit, Gigasheet or Row Zero. Pull requests adding a tool to `run.py` are welcome — it only needs a way to drive it from a script.

## The video

[Watch it on YouTube](https://youtu.be/4sEEdzOZrWk). `benchmark/video.py` films csvfab on the 5 M-row file — open, scroll, sort, filter — in a headless Chromium, frame by frame in real time, then shows the results above as bars that grow at the pace of the measured times. Every time on screen is measured while filming (the app's own) or read from `run.py`'s JSON.

```sh
python3 benchmark/run.py --json results.json
python3 benchmark/video.py --results results.json     # needs ffmpeg → benchmark/data/csvfab-5m.mp4
```
