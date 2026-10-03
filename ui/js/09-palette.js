/* ---------------------------------------------------------------
   COMMAND PALETTE (Ctrl+P, Ctrl+Shift+P)
   Rebuilt at each opening from the current state, so it only offers
   what applies (Undo when there is something to undo, Save when a file
   is loaded…). Fuzzy search: the letters typed must appear in order,
   word starts and runs score higher. Without a query the per-column
   commands stay out, to keep the list short.
----------------------------------------------------------------*/
let pal = { items: [], shown: [], act: 0 };

function clearAllFilters() {
    const t = T(); if (!t) return;
    t.globalQuery = ''; t.colFilters = {}; t.valFilters = {}; t.onlyIrregular = false; t.onlyDups = false;
    if (t.rowMark) t.rowMark.only = false;
    document.getElementById('global-search').value = '';
    renderHeader(); applyColStyles(); applyFilters();
}
function goToColumn(c) {
    const t = T(); if (!t || !t.loaded) return;
    if (t.hiddenCols.has(c)) { t.hiddenCols.delete(c); applyColStyles(); }
    const r = Math.min(t.filteredData.length - 1, Math.max(0, sel && sel.tab === t.id ? sel.fr : Math.floor(container.scrollTop / ROW_H)));
    if (r < 0) { const th = thead.rows[0].cells[c + 1]; if (th) th.scrollIntoView({ inline: 'center', block: 'nearest' }); return; }
    setSel(t, r, c, r, c); revealCell(r, c);
}
function openColPanelFor(c) {
    goToColumn(c);
    const btn = document.querySelectorAll('thead tr:first-child .col-menu')[c];
    if (btn) setTimeout(() => openColPanel({ stopPropagation() { }, currentTarget: btn }, c), 30);
}

