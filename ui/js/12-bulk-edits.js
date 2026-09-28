/* ---------------------------------------------------------------
   EDIT FILTERED ROWS
   One column, every row the filters show: set, clear, trim, change
   case. Only the cells that actually change are touched and logged,
   so undo puts back exactly those.
----------------------------------------------------------------*/
function bulkFn() {
    const op = document.querySelector('input[name="bulk-op"]:checked').value, val = document.getElementById('bulk-val').value;
    return {
        set: () => val, clear: () => '',
        trim: v => v.replace(/[\s ]+/g, ' ').trim(),
        upper: v => v.toLocaleUpperCase('fr'), lower: v => v.toLocaleLowerCase('fr'),
        title: v => v.toLocaleLowerCase('fr').replace(/(^|[\s\-'’(])(\p{L})/gu, (m, a, b) => a + b.toLocaleUpperCase('fr'))
    }[op];
}
function bulkChanges() {
    const t = T(), col = +document.getElementById('bulk-col').value, f = bulkFn(), out = [];
    for (const r of t.filteredData) {
        const old = r.data[col], nv = f(cellStr(old));
        if (nv !== cellStr(old)) out.push([r, old, nv]);
    }
    return out;
}
function openBulk() {
    const t = T(); if (!t || !t.loaded) return;
    const sel = document.getElementById('bulk-col'), prev = sel.value;
    sel.innerHTML = t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    if (prev !== '' && +prev < t.headers.length) sel.value = prev;
    updateBulkNote();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-bulk').style.display = 'block';
}
function updateBulkNote() {
    const t = T(); if (!t) return;
    const n = bulkChanges().length;
    document.getElementById('bulk-note').innerHTML = (n ? `<b>${fmt(n)}</b> cells will change` : 'No cell would change')
        + ` in the ${fmt(t.filteredData.length)} ${hasFilter(t) ? 'filtered' : ''} rows.`;
    document.getElementById('bulk-go').disabled = !n;
}
function applyBulk() {
    const t = T(); if (!t) return;
    const col = +document.getElementById('bulk-col').value, ch = bulkChanges();
    closeAllModals();
    if (!ch.length) return;
    for (const [r, , nv] of ch) r.data[col] = nv;
    t.modificationsLog.push({ id: '-', col: t.headers[col], old: 'bulk', new: `${ch.length} cells`, what: `${fmt(ch.length)} cells edited in ${t.headers[col]}`,
        undo: () => { for (const [r, old] of ch) r.data[col] = old; } });
    updateSaveBtn(); renderTabBar(); render();
    setStats(`${t.name} | ${fmt(ch.length)} cells edited in ${t.headers[col]} — not written yet, use Save.`);
}

/* ---------------------------------------------------------------
   BULK ROW EDITS
   Sorting, removing duplicates and deleting hidden rows all replace
   t.allData wholesale. Like every other edit they stay in memory until
   Save; row ids are renumbered to the new order, as deleteRow() does.
----------------------------------------------------------------*/
function commitRows(t, rows, log, undoExtra) {
    const prev = t.allData;               // the row objects are shared: keeping the old order costs one array
    log.undo = t => { t.allData = prev; if (undoExtra) undoExtra(t); };
    t.allData = rows;
    rows.forEach((r, i) => r.id = i + 1);
    t.rowCount = rows.length;
    t.modificationsLog.push(log);
    updateSaveBtn(); applyFilters(); renderTabBar(); updateStats();
}

/* Sort keys: a column is numeric or date when ≥ 90 % of its non-empty cells
   are (French decimal commas and day-first dates included); text compares
   with a French, digit-aware collator ("Lot 2" < "Lot 10"). Empty cells go
   last in both directions. */
function numKey(v) {
    let s = String(v).replace(/[\s  ]/g, '');
    if (s.includes(',') && s.includes('.')) s = s.lastIndexOf(',') > s.lastIndexOf('.') ? s.replace(/\./g, '').replace(',', '.') : s.replace(/,/g, '');
    else if (/^[-+]?\d+,\d+$/.test(s)) s = s.replace(',', '.');
    else s = s.replace(/,/g, '');
    return parseFloat(s);
}
function dateKey(v) {
    const m = String(v).trim().match(/^(\d{1,4})[-\/.](\d{1,2})[-\/.](\d{1,4})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?/);
    if (!m) return NaN;
    let [, a, b, c, h, mi, se] = m;
    const [y, mo, d] = a.length === 4 ? [a, b, c] : [c.length === 2 ? '20' + c : c, b, a];   // ISO, else day first
    return ((((+y * 100 + +mo) * 100 + +d) * 100 + +(h || 0)) * 100 + +(mi || 0)) * 100 + +(se || 0);
}
function sortBy(col, forceDir) {
    const t = T(); if (!t || !t.loaded) return;
    const dir = forceDir || (t.sort && t.sort.col === col ? -t.sort.dir : 1), prevSort = t.sort;
    const counts = { n: 0, d: 0, t: 0 }; let seen = 0;
    for (const r of t.allData) { const ty = cellType(r.data[col]); if (ty) { counts[ty]++; if (++seen >= 2000) break; } }
    const kind = seen && counts.n / seen >= 0.9 ? 'n' : (seen && counts.d / seen >= 0.9 ? 'd' : 't');
    const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' });
    const keyed = t.allData.map(r => {
        const v = r.data[col], s = v == null ? '' : String(v).trim();
        const k = !s ? null : kind === 'n' ? numKey(s) : kind === 'd' ? dateKey(s) : s;
        return { r, k: (typeof k === 'number' && isNaN(k)) ? null : k };
    });
    keyed.sort((a, b) => {
        if (a.k === null || b.k === null) return (a.k === null) - (b.k === null);
        return dir * (kind === 't' ? coll.compare(a.k, b.k) : a.k - b.k);
    });
    t.sort = { col, dir };
    container.scrollTop = 0;
    commitRows(t, keyed.map(x => x.r), { id: '-', col: t.headers[col], old: 'Sort', new: dir > 0 ? 'ascending' : 'descending', what: `sort by ${t.headers[col]}` },
        t => { t.sort = prevSort; });
    renderHeader(); applyColStyles();
    setStats(`${t.name} | Sorted by ${t.headers[col]}, ${dir > 0 ? 'ascending' : 'descending'} (${{ n: 'numbers', d: 'dates', t: 'text' }[kind]}) — not written yet, use Save.`);
}

/* Keeps only the rows the filters show. The filters are then cleared: they
   would now match every row, and leaving them on would hide that. */
async function deleteHiddenRows() {
    const t = T(); if (!t || !t.loaded) return;
    const hidden = t.allData.length - t.filteredData.length;
    if (!hasFilter(t) || !hidden) { uiAlert('No row is hidden: set a filter first — every row it hides will be deleted.'); return; }
    if (!await uiConfirm(`Delete the ${fmt(hidden)} hidden rows?\n\nThe ${fmt(t.filteredData.length)} rows shown are kept, and the filters are cleared.`, { ok: 'Delete rows', danger: true })) return;
    const kept = t.filteredData.slice(), prevQuery = t.globalQuery, prevFilters = t.colFilters, prevVals = t.valFilters, prevIrr = t.onlyIrregular;
    t.globalQuery = ''; t.colFilters = {}; t.valFilters = {}; t.onlyIrregular = false;
    document.getElementById('global-search').value = '';
    renderHeader(); applyColStyles();
    commitRows(t, kept, { id: '-', col: '---', old: `${hidden} hidden rows`, new: 'Deleted', what: `${fmt(hidden)} hidden rows deleted` },
        t => { t.globalQuery = prevQuery; t.colFilters = prevFilters; t.valFilters = prevVals; t.onlyIrregular = prevIrr; document.getElementById('global-search').value = prevQuery; });
    setStats(`${t.name} | ${fmt(hidden)} hidden rows deleted, ${fmt(kept.length)} kept — not written yet, use Save.`);
}

/* Duplicates are counted over ALL rows, filtered or not. A row whose
   compared cells are all empty is never a duplicate: 46 contacts without
   an e-mail are not one contact. */
function dedupeKeep() {
    const t = T();
    const cols = [...document.querySelectorAll('#dedupe-cols input:checked')].map(i => +i.value);
    const use = cols.length ? cols : t.headers.map((_, i) => i);
    const match = document.querySelector('input[name="dedupe-match"]:checked').value;
    const last = document.querySelector('input[name="dedupe-keep"]:checked').value === 'last';
    const str = v => String(v == null ? '' : v);
    const norm = match === 'slug' ? v => slugify(str(v)) : match === 'loose' ? v => str(v).trim().toLocaleLowerCase('fr') : str;
    const seen = new Set(), keep = new Array(t.allData.length);
    const n = t.allData.length;
    for (let k = 0; k < n; k++) {
        const i = last ? n - 1 - k : k;
        const parts = use.map(c => norm(t.allData[i].data[c]));
        if (parts.every(p => p.trim() === '')) { keep[i] = true; continue; }
        const key = parts.join('\u0001');
        keep[i] = !seen.has(key);
        seen.add(key);
    }
    return keep;
}
function openDedupe() {
    const t = T(); if (!t || !t.loaded) return;
    document.getElementById('dedupe-cols').innerHTML = t.headers.map((h, i) =>
        `<label class="col-label"><input type="checkbox" value="${i}" onchange="updateDedupeNote()"> ${esc(h)}</label>`).join('');
    updateDedupeNote();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-dedupe').style.display = 'block';
}
function updateDedupeNote() {
    const t = T(); if (!t) return;
    const dup = dedupeKeep().filter(k => !k).length;
    document.getElementById('dedupe-note').innerHTML = dup
        ? `<b>${fmt(dup)}</b> duplicate rows out of ${fmt(t.allData.length)} will be deleted (all rows are checked, filtered or not).`
        : `No duplicate among the ${fmt(t.allData.length)} rows.`;
    document.getElementById('dedupe-go').disabled = !dup;
}
function applyDedupe() {
    const t = T(); if (!t) return;
    const keep = dedupeKeep();
    const rows = t.allData.filter((_, i) => keep[i]), dup = t.allData.length - rows.length;
    closeAllModals();
    if (!dup) return;
    commitRows(t, rows, { id: '-', col: '---', old: `${dup} duplicate rows`, new: 'Deleted', what: `${fmt(dup)} duplicates removed` });
    setStats(`${t.name} | ${fmt(dup)} duplicate rows deleted — not written yet, use Save.`);
}

/* Virtual rendering: only the visible slice of the ACTIVE tab is in the DOM */
function render() {
    const t = T();
    if (!t || !t.loaded) { tbody.innerHTML = ''; return; }
    const data = t.filteredData;
    if (data.length === 0) { tbody.innerHTML = '<tr><td colspan="100" style="padding: 20px; text-align: center;">No results found</td></tr>'; return; }

    const start = Math.max(0, Math.floor((container.scrollTop - thead.offsetHeight) / ROW_H));   // rows start below the header
    const end = start + Math.ceil(container.clientHeight / ROW_H) + 3;
    const colSpan = t.headers.length + 1;
    const hl = t.hl, rg = selRange(t), fp = fillRect();

    let html = '';
    if (start > 0) html += `<tr style="height: ${start * ROW_H}px; background: transparent;"><td colspan="${colSpan}" style="padding:0; border:none;"></td></tr>`;

    for (let i = start; i < Math.min(end, data.length); i++) {
        const r = data[i];
        const displayId = r.id.toLocaleString('fr-FR');
        html += `<tr class="${(r.id % 2 === 0) ? 'row-even' : 'row-odd'}${r.data.length !== t.headers.length ? ' irr' : ''}" style="height:${ROW_H}px" data-idx="${i}">
            <td class="col-idx" draggable="true" title="Click: select the row · Drag: move it">
                <span class="row-num">${displayId}</span>
                <span class="row-btn" onclick="openRowMenu(event, ${r.id})" title="Insert, duplicate or delete this row"><svg viewBox="0 0 16 16" fill="currentColor"><circle cx="8" cy="3.5" r="1.4"/><circle cx="8" cy="8" r="1.4"/><circle cx="8" cy="12.5" r="1.4"/></svg></span>
            </td>
            ${r.data.map((c, cIdx) => `<td data-c="${cIdx}"${cellCls(i, cIdx, rg, fp, r)}${barStyle(t, cIdx, c)}>${showBreaks(highlightCell(c, cIdx, hl))}</td>`).join('')}
        </tr>`;
    }

    const btm = Math.max(0, (data.length - end) * ROW_H);
    if (btm > 0) html += `<tr style="height: ${btm}px; background: transparent;"><td colspan="${colSpan}" style="padding:0; border:none;"></td></tr>`;
    tbody.innerHTML = html;
    pinColWidths(t);
}

/* Column widths follow content, and only the visible slice is in the DOM, so
   each scroll re-sized the columns against different rows and they jumped.
   The first slice rendered decides, then the widths are pinned exactly like
   a manual resize (hence border-box: getBoundingClientRect and the resizer
   both measure the outer width). A hidden column measures 0, so it waits
   until it is shown; an added column is picked up on the next render. */
function pinColWidths(t) {
    if (!t.loaded || !t.filteredData.length) return;   // header alone would size to the labels
    /* A window opened from the file manager is born tiny, then tiled to full size:
       measuring in between would freeze every column at that tiny size. */
    if (container.clientWidth < 400) return;
    const missing = [];
    for (let i = 0; i < t.headers.length; i++) if (t.colWidths[i] == null && !t.hiddenCols.has(i)) missing.push(i);
    const cells = thead.rows[0] && thead.rows[0].cells;
    if (!missing.length || !cells) return;
    let changed = false;
    if (!idxColW) idxColW = Math.ceil(cells[0].getBoundingClientRect().width);
    missing.forEach(i => {
        const w = cells[i + 1] ? cells[i + 1].getBoundingClientRect().width : 0;
        /* A fixed cap, as on td: one relative to the window would depend on the
           window's size at the moment of measuring, which can be tiny. */
        if (w) { t.colWidths[i] = Math.min(Math.ceil(w), 480); changed = true; }
    });
    if (changed) applyColStyles();
}
