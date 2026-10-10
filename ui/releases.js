/* What's new: the features each version brought, newest first — shown by openWhatsNew()
   (57-whats-new.js), loaded on its first opening only, so app.js does not carry it at every launch.
   Only what a product page would show: a release that fixed or tuned things has no entry.
   Each item is [title, one sentence]; static text, still escaped where it is drawn. */
window.CSVFAB_RELEASES = [
    { v: '1.25.0', date: '2026-10-10', items: [
        ['Filters at a glance', 'Every filter in force is a chip in the toolbar — even one set from a chart or a column\'s panel — removed in one click.'],
        ['Links, checks and colours', 'Columns of links or e-mail addresses open with Ctrl+click; yes / no columns show as checks, colour codes as swatches.'],
        ['Colour scales', 'Tint a numeric column by value to see its highs and lows at once.'],
        ['Whole values on hover', 'Rest the pointer on a value cut short to read it whole, JSON indented.'],
        ['Freeze up to any column', 'Keep several columns in view while scrolling sideways (right-click a title).'],
    ] },
    { v: '1.24.0', date: '2026-10-10', items: [
        ['Totals row', 'A row held at the bottom of the grid totals the rows shown — sum, average, median, min, max, distinct values… per column, kept up to date as you filter.'],
    ] },
    { v: '1.23.0', date: '2026-10-09', items: [
        ['Category pills', 'A column of a few values shows each one as a coloured pill, in its chart\'s colours.'],
        ['Grouped sorted views', 'A sorted column draws a line between groups and names the group in view.'],
        ['Right-click menus', 'Titles, row numbers, cells and text fields each have their own menu.'],
    ] },
    { v: '1.22.0', date: '2026-10-08', items: [
        ['Fill by example', 'Type two results, csvfab reads the rule from the row\'s other cells and offers to fill the whole column (Ctrl+E).'],
        ['What I noticed', 'The Insights panel points out near keys, outliers, values written otherwise, copied rows and hidden arithmetic between columns.'],
        ['Sorts in motion', 'Rows glide to their new place, so you see what a sort changed.'],
        ['Range filters', 'Drag across a column chart to keep a range of values.'],
    ] },
    { v: '1.20.0', date: '2026-10-07', items: [
        ['Column charts', 'Under each title, the column at a glance: a curve for numbers and dates, the most frequent values for text, how full it is — a click filters.'],
    ] },
    { v: '1.19.0', date: '2026-10-05', items: [
        ['File map', 'The whole file as one picture: empty stretches, values of the wrong type and broken rows stand out at once (Ctrl+M).'],
        ['Every core at work', 'Big files are read and filtered on all the processor\'s cores.'],
        ['Find on the scrollbar', 'Search matches show as ticks along the scrollbar.'],
    ] },
    { v: '1.18.0', date: '2026-10-03', items: [
        ['Pivot table', 'Counts, sums, averages or distinct values by row and column, live on the rows shown — a click on a number shows its rows.'],
        ['Split into files', 'One file per value of a column — per agency, per month — each row copied byte for byte.'],
        ['Combine files', 'Open tabs written into one file, columns matched by their titles whatever their order, delimiter or encoding.'],
    ] },
    { v: '1.17.0', date: '2026-10-03', items: [
        ['Schema validation', 'Learn the rules a good file follows, then check every later file against them — in the Table Schema standard.'],
    ] },
    { v: '1.16.0', date: '2026-10-03', items: [
        ['Remove noise words', 'Take legal forms, titles or any list of words out of a column, with a preview and one undo.'],
    ] },
    { v: '1.14.0', date: '2026-10-02', items: [
        ['Safe formulas', 'A formula can only use columns and the documented functions: one pasted from someone else cannot read your files.'],
        ['Nothing leaves your machine', 'The window cannot send anything to another site, whatever a file holds.'],
    ] },
    { v: '1.13.0', date: '2026-10-02', items: [
        ['Byte-exact saves', 'Saving an unchanged file gives back its exact bytes, valid or not; an edit changes only its own line.'],
        ['Why does this row look wrong?', 'A diagnosis of a broken row — stray delimiter, cut line, unclosed quote, wrong encoding — with a safe repair when there is one.'],
    ] },
    { v: '1.12.0', date: '2026-10-01', items: [
        ['Duplicates that sound alike', 'Dupont and Dupond, Lefebvre and Lefèvre grouped, with a spelling tolerance.'],
        ['SQLite', 'Open a table of a SQLite database, or save any tab as one.'],
        ['Instant reopening', 'A big file opened before reopens without being read through again.'],
        ['Follow a log', 'New lines appear as they are written, filters applied, like tail -f.'],
    ] },
    { v: '1.11.0', date: '2026-10-01', items: [
        ['IDs and hashes', 'uuid(), uuid7(), md5, sha256, blake3… in formulas.'],
    ] },
    { v: '1.10.0', date: '2026-10-01', items: [
        ['Text files', 'Logs, code and configuration open line by line, with syntax colours for some forty languages, and save back byte for byte.'],
        ['Duplicates by keys', 'Rows are duplicates when a name and postal code match, or an e-mail, or a phone — then merged into one.'],
        ['Formulas that read left to right', '{City}.upper(), {Amount}.num().round(2), chained as needed.'],
        ['Keyboard shortcuts', 'Every shortcut in one panel (F1).'],
        ['Monospace fonts', 'Pick any installed font, or install a free one in a click.'],
    ] },
    { v: '1.9.0', date: '2026-10-01', items: [
        ['Search by words', 'Words found in any order, anywhere in the row; quotes keep a phrase together.'],
    ] },
    { v: '1.8.0', date: '2026-10-01', items: [
        ['Function picker', 'Every function, searchable, each example run on your own data — and 25 new functions.'],
        ['Type colours', 'Numbers and dates coloured by column type, numbers right-aligned.'],
        ['Frozen first column', 'It stays in view when scrolling sideways.'],
        ['Excel sheets', 'A workbook with several sheets asks which one to open.'],
    ] },
    { v: '1.7.0', date: '2026-09-30', items: [
        ['Redo', 'Ctrl+Y or Ctrl+Shift+Z.'],
        ['Find without filtering', 'Every match highlighted, next and previous with a count (Ctrl+F).'],
    ] },
    { v: '1.5.0', date: '2026-09-30', items: [
        ['macOS application', 'csvfab.app opens CSV and TSV files from the Finder.'],
    ] },
    { v: '1.3.0', date: '2026-09-30', items: [
        ['Scroll strip', 'A band over the scrollbar with row numbers or sort values, and marks for duplicates and broken rows.'],
    ] },
    { v: '1.2.0', date: '2026-09-29', items: [
        ['Row card', 'The selected row as a form, with its raw line as it sits in the file (Ctrl+I).'],
        ['Filter by expression', 'num({Amount}) > 1000 && contains({City}, "lyon").'],
        ['File profile', 'Every column\'s type, fill, distinct values, min, max and average in one table.'],
        ['Anonymise', 'Names, e-mails, phones, addresses and dates replaced by consistent fakes.'],
        ['Group by', 'Counts and sums per group, opened as a new tab.'],
        ['Excel and JSON', 'Opened as a CSV beside them.'],
    ] },
    { v: '1.1.0', date: '2026-09-29', items: [
        ['Huge files', 'Rows decoded on demand: 2 GB and 22.8 million rows open in about 12 seconds.'],
        ['Look up', 'Bring values from another tab, like VLOOKUP.'],
        ['Computed columns', 'A formula per row, with helpers for numbers, dates and text.'],
        ['Clean up', 'Garbled accents, invisible characters, odd spaces, empty rows and columns, in one step.'],
        ['Convert formats', 'Dates, numbers and phone numbers of a column into one format.'],
        ['Multi-column sort', 'Shift+click a second title.'],
        ['Review changes', 'What Save would write, against the file on disk.'],
        ['Compare two tabs', 'Mark the differences, or add the missing rows.'],
    ] },
    { v: '1.0.2', date: '2026-09-28', items: [
        ['Six themes', 'GitHub, Tokyo Night, Catppuccin and VS Code, dark and light.'],
        ['Command palette', 'Every action, tab, recent file and column, fuzzy-searched (Ctrl+P).'],
        ['Welcome screen', 'A drop zone and the recent files.'],
        ['Visual cues', 'Type icons in the titles, histograms, data bars, undo in every toast.'],
    ] },
    { v: '1.0.1', date: '2026-09-28', first: true, items: [
        ['Saves in place', 'Writes back to the very file it opened, with a backup and atomic writes, keeping its delimiter, encoding and line endings.'],
        ['Reads any CSV', 'Delimiter, encoding and header line detected; irregular rows flagged.'],
        ['Filters', 'A search across every column and one filter per column, with regex and accent-blind matching.'],
        ['Edits like a spreadsheet', 'Selections, copy and paste with Excel and Sheets, fill handle and series.'],
        ['Column tools', 'Sort, split, merge, rename, move, find and replace, remove duplicates.'],
        ['Undo everything', 'Every edit, back to the last save.'],
        ['Linux, macOS and Windows', 'Its own window, a small local server, nothing sent anywhere.'],
    ] },
];
