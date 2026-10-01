/* ---------------------------------------------------------------
   DUPLICATES
   Rows are duplicates when ANY of several keys match (e-mail, OR phone,
   OR last name + first name + postal code), each key being one or more
   fields that must ALL match, each field read through its own cleaning
   (DUP_FNS: exact, case and spaces, slug, digits, phone, e-mail, or a
   formula on v with the FX helpers). A key counts only when every one of
   its fields is filled once cleaned: two contacts without an e-mail are
   not one contact. With no field at all, the whole row is the key.
   Groups are chained (union-find): A and B share an e-mail, B and C a
   phone, so A, B and C are one group — the same person seen three ways.
   t.dupSpec = { keys: [{ fields: [{ col: name, fn, expr }] }] } holds
   columns by NAME, so marks survive columns being moved; a name gone
   drops the spec. t.dupDraft is the dialog's state between openings.

   Marks (t.dupMarks = { group: Map(row → g), groups, rows: [[row]],
   diff: Map(g → …) }) are recomputed at each applyFilters() while the
   spec holds. In the grid: an edge per group, and the cells of a group
   whose filled values differ in red text (.dfc), computed per group when
   drawn (dupCellCls). Hovering a
   marked row shows a bar (#dup-bar): delete it, or merge it with the
   next row of its group — keeping the TOP row's value where both are
   filled, or the BOTTOM one's; empty cells take the other's value.
----------------------------------------------------------------*/
const DUP_FNS = [
    ['loose', 'Ignore case and spaces', v => v.trim().replace(/\s+/g, ' ').toLowerCase()],
    ['exact', 'Exactly as written', v => v],
    ['slug', 'Slug: no case, accents, symbols', v => slugify(v)],
    ['digits', 'Digits only', v => v.replace(/\D+/g, '')],
    ['phone', 'Phone number', v => { const x = v.replace(/\p{Cf}/gu, '').trim(); return x ? (parsePhoneCell(x, '33') || x.replace(/\D+/g, '')) : ''; }],
    ['email', 'E-mail', v => v.trim().toLowerCase().replace(/^mailto:/, '')],
    ['fx', 'Formula on v…', null]
];
const DUP_FN = Object.fromEntries(DUP_FNS.map(([k, , f]) => [k, f]));
const dupLoose = DUP_FN.loose;

/* A formula field: v is the cell's text, every FX helper is there — firstNumber(v), left(slug(v), 5).
   Compiled once per text; { fn } or { error }. */
function dupFormula(src) {
    const c = dupFormula.cache || (dupFormula.cache = new Map());
    if (c.has(src)) return c.get(src);
    let out;
    try { const f = new Function('v', ...Object.keys(FX), `"use strict"; return (${src}\n);`); out = { fn: v => fxOut(f(v, ...FX_VALUES)) }; }
    catch (e) { out = { error: e.message }; }
    if (c.size > 100) c.clear();
    c.set(src, out);
    return out;
}
/* The spec compiled against the tab's columns: [[{c, f}]] per key, [] for the whole row;
   null when a column is gone; { error } for a formula that does not compile. */
function dupCompile(t, spec) {
    const keys = [];
    for (const k of spec.keys) {
        const fields = [];
        for (const fd of k.fields) {
            if (fd.col == null || fd.col === '') continue;          // a field not set up yet
            const c = t.headers.indexOf(fd.col);
            if (c < 0) return null;
            let f = DUP_FN[fd.fn] || dupLoose;
            if (fd.fn === 'fx') {
                if (!String(fd.expr || '').trim()) f = dupLoose;
                else { const x = dupFormula(fd.expr); if (x.error) return { error: `${fd.col}: ${x.error}` }; f = x.fn; }
            }
            fields.push({ c, f });
        }
        if (fields.length) keys.push(fields);
    }
    return keys;
}
/* Every row's group, by union-find over ALL rows (filtered or not): the root is the
   group's first row in file order. Also how many rows each key ties to another. */
