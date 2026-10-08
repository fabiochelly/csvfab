/* ---------------------------------------------------------------
   COLUMN CHARTS
   A band under the titles where every column draws what it holds:
   numbers and dates as a histogram in their type colour (the grid's:
   cyan, violet), text as a ribbon of its most frequent values, each its
   own colour, the rest grey; under all of them a gauge of how full the
   column is, filled green, empty grey (a click keeps the filled or the
   empty rows). Filtering keeps the whole file
   faint behind and draws what the filters let through on top, so every
   other column shows where the rows shown sit — click a bar or a value
   to filter on it (a value filter, as the column panel sets), again to
   show all. Hover reads a bar: its values as written, its rows.

   Cost kept off the paths the bench measures: the header only gets one
   empty div per column; the pictures are CSS (a mask image per column
   for the bars, a gradient for the ribbons, in their own <style>, so a
   header rebuild reuses them), computed in idle time after the frame
   from at most SPK_ROWS rows — every row of a smaller file, else every
   k-th record of the file (by record, not by place: a sort gives the
   same rows, so the same picture), at most SPK_CELLS cells.
----------------------------------------------------------------*/
const SPK_ROWS = 10000;           // rows read at most: every row of a smaller file (a 32-bar picture needs no more)
const SPK_CELLS = 1.5e6;          // rows × columns read at most (a file of 1 000 columns: 1 500 rows)
const SPK_BINS = 32;              // bars of a histogram (an integer column of a few values: one each)
const SPK_SEGS = 5;               // text values given a colour of their own
const SPK_DISTINCT = 4000;        // distinct text values counted per column; later new ones count as "others"
const SPK_QUIET = 250;            // ms without a filter, edit or header drawn before a pass starts
const SPK_GAUGE = 10;             // px at the bottom of a box that stand for its gauge (6 px + 4 of the gap): no more, the curve ends 13 px up and its foot must stay its own
const SPK_SLICE = 4;              // ms of work at most per idle callback: whatever runs next waits no more
const SPK_SEG_MIN = 6;            // px a ribbon part (its 2 px gap included) or a gauge part takes at least: enough to hover it
let spkJob = null;                // { t, stamp, rows, i, f (a filter pass), acc }
let spkIdle = 0;                  // the idle callback asked for, if any
let spkWait = 0;                  // the quiet time before a pass starts

function spkOn() { return !document.documentElement.classList.contains('no-spk'); }
function toggleSparklines() {
    const on = !spkOn();
    document.documentElement.classList.toggle('no-spk', !on);
    try { localStorage.setItem('csvfab-sparklines', on ? '1' : '0'); } catch (e) { }
    renderStatusFormat(T());
    const t = T();
    if (t && t.loaded) { render(); spkSoon(t); }   // the header changed height: the grid's extent with it
}

/* Dates on a time line (dateKey packs yyyymmddhhmmss: months would leave gaps between them). */
function spkTime(s) {
    const k = dateKey(s); if (isNaN(k)) return NaN;
    const p = n => Math.floor(k / n) % 100;
    return Date.UTC(Math.floor(k / 1e10), p(1e8) - 1, p(1e6), p(1e4), p(100), k % 100);
}
const spkKey = (kind, s) => kind === 'n' ? numKey(s) : spkTime(s);
function spkBin(c, k) { return Math.max(0, Math.min(c.B - 1, Math.floor((k - c.lo) / c.w))); }

/* The headers as the pictures were made for (columns added in place, moved or renamed: no
   picture until the next pass, rather than another column's). */
const spkSig = t => t.headers.length + '\u0001' + t.headers.join('\u0001');
function spkShown(t) { const s = t && t.spk; return s && s.hsig === spkSig(t) ? s : null; }

/* The band's row of the header (renderHeader): an empty box per column, its picture coming
   from #spk-style; the only text is a text column's label (a curve's range is in its tooltip:
   written under it, it took the height the curve needs, the user). The gauge (its own
   span, .gg, for its rounded ends) is a histogram's only: a text column's ribbon ends with its
   empty cells, a second bar under it said the same thing twice (the user, 2026-10-08). */
function spkRowHtml(t) {
    if (t.lang) return '';
    const s = spkShown(t);
    let h = `<tr class="spk-row${s && s.anim ? ' in' : ''}"><th class="col-idx spk-idx" onclick="toggleSparklines()" title="${spkIdxTitle(t)}"></th>`;
    for (let i = 0; i < t.headers.length; i++) {
        const c = s && s.cols[i];
        const inner = !c ? '' : c.k === 't' ? `<span class="lb">${spkLabel(s, i)}</span><span class="rb"></span>`
            : '<span class="gg"></span>';
        h += `<th><div class="spk${c ? ' k-' + (c.k || 'e') : ''}" id="spk-${i}">${inner}</div></th>`;
    }
    return h + '</tr>';
}
/* renderHeader() keeps the band's row while it is still the right one (same tab, pictures,
   columns): its 100 boxes cost more to rebuild than the rest of the header — 6.7 → 15.4 ms
   a header on 100 columns, which runs after every filter, undo, tab switch. */