function paletteCommands() {
    const t = T(), loaded = !!(t && t.loaded), C = [];
    const add = (group, label, run, hint, ok = true) => { if (ok) C.push({ group, label, run, hint: hint || '' }); };
    const noEv = { stopPropagation() { } };
    add('File', 'Open files…', openFiles, 'Ctrl+O');
    add('File', 'Save', () => saveInPlace(), 'Ctrl+S', loaded);
    add('File', 'Save as… — another name, delimiter, encoding or Excel', openSaveModal, '', loaded);
    add('File', 'Save as Excel workbook', () => saveExcel(), '', loaded);
    add('File', 'Save as SQLite database — one table, typed columns', () => saveExcel(null, 'sqlite'), '', loaded);
    add('File', 'Extract the rows shown to a new file', extractFiltered, '', loaded);
    add('File', 'Discard all edits and reload from disk', discardEdits, '', loaded && isDirty(t));
    add('File', 'Close this tab', () => closeTab(t.id), '', !!t);
    add('File', 'Quit csvfab', quitApp, 'Ctrl+Q');
    add('Edit', 'Undo' + (loaded && isDirty(t) ? ` — ${t.modificationsLog[t.modificationsLog.length - 1].what || 'last edit'}` : ''), undo, 'Ctrl+Z', loaded && isDirty(t));
    add('Edit', 'Redo' + (loaded && t.redoStack.length ? ` — ${t.redoStack[t.redoStack.length - 1].what || 'edit'}` : ''), redo, 'Ctrl+Y', loaded && t.redoStack.length > 0);
    add('Edit', 'Review changes — what Save would write', openReview, '', loaded && isDirty(t));
    add('Edit', 'Find & replace', toggleSRBar, '', loaded);
    add('View', (rowCardIsOpen() ? 'Close the row card' : 'Row card — the selected row as a form'), () => toggleRowCard(), 'Ctrl+I', loaded);
    add('View', 'Raw line — the selected row as it sits in the file', () => { if (!rowCard.raw) rowCardRaw(true); toggleRowCard(true); }, '', loaded && !!sel);
    add('View', 'Why does this row look wrong? — its bytes examined', () => { if (!rowCard.diag) rowCardDiag(true); toggleRowCard(true); }, '', loaded && !!sel);
    add('View', 'Go to row…', goToRow, 'Ctrl+G', loaded);
    add('View', 'File profile — every column at a glance', openProfile, '', loaded);
    add('File', 'Create a schema — the rules this file follows, as name.schema.json…', openSchemaCreate, '', loaded);
    add('File', 'Validate with a schema — check this file against its rules…', openSchemaValidate, '', loaded);
    add('View', 'Insert a function (ƒx) — every helper of formulas, with examples', () => openFxPicker('search'), '', loaded);
    add('View', (document.getElementById('use-expr').checked ? 'Filter by text again (expression off)' : 'Filter by expression — {Amount}.num() > 1000 && …'), toggleExprFilter, '', loaded);
    add('Rows', 'Group by… — count, sum, average per value, in a new tab', () => openGroupBy(), '', loaded);
    add('Edit', 'Anonymise… — names, e-mails, phones, postal codes', openAnon, '', loaded);
    add('Edit', 'Edit filtered rows…', openBulk, '', loaded);
    add('Edit', 'Clean up — spaces, invisible characters, garbled accents, empty rows…', openClean, '', loaded);
    add('Edit', 'Remove noise words — a list of words taken out of a column or every column…', () => openNoise(), '', loaded);
    add('Edit', 'Fill series down', fillDown, 'Ctrl+D', loaded);
    add('Edit', 'Fill empty cells from above', fillBlanks, '', loaded);
    add('Edit', 'Select all rows shown', () => setSel(t, 0, 0, t.filteredData.length - 1, t.headers.length - 1), 'Ctrl+A', loaded && t.filteredData.length > 0);
    add('Rows', 'Remove duplicates…', openDedupe, '', loaded);
    add('Rows', 'Delete hidden rows', deleteHiddenRows, '', loaded);
    add('Rows', 'Mark duplicates…', openDedupe, '', loaded);
    if (loaded && t.dupSpec) add('Rows', t.onlyDups ? 'Show all rows again (leave the duplicates view)' : 'Show only the duplicate rows', toggleDupView);
    if (loaded && t.dupSpec) add('Rows', 'Remove the duplicate marks', () => clearDupMarks());
    add('Rows', loaded && t.onlyIrregular ? 'Show all rows again' : 'Show only irregular rows', toggleIrregular, '', loaded);
    add('Rows', 'Clear all filters', clearAllFilters, '', loaded && hasFilter(t));
    add('Columns', 'Split a column…', () => openSplit(), '', loaded);
    add('Columns', 'Merge columns…', () => openMerge(), '', loaded);
    add('Columns', 'Computed column (formula)…', () => openFormula(), '', loaded);
    add('Columns', 'Look up values from another tab (VLOOKUP)…', () => openLookup(), '', loaded);
    add('Rows', 'Compare with another tab — changed rows, rows only on one side…', openCompare, '', loaded);
    if (loaded && t.rowMark) add('Rows', t.rowMark.only ? 'Show all rows again (leave the marked rows)' : `Show only the ${t.rowMark.label}`, toggleMarkView);
    if (loaded && t.rowMark) add('Rows', 'Remove the marks', () => clearRowMark());
    add('Columns', 'Convert formats — dates, numbers, phone numbers…', () => openConvert(), '', loaded);
    add('Columns', 'Show or hide columns…', openColManager, '', loaded);
    add('Columns', t && t.frozen ? 'Unfreeze the first column' : 'Freeze the first column — it stays in view when scrolling sideways', toggleFreeze, t && t.frozen ? '✓' : '', loaded);
    add('Columns', 'Fit every column to its content (double-click a resize handle for one)', () => fitColumns(visibleCols(t)), '', loaded);
    add('Columns', 'Reset the column widths', resetColWidths, '', loaded && Object.keys(t.colWidths).length > 0);
    add('Help', 'Keyboard shortcuts', openKeys, 'F1');
    add('Help', 'About csvfab… — version, licence, website', openAbout);
    add('View', t && t.tail ? 'Stop following the file' : 'Follow the end of the file (tail -f) — new lines as they are written', () => toggleTail(), t && t.tail ? '✓' : '', loaded && !!t.lang);
    add('View', 'Clear the reopen cache — big files are scanned again', idxClear);
    add('View', 'Monospace font…' + (monoPick() ? ` — ${monoPick() === 'monospace' ? 'system monospace' : monoPick()}` : ''), openFontDialog);
    add('Edit', 'Find — step through the matches without filtering', () => { if (!findBarOpen()) toggleSRBar(); document.getElementById('sr-find').focus(); }, 'Ctrl+F', loaded);
    THEMES.forEach(([id, label]) => add('Theme', 'Theme: ' + label, () => setTheme(id), currentTheme() === id ? '✓' : ''));
    const s = t || parseDefaults;
    PARSE_DELIMS.forEach(([v, l]) => add('Reading', 'Delimiter: ' + l, () => setDelimiter(v), s.delimiter === v ? '✓' : ''));
    add('Reading', 'Encoding: Auto', () => setEncoding(''), s.encoding === '' ? '✓' : '');
    ENCODINGS.forEach(([v, l]) => add('Reading', 'Encoding: ' + l, () => setEncoding(v), s.encoding === v ? '✓' : ''));
    PARSE_HEADERS.forEach(([v, l]) => add('Reading', 'Line 1: ' + l, () => applyHeaderMode(v), s.headerMode === v ? '✓' : ''));
    if (loaded) add('Reading', `Line endings: switch to ${t.detectedEol === '\r\n' ? 'LF' : 'CRLF'}`, () => toggleEol(noEv));
    tabs.forEach(x => add('Tab', 'Go to tab: ' + x.name, () => activateTab(x.id), x === t ? '✓' : ''));
    recents.forEach((r, i) => add('Recent', 'Open recent: ' + r.name + (r.path ? '  ' + pathDir(r.path) : ''), () => openRecent(i)));
    if (loaded) t.headers.forEach((h, c) => {
        add('Column', `Go to column: ${h}`, () => goToColumn(c));
        add('Column', `Sort ascending: ${h}`, () => sortBy(c, 1));
        add('Column', `Sort descending: ${h}`, () => sortBy(c, -1));
        if (t.sort && !t.sort.some(k => k.col === c)) {
            add('Column', `Then sort ascending: ${h}`, () => sortBy(c, 1, true));
            add('Column', `Then sort descending: ${h}`, () => sortBy(c, -1, true));
        }
        add('Column', `Profile & filter by value: ${h}`, () => openColPanelFor(c));
        add('Column', `Group by: ${h}`, () => openGroupBy(c));
    });
    return C;
}