function dupUnion(t, spec) {
    const keys = dupCompile(t, spec);
    if (!keys || keys.error) return keys;
    const n = t.allData.length, parent = new Int32Array(n);
    for (let i = 0; i < n; i++) parent[i] = i;
    const find = i => { while (parent[i] !== i) { parent[i] = parent[parent[i]]; i = parent[i]; } return i; };
    const union = (a, b) => { a = find(a); b = find(b); if (a !== b) { if (a < b) parent[b] = a; else parent[a] = b; } };
    const whole = !keys.length, seen = keys.map(() => new Map()), hits = keys.map(() => 0);
    const tied = keys.map(() => new Uint8Array(n));
    const few = keys.every(k => k.length <= 3);            // a few cells: read each one; else one split of the record
    visitRows(t, t.allData, (r, i) => {
        if (whole) {
            const d = r.data; let key = '', empty = true;
            for (let c = 0; c < d.length; c++) { const p = dupLoose(cellStr(d[c])); if (p) empty = false; key += (c ? '\u0001' : '') + p; }
            if (empty) return;
            const j = seen[0] ? seen[0].get(key) : undefined;
            if (!seen[0]) seen[0] = new Map();
            if (j === undefined) seen[0].set(key, i); else union(i, j);
            return;
        }
        const d = few ? null : r.data;
        for (let k = 0; k < keys.length; k++) {
            let key = '', ok = true;
            for (const { c, f } of keys[k]) {
                let p; try { p = f(cellStr(few ? cellOf(r, c) : d[c])); } catch (e) { p = ''; }
                if (!p) { ok = false; break; }                     // a field empty once cleaned: this key does not count
                key += '\u0001' + p;
            }
            if (!ok) continue;
            const j = seen[k].get(key);
            if (j === undefined) seen[k].set(key, i);
            else { union(i, j); if (!tied[k][j]) { tied[k][j] = 1; hits[k]++; } if (!tied[k][i]) { tied[k][i] = 1; hits[k]++; } }
        }
    });
    const root = new Int32Array(n), size = new Int32Array(n);
    for (let i = 0; i < n; i++) { root[i] = find(i); size[root[i]]++; }
    return { root, size, hits };
}
/* Which rows a deletion keeps: one per group, its first (or last) row. */
function dedupeKeep(t, spec, last) {
    const u = dupUnion(t, spec); if (!u || u.error) return u;
    const n = t.allData.length, keep = new Array(n), lastOf = new Int32Array(n).fill(-1);
    if (last) for (let i = 0; i < n; i++) lastOf[u.root[i]] = i;
    for (let i = 0; i < n; i++) keep[i] = u.size[u.root[i]] < 2 || (last ? lastOf[u.root[i]] === i : u.root[i] === i);
    return { keep, u };
}

/* ---- MARKS ----------------------------------------------------------- */
function dupGroups(t) {
    const spec = t.dupSpec && JSON.stringify(t.dupSpec);
    if (spec && t.dupCache && t.dupCache.spec === spec && sameStamp(t.dupCache.stamp, dataStamp(t))) return;   // same rows, same spec: same groups
    t.dupMarks = null; t.dupCache = null;
    if (!t.dupSpec) return;
    const u = dupUnion(t, t.dupSpec);
    if (!u || u.error) { t.dupSpec = null; t.onlyDups = false; return; }   // a compared column is gone
    const group = new Map(), num = new Map(), rows = [];
    t.allData.forEach((r, i) => {
        const g0 = u.root[i];
        if (u.size[g0] < 2) return;
        if (!num.has(g0)) { num.set(g0, rows.length); rows.push([]); }
        const g = num.get(g0);
        group.set(r, g); rows[g].push(r);
    });
    t.dupMarks = { group, groups: rows.length, rows, diff: new Map() };
    t.dupCache = { spec, stamp: dataStamp(t) };
}
/* Per group, the cells of the columns that differ (conf): two filled values that differ, or a
   value one row has and another lacks — every cell of such a column is marked, the empty ones
   included, so the gap shows. Computed when a row of the group is drawn, kept until the marks
   change. Values
   are compared as the keys read them — a key field through its own cleaning, so ana@x.io and
   ANA@X.IO, or two ways of writing one phone number, are not a difference; any other column
   ignoring case and spaces. */
