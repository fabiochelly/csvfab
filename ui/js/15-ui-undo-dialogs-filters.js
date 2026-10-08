/* ---------------------------------------------------------------
   TOASTS
   toast(msg, {kind, undo: {t, entry}}): a card in the bottom right that
   leaves by itself (hover keeps it). Its Undo only acts if that entry is
   still the last edit of the active tab — otherwise it would undo
   something else. Every new undoable edit toasts through
   updateSaveBtn(), except single-cell edits (one toast per keystroke
   would be noise); saves and exports toast through doneMsg().
----------------------------------------------------------------*/
const TOAST_ICONS = {
    ok: '<path d="M20 6 9 17l-5-5"/>',
    danger: '<polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>',
    warn: '<path d="M12 9v4M12 17h.01"/><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 8h.01M11 12h1v4h1"/>'
};
function toast(msg, o = {}) {
    const box = document.getElementById('toasts'), kind = o.kind || 'info';
    const el = document.createElement('div');
    el.className = 'toast ' + kind;
    el.innerHTML = `<svg class="ti" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">${TOAST_ICONS[kind] || TOAST_ICONS.info}</svg>`
        + `<span class="tm">${esc(msg)}</span>` + (o.undo ? '<button class="tu">Undo</button>' : '') + '<span class="tx" title="Dismiss">×</span>';
    const leave = () => { if (el.classList.contains('out')) return; el.classList.add('out'); setTimeout(() => el.remove(), 200); };
    let timer = setTimeout(leave, o.ms || 6000);
    el.onmouseenter = () => clearTimeout(timer);
    el.onmouseleave = () => { timer = setTimeout(leave, 2500); };
    el.querySelector('.tx').onclick = leave;
    if (o.undo) el._entry = o.undo.entry;
    if (o.undo) el.querySelector('.tu').onclick = () => {
        const { t, entry } = o.undo;
        if (T() === t && t.modificationsLog[t.modificationsLog.length - 1] === entry) undo();
        else setStats('That edit is no longer the last one: use Ctrl+Z to step back to it.');
        leave();
    };
    box.appendChild(el);
    while (box.children.length > 4) box.firstElementChild.remove();
}
/* Status bar + toast for an outcome worth noticing (saves, exports, copy). */
function doneMsg(msg) { setStats(msg); toast(msg.split(' | ').slice(1).join(' | ') || msg, { kind: 'ok' }); }

/* Cells that just changed flash once (paste, fill, replace, bulk edits). */
function flash(pairs) {
    for (const [row, c] of pairs) { if (!flashCells.has(row)) flashCells.set(row, new Set()); flashCells.get(row).add(c); }
    clearTimeout(flash.timer);
    flash.timer = setTimeout(() => flashCells.clear(), 1300);   // the CSS animation has run by then; no re-render needed
}

// --- UI State & Modals ---
function updateSaveBtn() {
    const t = T();
    const badge = document.getElementById('edit-badge');
    const n = t ? t.modificationsLog.length : 0;

    badge.style.display = n > 0 ? 'inline-block' : 'none';
    badge.innerText = n;
    document.getElementById('save-split').classList.toggle('dirty', n > 0);
    document.getElementById('btn-discard').style.display = n > 0 ? '' : 'none';
    document.getElementById('menu-undo').classList.toggle('disabled', !(n > 0));
    /* A new edit since the last undo: what was undone can no longer be replayed on top of it. */
    if (t && t.redoStack.length && (n !== t.redoAt || t.modificationsLog[n - 1] !== t.redoAnchor)) t.redoStack = [];
    document.getElementById('menu-redo').classList.toggle('disabled', !(t && t.redoStack.length));
    /* A new undoable edit since last time: announce it, with Undo. */
    if (t) {
        const last = t.modificationsLog[n - 1];
        if (n > (t._seenLog || 0) && last && last !== t._seenLast && last.what && !/^edit in /.test(last.what))
            toast(last.what.charAt(0).toUpperCase() + last.what.slice(1), { undo: { t, entry: last }, kind: /delet|remov|clear/i.test(last.what) ? 'danger' : 'ok' });
        t._seenLog = n; t._seenLast = last;
        if (t.loaded) findRefresh(t);    // an edit that redraws only its rows still changes the find count (29-…)
        spkLater(t);                      // …and the column charts (47-…), when idle
    }
    ffCheck();                            // a fill-by-example rule offered for rows that changed goes (48-…)
}

/* Back to the file as it is on disk: the edits only ever lived in memory,
   so re-reading is the whole undo. View state (filters, hidden columns,
   widths, scroll) survives unless columns were added or deleted, which
   would leave those index-keyed maps pointing at the wrong columns. */
