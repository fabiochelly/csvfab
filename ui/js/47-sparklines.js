/* ---------------------------------------------------------------
   COLUMN CHARTS
   A band under the titles where every column draws what it holds:
   numbers and dates as a histogram in their type colour (the grid's:
   cyan, violet), text as a ribbon of its most frequent values, each its
   own colour, the rest grey; under all of them a gauge of how full the
   column is, the empty part in red (a click keeps the filled or the
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
const SPK_GAUGE = 9;              // px at the bottom of a box that stand for its gauge: hover and click it there
const SPK_SLICE = 4;              // ms of work at most per idle callback: whatever runs next waits no more
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
   from #spk-style; a text column's label is the only text. */
function spkRowHtml(t) {
    if (t.lang) return '';
    const s = spkShown(t);
    let h = `<tr class="spk-row${s && s.anim ? ' in' : ''}"><th class="col-idx spk-idx" onclick="toggleSparklines()" title="${spkIdxTitle(t)}"></th>`;
    for (let i = 0; i < t.headers.length; i++) {
        const c = s && s.cols[i];
        h += `<th><div class="spk${c ? ' k-' + (c.k || 'e') : ''}" id="spk-${i}">${c && c.k === 't' ? `<span class="lb">${spkLabel(s, i)}</span><span class="rb"></span>` : ''}</div></th>`;
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

/* A text column's label: its most frequent value shown (the filtered one under a filter) and its share. */
function spkLabel(s, i) {
    const c = s.cols[i], f = s.f && s.f[i], seg = f ? f.seg : c.segs.map(x => x[1]), tot = f ? f.fill : c.fill;
    if (!tot) return '<i>empty</i>';
    let top = -1; for (let k = 0; k < c.segs.length; k++) if (seg[k] && (top < 0 || seg[k] > seg[top])) top = k;
    const other = f ? f.other : c.other;
    if (top >= 0 && seg[top] >= other / 2) return `<b>${esc(cellShown(c.segs[top][0], 80))}</b><i>${spkPct(seg[top] / tot)}</i>`;
    if (!c.fill) return '<i>empty</i>';
    if (c.distinct === c.fill && !s.sampled) return '<i>all distinct</i>';
    return `<i>${fmt(c.distinct)}${c.many || s.sampled ? '+' : ''} values</i>`;
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
            const all = [...a.counts].sort((x, y) => y[1] - x[1]), min = Math.max(a.fill < 40 ? 1 : 2, a.fill * .025);
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
        const el = s.cols[i].k === 't' && document.getElementById('spk-' + i), lb = el && el.firstElementChild;
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
            let rule = `--fill:${(fill * 100).toFixed(1)}%;--i:${Math.min(pos.has(i) ? pos.get(i) : 0, 30)}`;
            const pk = spkPicked(t, i);
            if (pk) rule += `;--pl:${pk[0] >= 1 - 1e-9 ? 0 : (pk[0] / (1 - pk[1]) * 100).toFixed(2)}%;--pw:${(pk[1] * 100).toFixed(2)}%`;
            css += `#spk-${i}{${rule}}`;
            if (c.k === 'n' || c.k === 'd') {
                /* Each picture made once (the header is drawn again after every filter, undo, tab switch).
                   The rows shown at their own scale: their shape against the whole file's, readable
                   however few they are (at the file's scale a 2 % subset was a dotted line). */
                css += `#spk-${i}::before{--m:${c.svg || (c.svg = spkSvg(c.cnt, Math.max(...c.cnt)))}}`;
                if (fc) css += `#spk-${i}::after{--m:${fc.svg || (fc.svg = spkSvg(fc.cnt, Math.max(...fc.cnt)))}}`;
            } else if (c.k === 't') {
                css += `#spk-${i} .rb{background-image:${spkRibbon(c, fc)}}`;
            }
        }
        if (f) css += `.spk-row .spk::before{opacity:.3}.spk-row .spk::after{display:block;animation-name:spk-f${s.fa % 2}}`;
    }
    if (st.textContent !== css) st.textContent = css;
}

/* Bars as an SVG path, heights in % of the tallest; a bar holding anything is never under
   7 %, so a lone outlier still shows. */
function spkSvg(cnt, max) {
    const B = cnt.length;
    let d = '';
    for (let b = 0; b < B; b++) {
        if (!cnt[b]) continue;
        const h = Math.max(7, cnt[b] / max * 100);
        d += `M${(b + .12).toFixed(2)} ${(100 - h).toFixed(1)}h.76V100h-.76z`;
    }
    return `url("data:image/svg+xml,%3csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 ${B} 100' preserveAspectRatio='none'%3e%3cpath d='${d}'/%3e%3c/svg%3e")`;
}

