/* ---------------------------------------------------------------
   FUNCTION PICKER (ƒx)
   Opened from the ƒx button of the filter box (which turns Expression
   on), of the computed column, or from the palette. Everything comes
   from FX_DOC (17-…) plus the columns of the active tab: a search,
   families as tabs, and for the entry selected its signature, what it
   does and its examples — each run on the selected row (else the first
   one shown), with a real column of the right type in place of {N} {D}
   {T} {T2}, so the result shown is the one the formula will give.
   Inserting puts the signature at the caret of the box it was opened
   from and selects its first argument, to be typed over.
   A capture-phase key handler keeps Escape and the arrows to itself
   while it is open: Escape must not close the computed column below.
   State hangs off the function (openFxPicker.s), as elsewhere.
----------------------------------------------------------------*/
const FX_FAMS = ['Numbers', 'Dates', 'Text', 'Tests', 'Values', 'Methods', 'Operators'];
const FXP_TABS = ['All', ...FX_FAMS, 'Columns'];

function fxpTarget(s) { return document.getElementById(s.target === 'formula' ? 'fx-expr' : 'global-search'); }

function openFxPicker(target, name) {
    closeDDs(); exprAcClose();
    let box = document.getElementById('fxp');
    if (!box) {
        box = document.createElement('div'); box.id = 'fxp';
        box.innerHTML = `<div class="fxp-box" role="dialog" aria-label="Functions">
            <div class="cmdk-in-wrap"><span class="fxp-logo">ƒx</span>
                <input id="fxp-in" type="text" spellcheck="false" autocomplete="off" placeholder="Search a function, a column, an operator…"></div>
            <div class="fxp-tabs" id="fxp-tabs"></div>
            <div class="fxp-body"><div class="fxp-list" id="fxp-list"></div><div class="fxp-det" id="fxp-det"></div></div>
            <div class="cmdk-foot"><span><kbd>↑</kbd><kbd>↓</kbd> choose</span><span><kbd>Tab</kbd> next family</span><span><kbd>Enter</kbd> insert</span><span><kbd>Esc</kbd> close</span></div>
        </div>`;
        document.body.appendChild(box);
        box.addEventListener('mousedown', e => { if (e.target === box) closeFxPicker(); });
        const inp = box.querySelector('#fxp-in');
        inp.addEventListener('input', () => { const s = openFxPicker.s; s.q = inp.value; s.act = 0; fxpRender(); });
    }
    const s = openFxPicker.s = { target, fam: 'All', q: '', act: 0, items: [] };
    const inp = document.getElementById('fxp-in'); inp.value = '';
    fxpRender();
    if (name) { const k = s.items.findIndex(x => x.n === name); if (k >= 0) { s.act = k; fxpRender(); } }
    box.classList.add('open');
    const a = box.querySelector('.fxp-it.act'); if (a) a.scrollIntoView({ block: 'center' });   // rendered while hidden: scrolled only now
    document.addEventListener('keydown', fxpKey, true);
    inp.focus();
}
function closeFxPicker(refocus = true) {
    const box = document.getElementById('fxp'); if (!box || !box.classList.contains('open')) return;
    box.classList.remove('open');
    document.removeEventListener('keydown', fxpKey, true);
    if (refocus) fxpTarget(openFxPicker.s).focus();
}
function fxpKey(e) {
    const s = openFxPicker.s, n = s.items.length;
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); closeFxPicker(); }
    else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); e.stopPropagation(); if (n) { s.act = (s.act + (e.key === 'ArrowDown' ? 1 : -1) + n) % n; fxpRender(); } }
    else if (e.key === 'Enter') { e.preventDefault(); e.stopPropagation(); if (s.items[s.act]) fxpInsert(s.items[s.act].s, s.items[s.act]); }
    else if (e.key === 'Tab') { e.preventDefault(); e.stopPropagation(); const k = FXP_TABS.indexOf(s.fam); fxpFam(FXP_TABS[(k + (e.shiftKey ? -1 : 1) + FXP_TABS.length) % FXP_TABS.length]); }
    else if (!document.getElementById('fxp').contains(e.target)) e.stopPropagation();   // no grid shortcut underneath
}
function fxpFam(f) { const s = openFxPicker.s; s.fam = f; s.act = 0; fxpRender(); document.getElementById('fxp-in').focus(); }
/* A second click inserts: caught on click through e.detail, since the first one re-renders the list
   and the browser's dblclick would find its element gone. */
