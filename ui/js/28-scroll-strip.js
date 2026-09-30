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
    if (!t || !t.loaded || !t.filteredData.length || !tabs.length) { el.style.display = 'none'; el._s.key = null; hstripLayout(null); return; }
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
    hstripLayout(t, rc);
}
/* The thumb: from syncLayer(), at every scroll. */
function stripFollow() {
    const el = document.getElementById('scroll-strip'), t = T();
    hstripFollow(t);
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
    hstripSel(t);
    const el = document.getElementById('scroll-strip'); if (!el || el.style.display === 'none' || !t) return;
    const band = el.children[2], rg = selRange(t);
    if (!rg) { band.style.display = 'none'; return; }
    const g = stripGeom(t), y0 = rowY(g, rg.r0), y1 = rowY(g, rg.r1 + 1);
    band.style.display = ''; band.style.top = y0 + 'px'; band.style.height = Math.max(2, y1 - y0) + 'px';
}

/* ---------------------------------------------------------------
   HORIZONTAL STRIP
   The same band over the grid's horizontal scrollbar (#hscroll-strip,
   24 px tall, shown only when the grid is wider than the viewport):
   a thumb for the viewport, dragged or the track clicked to jump, a
   tip naming the column under the pointer (its number when the file
   has no title line), the columns laid along the band as labels — each
   at the span its width takes, named when the span is wide enough, the
   sort column in the accent colour — and the selected columns as a band.
   W pixels stand for the grid's whole width, row numbers included, so
   the thumb agrees with the native scrollbar underneath.
----------------------------------------------------------------*/
const HLABEL_MIN = 44, HNO_MIN = 14;      // a span at least this wide takes the column's name; a narrower one its number

function hstripEl() {
    let el = document.getElementById('hscroll-strip');
    if (el) return el;
    el = document.createElement('div'); el.id = 'hscroll-strip';
    el.innerHTML = '<div class="ss-labels"></div><div class="ss-sel"></div><div class="ss-thumb"></div><div class="ss-tip"></div>';
    el._s = { key: null, drag: false };
    document.body.appendChild(el);
    el.addEventListener('pointerdown', e => {
        if (e.button !== 0) return;
        e.preventDefault(); el.setPointerCapture(e.pointerId); el._s.drag = true; el.classList.add('drag');
        const th = el.children[2], x = e.clientX - el._s.left, left = parseFloat(th.style.left) || 0, w = parseFloat(th.style.width) || 0;
        el._s.grab = x >= left && x <= left + w ? x - left : null;
        hstripJump(e);
    });
    el.addEventListener('pointermove', e => { if (el._s.drag) hstripJump(e); else hstripTip(e); });
    const end = e => { if (!el._s.drag) return; el._s.drag = false; el.classList.remove('drag'); try { el.releasePointerCapture(e.pointerId); } catch (x) { } hstripTip(e); };
    el.addEventListener('pointerup', end); el.addEventListener('pointercancel', end);
    el.addEventListener('pointerleave', () => { if (!el._s.drag) el.classList.remove('tip'); });
    /* A wheel over the band scrolls sideways, a plain wheel too (as with Shift held over the grid). */
    el.addEventListener('wheel', e => { e.preventDefault(); const d = e.deltaX || e.deltaY; container.scrollLeft += e.deltaMode === 1 ? d * 40 : e.deltaMode === 2 ? d * container.clientWidth : d; }, { passive: false });
    return el;
}
/* W pixels stand for the grid's width sw (the pinned widths' sum, or the scroll extent while widths are unknown). */
function hstripGeom(t) {
    const W = container.clientWidth, cw = W, L = colLayout(t);
    return { W, cw, L, sw: Math.max(cw, L ? L.x[L.x.length - 1] : container.scrollWidth) };
}
const colX = (g, x) => x / g.sw * g.W;

