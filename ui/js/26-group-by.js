/* ---------------------------------------------------------------
   GROUP BY
   The rows shown, grouped on one or more columns: a count per group,
   and for the columns ticked a sum, average, min, max or number of
   distinct values. The result opens as a new tab — a small in-memory
   CSV — so it can be sorted, filtered, saved as a file or as a workbook
   like any other. Keys compare as in the lookup (exact, loose, slug);
   the first spelling met is the one shown.
----------------------------------------------------------------*/
let gb = null;           // { t, order: [group columns in tick order], aggs: {col: Set of functions}, timer }
const GB_FNS = [['sum', 'Sum'], ['avg', 'Average'], ['min', 'Min'], ['max', 'Max'], ['distinct', 'Distinct']];
const GB_PREVIEW_ROWS = 50000;

/* Summed by default: a numeric column that reads as a quantity — decimals in its
   values, or an amount-like title — not an ID, a phone number or a postal code. */
const AMOUNT_RE = /\b(montant|amount|total|totaux|prix|price|tarif|ca|chiffre|revenue|revenu|qty|quantite|quantity|nb|nombre|count|somme|sum|cout|cost|budget|salaire|salary|poids|weight|volume|surface|duree|duration|heures|hours|points|score|solde|balance|ht|ttc|tva|vat|remise|discount|marge|margin)\b/;
function gbSumByDefault(t, c) {
    if (columnKinds(t)[c] !== 'n') return false;
    if (AMOUNT_RE.test(slugify(t.headers[c]))) return true;
    let seen = 0, dec = 0, codes = 0;
    for (const r of t.allData) {
        const v = cellStr(cellOf(r, c)).trim(); if (!v) continue;
        if (/[.,]\d/.test(v)) dec++;
        if (/^0\d/.test(v) || v.replace(/\D/g, '').length >= 9) codes++;   // leading zero, or nine digits and more: a code, a phone
        if (++seen >= 400) break;
    }
    return seen > 0 && dec > 0 && codes === 0;
}
function openGroupBy(col) {
    const t = T(); if (!t || !t.loaded) return;
    const kinds = columnKinds(t), aggs = {};
    t.headers.forEach((_, c) => { if (c !== col && gbSumByDefault(t, c)) aggs[c] = new Set(['sum']); });
    gb = { t, order: col != null ? [col] : [], aggs, kinds };
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-group').style.display = 'block';
    groupRefresh();
}
function gbToggleCol(c, on) {
    gb.order = gb.order.filter(x => x !== c);
    if (on) gb.order.push(c);
    groupRefresh();
}
function gbToggleAgg(c, fn, on) {
    if (!gb.aggs[c]) gb.aggs[c] = new Set();
    if (on) gb.aggs[c].add(fn); else gb.aggs[c].delete(fn);
    groupRefresh(true);
}
function gbNorm() { return LK_NORM[document.querySelector('input[name="gb-match"]:checked').value]; }

/* groups: Map(key → { vals, n, a: [{sum, cnt, min, minV, max, maxV, set}] }), aggCols: the columns aggregated. */
function groupCompute(t, rows, order, aggs, kinds, norm) {
    const aggCols = Object.keys(aggs).map(Number).filter(c => aggs[c].size && !order.includes(c)).sort((a, b) => a - b);
    const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' }), groups = new Map();
    visitRows(t, rows, r => {
        const d = r.data, key = order.map(c => norm(cellStr(d[c]))).join('\u0001');
        let g = groups.get(key);
        if (!g) { g = { vals: order.map(c => cellStr(d[c])), n: 0, a: aggCols.map(() => ({ sum: 0, cnt: 0, min: null, minV: '', max: null, maxV: '', set: null })) }; groups.set(key, g); }
        g.n++;
        for (let k = 0; k < aggCols.length; k++) {
            const c = aggCols[k], v = cellStr(d[c]).trim(); if (!v) continue;
            const a = g.a[k], want = aggs[c], kind = kinds[c];
            if (want.has('distinct')) { if (!a.set) a.set = new Set(); if (a.set.size < 200000) a.set.add(v); }
            if (!(want.has('sum') || want.has('avg') || want.has('min') || want.has('max'))) continue;
            if (kind === 't') {
                if (!a.minV || coll.compare(v, a.minV) < 0) a.minV = v;
                if (!a.maxV || coll.compare(v, a.maxV) > 0) a.maxV = v;
                continue;
            }
            const x = kind === 'n' ? numKey(v) : dateKey(v);
            if (isNaN(x)) continue;
            if (kind === 'n') { a.sum += x; a.cnt++; }
            if (a.min === null || x < a.min) { a.min = x; a.minV = v; }
            if (a.max === null || x > a.max) { a.max = x; a.maxV = v; }
        }
    });
    return { groups, aggCols };
}
/* Header and rows of the result, groups by decreasing count. */
function groupTable(t, res, order, aggs, kinds, delim) {
    const { groups, aggCols } = res;
    fxComma = delim === ';';
    const num = x => fxNumText(String(Math.round(x * 1e6) / 1e6));
    const header = [...order.map(c => t.headers[c]), 'Count'];
    for (const c of aggCols) for (const [fn, label] of GB_FNS) if (aggs[c].has(fn)) header.push(`${t.headers[c]} (${label.toLowerCase()})`);
    const list = [...groups.values()].sort((a, b) => b.n - a.n);
    const rows = list.map(g => {
        const out = [...g.vals, String(g.n)];
        aggCols.forEach((c, k) => {
            const a = g.a[k];
            for (const [fn] of GB_FNS) if (aggs[c].has(fn)) out.push(
                fn === 'sum' ? (kinds[c] === 'n' ? num(a.sum) : '') : fn === 'avg' ? (a.cnt ? num(a.sum / a.cnt) : '')
                : fn === 'min' ? a.minV : fn === 'max' ? a.maxV : String(a.set ? a.set.size : 0));
        });
        return out;
    });
    return { header, rows };
}

