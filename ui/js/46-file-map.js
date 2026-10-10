/* ---------------------------------------------------------------
   FILE MAP
   The rows shown as one picture (Ctrl+M, palette, ☰ menu): each visible
   column a vertical band, each pixel row a run of rows. A cell's run is
   painted in its column's type colour (the grid's: numbers cyan, dates
   violet, text grey), as opaque as the run is filled — empty stays dark —
   and red where values are not of the column's type (text in a column
   of numbers, a date written otherwise). A gutter on the left shows the
   irregular rows (yellow) and the edited ones (orange). What scrolling
   never shows does at once: a column empty from some row on, a shifted
   export, a block of malformed dates.
   Computed in the background, in passes: one row of every run first (the
   whole map at once, coarse), then the next one of each, until every row
   is counted (MAP_EXACT rows at most: past that, as many rows of each run
   as fit, said so). Hovering says what a run holds; a click goes there —
   to the run's first value not of its column's type, if any.
----------------------------------------------------------------*/
const MAP_EXACT = 3e6;            // rows counted one by one; past that, sampled evenly
const MAP_COL_MAX = 120;          // CSS px a column band takes at most
let fmap = null;                  // { t, key, rows, cols, kinds, nb, bins…, counts, gen }

function mapDialog() {
    modalOnce('modal-map', `<div id="modal-map" class="modal-box">
    <div class="mp-head"><h4>File map</h4><span id="mp-sum" class="muted"></span>
        <span class="mp-legend"><i class="k-n"></i>Numbers<i class="k-d"></i>Dates<i class="k-t"></i>Text<i class="k-e"></i>Empty<i class="k-x"></i>Not the column's type<i class="k-i"></i>Irregular row<i class="k-m"></i>Edited</span></div>
    <div id="mp-cols"></div>
    <div id="mp-body"><div id="mp-rows"></div><canvas id="mp-cv"></canvas><div id="mp-hl"></div></div>
    <div id="mp-info" class="muted">Hover the map to read it, click to go there.</div>
</div>`);
    const cv = document.getElementById('mp-cv');
    if (cv._wired) return;
    cv._wired = true;
    cv.addEventListener('mousemove', e => mapHover(e));
    cv.addEventListener('mouseleave', () => { document.getElementById('mp-hl').style.display = 'none'; mapInfo(''); });
    cv.addEventListener('click', e => mapGo(e));
}

function openFileMap() {
    const t = T(); if (!t || !t.loaded || !t.filteredData.length) return;
    mapDialog();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-map').style.display = 'block';
    mapStart(t);
}

/* The geometry, from the box's size: GUT px of gutter, then one band per visible column. */
const MAP_GUT = 12;
function mapGeom(t) {
    const body = document.getElementById('mp-body'), cols = visibleCols(t);
    const W = body.clientWidth - 46, H = body.clientHeight;          // 46: the row labels on the left
    const cw = Math.max(2, Math.min(MAP_COL_MAX, Math.floor((W - MAP_GUT - 4) / Math.max(1, cols.length))));
    const shown = cols.slice(0, Math.floor((W - MAP_GUT - 4) / cw));   // a file of thousands of columns: the first ones
    const n = t.filteredData.length, nb = Math.max(1, Math.min(n, Math.floor(H)));
    return { W: MAP_GUT + 4 + shown.length * cw, H, cw, cols: shown, all: cols.length, n, nb };
}