function fxpClick(e, i) { const s = openFxPicker.s; if (e.detail >= 2 && s.act === i) return fxpInsert(s.items[i].s, s.items[i]); s.act = i; fxpRender(); }

/* The entries of the current family matching the search, best first. */
function fxpItems(s) {
    const t = T(), kinds = t && t.loaded ? columnKinds(t) : [];
    const cols = t ? t.headers.map((h, c) => ({ f: 'Columns', n: String(h), s: `{${h}}`, col: c, kind: kinds[c] || '' })) : [];
    const pool = s.fam === 'All' ? [...FX_DOC, ...cols] : s.fam === 'Columns' ? cols : FX_DOC.filter(x => x.f === s.fam);
    const q = removeAccents(s.q.trim().toLowerCase());
    if (!q) return pool.map(x => ({ ...x, hits: [] }));
    const out = [];
    for (const x of pool) {
        const m = fuzzy(q.replace(/\s+/g, ''), x.n);
        if (m) out.push({ ...x, hits: m.hits, score: m.score + 10 });
        else if (x.d && (' ' + removeAccents(x.d.toLowerCase()).replace(/[^\p{L}\p{N}]+/gu, ' ')).includes(' ' + q)) out.push({ ...x, hits: [], score: 0 });   // by what it does, at a word's start: "age" finds years, not average
    }
    return out.sort((a, b) => b.score - a.score);
}
function fxpRender() {
    const s = openFxPicker.s;
    s.items = fxpItems(s);
    if (s.act >= s.items.length) s.act = Math.max(0, s.items.length - 1);
    document.getElementById('fxp-tabs').innerHTML = FXP_TABS.map(f => `<span class="fxp-tab${f === s.fam ? ' on' : ''}" onclick="fxpFam('${f}')">${f}</span>`).join('');
    const mark = (label, hits) => { const H = new Set(hits); return [...label].map((ch, i) => H.has(i) ? `<b>${esc(ch)}</b>` : esc(ch)).join(''); };
    const list = document.getElementById('fxp-list');
    list.innerHTML = s.items.length ? s.items.map((x, i) => `<div class="cmdk-item fxp-it${i === s.act ? ' act' : ''}" onclick="fxpClick(event, ${i})">`
        + (x.f === 'Columns' ? typeIcon(x.kind) + `<span class="cl">${mark(x.n, x.hits)}</span>` : `<span class="cl fxp-mono">${mark(x.n, x.hits)}</span>`)
        + (s.fam === 'All' || s.q ? `<span class="cg">${x.f}</span>` : '') + '</div>').join('')
        : '<div class="cmdk-empty">Nothing matches.</div>';
    const a = list.querySelector('.act'); if (a) a.scrollIntoView({ block: 'nearest' });
    fxpDetail(s.items[s.act]);
}

/* The row the examples run on: the selected one, else the first shown. */
function fxpRow(t) {
    if (!t || !t.loaded || !t.filteredData.length) return null;
    return (sel && sel.tab === t.id && t.filteredData[sel.fr]) || t.filteredData[0];
}
/* The column a value of kind k ('n', 'd', 't') most likely comes from: the selected column
   when it fits, else the first of that kind; with pick ('email', 'phone'), the first column
   whose values mostly read that way (the first 50 filled), whatever its kind. -1: none. */