function dupDiff(t, g) {
    const m = t.dupMarks; let d = m.diff.get(g);
    if (d) return d;
    if (!m.clean) {
        m.clean = new Map();
        const keys = dupCompile(t, t.dupSpec);
        if (keys && !keys.error) for (const k of keys) for (const { c, f } of k) if (!m.clean.has(c)) m.clean.set(c, f);
    }
    const rows = m.rows[g], conf = new Map();
    const width = Math.max(t.headers.length, ...rows.map(r => r.len || 0));
    const vals = rows.map(r => r.data);
    for (let c = 0; c < width; c++) {
        const set = new Set(), f = m.clean.get(c) || dupLoose; let empty = 0;
        for (const dv of vals) {
            const v = cellStr(dv[c]); if (!v.trim()) { empty++; continue; }
            let k; try { k = f(v); } catch (e) { k = v; }
            set.add(k || v.trim());
        }
        if (set.size < 2 && !(set.size && empty)) continue;   // a value one row has and another lacks is a difference too
        rows.forEach(r => { let s = conf.get(r); if (!s) conf.set(r, s = new Set()); s.add(c); });   // every cell of the column, the empty ones included
    }
    d = { conf };
    if (m.diff.size > 2000) m.diff.clear();
    m.diff.set(g, d);
    return d;
}
function dupCellCls(t, r, c) {
    const g = t.dupMarks.group.get(r);
    if (g == null) return '';
    const a = dupDiff(t, g).conf.get(r);
    return a && a.has(c) ? ' dfc' : '';
}
function dupCls(t, r, prev) {
    const g = t.dupMarks && t.dupMarks.group.get(r);
    if (g == null) return '';             // no marks at all (dupMarks null) or a row outside every group
    return ' dup ' + (g % 2 ? 'dup-b' : 'dup-a') + (t.onlyDups && prev && t.dupMarks.group.get(prev) !== g ? ' dup-first' : '');
}
/* The spec in words, for the chip and the status bar. */
function dupSpecText(spec) {
    const ks = spec.keys.map(k => k.fields.filter(f => f.col).map(f => f.col + (f.fn && f.fn !== 'loose' ? ` (${f.fn === 'fx' ? f.expr : f.fn})` : '')).join(' + ')).filter(Boolean);
    return ks.length ? ks.join(' OR ') : 'the whole row';
}
function markDuplicates() {
    const t = T(); if (!t) return;
    const spec = dedupeDraftSpec(); if (!spec) return;
    t.dupSpec = spec; t.onlyDups = true;
    closeAllModals();
    applyFilters();
    if (!t.dupMarks || !t.dupMarks.groups) { t.dupSpec = null; t.onlyDups = false; applyFilters(); setStats(`${t.name} | No duplicate rows.`); return; }
    setStats(`${t.name} | ${fmt(t.dupMarks.group.size)} rows in ${fmt(t.dupMarks.groups)} duplicate groups, side by side — hover a row to delete it or merge it with the next; differing values in red.`);
}
function toggleDupView() {
    const t = T(); if (!t || !t.dupSpec) return;
    t.onlyDups = !t.onlyDups;
    applyFilters();
}
function clearDupMarks(e) {
    if (e) e.stopPropagation();
    const t = T(); if (!t) return;
    t.dupSpec = null; t.onlyDups = false; t.dupMarks = null;
    dupBarHide(); dupCellHide();
    applyFilters();
}