function mapStart(t) {
    const g = mapGeom(t), kinds = columnKinds(t);
    const key = [t.filteredData, ...dataStamp(t), g.cols.join(','), g.W, g.H, currentTheme()];
    if (fmap && fmap.t === t && fmap.key.length === key.length && fmap.key.every((x, i) => x === key[i])) { mapDraw(); mapLabels(); return; }
    if (fmap) fmap.gen++;
    const nc = g.cols.length, nb = g.nb, n = g.n;
    fmap = {
        t, key, g, kinds: g.cols.map(c => kinds[c]), rows: t.filteredData, gen: (fmap ? fmap.gen : 0) + 1,
        lo: new Float64Array(nb + 1),                       // run b = rows [lo[b], lo[b + 1])
        seen: new Uint32Array(nb), irr: new Uint32Array(nb), edit: new Uint32Array(nb),
        fill: new Uint32Array(nb * nc), bad: new Uint32Array(nb * nc), firstBad: new Int32Array(nb * nc).fill(-1),
        round: 0, rounds: 0, done: false, sampled: n > MAP_EXACT
    };
    for (let b = 0; b <= nb; b++) fmap.lo[b] = Math.floor(b * n / nb);
    let most = 0; for (let b = 0; b < nb; b++) most = Math.max(most, fmap.lo[b + 1] - fmap.lo[b]);
    fmap.rounds = fmap.sampled ? Math.max(1, Math.floor(MAP_EXACT / nb)) : most;   // rows of each run counted
    const cv = document.getElementById('mp-cv'), dpr = window.devicePixelRatio || 1;
    cv.width = Math.round(g.W * dpr); cv.height = Math.round(g.H * dpr);
    cv.style.width = g.W + 'px'; cv.style.height = g.H + 'px';
    fmap.colors = mapColors(cv.getContext('2d'));
    mapLabels();
    mapStep(fmap.gen);
}

/* Palette colours as [r, g, b] (stripRgb, 28-…): the map follows the theme like the grid. */
function mapColors(ctx) {
    const css = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
    const rgb = n => stripRgb(ctx, css(n) || '#888');
    return { n: rgb('--cyan-txt'), d: rgb('--violet-txt'), t: rgb('--txt-mut'), '': rgb('--txt-mut'), x: rgb('--danger'), i: rgb('--warn'), m: rgb('--orange-txt'), e: rgb('--surf-3'), bg: rgb('--bg') };
}

/* One slice of passes, ~12 ms: pass r counts the r-th row of every run (an even step through
   it when sampled), so the whole map shows from the first pass and sharpens. */
function mapStep(gen) {
    const m = fmap; if (!m || m.gen !== gen || document.getElementById('modal-map').style.display !== 'block') return;
    const { rows, kinds, lo } = m, nb = m.g.nb, cols = m.g.cols, nc = cols.length, W = m.t.headers.length, t0 = performance.now();
    while (m.round < m.rounds && performance.now() - t0 < 12) {
        const r = m.round++;
        for (let b = 0; b < nb; b++) {
            const size = lo[b + 1] - lo[b];
            if (!size) continue;
            const off = m.sampled ? Math.floor((r + .5) * size / m.rounds) : r;
            if (off >= size) continue;
            const i = lo[b] + off, row = rows[i], d = row.data;
            m.seen[b]++;
            if (row.len !== W) m.irr[b]++;
            if (row.d) m.edit[b]++;
            for (let k = 0, o = b * nc; k < nc; k++, o++) {
                const v = d[cols[k]];
                const ty = v == null || v === '' ? '' : cellType(v);
                if (!ty) continue;
                m.fill[o]++;
                const kind = kinds[k];
                if ((kind === 'n' || kind === 'd') && ty !== kind) { m.bad[o]++; if (m.firstBad[o] < 0 || i < m.firstBad[o]) m.firstBad[o] = i; }
            }
        }
    }
    m.done = m.round >= m.rounds;
    const now = performance.now();
    if (m.done || !m.drawn || now - m.drawn > 120) { m.drawn = now; mapDraw(); mapSum(); }
    if (!m.done) setTimeout(() => mapStep(gen), 0);
}