/* ---------------------------------------------------------------
   UNDO / REDO
   Every modificationsLog entry carries an undo(t) closure that puts back
   exactly what that edit changed, so the log doubles as the undo stack:
   popping it down to empty lands on the last save, and the Save button
   stops being yellow. Saving clears the log — there is no undo past it.
   Redo needs no closure of its own: undo() records what the undo closure
   touches — every Row.d assignment (the accessor in 21-…), and, compared
   before and after, t.allData, t.headers, the view maps, the column map
   and the line endings — and the entry, moved to t.redoStack, gets a
   redo(t) that puts those back. An undo closure must therefore never
   mutate t.allData in place (splice): it replaces it, so that the array
   recorded before the undo is the one to restore. Any new edit empties
   the stack (updateSaveBtn: the last entry is no longer the one the
   stack was built on), as do a save and a re-read.
----------------------------------------------------------------*/
function viewSnap(t) {
    return { hidden: new Set(t.hiddenCols), widths: { ...t.colWidths }, filters: { ...t.colFilters }, vals: { ...t.valFilters }, bars: { ...t.dataBars }, sort: t.sort ? t.sort.map(k => ({ ...k })) : null, colSrc: t.colSrc && t.colSrc.slice() };
}
/* Copies, not the snapshot's own objects: a closure may run twice (undo, redo, undo). */
function viewRestore(t, v) {
    t.hiddenCols = new Set(v.hidden); t.colWidths = { ...v.widths }; t.colFilters = { ...v.filters }; t.valFilters = { ...v.vals }; t.dataBars = { ...v.bars };
    t.sort = v.sort ? v.sort.map(k => ({ ...k })) : null; t.colSrc = v.colSrc && v.colSrc.slice();
}
const sameView = (a, b) => JSON.stringify(viewKey(a)) === JSON.stringify(viewKey(b));
function viewKey(v) { return [[...v.hidden].sort(), v.widths, v.filters, Object.keys(v.vals).map(k => [k, [...v.vals[k]]]), v.bars, v.sort, v.colSrc]; }
const REDO_MAX = 40e6;                    // recorded row assignments past which an edit is not redoable (a restructure of 20 M rows)
function undo() {
    const t = T(); if (!t || !t.loaded) return;
    const e = t.modificationsLog.pop();
    if (!e) { setStats(`${t.name} | Nothing to undo.`); return; }
    const before = { rows: t.allData, headers: t.headers, hcopy: t.headers.slice(), view: viewSnap(t), eol: t.detectedEol, moji: t.mojibake, cmap: t.base.cmap };
    dRec = [];
    try { if (e.undo) e.undo(t); } finally { var rec = dRec; dRec = null; }
    const R = {
        rows: t.allData !== before.rows ? before.rows : null,
        /* The undo replaced the headers (redo assigns the array back, a later undo may hold it) or changed them in place (redo does the same). */
        headers: t.headers !== before.headers ? before.headers : before.hcopy.some((h, i) => h !== t.headers[i]) || t.headers.length !== before.hcopy.length ? { inPlace: before.hcopy } : null,
        view: sameView(before.view, viewSnap(t)) ? null : before.view,
        eol: t.detectedEol !== before.eol ? before.eol : undefined, moji: t.mojibake !== before.moji ? before.moji : undefined,
        cmap: t.base.cmap !== before.cmap ? before.cmap : undefined, d: rec
    };
    e.redo = rec.length > REDO_MAX ? null : t => {
        if (R.rows) t.allData = R.rows;
        if (R.headers) { if (R.headers.inPlace) t.headers.splice(0, t.headers.length, ...R.headers.inPlace); else t.headers = R.headers; }
        if (R.view) viewRestore(t, R.view);
        if (R.eol !== undefined) t.detectedEol = R.eol;
        if (R.moji !== undefined) t.mojibake = R.moji;
        if (R.cmap !== undefined) setCmap(t.base, R.cmap);
        for (let k = R.d.length - 2; k >= 0; k -= 2) R.d[k].d = R.d[k + 1];   // last assignment first: a row assigned twice ends as before the first
    };
    if (e.redo) t.redoStack.push(e); else t.redoStack = [];   // an edit too big to redo: nothing above it could be replayed either
    t.redoAt = t.modificationsLog.length; t.redoAnchor = t.modificationsLog[t.redoAt - 1];
    afterUndoRedo(t);
    setStats(`${t.name} | Undone: ${e.what || 'last edit'}${t.modificationsLog.length ? ` — ${t.modificationsLog.length} left` : ' — back to the saved file'}.`);
}
function redo() {
    const t = T(); if (!t || !t.loaded) return;
    const e = t.redoStack.pop();
    if (!e) { setStats(`${t.name} | Nothing to redo.`); return; }
    e.redo(t); e.redo = null;
    t.modificationsLog.push(e);
    t.redoAt = t.modificationsLog.length; t.redoAnchor = e;
    afterUndoRedo(t);
    setStats(`${t.name} | Redone: ${e.what || 'edit'}${t.redoStack.length ? ` — ${t.redoStack.length} more to redo` : ''}.`);
}
function afterUndoRedo(t) {
    t.allData.forEach((r, i) => r.id = i + 1);
    t.rowCount = t.allData.length;
    renderHeader(); applyColStyles(); applyFilters(); updateSaveBtn(); renderTabBar(); updateStats(); refreshParseOpts();
}
window.addEventListener('keydown', (e) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const k = e.key.toLowerCase();
    if (!((k === 'z') || (k === 'y' && !e.shiftKey))) return;
    if (e.target.closest && e.target.closest('input, textarea, select')) return;   // a text field keeps its own undo
    if (document.getElementById('dlg') || document.getElementById('cmdk').classList.contains('open')) return;
    e.preventDefault();
    if (k === 'y' || e.shiftKey) redo(); else undo();
});

