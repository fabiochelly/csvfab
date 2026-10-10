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
/* Text keys → ranks: the values put in the collator's order (values it calls
   equal share a rank, so they keep their order, as ties do), then every row
   gets its value's rank. Empty (null) → NaN.
   Sorting with coll.compare costs n·log n collator calls, ~150 ns each: 12.8 M
   distinct values took 42 s. So the values are first radix-sorted on numeric
   keys made from their first TEXT_KEY_W characters, weighed by the collator
   itself (textWeights()), which puts them in place without a single call; one
   pass of coll.compare then checks the order while it numbers the ranks.
   Anything the keys cannot see — a tie past their length, a long number, an
   expansion such as ß = ss, a script beyond U+024F — shows up as a pair out
   of order, and the array is then sorted with the collator: nearly sorted,
   Array.prototype.sort (TimSort) needs few calls on it. The order is the
   collator's in every case, never the keys'.
   Values all distinct are ordered row by row: a Map of millions of strings
   cost more than the sort (4.6 s of 12 for 12.8 M). Repeated ones (names,
   cities) are ordered once each through the Map. Fewer than TEXT_KEY_MIN
   distinct values, or values whose keys tell them apart no better than by
   chance (a long common prefix: URLs, paths), keep the plain collator sort,
   which the keys would only add to. */
const TEXT_KEY_W = 18;                   // characters in the keys: three float64 of six 8-bit weights
const TEXT_KEY_MIN = 5000;               // distinct values below which the plain sort is as fast
function textRanks(col, coll) {
    const n = col.length, out = new Float64Array(n);
    if (n >= TEXT_KEY_MIN) {
        // Row by row only when 16 384 rows spread over the column hold no value
        // twice: 867 k e-mails over 5 M rows show ~160 repeats there, and were
        // 7× faster through the Map (the keys on 5 M rows instead of 867 k).
        const step = Math.max(1, Math.floor(n / 16384)), sample = new Set();
        let filled = 0;
        for (let i = 0; i < n; i += step) if (col[i] !== null) { sample.add(col[i]); filled++; }
        let w;
        if (sample.size === filled && (w = textKeysUseful([...sample].slice(0, 4096), coll))) {
            const idx = [];
            for (let i = 0; i < n; i++) if (col[i] !== null) idx.push(i);
            const vals = idx.map(i => col[i]), rk = collatorRanks(vals, coll, w);
            out.fill(NaN);
            for (let j = 0; j < idx.length; j++) out[idx[j]] = rk[j];
            return out;
        }
    }
    const rank = new Map();
    for (let i = 0; i < n; i++) { const v = col[i]; if (v !== null && !rank.has(v)) rank.set(v, 0); }
    const uniq = [...rank.keys()];
    let w;
    if (uniq.length >= TEXT_KEY_MIN && (w = textKeysUseful(uniq.slice(0, 4096), coll))) {
        const rk = collatorRanks(uniq, coll, w);
        for (let j = 0; j < uniq.length; j++) rank.set(uniq[j], rk[j]);
    } else {
        uniq.sort(coll.compare);
        let r = 0;
        for (let i = 0; i < uniq.length; i++) { if (i && coll.compare(uniq[i - 1], uniq[i]) !== 0) r++; rank.set(uniq[i], r); }
    }
    for (let i = 0; i < n; i++) out[i] = col[i] === null ? NaN : rank.get(col[i]);
    return out;
}
/* The character weights when the keys of a sample of distinct values tell at
   least half of them apart, else null. */
function textKeysUseful(sample, coll) {
    if (sample.length < 2) return null;
    const w = textWeights(coll), k = [new Float64Array(1), new Float64Array(1), new Float64Array(1)], seen = new Set();
    for (const v of sample) { textKeys(v, w, k[0], k[1], k[2], 0); seen.add(k[0][0] + ',' + k[1][0] + ',' + k[2][0]); }
    return seen.size * 2 >= sample.length ? w : null;
}
/* The rank of each of vals (non-null strings) in the collator's order: by
   the keys, each run of equal keys then sorted with the collator (values
   alike past TEXT_KEY_W characters: e-mails of the same name), checked by
   rankWalk(), sorted again with the collator only if a pair is still out of
   order. */
