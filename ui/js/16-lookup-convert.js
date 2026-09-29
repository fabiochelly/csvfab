/* ---------------------------------------------------------------
   LOOK UP (VLOOKUP between open tabs)
   Rows of this tab are matched to rows of another open tab on a key
   column; the picked columns of the matching row are copied in, as new
   columns right after the key. The first source row wins when a key
   repeats there. The source tab may have been released from RAM (or
   never parsed): tabRows() reads it back without making it active.
----------------------------------------------------------------*/
let lookup = null;       // { src, rows, cols: [source column indices, in tick order], idx }
const LK_NORM = { exact: v => v, loose: v => v.trim().toLowerCase(), slug: v => slugify(v) };   // toLowerCase: as 'fr', several times faster

function openLookup(col) {
    const t = T(); if (!t || !t.loaded) return;
    const others = tabs.filter(x => x !== t);
    if (!others.length) {
        uiAlert('Open the file that holds the values first.\n\nThe lookup reads another open tab: its rows are matched to this tab\'s rows on a key column (an ID, an e-mail…), and the columns you pick are copied over.');
        return;
    }
    const key = document.getElementById('lk-key');
    key.innerHTML = t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    key.value = col != null ? col : 0;
    lookup = { userKey: col != null };
    const src = document.getElementById('lk-src'), prev = +src.value;
    src.innerHTML = others.map(x => `<option value="${x.id}">${esc(x.name)}</option>`).join('');
    if (others.some(x => x.id === prev)) src.value = prev;
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-lookup').style.display = 'block';
    lookupSource();
}

async function lookupSource() {
    const t = T(), src = tabs.find(x => x.id === +document.getElementById('lk-src').value);
    if (!src) return;
    lookup = { userKey: lookup && lookup.userKey, src, rows: null, cols: [], idx: null };
    document.getElementById('lk-skey').innerHTML = '';
    lookupRefresh();
    const rows = await tabRows(src);
    if (!lookup || lookup.src !== src) return;          // another tab picked meanwhile, or closed
    if (!rows) { lookup.error = `Cannot read ${src.name}.`; lookupRefresh(); return; }
    lookup.rows = rows;
    /* Key columns: the same name on both sides if there is one. */
    const keySel = document.getElementById('lk-key');
    const srcSlugs = src.headers.map(h => slugify(h));
    let k = +keySel.value, sk = srcSlugs.indexOf(slugify(t.headers[k]));
    if (sk < 0 && !lookup.userKey) {
        const c = t.headers.findIndex(h => srcSlugs.includes(slugify(h)));
        if (c >= 0) { keySel.value = k = c; sk = srcSlugs.indexOf(slugify(t.headers[c])); }
    }
    document.getElementById('lk-skey').innerHTML = src.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    document.getElementById('lk-skey').value = Math.max(0, sk);
    lookupRefresh();
}

function lookupToggle(i, on) {
    lookup.cols = lookup.cols.filter(c => c !== i);
    if (on) lookup.cols.push(i);
    lookupRefresh();
}

/* Source key → first row with it; rebuilt only when the key column or the comparison changes. */
function lookupIndex() {
    const skey = +document.getElementById('lk-skey').value, match = document.querySelector('input[name="lk-match"]:checked').value;
    const sig = skey + '|' + match;
    if (lookup.idx && lookup.idx.sig === sig) return lookup.idx;
    const norm = LK_NORM[match], map = new Map(), rows = lookup.rows, keys = new Array(rows.length); let dups = 0;
    visitRows(lookup.src, rows, (r, i) => { keys[i] = norm(cellStr(cellOf(r, skey))); });
    for (let i = 0; i < rows.length; i++) {           // in the source's order: the first row wins
        const k = keys[i];
        if (!k.trim()) continue;
        if (map.has(k)) dups++; else map.set(k, rows[i]);   // the row: its cells are decoded when used
    }
    return lookup.idx = { sig, map, dups, norm };
}

