/* ---------------------------------------------------------------
   TOTALS ROW
   A line held at the bottom of the grid — sticky in #grid-layer, right
   after #tbody, so it sits under the last row when every row fits —
   giving each column a total of the rows shown: sum, average, median,
   smallest, largest for numbers; earliest, latest for dates; filled,
   empty, distinct values, share filled for any column. A click on a
   total picks its function (with every value in the menu); the Σ cell
   sums every number column, clears them all, or hides the row (also
   ☰ › Columns and the palette).
   Defaults: a sum under the columns that read as quantities
   (gbSumByDefault(), as Group by and the pivot table), nothing elsewhere.
   t.totals {col: fn} holds the choices ('' = none), keyed by column index
   like the other view maps (remapCols, shiftKeys, viewSnap).

   Cost kept off the measured paths: render() draws the row (a div per
   visible column, rebuilt only when its key changed) but computes
   nothing; the values come from a pass over the rows shown in idle time
   after a quiet moment (TOT_QUIET: typing a filter keeps postponing it),
   in file order (the row store decodes by blocks), every row — a total
   is exact or it is not one —, in slices of TOT_SLICE chained as
   background tasks (any other task goes first: an awaited continuation
   waits for one slice at most), and not at all while a worker, the disk
   or the server is awaited (spkBusy(), 47-…). Until it ends, the
   previous values stay, dimmed.
----------------------------------------------------------------*/
/* var: render() and syncSpace() reach this file during the boot run, before its top level. */
var totJob = null;                // { t, stamp, rows, len, hsig, acc, order, i }: the pass under way
var totIdle = 0, totWait = 0, totNext = 0, totIds = 0;
var totRowH = 0;                  // the row's height while it is shown (the grid's extent, revealCell, the strip)
const TOT_QUIET = 150;            // ms without a render, filter or edit before a pass starts
const TOT_SLICE = 4;              // ms of work at most per slice: whatever comes next waits no more
const TOT_DISTINCT = 1e6;         // distinct values counted at most (then "1 000 000+")
const TOT_DEC_ROWS = 2000;        // numbers whose decimals are looked at (the column's writing: 12,50 → two)
const TOT_PLAIN = /^-?\d+(?:\.\d+)?$/;   // read by +s, faster than numKey()
/* fn → [short label in the cell, name in the menu]; a date column's min / max are named apart. */
const TOT_FNS = {
    sum: ['Sum', 'Sum'], avg: ['Avg', 'Average'], median: ['Median', 'Median'], min: ['Min', 'Smallest'], max: ['Max', 'Largest'],
    filled: ['Filled', 'Filled cells'], empty: ['Empty', 'Empty cells'], distinct: ['Distinct', 'Distinct values'], pct: ['Filled', 'Share filled'],
};
const TOT_DATE = { min: ['First', 'Earliest'], max: ['Last', 'Latest'] };
const TOT_LIST = { n: ['sum', 'avg', 'median', 'min', 'max'], d: ['min', 'max'], any: ['filled', 'empty', 'distinct', 'pct'] };

function totOn() { return !document.documentElement.classList.contains('no-tot'); }
function totHeight() { return totRowH || 0; }
function toggleTotals() {
    const on = !totOn();
    document.documentElement.classList.toggle('no-tot', !on);
    try { localStorage.setItem('csvfab-totals', on ? '1' : '0'); } catch (e) { }
    const t = T();
    renderStatusFormat(t);                    // its switch in the status bar
    if (t && t.loaded) render(); else totDraw(null);
    setStats(on ? 'Totals row shown — click a total to change it.' : 'Totals row hidden.');
}

/* The defaults, known once the idle pass has read them (gbSumByDefault reads up to 400 rows a
   numeric column: not inside the opening's frame); kept on the headers and the base, not on
   dataStamp() — a sort or an edit would read them again for the same answer. */
