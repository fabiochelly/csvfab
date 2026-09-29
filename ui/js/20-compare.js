/* ---------------------------------------------------------------
   COMPARE WITH ANOTHER TAB
   Two versions of one export (Monday's and today's): rows matched on a
   key column, the other columns paired by name. Keys and values are
   compared the way the lookup compares keys (exact, loose, slug). The
   first row wins when a key repeats in the other tab, as in the lookup.
   Results: marks (tinted cells, the other value in the tooltip), a
   status column, or the rows only the other tab has, appended here.
----------------------------------------------------------------*/
let cmp = null;          // { src, rows, userKey, res }
const plural = (k, w) => `${fmt(k)} ${w}${k === 1 ? '' : 's'}`;

function openCompare() {
    const t = T(); if (!t || !t.loaded) return;
    const others = tabs.filter(x => x !== t);
    if (!others.length) {
        uiAlert('Open the other version of the file first.\n\nThe comparison reads another open tab: rows are matched on a key column (an ID, an e-mail…), and the columns with the same name are compared.');
        return;
    }
    const key = document.getElementById('cmp-key');
    key.innerHTML = t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    const src = document.getElementById('cmp-src'), prev = +src.value;
    src.innerHTML = others.map(x => `<option value="${x.id}">${esc(x.name)}</option>`).join('');
    if (others.some(x => x.id === prev)) src.value = prev;
    cmp = { userKey: false };
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-compare').style.display = 'block';
    compareSource();
}

async function compareSource() {
    const t = T(), src = tabs.find(x => x.id === +document.getElementById('cmp-src').value);
    if (!src || !cmp) return;
    cmp = { userKey: cmp.userKey, src, rows: null };
    document.getElementById('cmp-skey').innerHTML = '';
    compareRefresh();
    const rows = await tabRows(src);
    if (!cmp || cmp.src !== src) return;
    if (!rows) { cmp.error = `Cannot read ${src.name}.`; compareRefresh(); return; }
    cmp.rows = rows;
    /* Key columns: the same name on both sides, preferably. */
    const keySel = document.getElementById('cmp-key'), srcSlugs = src.headers.map(h => slugify(h));
    let k = +keySel.value, sk = srcSlugs.indexOf(slugify(t.headers[k]));
    if (sk < 0 && !cmp.userKey) {
        const c = t.headers.findIndex(h => srcSlugs.includes(slugify(h)));
        if (c >= 0) { keySel.value = k = c; sk = srcSlugs.indexOf(slugify(t.headers[c])); }
    }
    document.getElementById('cmp-skey').innerHTML = src.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    document.getElementById('cmp-skey').value = Math.max(0, sk);
    compareRefresh();
}

function compareResult() {
    const t = T(), key = +document.getElementById('cmp-key').value, skey = +document.getElementById('cmp-skey').value;
    const norm = LK_NORM[document.querySelector('input[name="cmp-match"]:checked').value], src = cmp.src;
    const oBySlug = new Map();
    src.headers.forEach((h, i) => { const s = slugify(h); if (i !== skey && !oBySlug.has(s)) oBySlug.set(s, i); });
    const pairs = [];
    t.headers.forEach((h, c) => { if (c === key) return; const o = oBySlug.get(slugify(h)); if (o !== undefined && !pairs.some(p => p[1] === o)) pairs.push([c, o]); });
    const onlyHereCols = t.headers.filter((h, c) => c !== key && !pairs.some(p => p[0] === c));
    const onlyThereCols = src.headers.filter((h, o) => o !== skey && !pairs.some(p => p[1] === o));

    /* Keys and differences are worked out in file order (visitRows), the
       results then gathered in each tab's own order: the first row wins. */
    const idx = new Map(), oRows = cmp.rows, oKeys = new Array(oRows.length); let dupThere = 0;
    visitRows(src, oRows, (r, i) => { oKeys[i] = norm(cellStr(cellOf(r, skey))); });
    for (let i = 0; i < oRows.length; i++) {
        const k = oKeys[i];
        if (!k.trim()) continue;
        if (idx.has(k)) dupThere++; else idx.set(k, oRows[i]);
    }
    const rows = t.allData, keys = new Array(rows.length), diffs = new Array(rows.length);
    visitRows(t, rows, (r, i) => {
        const k = keys[i] = norm(cellStr(cellOf(r, key)));
        const o = k.trim() && idx.get(k);
        if (!o) return;
        let cells = null;
        for (const [c, oc] of pairs) {
            const a = cellStr(cellOf(r, c)), b = cellStr(cellOf(o, oc));
            if (a !== b && norm(a) !== norm(b)) (cells = cells || []).push([c, b]);
        }
        diffs[i] = cells;
    });
    const hit = new Set(), onlyHere = [], changed = [];
    let same = 0, noKey = 0;
    for (let i = 0; i < rows.length; i++) {
        const r = rows[i], k = keys[i];
        if (!k.trim()) { noKey++; continue; }
        if (!idx.has(k)) { onlyHere.push(r); continue; }
        hit.add(k);
        if (diffs[i]) changed.push({ r, cells: diffs[i] }); else same++;
    }
    const onlyThere = [];
    for (const [k, r] of idx) if (!hit.has(k)) onlyThere.push(r);
    return { key, skey, pairs, onlyHereCols, onlyThereCols, onlyHere, onlyThere, changed, same, noKey, dupThere };
}

