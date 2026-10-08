/* ---------------------------------------------------------------
   READING THE GRID: category pills, group lines in a sorted view,
   the active row
   - A column of few values (a status, a country, a source…) shows
     each as a pill in its column chart's colour: the five most
     frequent in the ribbon's colours, in its order (--sg0…--sg4),
     the others grey — drawn as the cell's background. From the
     chart's own counts when it is shown (t.spk), else from the
     first rows.
   - (Numbers aligned on their decimal mark, padded with invisible
     zeros, came with them and went at the user's word, 2026-10-09.)
   - Sorted, a thin line parts two rows whose sort value differs, and
     a tag under the header says the group at the top of the view,
     over the sorted column — when the sort column has groups (fewer
     changes than half the rows in its first ones; decided when idle).
   - The active cell's row, tinted.
   Everything is decided once per state (gridLook(), cached on the
   headers and the charts): a cell drawn costs one array read for a
   column that has none of it.
----------------------------------------------------------------*/
const PILL_MAX = 12;              // distinct values at most for a column of pills
const PILL_LEN = 24;              // …each this long at most on average
const LOOK_ROWS = 400;            // rows read for the pills without the charts — columnKinds()' own, already decoded
const GROUP_ROWS = 400;           // rows of a sorted view read to tell whether it has groups

/* Per column: pills (Map value → class); cached on the headers, the bytes
   and the charts — not on the rows' stamp: a sort or an edit read 2 000 rows again (a sort +50 %
   on the bench's small file); the charts' next pass after an edit brings a new value's pill. */
function gridLook(t) {
    /* Keyed on the charts' object, not spkShown(): that joins every title, and this runs at every
       scroll step (colLayout). */
    const key = [t.headers, t.headers.length, t.base, t.spk];
    if (t.look && sameStamp(t.look.key, key)) return t.look;
    const s = spkShown(t);
    const kinds = columnKinds(t), cat = new Map();
    const rows = t.allData.length > LOOK_ROWS ? t.allData.slice(0, LOOK_ROWS) : t.allData;
    let split = null;                         // the rows' cells, read once for every column that needs them
    const cells = () => split || (split = rows.map(r => r.data));
    t.headers.forEach((_, c) => {
        if (kinds[c] === 't') {
            let segs = null, distinct = 0;
            const sc = s && s.cols[c];
            if (sc && sc.k === 't') { segs = sc.segs.map(x => x[0]); distinct = sc.many ? Infinity : sc.distinct; }
            else if (!s && !spkOn()) {                // charts on but not counted yet (an opening): their pass brings the pills (lookRefresh)
                const m = new Map();
                for (const d of cells()) { const v = cellStr(d[c]).trim(); if (v) m.set(v, (m.get(v) || 0) + 1); if (m.size > PILL_MAX) break; }
                distinct = m.size;
                segs = [...m].sort((a, b) => b[1] - a[1]).filter(x => x[1] >= 2).slice(0, SPK_SEGS).map(x => x[0]);
            }
            if (segs && segs.length >= 2 && distinct <= PILL_MAX && segs.reduce((a, v) => a + v.length, 0) / segs.length <= PILL_LEN)
                cat.set(c, new Map(segs.map((v, i) => [v, 'p' + i])));
        }
    });
    t.look = { key, cat, sig: [...cat].map(([c, m]) => c + ':' + [...m.keys()].join('\u0001')).join('\u0002') };
    return t.look;
}
/* The charts just changed their counts (47-…): the pills follow if their values or colours did. */
function lookRefresh(t) {
    const old = t.look && t.look.sig;
    if (T() !== t || !t.loaded || gridLook(t).sig === old || !drawn || drawn.t !== t) return;
    redrawRows(t, drawn.r0, drawn.r1);
}
/* A pill is the cell's own background, not an element: a rectangle and two round caps (gradients,
   .cell.pc in app.css) as wide as the text, given as --pw. A span per cell cost a redraw ~3 % (its
   parse, style and layout box — its font, colour or corners made no difference, measured); a
   background costs nothing to lay out. The text's width in the pills' font, kept per value: a
   column of pills repeats a dozen values. */
const pillW = new Map();
let pillCtx = null;
function pillWidth(v) {
    let w = pillW.get(v);
    if (w != null) return w;
    if (!pillCtx) {
        const probe = document.createElement('div'); probe.className = 'cell pc'; tbody.appendChild(probe);
        const st = getComputedStyle(probe); pillCtx = document.createElement('canvas').getContext('2d');
        pillCtx.font = `${st.fontStyle} ${st.fontWeight} ${st.fontSize} ${st.fontFamily}`;
        probe.remove();
    }
    w = Math.ceil(pillCtx.measureText(v).width);
    if (pillW.size > 5000) pillW.clear();
    pillW.set(v, w);
    return w;
}
/* A pill cell's class and style (cellHtml, 12-…): '' when the cell is empty. */
function pillAttrs(pill, c) {
    if (c == null || c === '') return null;
    const v = typeof c === 'string' ? c.trim() : String(c);
    return v ? { cls: ' pc ' + (pill.get(v) || 'po'), w: pillWidth(v) } : null;
}
/* The room a pill column's cells need beyond their text (pinColWidths, 12-…). */
function lookExtra(t, c) { return gridLook(t).cat.has(c) ? 4 : 0; }   // the pill's 7 px each side, less the cell's padding left over

