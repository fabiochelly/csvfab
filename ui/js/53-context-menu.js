/* ---------------------------------------------------------------
   RIGHT CLICK
   The browser's own menu never shows (it gave the app away as a web
   page). Instead:
   - in a text field (the filters, the cell editor, a dialog's), its own
     menu: undo, cut, copy, paste, select all, and clear for a filter;
   - on a column's title, its structure: split, merge, insert a column
     before or after, delete (the ▾'s whole panel at first: too much);
   - on a row's number, the row's menu — the ⋮'s;
   - on a cell, the cell's menu: copy and paste (the selection, or the
     cell clicked when it is outside it), copy the row, fill by example,
     keep or hide the rows holding this value, the row's card, why it
     looks wrong, insert or delete it. Edit, clear, find and the two
     sorts were in it at first: too much, the user (they keep their
     keys, Enter, Delete, Ctrl+F, and the title's click).
   Paste reads the system clipboard (navigator.clipboard): Chromium
   asks once for that permission, for this origin.
----------------------------------------------------------------*/
const CELL_ICONS = {
    copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
    paste: '<path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><rect x="8" y="2" width="8" height="4" rx="1"/>',
    magic: '<path d="M12 3l2 6 6 2-6 2-2 6-2-6-6-2 6-2z"/>',
    keep: '<path d="M22 3H2l8 9.5V19l4 2v-8.5z"/>',
    split: '<path d="M12 3v18"/><path d="M8 8l-4 4 4 4M4 12h5"/><path d="M16 8l4 4-4 4M20 12h-5"/>',
    merge: '<path d="M12 3v18"/><path d="M5 8l4 4-4 4M3 12h6"/><path d="M19 8l-4 4 4 4M21 12h-6"/>',
    before: '<path d="M9 4v16"/><path d="M15 9v6M12 12h6"/>',
    after: '<path d="M15 4v16"/><path d="M9 9v6M6 12h6"/>',
    undo: '<path d="M9 14L4 9l5-5"/><path d="M4 9h11a5 5 0 0 1 0 10h-3"/>',
    cut: '<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><line x1="20" y1="4" x2="8.1" y2="15.9"/><line x1="14.5" y1="14.5" x2="20" y2="20"/><line x1="8.1" y1="8.1" x2="12" y2="12"/>',
    all: '<rect x="4" y="4" width="16" height="16" rx="2" stroke-dasharray="3 3"/>',
    clearf: '<path d="M22 3H2l8 9.5V19l4 2v-8.5z"/><line x1="15" y1="15" x2="21" y2="21"/><line x1="21" y1="15" x2="15" y2="21"/>',
    hide: '<path d="M17.9 17.9A10 10 0 0 1 12 20c-7 0-10-8-10-8a18 18 0 0 1 5.1-5.9M9.9 4.2A9 9 0 0 1 12 4c7 0 10 8 10 8a18 18 0 0 1-2.2 3.2"/><line x1="2" y1="2" x2="22" y2="22"/>',
};
/* The browser's menu: never (text fields get fieldMenu()). */
document.addEventListener('contextmenu', e => {
    const tg = e.target;
    e.preventDefault();
    const box = tg.closest && tg.closest('.f-box');   // a column filter not typed in yet: its input first
    if (box) { editColFilter(box); const input = thead.querySelector(`input.f-in[data-col="${box.dataset.col}"]`); if (input) fieldMenu(e, input); return; }
    const field = tg.closest && tg.closest('input:not([type=checkbox]):not([type=radio]):not([type=range]), textarea');
    if (field) { fieldMenu(e, field); return; }
    if (tg.closest && tg.closest('select, [contenteditable]')) return;
    const t = T(); if (!t || !t.loaded) return;
    const th = tg.closest && tg.closest('thead tr:first-child th[data-col]');
    if (th) { colMenu(e, t, +th.dataset.col); return; }   // a title: the column's structure
    const row = tg.closest && tg.closest('#tbody .row[data-idx]');
    if (!row) return;
    const i = +row.dataset.idx, r = t.filteredData[i];
    if (!r) return;
    if (tg.closest('.col-idx')) {             // a row's number: the ⋮'s menu, from its ⋮
        const btn = row.querySelector('.row-btn');
        if (btn) openRowMenu({ stopPropagation() { }, preventDefault() { }, currentTarget: btn }, r.id);
        return;
    }
    const cell = tg.closest('.cell[data-c]');
    if (cell) cellMenu(e, t, i, +cell.dataset.c);
});