/* The picture, put as one ImageData. */
function mapDraw() {
    const m = fmap; if (!m) return;
    const cv = document.getElementById('mp-cv'), ctx = cv.getContext('2d'), w = cv.width, h = cv.height, dpr = cv.width / m.g.W;
    const img = ctx.createImageData(w, h), px = img.data, C = m.colors, nc = m.g.cols.length, cw = m.g.cw;
    const rect = (x0, y0, x1, y1, c, a) => {
        const X0 = Math.round(x0 * dpr), X1 = Math.round(x1 * dpr), Y0 = Math.round(y0 * dpr), Y1 = Math.max(Y0 + 1, Math.round(y1 * dpr)), A = Math.round(255 * a);
        for (let y = Y0; y < Y1 && y < h; y++) for (let x = X0, o = (y * w + X0) * 4; x < X1; x++, o += 4) { px[o] = c[0]; px[o + 1] = c[1]; px[o + 2] = c[2]; px[o + 3] = A; }
    };
    const gap = cw >= 6 ? 1 : 0, bh = m.g.H / m.g.nb;
    rect(0, 0, 5, m.g.H, C.e, .35); rect(6, 0, 11, m.g.H, C.e, .35);   // the gutter's two lanes, so a lone mark reads as one
    for (let b = 0; b < m.g.nb; b++) {
        const s = m.seen[b]; if (!s) continue;
        const y0 = b * bh, y1 = (b + 1) * bh;
        if (m.irr[b]) rect(0, y0, 5, Math.max(y1, y0 + 2), C.i, .6 + .4 * Math.sqrt(m.irr[b] / s));
        if (m.edit[b]) rect(6, y0, 11, Math.max(y1, y0 + 2), C.m, .6 + .4 * Math.sqrt(m.edit[b] / s));
        for (let k = 0; k < nc; k++) {
            const o = b * nc + k, x0 = MAP_GUT + 4 + k * cw, x1 = x0 + cw - gap;
            if (m.bad[o]) rect(x0, y0, x1, y1, C.x, .6 + .4 * Math.sqrt(m.bad[o] / s));
            else if (m.fill[o]) { const tx = m.kinds[k] !== 'n' && m.kinds[k] !== 'd'; rect(x0, y0, x1, y1, C[m.kinds[k]] || C.t, tx ? .15 + .35 * (m.fill[o] / s) : .3 + .7 * (m.fill[o] / s)); }   // text quieter: the typed columns and the misfits stand out
            else rect(x0, y0, x1, y1, C.e, .55);
        }
    }
    ctx.putImageData(img, 0, 0);
}

/* Column names over their bands (when there is room) and row positions down the side. */
function mapLabels() {
    const m = fmap, t = m.t, g = m.g;
    const head = document.getElementById('mp-cols');
    head.style.paddingLeft = (46 + MAP_GUT + 4) + 'px';
    head.innerHTML = g.cw >= 28 ? g.cols.map(c => `<span style="width:${g.cw}px" title="${esc(t.headers[c])}">${esc(t.headers[c])}</span>`).join('') : '';
    const side = document.getElementById('mp-rows'), step = tickStep({ H: g.H, sh: g.n * ROW_H }, g.n), out = [];
    for (let i = step; i < g.n; i += step) out.push(`<span style="top:${(i / g.n * g.H).toFixed(1)}px">${esc(stripCompact.format(i).replace(/\s/g, ''))}</span>`);
    side.innerHTML = out.join('');
}

function mapSum() {
    const m = fmap, g = m.g, el = document.getElementById('mp-sum');
    const cols = g.cols.length < g.all ? `the first ${fmt(g.cols.length)} of ${fmt(g.all)} columns` : `${fmt(g.all)} columns`;
    const state = !m.done ? ` · reading… ${Math.round(100 * m.round / m.rounds)} %` : m.sampled ? ` · ${fmt(m.rounds * g.nb)} rows sampled` : '';
    el.textContent = `${fmt(g.n)} rows × ${cols}${state}`;
}

function mapInfo(s) { document.getElementById('mp-info').textContent = s || 'Hover the map to read it, click to go there.'; }

