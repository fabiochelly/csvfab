/* ---------------------------------------------------------------
   FILTER BY EXPRESSION
   The "Expression" box of the toolbar turns the search field into a
   row test written like a formula (17-formula.js): {Column} is the
   row's cell, the helpers are the same — num({Amount}) > 1000 &&
   year({Date}) === 2024. The column filters still apply on top, and
   Invert flips the whole test. A formula that does not compile leaves
   the view as it was and says why in the status bar; a row whose
   evaluation throws is hidden and counted.
----------------------------------------------------------------*/
const EXPR_PLACEHOLDER = 'e.g.  num({Amount}) > 1000 && contains({City}, "lyon")';
const FX_VALUES = Object.values(FX);

/* { test(row) → boolean, errors, first } or { error } */
function exprRowTest(t, src) {
    const c = compileFormula(t, src);
    if (c.error !== undefined) return { error: c.error || 'empty expression' };
    const n = t.headers.length, run = { errors: 0, first: '' };
    run.test = row => {
        const d = row.data, dd = (d.length >= n ? d : pad(d, n)).map(cellStr);
        try { return !!c.fn(dd, row.id, ...FX_VALUES); }
        catch (e) { run.errors++; if (!run.first) run.first = e.message; return false; }
    };
    return run;
}
function markExprBox(bad) { document.getElementById('global-search').classList.toggle('bad', !!bad); }
/* The search box says what it expects. */
function updateExprUI() {
    const on = document.getElementById('use-expr').checked, box = document.getElementById('global-search');
    box.placeholder = on ? EXPR_PLACEHOLDER : 'Filter...';
    box.classList.toggle('expr', on);
    if (!on) markExprBox(false);
}
document.getElementById('use-expr').onchange = () => { updateExprUI(); exprAcClose(); applyFilters(); };

/* ---- Autocomplete ----------------------------------------------------
   Under the search box, in expression mode: inside an open brace the
   column names ({ alone lists them all), elsewhere the helper functions
   once a word is begun. Fuzzy-matched like the palette; ↑ ↓ move, Enter
   or Tab insert, Escape closes. The list is a fixed div under the box,
   kept open through a click on it (mousedown prevented, so the box does
   not blur before the click lands). */
const FX_SIG = { num: 'num(v)', date: 'date(v)', today: 'today()', days: 'days(a, b)', addDays: 'addDays(d, n)', year: 'year(d)', month: 'month(d)', day: 'day(d)',
    fmtDate: 'fmtDate(d, "dd/mm/yyyy")', round: 'round(n, d)', fixed: 'fixed(n, d)', upper: 'upper(s)', lower: 'lower(s)', trim: 'trim(s)', capitalize: 'capitalize(s)',
    slug: 'slug(s)', len: 'len(s)', left: 'left(s, n)', right: 'right(s, n)', pad: 'pad(s, n, "0")', extract: 'extract(s, regex, group)', replace: 'replace(s, regex, by)',
    contains: 'contains(s, part)', empty: 'empty(v)', first: 'first(a, b, …)', join: 'join(sep, a, b, …)' };
const acBox = document.createElement('div'); acBox.id = 'expr-ac'; document.body.appendChild(acBox);
acBox.addEventListener('mousedown', e => e.preventDefault());          // keep the focus in the search box
let exprAc = null;      // { items: [{label, insert, kind, hits}], act, start, end }