function cellMenu(e, t, i, c) {
    closeDDs(); if (colPanel) closeColPanel();
    /* The cell clicked becomes the selection, unless it is inside it: a copy then takes it all. */
    const rg = selRange(t);
    if (!rg || i < rg.r0 || i > rg.r1 || c < rg.c0 || c > rg.c1) setSel(t, i, c, i, c);
    const r = t.filteredData[i], v = cellStr(cellOf(r, c)), name = t.headers[c], many = (() => { const g = selRange(t); return g && (g.r1 > g.r0 || g.c1 > g.c0); })();
    const short = v.length > 28 ? v.slice(0, 27) + '…' : v;
    const item = (fn, icon, label, key, cls) => `<div class="dd-item${cls ? ' ' + cls : ''}" onclick="closeDDs(); ${fn}"><span class="lbl"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${CELL_ICONS[icon] || ROW_ICONS[icon]}</svg>${label}</span>${key ? `<span class="k">${key}</span>` : ''}</div>`;
    const m = document.getElementById('cell-menu');
    m.onmousedown = null;
    m.innerHTML = `<div class="dd-head">${esc(name)} · row ${fmt(r.id)}</div>`
        + item('cellCopy()', 'copy', many ? 'Copy the selection' : 'Copy', 'Ctrl+C')
        + item('cellPaste()', 'paste', 'Paste', 'Ctrl+V')
        + item('cellCopyRow()', 'copy', 'Copy the whole row')
        + item('fillByExample()', 'magic', 'Fill by example', 'Ctrl+E')
        + '<div class="dd-sep"></div>'
        + (v.trim() ? item(`cellKeepValue(${c}, true)`, 'keep', `Only the rows with “${esc(short)}”`)
            + item(`cellKeepValue(${c}, false)`, 'hide', `Hide the rows with “${esc(short)}”`)
            : item(`cellKeepValue(${c}, true)`, 'keep', 'Only the rows where it is empty')
            + item(`cellKeepValue(${c}, false)`, 'hide', 'Hide the rows where it is empty'))
        + '<div class="dd-sep"></div>'
        + item(`rowCardFor(${r.id}, false)`, 'card', 'Row card', 'Ctrl+I')
        + item(`rowCardFor(${r.id}); rowCardDiag(true)`, 'raw', 'Why does this row look wrong?')
        + item(`insertRow(${r.id}, 1)`, 'below', 'Insert a row below')
        + item(`deleteRow(${r.id})`, 'del', 'Delete the row', '', 'danger');
    menuAt(m, e);
}

/* A title's menu: what changes the column's structure, one item each — the ▾'s panel and its
   filters were first shown here: too much, the user. */
function colMenu(e, t, c) {
    closeDDs(); if (colPanel) closeColPanel();
    const item = (fn, icon, label, cls) => `<div class="dd-item${cls ? ' ' + cls : ''}" onclick="closeDDs(); ${fn}"><span class="lbl"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${CELL_ICONS[icon] || ROW_ICONS[icon]}</svg>${label}</span></div>`;
    const m = document.getElementById('cell-menu');
    m.onmousedown = null;
    m.innerHTML = `<div class="dd-head">${esc(t.headers[c])}</div>`
        + item(`openSplit(${c})`, 'split', 'Split the column…')
        + item(`openMerge(${c})`, 'merge', 'Merge columns…')
        + '<div class="dd-sep"></div>'
        + item(`addColumn(${c - 1})`, 'before', 'Insert a column before')
        + item(`addColumn(${c})`, 'after', 'Insert a column after')
        + '<div class="dd-sep"></div>'
        + item(`deleteColumn(${c})`, 'del', 'Delete the column', 'danger');
    menuAt(m, e);
}
/* A text field's menu, in place of the browser's: undo, cut, copy, paste, select all — and clear,
   for a filter. Through execCommand, so the field's own undo and its input event follow; paste
   reads the clipboard (navigator.clipboard, asked once). The menu never takes the focus (its
   mousedown is prevented): a column filter's input goes back to a box when it loses it. */
