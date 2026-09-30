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
    sortMarks(t);
    const kinds = specs.length === 1 ? ` (${{ n: 'numbers', d: 'dates', t: 'text' }[specs[0].kind]})` : '';
    setStats(`${t.name} | Sorted by ${desc}${kinds} — not written yet, use Save.`);
}

/* The ▲ / ▼ marks of the header, in place: rebuilding the whole header after
   each sort cost ~20 ms on an 85-column file (its layout, as for the rows). */
function sortMarks(t) {
    thead.querySelectorAll('.sort-ind').forEach(x => x.remove());
    (t.sort || []).forEach((k, n) => {
        const span = thead.querySelector(`th[data-col="${k.col}"] .col-name`);
        if (span) span.insertAdjacentHTML('beforeend', `<span class="sort-ind">${k.dir > 0 ? '▲' : '▼'}${t.sort.length > 1 ? `<sup>${n + 1}</sup>` : ''}</span>`);
    });
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
    const use = cols.length ? cols : t.headers.map((_, i) => i), n = use.length;
    const str = v => String(v == null ? '' : v);
    /* toLowerCase, not toLocaleLowerCase('fr'): the same result for every locale but
       Turkish, and several times faster (the whole-row key on 600 k rows: 4.4 s → 1.2 s). */
    const norm = spec.match === 'slug' ? v => slugify(str(v)) : spec.match === 'loose' ? v => str(v).trim().toLowerCase() : str;
    const keys = new Array(t.allData.length), few = n <= 3;   // a few columns: cellOf each; many: one split of the record
    visitRows(t, t.allData, (r, i) => {
        const d = few ? null : r.data;
        let key = '', empty = true;
        for (let k = 0; k < n; k++) {
            const p = norm(few ? cellOf(r, use[k]) : d[use[k]]);
            if (empty && p.trim() !== '') empty = false;
            key = k ? key + '\u0001' + p : p;
        }
        keys[i] = empty ? null : key;
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
   the rows around the viewport and only the columns around it. The
   rows are div.row placed absolutely in #tbody at top = i × ROW_H,
   each a flex line of div.cell with their widths (in flow, not
   positioned: a positioned box is a paint layer, and 600 of them made
   the layerization cost more than the table's layout it saved) — not
   a <table>: a table relays out every drawn row and repaints the
   whole window at every insertion (~1.8 ms fixed on this machine,
   whatever the count), so a step's frame cost ~12 ms and dropped a
   frame at 120 Hz; independent rows cost only what enters, and the
   paint stays local (traced on a synthetic grid: 10.5 → 8.5 ms a
   step with 8 rows, and under 8 ms with rows inserted one by one).
   The header stays a table (#mainTable, sticky at the top of the
   layer): its cells are pinned to the same widths, so both line up.
   Nothing is laid out to measure a width: pinColWidths() computes
   them from the values (monospace: length × advance) and the header
   cells' own natural width. The window reaches one screen beyond the
   viewport each way; a scroll draws only what enters (renderOnScroll)
   and the order of the rows in the DOM is immaterial. Rows not drawn
   show the stripes of #tbody's background until they arrive.
----------------------------------------------------------------*/
let drawn = null;                         // { t, n, r0, r1, c0, c1, vis }: the window in the DOM (c0/c1: indices into visible columns)

/* The visible columns and their left edges (after the row numbers); every
   visible column has a width once pinColWidths() ran, which render() does first. */
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
   way) and its columns (a screen's width each side), as indices into the
   visible columns of L. lean (a jump while the scrollbar is dragged): a
   quarter of a screen around the viewport instead of a whole one — the next
   frame of the drag will land elsewhere anyway, and a lighter redraw keeps
   the thumb following the mouse; the incremental steps grow the window back
   once the motion slows. */
function drawWindow(t, lean) {
    const n = t.filteredData.length, [v0, v1] = viewRows(t), L = colLayout(t);
    const page = lean ? Math.ceil((v1 - v0 + 1) / 4) : v1 - v0 + 1;
    const w = { r0: Math.max(0, v0 - page), r1: Math.min(n, v1 + page + 1) - 1, c0: 0, c1: -1, L };
    if (!L || !L.vis.length) return w;
    const m = lean ? container.clientWidth / 4 : container.clientWidth;
    const [k0, k1] = viewCols(L), a = container.scrollLeft - m, b = container.scrollLeft + container.clientWidth + m;
    let c0 = k0; while (c0 > 0 && L.x[c0] > a) c0--;
    let c1 = k1; while (c1 < L.vis.length - 1 && L.x[c1 + 1] < b) c1++;
    w.c0 = c0; w.c1 = c1;
    return w;
}
/* The cells' font, measured once on a canvas: the advance of one character (the
   cells are monospace) and a context for values in other scripts. */
const PLAIN_RE = /^[\x20-\x7e\xa0-ɏ–-…€]*$/;
let cellAdvPx = 0, cellCtx = null;
function cellAdv() {
    if (cellAdvPx) return cellAdvPx;
    const probe = document.createElement('div'); probe.className = 'cell'; tbody.appendChild(probe);
    const st = getComputedStyle(probe); cellCtx = document.createElement('canvas').getContext('2d');
    cellCtx.font = `${st.fontStyle} ${st.fontWeight} ${st.fontSize} ${st.fontFamily}`; cellCtx.fontKerning = 'none';
    probe.remove();
    return cellAdvPx = cellCtx.measureText('0'.repeat(50)).width / 50 || 8;
}
/* A value's width as a cell shows it (nowrap: runs of spaces as one, none at the ends). */
function textWidth(s) {
    if (!s) return 0;
    const adv = cellAdv();
    if (!PLAIN_RE.test(s)) return cellCtx.measureText(s).width;
    const n = s.length, len = s.indexOf('  ') < 0 && s[0] !== ' ' && s[n - 1] !== ' ' ? n : s.replace(/ {2,}/g, ' ').trim().length;
    return len * adv;
}
/* Whether a cell keeps its clip (div.cell.ov: overflow hidden + ellipsis). A clip is
   a property node and a paint chunk of its own, and the 600 of a drawn window made
   Chromium's layerization the biggest cost of a scroll step; yet almost no cell
   overflows, the widths being pinned from the values themselves. A plain Latin
   value fits when its width (textWidth) stays within the column's width less
   padding and border; highlight marks and line breaks keep the clip. A column
   being resized clips every cell (#grid-layer.rz) until the rows are redrawn. */
function cellOv(t, cIdx, c, html) {
    if (c == null || c === '') return false;
    const s = typeof c === 'string' ? c : String(c);
    if (html.indexOf('<') >= 0) return true;
    return textWidth(s) > (t.colWidths[cIdx] == null ? 480 : t.colWidths[cIdx]) - 21 + 0.05;   // the width was pinned at ceil(widest value + 21): that value fits by construction
}
/* One cell of row i, as HTML: column k of the layout L. */
function cellHtml(t, i, r, d, mk, k, L, rg, fp) {
    const cIdx = L.vis[k], c = d[cIdx], m = mk && mk.has(t.headers[cIdx]), html = showBreaks(highlightCell(c, cIdx, t.hl));
    return `<div class="cell${cellCls(i, cIdx, rg, fp, r, m, cellOv(t, cIdx, c, html))}" data-c="${cIdx}"${m ? markTitle(t, mk.get(t.headers[cIdx])) : ''} style="width:${L.x[k + 1] - L.x[k]}px${barStyle(t, cIdx, c)}">${html}</div>`;
}
/* The spacer standing, in a row, for the columns before the window (none when it starts at the first). */
const hsp = (L, c0) => c0 ? `<div class="hsp" style="width:${L.x[c0] - idxColW}px"></div>` : '';
/* Rows i0…i1 of the view, as HTML, in the window's columns w.c0…w.c1 of w.L. */
function rowsHtml(t, i0, i1, w) {
    const data = t.filteredData, rg = selRange(t), fp = fillRect(), L = w.L, W = L.x[L.x.length - 1], sp = hsp(L, w.c0);
    let html = '';
    for (let i = i0; i <= i1; i++) {
        const r = data[i], mk = markedCells(t, r), d = r.data;
        html += `<div class="row ${(r.id % 2 === 0) ? 'row-even' : 'row-odd'}${r.len !== t.headers.length ? ' irr' : ''}${dupCls(t, r, data[i - 1])}${t.rowMark && t.rowMark.rows.has(r) ? ' mk' : ''}" style="top:${i * ROW_H}px;width:${W}px" data-idx="${i}">
            <div class="cell col-idx" draggable="true" style="width:${idxColW}px" title="Click: select the row · Drag: move it">
                <span class="row-num">${r.id.toLocaleString('fr-FR')}</span>
                <span class="row-btn" onclick="openRowMenu(event, ${r.id})" title="Insert, duplicate or delete this row"></span>
            </div>${sp}`;
        for (let k = w.c0; k <= w.c1; k++) html += cellHtml(t, i, r, d, mk, k, L, rg, fp);
        html += '</div>';
    }
    return html;
}
/* Rows i0…i1 redrawn in place (those in the window), after an edit that
   moves no row: a cell edit, a paste, Delete. */
function redrawRows(t, i0, i1) {
    if (!drawn || drawn.t !== t || drawn.n !== t.filteredData.length) return render();
    const L = colLayout(t);
    if (!L || L.vis.join(',') !== drawn.vis) return render();
    const w = { c0: drawn.c0, c1: drawn.c1, L };
    for (let i = Math.max(i0, drawn.r0); i <= Math.min(i1, drawn.r1); i++) {
        const row = tbody.querySelector(`.row[data-idx="${i}"]`);
        if (row) row.outerHTML = rowsHtml(t, i, i, w);
    }
}
/* The drawn rows move from columns drawn.c0…c1 to w.c0…w.c1 (indices into the
   visible columns): in each row the cells leaving are removed, the ones entering
   inserted at the edge they enter by, and the spacer before the window resized,
   created or dropped. */
function shiftCols(t, w) {
    const o0 = drawn.c0, o1 = drawn.c1, n0 = w.c0, n1 = w.c1, L = w.L, data = t.filteredData, rg = selRange(t), fp = fillRect();
    const pos = new Map(L.vis.map((c, k) => [c, k])), spw = n0 ? (L.x[n0] - idxColW) + 'px' : '';
    for (const row of tbody.querySelectorAll('.row[data-idx]')) {
        const i = +row.dataset.idx, r = data[i], mk = markedCells(t, r), d = r.data;
        const html = (a, b) => { let h = ''; for (let k = a; k <= b; k++) h += cellHtml(t, i, r, d, mk, k, L, rg, fp); return h; };
        for (const el of [...row.children]) { const c = el.dataset.c; if (c == null) continue; const k = pos.get(+c); if (k == null || k < n0 || k > n1) el.remove(); }
        let sp = row.children[1]; if (sp && !sp.classList.contains('hsp')) sp = null;
        if (n0 < o0) (sp || row.firstElementChild).insertAdjacentHTML('afterend', html(n0, Math.min(o0 - 1, n1)));
        if (n1 > o1) row.insertAdjacentHTML('beforeend', html(Math.max(o1 + 1, n0), n1));
        if (!n0) { if (sp) sp.remove(); }
        else if (sp) sp.style.width = spw;
        else row.firstElementChild.insertAdjacentHTML('afterend', hsp(L, n0));
    }
}
/* The drawn cells re-sized from the widths (a column being resized): the width of
   every cell and spacer in place, no redraw. */
function placeCells(t) {
    const L = colLayout(t); if (!L || !drawn) return;
    const W = L.x[L.x.length - 1] + 'px', pos = new Map(L.vis.map((c, k) => [c, k])), spw = drawn.c0 ? (L.x[drawn.c0] - idxColW) + 'px' : '';
    tbody.style.width = W;
    for (const row of tbody.children) {
        if (row.dataset.idx == null) continue;
        row.style.width = W;
        for (const el of row.children) {
            if (el.classList.contains('hsp')) { el.style.width = spw; continue; }
            const c = el.dataset.c; if (c == null) continue; const k = pos.get(+c); if (k == null) continue; el.style.width = (L.x[k + 1] - L.x[k]) + 'px';
        }
    }
}

/* A scroll moves the window by steps: rows or columns come in only once the
   viewport is ROW_STEP rows / COL_STEP columns from the window's edge, then
   the rows (cells) leaving are removed and the ones entering appended. Traced
   under a wheel scroll: a step's frame is mostly the text shaping of what
   enters, so 8 rows a step keeps the number of such frames low. */
const ROW_STEP = 8, COL_STEP = 2;
/* A jump redraw (script + layout) past JUMP_BUDGET ms gives the following frames
   back: the next jumps are skipped for as long as that redraw took, the layer still
   showing the rows last drawn, and a trailing timer draws the last position. A
   narrow file redraws every frame; an 85-column one about every third, and the
   thumb follows the mouse instead of the redraws. */
let JUMP_BUDGET = 12, lastJumpAt = 0, lastJumpMs = 0, jumpTimer = 0;
function renderOnScroll() {
    const t = T();
    if (!t || !t.loaded || !drawn || drawn.t !== t || drawn.n !== t.filteredData.length) return render();
    const L = colLayout(t);
    if (!L || L.vis.join(',') !== drawn.vis) return render();   // columns shown or hidden since: draw anew
    const w = drawWindow(t);
    const rowsMove = Math.abs(w.r0 - drawn.r0) >= ROW_STEP || Math.abs(w.r1 - drawn.r1) >= ROW_STEP;
    const colsMove = Math.abs(w.c0 - drawn.c0) >= COL_STEP || Math.abs(w.c1 - drawn.c1) >= COL_STEP;
    if ((rowsMove && (w.r0 > drawn.r1 || w.r1 < drawn.r0)) || (colsMove && (w.c0 > drawn.c1 || w.c1 < drawn.c0))) {   // a jump: nothing to keep, a lean redraw
        const now = performance.now();
        if (lastJumpMs > JUMP_BUDGET && now - lastJumpAt < lastJumpMs) {
            /* Skipped: the layer is NOT moved either — moved without its rows redrawn, it
               would show the striped background where the rows should be, a frame of flicker. */
            clearTimeout(jumpTimer); jumpTimer = setTimeout(() => { jumpTimer = 0; renderOnScroll(); }, lastJumpMs);
            return;
        }
        render(true);                         // syncs the layer itself
        void tbody.offsetHeight;              // the layout now rather than at paint: it is part of what the jump costs
        lastJumpAt = performance.now(); lastJumpMs = lastJumpAt - now;
        return;
    }
    syncLayer();                              // the layer follows the scroll only with rows drawn at the new position
    if (!rowsMove && !colsMove) return;
    if (colsMove) { shiftCols(t, w); drawn.c0 = w.c0; drawn.c1 = w.c1; }
    else { w.c0 = drawn.c0; w.c1 = drawn.c1; }   // rows entering take the columns drawn
    if (!rowsMove) return;
    for (const row of tbody.querySelectorAll('.row[data-idx]')) { const i = +row.dataset.idx; if (i < w.r0 || i > w.r1) row.remove(); }
    let h = '';
    if (w.r0 < drawn.r0) h += rowsHtml(t, w.r0, Math.min(drawn.r0 - 1, w.r1), w);
    if (w.r1 > drawn.r1) h += rowsHtml(t, Math.max(drawn.r1 + 1, w.r0), w.r1, w);
    if (h) tbody.insertAdjacentHTML('beforeend', h);
    drawn.r0 = w.r0; drawn.r1 = w.r1;
}

function render(lean) {
    const t = T();
    drawn = null;
    clearTimeout(jumpTimer); jumpTimer = 0;   // a redraw for any reason: nothing left to catch up
    syncLayer();
    if (!t || !t.loaded) { tbody.innerHTML = ''; tbody.style.height = '0px'; tbody.classList.remove('no-rows'); return; }
    const data = t.filteredData;
    if (data.length === 0) { tbody.innerHTML = '<div class="empty-msg">No results found</div>'; tbody.style.height = ''; tbody.classList.add('no-rows'); syncSpace(t); return; }
    tbody.classList.remove('no-rows');
    pinColWidths(t);                          // every cell is placed by the widths: they come first
    const w = drawWindow(t, lean), L = w.L;
    tbody.style.height = data.length * ROW_H + 'px'; tbody.style.width = L.x[L.x.length - 1] + 'px';
    tbody.innerHTML = rowsHtml(t, w.r0, w.r1, w);
    drawn = { t, n: data.length, r0: w.r0, r1: w.r1, c0: w.c0, c1: w.c1, vis: L.vis.join(',') };
    syncSpace(t);
}

/* Column widths: a visible column without one gets the larger of its header
   cell's natural width (the header is a table laid out on its own, so that is
   its own need) and the widest value of the first rows in view — in the cells'
   monospace font, length × advance, a canvas measure for other scripts — plus
   padding and border, capped at 480 px like a manual resize, then pinned
   (applyColStyles, which sizes the header cells the same). No row is laid out
   to measure it. A hidden column measures 0 and is pinned once shown; an added
   column on the next render. */
function pinColWidths(t) {
    if (!t.loaded) return;
    const missing = [];
    for (let i = 0; i < t.headers.length; i++) if (t.colWidths[i] == null && !t.hiddenCols.has(i)) missing.push(i);
    const cells = thead.rows[0] && thead.rows[0].cells;
    if (!idxColW) idxColW = (cells && Math.ceil(cells[0].getBoundingClientRect().width)) || 90;
    if (!missing.length) return;
    const [v0] = viewRows(t), rows = t.filteredData.slice(v0, v0 + 80);
    missing.forEach(i => {
        let w = cells && cells[i + 1] ? cells[i + 1].getBoundingClientRect().width : 0;
        for (const r of rows) w = Math.max(w, textWidth(cellStr(r.data[i])) + 21);   // padding 10 + 10, border 1
        t.colWidths[i] = Math.min(Math.max(Math.ceil(w), 60), 480);
    });
    applyColStyles();
}

/* ---------------------------------------------------------------
   SCROLLING
   The table is not scrolled by the browser: it sits in #grid-layer, a
   sticky, clipped box the size of the container's viewport, and the
   native scrollbars belong to the container, whose extent comes from
   #scroll-space, sized like the table. Each scroll event copies the
   container's offsets onto the layer (syncLayer) and draws the window
   (renderOnScroll) — on the main thread, before the frame is painted.
   Why: the compositor scrolls a painted layer at once, a frame before the
   script draws the rows a jump landed on, so dragging the scrollbar's
   thumb over a big file showed a blank band at every frame. A sticky
   layer is repositioned by the compositor too, so it keeps showing the
   rows last drawn until the script replaces them: what VS Code does with
   its own scrollbar, with the native one kept. The sticky header and
   row numbers work as before, the layer being their scrollport.
----------------------------------------------------------------*/
/* Looked up at each call, not consts: resizeContainer() calls syncSpace() at boot, before this file's top level has run. */
function syncLayer() {
    const gridLayer = document.getElementById('grid-layer');
    if (gridLayer.scrollTop !== container.scrollTop) gridLayer.scrollTop = container.scrollTop;
    if (gridLayer.scrollLeft !== container.scrollLeft) gridLayer.scrollLeft = container.scrollLeft;
    stripFollow();                        // the scroll strip's thumb (28-…)
}
/* The layer the size of the viewport, the spacer the rest of the table's extent
   (rows × ROW_H + the header, and the pinned widths' sum — measured only while
   widths are still unknown). */
function syncSpace(t) {
    const gridLayer = document.getElementById('grid-layer'), scrollSpace = document.getElementById('scroll-space');
    const cw = container.clientWidth, ch = container.clientHeight, sbw = container.offsetWidth - cw;
    gridLayer.style.width = (sbw >= 16 ? cw : cw - (24 - sbw)) + 'px'; gridLayer.style.height = ch + 'px';   // overlay scrollbars: room left for the strip (24 = STRIP_W, a const of 28-… not yet declared when this runs at boot)
    if (!t || !t.loaded) { scrollSpace.style.height = '0px'; scrollSpace.style.width = '1px'; stripLayout(null); return; }
    const L = colLayout(t), w = L ? L.x[L.x.length - 1] : document.getElementById('mainTable').offsetWidth;
    const nohs = !!L && w <= cw;              // no sideways scroll: the row numbers need not stick (app.css)
    if (gridLayer.classList.contains('nohs') !== nohs) gridLayer.classList.toggle('nohs', nohs);
    const h = thead.offsetHeight + t.filteredData.length * ROW_H;
    scrollSpace.style.height = Math.max(0, h - ch) + 'px';
    scrollSpace.style.width = Math.max(1, w) + 'px';
    syncLayer();
    stripLayout(t);
}
