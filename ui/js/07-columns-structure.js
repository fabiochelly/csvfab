/* ---------------------------------------------------------------
   RENAME & MOVE COLUMNS
   A title answers three gestures: click sorts, double-click renames in
   place, drag moves the column. Shift+click adds the column as a further
   sort key. The click sorts at once — it used to wait 240 ms in case a
   second click came — and the second click of a double-click undoes that
   sort (scroll position included) before renaming. Both are caught on
   click (e.detail): the sort re-renders the header, so the browser's own
   dblclick would find the title it started on gone.
----------------------------------------------------------------*/
let titleSort = null;                     // { t, entry, top, left }: the sort the first click just made
function titleClick(e, i) {
    const t = T(); if (!t) return;
    if (e.detail === 2) {
        if (titleSort && titleSort.t === t && t.modificationsLog[t.modificationsLog.length - 1] === titleSort.entry) {
            const { entry, top, left } = titleSort;
            undo();
            document.querySelectorAll('#toasts .toast').forEach(x => { if (x._entry === entry) x.remove(); });
            container.scrollTop = top; container.scrollLeft = left; t.scrollTop = top; render();
            setStats(t.name);
        }
        titleSort = null;
        const span = thead.querySelector(`th[data-col="${i}"] .col-name`);
        if (span) renameColumn(i, span);
        return;
    }
    if (e.detail > 2) return;
    const top = container.scrollTop, left = container.scrollLeft, n = t.modificationsLog.length;
    sortBy(i, 0, e.shiftKey);
    titleSort = t.modificationsLog.length > n ? { t, entry: t.modificationsLog[t.modificationsLog.length - 1], top, left } : null;
}

function renameColumn(i, span) {
    const t = T(); if (!t || !t.loaded) return;
    if (t.syntheticHeader) {
        uiAlert('This file has no header line: the column numbers are generated and never written.\n\nIf line 1 does hold titles, choose "Titles on first line" in the reading options.');
        return;
    }
    const input = document.createElement('input');
    input.className = 'col-rename'; input.value = t.headers[i]; input.spellcheck = false;
    span.replaceWith(input); input.focus(); input.select();
    let done = false;
    const finish = ok => {
        if (done) return; done = true;
        const v = input.value.trim(), old = t.headers[i];
        if (ok && v && v !== old) {
            t.headers[i] = v;
            t.modificationsLog.push({ id: '-', col: v, old, new: v, what: `column "${old}" renamed to "${v}"`, undo: t => { t.headers[i] = old; } });
            updateSaveBtn(); renderTabBar();
            setStats(`${t.name} | Column "${old}" renamed to "${v}" — not written yet, use Save.`);
        }
        renderHeader(); applyColStyles();
    };
    input.onkeydown = e => { e.stopPropagation(); if (e.key === 'Enter') finish(true); else if (e.key === 'Escape') finish(false); };
    input.onblur = () => finish(true);
    ['click', 'dblclick', 'mousedown'].forEach(ev => input.addEventListener(ev, e => e.stopPropagation()));
}

/* References that follow the columns, f(c) saying where column c now is
   (-1: gone): the sort keys ({col, dir} list), and t.colSrc — each column's
   index in the file as last read or saved (-1: a new one), which Review
   changes compares against. Called after t.headers has its new shape. */
function remapColRefs(t, f) {
    if (t.sort) {
        const keys = t.sort.map(k => ({ col: f(k.col), dir: k.dir })).filter(k => k.col >= 0);
        t.sort = keys.length ? keys : null;
    }
    if (t.colSrc) {
        const out = new Array(t.headers.length).fill(-1);
        t.colSrc.forEach((s, c) => { const n = f(c); if (n >= 0 && n < out.length) out[n] = s; });
        t.colSrc = out;
    }
}

