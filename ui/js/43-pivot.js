/* ---------------------------------------------------------------
   PIVOT TABLE
   The rows shown, one column's values down the side, another's across
   the top (or none), and in each cell a count, sum, average, min, max or
   number of distinct values — with totals, a colour scale, dates grouped
   by year, quarter, month or day, and a click on any cell showing the rows
   it counts. Computed again at every change, in slices with a yield, so a
   file of millions of rows keeps the window responsive; only the latest
   run is drawn. Nothing in the tab changes: the result is copied, or
   opened as a new tab like Group by's.
----------------------------------------------------------------*/
const PV_SLICE = 100000;         // rows between two yields
const PV_SHOW_ROWS = 500;        // rows drawn in the dialog (all of them go to the copy and the new tab)
const PV_SHOW_COLS = 60;         // values across: beyond, the largest totals are drawn
const PV_FNS = [['count', 'Number of rows'], ['sum', 'Sum'], ['avg', 'Average'], ['min', 'Smallest'], ['max', 'Largest'], ['distinct', 'Distinct values']];
const PV_DATES = [['m', 'by month'], ['q', 'by quarter'], ['y', 'by year'], ['d', 'by day'], ['', 'as written']];
let pivot = null;                // { t, r, c, v, fn, rd, cd, heat, sort: {by, dir}, res, run }

function openPivot(col) {
    const t = T(); if (!t || !t.loaded || !t.headers.length) return;
    pivotDialog();
    const kinds = columnKinds(t);
    if (!pivot || pivot.t !== t || pivot.r >= t.headers.length) {
        const v = t.headers.findIndex((_, c) => c !== col && gbSumByDefault(t, c));
        pivot = { t, r: 0, c: -1, v, fn: v >= 0 ? 'sum' : 'count', rd: 'm', cd: 'm', heat: true, sort: { by: 'label', dir: 1 } };
        pivot.r = col != null ? col : splitGuess(t);
        /* Across: a second column of few values (a status, a type), when there is one. */
        const rows = t.filteredData.slice(0, 3000);
        for (let c = 0; c < t.headers.length && pivot.c < 0; c++) {
            if (c === pivot.r || c === v || kinds[c] === 'n') continue;
            const seen = new Set();
            for (const r of rows) { seen.add(cellStr(cellOf(r, c))); if (seen.size > 12) break; }
            if (seen.size >= 2 && seen.size <= 12) pivot.c = c;
        }
    } else if (col != null) pivot.r = col;
    const opt = (list, cur) => list.map(([v, l]) => `<option value="${v}"${String(v) === String(cur) ? ' selected' : ''}>${esc(l)}</option>`).join('');
    const cols = t.headers.map((h, i) => [i, h]);
    document.getElementById('pt-r').innerHTML = opt(cols, pivot.r);
    document.getElementById('pt-c').innerHTML = opt([[-1, '(nothing: one column)'], ...cols], pivot.c);
    document.getElementById('pt-v').innerHTML = opt([[-1, '(choose a column)'], ...cols], pivot.v);
    document.getElementById('pt-fn').innerHTML = opt(PV_FNS, pivot.fn);
    document.getElementById('pt-rd').innerHTML = opt(PV_DATES, pivot.rd);
    document.getElementById('pt-cd').innerHTML = opt(PV_DATES, pivot.cd);
    document.getElementById('pt-heat').checked = pivot.heat;
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-pivot').style.display = 'block';
    pivotRun();
}
function pivotDialog() { modalOnce('modal-pivot', `<div id="modal-pivot" class="modal-box">
    <h4>Pivot table</h4>
    <div class="pt-ctl">
        <label class="fld-label">Rows <select id="pt-r" class="bs-input bs-select" onchange="pivotChange()"></select></label>
        <select id="pt-rd" class="bs-input bs-select" onchange="pivotChange()" title="Dates grouped by"></select>
        <button class="btn btn-outline" onclick="pivotSwap()" title="Swap rows and columns" aria-label="Swap rows and columns">⇄</button>
        <label class="fld-label">Columns <select id="pt-c" class="bs-input bs-select" onchange="pivotChange()"></select></label>
        <select id="pt-cd" class="bs-input bs-select" onchange="pivotChange()" title="Dates grouped by"></select>
        <label class="fld-label">Show <select id="pt-fn" class="bs-input bs-select" onchange="pivotChange()"></select></label>
        <span class="fld-label" id="pt-of">of</span>
        <select id="pt-v" class="bs-input bs-select" onchange="pivotChange()"></select>
        <label class="col-label"><input type="checkbox" id="pt-heat" checked onchange="pivotHeat()"> Colour scale</label>
    </div>
    <div class="stat-line" id="pt-stats"></div>
    <div class="pv-wrap pt-wrap"><table class="pv pt" id="pt-table"></table></div>
    <div class="modal-actions" style="margin-top: 14px;">
        <button class="btn btn-outline" onclick="pivotCopy()" title="Tab-separated, to paste into a spreadsheet">Copy</button>
        <button class="btn btn-outline" onclick="pivotTab()">Open in a new tab</button>
        <button class="btn" onclick="closeAllModals()">Close</button>
    </div>
</div>`); }
function pivotRead() {
    const P = pivot, g = id => document.getElementById(id);
    P.r = +g('pt-r').value; P.c = +g('pt-c').value; P.v = +g('pt-v').value; P.fn = g('pt-fn').value;
    P.rd = g('pt-rd').value; P.cd = g('pt-cd').value; P.heat = g('pt-heat').checked;
    if (P.c === P.r) { P.c = -1; g('pt-c').value = -1; }
}
function pivotSwap() {
    const P = pivot; if (!P || P.c < 0) return;
    pivotRead();
    [P.r, P.c, P.rd, P.cd] = [P.c, P.r, P.cd, P.rd];
    P.sort = { by: 'label', dir: 1 };
    openPivot();
}
function pivotChange() { pivotRead(); if (typeof pivot.sort.by === 'number') pivot.sort = { by: 'label', dir: 1 }; pivotRun(); }
function pivotHeat() { pivot.heat = document.getElementById('pt-heat').checked; pivotDraw(); }

