/* ---------------------------------------------------------------
   SORT IN MOTION
   A sort clicked on a title shows every row moving between its old
   and its new place: the rows shown after it come from where they
   were (on screen, or beyond the edge they were past), the rows that
   were on screen leave toward their new places — up or down, at
   speeds from their distance in the file, so they cross — woven, one
   row in two passing in front, the other behind. Arrivals
   come in fast, departures start slowly: the screen is never empty,
   and a faint copy of the old screen fills what gaps remain.
   Nothing is added to the sort itself but a note of the rows on
   screen (~30 elements) and of the old place of the new first rows
   (the sort's own `order`): the motion starts in the next frame,
   before it is drawn. Leaving rows and the copy are elements in
   #tbody without data-idx — every row walker of the grid looks for
   .row[data-idx] — removed when the motion ends, or by any redraw.
   A transform runs on the compositor; the rows' own top stays the
   truth. Off under prefers-reduced-motion. The versions the user
   turned down are in .claude/rules/ui-look.md (Motion).
----------------------------------------------------------------*/
const SORT_MS = 280;            // 560, then 380 (+ up to 200, then 120, by distance): the user found them slow
/* Opposite curves, so the screen is never empty: the rows arriving come in fast and settle (they
   are on screen at once), the rows leaving start slowly (they stay while the others cross them). */
const SORT_IN = 'cubic-bezier(.12, .7, .25, 1)', SORT_OUT = 'cubic-bezier(.6, 0, .8, .4)';
const sortCalm = matchMedia('(prefers-reduced-motion: reduce)');
/* Before the sort: the rows on screen — their element, row and top in the viewport. */
function sortMotionBefore(t) {
    if (sortCalm.matches || !drawn || drawn.t !== t) return null;
    const top = container.scrollTop, h = container.clientHeight, on = [];
    for (const el of tbody.children) {
        if (el.dataset.idx == null) continue;
        const i = +el.dataset.idx, y = i * ROW_H - top;
        if (y > -ROW_H && y < h) on.push({ el, r: t.filteredData[i], y });
    }
    /* Every row shown (no filter): a row's old place in the view is its old index in allData,
       which the sort's `order` holds for the new first rows (sortMotionFrom). */
    return on.length ? { on, top, all: t.filteredData === t.allData, from: null } : null;
}
/* The old index of the first rows of the new order (enough for a screen). */
function sortMotionFrom(m, order) {
    if (m && m.all) m.from = order.slice(0, Math.ceil(container.clientHeight / ROW_H) + 2);
}
function sortMotionAfter(t, m) {
    if (!m) return;
    const rows = t.filteredData;
    requestAnimationFrame(() => { if (t.filteredData === rows && T() === t) sortMotionRun(t, m); });
}
/* Beyond an edge, a place in the file far away is drawn compressed: d rows past the edge are
   0.5 + 1.3 × log2(1 + d) rows out. A row coming from (or going to) far away moves faster than one
   from just past the edge, so the rows spread and cross instead of moving as one block. */
const sortOff = d => ROW_H * (.5 + 1.3 * Math.log2(1 + Math.max(0, d) / ROW_H));
function sortMotionRun(t, m) {
    if (!drawn || drawn.t !== t) return;
    const top = container.scrollTop, h = container.clientHeight, fresh = new Set();
    const onScreen = y => y > -ROW_H && y < h;
    const screen = (y, k) => y == null ? h + sortOff(ROW_H * (4 + k)) : y >= h ? h + sortOff(y - h) : y < 0 ? -ROW_H - sortOff(-y) : y;
    /* The new rows on screen, from their old place: on screen before, or beyond the edge they
       were past. */
    const before = new Map(m.on.map(o => [o.r, o.y]));
    const moves = [];
    let k = 0;
    for (const el of tbody.children) {
        if (el.dataset.idx == null) continue;
        const i = +el.dataset.idx, y = i * ROW_H - top;
        if (!onScreen(y)) continue;
        const r = t.filteredData[i];
        fresh.add(r);
        const old = before.has(r) ? before.get(r) : m.from && i < m.from.length ? m.from[i] * ROW_H - m.top : null;
        moves.push({ el, y, from: before.has(r) ? old : screen(old, k++) });
    }
    /* Under it all, the screen as it was, faint and fading: rows crossing leave gaps, which show it
       rather than the empty grid. */
    const ghost = m.on.map(o => { const g = o.el.cloneNode(true); g.removeAttribute('data-idx'); g.classList.add('leaving'); g.style.top = (o.y + top) + 'px'; return g; });
    tbody.prepend(...ghost);
    for (const g of ghost) g.animate([{ opacity: .5 }, { opacity: 0 }], { duration: SORT_MS + 100, easing: 'ease-in', fill: 'forwards' });
    const dur = d => SORT_MS + Math.min(120, Math.abs(d) / h * 90);
    /* The rows that were on screen and are not now leave toward their new places (id − 1 once
       renumbered, when every row is shown), over the faint screen, under the rows arriving. */
    let at = ghost[ghost.length - 1];
    const out = m.on.filter(o => !fresh.has(o.r));
    out.forEach((o, n) => {
        const el = o.el, d = screen(m.all ? (o.r.id - 1) * ROW_H - top : null, n) - o.y;
        el.removeAttribute('data-idx'); el.classList.add('leaving'); el.style.top = (o.y + top) + 'px';
        el.style.zIndex = n % 2 ? 2 : 3;      // woven: see below
        at.after(el); at = el;
        el.animate([{ transform: 'none' }, { transform: `translateY(${d}px)` }], { duration: dur(d), easing: SORT_OUT, fill: 'forwards' });
    });
    /* Woven, as cards riffled: one row arriving in two passes over everything (z 4), the other
       under everything (1); the rows leaving alternate between (3, 2) — so where rows cross, now
       the one going up passes in front, now the one going down. Their stacking goes at the end. */
    let last = null, longest = 0, w = 0;
    for (const x of moves) {
        const d = x.from - x.y;
        if (Math.abs(d) < 1) continue;
        const el = x.el;
        el.style.zIndex = w++ % 2 ? 1 : 4;
        const a = el.animate([{ transform: `translateY(${d}px)` }, { transform: 'none' }], { duration: dur(d), easing: SORT_IN });
        a.onfinish = a.oncancel = () => { el.style.zIndex = ''; };
        if (dur(d) >= longest) { longest = dur(d); last = a; }
    }
    const gone = () => { for (const g of ghost) g.remove(); for (const o of out) o.el.remove(); };
    if (last) last.addEventListener('finish', gone); else setTimeout(gone, SORT_MS);
    if (last) last.addEventListener('cancel', gone);
}