function fxColumn(t, k, pick) {
    if (!t || !t.loaded || !t.headers.length) return -1;
    const kinds = columnKinds(t), cur = sel && sel.tab === t.id ? sel.fc : -1;
    if (pick) {
        const test = pick === 'email' ? FX.isEmail : FX.isPhone, rows = t.allData.slice(0, 400);
        const fits = c => { let f = 0, ok = 0; for (const r of rows) { const v = cellStr(cellOf(r, c)).trim(); if (!v) continue; f++; if (test(v)) ok++; if (f >= 50) break; } return f > 0 && ok / f >= 0.6; };
        if (cur >= 0 && fits(cur)) return cur;
        for (let c = 0; c < t.headers.length; c++) if (!t.hiddenCols.has(c) && fits(c)) return c;
    }
    if (cur >= 0 && kinds[cur] === k) return cur;
    const c = kinds.findIndex((x, i) => x === k && !t.hiddenCols.has(i));
    return c >= 0 ? c : kinds.indexOf(k);
}
/* {N} {D} {T} {T2} → columns of the file of that type (fxColumn()); pick steers {T}. */
function fxExample(t, src, pick) {
    if (!t || !t.headers.length) return src.replace(/\{N\}/g, '{Amount}').replace(/\{D\}/g, '{Date}').replace(/\{T2\}/g, '{City}').replace(/\{T\}/g, '{Name}');
    const n = fxColumn(t, 'n'), d = fxColumn(t, 'd'), x = fxColumn(t, 't', pick);
    const x2 = columnKinds(t).findIndex((k, c) => k === 't' && c !== x);
    const name = (c, fb) => `{${t.headers[c >= 0 ? c : fb]}}`;
    return src.replace(/\{N\}/g, name(n, 0)).replace(/\{D\}/g, name(d, 0)).replace(/\{T2\}/g, name(x2, x >= 0 ? x : 0)).replace(/\{T\}/g, name(x, 0));
}
/* The column to put in an entry's first argument on insert: the kind its examples give that
   argument ({N} {D} {T} first met), the e-mail or phone column for those tests. null: leave it. */
function fxArgColumn(t, x) {
    if (!t || !t.loaded) return null;
    const m = (x.ex || []).map(e => e[0].match(/\{(N|D|T)2?\}/)).find(Boolean);
    if (!m) return null;
    const c = fxColumn(t, m[1].toLowerCase(), x.pick);
    return c >= 0 ? `{${t.headers[c]}}` : null;
}
/* What a formula gives on a row: {v} or {err}. */
function fxTry(t, r, src) {
    const c = compileFormula(t, src);
    if (c.error !== undefined) return { err: c.error || 'empty' };
    try { return { v: fxOut(c.fn(rowArgs(t, r, c.used), r.id, ...FX_VALUES)) }; }
    catch (e) { return { err: e.message }; }
}
function fxpDetail(x) {
    const det = document.getElementById('fxp-det');
    if (!x) { det.innerHTML = ''; return; }
    const t = T(), r = fxpRow(t);
    const res = src => {
        if (!r) return '';
        const o = fxTry(t, r, src);
        return o.err ? `<span class="fxp-res err">${esc(o.err)}</span>` : `<span class="fxp-res">${o.v === '' ? '<i>empty</i>' : esc(o.v)}</span>`;
    };
    let html = `<div class="fxp-sig">${x.f === 'Columns' ? typeIcon(x.kind) : ''}<code>${esc(x.s.trim())}</code><span class="cg">${x.f}</span></div>`;
    if (x.f === 'Columns') {
        html += `<div class="fxp-d">A column: in a formula, <code>${esc(x.s)}</code> is the row's cell as text. Read it with <code>num()</code> or <code>date()</code> to compute, or use its string methods: <code>${esc(x.s)}.startsWith("A")</code>.</div>`;
        const k = x.kind, ex = k === 'n' ? [`num(${x.s}) > 0`, `round(num(${x.s}))`] : k === 'd' ? [`year(${x.s})`, `days(${x.s}, today())`] : [`${x.s}`, `upper(${x.s})`, `empty(${x.s})`];
        html += ex.map(src => fxpExHtml(src, '', res(src))).join('');
    } else {
        html += `<div class="fxp-d">${esc(x.d)}</div>`;
        html += (x.ex || []).map(([src, note]) => { const f = fxExample(t, src, x.pick); return fxpExHtml(f, note, res(f)); }).join('');
    }
    if (r) html += `<div class="fxp-row">Results on row ${fmt(r.id)}${sel && sel.tab === t.id ? ', the one selected' : ', the first shown — select a cell to try another'}. Click an example to insert it.</div>`;
    html += `<div class="modal-actions"><button class="btn" onclick="fxpInsert(openFxPicker.s.items[openFxPicker.s.act].s, openFxPicker.s.items[openFxPicker.s.act])">Insert</button></div>`;
    det.innerHTML = html;
}
function fxpExHtml(src, note, result) {
    return `<div class="fxp-ex" onclick="fxpInsert(${esc(JSON.stringify(src))})" title="Insert this formula"><code>${esc(src)}</code>`
        + `<div class="fxp-exl">${result ? '<span class="fxp-eq">=</span>' + result : ''}<span class="fxp-note">${esc(note)}</span></div></div>`;
}