/* A date as its bucket: 2024-03-15 → 2024 / 2024-Q1 / 2024-03 / 2024-03-15, which sort as text in time order. */
function pivotBucket(v, how) {
    const k = dateKey(v);
    if (isNaN(k)) return v;
    const y = Math.floor(k / 1e10), m = Math.floor(k / 1e8) % 100, d = Math.floor(k / 1e6) % 100, p = n => String(n).padStart(2, '0');
    return how === 'y' ? String(y) : how === 'q' ? `${y}-Q${Math.ceil(m / 3)}` : how === 'm' ? `${y}-${p(m)}` : `${y}-${p(m)}-${p(d)}`;
}
/* The key of a row on column c: its value trimmed, or its date's bucket when the column holds dates. */
function pivotKeyFn(t, c, how) {
    if (c < 0) return () => '';
    const date = how && columnKinds(t)[c] === 'd';
    return date ? r => { const v = cellStr(cellOf(r, c)).trim(); return v ? pivotBucket(v, how) : ''; } : r => cellStr(cellOf(r, c)).trim();
}
const pvAcc = () => ({ n: 0, s: 0, mn: Infinity, mx: -Infinity, set: null });
function pvVal(a, fn) {
    if (!a) return null;
    switch (fn) {
        case 'count': return a.n;
        case 'sum': return a.n ? a.s : null;
        case 'avg': return a.n ? a.s / a.n : null;
        case 'min': return a.n ? a.mn : null;
        case 'max': return a.n ? a.mx : null;
        default: return a.set ? a.set.size : 0;
    }
}