function exprAcClose() { exprAc = null; acBox.classList.remove('open'); }
/* What sits before the caret decides the list: a fragment after an unclosed { → columns; a word → functions. */
function exprAcUpdate() {
    const t = T(), box = document.getElementById('global-search');
    if (!t || !t.loaded || !document.getElementById('use-expr').checked || document.activeElement !== box) return exprAcClose();
    const text = box.value, pos = box.selectionStart;
    const open = text.lastIndexOf('{', pos - 1), close = open >= 0 ? text.indexOf('}', open) : -1;
    let items = [], start, end = pos;
    const rank = (q, list) => (q ? list.map(x => { const f = fuzzy(q, x.label); return f && { ...x, hits: f.hits, score: f.score }; }).filter(Boolean).sort((a, b) => b.score - a.score) : list.map(x => ({ ...x, hits: [] }))).slice(0, 12);
    if (open >= 0 && (close < 0 || close >= pos)) {
        start = open;
        if (close === pos) end = pos + 1;                         // the caret sits before an existing }: replace it too
        const q = removeAccents(text.slice(open + 1, pos).trim().toLowerCase());
        items = rank(q, t.headers.map(h => ({ label: h, insert: `{${h}}`, kind: 'column' })));
    } else {
        const m = text.slice(0, pos).match(/[A-Za-z_]\w*$/);
        if (m && !/[\w}"']$/.test(text.slice(0, pos - m[0].length))) {      // a word begun, not the tail of something else
            start = pos - m[0].length;
            items = rank(m[0].toLowerCase(), Object.keys(FX).map(k => ({ label: k, insert: k + '(', kind: FX_SIG[k] || '' })));
        }
    }
    if (!items.length) return exprAcClose();
    exprAc = { items, act: 0, start, end };
    exprAcRender();
    const r = box.getBoundingClientRect();
    acBox.style.left = r.left + 'px'; acBox.style.top = (r.bottom + 4) + 'px'; acBox.style.minWidth = r.width + 'px';
    acBox.classList.add('open');
}
function exprAcRender() {
    const mark = (label, hits) => { const H = new Set(hits); return [...label].map((ch, i) => H.has(i) ? `<b>${esc(ch)}</b>` : esc(ch)).join(''); };
    acBox.innerHTML = exprAc.items.map((it, i) => `<div class="ac-item${i === exprAc.act ? ' act' : ''}" onmousemove="exprAc && (exprAc.act = ${i}, exprAcRender())" onclick="exprAcAccept(${i})">`
        + `<span class="ac-l">${it.kind === 'column' ? '{' + mark(it.label, it.hits.map(h => h)) + '}' : mark(it.label, it.hits)}</span><span class="ac-k">${esc(it.kind)}</span></div>`).join('');
    const a = acBox.querySelector('.ac-item.act'); if (a) a.scrollIntoView({ block: 'nearest' });
}
function exprAcAccept(i) {
    const it = exprAc && exprAc.items[i != null ? i : exprAc.act]; if (!it) return;
    const box = document.getElementById('global-search'), { start, end } = exprAc;
    box.setRangeText(it.insert, start, end, 'end');
    exprAcClose();
    box.dispatchEvent(new Event('input'));                           // filterSoon(), and a fresh look for the next suggestion
}
const searchBox = document.getElementById('global-search');
searchBox.addEventListener('input', exprAcUpdate);
searchBox.addEventListener('click', exprAcUpdate);
searchBox.addEventListener('blur', () => setTimeout(exprAcClose, 0));
searchBox.addEventListener('keydown', e => {
    if (!exprAc) { if (e.key === 'ArrowDown' && document.getElementById('use-expr').checked) { exprAcUpdate(); if (exprAc) e.preventDefault(); } return; }
    const n = exprAc.items.length;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); exprAc.act = (exprAc.act + (e.key === 'ArrowDown' ? 1 : -1) + n) % n; exprAcRender(); }
    else if (e.key === 'Enter' || e.key === 'Tab') { e.preventDefault(); e.stopPropagation(); exprAcAccept(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); exprAcClose(); }   // the list first, not the selection
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') setTimeout(exprAcUpdate, 0);
});
window.addEventListener('resize', exprAcClose);
function toggleExprFilter() {
    const cb = document.getElementById('use-expr');
    cb.checked = !cb.checked;
    updateExprUI(); applyFilters();
    if (cb.checked) document.getElementById('global-search').focus();
}