function compareRefresh() {
    const t = T(); if (!t || !cmp) return;
    const stats = document.getElementById('cmp-stats'), pv = document.getElementById('cmp-pv');
    const btns = ['cmp-go', 'cmp-col', 'cmp-add'].map(id => document.getElementById(id));
    btns.forEach(b => b.disabled = true);
    pv.innerHTML = '';
    if (cmp.error || !cmp.rows) { stats.innerHTML = cmp.error ? `<span class="warn">${esc(cmp.error)}</span>` : `Reading ${esc(cmp.src ? cmp.src.name : '')}…`; return; }
    const res = cmp.res = compareResult(), other = esc(cmp.src.name);
    const n = (k, w) => `<b>${fmt(k)}</b> ${w}`;
    stats.innerHTML = [
        n(res.changed.length, 'changed'), n(res.same, 'identical'),
        n(res.onlyHere.length, 'only here'), `${n(res.onlyThere.length, 'only in')} ${other}`
    ].join(' · ')
        + `<br>Compared on ${res.pairs.length} common column${res.pairs.length === 1 ? '' : 's'}`
        + (res.onlyHereCols.length ? ` · <span title="${esc(res.onlyHereCols.join(', '))}">${res.onlyHereCols.length} only here</span>` : '')
        + (res.onlyThereCols.length ? ` · <span title="${esc(res.onlyThereCols.join(', '))}">${res.onlyThereCols.length} only in ${other}</span>` : '')
        + (res.noKey ? ` · <span class="warn">${plural(res.noKey, 'row')} without a key</span>` : '')
        + (res.dupThere ? ` · <span class="warn" title="The first row of each key is used">${plural(res.dupThere, 'repeated key')} in ${other}</span>` : '');
    if (!res.pairs.length && !res.onlyHere.length && !res.onlyThere.length) stats.innerHTML += '<br><span class="warn">No column in common besides the key.</span>';

    const cut = v => { const s = cellStr(v); return s === '' ? '<span class="empty">empty</span>' : esc(s.length > 60 ? s.slice(0, 60) + '…' : s); };
    let html = `<tr><th></th><th>${esc(t.headers[res.key])}</th><th>Differences</th></tr>`, k = 0;
    for (const { r, cells } of res.changed) {
        if (k++ >= 60) break;
        html += `<tr><td class="k chg">changed</td><td>${cut(r.data[res.key])}</td><td>${cells.map(([c, b]) => `${esc(t.headers[c])}: <span class="new">${cut(r.data[c])}</span> <span class="muted">/</span> <span class="old">${cut(b)}</span>`).join(' · ')}</td></tr>`;
    }
    res.onlyHere.slice(0, 20).forEach(r => html += `<tr><td class="k add">only here</td><td>${cut(r.data[res.key])}</td><td></td></tr>`);
    res.onlyThere.slice(0, 20).forEach(r => html += `<tr><td class="k del">only there</td><td>${cut(r.data[res.skey])}</td><td></td></tr>`);
    pv.innerHTML = html;
    btns[0].disabled = !res.changed.length && !res.onlyHere.length;
    btns[1].disabled = false;
    btns[2].disabled = !res.onlyThere.length;
    btns[2].textContent = res.onlyThere.length ? `Append the ${fmt(res.onlyThere.length)} rows only there` : 'Append the rows only there';
}

/* Changed rows and rows only here, as marks; the tooltip gives the other tab's value. */
function compareMark() {
    const t = T(), res = cmp && cmp.res; if (!t || !res) return;
    const rows = new Set(res.onlyHere), cells = new Map();
    for (const { r, cells: cs } of res.changed) { rows.add(r); cells.set(r, new Map(cs.map(([c, b]) => [t.headers[c], b]))); }
    closeAllModals();
    setRowMark(t, { kind: 'compare', label: `differences with ${cmp.src.name}`, tip: `In ${cmp.src.name}`, rows, cells, only: true });
    setStats(`${t.name} | ${plural(res.changed.length, 'changed row')} and ${plural(res.onlyHere.length, 'row')} only here, marked — hover a tinted cell to see the value in ${cmp.src.name}.`);
}

/* One column at the end: "changed: A, B", "same", "only here" (empty without a key). */
function compareStatusColumn() {
    const t = T(), res = cmp && cmp.res; if (!t || !res) return;
    const status = new Map();
    res.onlyHere.forEach(r => status.set(r, 'only here'));
    res.changed.forEach(({ r, cells }) => status.set(r, 'changed: ' + cells.map(([c]) => t.headers[c]).join(', ')));
    const norm = LK_NORM[document.querySelector('input[name="cmp-match"]:checked').value];
    const n = t.headers.length, name = `vs ${cmp.src.name}`;
    closeAllModals();
    restructure(t, t.headers.concat([name]),
        (d, r) => { const out = pad(d, n).slice(0, n); out.push(status.get(r) || (norm(cellStr(d[res.key])).trim() ? 'same' : '')); return out.concat(d.slice(n)); },
        c => c, `column "${name}" added`);
}

/* The rows only the other tab has, their columns matched by name. */
function compareAppend() {
    const t = T(), res = cmp && cmp.res; if (!t || !res || !res.onlyThere.length) return;
    const map = t.headers.map((h, c) => c === res.key ? res.skey : (res.pairs.find(p => p[0] === c) || [0, -1])[1]);
    const added = res.onlyThere.map(o => newRow(t, map.map(oc => oc < 0 ? '' : cellStr(o.data[oc]))));
    const from = cmp.src.name;
    closeAllModals();
    commitRows(t, t.allData.concat(added), { id: '-', col: '---', old: 'append', new: `${added.length} rows`, what: `${plural(added.length, 'row')} appended from ${from}` });
    setStats(`${t.name} | ${plural(added.length, 'row')} only in ${from} appended at the end`
        + (res.onlyThereCols.length ? ` (its columns ${res.onlyThereCols.join(', ')} have no match here and were left out)` : '') + ' — not written yet, use Save.');
}