/* ---- DIALOG ---------------------------------------------------------- */
function dedupeNewField(t, c) {
    const kind = c >= 0 ? anonGuess(t, c) : '';
    const fn = kind === 'email' ? 'email' : kind === 'phone' ? 'phone' : kind === 'postal' ? 'digits' : ['first', 'last', 'full', 'company', 'address'].includes(kind) ? 'slug' : 'loose';
    return { col: c >= 0 ? t.headers[c] : '', fn, expr: '' };
}
/* Keys from the columns' titles and values: last name + first name (+ postal code), e-mail, phone. */
function dedupeSuggest() {
    const t = T(); if (!t) return;
    const kinds = t.headers.map((_, c) => anonGuess(t, c)), of = k => kinds.indexOf(k), keys = [];
    const last = of('last'), first = of('first'), full = of('full'), postal = of('postal');
    const name = last >= 0 && first >= 0 ? [last, first] : full >= 0 ? [full] : last >= 0 ? [last] : [];
    if (name.length) keys.push({ fields: [...name, ...(postal >= 0 ? [postal] : [])].map(c => dedupeNewField(t, c)) });
    kinds.forEach((k, c) => { if (k === 'email' || k === 'phone') keys.push({ fields: [dedupeNewField(t, c)] }); });
    if (!keys.length) { setStats('No name, e-mail or phone column recognised: build the keys by hand.'); return; }
    t.dupDraft.keys = keys;
    renderDedupe();
}
function openDedupe() {
    const t = T(); if (!t || !t.loaded) return;
    const ok = s => s && s.keys && s.keys.every(k => k.fields.every(f => !f.col || t.headers.includes(f.col)));
    t.dupDraft = JSON.parse(JSON.stringify(ok(t.dupSpec) ? t.dupSpec : ok(t.dupDraft) ? t.dupDraft : { keys: [{ fields: [dedupeNewField(t, -1)] }] }));
    renderDedupe();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-dedupe').style.display = 'block';
}
function renderDedupe() {
    const t = T(), d = t.dupDraft;
    const colOpts = sel => `<option value="">Column…</option>` + t.headers.map(h => `<option${h === sel ? ' selected' : ''} value="${esc(h)}">${esc(h)}</option>`).join('');
    const fnOpts = sel => DUP_FNS.map(([k, l]) => `<option value="${k}"${k === sel ? ' selected' : ''}>${esc(l)}</option>`).join('');
    document.getElementById('dedupe-keys').innerHTML = d.keys.map((k, ki) => (ki ? '<div class="dk-or">or</div>' : '')
        + `<div class="dk-key"><div class="dk-head"><b>Key ${ki + 1}</b><span>${k.fields.length > 1 ? 'every field must match' : 'this field must match'}</span>`
        + (d.keys.length > 1 ? `<button class="dk-x" onclick="dedupeEdit(${ki}, -1, 'dropKey')" title="Remove this key">×</button>` : '') + '</div>'
        + k.fields.map((f, fi) => `<div class="dk-field">`
            + `<select class="bs-input bs-select dk-col" onchange="dedupeEdit(${ki}, ${fi}, 'col', this.value)">${colOpts(f.col)}</select>`
            + `<select class="bs-input bs-select dk-fn" onchange="dedupeEdit(${ki}, ${fi}, 'fn', this.value)">${fnOpts(f.fn)}</select>`
            + (f.fn === 'fx' ? `<input type="text" class="bs-input dk-expr" spellcheck="false" autocomplete="off" placeholder="firstNumber(v)" value="${esc(f.expr || '')}" oninput="dedupeEdit(${ki}, ${fi}, 'expr', this.value)" title="v is the cell's text, e.g. left(slug(v), 5)">` : '')
            + `<button class="dk-x" onclick="dedupeEdit(${ki}, ${fi}, 'drop')" title="Remove this field">×</button></div>`).join('')
        + `<button class="dk-add" onclick="dedupeEdit(${ki}, -1, 'add')">+ Field</button></div>`).join('');
    dedupeNoteSoon();
}
function dedupeEdit(ki, fi, what, v) {
    const t = T(), d = t.dupDraft;
    if (what === 'add') d.keys[ki].fields.push(dedupeNewField(t, -1));
    else if (what === 'addKey') d.keys.push({ fields: [dedupeNewField(t, -1)] });
    else if (what === 'dropKey') d.keys.splice(ki, 1);
    else if (what === 'drop') { d.keys[ki].fields.splice(fi, 1); if (!d.keys[ki].fields.length && d.keys.length > 1) d.keys.splice(ki, 1); }
    else if (what === 'col') { const f = d.keys[ki].fields[fi]; Object.assign(f, dedupeNewField(t, t.headers.indexOf(v)), f.fn === 'fx' ? { fn: 'fx', expr: f.expr } : {}); }
    else if (what === 'fn') d.keys[ki].fields[fi].fn = v;
    else if (what === 'expr') { d.keys[ki].fields[fi].expr = v; return dedupeNoteSoon(); }   // typing: no redraw, the field keeps its focus
    renderDedupe();
}
/* The dialog's spec, or null (and the note says why) when a formula does not compile. */
function dedupeDraftSpec() {
    const t = T(), c = dupCompile(t, t.dupDraft);
    if (c && c.error) return null;
    return JSON.parse(JSON.stringify({ keys: t.dupDraft.keys.map(k => ({ fields: k.fields.filter(f => f.col) })).filter(k => k.fields.length) }));
}
function dedupeNoteSoon() { clearTimeout(dedupeNoteSoon.timer); dedupeNoteSoon.timer = setTimeout(updateDedupeNote, 150); }
function updateDedupeNote() {
    const t = T(); if (!t || !t.dupDraft) return;
    const note = document.getElementById('dedupe-note'), go = document.getElementById('dedupe-go'), show = document.getElementById('dedupe-show');
    const c = dupCompile(t, t.dupDraft);
    document.querySelectorAll('#dedupe-keys .dk-expr').forEach(i => i.classList.remove('bad'));
    if (c && c.error) { note.innerHTML = `<span class="warn">${esc(c.error)}</span>`; go.disabled = show.disabled = true; return; }
    const last = document.querySelector('input[name="dedupe-keep"]:checked').value === 'last';
    const r = dedupeKeep(t, dedupeDraftSpec(), last); if (!r) return;
    const dup = r.keep.filter(k => !k).length;
    let groups = 0; for (let i = 0; i < r.u.size.length; i++) if (r.u.size[i] > 1) groups++;
    const spec = dedupeDraftSpec(), per = spec.keys.length > 1 ? ' — ' + spec.keys.map((k, i) => `key ${i + 1}: ${fmt(r.u.hits[i] || 0)} rows`).join(' · ') : '';
    note.innerHTML = dup
        ? `<b>${fmt(dup + groups)}</b> rows in <b>${fmt(groups)}</b> groups${per}. Delete keeps one row per group: <b>${fmt(dup)}</b> deleted (all ${fmt(t.allData.length)} rows are checked, filtered or not).`
        : `No duplicate among the ${fmt(t.allData.length)} rows${spec.keys.length ? '' : ' (compared on the whole row)'}.`;
    go.disabled = show.disabled = !dup;
}
function applyDedupe() {
    const t = T(); if (!t) return;
    const spec = dedupeDraftSpec(); if (!spec) return;
    const { keep } = dedupeKeep(t, spec, document.querySelector('input[name="dedupe-keep"]:checked').value === 'last');
    const rows = t.allData.filter((_, i) => keep[i]), dup = t.allData.length - rows.length;
    closeAllModals();
    if (!dup) return;
    commitRows(t, rows, { id: '-', col: '---', old: `${dup} duplicate rows`, new: 'Deleted', what: `${fmt(dup)} duplicates removed` });
    setStats(`${t.name} | ${fmt(dup)} duplicate rows deleted — not written yet, use Save.`);
}