async function pivotRun() {
    const P = pivot; if (!P) return;
    const t = P.t, run = P.run = {}, fn = P.fn, kinds = columnKinds(t);
    const stats = document.getElementById('pt-stats'), needV = fn !== 'count';
    const showDate = (sel, c) => { document.getElementById(sel).style.display = c >= 0 && kinds[c] === 'd' ? '' : 'none'; };
    showDate('pt-rd', P.r); showDate('pt-cd', P.c);
    document.getElementById('pt-v').style.display = needV ? '' : 'none';
    document.getElementById('pt-of').style.display = needV ? '' : 'none';
    if (needV && P.v < 0) { stats.innerHTML = '<span class="warn">Choose the column to compute.</span>'; document.getElementById('pt-table').innerHTML = ''; P.res = null; return; }
    const rKey = pivotKeyFn(t, P.r, P.rd), cKey = pivotKeyFn(t, P.c, P.cd), v = P.v;
    const num = fn === 'sum' || fn === 'avg' || fn === 'min' || fn === 'max';
    const cells = new Map(), rowT = new Map(), colT = new Map(), all = pvAcc();
    let skipped = 0, dec = fn === 'avg';
    const add = (a, x) => {
        a.n++;
        if (num) { a.s += x; if (x < a.mn) a.mn = x; if (x > a.mx) a.mx = x; }
        else if (fn === 'distinct') (a.set || (a.set = new Set())).add(x);
    };
    const rows = t.filteredData;
    for (let i0 = 0; i0 < rows.length; i0 += PV_SLICE) {
        visitRows(t, rows.slice(i0, i0 + PV_SLICE), r => {
            let x = 1;
            if (needV) {
                const s = cellStr(cellOf(r, v)).trim();
                if (!s) return;
                if (num) { x = numKey(s); if (x !== x) { skipped++; return; } if (!dec && x !== Math.floor(x)) dec = true; } else x = s;
            }
            const rk = rKey(r), ck = cKey(r);
            let line = cells.get(rk);
            if (!line) { cells.set(rk, line = new Map()); rowT.set(rk, pvAcc()); }
            let a = line.get(ck);
            if (!a) { line.set(ck, a = pvAcc()); if (!colT.has(ck)) colT.set(ck, pvAcc()); }
            add(a, x); add(rowT.get(rk), x); add(colT.get(ck), x); add(all, x);
        });
        if (rows.length > PV_SLICE) {
            stats.textContent = `Computing… ${Math.round(Math.min(1, (i0 + PV_SLICE) / rows.length) * 100)} %`;
            await new Promise(r => setTimeout(r, 0));
            if (pivot !== P || P.run !== run) return;
        }
    }
    const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' });
    const byLabel = (a, b) => (a === '') - (b === '') || coll.compare(a, b);    // empty last
    P.res = { cells, rowT, colT, all, skipped, dec, n: rows.length, colKeys: [...colT.keys()].sort(byLabel), coll, byLabel };
    pivotDraw();
}

/* The rows in the order asked: by label, by total, or by the values of column j across (sort.by = j —
   an index, never the value itself, which would have to sit in an onclick). */
