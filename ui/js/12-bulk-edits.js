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
    visitRows(t, t.filteredData, r => {
        const old = cellOf(r, col), nv = f(cellStr(old));
        if (nv !== cellStr(old)) out.push([r, old, nv]);
    });
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
    const ed = rowEdits();
    for (const [r, , nv] of ch) ed.set(r, col, nv);
    t.modificationsLog.push({ id: '-', col: t.headers[col], old: 'bulk', new: `${ch.length} cells`, what: `${fmt(ch.length)} cells edited in ${t.headers[col]}`,
        undo: () => ed.undo() });
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
function sortKind(t, col) {
    const counts = { n: 0, d: 0, t: 0 }; let seen = 0;
    for (const r of t.allData) { const ty = cellType(cellOf(r, col)); if (ty) { counts[ty]++; if (++seen >= 2000) break; } }
    return seen && counts.n / seen >= 0.9 ? 'n' : (seen && counts.d / seen >= 0.9 ? 'd' : 't');
}
/* Text keys → ranks: the distinct values sorted once with the collator
   (values it calls equal share a rank, so they keep their order, as ties
   do), then every row gets its value's rank. Empty (null) → NaN. */
function textRanks(col, coll) {
    const rank = new Map();
    for (let i = 0; i < col.length; i++) { const v = col[i]; if (v !== null && !rank.has(v)) rank.set(v, 0); }
    const uniq = [...rank.keys()].sort(coll.compare);
    let r = 0;
    for (let i = 0; i < uniq.length; i++) { if (i && coll.compare(uniq[i - 1], uniq[i]) !== 0) r++; rank.set(uniq[i], r); }
    const out = new Float64Array(col.length);
    for (let i = 0; i < col.length; i++) out[i] = col[i] === null ? NaN : rank.get(col[i]);
    return out;
}
/* A stable LSD radix sort of `order` on a Float64 key per row, 16 bits a
   pass: no comparison function, so 20 million rows sort in seconds rather
   than the better part of a minute. A double's bits compare as unsigned
   integers once the sign is folded; descending flips them; an empty key
   (NaN) is set to all ones, last both ways. */
function radixOrder(order, keys, dir) {
    const n = order.length, hi = new Uint32Array(n), lo = new Uint32Array(n);
    const f = new Float64Array(1), u = new Uint32Array(f.buffer);
    for (let i = 0; i < n; i++) {
        const x = keys[i];
        if (x !== x) { hi[i] = lo[i] = 0xFFFFFFFF; continue; }
        f[0] = x === 0 ? 0 : x;                          // -0 sorts as 0
        let h = u[1], l = u[0];
        if (h & 0x80000000) { h = ~h; l = ~l; } else h |= 0x80000000;
        if (dir < 0) { h = ~h; l = ~l; }
        hi[i] = h >>> 0; lo[i] = l >>> 0;
        if (hi[i] === 0xFFFFFFFF && lo[i] === 0xFFFFFFFF) lo[i] = 0xFFFFFFFE;   // keep all ones for the empty ones
    }
    let src = order, dst = new Uint32Array(n);
    const cnt = new Uint32Array(65536);
    for (const [arr, sh] of [[lo, 0], [lo, 16], [hi, 0], [hi, 16]]) {
        cnt.fill(0);
        for (let i = 0; i < n; i++) cnt[(arr[src[i]] >>> sh) & 0xFFFF]++;
        if (n && cnt[(arr[src[0]] >>> sh) & 0xFFFF] === n) continue;   // one digit for all: nothing to move
        let sum = 0;
        for (let d = 0; d < 65536; d++) { const c = cnt[d]; cnt[d] = sum; sum += c; }
        for (let i = 0; i < n; i++) { const v = src[i]; dst[cnt[(arr[v] >>> sh) & 0xFFFF]++] = v; }
        const tmp = src; src = dst; dst = tmp;
    }
    return src;
}

/* A plain click sorts on that column alone (again: reverses it). With `add`
   (Shift+click) the column becomes a further key, or flips if it is one. */
