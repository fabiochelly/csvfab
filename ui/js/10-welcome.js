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

function renderWelcome() {
    const k = (a, b) => `<div><span>${a}</span><span>${b}</span></div>`, kb = x => '<span class="kbs">' + x.split('+').map(p => `<kbd>${p}</kbd>`).join('+') + '</span>';
    emptyState.innerHTML = `<div class="wl">
        <img class="wl-logo" src="icons/csvfab.svg" alt="">
        <h1>csvfab</h1>
        <p class="wl-sub">Open, filter, clean and rewrite CSV files — in place.</p>
        <div class="wl-drop" onclick="openFiles()">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>
            <b>Drop CSV files here</b><span>or click to choose them — several at once, each in its own tab</span>
        </div>
        <div class="wl-actions">
            <button class="btn" onclick="openFiles()">Open files ${kb('Ctrl+O')}</button>
            <button class="btn btn-outline" onclick="openPalette()">Command palette ${kb('Ctrl+K')}</button>
        </div>
        ${recents.length ? `<div class="wl-sec">Recent</div><div class="wl-recent">${recents.map((r, i) => `
            <div class="wl-file" onclick="openRecent(${i})" title="${esc(r.path || 'Opened with the file picker: reopening asks for access again')}">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><path d="M8 13h8M8 17h8"/></svg>
                <span class="n">${esc(r.name)}</span>
                <span class="m">${fmtBytes(r.size)} · ${relTime(r.at)}</span>
                <span class="x" onclick="event.stopPropagation(); forgetRecent(${i})" title="Remove from the list">×</span>
                <span class="d">${esc(r.path ? pathDir(r.path) : 'file picker')}</span>
            </div>`).join('')}</div>` : ''}
        <div class="wl-keys">
            ${k('Command palette', kb('Ctrl+K'))}${k('Save', kb('Ctrl+S'))}
            ${k('Undo', kb('Ctrl+Z'))}${k('Copy / paste a range', kb('Ctrl+C') + ' ' + kb('Ctrl+V'))}
            ${k('Fill a series down', kb('Ctrl+D'))}${k('Switch tab', kb('Alt+←') + ' ' + kb('Alt+→'))}
            ${k('Row card', kb('Ctrl+I'))}${k('Go to row', kb('Ctrl+G'))}
        </div>
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
        undo: t => { t.allData.splice(t.allData.indexOf(row), 1); } });
    const keepTop = container.scrollTop;
    updateSaveBtn(); applyFilters(); renderTabBar();
    container.scrollTop = keepTop; render();
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
    const keepTop = container.scrollTop;
    commitRows(t, kept, { id: '-', col: '---', old: `${n} rows`, new: 'Deleted', what: `${fmt(n)} rows deleted` });
    container.scrollTop = keepTop; render();
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
        t.allData.splice(index, 0, gone);
    } });
    updateSaveBtn(); applyFilters(); renderTabBar();
}