async function discardEdits() {
    const t = T(); if (!t || !isDirty(t)) return;
    if (!await uiConfirm(`Discard the ${t.modificationsLog.length} pending edits of "${t.name}" and reload it from disk?`, { ok: 'Discard edits', danger: true })) return;
    reloadKeepingView(t, t.modificationsLog.some(l => l.new === 'Column added' || l.old === 'Column deleted'));
}
function reloadKeepingView(t, colsChanged) {
    t.modificationsLog = []; t.headers = []; t.syntheticHeader = false; t.sort = null; t.rowMark = null;
    if (colsChanged) { t.hiddenCols.clear(); t.colWidths = {}; t.colFilters = {}; t.valFilters = {}; t.dataBars = {}; }
    updateSaveBtn(); renderTabBar();
    parseTab(t);
}

/* ---------------------------------------------------------------
   DIALOGS
   One dialog at a time, stacked above any open modal. The first
   paragraph of the message is the heading, the rest the details.
   Resolves true / false (confirm), the text or null (prompt). Enter
   confirms, Escape or a click beside it cancels.
----------------------------------------------------------------*/
function uiDialog(message, o) {
    return new Promise(resolve => {
        const [main, ...rest] = String(message).split('\n\n');
        const bg = document.createElement('div'); bg.id = 'dlg-bg';
        const box = document.createElement('div'); box.id = 'dlg';
        box.setAttribute('role', 'dialog'); box.setAttribute('aria-modal', 'true');
        box.innerHTML = `<div class="dlg-main">${esc(main)}</div>`
            + (rest.length ? `<div class="dlg-sub">${esc(rest.join('\n\n'))}</div>` : '')
            + (o.input != null ? `<input type="text" class="bs-input" spellcheck="false" autocomplete="off" value="${esc(o.input)}">` : '')
            + '<div class="modal-actions">'
            + (o.choices || []).map((c, i) => `<button class="btn btn-outline" data-r="c${i}">${esc(c)}</button>`).join('')
            + (o.cancel ? `<button class="btn btn-outline" data-r="0">${esc(o.cancel)}</button>` : '')
            + `<button class="btn${o.danger ? ' btn-danger' : ''}" data-r="1">${esc(o.ok || 'OK')}</button></div>`;
        document.body.append(bg, box);
        const input = box.querySelector('input'), prevFocus = document.activeElement;
        const done = ok => {
            document.removeEventListener('keydown', onKey, true);
            bg.remove(); box.remove();
            if (prevFocus && prevFocus.focus) prevFocus.focus();
            resolve(typeof ok === 'string' ? ok : o.input != null ? (ok ? input.value : null) : ok);   // a string = an extra choice
        };
        /* Capture phase + stopPropagation: while a dialog is up, no other
           shortcut (Escape closing the modal below, Ctrl+Z, Ctrl+S…) fires. */
        const onKey = e => {
            if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(false); }
            else if (e.key === 'Enter') { e.stopPropagation(); if (e.target.tagName !== 'BUTTON') { e.preventDefault(); done(true); } }   // a focused button clicks itself
            else if (!box.contains(e.target)) e.stopPropagation();
        };
        document.addEventListener('keydown', onKey, true);
        bg.onclick = () => done(!o.cancel);          // an alert has nothing to cancel
        box.querySelectorAll('button').forEach(b => b.onclick = () => done(b.dataset.r[0] === 'c' ? o.choices[+b.dataset.r.slice(1)] : b.dataset.r === '1'));
        if (input) { input.focus(); input.select(); } else box.querySelector('[data-r="1"]').focus();
    });
}
function uiAlert(message) { return uiDialog(message, {}); }
function uiConfirm(message, o) { return uiDialog(message, Object.assign({ ok: 'OK', cancel: 'Cancel' }, o)); }
/* OK → true, Cancel/Escape → false, an extra choice → its label. */
function uiChoice(message, o) { return uiDialog(message, Object.assign({ ok: 'OK', cancel: 'Cancel' }, o)); }
function uiPrompt(message, value) { return uiDialog(message, { input: value || '', ok: 'OK', cancel: 'Cancel' }); }