const spkRowKey = t => `${t.id}\u0002${t.spk ? t.spk.id : 0}\u0002${!!(t.spk && t.spk.anim)}\u0002${spkSig(t)}`;
function spkRowKept(t) { const tr = thead.querySelector('.spk-row'); return !!tr && tr._k === spkRowKey(t); }
function spkRowMark(t) { const tr = thead.querySelector('.spk-row'); if (tr) tr._k = spkRowKey(t); }
let spkIds = 0;
function spkIdxTitle(t) {
    const s = t.spk;
    const from = !s ? 'Column charts' : s.sampled ? `Column charts — from ${fmt(s.S)} rows taken evenly through the ${fmt(s.n)}` : `Column charts — from all ${fmt(s.n)} rows`;
    return `${from}&#10;Hover a bar to read it, click it to filter&#10;Click here to hide them`;
}

/* A text column's label: its most frequent value shown (the filtered one under a filter), in its
   ribbon colour, and its share. */
function spkTop(s, i) {
    const c = s.cols[i], f = s.f && s.f[i], seg = f ? f.seg : c.segs.map(x => x[1]);
    let top = -1; for (let k = 0; k < c.segs.length; k++) if (seg[k] && (top < 0 || seg[k] > seg[top])) top = k;
    return top >= 0 && seg[top] >= (f ? f.other : c.other) / 2 ? top : -1;
}
function spkLabel(s, i) {
    const c = s.cols[i], f = s.f && s.f[i], seg = f ? f.seg : c.segs.map(x => x[1]), tot = f ? f.fill : c.fill;
    if (!tot) return '<i>empty</i>';
    const top = spkTop(s, i);
    if (top >= 0) return `<b class="s${top}">${esc(cellShown(c.segs[top][0], 80))}</b><i>${spkPct(seg[top] / Math.max(1, f ? s.fS : s.S))}</i>`;   // of the rows, as the ribbon's widths
    if (!c.fill) return '<i>empty</i>';
    /* No value stands out: how many there are and how full the column is, on two lines in two
       colours — the ribbon reads by hovering it (a legend of its coloured values under it was clutter). */
    const fill = spkPct(f ? f.fill / Math.max(1, s.fS) : c.fill / Math.max(1, s.S));
    return `<span class="two"><i>${c.distinct === c.fill && !s.sampled ? 'all distinct' : `${fmt(c.distinct)}${c.many || s.sampled ? '+' : ''} values`}</i><i class="fl">${fill} filled</i></span>`;
}
const spkPct = x => (x * 100).toLocaleString('fr-FR', { maximumFractionDigits: x < .1 ? 1 : 0 }) + ' %';

/* Called whenever the header is drawn, the rows filtered or an edit logged: shows what is
   known for this tab, and asks for what is not once things have been quiet for SPK_QUIET.
   Nothing else here: the sample's setup (a copy sorted by record, the arrays) cost 1–2 ms
   when it ran right away, inside every filter, sort and edit — the page bench caught it;
   and an operation that awaits a worker or the server leaves the page idle meanwhile,
   so a pass started at once ran inside it (opening and Review changes measured 4–7 %
   slower) — hence the wait, which typing also keeps postponing. */
function spkSoon(t) {
    if (!t || T() !== t || !t.loaded || t.lang || !spkOn()) return;
    const s = t.spk;
    if (s && s.f && t.filteredData === t.allData) { s.f = null; spkLabels(t); }
    spkApply(t);
    if (spkStale(t)) spkQuiet();
}
function spkQuiet() {
    clearTimeout(spkWait);
    spkWait = setTimeout(() => { if (!spkIdle) spkIdle = requestIdleCallback(spkTick, { timeout: 1000 }); }, SPK_QUIET);
}
/* Something in flight that awaits a worker, the disk or the server — a file being read, a
   filtering in the workers, a save or an import (the progress bar is on; for 30 s at most, in
   case a path never ended it): its continuation must not wait behind a slice. */
function spkBusy() {
    return tabs.some(x => x.loading || x.filterPending) || !!(par && (par.run || par.next))
        || (progressAt > 0 && performance.now() - progressAt < 30000);
}
/* The same, in the next task: after a filtering or an edit, whose frame does not need the
   charts — a few hundredths of a ms each time, but inside every filter the bench times. */
let spkNext = 0;
function spkLater(t) {
    if (!spkNext) spkNext = setTimeout(() => { spkNext = 0; spkSoon(T()); }, 0);
}
/* What is missing: the whole file's picture ('base'), or that of the rows shown ('filter'). */
function spkStale(t) {
    const s = t.spk;
    if (!s || !sameStamp(s.stamp, dataStamp(t))) return 'base';
    if (t.filteredData !== t.allData && !(s.f && s.fData === t.filteredData && s.fLen === t.filteredData.length)) return 'filter';
    return null;
}
const spkJobOk = j => sameStamp(j.stamp, dataStamp(j.t)) && (!j.f || (j.rows0 === j.t.filteredData && j.len0 === j.t.filteredData.length));