/* ---- Groups of a sorted view ---- */
/* The first sort key's column, when it holds groups worth a line: among its first LOOK_ROWS rows
   shown, fewer changes of value than half the rows. Cached on the view. */
function groupCol(t) {
    if (!t.sort || !t.sort.length || t.lang) return -1;
    const c = t.sort[0].col, g = t.groupLook;
    if (g && g.rows === t.filteredData && g.n === t.filteredData.length && g.c === c && sameStamp(g.stamp, dataStamp(t))) return g.on ? c : -1;
    /* Decided in idle time, the lines drawn then: inside the sort it cost ~4 ms of reads in
       the sorted order (each record decoded alone). */
    if (!groupCol.wait) groupCol.wait = requestIdleCallback(() => {
        groupCol.wait = 0;
        if (T() !== t || !t.loaded || !t.sort) return;
        const c = t.sort[0].col, rows = t.filteredData, n = Math.min(rows.length, GROUP_ROWS);
        let changes = 0, prev = null;
        for (let i = 0; i < n; i++) { const v = cellStr(cellOf(rows[i], c)).trim(); if (i && v !== prev) changes++; prev = v; }
        t.groupLook = { rows, n: rows.length, c, stamp: dataStamp(t), on: n > 3 && changes < n / 2 };
        if (t.groupLook.on && drawn && drawn.t === t) { redrawRows(t, drawn.r0, drawn.r1); groupTag(t); }
    }, { timeout: 500 });
    return -1;
}
/* Row i, whose cells rowsHtml (12-…) has split already (d), starts a group. */
function groupStart(t, c, i, d) {
    return i > 0 && cellStr(d[c]).trim() !== cellStr(cellOf(t.filteredData[i - 1], c)).trim();
}
/* The tag under the header: the group of the row at the top of the view, over its column. Placed
   from the widths and the container's box (measured by render(), not at every scroll). */
var groupBox = null;                      // the container's box; var: resizeContainer() (04-…) clears it from the boot run
function groupTag(t, L) {
    let el = document.getElementById('grp-tag');
    const c = t && t.loaded ? groupCol(t) : -1;
    if (c < 0 || !t.filteredData.length || t.hiddenCols.has(c)) { if (el && el.style.display !== 'none') el.style.display = 'none'; return; }
    if (!el) { el = document.createElement('div'); el.id = 'grp-tag'; document.body.appendChild(el); }
    if (!groupBox) groupBox = container.getBoundingClientRect();
    L = L || colLayout(t); if (!L) return;
    const k = L.vis.indexOf(c), th = thead.offsetHeight;
    const top = Math.max(0, Math.min(t.filteredData.length - 1, Math.floor((container.scrollTop - th) / ROW_H) + 1));
    const v = cellStr(cellOf(t.filteredData[top], c)).trim();
    const x = k < L.F ? L.x[k] : L.x[k] - container.scrollLeft, w = L.x[k + 1] - L.x[k];
    const left = groupBox.left + Math.max(idxColW, x) + 6, right = groupBox.left + x + w - 6;
    if (container.scrollTop < ROW_H || right - left < 40 || right < groupBox.left + idxColW) { if (el.style.display !== 'none') el.style.display = 'none'; return; }
    const pos = `${groupBox.top + th + 4}|${left}|${right - left}`;   // written only when it moved: a scroll step changes nothing there
    if (el.style.display === 'none') el.style.display = '';
    if (el._pos !== pos) { el._pos = pos; el.style.top = (groupBox.top + th + 4) + 'px'; el.style.left = left + 'px'; el.style.maxWidth = (right - left) + 'px'; }
    if (el._v !== v) { el._v = v; el.textContent = v === '' ? '(empty)' : v; el.classList.remove('flip'); void el.offsetWidth; el.classList.add('flip'); }
}

function groupTagHide() { const el = document.getElementById('grp-tag'); if (el && el.style.display !== 'none') el.style.display = 'none'; }

/* ---- The active row ---- */
/* The active cell's row, tinted on the rows drawn (paintSel, 06-…); rows drawn later get it from
   rowsHtml. Its column was tinted too at first: the user did not like it (2026-10-08). */
function paintCross(t) {
    for (const el of tbody.querySelectorAll('.row.xr')) el.classList.remove('xr');
    if (!sel || sel.tab !== t.id) return;
    const row = tbody.querySelector(`.row[data-idx="${sel.fr}"]`);
    if (row) row.classList.add('xr');
}