function lookupRefresh() {
    const t = T(); if (!t || !lookup) return;
    const stats = document.getElementById('lk-stats'), pv = document.getElementById('lk-pv'), go = document.getElementById('lk-go');
    const colsBox = document.getElementById('lk-cols');
    go.disabled = true;
    if (lookup.error || !lookup.rows) {
        stats.innerHTML = lookup.error ? `<span class="warn">${esc(lookup.error)}</span>` : `Reading ${esc(lookup.src ? lookup.src.name : '')}…`;
        colsBox.innerHTML = ''; pv.innerHTML = ''; return;
    }
    const { src, cols } = lookup, skey = +document.getElementById('lk-skey').value, key = +document.getElementById('lk-key').value;
    colsBox.innerHTML = src.headers.map((h, i) => {
        if (i === skey) return '';
        const k = cols.indexOf(i);
        return `<label class="col-label"><input type="checkbox" ${k >= 0 ? 'checked' : ''} onchange="lookupToggle(${i}, this.checked)"> ${esc(h)}${k >= 0 ? `<span class="ord">${k + 1}</span>` : ''}</label>`;
    }).join('');
    lookup.cols = cols.filter(c => c !== skey);

    const { map, dups, norm } = lookupIndex();
    let hit = 0, empty = 0;
    visitRows(t, t.allData, r => {
        const k = norm(cellStr(cellOf(r, key)));
        if (!k.trim()) empty++; else if (map.has(k)) hit++;
    });
    const miss = t.allData.length - hit - empty;
    stats.innerHTML = `<b>${fmt(hit)}</b> of ${fmt(t.allData.length)} rows find a match in ${esc(src.name)}`
        + (miss ? ` · ${fmt(miss)} don't` : '') + (empty ? ` · ${fmt(empty)} have an empty key` : '')
        + (miss || empty ? ' — their new cells stay empty' : '')
        + (dups ? `<br><span class="warn">${fmt(dups)} rows of ${esc(src.name)} repeat a key already seen: the first one is used.</span>` : '')
        + (!hit ? '<br><span class="warn">No key matches: check the two key columns and the comparison.</span>' : '');
    const names = lookupNames(t);
    const sample = t.allData.slice(0, 8);
    pv.innerHTML = !lookup.cols.length ? '<tr><td class="empty">Tick the columns to bring over.</td></tr>'
        : `<tr><th style="padding: 4px 8px;">${esc(t.headers[key])}</th>` + names.map(n => `<th style="padding: 4px 8px; color: var(--prim);">${esc(n)}</th>`).join('') + '</tr>'
        + sample.map(r => {
            const m = map.get(norm(cellStr(r.data[key]))), d = m && m.data;
            return `<tr><td class="src">${esc(cellStr(r.data[key]))}</td>` + lookup.cols.map(c => {
                const v = d ? cellStr(d[c]) : '';
                return !d ? `<td class="empty">${cellStr(r.data[key]).trim() ? 'no match' : 'empty key'}</td>` : v ? `<td title="${esc(v)}">${esc(v)}</td>` : '<td class="empty">empty</td>';
            }).join('') + '</tr>';
        }).join('');
    go.disabled = !lookup.cols.length || !hit;
}

/* A column name this tab already has gets the source's name after it. */
function lookupNames(t) {
    const src = lookup.src, tag = src.name.replace(/\.[^.]+$/, ''), taken = new Set(t.headers);
    return lookup.cols.map(c => { const h = src.headers[c]; const n = taken.has(h) ? `${h} (${tag})` : h; taken.add(n); return n; });
}

function applyLookup() {
    const t = T(); if (!t || !lookup || !lookup.rows || !lookup.cols.length) return;
    const key = +document.getElementById('lk-key').value, { map, norm } = lookupIndex();
    const cols = lookup.cols.slice(), names = lookupNames(t), src = lookup.src, n = cols.length;
    closeAllModals();
    lookup = null;                        // drop the reference to the source rows
    let hit = 0;
    const headers = [...t.headers.slice(0, key + 1), ...names, ...t.headers.slice(key + 1)];
    restructure(t, headers, d => {
        const row = pad(d, key + 1), o = map.get(norm(cellStr(row[key]))), m = o && o.data;
        if (m) hit++;
        return [...row.slice(0, key + 1), ...cols.map(c => m ? cellStr(m[c]) : ''), ...row.slice(key + 1)];
    }, c => c <= key ? c : c + n, `${n} column${n > 1 ? 's' : ''} looked up in ${src.name}`);
    setStats(`${t.name} | ${n} column${n > 1 ? 's' : ''} looked up in ${src.name}: ${fmt(hit)} of ${fmt(t.allData.length)} rows matched — not written yet, use Save.`);
}

/* ---------------------------------------------------------------
   CONVERT FORMATS
   One column, the rows the filters show, rewritten into one format:
   dates, numbers or phone numbers. Each converter returns the new text,
   or null when it does not recognise the value — such cells are left as
   they are and counted, and "Show them" filters the grid down to them.
   Everything is done on the text, never through a float, so digits are
   never lost (unless a number of decimals is imposed).
----------------------------------------------------------------*/
const pad2 = n => String(n).padStart(2, '0');

