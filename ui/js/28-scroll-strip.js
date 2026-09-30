/* ---------------------------------------------------------------
   SCROLL STRIP
   A 24 px band over the grid's vertical scrollbar (#scroll-strip, a
   fixed element the script keeps at the container's right edge; the
   native scrollbar is widened to 24 px underneath and still takes the
   wheel and the keyboard). It is the index of a big file:
   - a thumb standing for the viewport, dragged or the track clicked to
     jump, with a tip giving the row number under the pointer and, when
     the view is sorted, the sort column's value there (hovering shows
     the tip without scrolling);
   - marks along the track (a canvas): irregular rows, duplicate groups,
     marked rows (review, compare), the selection;
   - labels: sorted by a text column the initials, by a date the years,
     by a number the values (compact), else row numbers — the alphabet
     index of an address book, sampled every LABEL_STEP pixels.
   Geometry comes from the container's client size and the table's
   extent (header + rows × ROW_H), no layout read beyond what
   syncSpace() already does. Marks and labels are recomputed only when
   what they depend on changes (stripKey). No module-level state:
   stripLayout() runs from syncSpace() at boot, before this file's top
   level; everything hangs off the element.
----------------------------------------------------------------*/
const STRIP_W = 24, LABEL_STEP = 44, THUMB_MIN = 22;

function stripEl() {
    let el = document.getElementById('scroll-strip');
    if (el) return el;
    el = document.createElement('div'); el.id = 'scroll-strip';
    el.innerHTML = '<canvas class="ss-marks"></canvas><div class="ss-labels"></div><div class="ss-sel"></div><div class="ss-thumb"></div><div class="ss-tip"></div>';
    el._s = { key: null, kind: '', col: -1, drag: false };
    document.body.appendChild(el);
    /* Pressed on the thumb: it is dragged from where it was grabbed, like a scrollbar's;
       pressed on the track: the viewport centres on the pointer, and follows it. */
    el.addEventListener('pointerdown', e => {
        if (e.button !== 0) return;
        e.preventDefault(); el.setPointerCapture(e.pointerId); el._s.drag = true; el.classList.add('drag');
        const th = el.children[3], y = e.clientY - el._s.top, top = parseFloat(th.style.top) || 0, h = parseFloat(th.style.height) || 0;
        el._s.grab = y >= top && y <= top + h ? y - top : null;
        stripJump(e);
    });
    el.addEventListener('pointermove', e => { if (el._s.drag) stripJump(e); else stripTip(e); });
    const end = e => { if (!el._s.drag) return; el._s.drag = false; el.classList.remove('drag'); try { el.releasePointerCapture(e.pointerId); } catch (x) { } stripTip(e); };
    el.addEventListener('pointerup', end); el.addEventListener('pointercancel', end);
    el.addEventListener('pointerleave', () => { if (!el._s.drag) el.classList.remove('tip'); });
    el.addEventListener('wheel', e => { e.preventDefault(); container.scrollTop += e.deltaMode === 1 ? e.deltaY * ROW_H : e.deltaMode === 2 ? e.deltaY * container.clientHeight : e.deltaY; }, { passive: false });
    return el;
}
/* The strip's coordinates: H pixels stand for the container's whole scroll extent. */
function stripGeom(t) {
    const H = container.clientHeight, ch = H, th = thead.offsetHeight;
    return { H, ch, th, sh: Math.max(ch, th + t.filteredData.length * ROW_H) };
}
const rowY = (g, i) => (g.th + i * ROW_H) / g.sh * g.H;