/* One idle slice: start the pass the active tab needs (a pass whose rows changed meanwhile is
   dropped), read rows until the idle time runs out, finish it or ask for the next slice. */
function spkTick(dl) {
    spkIdle = 0;
    const t = T();
    if (!t || !t.loaded || t.lang || !spkOn()) { spkJob = null; return; }   // the next header drawn asks again
    if (spkBusy()) return spkQuiet();
    let j = spkJob;
    if (j && (j.t !== t || !spkJobOk(j))) j = spkJob = null;
    if (!j) { const need = spkStale(t); if (!need) return; j = spkStart(t, need === 'filter'); }
    if (!spkRun(j, dl)) { spkIdle = requestIdleCallback(spkTick, { timeout: 1000 }); return; }
    spkJob = null;
    if (j.f) spkFinishFilter(t, j); else spkFinishBase(t, j);
    spkSoon(t);                               // the filter pass, if the rows shown are filtered
}

function spkStart(t, filterPass) {
    const kinds = columnKinds(t), nc = t.headers.length, S = Math.max(500, Math.min(SPK_ROWS, Math.floor(SPK_CELLS / Math.max(1, nc))));
    const src = filterPass ? t.filteredData : t.allData, n = src.length;
    /* Every row of a smaller file; else every k-th record of the file, whatever the order shown
       (rows added by hand, with no record, by their place) — or, for the rows shown, an even
       sample. Read in file order, for the row store's block decoding: sorted only when not
       already (a view not sorted is in file order). */
    const k = n <= S ? 1 : Math.ceil(n / S), rows = [];
    let inOrder = true, prev = -1;
    for (let i = 0; i < n; i++) {
        const r = src[i];
        if (k > 1 && (filterPass ? i % k : r.b >= 0 ? r.b % k : i % k)) continue;
        rows.push(r);
        const b = r.b < 0 ? 1e15 : r.b; if (b < prev) inOrder = false; prev = b;
    }
    if (!inOrder) rows.sort((a, b) => (a.b < 0 ? 1e15 : a.b) - (b.b < 0 ? 1e15 : b.b));
    const acc = [];
    for (let c = 0; c < nc; c++) {
        const k = kinds[c] || 't';
        if (filterPass) {
            const bc = t.spk.cols[c];
            acc.push(bc.k === 'n' || bc.k === 'd' ? { k: bc.k, cnt: new Float64Array(bc.B), fill: 0, bad: 0 }
                : bc.k === 't' ? { k: 't', idx: new Map(bc.segs.map((x, i) => [x[0], i])), seg: new Float64Array(bc.segs.length), other: 0, fill: 0 }
                : { k: '', fill: 0 });
        } else acc.push(k === 'n' || k === 'd' ? { k, keys: [], strs: [], m: 0, fill: 0, bad: 0, frac: false }
            : { k: 't', counts: new Map(), over: 0, fill: 0 });
    }
    return spkJob = { t, stamp: dataStamp(t), hsig: spkSig(t), f: filterPass, rows, i: 0, acc, rows0: t.filteredData, len0: t.filteredData.length, n };
}

/* Reads rows of the pass for SPK_SLICE ms at most, less if the idle time runs out first;
   true once every row is read. */
function spkRun(j, dl) {
    const { rows, acc } = j, nc = acc.length, t0 = performance.now();
    const base = j.f ? j.t.spk.cols : null;
    while (j.i < rows.length) {
        if ((j.i & 31) === 0 && (performance.now() - t0 > SPK_SLICE || (!dl.didTimeout && dl.timeRemaining() < 1))) break;
        const d = rows[j.i++].data;
        for (let c = 0; c < nc; c++) {
            const a = acc[c], v = d[c];
            if (v == null || v === '') continue;
            if (a.k === 'n' || a.k === 'd') {
                const ty = cellType(v); if (!ty) continue;
                a.fill++;
                if (ty !== a.k) { a.bad++; continue; }
                const s = v.trim(), x = spkKey(a.k, s);
                if (isNaN(x)) { a.bad++; continue; }
                if (base) { a.cnt[spkBin(base[c], x)]++; continue; }
                a.keys[a.m] = x; a.strs[a.m++] = s;
                if (x % 1) a.frac = true;
            } else if (a.k === 't') {
                if (!cellType(v)) continue;
                a.fill++;
                if (base) { const k = a.idx.get(v); if (k === undefined) a.other++; else a.seg[k]++; continue; }
                const n = a.counts.get(v);
                if (n !== undefined) a.counts.set(v, n + 1);
                else if (a.counts.size < SPK_DISTINCT) a.counts.set(v, 1);
                else a.over++;
            } else if (cellType(v)) a.fill++;
        }
    }
    return j.i >= rows.length;
}

