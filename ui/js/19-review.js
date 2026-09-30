/* ---------------------------------------------------------------
   REVIEW CHANGES
   What Save would write, against what the file holds now. Every row
   remembers its line in the file (r.src, set at read and after each
   save) and every column its index there (t.colSrc, kept by
   remapColRefs): the review re-reads the file and compares, so it needs
   no cooperation from the many kinds of edit. A row without a line is
   new; a line no row claims was deleted.
----------------------------------------------------------------*/

/* After a save the file is exactly the rows: new origins. A row that writes
   as an empty line (one column, empty) is skipped when read back, so it has
   no line of its own. */
function markPristine(t) {
    let line = t.syntheticHeader ? 0 : 1;
    for (const r of t.allData) r.src = r.len <= 1 && cellStr(r.data[0]) === '' ? null : line++;
    t.headerSrc = t.syntheticHeader ? null : 0;
    t.colSrc = t.headers.map((_, i) => i);
    t.redoStack = [];                          // redoing past a save would restore a colSrc from before it
    if (t.rowMark && t.rowMark.kind === 'review') t.rowMark = null;   // those changes are the file now
}

/* The file's records as read now, with the same delimiter and encoding
   (readDiskRecords: .length and .get(i), decoded on demand). */
async function computeChanges(t) {
    const lines = await readDiskRecords(t);
    const cs = t.colSrc || t.headers.map((_, i) => i);
    const dh = t.headerSrc != null && t.headerSrc < lines.length ? lines.get(t.headerSrc) : t.headerSrc != null ? [] : null;
    let width = dh ? dh.length : 0;
    if (!dh) for (let i = 0; i < lines.length; i++) { const l = lines.get(i).length; if (l > width) width = l; }
    const kept = new Set(cs.filter(s => s >= 0));
    const oldName = o => dh ? cellStr(dh[o]) : String(o);
    const cols = {
        added: t.headers.filter((_, c) => cs[c] < 0),
        deleted: Array.from({ length: width }, (_, o) => o).filter(o => !kept.has(o)).map(oldName),
        renamed: dh && !t.syntheticHeader ? t.headers.map((h, c) => cs[c] >= 0 && oldName(cs[c]) !== h ? [oldName(cs[c]), h] : null).filter(Boolean) : [],
        moved: cs.filter(s => s >= 0).some((s, i, a) => i && s < a[i - 1])
    };
    const seen = new Uint8Array(lines.length), changed = [], added = [];
    let cells = 0, last = -1, reordered = false;
    const diffs = new Array(t.allData.length);   // worked out in file order, gathered in the tab's
    visitRows(t, t.allData, (r, i) => {
        if (r.src == null || r.src >= lines.length) return;
        const d = lines.get(r.src), rd = r.data;
        let diff = null;
        for (let c = 0; c < cs.length; c++) {
            if (cs[c] < 0) continue;
            const before = cellStr(d[cs[c]]);
            if (cellStr(rd[c]) !== before) (diff = diff || []).push([c, before]);
        }
        diffs[i] = diff;
    });
    t.allData.forEach((r, i) => {
        if (r.src == null || r.src >= lines.length || seen[r.src]) { added.push(r); return; }
        seen[r.src] = 1;
        if (r.src < last) reordered = true;
        last = r.src;
        const diff = diffs[i];
        if (diff) { changed.push({ r, cells: diff }); cells += diff.length; }
    });
    const deleted = [];
    for (let i = 0; i < lines.length; i++) if (!seen[i] && i !== t.headerSrc) deleted.push({ line: i + 1, data: lines.get(i) });
    return { cols, changed, cells, added, deleted, reordered };
}

let review = null;       // { t, res }
const REVIEW_MAX = 500;

async function openReview() {
    const t = T(); if (!t || !t.loaded) return;
    if (!t.path && !t.handle && !t.file) return;
    setStats(`${t.name} | Comparing with the file on disk…`);
    let res;
    try { res = await computeChanges(t); }
    catch (e) { uiAlert(`Cannot compare with the file on disk.\n\n${e.message || e}`); updateStats(); return; }
    if (T() !== t) return;
    review = { t, res };
    renderReview();
    updateStats();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-review').style.display = 'block';
}