function parseDateCell(s, monthFirst) {
    const m = s.match(/^(\d{1,4})[-\/.](\d{1,2})[-\/.](\d{1,4})((?:[ T]\d{1,2}:\d{2}.*)?)$/);
    if (!m) return null;
    let [, a, b, c, rest] = m, y, mo, d;
    if (a.length === 4) { if (c.length > 2) return null; [y, mo, d] = [a, b, c]; }
    else {
        if (a.length > 2 || (c.length !== 2 && c.length !== 4)) return null;
        y = c; [d, mo] = monthFirst ? [b, a] : [a, b];
    }
    y = +y; mo = +mo; d = +d;
    /* Two-digit years: up to ten years ahead is this century, the rest the last one (birth dates). */
    if (a.length !== 4 && c.length === 2) y += y <= new Date().getFullYear() % 100 + 10 ? 2000 : 1900;
    if (mo < 1 || mo > 12 || d < 1 || d > new Date(Date.UTC(y, mo, 0)).getUTCDate()) return null;
    return { y, mo, d, rest };
}
const DATE_OUT = {
    iso: p => `${p.y}-${pad2(p.mo)}-${pad2(p.d)}`,
    dmy: p => `${pad2(p.d)}/${pad2(p.mo)}/${p.y}`,
    mdy: p => `${pad2(p.mo)}/${pad2(p.d)}/${p.y}`,
    dmyd: p => `${pad2(p.d)}.${pad2(p.mo)}.${p.y}`,
    dmyh: p => `${pad2(p.d)}-${pad2(p.mo)}-${p.y}`
};