function spkFinishBase(t, j) {
    const S = j.rows.length, cols = j.acc.map(a => {
        if (a.k === 't') {
            /* The SPK_SEGS most frequent values held twice or more, whatever their share: the colours
               always come in the same order (with a 2.5 % floor a column often had its first value
               blue, then grey — the user expected the second colour each time). */
            const all = [...a.counts].sort((x, y) => y[1] - x[1]), min = 2;
            const segs = all.slice(0, SPK_SEGS).filter(x => x[1] >= min);
            const other = a.fill - segs.reduce((s, x) => s + x[1], 0);
            return { k: a.fill ? 't' : '', fill: a.fill, segs, other, distinct: all.length + (a.over ? 1 : 0), many: !!a.over };
        }
        if (!a.m) return { k: '', fill: a.fill };
        /* The range: min to max, unless a few outliers would squeeze every other value into
           one bar — then the 1st to 99th percentile, the end bars taking the outliers. */
        const ks = Float64Array.from(a.keys).sort(), q = p => ks[Math.round(p * (a.m - 1))];
        let lo = ks[0], hi = ks[a.m - 1];
        const p1 = q(.01), p99 = q(.99);
        if (p99 > p1 && hi - lo > 4 * (p99 - p1)) { lo = p1; hi = p99; }
        let B, w;
        if (a.k === 'n' && !a.frac && hi - lo < 40) { B = hi - lo + 1; w = 1; lo -= .5; }   // a few integers: one bar each
        else if (hi === lo) { B = 1; w = 1; lo -= .5; }
        else { B = SPK_BINS; w = (hi - lo) / B; }
        const c = { k: a.k, B, lo, w, fill: a.fill, bad: a.bad, typed: a.m, cnt: new Float64Array(B), first: new Array(B), last: new Array(B) };
        for (let i = 0; i < a.m; i++) {
            const x = a.keys[i], b = spkBin(c, x);
            c.cnt[b]++;
            if (!c.first[b] || x < c.first[b][0]) c.first[b] = [x, a.strs[i]];
            if (!c.last[b] || x > c.last[b][0]) c.last[b] = [x, a.strs[i]];
        }
        return c;
    });
    const old = t.spk;
    t.spk = { id: ++spkIds, stamp: j.stamp, hsig: j.hsig, n: j.n, S, sampled: j.n > S, cols, f: null,
              anim: !old, fa: old ? old.fa : 0 };
    if (t.spk.anim) {                         // the bars rise once, when they first come (a later rebuild of the header does not replay it)
        const s = t.spk;
        setTimeout(() => {
            s.anim = false;
            const tr = thead.querySelector('.spk-row');
            if (tr && T() === t && t.spk === s) { tr.classList.remove('in'); spkRowMark(t); }
        }, 1600);
    }
    spkRowSwap(t);                            // the boxes get their kind, a text column its label
}
/* The band's row alone, redrawn in place: a whole header rebuild would take the focus from a
   column filter being typed in. */
function spkRowSwap(t) {
    const tr = thead.querySelector('.spk-row');
    if (tr && T() === t) { tr.outerHTML = spkRowHtml(t); spkRowMark(t); }
}

function spkFinishFilter(t, j) {
    const s = t.spk;
    s.f = j.acc.map(a => a.k === 't' ? { seg: a.seg, other: a.other, fill: a.fill } : a.k ? { cnt: a.cnt, fill: a.fill, bad: a.bad } : { fill: a.fill });
    s.fData = j.rows0; s.fLen = j.len0; s.fS = j.rows.length; s.fa = (s.fa || 0) + 1;
    spkLabels(t);
}
/* A text column's label follows the rows shown. */
function spkLabels(t) {
    const s = spkShown(t); if (!s || T() !== t) return;
    for (let i = 0; i < s.cols.length; i++) {
        /* By class: the hover's light (spkHl) may be the box's last child — text written into it
           followed the pointer as an unstyled italic tooltip. */
        const lb = s.cols[i].k === 't' && document.querySelector('#spk-' + i + ' .lb');
        if (lb) lb.innerHTML = spkLabel(s, i);
    }
}

/* The pictures, as one stylesheet: per column its gauge, its pick, its bars (an SVG mask
   stretched to the box, so any width keeps them sharp) or its ribbon (a gradient). */