/* Position and size: from syncSpace(), at every render, resize and width change. */
function stripLayout(t) {
    const el = stripEl();
    if (!t || !t.loaded || !t.filteredData.length || !tabs.length) { el.style.display = 'none'; el._s.key = null; return; }
    /* Over the native vertical scrollbar, whatever its width (24 px from app.css, but a
       platform may draw overlay scrollbars of no width: then the band takes STRIP_W
       at the right edge and syncSpace() narrows the grid layer to leave it room). */
    const rc = container.getBoundingClientRect(), H = container.clientHeight, cw = container.clientWidth;
    const sbw = Math.round(rc.width - cw), w = sbw >= 16 ? sbw : STRIP_W;
    const left = Math.round(rc.right - w), top = Math.round(rc.top);
    if (el._s.left !== left || el._s.top !== top || el._s.H !== H || el._s.w !== w) {
        Object.assign(el._s, { left, top, H, w });
        Object.assign(el.style, { left: left + 'px', top: top + 'px', height: H + 'px', width: w + 'px' });
        el.children[1].style.right = '0px';
    }
    el.style.display = '';
    stripUpdate(t);
    stripFollow();
}
/* The thumb: from syncLayer(), at every scroll. */
function stripFollow() {
    const el = document.getElementById('scroll-strip'), t = T();
    if (!el || el.style.display === 'none' || !t || !t.loaded) return;
    const g = stripGeom(t), thumb = el.children[3];
    const h = Math.max(THUMB_MIN, g.ch / g.sh * g.H), top = g.sh > g.ch ? container.scrollTop / (g.sh - g.ch) * (g.H - h) : 0;
    thumb.style.top = top + 'px'; thumb.style.height = h + 'px';
}
/* Pointer y → the row there, and the scroll that centres the viewport on it. */
function stripRowAt(t, e) {
    const el = stripEl(), g = stripGeom(t), y = Math.min(g.H, Math.max(0, e.clientY - el._s.top));
    const i = Math.min(t.filteredData.length - 1, Math.max(0, Math.floor((y / g.H * g.sh - g.th) / ROW_H)));
    return { i, y, scroll: Math.min(g.sh - g.ch, Math.max(0, y / g.H * g.sh - g.ch / 2)) };
}
function stripJump(e) {
    const t = T(), el = stripEl(); if (!t || !t.loaded || !t.filteredData.length) return;
    const at = stripRowAt(t, e), g = stripGeom(t);
    if (el._s.grab != null) {                 // dragging the thumb: its top follows the pointer, offset kept
        const h = Math.max(THUMB_MIN, g.ch / g.sh * g.H), top = Math.min(g.H - h, Math.max(0, at.y - el._s.grab));
        container.scrollTop = g.H > h ? top / (g.H - h) * (g.sh - g.ch) : 0;
    } else container.scrollTop = at.scroll;
    renderOnScroll();                         // now, not at the scroll event: the tip below reads the new position
    stripTip(e);
}
function stripTip(e) {
    const t = T(), el = stripEl(); if (!t || !t.loaded || !t.filteredData.length) return;
    const at = stripRowAt(t, e), r = t.filteredData[at.i], s = el._s;
    let v = '';
    if (s.col >= 0) { const x = cellStr(cellOf(r, s.col)); v = x ? ` · ${t.headers[s.col]}: ${x.length > 40 ? x.slice(0, 39) + '…' : x}` : ''; }
    el.children[4].textContent = `Row ${fmt(r.id)}${v}`;
    el.children[4].style.top = Math.min(s.H - 26, Math.max(0, at.y - 12)) + 'px';
    el.classList.add('tip');
}