function totDefKnown(t) { const d = t.totDef; return d && d.sig === spkSig(t) && d.base === t.base ? d.fns : null; }
function totDefaults(t) { totDefStep(t, Infinity); return t.totDef.fns; }
/* Column by column until `end`: true once every column is decided (a hundred columns are not read in one slice). */
function totDefStep(t, end) {
    if (totDefKnown(t)) return true;
    let w = t.totDefWip;
    if (!w || w.sig !== spkSig(t) || w.base !== t.base) w = t.totDefWip = { sig: spkSig(t), base: t.base, fns: [] };
    while (w.fns.length < t.headers.length) {
        w.fns.push(gbSumByDefault(t, w.fns.length) ? 'sum' : '');
        if (performance.now() > end && w.fns.length < t.headers.length) return false;
    }
    t.totDef = { sig: w.sig, base: w.base, fns: w.fns }; t.totDefWip = null;
    return true;
}
function totFn(t, c, def) { const m = t.totals || {}; return c in m ? m[c] : def ? def[c] || '' : ''; }

/* The values computed are for these rows, these edits, these columns. */
function totFresh(t) {
    const s = t.tot;
    return !!s && sameStamp(s.stamp, dataStamp(t)) && s.rows === t.filteredData && s.len === t.filteredData.length && s.hsig === spkSig(t);
}
/* What a column's function needs beyond the running counts. */
const totHas = (a, fn) => !!a && (fn !== 'median' || !!a.vals) && (fn !== 'distinct' || !!a.set);
function totMissing(t) {
    const def = totDefKnown(t); if (!def) return true;
    const fresh = totFresh(t);
    for (const c of visibleCols(t)) { const fn = totFn(t, c, def); if (fn && !(fresh && totHas(t.tot.cols[c], fn))) return true; }
    return false;
}

/* ---- Drawing --------------------------------------------------- */
function totNum(x, d0, d1) { return x.toLocaleString('fr-FR', { minimumFractionDigits: d0, maximumFractionDigits: d1 }); }
/* What a column shows for fn: { text, raw (copied: no thousands spaces), tip }; null while unknown. */
function totValue(a, fn, n) {
    if (!a) return null;
    const num = x => ({ text: totNum(x, a.dec, a.dec), raw: x.toLocaleString('fr-FR', { useGrouping: false, maximumFractionDigits: a.dec }) });
    const left = a.bad ? ` · ${fmt(a.bad)} value${a.bad > 1 ? 's' : ''} not ${a.kind === 'd' ? 'a date' : 'a number'} left out` : '';
    const of = `${fmt(a.n)} ${a.kind === 'd' ? 'date' : 'number'}${a.n === 1 ? '' : 's'} in ${fmt(n)} row${n === 1 ? '' : 's'} shown${left}`;
    const none = { text: '—', raw: '', tip: `No ${a.kind === 'd' ? 'date' : 'number'} in the ${fmt(n)} rows shown` };
    const cnt = (k, tip) => ({ text: fmt(k), raw: String(k), tip });
    switch (fn) {
        case 'sum': return a.n ? { ...num(a.sum), tip: `Sum of ${of}` } : none;
        case 'avg': case 'median': {
            if (!a.n) return none;
            const x = fn === 'avg' ? a.sum / a.n : totMedian(a), d1 = Math.max(a.dec, 2);
            return { text: totNum(x, a.dec, d1), raw: x.toLocaleString('fr-FR', { useGrouping: false, maximumFractionDigits: d1 }), tip: `${fn === 'avg' ? 'Average' : 'Median'} of ${of}` };
        }
        case 'min': case 'max': {
            if (!a.n) return none;
            const lo = fn === 'min', what = a.kind === 'd' ? (lo ? 'Earliest' : 'Latest') : (lo ? 'Smallest' : 'Largest');
            if (a.kind === 'd') { const s = lo ? a.minS : a.maxS; return { text: s, raw: s, tip: `${what} of ${of}` }; }
            return { ...num(lo ? a.min : a.max), tip: `${what} of ${of}` };
        }
        case 'filled': return cnt(a.filled, `${fmt(a.filled)} filled cells in ${fmt(n)} rows shown`);
        case 'empty': return cnt(a.empty, `${fmt(a.empty)} empty cells in ${fmt(n)} rows shown`);
        case 'pct': { const p = n ? a.filled / n * 100 : 0, s = totNum(p, 0, p > 0 && p < 1 ? 1 : 0) + ' %'; return { text: s, raw: s, tip: `${fmt(a.filled)} of ${fmt(n)} rows shown are filled` }; }
        case 'distinct': { const k = a.set.size; return { text: fmt(k) + (a.capped ? '+' : ''), raw: String(k), tip: `${fmt(k)}${a.capped ? '+' : ''} distinct values in ${fmt(n)} rows shown` }; }
    }
    return null;
}
function totMedian(a) {
    if (a.med == null) {
        const v = Float64Array.from(a.vals).sort(), h = v.length >> 1;
        a.med = v.length % 2 ? v[h] : (v[h - 1] + v[h]) / 2;
    }
    return a.med;
}
function totLabel(fn, kind) { return ((kind === 'd' && TOT_DATE[fn]) || TOT_FNS[fn] || ['', ''])[0]; }