function spkApply(t) {
    let st = document.getElementById('spk-style');
    if (!st) { st = document.createElement('style'); st.id = 'spk-style'; document.head.appendChild(st); }
    const s = spkShown(t);
    /* Called after every header rebuild, filter and edit: nothing to do unless the pictures,
       the rows shown's or a pick changed. */
    const picks = t && t.spkPicks ? Object.keys(t.spkPicks).map(i => i + ':' + (t.valFilters[i] === t.spkPicks[i].set ? t.spkPicks[i].bar : '')).join() : '';
    if (st._k && st._k[0] === s && st._k[1] === (s && s.f) && st._k[2] === picks) return;
    st._k = [s, s && s.f, picks];
    let css = '';
    if (s) {
        const f = s.f, vis = visibleCols(t), pos = new Map(vis.map((c, k) => [c, k]));
        for (let i = 0; i < s.cols.length; i++) {
            const c = s.cols[i], fc = f && f[i];
            const fill = fc ? fc.fill / Math.max(1, s.fS) : c.fill / Math.max(1, s.S);
            let rule = `--fill:${(fill * 100).toFixed(1)}%;--gm:${fill > 0 && fill < 1 ? SPK_SEG_MIN : 0}px;--i:${Math.min(pos.has(i) ? pos.get(i) : 0, 30)}`;
            const pk = spkPicked(t, i);
            if (pk) rule += `;--pl:${pk[0] >= 1 - 1e-9 ? 0 : (pk[0] / (1 - pk[1]) * 100).toFixed(2)}%;--pw:${(pk[1] * 100).toFixed(2)}%`;
            css += `#spk-${i}{${rule}}`;
            if (c.k === 'n' || c.k === 'd') {
                /* Each picture made once (the header is drawn again after every filter, undo, tab switch).
                   The rows shown at their own scale: their shape against the whole file's, readable
                   however few they are (at the file's scale a 2 % subset was a dotted line). */
                css += `#spk-${i}::before{--m:${c.svg || (c.svg = spkSvg(c, c.cnt))}}`;
                if (fc) css += `#spk-${i}::after{--m:${fc.svg || (fc.svg = spkSvg(c, fc.cnt))}}`;
            } else if (c.k === 't') {
                css += `#spk-${i} .rb{background-image:${spkRibbon(s, i)}}`;
            }
        }
        if (f) css += `.spk-row .spk::before{opacity:.3}.spk-row .spk::after{display:block;animation-name:spk-f${s.fa % 2}}`;
    }
    if (st.textContent !== css) st.textContent = css;
}

/* Bars as an SVG path, heights in % of the tallest, tops rounded; a bar holding anything is
   never under 7 %, so a lone outlier still shows. */
let spkCurve = true;               // a continuous column as a smooth area (the bars kept for a few values: one each)
function spkSvg(c, cnt) {
    const B = cnt.length, max = Math.max(...cnt);
    if (spkCurve && B === SPK_BINS) return spkArea(cnt, max);
    let d = '';
    for (let b = 0; b < B; b++) {
        if (!cnt[b]) continue;
        const h = Math.max(7, cnt[b] / max * 100), r = Math.min(h, 8);
        d += `M${(b + .12).toFixed(2)} 100v-${(h - r).toFixed(1)}q0-${r.toFixed(1)} .2-${r.toFixed(1)}h.36q.2 0 .2 ${r.toFixed(1)}V100z`;
    }
    return `url("data:image/svg+xml,%3csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 ${B} 100' preserveAspectRatio='none'%3e%3cpath d='${d}'/%3e%3c/svg%3e")`;
}

/* The same counts as a smooth area: a monotone cubic through the middle of every bar (Fritsch–
   Carlson: it never rises above a bar nor dips under zero between two, so a peak or a gap stays
   where it is), filled faint under a crisp line — the line in px whatever the stretch. */
function spkArea(cnt, max) {
    const B = cnt.length, y = Array.from(cnt, n => n ? Math.max(7, n / max * 100) : 0), d = [], t = [];
    for (let k = 0; k < B - 1; k++) d.push(y[k + 1] - y[k]);
    for (let k = 0; k < B; k++) t.push(k === 0 ? d[0] || 0 : k === B - 1 ? d[B - 2] : d[k - 1] * d[k] <= 0 ? 0 : (d[k - 1] + d[k]) / 2);
    for (let k = 0; k < B - 1; k++) {
        if (!d[k]) { t[k] = t[k + 1] = 0; continue; }
        const a = t[k] / d[k], b = t[k + 1] / d[k], h = a * a + b * b;
        if (h > 9) { const r = 3 / Math.sqrt(h); t[k] = r * a * d[k]; t[k + 1] = r * b * d[k]; }
    }
    const Y = v => (100 - v).toFixed(1);
    let line = `M0 ${Y(y[0])}H.5`;
    for (let k = 0; k < B - 1; k++) line += `C${(k + .5 + 1 / 3).toFixed(2)} ${Y(y[k] + t[k] / 3)} ${(k + 1.5 - 1 / 3).toFixed(2)} ${Y(y[k + 1] - t[k + 1] / 3)} ${k + 1.5} ${Y(y[k + 1])}`;
    line += `H${B}`;
    return `url("data:image/svg+xml,%3csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 ${B} 100' preserveAspectRatio='none'%3e%3cpath d='${line}V100H0z' fill-opacity='.55'/%3e%3cpath d='${line}' fill='none' stroke='black' stroke-width='1.6' stroke-linejoin='round' vector-effect='non-scaling-stroke'/%3e%3c/svg%3e")`;
}

/* A ribbon: the colour values' shares of the rows, then the others in grey, then the empty cells
   fainter, 2 px apart, none under SPK_SEG_MIN px. */