function collatorRanks(vals, coll, w) {
    const n = vals.length, k1 = new Float64Array(n), k2 = new Float64Array(n), k3 = new Float64Array(n);
    for (let i = 0; i < n; i++) textKeys(vals[i], w, k1, k2, k3, i);
    let o = new Uint32Array(n);
    for (let i = 0; i < n; i++) o[i] = i;
    o = radixOrder(radixOrder(radixOrder(o, k3, 1), k2, 1), k1, 1);
    const cmp = (a, b) => vals[a] === vals[b] ? 0 : coll.compare(vals[a], vals[b]);
    for (let i = 0; i < n;) {
        const a = o[i];
        let j = i + 1;
        while (j < n && k1[o[j]] === k1[a] && k2[o[j]] === k2[a] && k3[o[j]] === k3[a]) j++;
        if (j - i > 1) o.set(Array.from(o.subarray(i, j)).sort(cmp), i);
        i = j;
    }
    const rk = new Float64Array(n);
    if (!rankWalk(vals, o, coll, rk)) {
        o = Array.from(o).sort(cmp);
        rankWalk(vals, o, coll, rk);
    }
    return rk;
}
/* Ranks along order o into rk (indexed like vals); false at the first pair
   out of order. Equal strings need no collator call. */
function rankWalk(vals, o, coll, rk) {
    let r = 0;
    for (let i = 0; i < o.length; i++) {
        const v = vals[o[i]];
        if (i) {
            const p = vals[o[i - 1]];
            if (p !== v) { const c = coll.compare(p, v); if (c > 0) return false; if (c) r++; }
        }
        rk[o[i]] = r;
    }
    return true;
}
/* A weight per character up to U+024F, from the collator's own order of
   them one by one (equal ones share it: a = A = à under sensitivity 'base'),
   0 for those it ignores; 188 weights for 'fr', so 8 bits each. Built at
   every sort: ~1 ms. */
function textWeights(coll) {
    const chars = [];
    for (let c = 0x20; c < 0x250; c++) chars.push(String.fromCharCode(c));
    chars.sort(coll.compare);
    const w = new Uint8Array(0x250);
    let r = 0, prev = null;
    for (const ch of chars) {
        if (coll.compare(ch, '') === 0) continue;
        if (prev !== null && coll.compare(prev, ch) !== 0) r++;
        w[ch.charCodeAt(0)] = Math.min(254, r + 1); prev = ch;
    }
    return w;
}
/* The keys of value s, written at i in k1–k3. A run of digits is compared as
   a number (the collator is numeric): its length without leading zeros, then
   its digits, so 9 < 10 < 010 = 10. Characters past U+024F weigh 255. */
function textKeys(s, w, k1, k2, k3, i) {
    let a = 0, b = 0, c3 = 0, n = 0;
    const put = x => { if (n < 6) a = a * 256 + x; else if (n < 12) b = b * 256 + x; else c3 = c3 * 256 + x; n++; };
    for (let p = 0; p < s.length && n < TEXT_KEY_W; p++) {
        let c = s.charCodeAt(p);
        if (c >= 48 && c <= 57) {
            let j = p; while (j < s.length && s.charCodeAt(j) === 48) j++;
            let e = j; while (e < s.length && (c = s.charCodeAt(e)) >= 48 && c <= 57) e++;
            if (e === j) j = e - 1;                  // only zeros: the number 0
            put(w[48 + Math.min(9, e - j)]);
            for (let d = j; d < e && n < TEXT_KEY_W; d++) put(w[s.charCodeAt(d)]);
            p = e - 1; continue;
        }
        const x = c < 0x250 ? w[c] : 255;
        if (x) put(x);
    }
    while (n < TEXT_KEY_W) put(0);
    k1[i] = a; k2[i] = b; k3[i] = c3;
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
    const motion = sortMotionBefore(t);       // where the rows on screen are now (50-…)
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
    sortMotionFrom(motion, order);            // where the new first rows were (50-…)
    const all = t.allData, sorted = new Array(n);
    for (let i = 0; i < n; i++) sorted[i] = all[order[i]];
    t.sort = keys;
    container.vTop = 0;
    const desc = specs.map(sp => `${t.headers[sp.col]} ${sp.dir > 0 ? '↑' : '↓'}`).join(', then ');
    cols.length = 0; order.length = 0;    // the undo closure below shares this scope: don't let it keep them
    motionSort = true;                        // the sort's own motion (50-…), not the filter's (52-…)
    try {
        commitRows(t, sorted, { id: '-', col: t.headers[col], old: 'Sort', new: desc, what: `sort by ${desc}` },
            t => { t.sort = prevSort; });
    } finally { motionSort = false; }
    sortMarks(t);
    sortMotionAfter(t, motion);               // …and the glide to where they are now
    const kinds = specs.length === 1 ? ` (${{ n: 'numbers', d: 'dates', t: 'text' }[specs[0].kind]})` : '';
    setStats(`${t.name} | Sorted by ${desc}${kinds} — not written yet, use Save.`);
}