/* Where column c lands when the column at `from` moves to `to`. */
function movedIndex(c, from, to) {
    if (c === from) return to;
    if (from < to && c > from && c <= to) return c - 1;
    if (from > to && c >= to && c < from) return c + 1;
    return c;
}
function moveColumn(from, to) {
    const t = T(); if (!t || from === to) return;
    const view = viewSnap(t);
    const shift = map => { const out = {}; Object.keys(map).forEach(k => out[movedIndex(+k, from, to)] = map[k]); return out; };
    const order = t.headers.map((_, i) => i); order.splice(to, 0, order.splice(from, 1)[0]);   // new column → old one
    const undoRows = remapRows(t, order);
    const [h] = t.headers.splice(from, 1); t.headers.splice(to, 0, h);
    t.hiddenCols = new Set([...t.hiddenCols].map(c => movedIndex(c, from, to)));
    t.colWidths = shift(t.colWidths); t.colFilters = shift(t.colFilters); t.valFilters = shift(t.valFilters); t.dataBars = shift(t.dataBars);
    remapColRefs(t, c => movedIndex(c, from, to));
    t.modificationsLog.push({ id: '-', col: h, old: 'moved', new: `${from} → ${to}`, what: `column "${h}" moved`, undo: t => {
        const [x] = t.headers.splice(to, 1); t.headers.splice(from, 0, x);
        undoRows(); viewRestore(t, view);
    } });
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
    setStats(`${t.name} | Column "${h}" moved — not written yet, use Save.`);
}

/* ---------------------------------------------------------------
   RESTRUCTURE (split, merge)
   Both rebuild every row's array — each row then owns one, the base
   record is no longer read — so undo puts the previous d back (null for
   a row that was still its record). mapOld(c) says where old column c
   now is (-1: gone), and remaps every index-keyed map; new columns get
   measured widths. rowFn(d, r, i) must not mutate d (it may be shared),
   nor count on being called in row order: i is the row's index.
----------------------------------------------------------------*/
function restructure(t, headers, rowFn, mapOld, what) {
    const view = viewSnap(t), prevHeaders = t.headers, prevD = t.allData.map(r => r.d), rows = t.allData.slice();
    visitRows(t, rows, (r, i) => { r.d = rowFn(r.data, r, i); });
    t.headers = headers;
    remapCols(t, mapOld);
    t.modificationsLog.push({ id: '-', col: '---', old: 'columns', new: what, what, undo: t => {
        t.headers = prevHeaders; rows.forEach((r, i) => r.d = prevD[i]); viewRestore(t, view);
    } });
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
    setStats(`${t.name} | ${what} — not written yet, use Save.`);
}
/* Every index-keyed map of the tab, through mapOld (-1: column gone). */
function remapCols(t, mapOld) {
    const remap = m => { const o = {}; Object.keys(m).forEach(k => { const c = mapOld(+k); if (c >= 0) o[c] = m[k]; }); return o; };
    t.hiddenCols = new Set([...t.hiddenCols].map(mapOld).filter(c => c >= 0));
    t.colWidths = remap(t.colWidths); t.colFilters = remap(t.colFilters); t.valFilters = remap(t.valFilters); t.dataBars = remap(t.dataBars);
    remapColRefs(t, mapOld);
}
const pad = (d, n) => { if (d.length >= n) return d; const c = d.slice(); while (c.length < n) c.push(''); return c; };

/* ---- SPLIT ------------------------------------------------------
   The separator is literal text (with \t for a tab) or a regex. Every
   cell is analysed first: how many parts it would give. The proposed
   column count is the largest one, unless a few outliers would blow it
   up (then the count that covers 99 % of the cells) — any cell with more
   parts keeps the rest, separators included, in the last column. */
const SPLIT_CHIPS = [[' ', 'space'], [',', ','], [';', ';'], ['-', '-'], ['/', '/'], ['|', '|'], ['@', '@'], ['\\t', '\\t'], ['\\s+', 'spaces', true]];
let splitState = { userN: false, names: [] };

