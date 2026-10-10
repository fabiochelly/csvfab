/* ---------------------------------------------------------------
   RECENT FILES & WELCOME SCREEN
   Recents live in IndexedDB: bridge paths, and file handles (IndexedDB
   can store those; reopening one only asks for access again). Newest
   first, 12 at most, one entry per file.
----------------------------------------------------------------*/
let recents = [];
/* Drawn at once, recents added when IndexedDB answers: at a cold start its
   storage service comes up late, and the welcome screen waited for it. */
renderWelcome();
idbGet('recents').then(l => { recents = l || []; if (recents.length) renderWelcome(); });
async function recordRecent(entries) {
    let list = recents.slice();
    for (const e of entries) {
        if (!e.path && !e.handle) continue;               // a read-only copy cannot be reopened
        const item = { kind: e.path ? 'path' : 'handle', path: e.path || null, handle: e.handle || null, name: e.name, size: e.size || 0, at: Date.now() };
        const keep = [];
        for (const x of list) {
            let same = false;
            if (item.kind === 'path') same = x.path === item.path;
            else if (x.kind === 'handle' && x.handle) { try { same = await x.handle.isSameEntry(item.handle); } catch (err) { } }
            if (!same) keep.push(x);
        }
        list = [item, ...keep];
    }
    recents = list.slice(0, 12);
    await idbSet('recents', recents);
    renderWelcome();
}
async function openRecent(i) {
    const r = recents[i]; if (!r) return;
    if (r.kind === 'path') return addPathTabs([r.path]);
    try { if (await r.handle.queryPermission({ mode: 'readwrite' }) !== 'granted') await r.handle.requestPermission({ mode: 'readwrite' }); } catch (e) { }
    try { await r.handle.getFile(); } catch (e) { toast(`${r.name} is no longer where it was.`, { kind: 'warn' }); return forgetRecent(i); }
    await addHandles([r.handle], null);
}
async function forgetRecent(i) { recents.splice(i, 1); await idbSet('recents', recents); renderWelcome(); }

function relTime(ms) {
    const d = (Date.now() - ms) / 1000;
    if (d < 60) return 'just now';
    if (d < 3600) return `${Math.floor(d / 60)} min ago`;
    if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
    if (d < 7 * 86400) return `${Math.floor(d / 86400)} d ago`;
    return new Date(ms).toLocaleDateString('fr-FR');
}
function fmtBytes(b) { return b >= 1048576 ? (b / 1048576).toFixed(1).replace('.', ',') + ' MB' : b >= 1024 ? Math.round(b / 1024) + ' KB' : b + ' B'; }

/* Every shortcut, by theme: the welcome screen shows a few, the panel (F1, ?, palette, ☰ menu,
   welcome screen) all of them. A key list is 'Ctrl+Z' or several alternatives in an array. */
const KEY_GROUPS = [
    ['Files and tabs', [['Open files', 'Ctrl+O'], ['Save', 'Ctrl+S'], ['Command palette', ['Ctrl+P', 'Ctrl+Shift+P']], ['Previous / next tab', ['Alt+←', 'Alt+→']],
        ['Go to tab 1 to 9', 'Alt+1…9'], ['Close the tab', 'Ctrl+W'], ['Quit', 'Ctrl+Q']]],
    ['Editing', [['Edit the cell', ['Enter', 'F2']], ['Replace its value', 'start typing'], ['Line break in a cell', 'Shift+Enter'], ['Same value in every selected cell', 'Enter'],
        ['A series in the selected cells', 'Ctrl+Enter'], ['Fill a series down', 'Ctrl+D'], ['Fill by example (the values offered: Tab takes, Esc drops)', 'Ctrl+E'], ['Clear the cells', 'Delete'], ['Undo', 'Ctrl+Z'], ['Redo', ['Ctrl+Y', 'Ctrl+Shift+Z']],
        ['Copy / paste a range', ['Ctrl+C', 'Ctrl+V']]]],
    ['Moving and selecting', [['Move', '↑ ↓ ← →'], ['Extend the selection', 'Shift+arrows'], ['To the edge of the data', 'Ctrl+arrows'], ['A screen up / down', ['PgUp', 'PgDn']],
        ['Start / end of the row', ['Home', 'End']], ['Start / end of the file', ['Ctrl+Home', 'Ctrl+End']], ['Select every row shown', 'Ctrl+A'], ['Go to row', 'Ctrl+G'], ['Row card', 'Ctrl+I'], ['File map', 'Ctrl+M']]],
    ['Searching', [['Find without filtering', 'Ctrl+F'], ['Next / previous match', ['F3', 'Shift+F3']], ['Close what is open', 'Esc'], ['These shortcuts', ['F1', '?']]]],
    ['Columns, with the mouse', [['Sort', 'click the title'], ['Add a sort key', 'Shift+click'], ['Rename', 'double-click the title'], ['Move', 'drag the title'],
        ['Fit to the content', 'double-click its edge'], ['Filter a range of values', 'drag across a column chart'], ['Split, merge, insert, delete, freeze up to a column', 'right-click the title'], ['Open a link, write to an address', 'Ctrl+click it'], ['A cell cut short, whole', 'rest the pointer on it'], ['The row\'s menu', 'right-click its number'], ['Copy, paste, keep a value…', 'right-click a cell'], ['Extend a series', 'drag the selection\'s corner'], ['Copy instead of a series', 'Ctrl+drag']]]
];
/* Keys as <kbd> chips: Ctrl+Shift+P → Ctrl + Shift + P; words (click the title) stay text. */
function kbdHtml(keys) {
    const one = x => /^[a-z]/.test(x) && !/^[a-z]$/.test(x) ? `<span class="kw">${esc(x)}</span>` : x.split('+').map(p => `<kbd>${esc(p)}</kbd>`).join('<i>+</i>');
    return (Array.isArray(keys) ? keys : [keys]).map(one).join('<span class="kor">or</span>');
}
function openKeys() {
    closeDDs(); closeAllModals();
    document.getElementById('keys-list').innerHTML = KEY_GROUPS.map(([title, rows]) => `<section class="kg"><h5>${esc(title)}</h5>`
        + rows.map(([what, keys]) => `<div class="kr"><span>${esc(what)}</span><span class="kk">${kbdHtml(keys)}</span></div>`).join('') + '</section>').join('');
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-keys').style.display = 'block';
}
/* The version lives in bridge/config.py alone: the page asks the server (/api/ping, no token),
   so a release never has to touch the page. The browser line is what a bug report needs. */
