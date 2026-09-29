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
document.getElementById('use-expr').onchange = () => { updateExprUI(); applyFilters(); };
function toggleExprFilter() {
    const cb = document.getElementById('use-expr');
    cb.checked = !cb.checked;
    updateExprUI(); applyFilters();
    if (cb.checked) document.getElementById('global-search').focus();
}