function sepFinder(sep, isRegex) {
    if (isRegex) {
        let re; try { re = new RegExp(sep, 'g'); } catch (e) { return { error: e.message }; }
        return { find: v => { const out = []; re.lastIndex = 0; let m;
            while ((m = re.exec(v))) { if (!m[0]) { re.lastIndex++; continue; } out.push([m.index, m.index + m[0].length]); }
            return out; } };
    }
    const lit = sep.replace(/\\t/g, '\t');
    if (!lit) return null;
    return { find: v => { const out = []; let i = 0; while ((i = v.indexOf(lit, i)) !== -1) { out.push([i, i + lit.length]); i += lit.length; } return out; } };
}
/* Separators touching each other merge into one; with that option on,
   separators at either end of the value split nothing off. */
function sepAt(finder, v, collapse) {
    let occ = finder.find(v);
    if (!collapse || !occ.length) return occ;
    const out = [];
    for (const o of occ) { const last = out[out.length - 1]; if (last && o[0] === last[1]) last[1] = o[1]; else out.push(o.slice()); }
    return out.filter(o => o[0] > 0 && o[1] < v.length);
}
function cutAt(v, occ, n, trim) {
    const parts = []; let pos = 0;
    for (const [a, b] of occ.slice(0, n - 1)) { parts.push(v.slice(pos, a)); pos = b; }
    parts.push(v.slice(pos));
    while (parts.length < n) parts.push('');
    return trim ? parts.map(x => x.trim()) : parts;
}

function openSplit(col) {
    const t = T(); if (!t || !t.loaded) return;
    const sel = document.getElementById('split-col');
    sel.innerHTML = t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    if (col != null) sel.value = col;
    document.getElementById('split-chips').innerHTML = SPLIT_CHIPS.map(([v, l, re]) =>
        `<span class="dk" title="${re ? 'Regex: any run of spaces' : 'Use this separator'}" onclick="document.getElementById('split-sep').value = ${esc(JSON.stringify(v))}; document.getElementById('split-re').checked = ${!!re}; splitRefresh(true)">${esc(l)}</span>`).join('');
    splitState = { userN: false, names: [] };
    splitRefresh(true);
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-split').style.display = 'block';
    document.getElementById('split-sep').focus();
}

