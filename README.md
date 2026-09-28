<p align="center"><img src="icons/csvfab.svg" width="96" alt="csvfab"></p>

# csvfab

A desktop CSV editor that **edits the file in place** — fast on files of hundreds of MB, with no build step and no dependency beyond Python and a Chromium-based browser.

- **Open anything**: tabs, drag & drop, "Open with" from your file manager; delimiter (`;` `,` tab `|`), header line and encoding (UTF-8, Windows-1252, ISO-8859-1, ISO-8859-15, Mac Roman, UTF-16) detected, and changeable from the status bar.
- **Find what matters**: global and per-column filters (accents ignored, regex, invert), a column panel with a profile (type, distinct values, min / max / sum / average) and a filter by value, sort by clicking a title.
- **Edit like a spreadsheet**: in-place and multi-line editing, range selection, copy & paste with Excel or Google Sheets, series fill (drag the corner or Ctrl+D), typing into many cells at once, find & replace with regex, bulk edits on filtered rows, remove duplicates (exact, case-insensitive or slugified), delete hidden rows, split and merge columns, insert / move / rename / delete rows and columns — and **undo** (Ctrl+Z) all the way back to the last save.
- **Write safely**: Save rewrites the file in its own delimiter, line endings and encoding, keeps a timestamped `.bak` before the first overwrite, notices when another program changed the file meanwhile, and flags irregular rows. Save as another name, delimiter or encoding, or as a formatted Excel workbook (typed numbers and dates, bold frozen header, filters).

## Install

| | |
|---|---|
| **Arch Linux** (AUR) | `yay -S csvfab` |
| **macOS** (Homebrew) | `brew install fabiochelly/csvfab/csvfab` |
| **Windows** (winget) | `winget install FabioChelly.csvfab` |
| **Linux / macOS** (script) | `curl -fsSL https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.sh \| sh` |
| **Windows** (script) | `irm https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.ps1 \| iex` |

Requirements: **Python 3.8+** and a **Chromium-based browser** (Chrome, Chromium, Brave or Edge) — csvfab opens in its own app window with a dedicated profile, so your usual browser is untouched. The scripts install per user, without admin rights; `install.sh --uninstall` / `install.ps1 -Uninstall` remove it.

## Use

```sh
csvfab                  # open the window
csvfab data.csv more.tsv
```

Files opened while the window is already there land in it as new tabs.

| Shortcut | |
|---|---|
| Ctrl+O / Ctrl+S / Ctrl+Q | open / save / quit |
| Ctrl+Z | undo |
| Ctrl+C / Ctrl+V | copy / paste a range (Excel format) |
| Ctrl+A, arrows, Shift+arrows | select |
| Enter, F2, or just type | edit — Enter fills every selected cell, Ctrl+Enter a series, Shift+Enter a new line |
| Ctrl+D | fill the series down |
| Alt+← / Alt+→ | switch tab |

## How it works

`csvfab` starts a small local server (`server.py`, Python standard library only, bound to `127.0.0.1` and protected by a per-session token) and opens `viewer.htm` in a Chromium app window. The page parses and renders the CSV; the server reads and writes the files you open, atomically, and builds the Excel workbooks. The server stops by itself a few seconds after the last window closes.

Settings and the browser profile live in `~/.local/state/csvfab` (Linux), `~/Library/Application Support/csvfab` (macOS) or `%LOCALAPPDATA%\csvfab` (Windows).

## License

[MIT](LICENSE) © Fabio Chelly
