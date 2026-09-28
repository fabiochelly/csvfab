/* ---------------------------------------------------------------
   RANGE SELECTION & CLIPBOARD
   Click a cell, drag or Shift+click to extend, click a row number for
   the whole row, Ctrl+A for everything shown. Arrows move (Shift
   extends), Enter/F2 edit, Delete clears. Ctrl+C copies the range as
   tab-separated text — what Excel and Sheets put on the clipboard and
   read back — skipping hidden columns (and hidden rows: the range is
   over the rows shown). Ctrl+V pastes such text at the active cell, one
   value into every selected cell, or a block from there (new rows are
   added at the end when no filter hides any). One undo entry per paste.
----------------------------------------------------------------*/
/* Data bars: a numeric column's values drawn as bars in their cells, scaled on its min–max. */
function toggleDataBars(col) {
    const t = T(); if (!t) return;
    if (t.dataBars[col]) { delete t.dataBars[col]; render(); return; }
    let min = Infinity, max = -Infinity;
    for (const r of t.allData) { const v = cellStr(r.data[col]).trim(); if (v && isNumericLike(v)) { const x = numKey(v); if (!isNaN(x)) { if (x < min) min = x; if (x > max) max = x; } } }
    if (min === Infinity) return;
    t.dataBars[col] = { min: Math.min(0, min), max: Math.max(0, max) };
    render();
}
function barStyle(t, c, v) {
    const b = t.dataBars[c]; if (!b) return '';
    v = cellStr(v).trim(); if (!v || !isNumericLike(v)) return '';
    const x = numKey(v), span = (b.max - b.min) || 1;
    if (isNaN(x)) return '';
    const z = (0 - b.min) / span * 100, p = (x - b.min) / span * 100, a = Math.min(z, p), w = Math.abs(p - z);
    const rgb = x < 0 ? 'var(--danger-rgb)' : 'var(--accent-rgb)';
    return ` style="background-image: linear-gradient(90deg, transparent ${a.toFixed(1)}%, rgba(${rgb}, .28) ${a.toFixed(1)}%, rgba(${rgb}, .28) ${(a + w).toFixed(1)}%, transparent ${(a + w).toFixed(1)}%)"`;
}
function cellCls(i, c, rg, fp, row) {
    const k = [];
    if (flashCells.size) { const f = flashCells.get(row); if (f && f.has(c)) k.push('flash'); }
    if (rg && i >= rg.r0 && i <= rg.r1 && c >= rg.c0 && c <= rg.c1) {
        k.push('sel');
        if (i === sel.fr && c === sel.fc) k.push('cur');
        if (!fillDrag && i === rg.r1 && c === rg.c1) k.push('fh');
    }
    if (fp && i >= fp.r0 && i <= fp.r1 && c >= fp.c0 && c <= fp.c1) k.push('fillp');
    return k.length ? ` class="${k.join(' ')}"` : '';
}
function selRange(t) {
    if (!sel || !t || sel.tab !== t.id) return null;
    return { r0: Math.min(sel.ar, sel.fr), r1: Math.max(sel.ar, sel.fr), c0: Math.min(sel.ac, sel.fc), c1: Math.max(sel.ac, sel.fc) };
}
const visibleCols = t => t.headers.map((_, i) => i).filter(i => !t.hiddenCols.has(i));
function setSel(t, ar, ac, fr, fc) {
    sel = { tab: t.id, ar, ac, fr, fc };
    render(); selStats(t);
}
/* Status bar: size of the range, and sum / average when it holds numbers. */
function selStats(t) {
    const rg = selRange(t); if (!rg) return updateStats();
    const cols = visibleCols(t).filter(c => c >= rg.c0 && c <= rg.c1), rows = rg.r1 - rg.r0 + 1;
    let msg = `${t.name} | ${fmt(rows)} × ${fmt(cols.length)} selected`;
    if (rows * cols.length <= 1e6) {
        let n = 0, sum = 0;
        for (let r = rg.r0; r <= rg.r1; r++) for (const c of cols) {
            const v = cellStr(t.filteredData[r].data[c]).trim();
            if (v && isNumericLike(v)) { const x = numKey(v); if (!isNaN(x)) { n++; sum += x; } }
        }
        const f = x => x.toLocaleString('fr-FR', { maximumFractionDigits: 2 });
        if (n) msg += ` · Sum ${f(sum)} · Avg ${f(sum / n)} · ${fmt(n)} numbers`;
    }
    setStats(msg);
}
/* Keep the active cell in view, below the chrome and the sticky header. */
function revealCell(r, c) {
    const top = thead.offsetHeight, y = top + r * ROW_H;   // row r's top in content coordinates, below the sticky header
    if (y < container.scrollTop + top) container.scrollTop = y - top;
    else if (y + ROW_H > container.scrollTop + container.clientHeight) container.scrollTop = y + ROW_H - container.clientHeight;
    render();
    const td = tbody.querySelector(`tr[data-idx="${r}"] td[data-c="${c}"]`);
    if (td) {
        const b = td.getBoundingClientRect(), cb = container.getBoundingClientRect(), left = cb.left + 70;
        if (b.left < left) container.scrollLeft -= left - b.left;
        else if (b.right > cb.right - 16) container.scrollLeft += b.right - cb.right + 16;
    }
}