/* Marks, labels and the sort column, once per state. */
function stripUpdate(t, force) {
    const el = stripEl(), s = el._s, g = stripGeom(t);
    const key = [t.id, t.filteredData, t.filteredData.length, t.modificationsLog.length, t.rowMark, t.dupMarks, t.sort, t.headers.length, g.H, g.sh, currentTheme()];
    if (!force && s.key && key.every((x, i) => x === s.key[i])) { stripSel(t); return; }
    s.key = key;
    const col = t.sort ? t.sort[0].col : -1;
    s.col = col; s.kind = col >= 0 ? sortKind(t, col) : '';
    stripMarks(t, g);
    stripLabels(t, g);
    stripSel(t);
}
function stripColor(name) { return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#888'; }
function stripMarks(t, g) {
    const el = stripEl(), cv = el.children[0], dpr = window.devicePixelRatio || 1, W = el._s.w || STRIP_W;
    cv.width = Math.round(W * dpr); cv.height = Math.round(g.H * dpr);
    cv.style.width = W + 'px'; cv.style.height = g.H + 'px';
    const ctx = cv.getContext('2d'); ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, STRIP_W, g.H);
    /* One band per pixel row, the strongest kind winning: 1 irregular, 2 duplicate, 3 marked. */
    const H = Math.ceil(g.H), bands = new Uint8Array(H + 1), rows = t.filteredData, n = t.headers.length;
    const mk = t.rowMark && t.rowMark.rows, dg = t.dupMarks && t.dupMarks.group, scale = g.H / g.sh;
    /* Irregular rows exist only when the scan found records of another width (base.irr):
       a row born here or edited has the header's width, a short one was short in the file.
       Nothing to mark — the usual case — and the rows are not visited at all: that pass
       (~6 ns a row, 9 ms on 1.4 M rows) ran at every edit and every resize event. */
    const irrAll = t.onlyIrregular, irr = !irrAll && !!(t.base && t.base.irr);
    if (!mk && !dg && !irr) return;
    for (let i = 0; i < rows.length; i++) {
        const r = rows[i];
        let k = 0;
        if (mk && mk.has(r)) k = 3; else if (dg && dg.has(r)) k = 2; else if (irr && r.len !== n) k = 1;
        if (!k) continue;
        const y = Math.floor((g.th + i * ROW_H) * scale);
        if (bands[y] < k) bands[y] = k;
    }
    const colors = ['', stripColor('--warn'), stripColor('--violet-txt'), stripColor('--orange-txt')];
    let from = -1, kind = 0;
    const flush = to => { if (kind) { ctx.fillStyle = colors[kind]; ctx.fillRect(3, from, W - 6, Math.max(1.5, to - from)); } };
    for (let y = 0; y <= H; y++) {
        const k = bands[y];
        if (k !== kind) { flush(y); from = y; kind = k; }
    }
    flush(H + 1);
}
const stripCompact = new Intl.NumberFormat('fr-FR', { notation: 'compact', maximumFractionDigits: 1 });
/* Row numbers along the band at a step that is a power of ten — the smallest whose
   ticks sit LABEL_STEP apart, from the geometry itself (a step of k rows is
   k × ROW_H × H / sh pixels): 10, 20 … 150 on 150 rows, 1k … 5k on 5 000, 100k … 1,4M
   on 1.4 M. Every multiple of the step is written, decimals included (1,1M after 1M),
   and the last one, at the band's very end, is held inside it. Steps of 2 or 5 × 10^n
   (500, 1k, 1,5k…) were tried and refused: multiples of a power of ten only — except
   that under 5 ticks the step is halved (1 600 rows: 500, 1k, 1,5k rather than a lone
   1k), which always fits, the ticks being then over H / 10 apart. */
function tickStep(g, n) {
    const px = k => k * ROW_H * g.H / g.sh;   // a step of k rows, in pixels
    let step = 10 ** Math.max(0, Math.ceil(Math.log10(LABEL_STEP / px(1)) - 1e-9));
    if (step >= 10 && n < 5 * step && px(step / 2) >= LABEL_STEP) step /= 2;
    return step;
}
function stripLabels(t, g) {
    const el = stripEl(), box = el.children[1], s = el._s, rows = t.filteredData;
    const out = [];
    let last = null;
    if (s.col < 0) {                          // not sorted: round row positions
        const step = tickStep(g, rows.length);
        for (let k = step; k <= rows.length; k += step) {
            const y = Math.min(g.H - 6, rowY(g, k - 1));
            if (y < 8) continue;
            const label = stripCompact.format(k).replace(/\s/g, '');
            out.push(`<span style="top: ${y}px" title="${esc(label)}">${esc(label)}</span>`);
        }
        box.innerHTML = out.join(''); box.classList.remove('sorted');
        return;
    }
    for (let y = 8; y < g.H - 10; y += LABEL_STEP) {
        const i = Math.min(rows.length - 1, Math.max(0, Math.floor((y / g.H * g.sh - g.th) / ROW_H)));
        let label = '';
        if (s.col >= 0) {
            const v = cellStr(cellOf(rows[i], s.col)).trim();
            if (!v) label = '';
            else if (s.kind === 'n') { const x = numKey(v); label = isNaN(x) ? '' : stripCompact.format(x).replace(/\s/g, ''); }
            else if (s.kind === 'd') { const m = v.match(/(\d{4})/); label = m ? m[1] : ''; }
            else label = removeAccents(v[0].toUpperCase());
        }
        if (label === '' || label === last) continue;
        last = label;
        out.push(`<span style="top: ${y}px" title="${esc(label)}">${esc(label)}</span>`);
    }
    box.innerHTML = out.join('');
    box.classList.toggle('sorted', s.col >= 0);
}
/* The selection's rows as a band: from paintSel(), at every selection change. */
function stripSel(t) {
    const el = document.getElementById('scroll-strip'); if (!el || el.style.display === 'none' || !t) return;
    const band = el.children[2], rg = selRange(t);
    if (!rg) { band.style.display = 'none'; return; }
    const g = stripGeom(t), y0 = rowY(g, rg.r0), y1 = rowY(g, rg.r1 + 1);
    band.style.display = ''; band.style.top = y0 + 'px'; band.style.height = Math.max(2, y1 - y0) + 'px';
}