function pivotRowKeys(P) {
    const R = P.res, keys = [...R.rowT.keys()], s = P.sort, fn = P.fn;
    if (s.by === 'label') return keys.sort((a, b) => s.dir * R.byLabel(a, b) || 0);
    const col = R.colKeys[s.by], val = s.by === 'total' ? k => pvVal(R.rowT.get(k), fn) : k => pvVal(R.cells.get(k).get(col), fn);
    return keys.sort((a, b) => { const x = val(a), y = val(b); return (x == null) - (y == null) || (x == null ? 0 : s.dir * (y - x)) || R.byLabel(a, b); });
}
function pivotSort(by) {
    const P = pivot; if (!P || !P.res) return;
    const dir = P.sort.by === by ? -P.sort.dir : 1;
    P.sort = { by, dir };
    pivotDraw();
}
const pvLabel = k => k === '' ? '<span class="muted">(empty)</span>' : esc(k);
/* Amounts with decimals all get two, as a spreadsheet's number format would (229 659,70, not 229 659,7). */
const PV_FMT2 = new Intl.NumberFormat('fr-FR', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
function pvNum(x, fn) {
    if (x == null) return '';
    if (pivot && pivot.res && pivot.res.dec && fn !== 'count' && fn !== 'distinct') return PV_FMT2.format(x);
    return fmt(Math.round(x * 1e6) / 1e6);
}

function pivotDraw() {
    const P = pivot; if (!P || !P.res) return;
    const R = P.res, t = P.t, fn = P.fn, rk = pivotRowKeys(P);
    P.view = { rk, ck: R.colKeys };
    /* Across, too many values to draw: those with the largest totals, kept in their order. */
    let ck = R.colKeys, ckIdx = ck.map((_, j) => j);
    if (ck.length > PV_SHOW_COLS) {
        const keep = new Set(ckIdx.slice().sort((a, b) => (pvVal(R.colT.get(ck[b]), fn) || 0) - (pvVal(R.colT.get(ck[a]), fn) || 0)).slice(0, PV_SHOW_COLS));
        ckIdx = ckIdx.filter(j => keep.has(j));
    }
    /* The colour scale runs from the smallest number drawn to the largest, as a spreadsheet's does:
       from zero, close values (sales by month) all came out the same shade. */
    let lo = Infinity, hi = -Infinity;
    for (const k of rk.slice(0, PV_SHOW_ROWS)) for (const j of ckIdx) { const x = pvVal(R.cells.get(k).get(ck[j]), fn); if (x != null) { if (x < lo) lo = x; if (x > hi) hi = x; } }
    const heat = x => P.heat && x != null && hi > -Infinity ? ` style="background: rgba(var(--accent-rgb), ${(0.06 + 0.6 * (hi > lo ? (x - lo) / (hi - lo) : 0.5)).toFixed(3)})"` : '';
    const arrow = by => P.sort.by === by ? (P.sort.dir > 0 ? (by === 'label' ? ' ▲' : ' ▼') : (by === 'label' ? ' ▼' : ' ▲')) : '';
    const across = P.c >= 0;
    let html = `<tr><th class="pt-corner" onclick="pivotSort('label')" title="Sort by ${esc(t.headers[P.r])}">${esc(t.headers[P.r])}${across ? ` <span class="muted">\\ ${esc(t.headers[P.c])}</span>` : ''}${arrow('label')}</th>`;
    if (across) html += ckIdx.map(j => `<th class="k" onclick="pivotSort(${j})" title="Sort the rows by this column">${pvLabel(ck[j])}${arrow(j)}</th>`).join('');
    html += `<th class="k pt-tot" onclick="pivotSort('total')" title="Sort the rows by their total">${across ? 'Total' : esc(PV_FNS.find(f => f[0] === fn)[1])}${arrow('total')}</th></tr>`;
    html += rk.slice(0, PV_SHOW_ROWS).map((k, i) => {
        const line = R.cells.get(k);
        return `<tr><th onclick="pivotDrill(${i}, -1)" title="Show these rows">${pvLabel(k)}</th>`
            + (across ? ckIdx.map(j => { const x = pvVal(line.get(ck[j]), fn); return `<td class="k"${heat(x)}${x != null ? ` onclick="pivotDrill(${i}, ${j})" title="Show these rows"` : ''}>${pvNum(x, fn)}</td>`; }).join('') : '')
            + `<td class="k pt-tot"${across ? '' : heat(pvVal(R.rowT.get(k), fn))} onclick="pivotDrill(${i}, -1)">${pvNum(pvVal(R.rowT.get(k), fn), fn)}</td></tr>`;
    }).join('');
    html += `<tr class="pt-sum"><th>Total</th>${across ? ckIdx.map(j => `<td class="k" onclick="pivotDrill(-1, ${j})" title="Show these rows">${pvNum(pvVal(R.colT.get(ck[j]), fn), fn)}</td>`).join('') : ''}<td class="k pt-tot">${pvNum(pvVal(R.all, fn), fn)}</td></tr>`;
    document.getElementById('pt-table').innerHTML = html;
    const notes = [];
    if (rk.length > PV_SHOW_ROWS) notes.push(`the first ${fmt(PV_SHOW_ROWS)} of ${fmt(rk.length)} rows drawn`);
    if (ckIdx.length < ck.length) notes.push(`the ${fmt(PV_SHOW_COLS)} largest of ${fmt(ck.length)} columns drawn`);
    if (R.skipped) notes.push(`${fmt(R.skipped)} value${R.skipped === 1 ? '' : 's'} not a number left out`);
    document.getElementById('pt-stats').innerHTML = `<b>${fmt(rk.length)}</b> × <b>${fmt(across ? ck.length : 1)}</b> from ${fmt(R.n)} ${hasFilter(t) ? 'filtered ' : ''}rows`
        + (notes.length ? ` <span class="muted">— ${notes.join(' · ')}; the copy and the new tab hold everything</span>` : '')
        + ' <span class="muted">· click a number to see its rows</span>';
}

/* The rows behind a cell (i: row of the view or -1 for all, j: column or -1), shown alone as row marks. */
function pivotDrill(i, j) {
    const P = pivot; if (!P || !P.res || T() !== P.t) return;
    const t = P.t, rk = i >= 0 ? P.view.rk[i] : null, ck = j >= 0 ? P.view.ck[j] : null;
    const rKey = pivotKeyFn(t, P.r, P.rd), cKey = pivotKeyFn(t, P.c, P.cd), needV = P.fn !== 'count', v = P.v;
    const out = new Set();
    visitRows(t, t.filteredData, r => {
        if (rk !== null && rKey(r) !== rk) return;
        if (ck !== null && cKey(r) !== ck) return;
        if (needV && !cellStr(cellOf(r, v)).trim()) return;
        out.add(r);
    });
    const part = (c, k) => `${t.headers[c]} = ${k === '' ? '(empty)' : k}`;
    const label = [rk !== null ? part(P.r, rk) : '', ck !== null ? part(P.c, ck) : ''].filter(Boolean).join(' · ') || 'pivot rows';
    closeAllModals();
    setRowMark(t, { kind: 'pivot', label, tip: 'Pivot', rows: out, cells: new Map(), only: true });
    setStats(`${t.name} | ${fmt(out.size)} rows: ${label} — the chip in the status bar shows all rows again, ☰ › Rows › Pivot table comes back to the table.`);
}

/* The whole table as rows of text: the header, every row, the total line. */
function pivotMatrix(numText) {
    const P = pivot, R = P.res, t = P.t, fn = P.fn, across = P.c >= 0, rk = pivotRowKeys(P), ck = R.colKeys;
    const cell = x => x == null ? '' : numText(fn === 'avg' ? Math.round(x * 100) / 100 : Math.round(x * 1e6) / 1e6);
    const head = [t.headers[P.r], ...(across ? ck.map(k => k === '' ? '(empty)' : k) : []), across ? 'Total' : PV_FNS.find(f => f[0] === fn)[1] + (fn !== 'count' ? ` of ${t.headers[P.v]}` : '')];
    const body = rk.map(k => [k === '' ? '(empty)' : k, ...(across ? ck.map(c => cell(pvVal(R.cells.get(k).get(c), fn))) : []), cell(pvVal(R.rowT.get(k), fn))]);
    const tot = ['Total', ...(across ? ck.map(c => cell(pvVal(R.colT.get(c), fn))) : []), cell(pvVal(R.all, fn))];
    return [head, ...body, tot];
}
async function pivotCopy() {
    const P = pivot; if (!P || !P.res) return;
    fxComma = currentDelim(P.t) === ';';
    const text = pivotMatrix(x => fxNumText(String(x))).map(r => r.map(v => /[\t\n"]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v).join('\t')).join('\n');
    try { await navigator.clipboard.writeText(text); toast('Pivot table copied — paste it into a spreadsheet.', { kind: 'ok' }); }
    catch (e) { uiAlert(`Could not copy: ${e.message || e}`); }
}
function pivotTab() {
    const P = pivot; if (!P || !P.res) return;
    const t = P.t, delim = currentDelim(t);
    fxComma = delim === ';';
    const quote = v => (v.includes(delim) || v.includes('"') || v.includes('\n') || v.includes('\r')) ? `"${v.replace(/"/g, '""')}"` : v;
    const m = pivotMatrix(x => fxNumText(String(x)));
    const name = `${stemOf(t.name)} — pivot ${t.headers[P.r]}${P.c >= 0 ? ' × ' + t.headers[P.c] : ''}.csv`;
    closeAllModals();
    processFiles([new File([m.map(r => r.map(quote).join(delim)).join('\n') + '\n'], name.replace(/[\\/:*?"<>|]/g, '_'), { type: 'text/csv' })]);
    toast(`${fmt(m.length - 2)} rows — a new tab, not on disk yet: Save as… writes it.`, { kind: 'ok' });
}