let fieldEl = null;
function fieldMenu(e, el) {
    closeDDs(); if (colPanel) closeColPanel();
    fieldEl = el;
    if (document.activeElement !== el) el.focus();
    const hasSel = el.selectionStart !== el.selectionEnd, ro = el.readOnly || el.disabled, filter = el.id === 'global-search' || el.classList.contains('f-in') || el.id === 'sr-find';
    const item = (fn, icon, label, key, off) => `<div class="dd-item${off ? ' disabled' : ''}" onclick="closeDDs(); ${fn}"><span class="lbl"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${CELL_ICONS[icon]}</svg>${label}</span>${key ? `<span class="k">${key}</span>` : ''}</div>`;
    const m = document.getElementById('cell-menu');
    m.innerHTML = item(`fieldDo('undo')`, 'undo', 'Undo', 'Ctrl+Z', ro)
        + '<div class="dd-sep"></div>'
        + item(`fieldDo('cut')`, 'cut', 'Cut', 'Ctrl+X', ro || !hasSel)
        + item(`fieldDo('copy')`, 'copy', 'Copy', 'Ctrl+C', !hasSel)
        + item('fieldPaste()', 'paste', 'Paste', 'Ctrl+V', ro)
        + '<div class="dd-sep"></div>'
        + item(`fieldDo('selectAll')`, 'all', 'Select all', 'Ctrl+A', !el.value)
        + (filter ? item('fieldClear()', 'clearf', 'Clear the filter', '', !el.value || ro) : '');
    m.onmousedown = ev => ev.preventDefault();
    menuAt(m, e);
}
function fieldDo(cmd) {
    const el = fieldEl; if (!el || !el.isConnected) return;
    el.focus();
    if (cmd === 'selectAll') el.select(); else document.execCommand(cmd);
}
async function fieldPaste() {
    const el = fieldEl; if (!el || !el.isConnected) return;
    let text = '';
    try { text = await navigator.clipboard.readText(); } catch (e) { setStats('The clipboard could not be read here: use Ctrl+V.'); return; }
    if (!el.isConnected) return;
    el.focus();
    if (text) document.execCommand('insertText', false, el.tagName === 'INPUT' ? text.replace(/\r?\n/g, ' ') : text);
}
function fieldClear() {
    const el = fieldEl; if (!el || !el.isConnected) return;
    el.focus(); el.select(); document.execCommand('delete');
}
function menuAt(m, e) {
    m.style.left = '0px'; m.style.top = '0px';
    m.classList.add('open');
    const w = m.offsetWidth, h = m.offsetHeight;
    m.style.left = Math.max(8, Math.min(e.clientX, window.innerWidth - w - 8)) + 'px';
    m.style.top = Math.max(8, Math.min(e.clientY, window.innerHeight - h - 8)) + 'px';
}

async function cellCopy() {
    const t = T(), s = t && selectionTSV(t); if (!s) return;
    try { await navigator.clipboard.writeText(s.text); doneMsg(`${t.name} | ${fmt(s.rows)} × ${fmt(s.cols)} cells copied.`); }
    catch (e) { setStats(`${t.name} | The clipboard refused the copy: use Ctrl+C.`); }
}
async function cellCopyRow() {
    const t = T(); if (!t || !sel || sel.tab !== t.id) return;
    const r = t.filteredData[sel.fr]; if (!r) return;
    const text = visibleCols(t).map(c => tsvQuote(r.data[c])).join('\t');
    try { await navigator.clipboard.writeText(text); doneMsg(`${t.name} | Row ${fmt(r.id)} copied.`); }
    catch (e) { setStats(`${t.name} | The clipboard refused the copy.`); }
}
async function cellPaste() {
    const t = T(); if (!t || !sel || sel.tab !== t.id) return;
    let text = '';
    try { text = await navigator.clipboard.readText(); }
    catch (e) { setStats(`${t.name} | The clipboard could not be read here: use Ctrl+V.`); return; }
    if (text && T() === t) writeCells(t, parseTSV(text), false, 'pasted');
}
/* Keep only the rows holding the cell's value in its column, or hide them: a value filter, as the
   column panel sets it (every other value excluded, or this one). */
function cellKeepValue(c, only) {
    const t = T(); if (!t || !sel || sel.tab !== t.id) return;
    const v = cellStr(cellOf(t.filteredData[sel.fr], c));
    let ex;
    if (only) {
        ex = new Set();
        visitRows(t, t.allData, r => { const x = cellStr(cellOf(r, c)); if (x !== v) ex.add(x); });
        if (!ex.size) { setStats(`${t.headers[c]}: every row already holds that value.`); return; }
    } else ex = new Set(t.valFilters[c] || []).add(v);
    t.valFilters[c] = ex;
    renderHeader(); applyColStyles(); applyFilters();
    setStats(`${t.name} | ${t.headers[c]}: ${only ? 'only' : 'without'} “${v === '' ? '(empty)' : v}” — the column's ▾ or Clear all filters brings them back.`);
}