/* Escape closes the innermost thing open: a modal, else a dropdown, else the replace bar. */
document.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if ([...document.querySelectorAll('.modal-box')].some(m => m.style.display === 'block')) return closeAllModals();
    if (colPanel) return closeColPanel();
    if (document.querySelector('.dd-menu.open')) return closeDDs();
    if (srBar.style.display === 'flex') return toggleSRBar();
    if (sel) { const t = T(); sel = null; if (t) paintSel(t); updateStats(); }
});
function closeAllModals() {
    document.getElementById('modal-bg').style.display = 'none';
    document.querySelectorAll('.modal-box').forEach(m => m.style.display = 'none');
}

// --- Filters ---
/* Text filters (global search + per-column inputs, Invert applying to
   both) as one row test: null when none is set, false when a regex does
   not compile yet (the view is then left as it was). */
/* A simple filter's words (not under Regex): split on spaces, and every word must be found —
   in any order, anywhere in the row for the search box, in the cell for a column filter — so
   "dupont lyon" finds Lyon's Dupont whatever the columns' order. "Quoted words" stay one exact
   phrase; an unclosed quote runs to the end, as while it is being typed. A query that only grew
   still narrows (narrowsLast()): each old word stays inside a new one. */
function queryTerms(q) {
    const out = [], re = /"([^"]*)"?|(\S+)/g; let m;
    while ((m = re.exec(q))) { const s = m[1] != null ? m[1] : m[2]; if (s) out.push(s); }
    return out;
}
function textFilterTest(t, sparse) {   // sparse: the rows to test are few among the file's (a narrowing search)
    const spec = textFilterSpec(t);
    return spec ? textRowTest(spec, t.base, sparse) : spec;
}
/* What the text filters ask, read from the tab: null when there is none, false
   when an expression or a regex keeps the view (invalid, refused, or being
   checked: applyFilters() is called back). The test itself is built from it by
   textRowTest(), pure, so the filter workers (44-…) build the very same one. */
function textFilterSpec(t) {
    let globalQuery = t.globalQuery;
    const isRegex = t.useRegex;
    const cols = Object.keys(t.colFilters).map(k => ({ idx: parseInt(k, 10), raw: t.colFilters[k] }));
    /* Expression mode: the search box is a formula (23-expr-filter.js), tested
       on top of the column filters; one that does not compile keeps the view. */
    let exprRun = null;
    t.exprErr = ''; t.exprRun = null; t.reErr = '';
    if (t.useExpr && globalQuery.trim()) {
        exprRun = exprRowTest(t, globalQuery);
        if (exprRun.error) { t.exprErr = exprRun.error; markExprBox(!exprRun.pending); return false; }
        t.exprRun = exprRun; globalQuery = '';
    }
    markExprBox(false);
    if (t.useExpr) globalQuery = '';
    if (!globalQuery && !cols.length && !exprRun) return null;

    /* A regex is tried apart first (40-…): one that backtracks without end would freeze the
       window, edits not saved included. The view stays while it is checked, then filters. */
    if (isRegex) {
        if (globalQuery) { try { new RegExp(globalQuery, 'i'); } catch (e) { return false; } }
        const valid = f => { try { new RegExp(f.raw, 'i'); return true; } catch (e) { return false; } };
        const g = regexGate(t, [...(globalQuery ? [['g', globalQuery, 'i']] : []), ...cols.filter(valid).map(f => ['c' + f.idx, f.raw, 'i'])],
            () => { if (T() === t) applyFilters(); });
        if (g) { t.reErr = g.error; return false; }
    }
    return { g: globalQuery, cols, isRegex, isReverse: t.useReverse, useSlug: t.useSlug, expr: exprRun };
}
/* The row test of a spec, over a base (its delimiter, its column map). No tab, no DOM:
   the filter workers run it on their own slice of the file. */