function fuzzy(q, text) {
    const s = removeAccents(text.toLowerCase());
    let from = 0, prev = -2, score = 0; const hits = [];
    for (const ch of q) {
        const k = s.indexOf(ch, from);
        if (k < 0) return null;
        score += (k === prev + 1 ? 6 : 1) + (k === 0 || /[\s:—\-_(]/.test(s[k - 1]) ? 5 : 0);
        hits.push(k); prev = k; from = k + 1;
    }
    return { score: score - hits[0] * 0.05 - s.length * 0.01, hits };
}

function openPalette() {
    closeDDs(); closeColPanel();
    pal = { items: paletteCommands(), shown: [], act: 0 };
    const box = document.getElementById('cmdk'), inp = document.getElementById('cmdk-in');
    inp.value = '';
    box.classList.add('open');
    renderPalette();
    inp.focus();
}
function closePalette() { document.getElementById('cmdk').classList.remove('open'); }

function renderPalette() {
    const q = removeAccents(document.getElementById('cmdk-in').value.trim().toLowerCase());
    let list;
    if (!q) list = pal.items.filter(c => c.group !== 'Column').map(c => ({ c, hits: [] }));
    else list = pal.items.map(c => { const f = fuzzy(q, c.label); return f && { c, hits: f.hits, score: f.score }; }).filter(Boolean).sort((a, b) => b.score - a.score);
    pal.shown = list.slice(0, 120);
    pal.act = Math.min(pal.act, Math.max(0, pal.shown.length - 1));
    const mark = (label, hits) => { const H = new Set(hits); return [...label].map((ch, i) => H.has(i) ? `<b>${esc(ch)}</b>` : esc(ch)).join(''); };
    document.getElementById('cmdk-list').innerHTML = pal.shown.length
        ? pal.shown.map(({ c, hits }, i) => `<div class="cmdk-item${i === pal.act ? ' act' : ''}" data-i="${i}" onmousemove="palHover(${i})" onclick="runPalette(${i})">`
            + `<span class="cl">${mark(c.label, hits)}</span>${c.hint === '✓' ? '<span class="cur">✓ current</span>' : c.hint ? `<kbd>${esc(c.hint)}</kbd>` : ''}<span class="cg">${esc(c.group)}</span></div>`).join('')
        : '<div class="cmdk-empty">No command matches.</div>';
    const a = document.querySelector('.cmdk-item.act'); if (a) a.scrollIntoView({ block: 'nearest' });
}
function palHover(i) { if (i === pal.act) return; pal.act = i; document.querySelectorAll('.cmdk-item').forEach((el, k) => el.classList.toggle('act', k === i)); }
function runPalette(i) {
    const it = pal.shown[i]; if (!it) return;
    closePalette();
    it.c.run();                               // synchronous: a picker opened here keeps the key press's user activation
}
document.getElementById('cmdk-in').addEventListener('input', () => { pal.act = 0; renderPalette(); });
document.getElementById('cmdk-in').addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const n = pal.shown.length; if (!n) return;
        pal.act = (pal.act + (e.key === 'ArrowDown' ? 1 : -1) + n) % n; renderPalette();
    } else if (e.key === 'Enter') { e.preventDefault(); runPalette(pal.act); }
    else if (e.key === 'Escape') { e.preventDefault(); closePalette(); }
    else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'p') { e.preventDefault(); e.stopPropagation(); closePalette(); }   // the same keys toggle it
});
window.addEventListener('keydown', e => {
    if (document.getElementById('dlg')) return;
    const k = e.key.toLowerCase();
    if ((e.ctrlKey || e.metaKey) && k === 'p' && !e.altKey) { e.preventDefault(); if (!document.getElementById('cmdk').classList.contains('open')) openPalette(); }   // Ctrl+P (the browser's print has no use here), Ctrl+Shift+P as in VS Code
});