/* A ribbon: the colour values' shares of the filled cells, then the others in grey, 1.5 px apart. */
function spkRibbon(c, fc) {
    const seg = fc ? Array.from(fc.seg) : c.segs.map(x => x[1]), other = fc ? fc.other : c.other;
    const tot = seg.reduce((a, x) => a + x, 0) + other;
    if (!tot) return 'none';
    const stops = []; let at = 0;
    const part = (col, n) => {
        if (!n) return;
        const a = at; at += n / tot * 100;
        stops.push(`${stops.length ? `transparent 0 calc(${a.toFixed(2)}% + 1.5px),` : ''}${col} 0 ${at.toFixed(2)}%`);
    };
    seg.forEach((n, k) => part(`var(--sg${k})`, n));
    part('var(--sgo)', other);
    return `linear-gradient(90deg,${stops.join(',')})`;
}

/* What a point of column i's band stands for: a bar of a histogram, a value of a ribbon. */
function spkPart(t, i, x) {
    const s = t.spk, c = s.cols[i];
    if (c.k === 'n' || c.k === 'd') { const b = Math.min(c.B - 1, Math.floor(x * c.B)); return { bar: b, x0: b / c.B, w: 1 / c.B }; }
    if (c.k !== 't') return null;
    const fc = s.f && s.f[i], seg = fc ? Array.from(fc.seg) : c.segs.map(z => z[1]), other = fc ? fc.other : c.other;
    const tot = seg.reduce((a, z) => a + z, 0) + other;
    if (!tot) return null;
    let at = 0;
    for (let k = 0; k <= seg.length; k++) {
        const n = k < seg.length ? seg[k] : other; if (!n) continue;
        const w = n / tot;
        if (x < at + w || k === seg.length) return k < seg.length ? { seg: k, x0: at, w } : { other: true, x0: at, w };
        at += w;
    }
    return null;
}

/* What a point of the gauge stands for: the filled part or the empty one (of the rows shown). */
function spkGaugePart(t, i, x) {
    const s = t.spk, c = s.cols[i], f = s.f && s.f[i], ff = f ? f.fill / Math.max(1, s.fS) : c.fill / Math.max(1, s.S);
    return ff > 0 && (x < ff || ff >= 1) ? { fill: 'f', x0: 0, w: ff } : { fill: 'e', x0: ff, w: 1 - ff };
}

/* The bar picked by a click, while its value filter is still the one the click set:
   [left, width] as fractions of the band. */
function spkPicked(t, i) {
    const p = t.spkPicks && t.spkPicks[i];
    if (!p || t.valFilters[i] !== p.set) return null;
    const c = t.spk.cols[i];
    return p.bar != null && c.B ? [p.bar / c.B, 1 / c.B] : null;   // a value of a ribbon: the ribbon is that value alone now
}