function spkRibbon(s, i) {
    const parts = spkSegs(s, i), n = parts.length;
    if (!n) return 'none';
    const end = spkEnds(parts), stop = k => `calc(${(end[k][0] * 100).toFixed(3)}% + ${end[k][1].toFixed(2)}px)`;
    return `linear-gradient(90deg,${parts.map((p, k) => `${k ? `transparent 0 calc(${stop(k - 1).slice(5, -1)} + 2px),` : ''}var(--${p.cls === 'se' ? 'sge' : p.cls === 'so' ? 'sgo' : 'sg' + p.seg}) 0 ${stop(k)}`).join(',')})`;
}
/* Where each part of a ribbon ends, as [fraction of the width, px]: its share of what is left
   once every part has SPK_SEG_MIN px — counted among all the rows, empty ones included, a value
   held by 0.4 % of a mostly empty column was under a px, hidden by the 2 px gap before it (the
   gaps stay: parts side by side were ugly, the user). The
   same ends, in px, map the pointer to a part (spkPart()). */
function spkEnds(parts) {
    const n = parts.length, tot = parts.reduce((a, p) => a + p.n, 0);
    let cum = 0;
    return parts.map((p, k) => { cum += p.n / tot; return k === n - 1 ? [1, 0] : [cum, (k + 1 - cum * n) * SPK_SEG_MIN]; });
}
/* A ribbon's parts in order, those holding rows: the coloured values, the others, the empty cells
   (of the rows shown under a filter). */
function spkSegs(s, i) {
    const c = s.cols[i], f = s.f && s.f[i], seg = f ? Array.from(f.seg) : c.segs.map(x => x[1]);
    const parts = seg.map((n, k) => ({ n, seg: k, cls: 's' + k }));
    parts.push({ n: f ? f.other : c.other, other: true, cls: 'so' }, { n: f ? s.fS - f.fill : s.S - c.fill, fill: 'e', cls: 'se' });
    return parts.filter(p => p.n > 0);
}

/* What a point of column i's band stands for: a bar of a histogram, a value of a ribbon. */
function spkPart(t, i, x, W) {
    const s = t.spk, c = s.cols[i];
    if (c.k === 'n' || c.k === 'd') { const b = Math.min(c.B - 1, Math.floor(x * c.B)); return { bar: b, x0: b / c.B, w: 1 / c.B }; }
    if (c.k !== 't') return null;
    const parts = spkSegs(s, i), end = spkEnds(parts).map(e => (e[0] * W + e[1]) / W);
    for (let k = 0, at = 0; k < parts.length; at = end[k++]) {
        const p = parts[k], w = end[k] - at;
        if (x < end[k] || k === parts.length - 1) return { ...(p.fill ? { fill: p.fill } : p.other ? { other: true } : { seg: p.seg }), x0: at, w, cls: p.cls };
    }
    return null;
}

/* What a point of the gauge stands for: the filled part or the empty one (of the rows shown). */
function spkGaugePart(t, i, x, W) {
    const s = t.spk, c = s.cols[i], f = s.f && s.f[i];
    let ff = f ? f.fill / Math.max(1, s.fS) : c.fill / Math.max(1, s.S);
    if (ff > 0 && ff < 1) ff = Math.min(Math.max(ff * W, SPK_SEG_MIN), W - SPK_SEG_MIN) / W;   // as drawn: each part SPK_SEG_MIN px at least (--gm)
    return ff > 0 && (x < ff || ff >= 1) ? { fill: 'f', x0: 0, w: ff } : { fill: 'e', x0: ff, w: 1 - ff };
}

/* The bar picked by a click, while its value filter is still the one the click set:
   [left, width] as fractions of the band. */
function spkPicked(t, i) {
    const p = t.spkPicks && t.spkPicks[i];
    if (!p || t.valFilters[i] !== p.set) return null;
    const c = t.spk.cols[i];
    return p.bar != null && c.B ? [p.bar / c.B, ((p.bar1 ?? p.bar) - p.bar + 1) / c.B] : null;   // a value of a ribbon: the ribbon is that value alone now
}

/* ---- Hover and click ---- */
/* The part under the pointer, lit: the rest of its box fades (.hv) and spkHl draws the part
   alone in full colour — a bar through the box's own mask, shifted so its bar lands under it
   (the rows shown's bars under a filter), a ribbon value in its colour, a part of the gauge.
   A grey box laid over the part dulled the very thing pointed at (the user, 2026-10-08). */