/* ---- HOVER BAR: delete, merge ----------------------------------------
   One fixed bar, moved to the marked row under the pointer and centred on
   it, so everything on it is about THAT row: delete it, or merge its whole
   group into it — "Merge" (it keeps this row): its filled cells stay, each empty
   one takes the first value the other rows of the group have (in the order
   shown), and the other rows go. Only the rows shown take part: a row of
   the group hidden by another filter is neither read nor deleted. A value
   to keep from another row is copied first with the cell arrows. Hovering
   the button says what happens, in the row numbers: a check in place of
   this row's number, a red cross in place of each one that goes. The bar
   sits at the left, by the row numbers, and stays there when the grid
   scrolls sideways — a cell far right can be read while merging; only a
   vertical scroll, which moves the row, hides it. */
const dupBar = document.createElement('div'); dupBar.id = 'dup-bar'; document.body.appendChild(dupBar);
const DUP_ICON_DEL = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"></path><path d="M10 11v6M14 11v6"></path></svg>';
const DUP_ICON_MERGE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 4v4a4 4 0 0 0 4 4 4 4 0 0 1 4 4v4"></path><path d="M16 4v4a4 4 0 0 1-4 4"></path><polyline points="13 17 16 20 19 17"></polyline></svg>';
function dupBarHide() { dupBar.classList.remove('open'); dupBar._i = -1; dupMergeHint(false); dupGroupBox(null); }
/* The hovered row's group framed in dashed yellow: one frame per run of consecutive rows drawn
   (the duplicates view keeps a group in one run), from the row numbers to the grid's last
   column or the view's right edge, clipped below the header. Fixed boxes in a small pool, never in the rows' DOM. */
