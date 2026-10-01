/* ---------------------------------------------------------------
   ROW CARD, RAW LINE, GO TO ROW
   The selected row as a form (Ctrl+I): one field per column, editable,
   docked on the right of the grid — reading one record across 85
   columns by scrolling sideways is what this replaces. The card follows
   the selection; a field committed writes the cell like an inline edit.
   Its "Raw" view shows the record's bytes as text, delimiters, quotes
   and invisible characters marked: what an irregular row or a quote
   error really looks like in the file.
   Ctrl+G jumps to a row by its number in the file.
----------------------------------------------------------------*/
let rowCard = { t: null, row: null, vi: -1, key: '', raw: false, diag: false, q: '' };

function rowCardIsOpen() { return document.getElementById('row-card').classList.contains('open'); }
function toggleRowCard(force) {
    const card = document.getElementById('row-card');
    const open = force != null ? force : !card.classList.contains('open');
    card.classList.toggle('open', open);
    resizeContainer();                        // the grid makes room for the card
    if (open) rowCardSync(true); else render();
}
function rowCardRaw(on) {
    rowCard.raw = on != null ? on : !rowCard.raw;
    document.getElementById('row-card').classList.toggle('raw', rowCard.raw);
    document.getElementById('rc-rawbtn').classList.toggle('on', rowCard.raw);
    rowCardSync(true);
}
/* "Why?": the row's diagnosis (37-…) above its fields. */
function rowCardDiag(on) {
    rowCard.diag = on != null ? on : !rowCard.diag;
    document.getElementById('row-card').classList.toggle('diag', rowCard.diag);
    document.getElementById('rc-diagbtn').classList.toggle('on', rowCard.diag);
    rowCardSync(true);
}
/* From the row menu: select that row, open the card (on its raw view or not). */
function rowCardFor(id, raw) {
    const t = T(); if (!t) return;
    const vi = t.filteredData.findIndex(r => r.id === id); if (vi < 0) return;
    const c = sel && sel.tab === t.id ? sel.fc : (visibleCols(t)[0] || 0);
    setSel(t, vi, c, vi, c);
    if (raw != null && raw !== rowCard.raw) rowCardRaw(raw);
    toggleRowCard(true);
}

/* Called after every selection change and updateStats(): redraws only when
   the row, the tab or the data changed, and never under a field being typed in. */
function rowCardSync(force) {
    const card = document.getElementById('row-card');
    if (!card.classList.contains('open')) return;
    const typing = card.contains(document.activeElement) && document.activeElement.tagName === 'TEXTAREA';
    if (typing && !force) { rowCard.pending = true; return; }   // redrawn when the focus leaves the field
    rowCard.pending = false;
    const t = T(), vi = t && sel && sel.tab === t.id ? sel.fr : -1;
    const row = t && t.loaded && vi >= 0 ? t.filteredData[vi] : null;
    const last = t && t.modificationsLog[t.modificationsLog.length - 1];
    const key = t ? [t.id, row ? row.id : -1, vi, t.modificationsLog.length, last && last.what, t.headers.join('\u0001'), [...t.hiddenCols].join(','), sel && sel.fc].join('|') : '';
    if (!force && key === rowCard.key) return;
    rowCard.t = t; rowCard.row = row; rowCard.vi = vi; rowCard.key = key;
    renderRowCard(t, row, vi);
}

function renderRowCard(t, row, vi) {
    const title = document.getElementById('rc-title'), sub = document.getElementById('rc-sub');
    const fields = document.getElementById('rc-fields'), raw = document.getElementById('rc-raw');
    document.getElementById('rc-prev').disabled = !row || vi <= 0;
    document.getElementById('rc-next').disabled = !row || !t || vi >= t.filteredData.length - 1;
    if (!row) {
        title.textContent = t && t.loaded ? 'No row selected' : 'Row card';
        sub.textContent = t && t.loaded ? 'Click a cell or a row number in the grid.' : '';
        fields.innerHTML = ''; raw.innerHTML = ''; document.getElementById('rc-diag').innerHTML = ''; return;
    }
    const n = t.headers.length, irr = row.len !== n;
    title.textContent = `Row ${fmt(row.id)}`;
    sub.innerHTML = `${fmt(vi + 1)} of ${fmt(t.filteredData.length)} shown`
        + (row.src != null ? ` · line ${fmt(row.src + 1)} in the file` : ' · <span class="warn">new row, not in the file yet</span>')
        + (irr ? ` · <span class="warn">${fmt(row.len)} fields for ${fmt(n)} columns</span>` : '')
        + (row.d ? ' · edited' : '');
    /* Problems found in the line's bytes: counted here, explained by "Why?". */
    const probs = t.base && row.b >= 0 ? diagnoseRow(t, row).findings.filter(f => f.severity !== 'info').length : 0;
    if (probs && !rowCard.diag) sub.insertAdjacentHTML('beforeend', ` · <a class="warn rc-why" onclick="rowCardDiag(true)">${fmt(probs)} problem${probs > 1 ? 's' : ''} — why?</a>`);
    document.getElementById('rc-diag').innerHTML = rowCard.diag ? diagnosisHtml(t, row) : '';
    raw.innerHTML = rowCard.raw ? rawLineHtml(t, row) : '';
    rowCardFields();
}