function openAbout() {
    closeDDs(); closeAllModals();
    const ver = document.getElementById('ab-ver');
    ver.textContent = '';
    fetch('/api/ping').then(r => r.json()).then(j => { ver.textContent = j.version ? 'Version ' + j.version : ''; }).catch(() => { });
    const ua = navigator.userAgentData, b = ua && ua.brands.find(x => !/Not.?A.?Brand|Chromium/i.test(x.brand)) || ua && ua.brands.find(x => /Chromium/.test(x.brand));
    document.getElementById('ab-env').textContent = [b ? `${b.brand} ${b.version}` : '', ua ? ua.platform : ''].filter(Boolean).join(' · ');
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-about').style.display = 'block';
}
/* F1, or ? outside a text field: the shortcuts. */
window.addEventListener('keydown', e => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key !== 'F1' && e.key !== '?') return;
    const tag = document.activeElement && document.activeElement.tagName;
    if (e.key === '?' && (tag === 'INPUT' || tag === 'TEXTAREA' || document.activeElement.isContentEditable)) return;
    if (document.getElementById('dlg')) return;
    e.preventDefault(); openKeys();
});

/* Light on purpose: the name, a drop zone, the palette and the shortcuts, the recent files; what
   the app does is said once, quietly, at the bottom. Opening is the drop zone (and Ctrl+O). */
function renderWelcome() {
    const ico = d => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
    emptyState.innerHTML = `<div class="wl">
        <div class="wl-hero"><img class="wl-logo" src="icons/csvfab.svg" alt=""><h1>csvfab</h1></div>
        <div class="wl-drop wl-glass" onclick="openFiles()" title="Or Ctrl+O">
            <span class="wl-drop-ico">${ico('<path d="M12 15V4"/><polyline points="7 9 12 4 17 9"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>')}</span>
            <b>Drop files to open them</b><span>CSV, TSV, Excel or JSON — or click to choose</span>
        </div>
        <div class="wl-actions">
            <button class="wl-key" onclick="openPalette()">Command palette <span class="kk">${kbdHtml('Ctrl+P')}</span></button>
            <button class="wl-key" onclick="openKeys()">Shortcuts <span class="kk">${kbdHtml('F1')}</span></button>
        </div>
        ${recents.length ? `<div class="wl-sec">Recent</div><div class="wl-recent wl-glass">${recents.map((r, i) => `
            <div class="wl-file" onclick="openRecent(${i})" title="${esc(r.path || 'Opened with the file picker: reopening asks for access again')}">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><path d="M8 13h8M8 17h8"/></svg>
                <span class="n">${esc(r.name)}</span>
                <span class="m">${fmtBytes(r.size)} · ${relTime(r.at)}</span>
                <span class="x" onclick="event.stopPropagation(); forgetRecent(${i})" title="Remove from the list">×</span>
                <span class="d">${esc(r.path ? pathDir(r.path) : 'file picker')}</span>
            </div>`).join('')}</div>` : ''}
        <p class="wl-foot"><span>open</span><span>filter</span><span>clean</span><span>rewrite in place</span></p>
    </div>`;
}