function textRowTest(spec, base, sparse) {
    let globalQuery = spec.g;
    const { isRegex, isReverse, useSlug } = spec, exprRun = spec.expr || null;
    if (useSlug && !isRegex && globalQuery) globalQuery = removeAccents(globalQuery.toLowerCase());
    else if (!isRegex && globalQuery) globalQuery = globalQuery.toLowerCase();

    const globalRegex = isRegex && globalQuery ? new RegExp(globalQuery, 'i') : null;
    const globalTerms = !isRegex && globalQuery ? queryTerms(globalQuery) : [];
    const compiledColFilters = spec.cols.map(i => {
        let re = null, val = i.raw;
        if (useSlug && !isRegex) val = removeAccents(val.toLowerCase()); else if (!isRegex) val = val.toLowerCase();
        if (isRegex) { try { re = new RegExp(i.raw, 'i'); } catch (e) { } }
        return { idx: i.idx, val: val, re: re, raw: i.raw, terms: isRegex ? null : queryTerms(val) };
    }).filter(f => isRegex || f.terms.length);   // spaces only: no filter

    /* The record's own text first: a query without quote or delimiter (and,
       for the search across columns, without space, which joins the cells)
       that is not in it is in none of its cells — most rows are then
       rejected without being split into cells. */
    const D = base && base.delim, lower = useSlug ? v => removeAccents(v.toLowerCase()) : v => v.toLowerCase();
    const plain = q => q && !q.includes('"') && !q.includes(D);
    const colTerms = compiledColFilters.flatMap(f => f.terms || []);
    let pre = !isRegex && D && globalTerms.every(q => plain(q) && !q.includes(' ')) && colTerms.every(plain)
        ? [...globalTerms, ...colTerms] : null;   // every word must be in the record (blockMatches ANDs them)
    if (pre && !pre.length) pre = null;   // an expression alone: no text to look for, so no block to lower (that lowered the whole file for nothing)

    /* The other way round too, for the search across columns alone: such a
       query found in the record's text is inside one of its cells — unless
       the columns were re-mapped (a deleted column's text is still there). */
    const sure = pre && !compiledColFilters.length && !exprRun && !base.cmap;
    let bk = -1, hits = null;               // the block of records last searched, and its matches
    return row => {
        if (pre && !row.d && row.b >= 0) {
            const k = row.b >> BLK_BITS, mode = useSlug ? 's' : 'l';
            if (sparse) { const raw = loweredRecord(row.base, row.b, lower, mode); for (const q of pre) if (!raw.includes(q)) return isReverse; }
            else {
                if (k !== bk) { bk = k; hits = blockMatches(row.base, k, lower, pre, mode); }
                if (hits) { if (!hits[row.b - (k << BLK_BITS)]) return isReverse; }
                else { const raw = lower(recordText(row.base, row.b)); for (const q of pre) if (!raw.includes(q)) return isReverse; }
            }
            if (sure) return !isReverse;
        }
        let match = true;
        for (let f of compiledColFilters) {
            let cellVal = String(cellOf(row, f.idx) || '');
            if (useSlug) cellVal = removeAccents(cellVal.toLowerCase()); else cellVal = cellVal.toLowerCase();
            if (isRegex) { if (f.re && !f.re.test(cellVal)) { match = false; break; } }
            else if (!f.terms.every(q => cellVal.includes(q))) { match = false; break; }
        }
        if (match && globalQuery) {          // the local one: in expression mode t.globalQuery is the formula, already consumed
            let rowText = row.data.join(' ');
            if (useSlug) rowText = removeAccents(rowText.toLowerCase()); else rowText = rowText.toLowerCase();
            if (isRegex) { if (globalRegex && !globalRegex.test(rowText)) match = false; }
            else if (!globalTerms.every(q => rowText.includes(q))) match = false;
        }
        if (match && exprRun && !exprRun.test(row)) match = false;
        return isReverse ? !match : match;
    };
}

function cellStr(v) { return v == null ? '' : String(v); }

/* Value filters (column panel): a row is hidden when its value is one of
   the column's excluded values. Excluding rather than listing the kept
   ones means a value typed later, or one out of view when the filter was
   set, still shows. Invert does not apply: these were ticked by hand.
   skipCol leaves one column out — its panel lists the values the OTHER
   filters let through, so unticked ones stay listed. */
function valueFilterTest(t, skipCol) {
    const fs = Object.keys(t.valFilters).map(Number).filter(c => c !== skipCol).map(c => [c, t.valFilters[c]]);
    return fs.length ? valueRowTest(fs) : null;
}
function valueRowTest(fs) { return row => { for (const [c, ex] of fs) if (ex.has(cellStr(cellOf(row, c)))) return false; return true; }; }

