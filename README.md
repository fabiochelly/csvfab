<p align="center"><img src="icons/csvfab.svg" width="96" alt="csvfab"></p>

# csvfab

A desktop CSV editor that **edits the file in place** — fast on files of several GB, with no build step and no dependency beyond Python and a Chromium-based browser.

- **Open anything**: tabs, drag & drop, "Open with" from your file manager; delimiter (`;` `,` tab `|`), header line and encoding (UTF-8, Windows-1252, ISO-8859-1, ISO-8859-15, Mac Roman, UTF-16) detected, and changeable from the status bar. An Excel workbook or a JSON file is first written as a CSV beside it, then opened.
- **Find what matters**: global and per-column filters (accents ignored, regex, invert), or a filter written as a formula (`num({Amount}) > 1000 && contains({City}, "lyon")`); a column panel with a profile (type, distinct values, min / max / sum / average) and a filter by value; a profile of the whole file, every column on one line; sort by clicking a title (Shift+click to sort on several columns); group by one or more columns with counts, sums and averages into a new tab; go to a row by number (Ctrl+G); find without filtering (Ctrl+F, F3): every match highlighted in the rows shown, stepped through one by one; a scroll strip over the scrollbar with the initials, years or values of the sort column along it, marks for irregular, duplicate and changed rows, and the row under the pointer as you drag.
- **Read one record**: the row card (Ctrl+I) shows the selected row as a form, every column with its editable value, and its raw line as it sits in the file — delimiters, quotes and invisible characters marked.
- **Edit like a spreadsheet**: in-place and multi-line editing, range selection, copy & paste with Excel or Google Sheets, series fill (drag the corner or Ctrl+D), typing into many cells at once, find & replace with regex, bulk edits on filtered rows, mark or remove duplicates (exact, case-insensitive or slugified), delete hidden rows, split and merge columns, computed columns (formulas), look up values from another open file (like VLOOKUP), convert dates, numbers and phone numbers to one format, clean up the whole file (garbled accents like `Ã©`, invisible characters, odd spaces, empty rows and columns), fill empty cells from the value above, compare two versions of a file on a key column, anonymise a file for a demo (names swapped for others with the same initial, e-mails and phone numbers reshaped, postal codes keeping their department, dates shifted — the same value always giving the same fake), insert / move / rename / delete rows and columns — and **undo** (Ctrl+Z) all the way back to the last save, **redo** (Ctrl+Y) what you undid, with a review of every pending change against the file on disk before you save.
- **Large files**: the file stays as its own bytes in memory, rows are decoded only when shown or searched — a 2 GB CSV of 20 million rows opens in seconds without freezing the window, and a save copies the untouched lines as they are.
- **Write safely**: Save rewrites the file in its own delimiter, line endings and encoding, keeps a timestamped `.bak` before the first overwrite, notices when another program changed the file meanwhile, and flags irregular rows. Save as another name, delimiter or encoding, or as a formatted Excel workbook (typed numbers and dates, bold frozen header, filters).

## Install

| | |
|---|---|
| **Arch Linux** | the script below — the AUR package `csvfab` will follow once the AUR reopens registrations |
| **macOS** (Homebrew) | `brew install --cask fabiochelly/csvfab/csvfab` — csvfab.app in /Applications, offered in the Finder's "Open with" for CSV files (`brew install fabiochelly/csvfab/csvfab` for the command line alone) |
| **Windows** (winget) | `winget install FabioChelly.csvfab` |
| **Linux / macOS** (script) | `curl -fsSL https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.sh \| sh` |
| **Windows** (script) | `irm https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.ps1 \| iex` |

Requirements: **Python 3.8+** and a **Chromium-based browser** (Chrome, Chromium, Brave or Edge) — csvfab opens in its own app window with a dedicated profile, so your usual browser is untouched. The scripts install per user, without admin rights; `install.sh --uninstall` / `install.ps1 -Uninstall` remove it. On macOS, `install.sh` also builds `csvfab.app` in `~/Applications` and offers to make it the default app for CSV files (with [duti](https://github.com/moretension/duti); otherwise: select a `.csv`, File › Get Info › Open with › csvfab › Change All…).

## Use

```sh
csvfab                  # open the window
csvfab data.csv more.tsv
csvfab export.xlsx      # converted to export.csv beside it, which is what opens
```

Files opened while the window is already there land in it as new tabs. A double-click on a column's resize handle fits it to its content.

| Shortcut | |
|---|---|
| Ctrl+O / Ctrl+S / Ctrl+Q | open / save / quit |
| Ctrl+Z / Ctrl+Y | undo / redo |
| Ctrl+F, F3 / Shift+F3 | find in the rows shown without filtering them — every match highlighted, next / previous match |
| Ctrl+C / Ctrl+V | copy / paste a range (Excel format) |
| Ctrl+A, arrows, Shift+arrows | select |
| Home / End, Ctrl+Home / Ctrl+End | first / last cell of the row, of the file |
| Enter, F2, double-click, or just type | edit — Enter selects the text, F2 and double-click put the caret at its end; Enter fills every selected cell, Ctrl+Enter a series, Shift+Enter a new line |
| Ctrl+D | fill the series down |
| Ctrl+I | row card — the selected row as a form, and its raw line |
| Ctrl+G | go to a row by its number |
| Alt+← / Alt+→ | switch tab |

## How it works

`csvfab` starts a small local server (`server.py`, Python standard library only, bound to `127.0.0.1` and protected by a per-session token) and opens `viewer.htm` (with its `ui/` styles and scripts) in a Chromium app window. The page reads the CSV — its records found by a background worker, each row decoded on demand — and renders it; the server reads and writes the files you open, atomically, and builds the Excel workbooks. The server stops by itself a few seconds after the last window closes.

Settings and the browser profile live in `~/.local/state/csvfab` (Linux), `~/Library/Application Support/csvfab` (macOS) or `%LOCALAPPDATA%\csvfab` (Windows).

## License

[MIT](LICENSE) © Fabio Chelly