function sortBy(col, forceDir, add) {
    const t = T(); if (!t || !t.loaded) return;
    const prevSort = t.sort;
    let keys;
    if (add && prevSort) {
        const i = prevSort.findIndex(k => k.col === col);
        keys = i < 0 ? prevSort.concat([{ col, dir: forceDir || 1 }]) : prevSort.map((k, j) => j === i ? { col, dir: forceDir || -k.dir } : k);
    } else keys = [{ col, dir: forceDir || (prevSort && prevSort.length === 1 && prevSort[0].col === col ? -prevSort[0].dir : 1) }];
    const specs = keys.map(k => ({ ...k, kind: sortKind(t, k.col) }));
    const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' });
    /* One key column per sort key — numbers in a Float64Array (NaN: empty),
       text in an array (null: empty) — and a sort of row indices: no object
       per row. Array.prototype.sort is stable, so ties keep the current order. */
    const n = t.allData.length, cols = specs.map(sp => sp.kind === 't' ? new Array(n) : new Float64Array(n));
    visitRows(t, t.allData, (r, i) => {
        for (let k = 0; k < specs.length; k++) {
            const sp = specs[k], v = cellOf(r, sp.col), s = v == null ? '' : String(v).trim();
            cols[k][i] = sp.kind === 't' ? (s || null) : !s ? NaN : sp.kind === 'n' ? numKey(s) : dateKey(s);
        }
    });
    let order = new Uint32Array(n);
    for (let i = 0; i < n; i++) order[i] = i;
    for (let k = specs.length - 1; k >= 0; k--) {        // least significant key first: each pass is stable
        const num = specs[k].kind === 't' ? textRanks(cols[k], coll) : cols[k];
        order = radixOrder(order, num, specs[k].dir);
    }
    const all = t.allData, sorted = new Array(n);
    for (let i = 0; i < n; i++) sorted[i] = all[order[i]];
    t.sort = keys;
    container.scrollTop = 0;
    const desc = specs.map(sp => `${t.headers[sp.col]} ${sp.dir > 0 ? '↑' : '↓'}`).join(', then ');
    cols.length = 0; order.length = 0;    // the undo closure below shares this scope: don't let it keep them
    commitRows(t, sorted, { id: '-', col: t.headers[col], old: 'Sort', new: desc, what: `sort by ${desc}` },
        t => { t.sort = prevSort; });
    renderHeader(); applyColStyles();
    const kinds = specs.length === 1 ? ` (${{ n: 'numbers', d: 'dates', t: 'text' }[specs[0].kind]})` : '';
    setStats(`${t.name} | Sorted by ${desc}${kinds} — not written yet, use Save.`);
}

/* Keeps only the rows the filters show. The filters are then cleared: they
   would now match every row, and leaving them on would hide that. */
async function deleteHiddenRows() {
    const t = T(); if (!t || !t.loaded) return;
    const hidden = t.allData.length - t.filteredData.length;
    if (!hasFilter(t) || !hidden) { uiAlert('No row is hidden: set a filter first — every row it hides will be deleted.'); return; }
    if (!await uiConfirm(`Delete the ${fmt(hidden)} hidden rows?\n\nThe ${fmt(t.filteredData.length)} rows shown are kept, and the filters are cleared.`, { ok: 'Delete rows', danger: true })) return;
    /* In file order: the duplicates view shows its rows grouped, not in order. */
    const shown = new Set(t.filteredData), kept = t.allData.filter(r => shown.has(r));
    const prevQuery = t.globalQuery, prevFilters = t.colFilters, prevVals = t.valFilters, prevIrr = t.onlyIrregular, prevDups = t.onlyDups, prevMark = t.rowMark && t.rowMark.only;
    t.globalQuery = ''; t.colFilters = {}; t.valFilters = {}; t.onlyIrregular = false; t.onlyDups = false;
    if (t.rowMark) t.rowMark.only = false;
    document.getElementById('global-search').value = '';
    renderHeader(); applyColStyles();
    commitRows(t, kept, { id: '-', col: '---', old: `${hidden} hidden rows`, new: 'Deleted', what: `${fmt(hidden)} hidden rows deleted` },
        t => { t.globalQuery = prevQuery; t.colFilters = prevFilters; t.valFilters = prevVals; t.onlyIrregular = prevIrr; t.onlyDups = prevDups; if (t.rowMark) t.rowMark.only = prevMark; document.getElementById('global-search').value = prevQuery; });
    setStats(`${t.name} | ${fmt(hidden)} hidden rows deleted, ${fmt(kept.length)} kept — not written yet, use Save.`);
}