tbody.addEventListener('mousedown', e => {
    if (e.button !== 0) return;
    const t = T(); if (!t || !t.loaded) return;
    const td = e.target.closest('td'), tr = td && td.parentElement;
    if (!td || !tr.dataset.idx || td.classList.contains('editing') || e.target.closest('.row-btn')) return;
    if (td.classList.contains('fh')) {                    // the fill handle: its 9 × 9 px corner
        const b = td.getBoundingClientRect();
        if (e.clientX >= b.right - 9 && e.clientY >= b.bottom - 9) {
            e.preventDefault();
            fillDrag = { rg: selRange(t), r: +tr.dataset.idx, c: +td.dataset.c };
            return;
        }
    }
    const r = +tr.dataset.idx, last = t.headers.length - 1;
    if (td.classList.contains('col-idx')) {                // row number: whole rows (dragging it moves the row instead)
        if (e.shiftKey && sel && sel.tab === t.id) setSel(t, sel.ar, 0, r, last);
        else setSel(t, r, 0, r, last);
        return;
    }
    const c = +td.dataset.c;
    if (e.shiftKey && sel && sel.tab === t.id) setSel(t, sel.ar, sel.ac, r, c);
    else setSel(t, r, c, r, c);
    selDragging = true;
});
document.addEventListener('mousemove', e => {
    if (fillDrag) {
        if (!(e.buttons & 1)) { fillDrag = null; render(); return; }
        const box = container.getBoundingClientRect();
        if (e.clientY > box.bottom - 24) container.scrollTop += 20;
        else if (e.clientY < box.top + thead.offsetHeight + 12) container.scrollTop -= 20;
        const cell = document.elementFromPoint(e.clientX, e.clientY), td = cell && cell.closest && cell.closest('#tbody td[data-c]');
        if (td) { const r = +td.parentElement.dataset.idx, c = +td.dataset.c; if (r !== fillDrag.r || c !== fillDrag.c) { fillDrag.r = r; fillDrag.c = c; render(); } }
        return;
    }
    if (!selDragging || !(e.buttons & 1)) { selDragging = false; return; }
    const t = T(), box = container.getBoundingClientRect();
    if (e.clientY > box.bottom - 24) container.scrollTop += 20;
    else if (e.clientY < box.top + thead.offsetHeight + 12) container.scrollTop -= 20;
    const td = document.elementFromPoint(e.clientX, e.clientY);
    const cell = td && td.closest && td.closest('#tbody td[data-c]');
    if (!cell) return;
    const r = +cell.parentElement.dataset.idx, c = +cell.dataset.c;
    if (r !== sel.fr || c !== sel.fc) setSel(t, sel.ar, sel.ac, r, c);
});
document.addEventListener('mouseup', e => {
    selDragging = false;
    if (!fillDrag) return;
    const fr = fillRect(), rg = fillDrag.rg; fillDrag = null;
    if (fr) doFill(T(), rg, fr, e.ctrlKey || e.metaKey); else render();
});