/* Where the pointer is: the run, the column (-1: the gutter), and the box to outline. */
function mapAt(e) {
    const m = fmap; if (!m) return null;
    const r = document.getElementById('mp-cv').getBoundingClientRect(), x = e.clientX - r.left, y = e.clientY - r.top;
    const nb = m.g.nb, b = Math.max(0, Math.min(nb - 1, Math.floor(y / (m.g.H / nb))));
    const k = x < MAP_GUT + 4 ? -1 : Math.floor((x - MAP_GUT - 4) / m.g.cw);
    if (k >= m.g.cols.length) return null;
    return { b, k, x, y };
}
function mapHover(e) {
    const m = fmap, at = mapAt(e), hl = document.getElementById('mp-hl');
    if (!at) { hl.style.display = 'none'; mapInfo(''); return; }
    const { b, k } = at, s = m.seen[b], a = m.lo[b] + 1, z = m.lo[b + 1], t = m.t;
    const span = a === z ? `Row ${fmt(a)}` : `Rows ${fmt(a)}–${fmt(z)}`, part = s < z - a + 1 ? ` (${fmt(s)} read)` : '';
    let what;
    if (k < 0) what = [m.irr[b] ? `${fmt(m.irr[b])} irregular` : '', m.edit[b] ? `${fmt(m.edit[b])} edited` : ''].filter(Boolean).join(' · ') || 'no irregular nor edited row';
    else {
        const o = b * m.g.cols.length + k, kind = { n: 'numbers', d: 'dates', t: 'text', '': 'empty' }[m.kinds[k]];
        what = `${t.headers[m.g.cols[k]]} (${kind}) · ${s ? Math.round(100 * m.fill[o] / s) : 0} % filled` + (m.bad[o] ? ` · ${fmt(m.bad[o])} not ${kind}` : '');
    }
    mapInfo(`${span}${part} · ${what}`);
    const x0 = k < 0 ? 0 : MAP_GUT + 4 + k * m.g.cw, bh = m.g.H / m.g.nb;
    Object.assign(hl.style, { display: 'block', left: (46 + x0) + 'px', top: (b * bh) + 'px', width: (k < 0 ? MAP_GUT - 1 : m.g.cw) + 'px', height: Math.max(2, bh) + 'px' });
}
/* A click: the run's first value not of its column's type, else the row under the pointer. */
function mapGo(e) {
    const m = fmap, at = mapAt(e); if (!at) return;
    const t = m.t, { b, k } = at, size = m.lo[b + 1] - m.lo[b];
    let i = m.lo[b] + Math.min(size - 1, Math.floor((at.y / (m.g.H / m.g.nb) - b) * size));
    if (k >= 0 && m.firstBad[b * m.g.cols.length + k] >= 0) i = m.firstBad[b * m.g.cols.length + k];
    else if (k < 0) for (let j = m.lo[b]; j < m.lo[b + 1]; j++) { const r = m.rows[j]; if (r.len !== t.headers.length || r.d) { i = j; break; } }
    const c = k >= 0 ? m.g.cols[k] : (visibleCols(t)[0] || 0);
    closeAllModals();
    if (T() !== t) return;
    container.vTop = Math.max(0, thead.offsetHeight + i * ROW_H - container.clientHeight / 2);
    setSel(t, i, c, i, c); revealCell(i, c);
}

window.addEventListener('keydown', e => {
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey || e.key.toLowerCase() !== 'm') return;
    if (document.getElementById('dlg') || document.getElementById('cmdk').classList.contains('open')) return;
    e.preventDefault();
    const box = document.getElementById('modal-map');
    if (box && box.style.display === 'block') closeAllModals(); else openFileMap();
});
window.addEventListener('resize', () => {
    const box = document.getElementById('modal-map');
    if (!box || box.style.display !== 'block' || !fmap) return;
    clearTimeout(mapStart.timer); mapStart.timer = setTimeout(() => { if (T() === fmap.t) mapStart(fmap.t); }, 150);
});