function splitRefresh(reanalyse) {
    const t = T(); if (!t) return;
    const col = +document.getElementById('split-col').value, h = t.headers[col];
    const sep = document.getElementById('split-sep').value, isRe = document.getElementById('split-re').checked;
    const collapse = document.getElementById('split-collapse').checked, trim = document.getElementById('split-trim').checked;
    const stats = document.getElementById('split-stats'), go = document.getElementById('split-go'), nIn = document.getElementById('split-n');
    const finder = sepFinder(sep, isRe);
    const clear = msg => { stats.innerHTML = msg; document.getElementById('split-pv').innerHTML = ''; document.getElementById('split-nnote').textContent = ''; go.disabled = true; };
    if (!finder) return clear('Type a separator, or pick one above.');
    if (finder.error) return clear(`<span class="warn">Invalid regex: ${esc(finder.error)}</span>`);

    if (reanalyse || !splitState.dist) {
        const dist = new Map(); let filled = 0;
        visitRows(t, t.allData, r => {
            const v = cellStr(cellOf(r, col)); if (!v.trim()) return;
            filled++;
            const k = sepAt(finder, v, collapse).length + 1;
            dist.set(k, (dist.get(k) || 0) + 1);
        });
        const ks = [...dist.keys()].sort((a, b) => a - b), max = ks.length ? ks[ks.length - 1] : 1;
        let cover = max, acc = 0;
        for (const k of ks) { acc += dist.get(k); if (acc >= filled * 0.99) { cover = k; break; } }
        splitState.dist = dist; splitState.filled = filled; splitState.max = max;
        if (!splitState.userN) nIn.value = Math.min(50, Math.max(2, max <= 10 ? max : cover));
    }
    const { dist, filled, max } = splitState;
    const n = Math.min(50, Math.max(2, parseInt(nIn.value, 10) || 2));
    const parts = [...dist.keys()].sort((a, b) => a - b).map(k => `${k} part${k > 1 ? 's' : ''} × <b>${fmt(dist.get(k))}</b>`);
    stats.innerHTML = filled
        ? `${fmt(filled)} non-empty cells: ${parts.join(' · ')}` + (max === 1 ? '<br><span class="warn">The separator appears in no cell.</span>' : '')
        : '<span class="warn">This column is empty.</span>';
    const over = [...dist].filter(([k]) => k > n).reduce((a, [, c]) => a + c, 0);
    document.getElementById('split-nnote').innerHTML = over ? `<span class="warn">${fmt(over)} cells have more parts: the rest stays joined in column ${n}.</span>` : '';

    /* Names: keep what the user typed, default to "<column> 1…n". */
    for (let i = 0; i < n; i++) if (splitState.names[i] == null || splitState.names[i].auto) splitState.names[i] = { v: `${h} ${i + 1}`, auto: true };
    const keep = document.getElementById('split-keep').checked;
    const sample = [];
    for (const r of t.allData) { const v = cellStr(r.data[col]); if (v.trim() && sepAt(finder, v, collapse).length) sample.push(v); if (sample.length >= 6) break; }
    document.getElementById('split-pv').innerHTML =
        '<tr>' + (keep ? `<th style="padding: 4px 8px;">${esc(h)}</th>` : '')
        + splitState.names.slice(0, n).map((x, i) => `<th><input class="bs-input" value="${esc(x.v)}" oninput="splitState.names[${i}] = { v: this.value, auto: false }"></th>`).join('') + '</tr>'
        + sample.map(v => '<tr>' + (keep ? `<td class="src">${esc(v)}</td>` : '')
            + cutAt(v, sepAt(finder, v, collapse), n, trim).map(x => x ? `<td title="${esc(x)}">${esc(x)}</td>` : '<td class="empty">empty</td>').join('') + '</tr>').join('');
    go.disabled = !(filled && max > 1);
}

function applySplit() {
    const t = T(); if (!t) return;
    const col = +document.getElementById('split-col').value;
    const finder = sepFinder(document.getElementById('split-sep').value, document.getElementById('split-re').checked);
    if (!finder || finder.error) return;
    const collapse = document.getElementById('split-collapse').checked, trim = document.getElementById('split-trim').checked;
    const keep = document.getElementById('split-keep').checked;
    const n = Math.min(50, Math.max(2, parseInt(document.getElementById('split-n').value, 10) || 2));
    const names = splitState.names.slice(0, n).map((x, i) => (x.v || '').trim() || `${t.headers[col]} ${i + 1}`);
    closeAllModals();
    const add = n - (keep ? 0 : 1);        // columns gained
    const headers = [...t.headers.slice(0, col + 1 - (keep ? 0 : 1)), ...names, ...t.headers.slice(col + 1)];
    restructure(t, headers, d => {
        const v = cellStr(d[col]), row = pad(d, col + 1);
        return [...row.slice(0, keep ? col + 1 : col), ...cutAt(v, sepAt(finder, v, collapse), n, trim), ...row.slice(col + 1)];
    }, c => c < col ? c : c === col ? (keep ? col : -1) : c + add, `column "${t.headers[col]}" split into ${n}`);
}

/* ---- MERGE ------------------------------------------------------ */
const MERGE_CHIPS = [[' ', 'space'], [', ', ', '], [' - ', ' - '], [';', ';'], ['', 'none']];
let mergeState = { order: [], userName: false };