/* What the rows are: any edit, undo, re-read or re-order changes one of
   these (every edit pushes or pops a log entry; convertHeader changes the
   length). Results computed from the rows — a filter's matches, duplicate
   groups, a column's profile — are reused while the stamp is the same. */
function dataStamp(t) { return [t.allData, t.allData.length, t.modificationsLog.length, t.modificationsLog[t.modificationsLog.length - 1], t.headers, t.base && t.base.cmap]; }
function sameStamp(a, b) { return !!a && !!b && a.length === b.length && a.every((x, i) => x === b[i]); }
/* A small id per object (a value filter's Set), for signatures. */
const objIds = new WeakMap(); let objSeq = 0;
function objId(o) { if (!objIds.has(o)) objIds.set(o, ++objSeq); return objIds.get(o); }
/* Everything a filter depends on besides the text queries. */
function filterSig(t, skipCol) {
    return [t.useRegex, t.useSlug, t.useReverse, t.useExpr, t.onlyIrregular, t.onlyDups, t.dupSpec && JSON.stringify(t.dupSpec), t.rowMark && t.rowMark.only && objId(t.rowMark),
        ...Object.keys(t.valFilters).filter(c => +c !== skipCol).sort().map(c => c + ':' + objId(t.valFilters[c]))].join('|');
}

/* Typing in a filter: on a small file each keystroke filters at once; the
   wait grows with what the last filtering cost (up to 400 ms on a large
   one), rather than a fixed 400 ms that made 5 000 rows feel slow. */
let filterTimer = 0, lastFilterMs = 0;
function filterSoon() {
    clearTimeout(filterTimer);
    filterTimer = setTimeout(applyFilters, lastFilterMs < 25 ? 0 : Math.min(400, 80 + lastFilterMs));
}

/* A query that only grew (typing on: "dup" → "dupo"), with nothing else
   changed, can only match fewer rows: then only the rows shown are tested. */
function narrowsLast(t) {
    const L = t.lastFilter;
    if (!L || t.useRegex || t.useReverse || t.useExpr || !sameStamp(L.stamp, dataStamp(t)) || L.sig !== filterSig(t, -1)) return false;
    const low = v => (t.useSlug ? removeAccents(String(v).toLowerCase()) : String(v).toLowerCase());
    if (!low(t.globalQuery).includes(low(L.g))) return false;
    for (const c of Object.keys(L.cols)) if (t.colFilters[c] == null || !low(t.colFilters[c]).includes(low(L.cols[c]))) return false;
    return true;
}

function applyFilters() {
    const t = T(); if (!t || !t.loaded) return;
    clearTimeout(filterTimer);
    const t0 = performance.now();
    collectUIState(t);

    /* Highlight context used by render() */
    t.hl = { globalQuery: t.useExpr ? '' : t.globalQuery, colFilters: t.colFilters, isRegex: t.useRegex, useSlug: t.useSlug, reverse: t.useReverse };   // an expression highlights nothing
    findHl(t);                            // the replace bar's find, highlighted apart (29-…)

    const spec = textFilterSpec(t);
    t.hl.reOk = !t.reErr;                 // render() highlights with the regex only once it passed the guard
    if (spec === false) { updateStats(); return; }   // an invalid regex or expression: the view stays, the status bar says why
    let tt = spec && textRowTest(spec, t.base, false);
    const narrow = tt && narrowsLast(t), few = narrow && t.filteredData.length * 8 < t.allData.length;
    if (few) tt = textRowTest(spec, t.base, true);   // few rows left: record by record
    if (!narrow) dupGroups(t);
    const vt = valueFilterTest(t, -1), n = t.headers.length, irr = t.onlyIrregular, dg = t.onlyDups && t.dupMarks && t.dupMarks.group;
    const mk = t.rowMark && t.rowMark.only && t.rowMark.rows;
    /* A big file: the text and value filters run in the workers (44-…). Their answer is awaited
       only when the rows are those of the view shown, which then stays meanwhile. */
    let bits = null;
    if (!few && (tt || vt) && !(spec && spec.expr) && parUsable(t)) {
        const q = parQuery(t, spec), base = t.base;
        bits = parCached(base, q);
        if (!bits && sameStamp(t.viewStamp, dataStamp(t))) {
            t.filterPending = true;
            parRequest(base, q, () => { if (T() === t && t.base === base) applyFilters(); });
            setStats(`${t.name} | Filtering…`);
            return;
        }
    }
    if (!keepSel) sel = null;             // view indices are about to change
    if (narrow && !bits) {
        const from = t.filteredData, keep = new Uint8Array(from.length);   // the rows shown are already through every other filter
        visitRows(t, from, (row, i) => { if (tt(row)) keep[i] = 1; });
        t.filteredData = from.filter((_, i) => keep[i]);
    } else if (bits || tt || vt || irr || dg || mk) {
        const all = t.allData, keep = new Uint8Array(all.length);   // tested in file order (visitRows), kept in view order
        const other = row => (!irr || row.len !== n) && (!dg || dg.has(row)) && (!mk || mk.has(row));
        if (bits) {                       // the workers answered for the records; a row no longer its record is tested here
            for (let i = 0; i < all.length; i++) {
                const row = all[i];
                if (other(row) && (row.d || row.b < 0 ? (!tt || tt(row)) && (!vt || vt(row)) : bits[row.b])) keep[i] = 1;
            }
        } else visitRows(t, all, (row, i) => { if (other(row) && (!tt || tt(row)) && (!vt || vt(row))) keep[i] = 1; });
        t.filteredData = all.filter((_, i) => keep[i]);
        if (dg) t.filteredData.sort((a, b) => dg.get(a) - dg.get(b));   // groups side by side (a stable sort keeps file order within one)
    } else t.filteredData = t.allData;   /* no filter: reuse the same array, no copy in RAM */
    t.viewStamp = dataStamp(t); t.filterPending = false;
    t.lastFilter = { stamp: dataStamp(t), sig: filterSig(t, -1), g: t.globalQuery, cols: { ...t.colFilters } };
    if (t.exprRun && t.exprRun.errors) t.exprErr = `${fmt(t.exprRun.errors)} rows raise an error and are hidden (${t.exprRun.first})`;
    t.exprRun = null;

    container.scrollTop = 0; t.scrollTop = 0; renderFirst();
    updateStats();
    lastFilterMs = performance.now() - t0;
    spkLater(t);                              // the column charts (47-…): what the filters let through, when idle
}

