/* ---------------------------------------------------------------
   ACTIVE FILTERS
   One chip per filter in force, in the toolbar's free room right of
   the search options: the search box (with its regex / accents /
   invert / expression), each column's text filter, each column's
   value filter (from the panel, a chart or a right click), the
   irregular rows, the duplicates, the row marks shown alone — each
   with its × to drop that one, and Clear all when there are several.
   Why: some filters are set out of sight (a chart's click, the panel,
   a status bar chip), and the rows they hide went unexplained.
   Kept up to date by updateStats() (every filtering, tab switch) in
   place: a chip still there keeps its element, only its text changes —
   rebuilt whole, every chip replayed its pop-in at each key typed in a
   filter, a blink. Only a new chip pops in.
----------------------------------------------------------------*/
/* updateStats() runs during the boot run, before this file's top level: functions only, no const read there. */
function fchipCut(s) { s = String(s); return s.length > 28 ? s.slice(0, 27) + '…' : s; }   // a value's beginning (the whole in the tooltip)
function renderFilterChips(t) {
    const box = document.getElementById('fchips');
    const chips = [];                     // [kind, arg, label HTML, tooltip]
    if (t && t.loaded) {
        const q = t.globalQuery;
        if (q) {
            const flags = [t.useExpr ? 'expression' : '', !t.useExpr && t.useRegex ? 'regex' : '', !t.useExpr && t.useSlug ? 'accents ignored' : '', t.useReverse ? 'inverted' : ''].filter(Boolean);
            chips.push(['g', 0, `<b>${t.useExpr ? 'Expression' : 'Search'}</b> ${esc(fchipCut(q))}`,   // its options: the toggles beside, and the tooltip
                `${t.useExpr ? 'Rows where' : 'Rows holding'}: ${q}${flags.length ? ' (' + flags.join(', ') + ')' : ''}`]);
        }
        for (const c of Object.keys(t.colFilters)) if (t.colFilters[c])
            chips.push(['c', +c, `<b>${esc(fchipCut(t.headers[c]))}</b> ${esc(fchipCut(t.colFilters[c]))}${t.useReverse ? ' <i>inverted</i>' : ''}`, `Column "${t.headers[c]}" holds: ${t.colFilters[c]}`]);
        for (const c of Object.keys(t.valFilters)) {
            const ex = t.valFilters[c]; if (!ex || !ex.size) continue;
            const only = ex.size === 1 ? [...ex][0] : null;
            chips.push(['v', +c, `<b>${esc(fchipCut(t.headers[c]))}</b> ${only === '' ? 'not empty' : only !== null ? `≠ ${esc(fchipCut(only))}` : `${fmt(ex.size)} values hidden`}`,
                `Column "${t.headers[c]}": ${fmt(ex.size)} value${ex.size > 1 ? 's' : ''} hidden — set in its panel (▾)`]);
        }
        if (t.onlyIrregular) chips.push(['i', 0, '<b>Irregular rows</b> only', 'Only the rows whose field count differs from the header']);
        if (t.onlyDups) chips.push(['d', 0, '<b>Duplicates</b> only', 'Only the duplicate rows, by group']);
        if (t.rowMark && t.rowMark.only) chips.push(['m', 0, `<b>${esc(fchipCut(t.rowMark.label || 'Marked rows'))}</b> only`, `Only the marked rows: ${t.rowMark.label || ''}`]);
    }
    const want = chips.map(([k, a, label, tip]) => ({ key: k + a, k, a, label, tip }));
    if (chips.length > 1) want.push({ key: 'all' });
    const sig = want.map(w => w.key + '\u0001' + (w.label || '') + '\u0001' + (w.tip || '')).join('\u0002');
    if (box._sig === sig) return;
    box._sig = sig;
    const have = new Map([...box.children].map(el => [el.dataset.k, el]));
    want.forEach((w, i) => {
        let el = have.get(w.key);
        if (el) have.delete(w.key);
        else {
            el = document.createElement('span'); el.dataset.k = w.key;
            if (w.key === 'all') { el.className = 'fchip-all'; el.title = 'Clear every filter: show every row'; el.onclick = clearAllFilters; }
            else { el.className = 'fchip'; el.innerHTML = `<span class="fc-l"></span><span class="fc-x" title="Remove this filter"></span>`; el.lastChild.onclick = () => fchipDrop(w.k, w.a); }
        }
        if (w.key !== 'all') {
            const l = el.firstChild; if (l._h !== w.label) { l._h = w.label; l.innerHTML = w.label; }   // the label: escaped HTML built above
            if (el._tip !== w.tip) { el._tip = w.tip; el.title = w.tip; delete el.dataset.t; }   // a tooltip already shown moved it to data-t (34-…)
        }
        if (box.children[i] !== el) box.insertBefore(el, box.children[i] || null);   // moved only when out of place: a move replays the pop-in
    });
    for (const el of have.values()) el.remove();
}
function fchipDrop(kind, a) {
    const t = T(); if (!t || !t.loaded) return;
    if (kind === 'g') { document.getElementById('global-search').value = ''; t.lastFilter = null; applyFilters(); }
    else if (kind === 'c') { delete t.colFilters[a]; t.lastFilter = null; renderHeader(); applyFilters(); }
    else if (kind === 'v') { delete t.valFilters[a]; renderHeader(); applyColStyles(); applyFilters(); }
    else if (kind === 'i') toggleIrregular();
    else if (kind === 'd') toggleDupView();
    else if (kind === 'm') toggleMarkView();
}