function openMerge(col) {
    const t = T(); if (!t || !t.loaded) return;
    mergeState = { order: col != null ? [col] : [], userName: false };
    document.getElementById('merge-chips').innerHTML = MERGE_CHIPS.map(([v, l]) =>
        `<span class="dk" onclick="document.getElementById('merge-sep').value = ${esc(JSON.stringify(v))}; mergeRefresh()">${esc(l)}</span>`).join('');
    mergeRefresh();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-merge').style.display = 'block';
}
function mergeToggle(i, on) {
    mergeState.order = mergeState.order.filter(c => c !== i);
    if (on) mergeState.order.push(i);
    mergeRefresh();
}
function mergeValue(d, order, sep, skip) {
    const vals = order.map(c => cellStr(d[c]));
    return (skip ? vals.filter(v => v.trim() !== '') : vals).join(sep);
}
function mergeRefresh() {
    const t = T(); if (!t) return;
    const { order } = mergeState;
    document.getElementById('merge-cols').innerHTML = t.headers.map((h, i) => {
        const k = order.indexOf(i);
        return `<label class="col-label"><input type="checkbox" ${k >= 0 ? 'checked' : ''} onchange="mergeToggle(${i}, this.checked)"> ${esc(h)}${k >= 0 ? `<span class="ord">${k + 1}</span>` : ''}</label>`;
    }).join('');
    const sep = document.getElementById('merge-sep').value, skip = document.getElementById('merge-skip').checked;
    if (!mergeState.userName) document.getElementById('merge-name').value = order.map(c => t.headers[c]).join(' ');
    const sample = t.allData.slice(0, 6);
    document.getElementById('merge-pv').innerHTML = order.length < 2 ? '<tr><td class="empty">Tick at least two columns.</td></tr>'
        : '<tr>' + order.map(c => `<th style="padding: 4px 8px;">${esc(t.headers[c])}</th>`).join('') + `<th style="padding: 4px 8px; color: var(--prim);">${esc(document.getElementById('merge-name').value || 'merged')}</th></tr>`
        + sample.map(r => '<tr>' + order.map(c => `<td class="src">${esc(cellStr(r.data[c]))}</td>`).join('') + `<td>${esc(mergeValue(r.data, order, sep, skip))}</td></tr>`).join('');
    document.getElementById('merge-go').disabled = order.length < 2;
}
function applyMerge() {
    const t = T(); if (!t) return;
    const order = mergeState.order.slice(); if (order.length < 2) return;
    const sep = document.getElementById('merge-sep').value, skip = document.getElementById('merge-skip').checked;
    const keep = document.getElementById('merge-keep').checked;
    const name = document.getElementById('merge-name').value.trim() || order.map(c => t.headers[c]).join(' ');
    closeAllModals();
    /* The new column goes after the last merged one when they stay, in place of the first when they go. */
    const gone = new Set(keep ? [] : order), at = keep ? Math.max(...order) + 1 : Math.min(...order);
    const kept = t.headers.map((_, i) => i).filter(i => !gone.has(i));
    const pos = kept.filter(i => i < at).length;        // insertion point among the kept columns
    const newIdx = c => { if (gone.has(c)) return -1; const k = kept.indexOf(c); return k < pos ? k : k + 1; };
    const headers = kept.map(i => t.headers[i]); headers.splice(pos, 0, name);
    const width = t.headers.length;
    restructure(t, headers, d => {
        const row = pad(d, width), out = kept.map(i => row[i]);
        out.splice(pos, 0, mergeValue(row, order, sep, skip));
        return d.length > width ? out.concat(d.slice(width)) : out;   // extra fields of an irregular row stay at the end
    }, newIdx, `${order.length} columns merged into "${name}"`);
}

/* ---- MOVE ROWS --------------------------------------------------
   Drag a row by its number. Near the top or bottom edge the grid
   scrolls, so a row can travel beyond the rows on screen. The move is
   made in t.allData, next to the target row, so it holds under a filter;
   it breaks any sort order, which is cleared. */