function groupRefresh(aggsOnly) {
    const t = T(); if (!t || !gb || gb.t !== t) return;
    const { order, aggs, kinds } = gb;
    if (!aggsOnly) document.getElementById('gb-cols').innerHTML = t.headers.map((h, c) => {
        const k = order.indexOf(c);
        return `<label class="col-label"><input type="checkbox" ${k >= 0 ? 'checked' : ''} onchange="gbToggleCol(${c}, this.checked)"> ${esc(h)}${k >= 0 ? `<span class="ord">${k + 1}</span>` : ''}</label>`;
    }).join('');
    document.getElementById('gb-aggs').innerHTML = `<tr><th>Column</th>${GB_FNS.map(([, l]) => `<th>${l}</th>`).join('')}</tr>`
        + t.headers.map((h, c) => order.includes(c) ? '' : `<tr><td class="pf-name" title="${esc(h)}">${typeIcon(kinds[c])}${esc(h)}</td>`
            + GB_FNS.map(([fn]) => { const numOnly = fn === 'sum' || fn === 'avg', ok = !numOnly || kinds[c] === 'n';
                return `<td class="gb-chk">${ok ? `<input type="checkbox" ${aggs[c] && aggs[c].has(fn) ? 'checked' : ''} onchange="gbToggleAgg(${c}, '${fn}', this.checked)">` : ''}</td>`; }).join('') + '</tr>').join('');
    clearTimeout(gb.timer);
    gb.timer = setTimeout(groupPreview, 120);
}
function groupPreview() {
    const t = T(); if (!t || !gb || gb.t !== t) return;
    const stats = document.getElementById('gb-stats'), pv = document.getElementById('gb-pv'), go = document.getElementById('gb-go');
    const { order, aggs, kinds } = gb, rows = t.filteredData;
    if (!order.length) { stats.innerHTML = 'Tick the column(s) to group by.'; pv.innerHTML = ''; go.disabled = true; return; }
    const sample = rows.length > GB_PREVIEW_ROWS ? rows.slice(0, GB_PREVIEW_ROWS) : rows;
    const res = groupCompute(t, sample, order, aggs, kinds, gbNorm()), { header, rows: out } = groupTable(t, res, order, aggs, kinds, currentDelim(t));
    stats.innerHTML = `<b>${fmt(res.groups.size)}</b> group${res.groups.size === 1 ? '' : 's'} in ${fmt(sample.length)} ${hasFilter(t) ? 'filtered ' : ''}rows`
        + (sample !== rows ? `<span class="muted"> — preview on the first ${fmt(GB_PREVIEW_ROWS)} rows; the result covers all ${fmt(rows.length)}</span>` : '')
        + ` · ${header.length} columns in the result`;
    pv.innerHTML = `<tr>${header.map((h, i) => `<th style="padding: 4px 8px;${i >= order.length ? ' color: var(--prim);' : ''}">${esc(h)}</th>`).join('')}</tr>`
        + out.slice(0, 8).map(r => `<tr>${r.map(v => v === '' ? '<td class="empty">empty</td>' : `<td title="${esc(v)}">${esc(v)}</td>`).join('')}</tr>`).join('')
        + (out.length > 8 ? `<tr><td colspan="${header.length}" class="muted">…and ${fmt(out.length - 8)} more</td></tr>` : '');
    go.disabled = false;
}

/* The result as a small CSV, opened as a new (unsaved) tab. */
function applyGroupBy() {
    const t = T(); if (!t || !gb || gb.t !== t || !gb.order.length) return;
    const { order, aggs, kinds } = gb, delim = currentDelim(t);
    closeAllModals();
    const res = groupCompute(t, t.filteredData, order, aggs, kinds, gbNorm()), { header, rows } = groupTable(t, res, order, aggs, kinds, delim);
    const quote = v => (v.includes(delim) || v.includes('"') || v.includes('\n') || v.includes('\r')) ? `"${v.replace(/"/g, '""')}"` : v;
    const text = [header, ...rows].map(r => r.map(quote).join(delim)).join('\n') + '\n';
    const name = `${stemOf(t.name)} — by ${order.map(c => t.headers[c]).join(', ')}.csv`;
    gb = null;
    processFiles([new File([text], name, { type: 'text/csv' })]);
    toast(`${fmt(rows.length)} groups — a new tab, not on disk yet: Save as… writes it.`, { kind: 'ok' });
}