const spkHl = document.createElement('i'); spkHl.className = 'spk-hl';
function spkHlOff() { if (spkHl.parentElement) { spkHl.parentElement.classList.remove('hv'); spkHl.remove(); } }
function spkHlOn(h) {
    const p = h.part, el = spkHl, st = el.style;
    if (spkHl.parentElement !== h.box) { spkHlOff(); h.box.appendChild(el); }
    h.box.classList.add('hv');
    el.className = 'spk-hl ' + (h.gauge ? 'g ' + p.fill : p.bar != null ? 'b' : 'r ' + p.cls);
    st.left = (p.x0 * 100).toFixed(2) + '%'; st.width = (p.w * 100).toFixed(2) + '%';
    st.borderRadius = '';
    if (h.gauge || p.bar == null) {           // a ribbon value (past the 2 px gap before it), a gauge part: rounded only at the bar's own ends
        const first = p.x0 < 1e-9, last = p.x0 + p.w > 1 - 1e-9, r = k => k ? '3px' : '0';
        if (!first && !h.gauge) { st.left = `calc(${(p.x0 * 100).toFixed(2)}% + 2px)`; st.width = `calc(${(p.w * 100).toFixed(2)}% - 2px)`; }
        st.borderRadius = `${r(first)} ${r(last)} ${r(last)} ${r(first)}`;
    }
    if (p.bar != null && !h.gauge) {
        const m = getComputedStyle(h.box, h.t.spk.f ? '::after' : '::before').getPropertyValue('--m');
        st.setProperty('--m', m);
        st.setProperty('--ms', h.r.width + 'px 100%');
        st.setProperty('--mp', -(p.x0 * h.r.width) + 'px 0');
    }
}
function spkAt(e) {
    const box = e.target.closest && e.target.closest('.spk'), t = T();
    if (!box || !spkShown(t)) return null;
    const i = +box.id.slice(4), c = t.spk.cols[i]; if (!c) return null;
    const r = box.getBoundingClientRect(), x = Math.max(0, Math.min(.9999, (e.clientX - r.left) / r.width));
    const gauge = c.k !== 't' && e.clientY > r.bottom - SPK_GAUGE;   // a text column has no gauge: its ribbon holds the empty cells
    return { t, i, c, box, r, x, gauge, part: gauge ? spkGaugePart(t, i, x, r.width) : spkPart(t, i, x, r.width) };
}
function spkTip(h) {
    const { t, i, c } = h, s = t.spk, f = s.f && s.f[i], est = n => (s.sampled ? '≈ ' : '') + fmt(Math.round(n * s.n / s.S));
    const fest = n => (s.fS < t.filteredData.length ? '≈ ' : '') + fmt(Math.round(n * t.filteredData.length / s.fS));
    const p = h.part, pk = t.spkPicks && t.spkPicks[i], picked = !!pk && t.valFilters[i] === pk.set;
    if (!p || p.fill) {                       // the gauge: filled or empty, of the whole file and of the rows shown
        const e = p && p.fill === 'e', n = e ? s.S - c.fill : c.fill, fn = f && (e ? s.fS - f.fill : f.fill);
        const hint = !p || !(f ? fn : n) ? '' : picked && pk.fill === p.fill ? '\nClick: show all again' : `\nClick: show only the ${e ? 'empty' : 'filled'} rows`;
        return `${t.headers[i]} — ${e ? 'empty' : 'filled'}: ${est(n)} row${n === 1 && !s.sampled ? '' : 's'} · ${spkPct(n / s.S)}`
            + (f ? `\n${fest(fn)} of the rows shown` : '')
            + (c.bad && !e ? `\n${est(c.bad)} not ${c.k === 'n' ? 'numbers' : 'dates'}` : '') + hint;
    }
    let what, n, fn;
    if (p.bar != null) {                      // a bar, or a range of them being dragged across (bar1)
        const hi = p.bar1 ?? p.bar;
        let a = null, b = null; n = 0; fn = f ? 0 : null;
        for (let k = p.bar; k <= hi; k++) { if (!a && c.first[k]) a = c.first[k]; if (c.last[k]) b = c.last[k]; n += c.cnt[k]; if (f) fn += f.cnt[k]; }
        what = !a ? 'No value here' : a[1] === b[1] ? a[1] : `${a[1]} – ${b[1]}`;
    } else if (p.seg != null) { what = c.segs[p.seg][0]; n = c.segs[p.seg][1]; fn = f && f.seg[p.seg]; }
    else { what = `${fmt(Math.max(0, c.distinct - c.segs.length))}${c.many || s.sampled ? '+' : ''} other values`; n = c.other; fn = f && f.other; }
    return `${what.length > 120 ? what.slice(0, 120) + '…' : what}\n${est(n)} row${n === 1 && !s.sampled ? '' : 's'} · ${spkPct(n / s.S)}`
        + (f ? `\n${fest(fn)} of the rows shown` : '')
        + (h.brush ? (n ? '\nRelease: show only these' : '') : n ? `\n${picked ? 'Click: show all again' : 'Click: show only these'}${p.bar != null ? ' · drag across: a range' : ''}` : '');
}
thead.addEventListener('mousemove', e => {
    if (spkBrush) return;                     // a range being dragged: the window's handler draws it
    const h = spkAt(e);
    spkGz(h && h.gauge && h.box);
    if (!h) return spkHlOff();
    const p = h.part;
    if (p && p.w > 0) spkHlOn(h); else spkHlOff();
    spkTipAt(h, e);
});
function spkTipAt(h, e) {
    const p = h.part;
    tipFor = h.box;                           // 34-…: leaving the band hides it
    tipBox.textContent = spkTip(h);
    tipBox.classList.add('open');
    const w = tipBox.offsetWidth, x = p ? h.r.left + (p.x0 + p.w / 2) * h.r.width : e.clientX;
    tipBox.style.left = Math.max(6, Math.min(x - w / 2, innerWidth - w - 6)) + 'px';
    tipBox.style.top = (h.r.bottom + 8) + 'px';
}
thead.addEventListener('mouseleave', () => { if (!spkBrush) { spkGz(null); spkHlOff(); } });
/* Dragging across the bars of a curve picks a range of them (2026-10-08, the user's pick): lit as
   the pointer goes, said in the tooltip, filtered on release — a value filter keeping every value
   of those bars, as one bar's click does. A press and release on one bar stays a click. */