/* Into the box the picker was opened from, at its caret. A function's first argument, when it
   is a value (v, s, n, a, date, from — not a test, nor ifs' or join's), becomes the most likely
   column and the next argument is selected, to be typed over; a string method is given a
   column in front unless it follows one. */
const FXP_COL_ARG = /^(v|s|n|a|date|from)$/;
function fxpInsert(text, entry) {
    const s = openFxPicker.s, box = fxpTarget(s), t = T();
    if (s.target === 'search' && !document.getElementById('use-expr').checked) { document.getElementById('use-expr').checked = true; updateExprUI(); }
    const at = box.selectionStart ?? box.value.length, before = box.value.slice(0, at).trimEnd();
    let sel0 = -1, sel1 = -1;                       // the range to select, relative to the inserted text
    const col = entry ? fxArgColumn(t, entry) : null;
    if (entry && entry.f === 'Methods') {
        if (col && !/[}\])"'\w]$/.test(before)) text = col + text;
    } else if (entry && entry.f !== 'Columns' && entry.f !== 'Operators') {
        const p = text.indexOf('(');
        if (p >= 0 && text[p + 1] !== ')') {
            let e = p + 1; while (e < text.length && text[e] !== ',' && text[e] !== ')') e++;
            if (col && FXP_COL_ARG.test(text.slice(p + 1, e).trim())) text = text.slice(0, p + 1) + col + text.slice(e);
            else { sel0 = p + 1; sel1 = e; }
        }
    }
    if (sel0 < 0 && entry && entry.f !== 'Columns' && entry.f !== 'Operators') {
        /* The first argument still a placeholder, else the next one; nothing left: the caret after. */
        const p = text.indexOf('(');
        if (p >= 0) {
            let k = p + 1;
            if (col && text.startsWith(col, k)) { k += col.length; k = text[k] === ',' ? k + 1 : -1; while (k > 0 && text[k] === ' ') k++; }
            if (k > 0 && text[k] !== ')') { let e = k; while (e < text.length && text[e] !== ',' && text[e] !== ')') e++; sel0 = k; sel1 = e; }
        }
    }
    box.setRangeText(text, at, box.selectionEnd ?? at, 'end');
    const a = sel0 >= 0 ? at + sel0 : at + text.length, b = sel0 >= 0 ? at + sel1 : a;
    closeFxPicker(false);
    box.focus(); box.setSelectionRange(a, b);
    box.dispatchEvent(new Event('input'));          // the filter or the computed column's preview
    if (s.target === 'search') exprAcClose();
}