/* Keys act on the grid only when nothing else has the focus. */
function gridKeysAllowed(e) {
    if (document.getElementById('cmdk').classList.contains('open')) return false;
    if (e.target.closest && e.target.closest('input, textarea, select, [contenteditable]')) return false;
    if (document.getElementById('dlg') || colPanel || document.querySelector('.dd-menu.open')) return false;
    return ![...document.querySelectorAll('.modal-box')].some(m => m.style.display === 'block');
}
document.addEventListener('keydown', e => {
    const t = T(); if (!t || !t.loaded || !gridKeysAllowed(e)) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.shiftKey && e.key.toLowerCase() === 'a') {
        if (!t.filteredData.length) return;
        e.preventDefault(); setSel(t, 0, 0, t.filteredData.length - 1, t.headers.length - 1); return;
    }
    if (!sel || sel.tab !== t.id) return;
    const vis = visibleCols(t), rows = t.filteredData.length;
    const step = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1], PageUp: [-Math.floor(container.clientHeight / ROW_H), 0], PageDown: [Math.floor(container.clientHeight / ROW_H), 0] }[e.key];
    if (step && !e.altKey) {
        e.preventDefault();
        let r = Math.min(rows - 1, Math.max(0, sel.fr + step[0]));
        let k = vis.indexOf(sel.fc); if (k < 0) k = 0;
        k = Math.min(vis.length - 1, Math.max(0, k + step[1]));
        if (ctrl && step[0]) r = step[0] < 0 ? 0 : rows - 1;
        if (ctrl && step[1]) k = step[1] < 0 ? 0 : vis.length - 1;
        const c = vis[k];
        if (e.shiftKey) sel = { ...sel, fr: r, fc: c }; else sel = { tab: t.id, ar: r, ac: c, fr: r, fc: c };
        revealCell(r, c); selStats(t);
        return;
    }
    if ((e.key === 'Enter' || e.key === 'F2') && !ctrl) {
        e.preventDefault(); revealCell(sel.fr, sel.fc);
        const td = tbody.querySelector(`tr[data-idx="${sel.fr}"] td[data-c="${sel.fc}"]`);
        if (td) startEdit(td, null);
        return;
    }
    if (e.key === 'Delete' || e.key === 'Backspace') { e.preventDefault(); writeCells(t, [['']], true, 'cleared'); return; }
    if (ctrl && !e.shiftKey && e.key.toLowerCase() === 'd') { e.preventDefault(); fillDown(); return; }
    /* A printable key starts editing the active cell with that key, as in a spreadsheet. */
    if (e.key.length === 1 && !ctrl && !e.altKey) {
        e.preventDefault(); revealCell(sel.fr, sel.fc);
        const td = tbody.querySelector(`tr[data-idx="${sel.fr}"] td[data-c="${sel.fc}"]`);
        if (td) startEdit(td, e.key);
    }
});

