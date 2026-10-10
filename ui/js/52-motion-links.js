/* ---------------------------------------------------------------
   FILTERS IN MOTION, CHARTS LINKED TO THE ROWS
   - A filter that changes the rows shown: the rows still on screen
     glide to their new places (the gaps close), the rows new on
     screen rise a little into place, the rows gone fade where they
     were. Like the sort (50-…): the rows on screen noted before the
     view changes (~30 elements), the motion in the next frame.
   - Hovering a part of a column chart (a slice, a value, the gauge)
     brightens, in that column, the values it holds — first whole rows
     were lit and the others dimmed: too much, the user — and ticks on
     the scroll strip's left edge show where they are in the whole view,
     counted in slices, a new hover dropping the count.
----------------------------------------------------------------*/
const FILTER_MS = 200;
var motionSort = false;                   // a sort is moving the rows (50-…): its applyFilters() does not move them again
var filterGhosts = [];   // var: applyFilters() (15-…) may run before this file's top level

/* Before the view changes: the rows on screen with their tops (applyFilters, 15-…). */
function filterMotionBefore(t, prev) {
    if (motionSort || sortCalm.matches || !drawn || drawn.t !== t || prev === t.filteredData) return null;
    const top = viewRows.st || 0, h = container._vh || container.clientHeight, on = [];   // no layout read: the DOM may be dirty here (viewRows.st, 12-…)
    for (const el of tbody.children) {
        if (el.dataset.idx == null) continue;
        const i = +el.dataset.idx, y = i * ROW_H - top;
        if (y > -ROW_H && y < h && prev[i]) on.push({ el, r: prev[i], y });
    }
    return on.length ? { on, top } : null;
}
function filterMotionAfter(t, m) {
    if (!m) return;
    const rows = t.filteredData;
    requestAnimationFrame(() => { if (t.filteredData === rows && T() === t && drawn && drawn.t === t) filterMotionRun(t, m); });
}
function filterMotionRun(t, m) {
    for (const g of filterGhosts) g.remove();
    const top = container.vTop, h = container.clientHeight, before = new Map(m.on.map(o => [o.r, o.y])), stay = new Set();
    const ease = 'cubic-bezier(.2, .8, .3, 1)';
    for (const el of tbody.children) {
        if (el.dataset.idx == null) continue;
        const i = +el.dataset.idx, y = i * ROW_H - top;
        if (y <= -ROW_H || y >= h) continue;
        const r = t.filteredData[i];
        if (before.has(r)) {
            stay.add(r);
            const d = before.get(r) - y;
            if (Math.abs(d) >= 1) el.animate([{ transform: `translateY(${d}px)` }, { transform: 'none' }], { duration: FILTER_MS, easing: ease });
        } else el.animate([{ transform: 'translateY(10px)', opacity: .35 }, { transform: 'none', opacity: 1 }], { duration: FILTER_MS, easing: ease });
    }
    /* The rows gone, where they were, fading under the others. */
    filterGhosts = m.on.filter(o => !stay.has(o.r)).map(o => {
        const el = o.el;
        el.removeAttribute('data-idx'); el.classList.add('leaving'); el.style.top = (o.y + top - vscroll.base) + 'px';
        return el;
    });
    if (!filterGhosts.length) return;
    tbody.prepend(...filterGhosts);
    const ghosts = filterGhosts;
    let last = null;
    for (const g of ghosts) last = g.animate([{ opacity: 1 }, { opacity: 0 }], { duration: FILTER_MS * .8, easing: 'ease-out', fill: 'forwards' });
    last.onfinish = last.oncancel = () => { for (const g of ghosts) g.remove(); if (filterGhosts === ghosts) filterGhosts = []; };
}

