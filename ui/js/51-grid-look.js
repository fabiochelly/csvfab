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
   - Rich values (richCols): a column of links or e-mail addresses
     (Ctrl+click opens or writes), of yes / no (a check, drawn as the
     cell's background), of #colours (a swatch), of JSON (the peek,
     56-…, indents it) — each with its own icon in the title.
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
    const key = [t.headers, t.headers.length, t.base, t.spk, t.rich && t.rich.map];
    if (t.look && sameStamp(t.look.key, key)) return t.look;
    const s = spkShown(t);
    const kinds = columnKinds(t), cat = new Map();
    const rows = t.allData.length > LOOK_ROWS ? t.allData.slice(0, LOOK_ROWS) : t.allData;
    let split = null;                         // the rows' cells, read once for every column that needs them
    const cells = () => split || (split = rows.map(r => r.data));
    const rich = richCols(t);
    t.headers.forEach((_, c) => {
        if (kinds[c] === 't' && !rich.has(c)) {   // a column of yes / no is checks, not pills
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
function lookExtra(t, c) {
    if (gridLook(t).cat.has(c)) return 4;     // the pill's 7 px each side, less the cell's padding left over
    const r = richCols(t).get(c);
    return r && r.k === 'color' ? 20 : 0;     // the swatch before the text
}

/* ---- Rich values ---- */
/* A text column whose filled values (first LOOK_ROWS rows) are at RICH_MIN or more of one kind. Cached
   on the headers and the bytes, as the pills; Map col → {k: 'url' | 'email' | 'bool' | 'color' | 'json',
   yes: the true word of a yes / no column}. */
const RICH_MIN = .9;
const RICH_RE = { url: /^https?:\/\/[^\s<>"'`]+$/i, email: /^[^\s@<>"'`()]+@[^\s@<>"'`()]+\.[a-z]{2,}$/i, color: /^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i };
const RICH_BOOL = [['true', 'false'], ['yes', 'no'], ['oui', 'non'], ['vrai', 'faux']];
const RICH_ICON = { url: ['↗', 'Links · Ctrl+click opens one'], email: ['@', 'E-mail addresses · Ctrl+click writes to one'], bool: ['✓', 'Yes / no'], color: ['◐', 'Colours'], json: ['{}', 'JSON · rest on a cell to read it whole'] };
/* Read in idle time, not in the frame of an opening (400 rows, every text cell tested: the small file's cold
   opening +9.6 %, measured): until then no column is rich, then the grid is drawn again — and a colour column
   gets its swatch's room. */
const RICH_NONE = new Map();
function richCols(t) {
    const key = [t.headers, t.headers.length, t.base];
    if (t.rich && sameStamp(t.rich.key, key)) return t.rich.map;
    if (!t.richWait && t.loaded && !t.lang) t.richWait = requestIdleCallback(function tick() {
        if (spkBusy()) { t.richWait = requestIdleCallback(tick, { timeout: 1000 }); return; }   // a read or a filtering awaited: later
        t.richWait = 0;
        if (!t.loaded) return;
        const map = richDetect(t);
        t.rich = { key: [t.headers, t.headers.length, t.base], map };
        if (!map.size) return;
        for (const [c, r] of map) if (r.k === 'color' && t.colWidths[c] != null) t.colWidths[c] += 20;
        if (T() === t) { renderHeader(); applyColStyles(); render(); }
    }, { timeout: 1000 });
    return RICH_NONE;
}
function richDetect(t) {
    const map = new Map();
    if (!t.loaded || t.lang) return map;
    const kinds = columnKinds(t), rows = (t.allData.length > LOOK_ROWS ? t.allData.slice(0, LOOK_ROWS) : t.allData).map(r => r.data);
    t.headers.forEach((_, c) => {
        if (kinds[c] !== 't') return;
        let n = 0, url = 0, mail = 0, col = 0, json = 0, parsed = 0;
        const words = new Set();
        for (const d of rows) {
            const s = cellStr(d[c]).trim(); if (!s) continue;
            n++;
            if (words.size <= 2) words.add(s.toLowerCase());
            const ch = s[0];
            if (ch === 'h' || ch === 'H') { if (RICH_RE.url.test(s)) url++; }
            else if (ch === '#') { if (RICH_RE.color.test(s)) col++; }
            else if (ch === '{' || ch === '[') { if (parsed++ < 40) { try { if (typeof JSON.parse(s) === 'object') json++; } catch (e) { } } }
            if (s.indexOf('@') > 0 && RICH_RE.email.test(s)) mail++;
        }
        if (n < 3) return;
        const pair = words.size <= 2 && RICH_BOOL.find(p => [...words].every(w => p.includes(w)));
        if (pair) map.set(c, { k: 'bool', yes: pair[0], no: pair[1], seen: new Map() });
        else if (url >= n * RICH_MIN) map.set(c, { k: 'url' });
        else if (mail >= n * RICH_MIN) map.set(c, { k: 'email' });
        else if (col >= n * RICH_MIN) map.set(c, { k: 'color' });
        else if (json >= Math.min(n, 40) * RICH_MIN) map.set(c, { k: 'json' });
    });
    return map;
}
/* The icon in the title, as wide as the text one ("Aa", hidden under it): a narrower icon made the column a few px
   narrower and shifted every column after it — enough to change how the bench's sideways scroll steps fall on
   column edges (+4-5 %, found the hard way). */
function richIcon(r) { const [g, tip] = RICH_ICON[r.k]; return `<span class="ty ty-r" title="${tip}"><b>${g}</b><i>Aa</i></span>`; }
/* A rich cell's class, style and content (cellHtml, 12-…): null when the value is not of the column's kind.
   Every answer has the same shape {cls, html, style, ov} (null / '' when unused): answers of mixed shapes
   made cellHtml's reads of them polymorphic, and a sideways scroll of the bench's wide file 4-5 % slower
   (10 rich columns of 100; invisible in a page already warm — the JIT had seen every shape). Constant
   answers where possible, the value trimmed only where it is read. */
const richA = (cls, html, style, ov) => ({ cls, html, style, ov });
const RA_LK = richA(' lk', null, '', null), RA_JS = richA(' js', null, '', null), RA_ON = richA(' cbx on', '', '', null), RA_OFF = richA(' cbx', '', '', null);
function richAttrs(r, c, w) {
    if (c == null || c === '') return null;
    const k = r.k;
    if (k === 'url' || k === 'email') return RA_LK;   // checked at the click, not at every cell drawn
    if (k === 'json') return RA_JS;
    if (k === 'bool') {                       // a few values, repeated: the answer kept per raw value (no trim nor lower case per cell)
        let a = r.seen.get(c);
        if (a === undefined) { const v = String(c).trim().toLowerCase(); a = v === r.yes ? RA_ON : v === r.no ? RA_OFF : null; if (r.seen.size < 64) r.seen.set(c, a); }
        return a;
    }
    const s = (typeof c === 'string' ? c : String(c)).trim();
    if (k === 'color') return RICH_RE.color.test(s) ? richA(' sw', null, `;--sw:${s}`, textWidth(s) + 20 > w - 21) : null;   // checked: the value goes into a style
    return null;
}
/* Ctrl+click (Cmd on a Mac) on a link opens it in the browser, on an address writes to it. A mailto:
   is not a page leaving: the prompt about unsaved edits must not come (quitting, 04-…). */
document.addEventListener('keydown', e => { if (e.key === 'Control' || e.key === 'Meta') document.body.classList.add('ctrl'); });
document.addEventListener('keyup', e => { if (e.key === 'Control' || e.key === 'Meta') document.body.classList.remove('ctrl'); });
window.addEventListener('blur', () => document.body.classList.remove('ctrl'));
tbody.addEventListener('click', e => {
    if (!(e.ctrlKey || e.metaKey)) return;
    const cell = e.target.closest('.cell.lk'), row = cell && cell.closest('.row[data-idx]'), t = T();
    if (!row || !t) return;
    const c = +cell.dataset.c, r = t.filteredData[+row.dataset.idx], k = r && richCols(t).get(c);
    if (!k) return;
    const s = cellStr(cellOf(r, c)).trim();
    e.preventDefault();
    if (k.k === 'url' && RICH_RE.url.test(s)) { window.open(s, '_blank', 'noopener,noreferrer'); setStats(`Opened ${s}`); }
    else if (k.k === 'email' && RICH_RE.email.test(s)) {
        const was = quitting; quitting = true;
        location.href = 'mailto:' + s;
        setTimeout(() => { quitting = was; }, 1500);
    }
});

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
    const top = Math.max(0, Math.min(t.filteredData.length - 1, Math.floor((container.vTop - th) / ROW_H) + 1));
    const v = cellStr(cellOf(t.filteredData[top], c)).trim();
    const x = k < L.F ? L.x[k] : L.x[k] - container.scrollLeft, w = L.x[k + 1] - L.x[k];
    const left = groupBox.left + Math.max(idxColW, x) + 6, right = groupBox.left + x + w - 6;
    if (container.vTop < ROW_H || right - left < 40 || right < groupBox.left + idxColW) { if (el.style.display !== 'none') el.style.display = 'none'; return; }
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