function tsvQuote(v) { v = cellStr(v); return /[\t\n\r"]/.test(v) ? '"' + v.replace(/"/g, '""') + '"' : v; }
/* Excel's clipboard text: tabs between cells, CRLF between rows, a cell
   holding a tab, a line break or a quote is quoted with "" doubling. */
function parseTSV(text) {
    const rows = []; let row = [], cell = '', i = 0, q = false;
    while (i < text.length) {
        const ch = text[i];
        if (q) {
            if (ch === '"') { if (text[i + 1] === '"') { cell += '"'; i += 2; continue; } q = false; i++; continue; }
            cell += ch; i++; continue;
        }
        if (ch === '"' && cell === '') { q = true; i++; continue; }
        if (ch === '\t') { row.push(cell); cell = ''; i++; continue; }
        if (ch === '\r' || ch === '\n') { row.push(cell); rows.push(row); row = []; cell = ''; i += (ch === '\r' && text[i + 1] === '\n') ? 2 : 1; continue; }
        cell += ch; i++;
    }
    if (cell !== '' || row.length) { row.push(cell); rows.push(row); }
    return rows;
}

document.addEventListener('copy', e => {
    const t = T(); if (!t || !t.loaded || !gridKeysAllowed(e)) return;
    const rg = selRange(t); if (!rg) return;
    const cols = visibleCols(t).filter(c => c >= rg.c0 && c <= rg.c1), out = [];
    for (let r = rg.r0; r <= rg.r1; r++) out.push(cols.map(c => tsvQuote(t.filteredData[r].data[c])).join('\t'));
    e.clipboardData.setData('text/plain', out.join('\r\n') + (out.length > 1 ? '\r\n' : ''));
    e.preventDefault();
    doneMsg(`${t.name} | ${fmt(out.length)} × ${fmt(cols.length)} cells copied.`);
});
document.addEventListener('paste', e => {
    const t = T(); if (!t || !t.loaded || !gridKeysAllowed(e) || !sel || sel.tab !== t.id) return;
    const text = e.clipboardData.getData('text/plain'); if (!text) return;
    e.preventDefault();
    writeCells(t, parseTSV(text), false, 'pasted');
});

/* Writes a block of values at the selection: one value fills the whole
   range, a block starts at its top-left corner (and grows the file at the
   end when nothing is filtered out). Hidden columns are skipped. */
function writeCells(t, grid, fillOnly, verb) {
    const rg = selRange(t); if (!rg || !grid.length) return;
    const vis = visibleCols(t), before = [], added = [];
    const fill = fillOnly || (grid.length === 1 && grid[0].length === 1);
    let dropRows = 0, dropCols = 0;
    const put = (row, c, v) => { if (cellStr(row.data[c]) === v) return; before.push([row, c, row.data[c]]); while (row.data.length <= c) row.data.push(''); row.data[c] = v; };
    let r1 = rg.r1, c1 = rg.c1;
    if (fill) {
        for (let r = rg.r0; r <= rg.r1; r++) for (const c of vis) if (c >= rg.c0 && c <= rg.c1) put(t.filteredData[r], c, grid[0][0]);
    } else {
        const k0 = Math.max(0, vis.indexOf(rg.c0) >= 0 ? vis.indexOf(rg.c0) : vis.findIndex(c => c > rg.c0));
        const canGrow = !hasFilter(t);
        grid.forEach((line, gi) => {
            let row = t.filteredData[rg.r0 + gi];
            if (!row) {
                if (!canGrow) { dropRows++; return; }
                row = { id: 0, data: t.headers.map(() => '') }; t.allData.push(row); added.push(row);
            }
            line.forEach((v, gj) => { const c = vis[k0 + gj]; if (c == null) { dropCols = Math.max(dropCols, gj + 1 - (vis.length - k0)); return; } put(row, c, v); });
        });
        r1 = Math.min(rg.r0 + grid.length - 1, t.allData.length - 1);
        c1 = vis[Math.min(vis.length - 1, k0 + Math.max(...grid.map(l => l.length)) - 1)];
    }
    if (!before.length && !added.length) { setStats(`${t.name} | Nothing changed.`); return; }
    flash(before.map(([row, c]) => [row, c]).concat(added.flatMap(row => row.data.map((_, c) => [row, c]))));
    t.allData.forEach((r, i) => r.id = i + 1); t.rowCount = t.allData.length;
    const n = before.length;
    t.modificationsLog.push({ id: '-', col: '---', old: verb, new: `${n} cells`, what: `${fmt(n)} cells ${verb}${added.length ? `, ${fmt(added.length)} rows added` : ''}`, undo: t => {
        for (const row of added) t.allData.splice(t.allData.indexOf(row), 1);
        for (let k = before.length - 1; k >= 0; k--) { const [row, c, v] = before[k]; row.data[c] = v; }
    } });
    const keepTop = container.scrollTop;
    keepSel = true; applyFilters(); keepSel = false;
    container.scrollTop = keepTop;
    sel = { tab: t.id, ar: rg.r0, ac: rg.c0, fr: Math.min(fill ? rg.r1 : r1, t.filteredData.length - 1), fc: fill ? rg.c1 : c1 };
    updateSaveBtn(); renderTabBar(); render();
    setStats(`${t.name} | ${fmt(n)} cells ${verb}${added.length ? `, ${fmt(added.length)} rows added` : ''}`
        + (dropRows ? ` · ${fmt(dropRows)} rows left out (a filter is active)` : '') + (dropCols ? ` · ${fmt(dropCols)} columns past the last one left out` : '')
        + ' — not written yet, use Save.');
}