/* The row: the Σ cell (under the row numbers), then a cell per visible column at its pinned width
   — every visible column, as the header has, not a window: one line of short cells. Rebuilt only
   when its key changed (columns, widths, choices, values); new values being counted only dim it.
   lazy (from render()): the cells are built in the next task, not in the frame of an opening or a
   tab switch — 100 columns cost that frame ~6 ms (measured, 16 duels in one page); meanwhile the
   bar shows, the same height (the grid's extent is right at once), the previous cells kept when
   they are this tab's. */
function totDraw(t, L, lazy) {
    const el = document.getElementById('tfoot');
    if (!t || !t.loaded || t.lang || !totOn() || !L || !L.vis.length || !t.filteredData.length) {
        if (el._k !== null) { el.style.display = 'none'; el.innerHTML = ''; el._k = null; totRowH = 0; }
        return;
    }
    const def = totDefKnown(t), hsig = spkSig(t), s = t.tot && t.tot.hsig === hsig ? t.tot : null;   // other columns: never their values
    const fns = L.vis.map(c => totFn(t, c, def));
    const key = `${t.id}|${L.vis.join(',')}|${L.x.join(',')}|${L.F}|${fns.join(',')}|${s ? s.id : 0}|${!!def}`;
    const missing = totMissing(t);
    el.classList.toggle('stale', missing);
    if (missing) totSoon(t);
    if (el._k === key) return;
    if (lazy) {
        if (!el._k || el._t !== t.id) el.innerHTML = `<div class="tf-c col-idx" style="width:${idxColW}px">Σ</div>`;
        el._k = 'pending'; el._t = t.id;
        el.style.width = L.x[L.x.length - 1] + 'px';
        if (el.style.display !== 'flex') { el.style.display = 'flex'; totRowH = ROW_H; }
        totLater();
        return;
    }
    el._k = key; el._t = t.id;
    const kinds = columnKinds(t), n = s ? s.len : t.filteredData.length;
    let h = `<div class="tf-c col-idx" style="width:${idxColW}px" title="Totals of the rows shown&#10;Click: sum every number column, clear, hide">Σ</div>`;
    L.vis.forEach((c, k) => {
        const w = L.x[k + 1] - L.x[k], fn = fns[k], kind = kinds[c] || '';
        const frz = k < L.F ? ` frz${k === L.F - 1 ? ' frz-last' : ''}` : '';
        const style = `width:${w}px${frz ? `;left:${L.x[k]}px` : ''}`;
        if (!fn) { h += `<div class="tf-c tf-none${frz}" data-c="${c}" style="${style}" title="Click: a total for this column"></div>`; return; }
        const a = s && s.cols[c], v = totHas(a, fn) ? totValue(a, fn, n) : null;
        const kc = (fn === 'min' || fn === 'max') && kind === 'd' ? ' kd' : TOT_LIST.n.includes(fn) && kind === 'n' ? ' kn' : '';
        /* The name gives way to the value when both do not fit (a date column is as wide as one
           date): it is still in the tooltip. ~6.6 px a letter of the small capitals, 8 of gap. */
        const lab = totLabel(fn, kind), room = w - 21 - lab.length * 6.6 - 8;
        const tip = v ? (textWidth(v.text) > room ? `${lab}: ${v.text} — ` : '') + v.tip : 'Counting…';
        h += `<div class="tf-c${kc}${frz}" data-c="${c}" style="${style}" title="${esc(tip)}&#10;Click: another total">${v && textWidth(v.text) > room ? '' : `<span class="tf-l">${lab}</span>`}<span class="tf-v">${v ? esc(v.text) : '…'}</span></div>`;
    });
    el.innerHTML = h;
    el.style.width = L.x[L.x.length - 1] + 'px';
    if (el.style.display !== 'flex') { el.style.display = 'flex'; totRowH = ROW_H; }
}
/* The row again, for the active tab as drawn (values arrived, a choice changed). */
function totRedraw(t) {
    if (!t || T() !== t || !drawn || drawn.t !== t) return;
    const L = colLayout(t); if (L) totDraw(t, L);
}