/* The fields, filtered by the search box (name or value). */
function rowCardFields() {
    const { t, row } = rowCard, box = document.getElementById('rc-fields');
    if (!t || !row) { box.innerHTML = ''; return; }
    const q = removeAccents(rowCard.q.trim().toLowerCase()), d = row.data, kinds = columnKinds(t), cur = sel && sel.tab === t.id ? sel.fc : -1;
    let html = '', shown = 0;
    t.headers.forEach((h, c) => {
        const v = cellStr(d[c]);
        if (q && !removeAccents(h.toLowerCase()).includes(q) && !removeAccents(v.toLowerCase()).includes(q)) return;
        shown++;
        html += `<div class="rc-f${t.hiddenCols.has(c) ? ' hid' : ''}${c === cur ? ' cur' : ''}" data-c="${c}">`
            + `<label onclick="rowCardGo(${c})" title="${t.hiddenCols.has(c) ? 'Hidden · ' : ''}Click: show in the grid">${typeIcon(kinds[c])}${esc(h)}</label>`
            + `<textarea rows="1" data-c="${c}" spellcheck="false" onfocus="rowCardFocus(this)" onblur="rowCardCommit(this)" onkeydown="rowCardKey(event, this)">${esc(v)}</textarea></div>`;
    });
    box.innerHTML = html || `<div class="rc-none">No field matches "${esc(rowCard.q)}".</div>`;
    if (q) box.insertAdjacentHTML('afterbegin', `<div class="rc-none">${fmt(shown)} of ${fmt(t.headers.length)} fields</div>`);
}
function rowCardFocus(ta) {
    ta.dataset.old = ta.value;
    const { t, vi } = rowCard, c = +ta.dataset.c;
    if (t && T() === t && vi >= 0 && (!sel || sel.fc !== c)) { keepSel = true; setSel(t, vi, c, vi, c); keepSel = false; }
}
/* Leaving a field writes it, like an inline edit: one log entry, that row redrawn. */
function rowCardCommit(ta) {
    const { t, row, vi } = rowCard; if (!t || !row || T() !== t) return;
    const c = +ta.dataset.c, v = ta.value, old = cellStr(row.data[c]);
    if (v === old) return;
    const ed = rowEdits(); ed.set(row, c, v);
    t.modificationsLog.push({ id: row.id, col: t.headers[c], old, new: v, what: `edit in ${t.headers[c]}`, undo: () => ed.undo() });
    flash([[row, c]]);
    updateSaveBtn(); renderTabBar();
    if (t.filteredData[vi] === row) redrawRows(t, vi, vi);
    rowCard.key = '';                         // the log moved on: the next sync redraws the card
}
function rowCardKey(e, ta) {
    e.stopPropagation();                      // the grid's own keys stay out of the card
    if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        const next = ta.closest('.rc-f').nextElementSibling, nta = next && next.querySelector('textarea');
        if (nta) nta.focus(); else ta.blur();
    } else if (e.key === 'Escape') { e.preventDefault(); ta.value = ta.dataset.old || ''; ta.blur(); }
}
/* After a field is left (its commit ran first, on blur): a redraw that was held back while typing. */
document.getElementById('rc-fields').addEventListener('focusout', () => setTimeout(() => { if (rowCard.pending || !rowCard.key) rowCardSync(); }, 0));
function rowCardGo(c) {
    const { t, vi } = rowCard; if (!t || T() !== t || vi < 0) return;
    if (t.hiddenCols.has(c)) { t.hiddenCols.delete(c); applyColStyles(); render(); }
    setSel(t, vi, c, vi, c); revealCell(vi, c);
}
function rowCardMove(step) {
    const { t, vi } = rowCard; if (!t || T() !== t || vi < 0) return;
    const r = Math.min(t.filteredData.length - 1, Math.max(0, vi + step)), c = sel && sel.tab === t.id ? sel.fc : 0;
    setSel(t, r, c, r, c); revealCell(r, c);
}