function renderReview() {
    const { t, res } = review, { cols } = res;
    const n = (k, w) => `<b>${fmt(k)}</b> ${w}${k === 1 ? '' : 's'}`;
    const parts = [];
    if (res.cells) parts.push(`${n(res.cells, 'cell')} changed in ${n(res.changed.length, 'row')}`);
    if (res.added.length) parts.push(`${n(res.added.length, 'row')} added`);
    if (res.deleted.length) parts.push(`${n(res.deleted.length, 'row')} deleted`);
    if (res.reordered) parts.push('rows reordered');
    if (cols.added.length) parts.push(`${n(cols.added.length, 'column')} added`);
    if (cols.deleted.length) parts.push(`${n(cols.deleted.length, 'column')} deleted`);
    if (cols.renamed.length) parts.push(`${n(cols.renamed.length, 'column')} renamed`);
    if (cols.moved) parts.push('columns moved');
    const edits = t.modificationsLog.length;
    document.getElementById('rv-sum').innerHTML = (parts.length ? parts.join(' · ') : 'The rows are the same as in the file.')
        + `<br><span class="muted">${fmt(edits)} edit${edits === 1 ? '' : 's'} pending — compared with ${esc(t.name)} as it is on disk now.</span>`;

    const cut = v => { const s = cellStr(v); return s === '' ? '<span class="empty">empty</span>' : esc(s.length > 80 ? s.slice(0, 80) + '…' : s); };
    const more = (k, shown) => k > shown ? `<tr><td colspan="4" class="muted">…and ${fmt(k - shown)} more</td></tr>` : '';
    let html = '';
    if (cols.added.length || cols.deleted.length || cols.renamed.length) {
        html += '<tr class="sec"><th colspan="4">Columns</th></tr>';
        cols.added.forEach(h => html += `<tr><td class="k add">added</td><td colspan="3">${esc(h)}</td></tr>`);
        cols.deleted.forEach(h => html += `<tr><td class="k del">deleted</td><td colspan="3">${esc(h)}</td></tr>`);
        cols.renamed.forEach(([a, b]) => html += `<tr><td class="k chg">renamed</td><td colspan="3"><s>${esc(a)}</s> → ${esc(b)}</td></tr>`);
    }
    if (res.changed.length) {
        html += '<tr class="sec"><th>Row</th><th>Column</th><th>Before</th><th>After</th></tr>';
        let shown = 0;
        outer: for (const { r, cells } of res.changed) for (const [c, before] of cells) {
            if (shown++ >= REVIEW_MAX) break outer;
            html += `<tr class="go" onclick="reviewGo(${r.id}, ${c})" title="Show this cell"><td class="k">${fmt(r.id)}</td><td>${esc(t.headers[c])}</td><td class="old">${cut(before)}</td><td class="new">${cut(r.data[c])}</td></tr>`;
        }
        html += more(res.cells, Math.min(res.cells, REVIEW_MAX));
    }
    const rowPv = d => d.slice(0, 4).map(cut).join(' <span class="muted">·</span> ');
    if (res.added.length) {
        html += '<tr class="sec"><th colspan="4">Rows added</th></tr>';
        res.added.slice(0, 100).forEach(r => html += `<tr class="go" onclick="reviewGo(${r.id}, 0)" title="Show this row"><td class="k add">row ${fmt(r.id)}</td><td colspan="3">${rowPv(r.data)}</td></tr>`);
        html += more(res.added.length, Math.min(res.added.length, 100));
    }
    if (res.deleted.length) {
        html += '<tr class="sec"><th colspan="4">Rows deleted</th></tr>';
        res.deleted.slice(0, 100).forEach(d => html += `<tr><td class="k del">line ${fmt(d.line)}</td><td colspan="3" class="old">${rowPv(d.data)}</td></tr>`);
        html += more(res.deleted.length, Math.min(res.deleted.length, 100));
    }
    if (edits) {
        html += '<tr class="sec"><th colspan="4">Edits, most recent first</th></tr>';
        t.modificationsLog.slice(-100).reverse().forEach(l => html += `<tr><td colspan="4">${esc(l.what || `${l.col}: ${l.old} → ${l.new}`)}</td></tr>`);
        html += more(edits, Math.min(edits, 100));
    }
    document.getElementById('rv-list').innerHTML = html;
    document.getElementById('rv-mark').disabled = !res.changed.length && !res.added.length;
}

/* Changed and added rows as marks: the changed cells tinted, their former
   value in the tooltip, and a chip to show only those rows. */
function reviewMark() {
    const { t, res } = review;
    if (T() !== t) return;
    const cells = new Map(), rows = new Set(res.added);
    for (const { r, cells: cs } of res.changed) { rows.add(r); cells.set(r, new Map(cs.map(([c, before]) => [t.headers[c], before]))); }
    setRowMark(t, { kind: 'review', label: 'changed rows', tip: 'Before', rows, cells, only: true });
    closeAllModals();
}
function reviewGo(id, c) {
    const t = review && review.t; if (!t || T() !== t) return;
    closeAllModals();
    let i = t.filteredData.findIndex(r => r.id === id);
    if (i < 0) { reviewMark(); i = t.filteredData.findIndex(r => r.id === id); }
    if (i < 0) return;
    if (t.hiddenCols.has(c)) { t.hiddenCols.delete(c); applyColStyles(); }
    setSel(t, i, c, i, c); revealCell(i, c);
}

/* ---- ROW MARKS --------------------------------------------------
   Shared by Review changes and Compare: t.rowMark = { kind, label, tip,
   rows: Set, cells: Map(row → Map(column name → other value)), only }.
   Cells are keyed by column name so that a column moved since still
   finds its marks. `only` is a filter like onlyIrregular. */
function setRowMark(t, m) {
    t.rowMark = m;
    renderHeader(); applyColStyles(); applyFilters();
}
function markedCells(t, r) { return t.rowMark ? t.rowMark.cells.get(r) : undefined; }
function markTitle(t, v) { return ` title="${esc(`${t.rowMark.tip}: ${cellStr(v) === '' ? '(empty)' : cellStr(v)}`)}"`; }
function toggleMarkView() {
    const t = T(); if (!t || !t.rowMark) return;
    t.rowMark.only = !t.rowMark.only;
    applyFilters();
}
function clearRowMark(e) {
    if (e) e.stopPropagation();
    const t = T(); if (!t) return;
    t.rowMark = null;
    applyFilters();
}
function updateMarkChip(t) {
    const chip = document.getElementById('mark-chip'), m = t && t.loaded && t.rowMark;
    chip.style.display = m ? '' : 'none';
    if (!m) return;
    chip.classList.toggle('on', !!m.only);
    chip.innerHTML = `◆ ${fmt(m.rows.size)} ${esc(m.label)}<span class="chip-x" onclick="clearRowMark(event)" title="Remove the marks">×</span>`;
    chip.title = (m.only ? 'Showing only the marked rows — click to show all rows. ' : 'Click to show only the marked rows. ')
        + `Tinted cells hold a different value; hover one to see the other (${m.tip.toLowerCase()}).`;
}