/* Sign, integer digits, fraction digits, and a currency or % kept around them. */
function parseNumberCell(s, dec) {
    const m = s.match(/^([€$£]\s?)?([+-])?\s*(\d[\d\s  .,']*?)(\s?(?:[€$£%]|EUR|USD))?$/);
    if (!m) return null;
    const [, pre = '', sign = '', body, post = ''] = m;
    const parts = body.split(dec);
    if (parts.length > 2) return null;
    let [int, frac = null] = parts;
    if (frac !== null && !/^\d+$/.test(frac)) return null;
    const other = dec === ',' ? '.' : ',';
    if (/[^\d]/.test(int)) {
        if (!new RegExp(`^\\d{1,3}(?:[\\s\\u00a0\\u202f'${other === '.' ? '\\.' : ','}]\\d{3})+$`).test(int)) return null;
        int = int.replace(/\D/g, '');
    }
    if (int.length > 1 && int[0] === '0') return null;     // 0612…, 01000: a code, not a quantity
    return { pre, neg: sign === '-', int, frac, post };
}
function numberOut(p, out, decimals) {
    let { int, frac } = p;
    if (decimals !== '') {
        const x = Number(int + '.' + (frac || '0')).toFixed(+decimals).split('.');
        int = x[0]; frac = x[1] || null;
    }
    const [grp, dec] = { point: ['', '.'], comma: ['', ','], fr: [' ', ','], en: [',', '.'] }[out];
    if (grp) int = int.replace(/\B(?=(\d{3})+$)/g, grp);
    return p.pre + (p.neg ? '-' : '') + int + (frac ? dec + frac : '') + p.post;
}
/* Which character is the decimal separator in this column: a separator
   followed by other than three digits is one, the last of two different
   ones is one, a repeated one is not. A tie goes to the comma. */
function guessDecimal(values) {
    let comma = 0, point = 0;
    for (const v of values) {
        const s = v.replace(/[\s  ]/g, ''), c = s.split(',').length - 1, p = s.split('.').length - 1;
        if (c && p) { if (s.lastIndexOf(',') > s.lastIndexOf('.')) comma++; else point++; }
        else if (c > 1) point++;
        else if (p > 1) comma++;
        else if (c === 1 && !/,\d{3}(?!\d)/.test(s)) comma++;
        else if (p === 1 && !/\.\d{3}(?!\d)/.test(s)) point++;
    }
    return point > comma ? '.' : ',';
}

/* Phone numbers become E.164 first (+33612345678); a national number
   (leading 0) takes the country code given; a French mobile whose 0 a
   spreadsheet ate (612345678) is recognised when that code is 33, as is
   33612345678 without its +. */
function parsePhoneCell(s, cc) {
    let x = s.replace(/\(0\)/g, '').replace(/[\s .\-()\/]/g, '');
    if (x.startsWith('00')) x = '+' + x.slice(2);
    if (!x.startsWith('+')) {
        if (!cc) return null;
        if (/^0\d{6,14}$/.test(x)) x = '+' + cc + x.slice(1);
        else if (cc === '33' && /^[1-9]\d{8}$/.test(x)) x = '+33' + x;
        else if (cc === '33' && /^330?[1-9]\d{8}$/.test(x)) x = '+33' + x.slice(-9);   // the + lost, sometimes the 0 kept
        else return null;
    }
    if (/^\+330[1-9]\d{8}$/.test(x)) x = '+33' + x.slice(-9);    // +33 06…: the national 0 kept after the code
    if (!/^\+[1-9]\d{7,14}$/.test(x)) return null;
    if (x.startsWith('+33') && x.length !== 12) return null;
    return x;
}
function phoneOut(e164, out, cc) {
    const fr = e164.startsWith('+33') && e164.length === 12, pairs = s => s.replace(/(\d{2})(?=\d)/g, '$1 ');
    if (out === 'intl') return fr ? '+33 ' + e164[3] + ' ' + pairs(e164.slice(4)) : e164;
    if (out === 'nat' && cc && e164.startsWith('+' + cc)) { const n = '0' + e164.slice(1 + cc.length); return fr ? pairs(n) : n; }
    return e164;
}

/* Invisible format characters (U+202D/U+202C around a number pasted from a
   phone's contact card, zero-width spaces) are dropped before reading. */
const clean = v => v.replace(/\p{Cf}/gu, '').trim();
function convertFn() {
    const kind = document.querySelector('input[name="cv-kind"]:checked').value;
    if (kind === 'date') {
        const mf = document.getElementById('cv-dorder').value === 'mdy', out = DATE_OUT[document.getElementById('cv-dout').value];
        return v => {
            const p = parseDateCell(clean(v), mf); if (!p) return null;
            let rest = p.rest;
            if (rest && document.getElementById('cv-dout').value !== 'iso') rest = rest.replace(/^T/, ' ');
            return out(p) + rest;
        };
    }
    if (kind === 'num') {
        const dec = document.getElementById('cv-ndec').value, out = document.getElementById('cv-nout').value, n = document.getElementById('cv-nfix').value;
        return v => { const p = parseNumberCell(clean(v), dec); return p && numberOut(p, out, n); };
    }
    const cc = document.getElementById('cv-cc').value.replace(/\D/g, ''), out = document.getElementById('cv-pout').value;
    return v => { const e = parsePhoneCell(clean(v), cc); return e && phoneOut(e, out, cc); };
}

/* cells that change, how many are already right, and the ones not recognised */
function convertScan() {
    const t = T(), col = +document.getElementById('cv-col').value, f = convertFn();
    const changes = [], bad = new Map(); let same = 0;
    visitRows(t, t.filteredData, r => {
        const v = cellStr(cellOf(r, col)); if (!v.trim()) return;
        const nv = f(v);
        if (nv === null) bad.set(v, (bad.get(v) || 0) + 1);
        else if (nv === v) same++;
        else changes.push([r, cellOf(r, col), nv]);
    });
    return { col, changes, same, bad };
}

function openConvert(col) {
    const t = T(); if (!t || !t.loaded) return;
    const sel = document.getElementById('cv-col'), prev = sel.value;
    sel.innerHTML = t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    if (col != null) sel.value = col;
    else if (prev !== '' && +prev < t.headers.length) sel.value = prev;
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-convert').style.display = 'block';
    convertColChanged();
}

/* A new column: guess what it holds (the kind that recognises most of its
   cells, a phone-like title breaking ties) and how it is written. */
function convertColChanged() {
    const t = T(), col = +document.getElementById('cv-col').value;
    const vals = [];
    for (const r of t.filteredData) { const v = clean(cellStr(cellOf(r, col))); if (v) vals.push(v); if (vals.length >= 2000) break; }
    let dm = 0, md = 0;
    for (const v of vals) {
        const m = v.match(/^(\d{1,2})[-\/.](\d{1,2})[-\/.]\d{2,4}/);
        if (m) { if (+m[1] > 12) dm++; if (+m[2] > 12) md++; }
    }
    const monthFirst = md > 0 && !dm, dec = guessDecimal(vals), cc = document.getElementById('cv-cc').value.replace(/\D/g, '') || '33';
    document.getElementById('cv-dorder').value = monthFirst ? 'mdy' : 'dmy';
    document.getElementById('cv-ndec').value = dec;
    document.getElementById('cv-dnote').textContent = dm || md
        ? `Detected: ${monthFirst ? 'month' : 'day'} first (${fmt(monthFirst ? md : dm)} values could only be read that way).`
        : 'No value tells day from month here: day first is assumed.';
    const phoneTitle = /\b(tel|telephone|phone|mobile|portable|fax|gsm|cell)/.test(slugify(t.headers[col]).replace(/number/g, ' '));
    const score = {
        date: vals.filter(v => parseDateCell(v, monthFirst)).length,
        num: vals.filter(v => parseNumberCell(v, dec)).length,
        phone: vals.filter(v => parsePhoneCell(v, cc)).length + (phoneTitle ? 0.5 : 0)
    };
    const best = Object.keys(score).reduce((a, b) => score[b] > score[a] ? b : a, 'date');
    document.querySelector(`input[name="cv-kind"][value="${phoneTitle && score.phone >= score.num ? 'phone' : best}"]`).checked = true;
    convertRefresh();
}

function convertRefresh() {
    const t = T(); if (!t) return;
    const kind = document.querySelector('input[name="cv-kind"]:checked').value;
    ['date', 'num', 'phone'].forEach(k => document.getElementById('cv-o-' + k).style.display = k === kind ? '' : 'none');
    const { col, changes, same, bad } = convertScan();
    const nBad = [...bad.values()].reduce((a, b) => a + b, 0);
    const eg = [...bad.keys()].slice(0, 3).map(v => `"${esc(v.length > 30 ? v.slice(0, 29) + '…' : v)}"`).join(', ');
    document.getElementById('cv-stats').innerHTML = (changes.length ? `<b>${fmt(changes.length)}</b> cells will change` : 'No cell would change')
        + (same ? ` · ${fmt(same)} already in this format` : '')
        + ` — in the ${fmt(t.filteredData.length)} ${hasFilter(t) ? 'filtered ' : ''}rows.`
        + (nBad ? `<br><span class="warn">${fmt(nBad)} cells not recognised, left as they are: ${eg}${bad.size > 3 ? '…' : ''}</span>` : '');
    const h = esc(t.headers[col]);
    document.getElementById('cv-pv').innerHTML = changes.length
        ? `<tr><th style="padding: 4px 8px;">${h}</th><th style="padding: 4px 8px; color: var(--prim);">converted</th></tr>`
            + changes.slice(0, 8).map(([, o, n]) => `<tr><td class="src">${esc(cellStr(o))}</td><td>${esc(n)}</td></tr>`).join('')
        : '<tr><td class="empty">Nothing to convert with these settings.</td></tr>';
    document.getElementById('cv-go').disabled = !changes.length;
    document.getElementById('cv-show').style.display = nBad ? '' : 'none';
}

function applyConvert() {
    const t = T(); if (!t) return;
    const { col, changes: ch, bad } = convertScan();
    closeAllModals();
    if (!ch.length) return;
    const ed = rowEdits();
    for (const [r, , nv] of ch) ed.set(r, col, nv);
    t.modificationsLog.push({ id: '-', col: t.headers[col], old: 'convert', new: `${ch.length} cells`, what: `${fmt(ch.length)} cells converted in ${t.headers[col]}`,
        undo: () => ed.undo() });
    updateSaveBtn(); renderTabBar(); render();
    flash(ch.map(([r]) => [r, col]));
    setStats(`${t.name} | ${fmt(ch.length)} cells converted in ${t.headers[col]}${bad.size ? `, ${fmt([...bad.values()].reduce((a, b) => a + b, 0))} not recognised` : ''} — not written yet, use Save.`);
}

/* Filter the grid down to the cells the converter did not recognise: a
   value filter excluding every other value of the column. */
function showUnrecognised() {
    const t = T(); if (!t) return;
    const { col, bad } = convertScan();
    closeAllModals();
    const ex = new Set();
    visitRows(t, t.allData, r => { const v = cellStr(cellOf(r, col)); if (!bad.has(v)) ex.add(v); });
    t.valFilters[col] = ex;
    renderHeader(); applyColStyles(); applyFilters();
    setStats(`${t.name} | ${fmt(t.filteredData.length)} rows whose "${t.headers[col]}" was not recognised — clear the filter from the column's ▾.`);
}