/* Field count ≠ header width: a stray delimiter, or an unclosed quote that
   swallowed the following lines. Saving pads short rows with empty fields
   and keeps the extra fields of long ones (streamCSV), so nothing is lost —
   but such rows usually mean the file needs a look. */
function updateIrregular(t) {
    const chip = document.getElementById('irr-chip');
    const n = t && t.loaded ? t.headers.length : 0;
    let bad = 0;
    if (n) for (const r of t.allData) if (r.len !== n) bad++;
    const q = t && t.loaded ? t.quoteErrors : 0;
    chip.style.display = bad || q || (t && t.onlyIrregular) ? '' : 'none';
    chip.classList.toggle('on', !!(t && t.onlyIrregular));
    chip.textContent = `⚠ ${fmt(bad)} irregular row${bad === 1 ? '' : 's'}` + (q ? ` · ${fmt(q)} quote error${q === 1 ? '' : 's'}` : '');
    chip.title = `Not ${fmt(n)} fields` + (q ? ' · quote errors: unclosed quotes' : '') + (t && t.onlyIrregular ? ' · click: show all' : ' · click: show only these');
}
function updateDupChip(t) {
    const chip = document.getElementById('dup-chip'), m = t && t.loaded && t.dupMarks;
    chip.style.display = m ? '' : 'none';
    if (!m) return;
    chip.classList.toggle('on', !!t.onlyDups);
    chip.innerHTML = `⧉ ${fmt(m.group.size)} duplicates · ${fmt(m.groups)} group${m.groups === 1 ? '' : 's'}<span class="chip-x" onclick="clearDupMarks(event)" title="Remove the duplicate marks">×</span>`;
    chip.title = `On ${dupSpecText(t.dupSpec)}` + (t.onlyDups ? ' · click: show all' : ' · click: show only these');
}
function toggleIrregular() {
    const t = T(); if (!t || !t.loaded) return;
    t.onlyIrregular = !t.onlyIrregular;
    applyFilters();
}

/* The status bar's counter: rows shown (when a filter hides some) / total,
   and columns, hidden ones counted apart. Other messages never overwrite it. */
function updateCount(t) {
    const box = document.getElementById('sb-count');
    if (!t || !t.loaded) { box.style.display = 'none'; return; }
    const shown = t.filteredData.length, total = t.allData.length, cols = t.headers.length, hid = t.hiddenCols.size;
    box.style.display = '';
    const unit = t.lang ? 'line' : 'row';     // a text file (33-…): lines, and its one column goes without saying
    box.innerHTML = (shown !== total || hasFilter(t) ? `<b class="n-filt">${fmt(shown)}</b> / ` : '') + `<b class="n-total">${fmt(total)}</b> ${unit}${total === 1 ? '' : 's'}`
        + (t.lang ? '' : ` · <b>${fmt(cols - hid)}</b> column${cols - hid === 1 ? '' : 's'}` + (hid ? ` <span class="hid">(${fmt(hid)} hidden)</span>` : ''));
    box.removeAttribute('title'); delete box.dataset.t;   // the counts say it all
}