const dupBoxes = [];
function dupGroupBox(t, g) {
    let n = 0;
    if (t && g != null) {
        const vc = container.getBoundingClientRect(), top0 = vc.top + thead.offsetHeight, right = vc.left + container.clientWidth;
        const runs = []; let cur = null;
        for (const rowEl of [...tbody.querySelectorAll('.row[data-idx]')].sort((a, b) => a.dataset.idx - b.dataset.idx)) {
            const k = +rowEl.dataset.idx;
            if (t.dupMarks.group.get(t.filteredData[k]) !== g) { cur = null; continue; }
            const r = rowEl.getBoundingClientRect();
            if (cur && k === cur.k + 1) { cur.bottom = r.bottom; cur.k = k; } else runs.push(cur = { top: r.top, bottom: r.bottom, k, left: rowEl.querySelector('.col-idx').getBoundingClientRect().left, end: r.right });
        }
        for (const run of runs) {
            const top = Math.max(run.top, top0), bottom = Math.min(run.bottom, vc.top + container.clientHeight);
            if (bottom <= top) continue;
            let el = dupBoxes[n];
            if (!el) { el = document.createElement('div'); el.className = 'dup-box'; document.body.appendChild(el); dupBoxes.push(el); }
            Object.assign(el.style, { left: run.left + 'px', top: top + 'px', width: (Math.min(right, run.end) - run.left) + 'px', height: (bottom - top) + 'px', display: 'block' });   // to the grid's last column or the view's edge
            n++;
        }
    }
    for (let k = n; k < dupBoxes.length; k++) dupBoxes[k].style.display = 'none';
}   // the cell arrows have their own life: the pointer leaves the grid for them
/* The rows of row r's group among the rows shown, as view indices. */
function dupShown(t, g) { const out = []; t.filteredData.forEach((x, k) => { if (t.dupMarks.group.get(x) === g) out.push(k); }); return out; }
function dupBarAt(rowEl) {
    const t = T(); if (!t || !t.dupMarks || !rowEl) return dupBarHide();
    const i = +rowEl.dataset.idx, r = t.filteredData[i], g = t.dupMarks.group.get(r);
    if (g == null) return dupBarHide();
    if (dupBar._i === i && dupBar.classList.contains('open')) return;
    const n = t.onlyDups ? null : dupShown(t, g).length;      // the duplicates view shows every row of the group
    const others = (n == null ? t.dupMarks.rows[g].length : n) - 1;
    dupBar._i = i;
    dupBar.innerHTML = `<button class="db-del" onclick="dupBarDelete()" title="Delete this row">${DUP_ICON_DEL}</button>`
        + (others > 0 ? `<button class="db-m" onclick="dupMerge(${i})" onmouseenter="dupMergeHint(true)" onmouseleave="dupMergeHint(false)" data-tip="${others === 1 ? 'Keep this row, fill its empty cells from the other, delete the other' : `Keep this row, fill its empty cells from the ${others} others, delete them`}">${DUP_ICON_MERGE}<span>Merge</span></button>` : '');
    const idx = rowEl.querySelector('.col-idx').getBoundingClientRect(), rr = rowEl.getBoundingClientRect();
    dupBar.classList.add('open');
    dupBar.style.left = (idx.right + 4) + 'px';
    dupBar.style.top = (rr.top + (rr.height - dupBar.offsetHeight) / 2) + 'px';
    dupBar._top = container.scrollTop;
    dupGroupBox(t, g);
}
/* The merge button's explanation, shown at once (no title delay) and clear of the rows it
   concerns: under the group's last row drawn, or above its first when the view ends first. */