/* ---- Counting -------------------------------------------------- */
function totSoon(t) {
    if (!t || T() !== t || !t.loaded || t.lang || !totOn()) return;
    clearTimeout(totWait);
    totWait = setTimeout(() => { if (!totIdle) totIdle = requestIdleCallback(totTick, { timeout: 1000 }); }, TOT_QUIET);
}
/* The next slice: a background task — chained idle callbacks would wait for the next idle period
   each, a few ms of work in 50; a setTimeout(0) chain is clamped to 4 ms. */
function totPost() {
    totIdle = -1;
    if (window.scheduler && scheduler.postTask) scheduler.postTask(totTick, { priority: 'background' });
    else setTimeout(totTick, 0);
}
/* After an edit (updateSaveBtn), in the next task: its frame does not need the totals, and an
   edit that redraws only its rows calls no render() to notice them stale. */
function totLater() {
    if (!totNext) totNext = setTimeout(() => { totNext = 0; totRedraw(T()); }, 0);
}
const totJobOk = j => T() === j.t && sameStamp(j.stamp, dataStamp(j.t)) && j.rows === j.t.filteredData && j.len === j.rows.length && j.hsig === spkSig(j.t);
function totTick() {
    totIdle = 0;
    const t = T();
    if (!t || !t.loaded || t.lang || !totOn()) { totJob = null; return; }
    if (spkBusy()) { totSoon(t); return; }   // a continuation awaited must not wait behind a slice
    const end = performance.now() + TOT_SLICE;
    if (totJob && !totJobOk(totJob)) totJob = null;
    if (!totJob) {
        if (!totDefKnown(t)) { if (!totDefStep(t, end)) { totPost(); return; } totRedraw(t); }
        totJob = totStart(t);
        if (!totJob) { totRedraw(t); return; }
    }
    if (totRun(totJob, end)) {
        totFinish(totJob); totJob = null; totRedraw(t);
        if (totMissing(t)) totPost();         // a choice made meanwhile
        return;
    }
    totPost();
}
function totAcc(c, kind, fn) {
    return { c, kind, n: 0, sum: 0, min: Infinity, max: -Infinity, minS: '', maxS: '', filled: 0, empty: 0, bad: 0, dec: 0, decSeen: 0,
        vals: fn === 'median' ? [] : null, set: fn === 'distinct' ? new Set() : null, capped: false, med: null };
}
/* The pass: the visible columns with a function whose values are missing (all of them when the
   rows changed), over the rows shown. */
function totStart(t) {
    const def = totDefaults(t), kinds = columnKinds(t), fresh = totFresh(t), acc = [];
    for (const c of visibleCols(t)) {
        const fn = totFn(t, c, def);
        if (fn && !(fresh && totHas(t.tot.cols[c], fn))) acc.push(totAcc(c, kinds[c] || '', fn));
    }
    if (!acc.length) return null;
    const rows = t.filteredData;
    return { t, stamp: dataStamp(t), rows, len: rows.length, hsig: spkSig(t), acc, order: totOrder(t, rows), i: 0 };
}
/* A view not in file order (sorted) is read in file order — a total does not care, and the row
   store decodes a run of records by blocks; rows with their own array first. null: in order. */
