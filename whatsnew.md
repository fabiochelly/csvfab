# What's new in csvfab

Newest first. Every version is on the [releases page](https://github.com/fabiochelly/csvfab/releases).

## 1.14.0 · 2 October 2026

**Security**
- **Formulas can no longer run arbitrary code.** A formula is checked before it runs: columns, the functions of the ƒx list, their methods, operators and the usual text, number and date methods get through; anything else — `window`, `fetch`, `eval`, assignments, `new`… — is refused with a message saying what is not available. A formula pasted from someone else can no longer read your files. Every documented formula works exactly as before, just as fast.
- **The window cannot send anything to another site.** Whatever runs in it, nothing can be sent anywhere but csvfab's own local server: no request, no image, no form to another address.
- **Booby-trapped files tested.** File names, column titles and values crafted to inject code into the window are checked in every view (grid, row card, panels, dialogs, comparisons, tooltips…): they are always shown as text.

**Fixes**
- **Two-digit years in Excel exports:** `31/12/99` is now 1999, as on screen (it was 2099).
- **Excel export of columns full of unique values** (IDs, amounts) no longer piles them up in memory — and is slightly faster.
- **Opening a SQLite database lists its tables at once**, however large they are; their row counts are shown as approximate (≈).
- **Two backups in the same second no longer overwrite each other** (`-2`, `-3`… are added), beside the file and in the backup folder.
- **Large Excel workbooks open with far less memory** (a 300 000-row sheet: 24 MB → 1 MB at peak), and a booby-trapped archive (zip bomb) is refused.
- **Saves survive a power cut** even right after they finish: the folder holding the file is written to disk too.

**Under the hood**
- **Security tests** run in a real, windowless Chromium: the hostile-file walk through every view (no injected code may run, no element may appear), some 55 attempts to escape the formula language, every documented formula compared row by row with the previous engine, and the window's outgoing requests blocked.
- **The external audit's findings were each checked against the code:** the ones still true are fixed above; those already fixed in 1.13.0 (temporary file names shared between simultaneous requests, permissions lost on save, no test suite, a single-file server) are listed there.

## 1.13.0 · 2 October 2026

- **Saving an unchanged file gives back its exact bytes**, whatever the file holds — valid or not — and an edit changes only the bytes of its own line. Checked on a corpus of 41 broken files: quoted line breaks, mixed line endings, BOMs, UTF-16 with or without BOM, mixed encodings, unclosed quotes, NUL bytes, random bytes. Fixed on the way: a quoted header line lost its quotes (a title holding the delimiter broke the file), a file without a final line break got one, an empty file became a line break, blank lines at the top were dropped, and a UTF-16 file was rewritten (quotes, BOM, invalid characters) — all now kept byte for byte.
- **Why does this row look wrong?** In the row card (*Why?*), the row menu and the palette: expected and observed field counts and the probable cause — an unquoted delimiter (in which column, found from the kind of value each column holds), a line cut by an unquoted line break, a quote never closed or running on past its line and swallowing the next rows, missing fields — the file's bytes around each problem in hex, UTF-8 lines in a Windows-1252 file, garbled accents, NUL and invisible characters, cells a spreadsheet would run as formulas. A repair is suggested when it is safe: quote the cut fields as one, join the cut line, pad, re-read a line as UTF-8 — an ordinary edit, undoable.
- **Huge fields no longer freeze the window:** a 5 MB value used to hang it; it now opens in a third of a second.
- **The last column is fully reachable:** when every row fitted on screen, its end stayed hidden under the scroll strip (and always would have with macOS overlay scrollbars).
- **A slightly faster start:** the window opens a few milliseconds sooner, the page and its script come from memory.
- **Saving keeps the file's permissions:** a private file stays private.
- **Following a growing log** no longer risks confusing the connection when lines arrive during a read.

**Under the hood**
- **The local server is split into modules** (`bridge/`: configuration shared with the launcher, window session, safe writes, HTTP routes, Excel and SQLite formats, fonts) instead of one 1 600-line file; `server.py` only opens the port and starts it. Conversions are loaded only when first used, so the server answers twice as fast after starting (43 → 27 ms), and the launcher skips two heavy imports before starting the browser (18 → 13.5 ms).
- **Simultaneous writes are safe:** two requests writing at the same moment no longer share a temporary file name, and an unexpected error answers cleanly instead of dropping the connection.
- **A test suite** (`python3 -m unittest`, standard library only): the server and the launcher tested from outside as the app uses them, each module on its own, the byte-for-byte guarantee checked in the real page on a generated corpus of broken files (`tests/corpus.py`), the row diagnosis, the grid's edges. Deliberately broken code makes it fail.
- **A performance guard** (`tests/bench.py`): the previous version and the new one measured alternately, on the same processor core, each change judged on its pairs of runs — nothing may get slower.
- **One version number** (`bridge/config.py`) instead of two; the packages (AUR, Homebrew, Windows installer, macOS app) ship the new `bridge/` folder, and the release archives no longer carry the tests.

## 1.12.0 · 1 October 2026

- **Duplicates that sound alike.** A *Sounds like* cleaning in the duplicates dialog: a Metaphone adapted to French groups Dupont and Dupond, Lefebvre and Lefèvre, Philippe and Filippe, Schmitt and Schmidt, and a spelling tolerance (very close, close, loose) keeps apart what only sounds alike (Martin and Martine). Also as formula helpers: `{Nom}.phonetic()` and `similarity(a, b)`.
- **SQLite databases.** Open a `.sqlite`, `.sqlite3`, `.db` or `.db3` file: pick a table (or a view), it opens as a CSV beside the database. Save any tab as a SQLite database of one table, its columns typed from their values (postal codes and phone numbers stay text).
- **Instant reopening of big files.** From 50 MB, the file's line index is kept, and reopening the same file skips reading it through (a 147 MB file: 460 → 215 ms; much more on files of several GB). Entries unused for 30 days, or past 1 GB in all, are cleaned up; the palette can clear them.
- **Follow a log as it grows (tail -f).** On a raw text file, *Follow* in the status bar adds new lines as they are written, keeps the view at the end and applies the filters to them; it pauses while you have edits pending.

## 1.11.1 · 1 October 2026

- **A drawn ƒx sign:** a long italic f whose crossbar runs into a smaller x, one ligature, on the filter box's button, the computed column's *ƒx Functions* and the function picker; it takes each button's colour in every theme.

## 1.11.0 · 1 October 2026

- **IDs and hashes in formulas:** `uuid()` (random), `uuid7()` (sorts in creation order), and `md5`, `sha1`, `sha256`, `sha3_256`, `sha3_512`, `blake2b`, `blake3` of a value, in hex as the usual command-line tools print them: `{Email}.lower().trim().sha256()`. An empty cell stays empty. A new *IDs & hashes* family in the ƒx picker.
- **The computed column relies on the ƒx picker alone:** its *Examples and functions* list is gone, the picker already shows every function with examples run on your data.

## 1.10.0 · 1 October 2026

- **Text files, line by line.** Logs, code, configuration, any file that is not a table opens in *Raw text* mode: one line per row, every space and empty line kept, saved back byte for byte (no quotes added, no line break added at the end). It is the last choice of the delimiter menu in the status bar, so a `.txt` that really is a table switches back in one click, and a CSV can be read as plain lines.
- **Syntax colours** for some forty languages recognised by the file name (JavaScript, TypeScript, Python, shell, SQL, CSS, HTML, XML, JSON, YAML, TOML, INI, Markdown, C, C++, C#, Java, Go, Rust, PHP, Ruby, Lua, diff, logs…), in each theme's own VS Code colours. Logs show dates, levels (ERROR, WARN, INFO, DEBUG), URLs, keys and numbers. Search and find marks stay over the colours.
- **Ln and Col** in the status bar, as in a code editor: the selected line, and while a line is edited the caret's column (tabs count as 4) and the size of the selection. Tab inserts a tab in a line being edited.
- **A faint stripe every other line** in text mode, instead of the grid's rules.
- **JSON asks first:** flatten its records into a CSV table, or open the file itself as raw text.
- **Instant, shorter tooltips.** Every tooltip shows at once, small and quiet, and says only what the label does not. A tab's tooltip gives the file's full path.

- **The command palette moves to Ctrl+P** (Ctrl+Shift+P still works).
- **Duplicates by keys.** Rows are duplicates when any key matches: for example last name + first name + postal code, **or** e-mail, **or** phone. Within a key, every field must match. *Suggest keys* builds these three from the columns it recognises.
- **A cleaning per field:** ignore case and spaces, exact, slug (no accents or symbols), digits only, phone number, e-mail, or a formula on `v` with any function of the ƒx list, such as `firstNumber(v)` or `left(slug(v), 5)`.
- **Chained groups:** a row sharing an e-mail with one row and a phone with another joins them into one group.
- **Merge from the marks.** Hovering a marked row shows a bar about that row: delete it, or *Merge*, which folds the other rows of its group into it. Its filled values stay, its empty cells take the first value found in the others, and the others go. While the pointer is on the button, a check replaces the number of the row that stays and a red cross the number of each row that goes; the buttons stay at the left when the grid scrolls sideways.
- **Differences shown:** within a group, a column whose values differ is marked in red on every row, including a value one row has and another lacks.
- **The hovered group is framed** in dashed yellow, so it is clear which rows belong together.
- **Copy one cell to its sister:** hovering a differing cell shows ▲ and ▼ to copy its value into the same column of the row above or below, nothing else, for instance to keep a value before merging.
- **Ctrl+W** closes the active tab (asking first when it has unsaved edits); Ctrl+Q quits.

- **Formulas read left to right:** every function also works as a method of the value, `{City}.upper()`, `{Date}.year() === 2024`, `{Amount}.num().round(2)`, chained as needed. Examples, the ƒx picker and the autocomplete after a dot use this form.
- **Interface font on every system:** Inter when installed, else the system's own (SF Pro on macOS, Segoe UI on Windows, Roboto or the desktop's font on Linux).
- **Keyboard shortcuts panel** (F1 or ?, the palette, the ☰ menu): every shortcut by theme.
- **A lighter welcome screen in frosted glass:** a barely visible granite background, clear glass panes for the drop zone (which also takes Excel and JSON files) and the recent files, the command palette and the shortcuts.
- **Choose the monospace font** (palette › *Monospace font…*): every monospace font installed on the system, each on a card with a sample sentence and the characters that tell fonts apart, and a search; the choice is kept, and column widths follow the new font.
- **Install free fonts in a click:** Cascadia Code, the Monaspace family, JetBrains Mono, Fira Code, Geist Mono, Commit Mono, Maple Mono, Intel One Mono, 0xProto, Source Code Pro, Hack and Meslo LG (the free counterpart of Menlo and Monaco), downloaded from their official releases and installed for your account only; csvfab then restarts to use the font, the open files coming back.

## 1.9.0 · 1 October 2026

- **Filters search by words.** A simple filter is now a list of words that must all be found, in any order: anywhere in the row for the search box, in the cell for a column filter. `dupont lyon` finds Lyon's Duponts whatever the order of the columns.
- **Exact phrases in quotes.** `"le havre"` keeps the words together.
- Each word is highlighted on its own. Regex mode is unchanged.

## 1.8.0 · 1 October 2026

**Formulas**
- **Function picker.** A **ƒx** button joined to the filter box, another beside the computed column's formula, and a palette command open a searchable list of every function, operator and column. Each example runs on the selected row and shows its result. Inserting places the most likely column in the first argument and selects the next one.
- **25 new functions:** `abs`, `sum`, `avg`, `min`, `max`, `months`, `years` (an age), `addMonths`, `weekday`, `week`, `split`, `mid`, `firstNumber` (`"68 - Rhin"` gives `68`), `digits`, `startsWith`, `endsWith`, `matches`, `isEmail`, `isPhone`, `isNumber`, `isDate`, `ifs`, `cases`.
- A column is a string, so its own methods work too: `{Code}.startsWith("76")`.

**Grid**
- Numbers are right-aligned, so their units line up.
- Numbers and dates are coloured by column type. A colour wheel in the status bar switches the colours off.
- **Freeze the first column** (menu › Columns, palette): it stays in view when scrolling sideways.

**Column panel**
- Data bars are now a thin rule at the base of the cell, and stay visible on selected cells.
- A **median** for numbers, clickable: it selects the first cell holding it.
- The histogram tells each bar's values and count in an instant tooltip.

**Excel**
- A workbook with **several sheets holding data asks which one to open**. Empty and hidden sheets are skipped, so a single sheet of data opens as before. The CSV is named after the sheet: `book - Sheet.csv`.
- Without a choice, the first visible sheet with data is converted, even when the first sheet is empty.

**Fonts**
- Monospace text uses MonoLisa, then Cascadia Code, Monaco, JetBrains Mono and Consolas, whichever is installed.
- Ligatures are off in text fields: the first `&` of `&& ` used to vanish while typing.

## 1.7.1 · 30 September 2026

- The files open at quit are no longer reopened at the next start. The window opens on the welcome screen, with the recent files one click away.

## 1.7.0 · 30 September 2026

- **Redo**, with Ctrl+Y or Ctrl+Shift+Z, the menu or the palette.
- **Find without filtering** (Ctrl+F, F3, Shift+F3): every match in the rows shown is highlighted, and next / previous step through them with a "3 / 128" count. **Match case** applies to find and replace alike.
- A double-click on a column's resize handle fits it to its content. The palette fits every column or resets the widths.

## 1.6.0 · 30 September 2026

- **Horizontal scroll strip** over the grid's horizontal scrollbar: a draggable thumb, the column names laid along the band, a tip naming the column under the pointer, the sort columns in colour and the selected columns marked.
- Row numbers keep their accent border.
- Homebrew cask fixes.

## 1.5.0 · 30 September 2026

- **macOS application.** `csvfab.app` is built on the Mac itself, by `install.sh` into `~/Applications` or by a Homebrew cask into `/Applications`, so Gatekeeper does not block it. It opens CSV and TSV files from the Finder.
- The grid's rows are drawn as independent blocks instead of a table: every scroll step costs less.
- `install.sh` recognises a checkout however it is invoked.

## 1.4.0 · 30 September 2026

- Lighter scrolling: cells are clipped only when their text overflows, and the row numbers stick only when the grid scrolls sideways.
- The scroll strip's row ticks fall on round numbers (100, 200…).
- The *Edit* submenu is folded into the menu and *Rows*.

## 1.3.0 · 30 September 2026

- **Scroll strip** over the vertical scrollbar: a draggable thumb with the row number in a tip, marks for duplicates, irregular and marked rows, and labels along the band (row numbers, or the sort column's values).

## 1.2.2 · 30 September 2026

- **Smooth scrollbar drags**, even on millions of rows: no more blank bands while the thumb is dragged.
- Faster on large files: expression filter, duplicates, anonymise, column panel, formulas, Excel export.
- No browser autocomplete on text fields.

## 1.2.1 · 29 September 2026

- Expression filter: autocomplete of column names and functions.
- Releases publish to the Homebrew tap and winget automatically.

## 1.2.0 · 29 September 2026

- **Row card** (Ctrl+I): the selected row as a form on the right, editable, with its raw line as it sits in the file.
- **Go to row** (Ctrl+G).
- **Filter by expression:** the search box becomes a formula, `num({Amount}) > 1000 && contains({City}, "lyon")`.
- **File profile:** every column's type, fill, distinct values, min, max, sum and average in one table.
- **Anonymise:** names, e-mails, phones, addresses, companies and dates replaced by consistent fakes, column by column.
- **Group by:** counts and sums per group, opened as a new tab.
- **Open Excel and JSON files:** converted to a CSV beside them, which becomes the tab.
- Submenus in the main menu.

## 1.1.0 · 29 September 2026

**New tools**
- **Look up** from another tab (VLOOKUP).
- **Convert formats:** dates, numbers and phone numbers of a column into one format.
- **Computed column:** a formula per row, with helpers for French numbers, dates and text.
- **Duplicate marks:** each group shown side by side before any deletion.
- **Clean up:** garbled accents (mojibake), invisible characters, odd spaces, line breaks, empty rows and columns, in one undo step.
- **Multi-column sort** with Shift+click.
- **Review changes:** what Save would write, against the file on disk.
- **Compare with another tab:** mark the differences, add a status column, or append the missing rows.
- **Fill empty cells from above.**

**Speed**
- **Huge files:** the file stays as bytes and rows are decoded on demand. 2 GB and 22.8 million rows open in about 12 s. Saving copies untouched rows byte for byte.
- Smooth scrolling, sideways too: only the rows and columns around the view are drawn.
- Instant filtering while typing.
- Faster gestures, faster opening, faster launch.
- Files opened while the app runs appear at once.

**Grid**
- Column numbers in the titles, F2 and double-click to edit, Home / End and Ctrl+Home / Ctrl+End.

## 1.0.2 · 28 September 2026

- **Six themes:** GitHub Dark, Tokyo Night, Catppuccin Mocha, VS Code, GitHub Light and Catppuccin Latte, picked from a swatch in the status bar.
- **Command palette** (Ctrl+K or Ctrl+Shift+P): every action, setting, tab, recent file and column command, fuzzy-searched.
- **Toasts** with Undo after each edit.
- **Welcome screen** with a drop zone and the recent files.
- Type icons in the headers, a histogram in the column panel, optional data bars in numeric cells, and discreet motion.
- The launcher is `csvfab.py`, still installed as the command `csvfab`.

## 1.0.1 · 28 September 2026

The first public release, and everything built before it: a desktop CSV editor that writes back to the very file it opened.

**Opening files**
- From the command line (`csvfab a.csv b.tsv`), the file manager's "Open with", the file picker (Ctrl+O, several files at once) or a drag and drop of files or whole folders onto the window.
- A file opened while the app runs lands as a new tab in the window already open, and a file already open is simply focused.
- **Tabs**, switched with Alt+← / Alt+→ or Alt+1…9. Only the active tab is parsed; the least recently used clean tabs are released from memory past a "Keep in RAM" count and read again from disk when reopened. A tab with pending edits is never released.

**Reading any CSV**
- **Delimiter** (`;`, `,`, tab, pipe…), **encoding** and **header line** detected, each shown as a pill in the status bar and changeable from it.
- Encodings: UTF-8 with or without BOM, Windows-1252, ISO-8859-1, ISO-8859-15, Mac Roman and UTF-16. Auto-detection checks the whole file before choosing, so a single accented character on the last line is not missed.
- **Header detection** compares the first line with the types of the rows below, so a file without titles does not lose its first record. A header can be turned into data, or the other way round, without reading the file again.
- Quoted fields, line breaks inside cells, blank lines and **irregular rows** (too many or too few fields) are handled; irregular rows and quote errors are counted in a status bar chip that filters them.

**Saving in place**
- **Save** (Ctrl+S) overwrites the source file. A timestamped `.bak` copy is written before the first overwrite of each session, and every write is atomic: a crash cannot truncate the original.
- The file is written back with its own delimiter, encoding, BOM and line endings; the line endings can be switched between CRLF and LF from the status bar.
- **Changed on disk:** if another program modified the file since it was read, Save offers to overwrite, reload or cancel.
- **Save as:** another name, another delimiter, another encoding (with a warning listing the characters the encoding cannot hold), or an **Excel workbook** with typed numbers, percentages and dates, a bold frozen header and an autofilter.
- **Extract** writes the rows shown to a new file, numbered after the source, and opens it in a new tab.
- **Discard** drops every pending edit and reloads the file as it is on disk.
- Pending edits turn the Save button yellow; quitting with unsaved edits asks first.

**Filtering**
- A **search across all columns**, a **filter per column** in the header, and options for **regex**, **ignoring accents** and **inverting** the match. Matches are highlighted in the cells.
- A **column panel** (the ▾ of each title) profiles the column — type, distinct values, empty cells, min and max, sum and average for numbers — and lists its values by frequency, to tick the ones to keep.
- The status bar shows the rows shown out of the total.

**Editing**
- **Cells:** type over a selection, Enter or F2 or a double-click to edit, Shift+Enter for a line break inside a cell, Delete to clear. With several cells selected, Enter writes the value into all of them and Ctrl+Enter fills them with a series.
- **Rows:** insert above or below, duplicate, delete, delete the selected rows, move a row by dragging its number.
- **Columns:** add, delete, rename (double-click the title), move (drag the title, or ◀ ▶ in the column panel), resize, show and hide.
- **Sort** by clicking a title, again to reverse: numbers, dates and text each sorted their own way, French decimal commas and day-first dates included.
- **Find and replace** in the rows shown, in one column or all, with regex and `$1` references.
- **Edit filtered rows:** one change over a column in every row shown: set a value, clear, trim spaces, UPPER, lower, Capitalised.
- **Split a column** at a separator or regex, with a preview and the distribution of parts; **merge columns** with a separator, skipping empty values if wanted.
- **Remove duplicates** on chosen columns, compared exactly, ignoring case and spaces, or ignoring accents and symbols, keeping the first or the last.
- **Delete hidden rows:** keep only what the filters show.
- **Undo** (Ctrl+Z) every edit, one at a time, back to the last save.

**Selection and clipboard, like a spreadsheet**
- Click, drag and Shift+click to select ranges; whole rows from their numbers; Ctrl+A for everything shown; arrows, Ctrl+arrows, Page Up / Down to move. The status bar shows the range's size, and the sum and average of its numbers.
- **Copy and paste with Excel and Sheets:** tabs and line breaks as they expect. A pasted block may add rows at the end.
- **Fill handle** at the corner of the selection: drag it to extend a series of numbers, dates, weekday or month names, or text ending in a number, or to copy with Ctrl held. **Ctrl+D** fills a series down.

**The app**
- Runs as its own window, started by `csvfab`, through a small local server written with Python's standard library only. It listens on this computer alone and every request carries a session token.
- Runs on **Linux, macOS and Windows**, with per-user installers, a Homebrew formula, a Windows installer and an AUR recipe, built by the release workflow on every version.
- Dialogs are drawn by the app itself, never the browser's; Ctrl+Q quits.