const dupTip = document.createElement('div'); dupTip.id = 'dup-tip'; document.body.appendChild(dupTip);
function dupMergeHint(on) {
    tbody.querySelectorAll('.row.mg-keep, .row.mg-drop').forEach(el => el.classList.remove('mg-keep', 'mg-drop'));
    dupTip.classList.remove('open');
    const t = T(); if (!on || !t || !t.dupMarks || dupBar._i < 0) return;
    const i = dupBar._i, g = t.dupMarks.group.get(t.filteredData[i]), bar = dupBar.getBoundingClientRect();
    let top = Infinity, bottom = -Infinity;
    for (const rowEl of tbody.querySelectorAll('.row[data-idx]')) {
        const k = +rowEl.dataset.idx;
        if (t.dupMarks.group.get(t.filteredData[k]) !== g) continue;
        rowEl.classList.add(k === i ? 'mg-keep' : 'mg-drop');   // ✓ / ✕ in place of the row number (app.css)
        const r = rowEl.getBoundingClientRect(); top = Math.min(top, r.top); bottom = Math.max(bottom, r.bottom);
    }
    const b = dupBar.querySelector('.db-m'); if (!b) return;
    dupTip.textContent = b.dataset.tip;
    dupTip.classList.add('open');
    const vc = container.getBoundingClientRect(), h = dupTip.offsetHeight;
    dupTip.style.left = bar.left + 'px';
    dupTip.style.top = (bottom + 10 + h <= vc.bottom ? bottom + 10 : Math.max(vc.top, top - 10 - h)) + 'px';
}

/* ---- CELL ARROWS: a red cell's value copied to its sister cell -------
   On a cell whose value differs within its group (.dfc) — not in the first
   column, where the row's bar sits — ▲ copies the value into the same
   column of the row above, ▼ of the row below, when that row is of the
   same group. Hovering an arrow marks the cell it would write. */