let dragRow = null;
tbody.addEventListener('dragstart', e => {
    const td = e.target.closest && e.target.closest('td.col-idx'); if (!td) return;
    const t = T(), tr = td.parentElement, r = t && t.filteredData[+tr.dataset.idx];
    if (!r) return;
    dragRow = r;
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/x-csv-row', String(r.id));
    tr.classList.add('row-dragging');
});
const clearRowMarks = () => tbody.querySelectorAll('.drop-above, .drop-below, .row-dragging').forEach(tr => tr.classList.remove('drop-above', 'drop-below', 'row-dragging'));
tbody.addEventListener('dragend', () => { dragRow = null; clearRowMarks(); });
tbody.addEventListener('dragover', e => {
    if (!dragRow) return;
    e.preventDefault(); e.dataTransfer.dropEffect = 'move';
    const box = container.getBoundingClientRect(), edge = 48;
    if (e.clientY < box.top + thead.offsetHeight + edge) container.scrollTop -= 24;   // below the sticky header
    else if (e.clientY > box.bottom - edge) container.scrollTop += 24;
    const tr = e.target.closest && e.target.closest('tr[data-idx]');
    tbody.querySelectorAll('.drop-above, .drop-below').forEach(x => x.classList.remove('drop-above', 'drop-below'));
    if (!tr) return;
    const r = tr.getBoundingClientRect();
    tr.classList.add(e.clientY < r.top + r.height / 2 ? 'drop-above' : 'drop-below');
});
tbody.addEventListener('drop', e => {
    if (!dragRow) return;
    e.preventDefault(); e.stopPropagation();
    const t = T(), tr = e.target.closest && e.target.closest('tr[data-idx]'), src = dragRow;
    const target = tr && t.filteredData[+tr.dataset.idx];
    const below = tr && e.clientY >= tr.getBoundingClientRect().top + tr.getBoundingClientRect().height / 2;
    dragRow = null; clearRowMarks();
    if (!target || target === src) return;
    const from = t.allData.indexOf(src);
    t.allData.splice(from, 1);
    const to = t.allData.indexOf(target) + (below ? 1 : 0);
    t.allData.splice(to, 0, src);
    if (to === from) { return; }
    const prevSort = t.sort; t.sort = null;
    const keepTop = container.scrollTop;
    t.allData.forEach((r, i) => r.id = i + 1);
    t.modificationsLog.push({ id: '-', col: '---', old: 'row moved', new: `${from + 1} → ${to + 1}`, what: `row ${from + 1} moved to ${to + 1}`, undo: t => {
        t.allData.splice(t.allData.indexOf(src), 1); t.allData.splice(from, 0, src); t.sort = prevSort;
    } });
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
    container.scrollTop = keepTop; render();              // stay where the row was dropped
    setStats(`${t.name} | Row ${from + 1} moved to ${to + 1} — not written yet, use Save.`);
});

let dragCol = null;
function colDragStart(e, i) {
    dragCol = i;
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/x-csv-column', String(i));
    e.currentTarget.closest('th').classList.add('dragging');
}
function colDragEnd() {
    dragCol = null;
    document.querySelectorAll('th.dragging, th.drop-before, th.drop-after').forEach(th => th.classList.remove('dragging', 'drop-before', 'drop-after'));
}
/* Dropping on the left half of a title inserts before it, on the right half after it. */
function dropSide(e, th) { const r = th.getBoundingClientRect(); return e.clientX < r.left + r.width / 2 ? 'before' : 'after'; }
function colDragOver(e) {
    if (dragCol === null) return;
    e.preventDefault(); e.dataTransfer.dropEffect = 'move';
    const th = e.currentTarget, side = dropSide(e, th);
    th.classList.toggle('drop-before', side === 'before');
    th.classList.toggle('drop-after', side === 'after');
}
function colDrop(e) {
    if (dragCol === null) return;
    e.preventDefault(); e.stopPropagation();
    const th = e.currentTarget, target = +th.dataset.col, from = dragCol;
    let to = dropSide(e, th) === 'before' ? target : target + 1;
    if (to > from) to--;                  // the column leaves its slot before landing
    colDragEnd();
    moveColumn(from, to);
}