/* The record as it sits in the file: delimiters and quotes marked, invisible
   characters (no-break spaces, zero-width, direction marks…) named. */
function rawLineHtml(t, row) {
    if (row.b < 0 || !t.base) return '<span class="rc-meta">This row was created here: it has no line in the file yet.</span>';
    const B = t.base, s = B.starts[row.b], e = B.starts[row.b + 1], text = recordText(B, row.b), D = B.delim;
    let n = 0, html = '';
    for (const ch of text) {
        const cp = ch.codePointAt(0);
        if (ch === D) { n++; html += `<span class="dl" title="delimiter">${ch === '\t' ? '⇥' : esc(ch)}</span>`; }
        else if (ch === '"') html += '<span class="qt">"</span>';
        else if (ch === '\n') html += '<span class="inv" title="line break inside the record">↵</span>\n';
        else if (ch === '\r') html += '<span class="inv" title="carriage return">CR</span>';
        else if (cp === 0xA0 || cp === 0x202F || cp === 0x2007 || cp === 0x2009 || /\p{Cf}/u.test(ch) || (cp < 32 && ch !== '\t'))
            html += `<span class="inv" title="${esc(invName(cp))}">U+${cp.toString(16).toUpperCase().padStart(4, '0')}</span>`;
        else html += esc(ch);
    }
    const meta = `line ${fmt(row.src != null ? row.src + 1 : row.b + 1)} · bytes ${fmt(s)}–${fmt(e)} (${fmt(e - s)}) · ${fmt(n + 1)} fields`
        + (B.transcoded ? ' · transcoded from UTF-16' : B.enc !== 'utf-8' ? ` · read as ${esc(encName(B.enc))}` : '')
        + (row.d ? ' · <span class="warn">as read from disk — this row was edited since</span>' : '');
    return `<span class="rc-meta">${meta}</span>${html}`;
}
function invName(cp) {
    return { 0xA0: 'no-break space', 0x202F: 'narrow no-break space', 0x2007: 'figure space', 0x2009: 'thin space', 0x200B: 'zero-width space',
        0x200C: 'zero-width non-joiner', 0x200D: 'zero-width joiner', 0x200E: 'left-to-right mark', 0x200F: 'right-to-left mark',
        0x202A: 'left-to-right embedding', 0x202C: 'pop directional formatting', 0x202D: 'left-to-right override', 0xFEFF: 'byte order mark', 0xAD: 'soft hyphen' }[cp]
        || (cp < 32 ? 'control character' : 'format character');
}

/* Ctrl+G: a row by its number in the file (its row number in the grid). */
async function goToRow() {
    const t = T(); if (!t || !t.loaded) return;
    const v = await uiPrompt(`Go to row\n\n1 to ${fmt(t.allData.length)} — the row's number in the file, filters aside.`, '');
    if (v == null) return;
    const n = parseInt(v.replace(/[\s  .,]/g, ''), 10);
    if (!n || n < 1 || n > t.allData.length) { setStats(`${t.name} | No row "${v}".`); return; }
    let i = t.filteredData === t.allData ? n - 1 : t.filteredData.findIndex(r => r.id === n);
    if (i < 0) {
        if (!await uiConfirm(`Row ${fmt(n)} is hidden by the filters.\n\nClear them to show it?`, { ok: 'Clear the filters' })) return;
        clearAllFilters();
        i = t.filteredData.findIndex(r => r.id === n);
        if (i < 0) return;
    }
    const vis = visibleCols(t), c = sel && sel.tab === t.id && vis.includes(sel.fc) ? sel.fc : (vis[0] || 0);
    container.scrollTop = Math.max(0, thead.offsetHeight + i * ROW_H - container.clientHeight / 2);   // the row in the middle of the view
    setSel(t, i, c, i, c); revealCell(i, c);
}

window.addEventListener('keydown', e => {
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey) return;
    if (document.getElementById('dlg') || document.getElementById('cmdk').classList.contains('open')) return;
    const k = e.key.toLowerCase();
    if (k === 'g') { e.preventDefault(); goToRow(); }
    else if (k === 'i') { e.preventDefault(); toggleRowCard(); }
});