const dupCell = document.createElement('div'); dupCell.id = 'dup-cell'; document.body.appendChild(dupCell);
const dupArrow = up => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><polyline points="${up ? '6 15 12 9 18 15' : '6 9 12 15 18 9'}"></polyline></svg>`;
function dupCellHide() { dupCell.classList.remove('open'); dupCell._at = null; dupCopyHint(false); }
function dupCellAt(cellEl) {
    const t = T(), rowEl = cellEl.parentElement, i = +rowEl.dataset.idx, c = +cellEl.dataset.c;
    if (dupCell._at && dupCell._at[0] === i && dupCell._at[1] === c && dupCell.classList.contains('open')) return;
    const g = t.dupMarks.group.get(t.filteredData[i]);
    const same = k => { const x = t.filteredData[k]; return !!x && t.dupMarks.group.get(x) === g; };
    const up = same(i - 1), down = same(i + 1);
    if (!up && !down) return dupCellHide();
    dupCell._at = [i, c];
    dupCell.innerHTML = (up ? `<button onclick="dupCopy(-1)" onmouseenter="dupCopyHint(true, -1)" onmouseleave="dupCopyHint(false)" title="Copy up">${dupArrow(true)}</button>` : '')
        + (down ? `<button onclick="dupCopy(1)" onmouseenter="dupCopyHint(true, 1)" onmouseleave="dupCopyHint(false)" title="Copy down">${dupArrow(false)}</button>` : '');
    const r = cellEl.getBoundingClientRect();
    dupCell.classList.add('open');
    dupCell.style.left = (r.right - dupCell.offsetWidth - 3) + 'px';
    dupCell.style.top = (r.top + (r.height - dupCell.offsetHeight) / 2) + 'px';
}
function dupCopyHint(on, dir) {
    tbody.querySelectorAll('.cell.cp-target').forEach(el => el.classList.remove('cp-target'));
    if (!on || !dupCell._at) return;
    const [i, c] = dupCell._at, el = tbody.querySelector(`.row[data-idx="${i + dir}"] .cell[data-c="${c}"]`);
    if (el) el.classList.add('cp-target');
}
function dupCopy(dir) {
    const t = T(); if (!t || !dupCell._at) return;
    const [i, c] = dupCell._at, from = t.filteredData[i], to = t.filteredData[i + dir];
    if (!from || !to) return;
    dupCellHide();
    const v = cellStr(from.data[c]), old = cellStr(to.data[c]);
    if (v === old) return;
    const ed = rowEdits(); ed.set(to, c, v);
    dupKeepView(t, () => {
        t.modificationsLog.push({ id: to.id, col: t.headers[c], old, new: v, what: `${t.headers[c]} copied to row ${to.id}`, undo: () => ed.undo() });
        updateSaveBtn(); applyFilters();          // the groups and their differences follow the new value
    });
    setStats(`${t.name} | "${v}" copied into row ${to.id}, column ${t.headers[c]} — not written yet, use Save.`);
}

tbody.addEventListener('mouseover', e => {
    const t = T(); if (!t || !t.dupMarks) return;
    const row = e.target.closest('.row[data-idx]');
    if (row) dupBarAt(row);
    const cell = e.target.closest('.cell.dfc'), L = cell && colLayout(t);
    if (cell && L && +cell.dataset.c !== L.vis[0]) dupCellAt(cell); else dupCellHide();
});
tbody.addEventListener('mouseleave', e => {
    if (!dupBar.contains(e.relatedTarget)) dupBarHide();
    if (!dupCell.contains(e.relatedTarget)) dupCellHide();
});
dupBar.addEventListener('mouseleave', e => { if (!tbody.contains(e.relatedTarget)) dupBarHide(); });
dupCell.addEventListener('mouseleave', e => { if (!tbody.contains(e.relatedTarget)) { dupCellHide(); dupBarHide(); } });
container.addEventListener('scroll', () => {
    if (dupBar.classList.contains('open') && container.scrollTop !== dupBar._top) dupBarHide();   // sideways: the bar stays by the row numbers
    if (dupCell.classList.contains('open')) dupCellHide();                                          // the arrows belong to a cell, which moves
}, { passive: true });

/* Edits from the bar keep the view where it is: a row operation re-filters, which scrolls to the top. */
function dupKeepView(t, fn) {
    const st = container.scrollTop, sl = container.scrollLeft;
    fn();
    container.scrollTop = st; container.scrollLeft = sl; t.scrollTop = st; render();
}
function dupBarDelete() {
    const t = T(), r = t && t.filteredData[dupBar._i]; if (!r) return;
    dupBarHide();
    dupKeepView(t, () => deleteRow(r.id));
}
/* The group of row i (its rows shown) into row i: its filled cells stay, each empty one takes the
   first value of the others in view order, the others go. One undo entry (row i's cells and the
   row array). */
function dupMerge(i) {
    const t = T(), keep = t && t.filteredData[i];
    if (!keep || !t.dupMarks) return;
    const g = t.dupMarks.group.get(keep); if (g == null) return;
    const others = dupShown(t, g).filter(k => k !== i).map(k => t.filteredData[k]);
    if (!others.length) return;
    dupBarHide();
    const id = keep.id, d = keep.data, n = Math.max(d.length, ...others.map(r => r.data.length)), out = new Array(n);
    let filled = 0, kept = 0;
    for (let c = 0; c < n; c++) {
        const own = cellStr(d[c]);
        if (own.trim()) { out[c] = own; if (others.some(r => { const v = cellStr(r.data[c]).trim(); return v && v !== own.trim(); })) kept++; continue; }
        const from = others.find(r => cellStr(r.data[c]).trim());
        out[c] = from ? cellStr(from.data[c]) : own;
        if (from) filled++;
    }
    const ed = rowEdits();
    if (filled) ed.put(keep, out);
    const gone = new Set(others);
    dupKeepView(t, () => commitRows(t, t.allData.filter(r => !gone.has(r)),
        { id: '-', col: '---', old: `${others.length + 1} rows`, new: 'Merged', what: `${others.length + 1} rows merged into row ${id}` }, () => ed.undo()));
    setStats(`${t.name} | ${others.length + 1} rows merged into row ${id}: ${fmt(filled)} empty cell${filled === 1 ? '' : 's'} filled, ${fmt(kept)} differing value${kept === 1 ? '' : 's'} kept from it — not written yet, use Save.`);
}