async function deleteColumns(t, cols) {
    const names = cols.map(c => `"${t.headers[c]}"`);
    const list = names.length > 6 ? names.slice(0, 5).join(', ') + ` and ${names.length - 5} more` : names.join(', ');
    if (!await uiConfirm(`Delete the ${cols.length} selected columns?\n\n${list}`, { ok: `Delete ${cols.length} columns`, danger: true })) return;
    const gone = new Set(cols);
    const before = c => { let n = 0; for (const g of cols) if (g < c) n++; return n; };
    sel = null;
    const view = viewSnap(t), prevHeaders = t.headers, what = `${cols.length} columns deleted`;
    const undoRows = remapRows(t, t.headers.map((_, i) => i).filter(i => !gone.has(i)));   // an irregular row's extra fields stay
    t.headers = t.headers.filter((_, i) => !gone.has(i));
    remapCols(t, c => gone.has(c) ? -1 : c - before(c));
    t.modificationsLog.push({ id: '-', col: '---', old: 'columns', new: what, what, undo: t => { t.headers = prevHeaders; undoRows(); viewRestore(t, view); } });
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
    setStats(`${t.name} | ${what} — not written yet, use Save.`);
}

async function addColumn(idx) {
    const t = T(); if (!t) return;
    const colName = await uiPrompt('Name of the new column:', 'New_Column');
    if (!colName) return;
    const view = viewSnap(t), order = t.headers.map((_, i) => i);
    order.splice(idx + 1, 0, -1);                          // new column → old one, -1: the new, empty one
    const undoRows = remapRows(t, order);
    t.headers.splice(idx + 1, 0, colName);

    const newHidden = new Set();
    t.hiddenCols.forEach(c => { if (c <= idx) newHidden.add(c); else newHidden.add(c + 1); });
    t.hiddenCols = newHidden;
    t.colWidths = shiftKeys(t.colWidths, idx, +1);
    t.colFilters = shiftKeys(t.colFilters, idx, +1);
    t.valFilters = shiftKeys(t.valFilters, idx, +1);
    t.dataBars = shiftKeys(t.dataBars, idx, +1);

    remapColRefs(t, c => c > idx ? c + 1 : c);
    t.modificationsLog.push({ id: '-', col: colName, old: '---', new: 'Column added', what: `column "${colName}" added`, undo: t => {
        t.headers.splice(idx + 1, 1);
        undoRows(); viewRestore(t, view);
    } });
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
}

async function deleteColumn(idx) {
    const t = T(); if (!t) return;
    /* A selection spanning several columns, this one among them: the − removes them all. */
    const rg = selRange(t);
    if (rg && rg.c1 > rg.c0 && idx >= rg.c0 && idx <= rg.c1) {
        const cols = visibleCols(t).filter(c => c >= rg.c0 && c <= rg.c1);
        if (cols.length > 1) return deleteColumns(t, cols);
    }
    if (!await uiConfirm(`Delete column "${t.headers[idx]}" permanently?`, { ok: 'Delete column', danger: true })) return;
    const colName = t.headers[idx];
    const view = viewSnap(t), undoRows = remapRows(t, t.headers.map((_, i) => i).filter(i => i !== idx));
    t.headers.splice(idx, 1);

    const newHidden = new Set();
    t.hiddenCols.forEach(c => { if (c < idx) newHidden.add(c); else if (c > idx) newHidden.add(c - 1); });
    t.hiddenCols = newHidden;
    t.colWidths = shiftKeys(t.colWidths, idx, -1);
    t.colFilters = shiftKeys(t.colFilters, idx, -1);
    t.valFilters = shiftKeys(t.valFilters, idx, -1);
    t.dataBars = shiftKeys(t.dataBars, idx, -1);

    remapColRefs(t, c => c === idx ? -1 : c > idx ? c - 1 : c);
    t.modificationsLog.push({ id: '-', col: colName, old: 'Column deleted', new: '---', what: `column "${colName}" deleted`, undo: t => {
        t.headers.splice(idx, 0, colName);
        undoRows(); viewRestore(t, view);
    } });
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar();
}