function updateStats() {
    const t = T();
    updateCount(t);
    ffCheck();                            // another tab shown: its fill-by-example offer goes (48-…)
    if (!t || !t.loaded) document.getElementById('btn-export').style.display = document.getElementById('btn-map').style.display = document.getElementById('btn-insights').style.display = 'none';
    if (!t || !t.loaded) { updateDupChip(null); updateMojiChip(null); updateMarkChip(null); rowCardSync(); }
    if (!t) { setStats('Ready.'); return; }
    if (!t.loaded) { setStats(`${t.name} | ${t.loading ? 'loading…' : 'released from RAM'}`); return; }
    const hasFilters = hasFilter(t);
    document.getElementById('btn-export').style.display = document.getElementById('btn-map').style.display = '';
    document.getElementById('btn-insights').style.display = t.lang ? 'none' : '';
    updateIrregular(t); updateDupChip(t); updateMojiChip(t); updateMarkChip(t);
    const gen = t.lang ? '' : t.syntheticHeader ? ' | no header line: columns numbered from 0' : '';
    const ex = (t.useExpr && t.exprErr ? ` | expression: ${t.exprErr}` : '') + (t.reErr ? ` | regex: ${t.reErr}` : '');
    setStats(`${t.name}${hasFilters ? ' | filtered' : ''}${gen}${ex}`);   // the counts: #sb-count, on the right
    rowCardSync();
    findRefresh(t);                       // the find count follows every change (29-…)
}

/* The column panel is position: fixed under its header, which only moves sideways (the header row is sticky). */
let lastScrollLeft = 0;
container.onscroll = () => {
    const t = T(); if (t) t.scrollTop = container.scrollTop;
    if (colPanel && container.scrollLeft !== lastScrollLeft) closeColPanel();
    lastScrollLeft = container.scrollLeft;
    renderOnScroll();
};
window.onresize = resizeContainer;
/* render() only mounts the rows that fit the container's height at that
   moment. A window launched from the file manager is born small and then
   tiled to full size by the compositor, so without this the first slice
   (a dozen rows) stayed alone until the user scrolled. The observer also
   covers the toolbar wrapping and the replace bar opening. A new size is
   met the way a scroll is (renderOnScroll): the rows drawn stay, the ones
   now in view are added — a full redraw here came right after every file
   was shown (its scrollbars appear with its rows: 24 px less of width and
   height), ~10 ms for what the grow had just drawn. Any render meanwhile
   drew at the new size, and cancels it (12-…). */
new ResizeObserver(() => {
    cancelAnimationFrame(resizeFrame);
    resizeFrame = requestAnimationFrame(() => { resizeFrame = 0; renderOnScroll(); });
}).observe(container);
function debounce(f, ms) { let t; return (...a) => { clearTimeout(t); t = setTimeout(() => f(...a), ms); }; }

function setupResizer(resizer) {
    let x = 0, w = 0; const colIdx = parseInt(resizer.dataset.col, 10);
    const mouseMoveHandler = (e) => {
        const t = T(); if (!t) return;
        const newW = Math.max(25, w + (e.clientX - x)), th = resizer.parentElement;
        th.style.width = `${newW}px`; th.style.minWidth = `${newW}px`;
        t.colWidths[colIdx] = newW;
        applyColStyles(); placeCells(t);      // the drawn cells follow, in place
    };
    /* Every cell clipped while the drag lasts (.rz), then redrawn: a cell that fitted
       the old width may overflow the new one, and its clip is decided at render (cellOv). */
    const mouseUpHandler = () => { document.removeEventListener('mousemove', mouseMoveHandler); document.removeEventListener('mouseup', mouseUpHandler); resizer.style.background = ""; document.getElementById('grid-layer').classList.remove('rz'); render(); };
    resizer.addEventListener('mousedown', (e) => { x = e.clientX; w = resizer.parentElement.getBoundingClientRect().width; document.getElementById('grid-layer').classList.add('rz'); document.addEventListener('mousemove', mouseMoveHandler); document.addEventListener('mouseup', mouseUpHandler); resizer.style.background = "var(--prim)"; });
    resizer.addEventListener('dblclick', e => { e.stopPropagation(); fitColumns([colIdx]); });   // fit the column to its content (12-…)
}