/* A sort key's mark in its title: an arrow (a CSS mask, up for ascending — .dn turns it down),
   numbered when there are several keys. */
function sortIndHtml(t, n) {
    return `<span class="sort-ind${t.sort[n].dir > 0 ? '' : ' dn'}" title="${t.sort[n].dir > 0 ? 'Ascending' : 'Descending'}">${t.sort.length > 1 ? `<sup>${n + 1}</sup>` : ''}</span>`;
}
/* The marks of the header, in place: rebuilding the whole header after
   each sort cost ~20 ms on an 85-column file (its layout, as for the rows). */
function sortMarks(t) {
    thead.querySelectorAll('.sort-ind').forEach(x => x.remove());
    thead.querySelectorAll('.col-name.srt').forEach(x => x.classList.remove('srt'));
    (t.sort || []).forEach((k, n) => {
        const span = thead.querySelector(`th[data-col="${k.col}"] .col-name`);
        if (!span) return;
        span.classList.add('srt');            // laid out as a line whose text gives way (app.css): the text in its own span
        if (!span.querySelector('.cn-t')) for (const x of span.childNodes) if (x.nodeType === 3) { const w = document.createElement('span'); w.className = 'cn-t'; x.replaceWith(w); w.appendChild(x); break; }
        const no = span.querySelector('.col-no');   // the arrow goes before the number, which gives way first
        if (no) no.insertAdjacentHTML('beforebegin', sortIndHtml(t, n)); else span.insertAdjacentHTML('beforeend', sortIndHtml(t, n));
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

/* Duplicates: 31-duplicates.js. */

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
    /* The column's kind as a class of its cells (numbers right-aligned, numbers and dates
       coloured): one class per cell, no per-column rule for every cell to be matched against. */
    const kinds = t.loaded ? columnKinds(t) : [];
    const kc = vis.map(c => kinds[c] === 'n' ? ' kn' : kinds[c] === 'd' ? ' kd' : '');
    /* Pills for a column of categories (51-…): null when none. */
    const look = t.loaded && !t.lang ? gridLook(t) : null;
    const pill = look && look.cat.size ? vis.map(c => look.cat.get(c) || null) : null;
    const rich = look ? richCols(t) : null, rk = rich && rich.size ? vis.map(c => rich.get(c) || null) : null;   // links, checks, swatches… (51-…)
    /* F: the visible columns frozen at the left (t.frozen: 1 from the menu, any count from a title's
       right click, freezeTo()). They are drawn in every row whatever the window, sticky right after the row
       numbers; the window of other columns starts after them. */
    /* hh: a colour scale somewhere (06-…) — otherwise no cell asks (a scroll step draws hundreds). */
    let hh = false; for (const _ in t.heat) { hh = true; break; }
    return { vis, x, kc, pill, rk, hh, F: Math.min(t.frozen || 0, vis.length) };
}
function viewRows(t) {
    /* The scroll read kept (viewRows.st): the motions of a sort or a filter (50-, 52-…) note the rows
       on screen when the DOM is dirty, where a read of scrollTop forced a layout (~3 ms a filter). */
    const top = Math.max(0, Math.floor(((viewRows.st = container.vTop) - thead.offsetHeight) / ROW_H));   // rows start below the header
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
    const w = { r0: Math.max(0, v0 - page), r1: Math.min(n, v1 + page + 1) - 1, c0: 0, c1: -1, L, v0, v1: Math.min(v1, n - 1), k0: 0, k1: -1 };
    if (!L || !L.vis.length) return w;
    const m = lean ? container.clientWidth / 4 : container.clientWidth;
    const [k0, k1] = viewCols(L), a = container.scrollLeft - m, b = container.scrollLeft + container.clientWidth + m;
    w.k0 = Math.max(k0, L.F); w.k1 = k1;
    let c0 = k0; while (c0 > 0 && L.x[c0] > a) c0--;
    let c1 = k1; while (c1 < L.vis.length - 1 && L.x[c1 + 1] < b) c1++;
    w.c0 = Math.max(c0, L.F); w.c1 = c1;      // the frozen columns are drawn apart (rowsHtml)
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
    if (html.indexOf('<') >= 0) return !t.lang || html.indexOf('<mark') >= 0 || s.length * cellAdv() > (t.colWidths[cIdx] || 480) - 30;   // a text line's colours are spans, not a reason to clip
    return textWidth(s) > (t.colWidths[cIdx] == null ? 480 : t.colWidths[cIdx]) - 21 + 0.05;   // the width was pinned at ceil(widest value + 21): that value fits by construction
}
/* One cell of row i, as HTML: column k of the layout L. */
/* What a cell puts in the DOM: its value's beginning only. A column is 900 px at
   most (30 000 for a text line), far less than these, and the browser lays a
   cell's text out in more than linear time — a 400 KB field took 17 s to show,
   a 5 MB one froze the page. The value itself is untouched (editor, row card,
   find, save all read it whole). */
const CELL_SHOWN = 2000, TEXT_SHOWN = 5000;
function cellShown(c, max) {
    if (c == null || c.length <= max) return c;
    const s = String(c);
    let n = max;
    if (s.charCodeAt(n - 1) >= 0xD800 && s.charCodeAt(n - 1) < 0xDC00) n--;   // never half a surrogate pair
    return s.slice(0, n) + '…';
}
function cellHtml(t, i, r, d, mk, k, L, rg, fp) {
    let cIdx = L.vis[k], c = d[cIdx], m = mk && mk.has(t.headers[cIdx]), html = t.lang ? textCellHtml(t, cellShown(c, TEXT_SHOWN), cIdx) : showBreaks(highlightCell(cellShown(c, CELL_SHOWN), cIdx, t.hl));
    const frz = k < L.F ? (k === L.F - 1 ? ' frz frz-last' : ' frz') : '';   // frozen: sticky at its own left edge
    const g = cIdx === ffCol ? ffGhostHtml(t, r, c, i) : '';   // a rule's value offered for an empty cell (48-…)
    const pa = !g && L.pill && L.pill[k] ? pillAttrs(L.pill[k], c) : null;   // a category's pill, the cell's background (51-…)
    const ra = !g && !pa && L.rk && L.rk[k] ? richAttrs(L.rk[k], c, L.x[k + 1] - L.x[k]) : null;   // a link, a check, a swatch (51-…)
    if (ra && ra.html != null) html = ra.html;
    const ov = g ? true : pa ? pa.w + 24 > L.x[k + 1] - L.x[k] || html.indexOf('<mark') >= 0 : ra && ra.ov != null ? ra.ov || html.indexOf('<mark') >= 0 : ra && ra.html != null ? false : cellOv(t, cIdx, c, html);
    return `<div class="cell${t.lang ? ' tx' : ''}${L.kc[k]}${frz}${pa ? pa.cls : ''}${ra ? ra.cls : ''}${t.dupMarks ? dupCellCls(t, r, cIdx) : ''}${cellCls(i, cIdx, rg, fp, r, m, ov)}" data-c="${cIdx}"${m ? markTitle(t, mk.get(t.headers[cIdx])) : ''} style="width:${L.x[k + 1] - L.x[k]}px${frz ? `;left:${L.x[k]}px` : ''}${pa ? `;--pw:${pa.w}px` : ''}${ra ? ra.style : ''}${barStyle(t, cIdx, c)}${L.hh ? heatStyle(t, cIdx, c) : ''}">${g || html}</div>`;
}
/* The spacer standing, in a row, for the columns between the frozen ones (or the row numbers)
   and the window — none when the window starts right there. */
const hspW = (L, c0) => c0 > L.F ? (L.x[c0] - L.x[L.F]) + 'px' : '';
const hsp = (L, c0) => c0 > L.F ? `<div class="hsp" style="width:${hspW(L, c0)}"></div>` : '';
/* Rows i0…i1 of the view, as HTML, in the window's columns w.c0…w.c1 of w.L. */
function rowsHtml(t, i0, i1, w) {
    const data = t.filteredData, rg = selRange(t), fp = fillRect(), L = w.L, W = L.x[L.x.length - 1], sp = hsp(L, w.c0);
    const gc = t.sort ? groupCol(t) : -1, xr = rg ? sel.fr : -1;   // a sorted view's group lines, the active row (51-…)
    let html = '';
    for (let i = i0; i <= i1; i++) {
        const r = data[i], mk = markedCells(t, r), d = r.data;
        html += `<div class="row ${(r.id % 2 === 0) ? 'row-even' : 'row-odd'}${r.len !== t.headers.length ? ' irr' : ''}${dupCls(t, r, data[i - 1])}${t.rowMark && t.rowMark.rows.has(r) ? ' mk' : ''}${gc >= 0 && groupStart(t, gc, i, d) ? ' gs' : ''}${i === xr ? ' xr' : ''}" style="top:${i * ROW_H - vscroll.base}px;width:${W}px" data-idx="${i}">
            <div class="cell col-idx" draggable="true" style="width:${idxColW}px" title="Drag: move">
                <span class="row-num">${r.id.toLocaleString('fr-FR')}</span>
                <span class="row-btn" onclick="openRowMenu(event, ${r.id})" title="Insert, duplicate or delete this row"></span>
            </div>`;
        for (let k = 0; k < L.F; k++) html += cellHtml(t, i, r, d, mk, k, L, rg, fp);
        html += sp;
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
    if (!L || L.vis.join(',') + '|' + L.F !== drawn.vis) return render();
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
    const pos = new Map(L.vis.map((c, k) => [c, k])), F = L.F, spw = hspW(L, n0);
    for (const row of tbody.querySelectorAll('.row[data-idx]')) {
        const i = +row.dataset.idx, r = data[i], mk = markedCells(t, r), d = r.data;
        const html = (a, b) => { let h = ''; for (let k = a; k <= b; k++) h += cellHtml(t, i, r, d, mk, k, L, rg, fp); return h; };
        for (const el of [...row.children]) { const c = el.dataset.c; if (c == null) continue; const k = pos.get(+c); if (k == null || (k >= F && (k < n0 || k > n1))) el.remove(); }
        const lead = row.children[F];             // the row number, or the last frozen cell: the spacer comes after it
        let sp = row.children[F + 1]; if (sp && !sp.classList.contains('hsp')) sp = null;
        if (n0 < o0) (sp || lead).insertAdjacentHTML('afterend', html(n0, Math.min(o0 - 1, n1)));
        if (n1 > o1) row.insertAdjacentHTML('beforeend', html(Math.max(o1 + 1, n0), n1));
        if (n0 <= F) { if (sp) sp.remove(); }
        else if (sp) sp.style.width = spw;
        else lead.insertAdjacentHTML('afterend', hsp(L, n0));
    }
}
/* The drawn cells re-sized from the widths (a column being resized): the width of
   every cell and spacer in place, no redraw. */
function placeCells(t) {
    const L = colLayout(t); if (!L || !drawn) return;
    const W = L.x[L.x.length - 1] + 'px', pos = new Map(L.vis.map((c, k) => [c, k])), spw = hspW(L, drawn.c0);
    tbody.style.width = W;
    for (const row of tbody.children) {
        if (row.dataset.idx == null) continue;
        row.style.width = W;
        for (const el of row.children) {
            if (el.classList.contains('hsp')) { el.style.width = spw; continue; }
            const c = el.dataset.c; if (c == null) continue; const k = pos.get(+c); if (k == null) continue; el.style.width = (L.x[k + 1] - L.x[k]) + 'px';
            if (k < L.F) el.style.left = L.x[k] + 'px';
        }
    }
    totRedraw(t);                             // the totals row (54-…) at the new widths
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
    if (!L || L.vis.join(',') + '|' + L.F !== drawn.vis) return render();   // columns shown or hidden since: draw anew
    const w = drawWindow(t);
    /* Moved by steps — or as soon as the viewport is no longer all drawn: near the file's end
       the window's edge can only move by what is left (n − 1 − r1 < ROW_STEP), and a lean
       window on a short screen grows by less than a step; either left rows undrawn on screen,
       the striped background in their place (the last 3 rows of a 31-row file, 2026-10-07). */
    const rowsMove = Math.abs(w.r0 - drawn.r0) >= ROW_STEP || Math.abs(w.r1 - drawn.r1) >= ROW_STEP || w.v0 < drawn.r0 || w.v1 > drawn.r1;
    const colsMove = Math.abs(w.c0 - drawn.c0) >= COL_STEP || Math.abs(w.c1 - drawn.c1) >= COL_STEP || w.k0 < drawn.c0 || w.k1 > drawn.c1;
    /* Past the scroll cap the rows are placed from a base (vscroll, 01-…): a new one moves them all, a jump too. */
    const rebase = vscroll.on && gridBase() !== vscroll.base;
    if (rebase || (rowsMove && (w.r0 > drawn.r1 || w.r1 < drawn.r0)) || (colsMove && (w.c0 > drawn.c1 || w.c1 < drawn.c0))) {   // a jump: nothing to keep, a lean redraw
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
    if (t.sort) groupTag(t, L);               // the group at the top of a sorted view (51-…)
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
    groupTagHide();                           // shown again at the end, if this view has one (51-…)
    /* Drawn at the size the container has now: a redraw the resize observer asked for (15-…) is
       done. Left pending, it ran in the next frame — when a file had just been read, the frame
       meant to show its lean window (the tab bar appearing resizes the grid while the file is
       read): a full redraw there, ~10 ms, then its paint. */
    if (resizeFrame) { cancelAnimationFrame(resizeFrame); resizeFrame = 0; }
    clearTimeout(jumpTimer); jumpTimer = 0;   // a redraw for any reason: nothing left to catch up
    syncLayer();
    tbody.classList.toggle('tx', !!(t && t.lang));   // a text file: lines striped, no rules between them (33-…)
    if (!t || !t.loaded) { tbody.innerHTML = ''; tbody.style.height = '0px'; tbody.classList.remove('no-rows'); totDraw(null); return; }
    const data = t.filteredData;
    if (data.length === 0) { tbody.innerHTML = '<div class="empty-msg">No results found</div>'; tbody.style.height = ''; tbody.classList.add('no-rows'); totDraw(null); syncSpace(t); return; }
    tbody.classList.remove('no-rows');
    pinColWidths(t);                          // every cell is placed by the widths: they come first
    vscrollSize(thead.offsetHeight + data.length * ROW_H + totHeight(), true);   // past the cap or not, before the window is read from the scroll
    const base = vscroll.base = gridBase(), w = drawWindow(t, lean), L = w.L;
    /* Past the cap #tbody is only as tall as the rows from the base need (3 blocks: the viewport
       is within one block of it) — its full height would be cut as the extent was. */
    tbody.style.height = (vscroll.on ? Math.min(data.length * ROW_H - base, 3 * vscroll.block * ROW_H) : data.length * ROW_H) + 'px'; tbody.style.width = L.x[L.x.length - 1] + 'px';
    tbody.innerHTML = rowsHtml(t, w.r0, w.r1, w);
    drawn = { t, n: data.length, r0: w.r0, r1: w.r1, c0: w.c0, c1: w.c1, vis: L.vis.join(',') + '|' + L.F };
    totDraw(t, L, true);                      // the totals row (54-…), part of the extent syncSpace gives
    syncSpace(t);
    if (t.sort) groupTag(t, L);               // the group at the top of a sorted view (51-…)
}

/* A redraw the user waits on — a file just read, a tab switched to, a filter typed: the lean
   window first (the viewport and a quarter of a screen around it, as a scroll jump draws), then,
   after that frame, the rest grown the way a scroll step grows it (renderOnScroll). A full
   window is ~4 times the cells of a lean one, and their layout is most of a redraw. */
let growFrame = 0, resizeFrame = 0;   // resizeFrame: a redraw the resize observer asked for (15-…)
function renderFirst() {
    render(true);
    cancelAnimationFrame(growFrame);
    growFrame = requestAnimationFrame(() => setTimeout(() => renderOnScroll(), 0));
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
    if (t.lang) { t.colWidths[0] = textColWidth(t); applyColStyles(true); return; }   // a text file: one wide column (33-…)
    const [v0] = viewRows(t), rows = t.filteredData.slice(v0, v0 + 80);
    missing.forEach(i => {
        let w = cells && cells[i + 1] ? cells[i + 1].getBoundingClientRect().width : 0;
        const ex = t.lang ? 0 : lookExtra(t, i);   // a pill's padding (51-…)
        for (const r of rows) w = Math.max(w, textWidth(cellStr(r.data[i])) + 21 + ex);   // padding 10 + 10, border 1
        t.colWidths[i] = Math.min(Math.max(Math.ceil(w), 60), 480);
    });
    applyColStyles(true);                 // render() calls syncSpace once the rows are in
}

/* Fit columns to their content — a double-click on a title's resize handle
   (one column), or the palette (every visible one): the widest value of the
   rows shown, every row up to FIT_ALL rows (shared between the columns when
   several are fitted, 20 000 each at least: 20 columns × 200 000 took 2.4 s),
   an even sample past that, read with cellOf (no split); the title's own
   width as the floor. Wider than the 480 px an automatic width stops at,
   since it is asked for: FIT_MAX. */
const FIT_ALL = 200000, FIT_MAX = 900;
function fitColumns(cols) {
    const t = T(); if (!t || !t.loaded) return;
    const rows = t.filteredData, n = rows.length, step = Math.max(1, Math.ceil(n / Math.max(20000, FIT_ALL / cols.length)));
    for (const c of cols) {
        let w = titleWidth(c);
        for (let i = 0; i < n; i += step) w = Math.max(w, textWidth(cellStr(cellOf(rows[i], c))) + 21);
        t.colWidths[c] = Math.min(Math.max(Math.ceil(w), 60), FIT_MAX);
    }
    applyColStyles(); render();
    setStats(`${t.name} | ${cols.length === 1 ? `Column "${t.headers[cols[0]]}"` : `${fmt(cols.length)} columns`} fitted to ${step > 1 ? 'a sample of ' : ''}the rows shown.`);
}
/* A title's natural width: its text in the header's font, plus the type icon, the number, the ▾ and the padding. */
function titleWidth(c) {
    const th = thead.rows[0] && thead.rows[0].cells[c + 1], span = th && th.querySelector('.col-name');
    if (!span) return 60;
    const st = getComputedStyle(span), ctx = document.createElement('canvas').getContext('2d');
    ctx.font = `${st.fontStyle} ${st.fontWeight} ${st.fontSize} ${st.fontFamily}`;
    let w = 0; for (const el of span.childNodes) w += el.nodeType === 3 || el.classList.contains('cn-t') ? ctx.measureText(el.textContent).width : el.getBoundingClientRect().width + 6;   // the text whole, not as its ellipsis shows it
    return Math.ceil(w) + 26 + 22;                                     // .col-title's padding-right (the ▾), the th's padding and border
}
function resetColWidths() {
    const t = T(); if (!t || !t.loaded) return;
    t.colWidths = {}; applyColStyles(); render();                     // pinColWidths measures them again at this render
    setStats(`${t.name} | Column widths reset.`);
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
    const y = container.vTop - vscroll.base;   // the rows are placed from the base (vscroll, 01-…), 0 under the cap
    if (gridLayer.scrollTop !== y) gridLayer.scrollTop = y;
    if (gridLayer.scrollLeft !== container.scrollLeft) gridLayer.scrollLeft = container.scrollLeft;
    stripFollow();                        // the scroll strip's thumb (28-…)
}
/* The scroll extent for a grid h px tall (vscroll, 01-…): the spacer gives the container that
   extent — or, past the cap, the cap's, the grid mapped onto it. The container's last scroll
   position is the layer (ch, less padB) + the spacer − ch, for the grid's extent as for the
   cap's. lazy (render(), before the window is read): only when past the cap, or just back
   under it — syncSpace() sizes the spacer at the end of every render. */
function vscrollSize(h, lazy) {
    const V = vscroll, g = V.geo || { ch: container.clientHeight, padB: 0 }, ext = h + g.padB;
    const cap = V.cap || Math.floor(2 ** 24 / Math.max(1, window.devicePixelRatio || 1));
    if (ext <= cap) {
        if (lazy && !V.on) return;
        document.getElementById('scroll-space').style.height = Math.max(0, ext - g.ch) + 'px';
        if (V.on) { V.on = false; V.base = 0; container.scrollTop = V.v; }
        return;
    }
    const vmax = ext - g.ch - g.padB, smax = cap - g.ch - g.padB;
    /* The scrollbar is put back only when the extent changed: set during a smooth wheel scroll,
       scrollTop stops its animation — a redraw (a jump, a new base) must not. */
    if (V.on && vmax === V.vmax && smax === V.smax) return;
    if (!V.on) { V.on = true; V.v = container.scrollTop; }
    V.vmax = vmax; V.smax = smax;
    document.getElementById('scroll-space').style.height = (cap - g.ch) + 'px';
    V.v = Math.min(V.v, V.vmax);
    vscrollAnchor(false);
}
/* Past the cap, the rows are placed in #tbody from the start of the block of vscroll.block rows
   (an even count: the stripes of #tbody's background stay in step) holding the scroll position. */
function gridBase() {
    if (!vscroll.on) return 0;
    const b = vscroll.block * ROW_H;
    return Math.floor(vscroll.v / b) * b;
}
/* The scrollbar where the grid's position puts it. kick: when it does not move (a step smaller
   than one of its pixels), no scroll event comes — the window is drawn anyway. */
function vscrollAnchor(kick) {
    const V = vscroll, s = V.vmax > 0 ? Math.round(V.v * V.smax / V.vmax) : 0;
    V.s = s;
    if (Math.abs(container.scrollTop - s) >= 1) { V.mute = s; container.scrollTop = s; }
    else if (kick) requestAnimationFrame(() => container.onscroll());
}
/* A scroll event past the cap: the scrollbar moved by us (vscrollAnchor) changes nothing; a
   step up to two screens (wheel, keys, a click on the track) moves the grid as much; a longer
   one is the thumb dragged or Home / End, which land at the same share of the grid. The drift
   that 1:1 steps leave between the two is taken back once the scroll ends (below). */
function vscrollFollow() {
    const V = vscroll; if (!V.on) return;
    const s = container.scrollTop;
    if (V.mute !== null && Math.abs(s - V.mute) < 1) { V.mute = null; V.s = s; return; }
    V.mute = null;
    const d = s - V.s; V.s = s;
    if (!d) return;
    V.v = Math.abs(d) <= 2 * container.clientHeight ? Math.min(Math.max(0, V.v + d), V.vmax) : s / V.smax * V.vmax;
}
container.addEventListener('scrollend', () => { if (vscroll.on) vscrollAnchor(false); });
/* The scrollbar at its end while the grid is not (a long wheel scroll, drift not yet taken
   back): the wheel moves the grid itself. Passive — a blocking wheel listener on the scroller
   would put every scroll through the main thread. */
container.addEventListener('wheel', e => {
    const V = vscroll; if (!V.on || !e.deltaY) return;
    const s = container.scrollTop;
    if (e.deltaY > 0 ? s < V.smax - 1 || V.v >= V.vmax : s > 0 || V.v <= 0) return;
    container.vTop = V.v + (e.deltaMode === 1 ? e.deltaY * ROW_H : e.deltaMode === 2 ? e.deltaY * container.clientHeight : e.deltaY);
}, { passive: true });
/* The layer the size of the viewport, the spacer the rest of the table's extent
   (rows × ROW_H + the header + the totals row, and the pinned widths' sum — measured
   only while widths are still unknown). */
function syncSpace(t) {
    const gridLayer = document.getElementById('grid-layer'), scrollSpace = document.getElementById('scroll-space');
    /* No native scrollbar of the strip's width there (none shown: every row fits;
       or overlay scrollbars of no width, as on macOS): the strip takes 24 px at
       the edge (24 = STRIP_W, a const of 28-… not yet declared when this runs at
       boot) and the grid layer is that much narrower — so the scroll extent is
       that much longer, or the last 24 px of the grid could never be scrolled
       into view (it hid the end of the last column). Same below for the
       horizontal band. container._vw / _vh: the grid's visible size (revealCell). */
    const cw = container.clientWidth, ch = container.clientHeight, sbw = container.offsetWidth - cw;
    const padR = sbw >= 16 ? 0 : 24 - sbw, vw = cw - padR;
    gridLayer.style.width = vw + 'px'; gridLayer.style.height = ch + 'px';
    container._vw = vw; container._vh = ch;
    if (!t || !t.loaded) { scrollSpace.style.height = '0px'; scrollSpace.style.width = '1px'; vscroll.on = false; vscroll.base = 0; stripLayout(null); return; }
    const L = colLayout(t), w = L ? L.x[L.x.length - 1] : document.getElementById('mainTable').offsetWidth;
    const nohs = !!L && w <= vw;              // no sideways scroll: the row numbers need not stick (app.css)
    if (gridLayer.classList.contains('nohs') !== nohs) gridLayer.classList.toggle('nohs', nohs);
    const sbh = container.offsetHeight - ch, padB = !nohs && sbh < 16 ? 24 - sbh : 0;
    if (padB) { gridLayer.style.height = (ch - padB) + 'px'; container._vh = ch - padB; }
    vscroll.geo = { ch, padB };
    vscrollSize(thead.offsetHeight + t.filteredData.length * ROW_H + totHeight());
    scrollSpace.style.width = Math.max(1, nohs ? w : w + padR) + 'px';
    syncLayer();
    stripLayout(t);
}