function totOrder(t, rows) {
    const base = t.base; if (!base) return null;
    let prev = -1, sorted = true;
    for (let i = 0; i < rows.length; i++) { const r = rows[i]; if (r.d || r.b < 0) continue; if (r.b < prev) { sorted = false; break; } prev = r.b; }
    if (sorted) return null;
    const at = new Int32Array(base.n).fill(-1), out = new Int32Array(rows.length);
    let k = 0;
    for (let i = 0; i < rows.length; i++) { const r = rows[i]; if (r.d || r.b < 0 || at[r.b] >= 0) out[k++] = i; else at[r.b] = i; }
    for (let b = 0; b < base.n; b++) if (at[b] >= 0) out[k++] = at[b];
    return out;
}
function totRun(j, end) {
    const rows = j.rows, o = j.order, A = j.acc, n = j.len, wide = A.length > 3;   // several columns: the row split once
    let i = j.i;
    while (i < n) {
        const lim = Math.min(n, i + 512);
        for (; i < lim; i++) {
            const r = rows[o ? o[i] : i];
            if (wide) { const d = r.data; for (const a of A) totAdd(a, d[a.c]); }
            else for (const a of A) totAdd(a, cellOf(r, a.c));
        }
        if (performance.now() > end) break;
    }
    j.i = i;
    return i >= n;
}
function totAdd(a, v) {
    let s = v == null ? '' : typeof v === 'string' ? v : String(v);
    if (s && (s.charCodeAt(0) <= 32 || s.charCodeAt(s.length - 1) <= 32 || s.charCodeAt(0) === 160 || s.charCodeAt(s.length - 1) === 160)) s = s.trim();
    if (!s) { a.empty++; return; }
    a.filled++;
    if (a.set && !a.capped) { a.set.add(s); if (a.set.size >= TOT_DISTINCT) a.capped = true; }
    if (a.kind === 'n') {
        const x = TOT_PLAIN.test(s) ? +s : isNumericLike(s) ? numKey(s) : NaN;
        if (!isFinite(x)) { a.bad++; return; }
        a.n++; a.sum += x;
        if (x < a.min) a.min = x;
        if (x > a.max) a.max = x;
        if (a.vals) a.vals.push(x);
        if (a.decSeen < TOT_DEC_ROWS) { a.decSeen++; const d = totDec(s, x); if (d > a.dec) a.dec = d; }
    } else if (a.kind === 'd') {
        const k = dateKey(s);
        if (!(k === k)) { a.bad++; return; }
        a.n++;
        if (k < a.min) { a.min = k; a.minS = s; }
        if (k > a.max) { a.max = k; a.maxS = s; }
    }
}
/* Decimals as the column writes them: "12,50" two, "1 234" none — and "1,234" none either, a
   whole number with three digits after its mark being thousands. Six at most. */
function totDec(s, x) {
    const m = /[.,](\d+)\D*$/.exec(s); if (!m) return 0;
    const d = m[1].length;
    return d === 3 && Number.isInteger(x) ? 0 : Math.min(d, 6);
}
function totFinish(j) {
    const t = j.t;
    let s = t.tot;
    if (!(s && sameStamp(s.stamp, j.stamp) && s.rows === j.rows && s.len === j.len && s.hsig === j.hsig))
        s = t.tot = { stamp: j.stamp, rows: j.rows, len: j.len, hsig: j.hsig, cols: {}, id: 0 };
    for (const a of j.acc) s.cols[a.c] = a;
    s.id = ++totIds;
}

