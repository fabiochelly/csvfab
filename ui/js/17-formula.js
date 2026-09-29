/* ---------------------------------------------------------------
   COMPUTED COLUMN
   A JavaScript expression evaluated once per row, where {Column} stands
   for that row's cell (always a string, "" when empty). The helpers
   below cover what a spreadsheet formula usually does — numbers read the
   French way, dates day first, text cleaning — and are plain arguments
   of the compiled function, so a formula calls them by name. It is the
   user's own code on their own data, compiled with new Function(); an
   error in one row leaves that cell empty and is counted, never thrown.
   The result goes into a new column, or replaces a column's values in
   the rows the filters show.
----------------------------------------------------------------*/
const DAY_MS = 864e5;
function fxDate(v) {
    if (v instanceof Date) return isNaN(v) ? null : v;
    const s = cellStr(v).trim(); if (!s) return null;
    const p = parseDateCell(s, false);
    if (p) {
        const m = p.rest.match(/(\d{1,2}):(\d{2})(?::(\d{2}))?/);
        return new Date(Date.UTC(p.y, p.mo - 1, p.d, m ? +m[1] : 0, m ? +m[2] : 0, m && m[3] ? +m[3] : 0));
    }
    return null;
}
function fxNum(v) {
    if (typeof v === 'number') return v;
    const s = cellStr(v).trim();
    return s ? numKey(s.replace(/[€$£%]/g, '')) : 0;       // an empty cell counts as 0, as in a spreadsheet
}
function fxFmtDate(d, pattern = 'dd/mm/yyyy') {
    d = fxDate(d); if (!d) return '';
    const map = { yyyy: d.getUTCFullYear(), yy: pad2(d.getUTCFullYear() % 100), mm: pad2(d.getUTCMonth() + 1), dd: pad2(d.getUTCDate()),
        hh: pad2(d.getUTCHours()), mi: pad2(d.getUTCMinutes()), ss: pad2(d.getUTCSeconds()) };
    return pattern.replace(/yyyy|yy|mm|dd|hh|mi|ss/g, k => map[k]);
}
const FX = {
    num: fxNum,
    date: fxDate,
    today: () => { const n = new Date(); return new Date(Date.UTC(n.getFullYear(), n.getMonth(), n.getDate())); },
    days: (a, b) => { a = fxDate(a); b = fxDate(b); return a && b ? Math.round((b - a) / DAY_MS) : NaN; },
    addDays: (d, n) => { d = fxDate(d); return d ? new Date(+d + fxNum(n) * DAY_MS) : null; },
    year: d => { d = fxDate(d); return d ? d.getUTCFullYear() : NaN; },
    month: d => { d = fxDate(d); return d ? d.getUTCMonth() + 1 : NaN; },
    day: d => { d = fxDate(d); return d ? d.getUTCDate() : NaN; },
    fmtDate: fxFmtDate,
    round: (n, d = 0) => { const k = 10 ** d; return Math.round(fxNum(n) * k) / k; },
    fixed: (n, d = 2) => { n = fxNum(n); return isNaN(n) ? '' : fxNumText(n.toFixed(d)); },
    upper: s => cellStr(s).toLocaleUpperCase('fr'),
    lower: s => cellStr(s).toLocaleLowerCase('fr'),
    trim: s => cellStr(s).replace(/[\s ]+/g, ' ').trim(),
    capitalize: s => cellStr(s).toLocaleLowerCase('fr').replace(/(^|[\s\-'’(])(\p{L})/gu, (m, a, b) => a + b.toLocaleUpperCase('fr')),
    slug: s => slugify(cellStr(s)),
    len: s => cellStr(s).length,
    left: (s, n) => cellStr(s).slice(0, n),
    right: (s, n) => n > 0 ? cellStr(s).slice(-n) : '',
    pad: (s, n, ch = '0') => cellStr(s).padStart(n, ch),
    extract: (s, re, g = 1) => { const m = cellStr(s).match(re instanceof RegExp ? re : new RegExp(re)); return m ? (m[g] ?? m[0]) : ''; },
    replace: (s, re, by = '') => cellStr(s).replace(re instanceof RegExp ? re : new RegExp(re, 'g'), by),
    contains: (s, part) => removeAccents(cellStr(s).toLowerCase()).includes(removeAccents(cellStr(part).toLowerCase())),
    empty: s => cellStr(s).trim() === '',
    first: (...v) => { for (const x of v) if (cellStr(x).trim() !== '') return x; return ''; },
    join: (sep, ...v) => v.map(cellStr).filter(x => x.trim() !== '').join(sep)
};
/* Numbers are written the way the file most likely writes them: a decimal
   comma in a ;-separated file (the French spreadsheet convention), a point otherwise. */
let fxComma = false;
function fxNumText(s) { return fxComma ? s.replace('.', ',') : s; }
function fxOut(v) {
    if (v == null || (typeof v === 'number' && !isFinite(v))) return '';
    if (typeof v === 'number') return fxNumText(String(Math.round(v * 1e10) / 1e10));
    if (v instanceof Date) return isNaN(v) ? '' : v.getUTCHours() || v.getUTCMinutes() || v.getUTCSeconds() ? v.toISOString().replace('.000Z', 'Z') : v.toISOString().slice(0, 10);
    return String(v);
}

/* {Name} → $[i]. A name is matched exactly, then ignoring case, accents and symbols. */
function compileFormula(t, src) {
    const slugs = t.headers.map(h => slugify(h)), unknown = [], used = [];
    const body = src.replace(/\{([^{}\n]+)\}/g, (m, name) => {
        let c = t.headers.indexOf(name);
        if (c < 0) c = t.headers.indexOf(name.trim());
        if (c < 0) c = slugs.indexOf(slugify(name));
        if (c < 0) { unknown.push(name); return 'undefined'; }
        if (!used.includes(c)) used.push(c);
        return `$[${c}]`;
    });
    if (unknown.length) return { error: `Unknown column: {${unknown[0]}}` };
    if (!body.trim()) return { error: '' };
    try { return { fn: new Function('$', 'row', ...Object.keys(FX), `"use strict"; return (${body}\n);`), used }; }
    catch (e) { return { error: e.message }; }
}
function evalRow(fn, t, r) {
    const d = r.data.length >= t.headers.length ? r.data.map(cellStr) : pad(r.data, t.headers.length).map(cellStr);
    return fxOut(fn(d, r.id, ...Object.values(FX)));
}

const FX_EXAMPLES = [
    ['{A} + " " + {B}', 'join two columns'],
    ['join(" ", {A}, {B})', 'join, skipping empty values'],
    ['num({A}) * 1.2', 'arithmetic'],
    ['fixed(num({A}) / num({B}), 2)', 'two decimals'],
    ['days({A}, today())', 'days since a date'],
    ['year({A})', 'year of a date'],
    ['fmtDate({A}, "yyyy-mm-dd")', 'reformat a date'],
    ['extract({A}, "@(.+)$")', 'regex group (e-mail domain)'],
    ['num({A}) > 1000 ? "big" : "small"', 'condition'],
    ['first({A}, {B}, "none")', 'first non-empty value']
];

function openFormula(col) {
    const t = T(); if (!t || !t.loaded) return;
    fxComma = t.detectedDelim === ';';
    document.getElementById('fx-cols').innerHTML = t.headers.map(h =>
        `<span class="dk" title="Insert this column" onclick="fxInsert(${esc(JSON.stringify('{' + h + '}'))})">${esc(h)}</span>`).join('');
    /* Examples use the first columns of the file, so they run as they are. */
    const a = t.headers[col != null ? col : 0] || 'A', b = t.headers[(col != null ? col : 0) + 1] || t.headers[0] || 'B';
    document.getElementById('fx-ex').innerHTML = FX_EXAMPLES.map(([x, l]) => {
        const s = x.replace(/\{A\}/g, `{${a}}`).replace(/\{B\}/g, `{${b}}`);
        return `<div class="fx-ex" onclick="fxSet(${esc(JSON.stringify(s))})"><code>${esc(s)}</code><span>${esc(l)}</span></div>`;
    }).join('');
    const dest = document.getElementById('fx-dest'), prev = dest.value;
    dest.innerHTML = `<optgroup label="New column">` + t.headers.map((h, i) => `<option value="new:${i}">after ${esc(h)}</option>`).join('') + '</optgroup>'
        + `<optgroup label="Replace the values of (rows shown)">` + t.headers.map((h, i) => `<option value="set:${i}">${esc(h)}</option>`).join('') + '</optgroup>';
    dest.value = col != null ? `new:${col}` : (prev && dest.querySelector(`option[value="${prev}"]`) ? prev : `new:${t.headers.length - 1}`);
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-formula').style.display = 'block';
    fxRefresh();
    document.getElementById('fx-expr').focus();
}
function fxInsert(text) {
    const ta = document.getElementById('fx-expr');
    ta.setRangeText(text, ta.selectionStart, ta.selectionEnd, 'end');
    ta.focus(); fxRefresh();
}
function fxSet(text) { const ta = document.getElementById('fx-expr'); ta.value = text; ta.focus(); fxRefresh(); }

/* Every row the formula would write, with the error count; the preview shows the first ones. */
function fxRun(t, fn, rows) {
    const out = new Array(rows.length); let errors = 0, firstErr = '';
    visitRows(t, rows, (r, i) => {
        try { out[i] = evalRow(fn, t, r); }
        catch (e) { errors++; if (!firstErr) firstErr = e.message; out[i] = ''; }
    });
    return { out, errors, firstErr };
}
let fxTimer = 0;
function fxRefresh() {
    clearTimeout(fxTimer);
    fxTimer = setTimeout(fxRefreshNow, 120);             // a formula runs over every row: not at each keystroke
    const dest = document.getElementById('fx-dest').value;
    document.getElementById('fx-name').disabled = dest.startsWith('set:');
}
function fxRefreshNow() {
    const t = T(); if (!t) return;
    const src = document.getElementById('fx-expr').value, stats = document.getElementById('fx-stats'), pv = document.getElementById('fx-pv');
    const go = document.getElementById('fx-go');
    go.disabled = true;
    const c = compileFormula(t, src);
    if (c.error !== undefined) {
        stats.innerHTML = c.error ? `<span class="warn">${esc(c.error)}</span>` : 'Type a formula, or pick an example below.';
        pv.innerHTML = ''; return;
    }
    const dest = document.getElementById('fx-dest').value, set = dest.startsWith('set:'), rows = set ? t.filteredData : t.allData;
    const { out, errors, firstErr } = fxRun(t, c.fn, rows);
    const filled = out.filter(v => v !== '').length;
    let changed = 0;
    if (set) { const col = +dest.slice(4); rows.forEach((r, i) => { if (out[i] !== cellStr(r.data[col])) changed++; }); }
    stats.innerHTML = (set ? `<b>${fmt(changed)}</b> cells will change in the ${fmt(rows.length)} ${hasFilter(t) ? 'filtered ' : ''}rows`
            : `<b>${fmt(filled)}</b> of ${fmt(rows.length)} rows get a value`)
        + (errors ? `<br><span class="warn">${fmt(errors)} rows raise an error (their cell stays empty): ${esc(firstErr)}</span>` : '');
    const show = c.used.slice(0, 4), name = set ? t.headers[+dest.slice(4)] : (document.getElementById('fx-name').value.trim() || 'Computed');
    pv.innerHTML = `<tr>${show.map(i => `<th style="padding: 4px 8px;">${esc(t.headers[i])}</th>`).join('')}<th style="padding: 4px 8px; color: var(--prim);">${esc(name)}</th></tr>`
        + rows.slice(0, 8).map((r, i) => `<tr>${show.map(c => `<td class="src">${esc(cellStr(r.data[c]))}</td>`).join('')}`
            + (out[i] ? `<td title="${esc(out[i])}">${esc(out[i])}</td>` : '<td class="empty">empty</td>') + '</tr>').join('');
    go.disabled = set ? !changed : !rows.length;
}

function applyFormula() {
    const t = T(); if (!t) return;
    clearTimeout(fxTimer);
    const src = document.getElementById('fx-expr').value, c = compileFormula(t, src);
    if (!c.fn) return;
    const dest = document.getElementById('fx-dest').value, at = +dest.slice(4);
    closeAllModals();
    if (dest.startsWith('set:')) {
        const rows = t.filteredData, { out, errors } = fxRun(t, c.fn, rows), ch = [];
        rows.forEach((r, i) => { if (out[i] !== cellStr(r.data[at])) ch.push([r, r.data[at], out[i]]); });
        if (!ch.length) return;
        const ed = rowEdits();
        for (const [r, , nv] of ch) ed.set(r, at, nv);
        t.modificationsLog.push({ id: '-', col: t.headers[at], old: 'formula', new: `${ch.length} cells`, what: `${fmt(ch.length)} cells computed in ${t.headers[at]}`,
            undo: () => ed.undo() });
        updateSaveBtn(); renderTabBar(); render();
        flash(ch.map(([r]) => [r, at]));
        setStats(`${t.name} | ${fmt(ch.length)} cells computed in ${t.headers[at]}${errors ? `, ${fmt(errors)} errors left empty` : ''} — not written yet, use Save.`);
        return;
    }
    const name = document.getElementById('fx-name').value.trim() || 'Computed';
    const { out, errors } = fxRun(t, c.fn, t.allData);
    const headers = [...t.headers.slice(0, at + 1), name, ...t.headers.slice(at + 1)];
    restructure(t, headers, (d, r, i) => { const row = pad(d, at + 1); return [...row.slice(0, at + 1), out[i], ...row.slice(at + 1)]; },
        c => c <= at ? c : c + 1, `column "${name}" computed`);
    if (errors) setStats(`${t.name} | Column "${name}" computed, ${fmt(errors)} rows with an error left empty — not written yet, use Save.`);
}