/* Position and size: from stripLayout(), with the container's rect it measured. */
function hstripLayout(t, rc) {
    const el = hstripEl();
    if (!t) { el.style.display = 'none'; el._s.key = null; return; }
    const g = hstripGeom(t);
    if (g.sw <= g.cw) { el.style.display = 'none'; el._s.key = null; return; }   // no sideways scroll: no bar, no band
    const sbh = Math.round(rc.height - container.clientHeight), h = sbh >= 16 ? sbh : STRIP_W;
    const left = Math.round(rc.left), top = Math.round(rc.bottom - h), W = g.W;
    if (el._s.left !== left || el._s.top !== top || el._s.W !== W || el._s.h !== h) {
        Object.assign(el._s, { left, top, W, h });
        Object.assign(el.style, { left: left + 'px', top: top + 'px', width: W + 'px', height: h + 'px' });
    }
    el.style.display = '';
    el._s.sw = g.sw;                          // for the thumb, which then needs no column layout at each scroll
    hstripUpdate(t, g);
    hstripFollow(t);
}
/* The thumb: from stripFollow(), at every scroll. */
function hstripFollow(t) {
    const el = document.getElementById('hscroll-strip');
    if (!el || el.style.display === 'none' || !t || !t.loaded) return;
    const g = { W: el._s.W, cw: el._s.W, sw: el._s.sw }, thumb = el.children[2];
    const w = Math.max(THUMB_MIN, g.cw / g.sw * g.W), left = g.sw > g.cw ? container.scrollLeft / (g.sw - g.cw) * (g.W - w) : 0;
    thumb.style.left = left + 'px'; thumb.style.width = w + 'px';
}
/* Pointer x → the column there (k, into the visible columns), and the scroll that centres the viewport on it. */
function hstripColAt(t, e) {
    const el = hstripEl(), g = hstripGeom(t), x = Math.min(g.W, Math.max(0, e.clientX - el._s.left)), cx = x / g.W * g.sw;
    let k = -1;
    if (g.L) { k = 0; while (k < g.L.vis.length - 1 && g.L.x[k + 1] <= cx) k++; }
    return { k, x, g, scroll: Math.min(g.sw - g.cw, Math.max(0, cx - g.cw / 2)) };
}
function hstripJump(e) {
    const t = T(), el = hstripEl(); if (!t || !t.loaded) return;
    const at = hstripColAt(t, e), g = at.g;
    if (el._s.grab != null) {
        const w = Math.max(THUMB_MIN, g.cw / g.sw * g.W), left = Math.min(g.W - w, Math.max(0, at.x - el._s.grab));
        container.scrollLeft = g.W > w ? left / (g.W - w) * (g.sw - g.cw) : 0;
    } else container.scrollLeft = at.scroll;
    renderOnScroll();
    hstripTip(e);
}
/* The column's name, or its number when the file has no title line (the headers are then 0, 1, 2…). */
function hstripName(t, c) { return t.syntheticHeader ? `Column ${t.headers[c]}` : cellStr(t.headers[c]) || `Column ${c + 1}`; }
function hstripTip(e) {
    const t = T(), el = hstripEl(); if (!t || !t.loaded) return;
    const at = hstripColAt(t, e), s = el._s, tip = el.children[3];
    if (at.k < 0) return;
    const c = at.g.L.vis[at.k], name = hstripName(t, c);
    tip.textContent = name.length > 60 ? name.slice(0, 59) + '…' : name;
    el.classList.add('tip');
    const tw = tip.offsetWidth;               // centred on the pointer, held inside the band
    tip.style.left = Math.min(s.W - tw, Math.max(0, at.x - tw / 2)) + 'px';
}
/* Labels and the sort column, once per state (the widths, the visible columns, the headers, the sort). */
function hstripUpdate(t, g) {
    const el = hstripEl(), s = el._s;
    const key = [t.id, t.headers, g.L && g.L.x.join(','), g.W, g.sw, t.syntheticHeader, (t.sort || []).map(k => k.col).join(','), currentTheme()];
    if (s.key && key.every((x, i) => x === s.key[i])) { hstripSel(t); return; }
    s.key = key;
    hstripLabels(t, g);
    hstripSel(t);
}
function hstripLabels(t, g) {
    const box = hstripEl().children[0], L = g.L, out = [];
    if (L) {
        const srt = new Set((t.sort || []).map(k => k.col));
        for (let k = 0; k < L.vis.length; k++) {
            const x0 = colX(g, L.x[k]), x1 = colX(g, L.x[k + 1]), w = x1 - x0;
            if (w < 3) continue;              // too narrow for even a separator
            /* The name when it has room, else the column's number as the header shows it
               (.col-no, from 0) — on a file of 85 columns the band is a row of numbers. */
            const c = L.vis[k], no = t.syntheticHeader || w < HLABEL_MIN;
            const label = w < HNO_MIN ? '' : no ? String(t.syntheticHeader ? t.headers[c] : c) : hstripName(t, c);
            out.push(`<span class="${srt.has(c) ? 'srt' : ''}${no ? ' no' : ''}" style="left: ${x0.toFixed(1)}px; width: ${w.toFixed(1)}px" title="${esc(hstripName(t, c))}">${esc(label)}</span>`);
        }
    }
    box.innerHTML = out.join('');
}
/* The selected columns as a band at the top: from stripSel(), at every selection change. */
function hstripSel(t) {
    const el = document.getElementById('hscroll-strip'); if (!el || el.style.display === 'none' || !t) return;
    const band = el.children[1], rg = selRange(t), g = hstripGeom(t);
    if (!rg || !g.L) { band.style.display = 'none'; return; }
    let x0 = Infinity, x1 = -Infinity;        // the visible columns of the range (hidden ones take no room)
    for (let k = 0; k < g.L.vis.length; k++) { const c = g.L.vis[k]; if (c >= rg.c0 && c <= rg.c1) { x0 = Math.min(x0, g.L.x[k]); x1 = Math.max(x1, g.L.x[k + 1]); } }
    if (x0 === Infinity) { band.style.display = 'none'; return; }
    band.style.display = ''; band.style.left = colX(g, x0) + 'px'; band.style.width = Math.max(2, colX(g, x1) - colX(g, x0)) + 'px';
}