/* ---- Choosing -------------------------------------------------- */
const totFoot = document.getElementById('tfoot');
totFoot.addEventListener('click', totClick);
totFoot.addEventListener('contextmenu', totClick);
function totClick(e) {
    const cell = e.target.closest('.tf-c'); if (!cell) return;
    e.preventDefault(); e.stopPropagation();   // the document's click would close the menu it opens
    const m = document.getElementById('tot-menu');
    const again = m.classList.contains('open') && m._cell === cell;
    closeDDs(); if (colPanel) closeColPanel();
    if (again) return;
    const t = T(); if (!t || !t.loaded) return;
    m.innerHTML = cell.classList.contains('col-idx') ? totIdxMenu(t) : totMenu(t, +cell.dataset.c);
    m._cell = cell;
    m.classList.add('open');
    const r = cell.getBoundingClientRect(), w = m.offsetWidth;
    m.style.left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8)) + 'px';
    m.style.top = 'auto'; m.style.bottom = (window.innerHeight - r.top + 4) + 'px';
}
function totItem(fn, label, k, on) {
    return `<div class="dd-item${on ? ' on' : ''}" onclick="closeDDs(); ${fn}"><span class="lbl">${CHECK_SVG}${label}</span>${k ? `<span class="k">${esc(k)}</span>` : ''}</div>`;
}
/* A column's menu: every function its kind takes, each with its value when known (the running
   counts give all but the median and the distinct values at once), then none, then copy. */
function totMenu(t, c) {
    const def = totDefKnown(t), cur = totFn(t, c, def), kind = columnKinds(t)[c] || '';
    const s = totFresh(t) ? t.tot : null, a = s && s.cols[c];
    const val = fn => { const v = totHas(a, fn) ? totValue(a, fn, s.len) : null; return v ? v.text : ''; };
    const list = fns => fns.map(fn => totItem(`totPick(${c}, '${fn}')`, ((kind === 'd' && TOT_DATE[fn]) || TOT_FNS[fn])[1], val(fn), fn === cur)).join('');
    const cv = cur && totHas(a, cur) ? totValue(a, cur, s.len) : null;
    return `<div class="dd-head">${esc(t.headers[c])} · total of the rows shown</div>`
        + (TOT_LIST[kind] ? list(TOT_LIST[kind]) + '<div class="dd-sep"></div>' : '')
        + list(TOT_LIST.any)
        + '<div class="dd-sep"></div>'
        + totItem(`totPick(${c}, '')`, 'None', '', !cur)
        + (cv && cv.raw ? '<div class="dd-sep"></div>' + totItem(`totCopy(${c})`, 'Copy the value', cv.raw, false) : '');
}
function totIdxMenu(t) {
    return `<div class="dd-head">Totals · ${fmt(t.filteredData.length)} rows shown</div>`
        + totItem("totAll('sum')", 'Sum every number column', '', false)
        + totItem("totAll(null)", 'Back to the usual totals', '', false)
        + totItem("totAll('')", 'Clear every total', '', false)
        + '<div class="dd-sep"></div>'
        + totItem('toggleTotals()', 'Hide the totals row', '', false);
}
function totPick(c, fn) {
    const t = T(); if (!t || !t.loaded) return;
    if (!t.totals) t.totals = {};
    t.totals[c] = fn;
    totRedraw(t);
}
/* fn 'sum': every number column summed (identifiers included: asked for), '': none, null: the defaults. */
function totAll(fn) {
    const t = T(); if (!t || !t.loaded) return;
    const kinds = columnKinds(t);
    t.totals = {};
    if (fn !== null) t.headers.forEach((_, c) => { t.totals[c] = fn === 'sum' && kinds[c] === 'n' ? 'sum' : ''; });
    totRedraw(t);
}
async function totCopy(c) {
    const t = T(); if (!t || !totFresh(t)) return;
    const fn = totFn(t, c, totDefKnown(t)), a = t.tot.cols[c], v = totHas(a, fn) && totValue(a, fn, t.tot.len);
    if (!v || !v.raw) return;
    const name = ((a.kind === 'd' && TOT_DATE[fn]) || TOT_FNS[fn])[1];
    try { await navigator.clipboard.writeText(v.raw); doneMsg(`${t.name} | ${name} of "${t.headers[c]}" copied: ${v.raw}`); }
    catch (e) { setStats(`${t.name} | The clipboard refused the copy.`); }
}