/* Duplicates are counted over ALL rows, filtered or not. A row whose
   compared cells are all empty is never a duplicate: 46 contacts without
   an e-mail are not one contact. */
function dedupeSpec(t) {
    const cols = [...document.querySelectorAll('#dedupe-cols input:checked')].map(i => +i.value);
    return { cols: cols.map(c => t.headers[c]), match: document.querySelector('input[name="dedupe-match"]:checked').value };
}
/* One comparison key per row of t.allData (null: all compared cells empty),
   or null when a column of the spec no longer exists. Columns are named,
   not indexed, so marks survive columns being added, moved or deleted. */
function dupKeys(t, spec) {
    const cols = spec.cols.map(h => t.headers.indexOf(h));
    if (cols.includes(-1)) return null;
    const use = cols.length ? cols : t.headers.map((_, i) => i);
    const str = v => String(v == null ? '' : v);
    const norm = spec.match === 'slug' ? v => slugify(str(v)) : spec.match === 'loose' ? v => str(v).trim().toLocaleLowerCase('fr') : str;
    const keys = new Array(t.allData.length);
    visitRows(t, t.allData, (r, i) => {
        const parts = use.map(c => norm(cellOf(r, c)));
        keys[i] = parts.every(p => p.trim() === '') ? null : parts.join('\u0001');
    });
    return keys;
}
function dedupeKeep() {
    const t = T(), keys = dupKeys(t, dedupeSpec(t));
    const last = document.querySelector('input[name="dedupe-keep"]:checked').value === 'last';
    const seen = new Set(), n = t.allData.length, keep = new Array(n);
    for (let k = 0; k < n; k++) {
        const i = last ? n - 1 - k : k, key = keys[i];
        keep[i] = key === null || !seen.has(key);
        if (key !== null) seen.add(key);
    }
    return keep;
}

/* ---- MARK DUPLICATES ------------------------------------------------
   The review before the deletion: every row of a duplicate group, the
   first occurrence included, gets a coloured edge (alternating per group),
   and a status bar chip filters the grid down to them, groups side by side.
   t.dupSpec keeps the settings, and the groups are recomputed at each
   applyFilters(), so edits and deletions update them. */