/* ---- A chart part lights its rows ---- */
let glow = null;                          // { t, key, col, keep, run }
/* What a part keeps: the test spkFilter() applies, on a cell's value (47-…). */
function spkKeep(c, part) {
    if (part.fill) return part.fill === 'f' ? (v => !!cellType(v)) : (v => !cellType(v));
    if (c.k === 't' && part.seg != null) { const s = c.segs[part.seg][0]; return v => v === s; }
    if (c.k === 't') { const top = new Set(c.segs.map(x => x[0])); return v => !top.has(v) && !!cellType(v); }
    return v => { const s = v.trim(); if (cellType(s) !== c.k) return false; const x = spkKey(c.k, s); if (isNaN(x)) return false; const b = spkBin(c, x); return b >= part.bar && b <= (part.bar1 ?? part.bar); };
}
function glowOn(t, i, part) {
    const key = [t.id, i, part.bar, part.bar1, part.seg, !!part.other, part.fill].join();
    if (glow && glow.key === key && glow.t === t) return;
    const keep = spkKeep(t.spk.cols[i], part), run = {};
    glow = { t, key, col: i, keep, run };
    glowRows();
    glowTicks(t, i, keep, run);
}
/* The column's cells drawn: lit or not (rows drawn later, by a scroll meanwhile, stay as they are). */
function glowRows() {
    const g = glow; if (!g || T() !== g.t) return;
    for (const el of tbody.querySelectorAll('.row[data-idx]')) {
        const td = el.querySelector(`.cell[data-c="${g.col}"]`); if (!td) continue;
        const r = g.t.filteredData[+el.dataset.idx];
        td.classList.toggle('lit', !!r && g.keep(cellStr(cellOf(r, g.col))));
    }
}
function glowOff() {
    if (!glow) return;
    glow = null;
    for (const el of tbody.querySelectorAll('.cell.lit')) el.classList.remove('lit');
    const cv = document.querySelector('#scroll-strip .ss-glow');
    if (cv) cv.width = 0;
}
/* Ticks on the strip's left edge: the rows shown that hold the part, counted per pixel row in
   slices of ~8 ms (a new hover drops the count), drawn as one image like the find's (28-…). */
async function glowTicks(t, i, keep, run) {
    const el = document.getElementById('scroll-strip');
    if (!el || el.style.display === 'none') return;
    let cv = el.querySelector('.ss-glow');
    if (!cv) { cv = document.createElement('canvas'); cv.className = 'ss-glow'; el.appendChild(cv); }
    const g = stripGeom(t), dpr = window.devicePixelRatio || 1, W = 4, rows = t.filteredData, scale = g.H / g.sh;
    const counts = new Uint32Array(Math.ceil(g.H) + 2);
    let last = performance.now();
    const draw = () => {
        let max = 0;
        for (let y = 0; y < counts.length; y++) if (counts[y] > max) max = counts[y];
        cv.width = Math.round(W * dpr); cv.height = Math.round(g.H * dpr);
        cv.style.width = W + 'px'; cv.style.height = g.H + 'px';
        if (!max) return;
        const ctx = cv.getContext('2d'), w = cv.width, h = cv.height, img = ctx.createImageData(w, h), px = img.data, rgb = stripRgb(ctx, stripColor('--accent-2'));
        for (let y = 0; y < counts.length; y++) {
            if (!counts[y]) continue;
            const a = Math.round(255 * (.45 + .55 * Math.sqrt(counts[y] / max)));
            for (let yy = Math.floor(y * dpr); yy < Math.min(h, Math.ceil((y + 1) * dpr)); yy++)
                for (let x = 0, o = yy * w * 4; x < w; x++, o += 4) { px[o] = rgb[0]; px[o + 1] = rgb[1]; px[o + 2] = rgb[2]; px[o + 3] = a; }
        }
        ctx.putImageData(img, 0, 0);
    };
    for (let r = 0; r < rows.length; r++) {
        if (keep(cellStr(cellOf(rows[r], i)))) counts[Math.floor((g.th + r * ROW_H) * scale)]++;
        if ((r & 2047) === 2047 && performance.now() - last > 8) {
            draw();
            await new Promise(res => setTimeout(res, 0));
            if (!glow || glow.run !== run) return;
            last = performance.now();
        }
    }
    if (glow && glow.run === run) draw();
}