/* ---- Hover and click ---- */
const spkHl = document.createElement('i'); spkHl.className = 'spk-hl';
function spkAt(e) {
    const box = e.target.closest && e.target.closest('.spk'), t = T();
    if (!box || !spkShown(t)) return null;
    const i = +box.id.slice(4), c = t.spk.cols[i]; if (!c) return null;
    const r = box.getBoundingClientRect(), x = Math.max(0, Math.min(.9999, (e.clientX - r.left) / r.width));
    const gauge = e.clientY > r.bottom - SPK_GAUGE;
    return { t, i, c, box, r, x, gauge, part: gauge ? spkGaugePart(t, i, x) : spkPart(t, i, x) };
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
    if (p.bar != null) {
        const a = c.first[p.bar], b = c.last[p.bar];
        what = !a ? 'No value here' : a[1] === b[1] ? a[1] : `${a[1]} – ${b[1]}`;
        n = c.cnt[p.bar]; fn = f && f.cnt[p.bar];
    } else if (p.seg != null) { what = c.segs[p.seg][0]; n = c.segs[p.seg][1]; fn = f && f.seg[p.seg]; }
    else { what = `${fmt(Math.max(0, c.distinct - c.segs.length))}${c.many || s.sampled ? '+' : ''} other values`; n = c.other; fn = f && f.other; }
    return `${what.length > 120 ? what.slice(0, 120) + '…' : what}\n${est(n)} row${n === 1 && !s.sampled ? '' : 's'} · ${spkPct(n / s.S)}`
        + (f ? `\n${fest(fn)} of the rows shown` : '')
        + (n ? `\n${picked ? 'Click: show all again' : 'Click: show only these'}` : '');
}
thead.addEventListener('mousemove', e => {
    const h = spkAt(e);
    spkGz(h && h.gauge && h.box);
    if (!h) { if (spkHl.parentElement) spkHl.remove(); return; }
    const p = h.part;
    if (p && p.w > 0) {
        spkHl.className = 'spk-hl' + (h.gauge ? ' g' : '');
        spkHl.style.left = (p.x0 * 100).toFixed(2) + '%'; spkHl.style.width = (p.w * 100).toFixed(2) + '%';
        if (spkHl.parentElement !== h.box) h.box.appendChild(spkHl);
    } else if (spkHl.parentElement) spkHl.remove();
    tipFor = h.box;                           // 34-…: leaving the band hides it
    tipBox.textContent = spkTip(h);
    tipBox.classList.add('open');
    const w = tipBox.offsetWidth, x = p ? h.r.left + (p.x0 + p.w / 2) * h.r.width : e.clientX;
    tipBox.style.left = Math.max(6, Math.min(x - w / 2, innerWidth - w - 6)) + 'px';
    tipBox.style.top = (h.r.bottom + 8) + 'px';
});
thead.addEventListener('mouseleave', () => { spkGz(null); if (spkHl.parentElement) spkHl.remove(); });
/* The box whose gauge is under the pointer: drawn thicker (.gz). */
let spkGzBox = null;
function spkGz(box) {
    if (spkGzBox === box) return;
    if (spkGzBox) spkGzBox.classList.remove('gz');
    spkGzBox = box || null;
    if (box) box.classList.add('gz');
}
thead.addEventListener('click', e => {
    const h = spkAt(e); if (!h || !h.part || !(h.part.w > 0)) return;
    spkFilter(h.t, h.i, h.part);
});

/* A click on a bar, a value or a part of the gauge: a value filter keeping only those values
   (every other distinct value of the column excluded — empty ones too, but for the gauge's
   empty part, which keeps only them), as the column panel would set it; on the part already
   picked, the filter goes. */
function spkFilter(t, i, part) {
    const picks = t.spkPicks || (t.spkPicks = {}), c = t.spk.cols[i], p = picks[i];   // by column: a pick elsewhere leaves this one
    if (p && t.valFilters[i] === p.set && p.bar === part.bar && p.seg === part.seg && !!p.other === !!part.other && p.fill === part.fill) {
        delete t.valFilters[i]; delete picks[i];
        setStats(`${t.headers[i]}: every value shown again.`);
    } else {
        const keep = part.fill ? (part.fill === 'f' ? (v => !!cellType(v)) : (v => !cellType(v)))
            : c.k === 't' && part.seg != null ? (v => v === c.segs[part.seg][0])
            : c.k === 't' ? (() => { const top = new Set(c.segs.map(x => x[0])); return v => !top.has(v) && !!cellType(v); })()
            : (v => { const s = v.trim(); if (cellType(s) !== c.k) return false; const x = spkKey(c.k, s); return !isNaN(x) && spkBin(c, x) === part.bar; });
        const seen = new Set();
        visitRows(t, t.allData, r => seen.add(cellStr(cellOf(r, i))));
        const ex = new Set(); let kept = 0;
        for (const v of seen) { if (keep(v)) kept++; else ex.add(v); }
        if (!kept) return setStats('No row holds those values.');
        if (!ex.size) return setStats(`${t.headers[i]}: every row already holds those values.`);
        t.valFilters[i] = ex;
        picks[i] = { set: ex, bar: part.bar, seg: part.seg, other: !!part.other, fill: part.fill };
        const what = part.fill ? `the ${part.fill === 'f' ? 'filled' : 'empty'} rows` : part.bar != null ? (c.first[part.bar] && c.first[part.bar][1] === c.last[part.bar][1] ? c.first[part.bar][1] : `${c.first[part.bar][1]} to ${c.last[part.bar][1]}`)
            : part.seg != null ? `"${c.segs[part.seg][0]}"` : 'the other values';
        setStats(`${t.headers[i]}: only ${what} — click it again to show all.`);
    }
    tipHide();
    renderHeader(); applyColStyles(); applyFilters();
}