let spkBrush = null;                      // { h: the press's spkAt(), b0, b1 } while dragging
let spkBrushed = false;                   // the click that ends a drag is not a click on a bar
thead.addEventListener('mousedown', e => {
    if (e.button !== 0) return;
    const h = spkAt(e);
    if (!h || h.gauge || !h.part || h.part.bar == null) return;
    e.preventDefault();                       // no text selection while dragging
    spkBrush = { h, b0: h.part.bar, b1: h.part.bar };
});
const spkRange = (B, c) => { const lo = Math.min(B.b0, B.b1), hi = Math.max(B.b0, B.b1); return { bar: lo, bar1: hi, x0: lo / c.B, w: (hi - lo + 1) / c.B }; };
window.addEventListener('mousemove', e => {
    const B = spkBrush; if (!B) return;
    const { h } = B, c = h.c, x = Math.max(0, Math.min(.9999, (e.clientX - h.r.left) / h.r.width));
    const b = Math.min(c.B - 1, Math.floor(x * c.B));
    if (b === B.b1) return;
    B.b1 = b;
    const g = { ...h, part: spkRange(B, c), brush: true };
    spkHlOn(g); spkTipAt(g, e);
});
window.addEventListener('mouseup', () => {
    const B = spkBrush; if (!B) return;
    spkBrush = null;
    if (B.b0 === B.b1) return;                // a click: thead's click handler takes it
    spkBrushed = true; setTimeout(() => { spkBrushed = false; }, 0);
    spkHlOff();
    if (T() === B.h.t && spkShown(B.h.t)) spkFilter(B.h.t, B.h.i, spkRange(B, B.h.c));
});
/* The box whose gauge is under the pointer: drawn thicker (.gz). */
let spkGzBox = null;
function spkGz(box) {
    if (spkGzBox === box) return;
    if (spkGzBox) spkGzBox.classList.remove('gz');
    spkGzBox = box || null;
    if (box) box.classList.add('gz');
}
thead.addEventListener('click', e => {
    if (spkBrushed) return;
    const h = spkAt(e); if (!h || !h.part || !(h.part.w > 0)) return;
    spkFilter(h.t, h.i, h.part);
});

/* A click on a bar, a value or a part of the gauge: a value filter keeping only those values
   (every other distinct value of the column excluded — empty ones too, but for the gauge's
   empty part, which keeps only them), as the column panel would set it; on the part already
   picked, the filter goes. */
function spkFilter(t, i, part) {
    const picks = t.spkPicks || (t.spkPicks = {}), c = t.spk.cols[i], p = picks[i];   // by column: a pick elsewhere leaves this one
    if (p && t.valFilters[i] === p.set && p.bar === part.bar && (p.bar1 ?? p.bar) === (part.bar1 ?? part.bar) && p.seg === part.seg && !!p.other === !!part.other && p.fill === part.fill) {
        delete t.valFilters[i]; delete picks[i];
        setStats(`${t.headers[i]}: every value shown again.`);
    } else {
        const keep = part.fill ? (part.fill === 'f' ? (v => !!cellType(v)) : (v => !cellType(v)))
            : c.k === 't' && part.seg != null ? (v => v === c.segs[part.seg][0])
            : c.k === 't' ? (() => { const top = new Set(c.segs.map(x => x[0])); return v => !top.has(v) && !!cellType(v); })()
            : (v => { const s = v.trim(); if (cellType(s) !== c.k) return false; const x = spkKey(c.k, s); if (isNaN(x)) return false; const b = spkBin(c, x); return b >= part.bar && b <= (part.bar1 ?? part.bar); });
        const seen = new Set();
        visitRows(t, t.allData, r => seen.add(cellStr(cellOf(r, i))));
        const ex = new Set(); let kept = 0;
        for (const v of seen) { if (keep(v)) kept++; else ex.add(v); }
        if (!kept) return setStats('No row holds those values.');
        if (!ex.size) return setStats(`${t.headers[i]}: every row already holds those values.`);
        t.valFilters[i] = ex;
        picks[i] = { set: ex, bar: part.bar, bar1: part.bar1, seg: part.seg, other: !!part.other, fill: part.fill };
        const hi = part.bar1 ?? part.bar, lo1 = part.bar != null && c.first.slice(part.bar, hi + 1).find(Boolean), hi1 = part.bar != null && c.last.slice(part.bar, hi + 1).filter(Boolean).pop();
        const what = part.fill ? `the ${part.fill === 'f' ? 'filled' : 'empty'} rows` : part.bar != null ? (lo1[1] === hi1[1] ? lo1[1] : `${lo1[1]} to ${hi1[1]}`)
            : part.seg != null ? `"${c.segs[part.seg][0]}"` : 'the other values';
        setStats(`${t.headers[i]}: only ${what} — click it again to show all.`);
    }
    tipHide();
    renderHeader(); applyColStyles(); applyFilters();
}