function dupGroups(t) {
    const spec = t.dupSpec && JSON.stringify(t.dupSpec);
    if (spec && t.dupCache && t.dupCache.spec === spec && sameStamp(t.dupCache.stamp, dataStamp(t))) return;   // same rows, same spec: same groups
    t.dupMarks = null; t.dupCache = null;
    if (!t.dupSpec) return;
    const keys = dupKeys(t, t.dupSpec);
    if (!keys) { t.dupSpec = null; t.onlyDups = false; return; }   // a compared column is gone
    const first = new Map(), count = new Map();
    keys.forEach(k => { if (k !== null) count.set(k, (count.get(k) || 0) + 1); });
    const group = new Map();
    t.allData.forEach((r, i) => {
        const k = keys[i];
        if (k === null || count.get(k) < 2) return;
        if (!first.has(k)) first.set(k, first.size);
        group.set(r, first.get(k));
    });
    t.dupMarks = { group, groups: first.size };
    t.dupCache = { spec, stamp: dataStamp(t) };
}
function markDuplicates() {
    const t = T(); if (!t) return;
    t.dupSpec = dedupeSpec(t); t.onlyDups = true;
    closeAllModals();
    applyFilters();
    if (!t.dupMarks || !t.dupMarks.groups) { t.dupSpec = null; t.onlyDups = false; applyFilters(); setStats(`${t.name} | No duplicate rows.`); return; }
    setStats(`${t.name} | ${fmt(t.dupMarks.group.size)} rows in ${fmt(t.dupMarks.groups)} duplicate groups, shown side by side — click the chip below to see all rows.`);
}
function toggleDupView() {
    const t = T(); if (!t || !t.dupSpec) return;
    t.onlyDups = !t.onlyDups;
    applyFilters();
}
function clearDupMarks(e) {
    if (e) e.stopPropagation();
    const t = T(); if (!t) return;
    t.dupSpec = null; t.onlyDups = false; t.dupMarks = null;
    applyFilters();
}
function dupCls(t, r, prev) {
    const g = t.dupMarks && t.dupMarks.group.get(r);
    if (g == null) return '';             // no marks at all (dupMarks null) or a row outside every group
    return ' dup ' + (g % 2 ? 'dup-b' : 'dup-a') + (t.onlyDups && prev && t.dupMarks.group.get(prev) !== g ? ' dup-first' : '');
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
    document.getElementById('dedupe-show').disabled = !dup;
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

/* ---------------------------------------------------------------
   VIRTUAL RENDERING
   Only a window of the ACTIVE tab is in the DOM, in both directions:
   the rows around the viewport and, once every column has a pinned
   width, only the columns around it — the others stand as one spacer
   cell on each side (colspan over the visible columns they replace).
   The window reaches one screen beyond the viewport each way, and a
   scroll re-renders only when the viewport nears its edge: the browser
   scrolls what is already drawn before any script runs, so rendering
   exactly the viewport showed black bands at every scroll, and an
   85-column file rebuilt 2 500 cells per scroll event.
----------------------------------------------------------------*/
let drawn = null;                         // { t, r0, r1, c0, c1 }: the window in the DOM (c0/c1: indices into visible columns)

/* The visible columns and their left edges in the table (after the row numbers), or null while widths are unknown. */
function colLayout(t) {
    const vis = visibleCols(t), x = new Array(vis.length + 1);
    let s = idxColW;
    for (let k = 0; k < vis.length; k++) { const w = t.colWidths[vis[k]]; if (w == null) return null; x[k] = s; s += w; }
    x[vis.length] = s;
    return { vis, x };
}
function viewRows(t) {
    const top = Math.max(0, Math.floor((container.scrollTop - thead.offsetHeight) / ROW_H));   // rows start below the header
    return [top, top + Math.ceil(container.clientHeight / ROW_H)];
}
function viewCols(L) {
    const a = container.scrollLeft + idxColW, b = container.scrollLeft + container.clientWidth;
    let k0 = 0; while (k0 < L.vis.length - 1 && L.x[k0 + 1] <= a) k0++;
    let k1 = k0; while (k1 < L.vis.length - 1 && L.x[k1 + 1] < b) k1++;
    return [k0, k1];
}
/* The window to draw around the viewport: its rows (a screen beyond it each
   way) and, once every column has a pinned width, its columns (a screen's
   width each side). */
function drawWindow(t) {
    const n = t.filteredData.length, [v0, v1] = viewRows(t), page = v1 - v0 + 1;
    const w = { r0: Math.max(0, v0 - page), r1: Math.min(n, v1 + page + 1) - 1, c0: null, c1: null, cols: null, left: 0, right: 0 };
    const L = colLayout(t);
    if (L) {
        const [k0, k1] = viewCols(L), a = container.scrollLeft - container.clientWidth, b = container.scrollLeft + 2 * container.clientWidth;
        let c0 = k0; while (c0 > 0 && L.x[c0] > a) c0--;
        let c1 = k1; while (c1 < L.vis.length - 1 && L.x[c1 + 1] < b) c1++;
        Object.assign(w, { c0, c1, cols: L.vis.slice(c0, c1 + 1), left: c0, right: L.vis.length - 1 - c1 });
    }
    return w;
}
/* One cell of row i, as HTML. */
function cellHtml(t, i, r, d, mk, cIdx, rg, fp) {
    const c = d[cIdx], m = mk && mk.has(t.headers[cIdx]);
    return `<td data-c="${cIdx}"${cellCls(i, cIdx, rg, fp, r, m)}${m ? markTitle(t, mk.get(t.headers[cIdx])) : ''}${barStyle(t, cIdx, c)}>${showBreaks(highlightCell(c, cIdx, t.hl))}</td>`;
}
/* Rows i0…i1 of the view, as HTML, in the window's columns. */
function rowsHtml(t, i0, i1, w) {
    const data = t.filteredData, rg = selRange(t), fp = fillRect();
    let html = '';
    for (let i = i0; i <= i1; i++) {
        const r = data[i], mk = markedCells(t, r), d = r.data;
        const cell = cIdx => cellHtml(t, i, r, d, mk, cIdx, rg, fp);
        html += `<tr class="${(r.id % 2 === 0) ? 'row-even' : 'row-odd'}${r.len !== t.headers.length ? ' irr' : ''}${dupCls(t, r, data[i - 1])}${t.rowMark && t.rowMark.rows.has(r) ? ' mk' : ''}" style="height:${ROW_H}px" data-idx="${i}">
            <td class="col-idx" draggable="true" title="Click: select the row · Drag: move it">
                <span class="row-num">${r.id.toLocaleString('fr-FR')}</span>
                <span class="row-btn" onclick="openRowMenu(event, ${r.id})" title="Insert, duplicate or delete this row"></span>
            </td>`
            + (w.cols ? (w.left ? `<td class="hsp" colspan="${w.left}"></td>` : '') + w.cols.map(cell).join('') + (w.right ? `<td class="hsp" colspan="${w.right}"></td>` : '')
                : d.map((_, cIdx) => cell(cIdx)).join(''))
            + '</tr>';
    }
    return html;
}
const spacer = (cls, rows, span) => `<tr class="${cls}" style="height: ${rows * ROW_H}px; background: transparent;"><td colspan="${span}" style="padding:0; border:none;"></td></tr>`;

/* The drawn rows move from columns drawn.c0…c1 to w.c0…w.c1 (indices into
   the visible columns): in each row, the cells leaving are removed, the
   ones entering inserted, and the spacer cell on each side (colspan: the
   visible columns it stands for) resized, created or dropped. */
function shiftCols(t, w, vis) {
    const o0 = drawn.c0, o1 = drawn.c1, n0 = w.c0, n1 = w.c1, data = t.filteredData, rg = selRange(t), fp = fillRect();
    const nl = n0, nr = vis.length - 1 - n1;
    for (const tr of tbody.querySelectorAll('tr[data-idx]')) {
        const i = +tr.dataset.idx, r = data[i], mk = markedCells(t, r), d = r.data;
        const html = (a, b) => { let h = ''; for (let k = a; k <= b; k++) h += cellHtml(t, i, r, d, mk, vis[k], rg, fp); return h; };
        const idxTd = tr.firstElementChild;
        let ls = idxTd.nextElementSibling; if (ls && !ls.classList.contains('hsp')) ls = null;
        let rs = tr.lastElementChild; if (rs === ls || !rs.classList.contains('hsp')) rs = null;
        /* Left edge */
        for (let k = o0; k < Math.min(n0, o1 + 1); k++) (ls || idxTd).nextElementSibling.remove();
        if (n0 < o0) (ls || idxTd).insertAdjacentHTML('afterend', html(n0, o0 - 1));
        if (nl && !ls) { idxTd.insertAdjacentHTML('afterend', `<td class="hsp" colspan="${nl}"></td>`); }
        else if (nl) ls.colSpan = nl;
        else if (ls) ls.remove();
        /* Right edge */
        for (let k = o1; k > Math.max(n1, o0 - 1); k--) (rs ? rs.previousElementSibling : tr.lastElementChild).remove();
        if (n1 > o1) (rs || tr).insertAdjacentHTML(rs ? 'beforebegin' : 'beforeend', html(o1 + 1, n1));
        if (nr && !rs) tr.insertAdjacentHTML('beforeend', `<td class="hsp" colspan="${nr}"></td>`);
        else if (nr) rs.colSpan = nr;
        else if (rs) rs.remove();
    }
    drawn.c0 = n0; drawn.c1 = n1;
}

/* On scroll, the window follows the viewport a few rows (ROW_STEP) or
   columns (COL_STEP) at a time: only the rows or cells entering and
   leaving it are added and removed. Re-laying out the whole table cost
   15 ms on 80 rows of an 85-column file, and a sideways move past the
   columns drawn redrew it all (~50 ms); adding rows costs ~1.5 ms plus
   ~0.4 ms a row, so small steps stay far under a frame. */
const ROW_STEP = 4, COL_STEP = 2;
function renderOnScroll() {
    const t = T();
    if (!t || !t.loaded || !drawn || drawn.t !== t || drawn.n !== t.filteredData.length) return render();
    const L = colLayout(t);
    /* Widths just pinned (all columns were drawn), or columns shown or hidden since: draw anew. */
    if (drawn.c0 == null ? !!L : (!L || L.vis.join(',') !== drawn.vis)) return render();
    const w = drawWindow(t);
    const rowsMove = Math.abs(w.r0 - drawn.r0) >= ROW_STEP || Math.abs(w.r1 - drawn.r1) >= ROW_STEP;
    const colsMove = L && (Math.abs(w.c0 - drawn.c0) >= COL_STEP || Math.abs(w.c1 - drawn.c1) >= COL_STEP);
    if (!rowsMove && !colsMove) return;
    if ((rowsMove && (w.r0 > drawn.r1 || w.r1 < drawn.r0)) || (colsMove && (w.c0 > drawn.c1 || w.c1 < drawn.c0))) return render();   // a jump: nothing to keep
    if (colsMove) shiftCols(t, w, L.vis);
    else if (L) Object.assign(w, { c0: drawn.c0, c1: drawn.c1, cols: L.vis.slice(drawn.c0, drawn.c1 + 1), left: drawn.c0, right: L.vis.length - 1 - drawn.c1 });   // rows entering take the columns drawn
    if (!rowsMove) return;
    const top = tbody.firstElementChild, btm = tbody.lastElementChild;
    /* Rows leaving at either end, then rows entering. */
    for (let i = drawn.r0; i < w.r0; i++) top.nextElementSibling.remove();
    for (let i = drawn.r1; i > w.r1; i--) btm.previousElementSibling.remove();
    if (w.r0 < drawn.r0) top.insertAdjacentHTML('afterend', rowsHtml(t, w.r0, drawn.r0 - 1, w));
    if (w.r1 > drawn.r1) btm.insertAdjacentHTML('beforebegin', rowsHtml(t, drawn.r1 + 1, w.r1, w));
    top.style.height = w.r0 * ROW_H + 'px';
    btm.style.height = (drawn.n - 1 - w.r1) * ROW_H + 'px';
    drawn.r0 = w.r0; drawn.r1 = w.r1;
}

function render() {
    const t = T();
    drawn = null;
    if (!t || !t.loaded) { tbody.innerHTML = ''; return; }
    const data = t.filteredData;
    if (data.length === 0) { tbody.innerHTML = '<tr><td colspan="100" style="padding: 20px; text-align: center;">No results found</td></tr>'; return; }
    const w = drawWindow(t), span = t.headers.length + 1;
    /* Both spacers are always there (height 0 at an end): the scroll updates resize them. */
    tbody.innerHTML = spacer('sp-top', w.r0, span) + rowsHtml(t, w.r0, w.r1, w) + spacer('sp-btm', data.length - 1 - w.r1, span);
    const L = colLayout(t);
    drawn = { t, n: data.length, r0: w.r0, r1: w.r1, c0: w.c0, c1: w.c1, vis: L ? L.vis.join(',') : null };
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
    const missing = [];
    for (let i = 0; i < t.headers.length; i++) if (t.colWidths[i] == null && !t.hiddenCols.has(i)) missing.push(i);
    const cells = thead.rows[0] && thead.rows[0].cells;
    if (!missing.length || !cells) return;   // the usual case, checked first: reading a size below forces a layout of the fresh rows
    /* A window opened from the file manager is born tiny, then tiled to full size:
       measuring in between would freeze every column at that tiny size. */
    if (container.clientWidth < 400) return;
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