/* Re-index a {colIdx: value} map after a column insert (+1) / delete (-1) at idx */
function shiftKeys(map, idx, dir) {
    const out = {};
    Object.keys(map).forEach(k => {
        const c = parseInt(k, 10);
        if (dir > 0) out[c <= idx ? c : c + 1] = map[k];
        else if (c < idx) out[c] = map[k];
        else if (c > idx) out[c - 1] = map[k];
    });
    return out;
}

function handleColCheck(e, idx) {
    const t = T(); if (!t) return;
    const checkboxes = document.querySelectorAll('#cols-list input[type="checkbox"]');
    let isChecked = e.target.checked;
    if (e.shiftKey && lastCheckedCol !== null) {
        let start = Math.min(lastCheckedCol, idx); let end = Math.max(lastCheckedCol, idx);
        for (let i = start; i <= end; i++) {
            checkboxes[i].checked = isChecked;
            if (isChecked) t.hiddenCols.delete(i); else t.hiddenCols.add(i);
        }
    } else {
        if (isChecked) t.hiddenCols.delete(idx); else t.hiddenCols.add(idx);
    }
    lastCheckedCol = idx; applyColStyles(); render(); updateCount(t);   // render: the spacers' colspans count the visible columns
}

/* Single style tag holding hidden columns + column widths of the ACTIVE tab */
function applyColStyles() {
    const t = T();
    let css = '';
    if (t) {
        /* table-layout: fixed only takes effect with an explicit table width;
           under width: max-content Chromium falls back to the auto algorithm,
           where a cell's width is a mere minimum and a long value in the
           rendered slice still widens its column. So once every visible
           column has a pinned width, the table gets their exact sum. */
        let sum = idxColW, all = idxColW > 0;
        t.headers.forEach((_, c) => { if (t.hiddenCols.has(c)) return; if (t.colWidths[c] == null) all = false; else sum += t.colWidths[c]; });
        if (all) css += `#mainTable { width: ${sum}px; }\n`;
        /* Header cells by position (the header always has every column), body
           cells by data-c: a row drawn with only the columns in view has fewer. */
        t.hiddenCols.forEach(c => { css += `#mainTable td[data-c="${c}"], #mainTable thead th:nth-child(${c + 2}) { display: none !important; }\n`; });
        Object.keys(t.colWidths).forEach(k => {
            const c = parseInt(k, 10), w = t.colWidths[k];
            css += `#mainTable td[data-c="${c}"], #mainTable thead th:nth-child(${c + 2}) { box-sizing: border-box; width: ${w}px !important; min-width: ${w}px !important; max-width: ${w}px !important; overflow: hidden; }\n`;
        });
    }
    let styleTag = document.getElementById('tab-cols-style');
    if (!styleTag) { styleTag = document.createElement('style'); styleTag.id = 'tab-cols-style'; document.head.appendChild(styleTag); }
    styleTag.innerHTML = css;
}

function openColManager() {
    const t = T(); if (!t) return;
    lastCheckedCol = null;
    const list = document.getElementById('cols-list');
    list.innerHTML = t.headers.map((h, i) => `<label class="col-label"><input type="checkbox" onclick="handleColCheck(event, ${i})" ${t.hiddenCols.has(i) ? '' : 'checked'}> ${esc(h)}</label>`).join('');
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-cols').style.display = 'block';
}