/* The ⋮ button of a row number: a small menu of row actions. */
const ROW_ICONS = {
    above: '<path d="M12 19V9"/><polyline points="7 13 12 8 17 13"/><line x1="4" y1="4" x2="20" y2="4"/>',
    below: '<path d="M12 5v10"/><polyline points="7 11 12 16 17 11"/><line x1="4" y1="20" x2="20" y2="20"/>',
    dup: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
    del: '<polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>',
    card: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="M6 16c0-1.7 1.3-3 3-3s3 1.3 3 3"/><path d="M14 9h4M14 13h4"/>',
    raw: '<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>'
};
function openRowMenu(e, id) {
    e.stopPropagation(); e.preventDefault();
    const m = document.getElementById('row-menu'), btn = e.currentTarget;
    const again = m.classList.contains('open') && m.dataset.id === String(id);
    closeDDs();
    if (again) return;
    const item = (fn, icon, label, cls) => `<div class="dd-item${cls ? ' ' + cls : ''}" onclick="closeDDs(); ${fn}"><span class="lbl"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ROW_ICONS[icon]}</svg>${label}</span></div>`;
    /* A multi-row selection holding this row: the delete applies to all of it. */
    const t = T(), rg = selRange(t), vi = t.filteredData.findIndex(r => r.id === id);
    const many = rg && rg.r1 > rg.r0 && vi >= rg.r0 && vi <= rg.r1 ? rg.r1 - rg.r0 + 1 : 0;
    m.dataset.id = id;
    m.innerHTML = `<div class="dd-head">Row ${fmt(id)}</div>`
        + item(`insertRow(${id}, 0)`, 'above', 'Insert a row above')
        + item(`insertRow(${id}, 1)`, 'below', 'Insert a row below')
        + item(`duplicateRow(${id})`, 'dup', 'Duplicate below')
        + '<div class="dd-sep"></div>'
        + item(`rowCardFor(${id}, false)`, 'card', 'Row card — every field as a form')
        + item(`rowCardFor(${id}, true)`, 'raw', 'Raw line — as it sits in the file')
        + item(`rowCardFor(${id}); rowCardDiag(true)`, 'raw', 'Why does this row look wrong?')
        + '<div class="dd-sep"></div>'
        + (many ? item('deleteSelectedRows()', 'del', `Delete the ${fmt(many)} selected rows`, 'danger') : item(`deleteRow(${id})`, 'del', 'Delete the row', 'danger'));
    m.classList.add('open'); btn.classList.add('open');
    const r = btn.getBoundingClientRect(), w = m.offsetWidth, h = m.offsetHeight;
    m.style.left = Math.min(r.right + 4, window.innerWidth - w - 8) + 'px';
    m.style.bottom = 'auto';
    m.style.top = Math.max(8, Math.min(r.top, window.innerHeight - h - 8)) + 'px';
}
/* An empty row next to row `id` (below = 1). Under a filter it may not show: say so. */
function insertRow(id, below) {
    const t = T(); if (!t) return;
    const index = t.allData.findIndex(r => r.id === id);
    if (index === -1) return;
    const row = newRow(t, t.headers.map(() => ''));
    t.allData.splice(index + below, 0, row);
    t.allData.forEach((r, i) => r.id = i + 1);
    t.rowCount = t.allData.length;
    t.modificationsLog.push({ id: index + below + 1, col: '---', old: '---', new: 'Row inserted', what: `row inserted ${below ? 'below' : 'above'} row ${id}`,
        undo: t => { t.allData = t.allData.filter(r => r !== row); } });   // a new array, never a splice (redo, 15-…)
    const keepTop = container.vTop;
    updateSaveBtn(); applyFilters(); renderTabBar();
    container.vTop = keepTop; render();
    setStats(`${t.name} | Empty row inserted as row ${index + below + 1}${hasFilter(t) ? ' — the filters may hide it' : ''} — not written yet, use Save.`);
}

/* The rows of the selection, in one pass (a selection can span a million
   rows). commitRows keeps the previous array, which is the whole undo. */
async function deleteSelectedRows() {
    const t = T(), rg = selRange(t); if (!t || !rg) return;
    const doomed = new Set(t.filteredData.slice(rg.r0, rg.r1 + 1));
    if (!await uiConfirm(`Delete the ${fmt(doomed.size)} selected rows?`, { ok: `Delete ${fmt(doomed.size)} rows`, danger: true })) return;
    const kept = t.allData.filter(r => !doomed.has(r)), n = t.allData.length - kept.length;
    sel = null;
    const keepTop = container.vTop;
    commitRows(t, kept, { id: '-', col: '---', old: `${n} rows`, new: 'Deleted', what: `${fmt(n)} rows deleted` });
    container.vTop = keepTop; render();
    setStats(`${t.name} | ${fmt(n)} rows deleted — not written yet, use Save.`);
}

function deleteRow(id) {
    const t = T(); if (!t) return;
    const index = t.allData.findIndex(r => r.id === id);
    if (index === -1) return;
    const [gone] = t.allData.splice(index, 1);
    t.allData.forEach((r, idx) => r.id = idx + 1);
    t.rowCount = t.allData.length;
    t.modificationsLog.forEach(log => { if (typeof log.id === 'number' && log.id > id) log.id--; });
    t.modificationsLog.push({ id: '-', col: '---', old: 'Row deleted', new: '---', what: `row ${id} deleted`, undo: t => {
        t.allData = t.allData.toSpliced(index, 0, gone);   // a new array, never a splice (redo, 15-…)
    } });
    updateSaveBtn(); applyFilters(); renderTabBar();
}
