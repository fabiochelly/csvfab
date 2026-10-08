// --- Inline Edit Module ---
/* In-place editing. Double-click or F2 (caret at the end), Enter (text selected), or simply typing on the
   selection (the key typed replaces the content, as in Excel). When the
   edited cell belongs to a multi-cell selection: Enter writes the value
   into every selected cell, Ctrl+Enter fills them with a series starting
   from it; clicking elsewhere only commits the edited cell. Escape cancels. */
let closeEditor = null;
/* typed: the key that started the edit (it replaces the value); caretEnd:
   the caret after the value (F2, double-click) rather than all of it
   selected (Enter). */
function startEdit(td, typed, caretEnd) {
    const t = T(); if (!t) return;
    const viewIdx = +td.parentElement.getAttribute('data-idx');
    const colIdx = +td.getAttribute('data-c');
    if (closeEditor) {                        // one editor at a time: commit the open one…
        closeEditor();
        td = tbody.querySelector(`.row[data-idx="${viewIdx}"] .cell[data-c="${colIdx}"]`);   // …whose re-render replaced this cell
        if (!td) return;
    }
    const rowObj = t.filteredData[viewIdx];
    if (!rowObj) return;
    const oldVal = cellStr(rowObj.data[colIdx]);
    const rg = selRange(t);
    const multi = rg && viewIdx >= rg.r0 && viewIdx <= rg.r1 && colIdx >= rg.c0 && colIdx <= rg.c1
        && (rg.r1 > rg.r0 || visibleCols(t).filter(c => c >= rg.c0 && c <= rg.c1).length > 1) ? rg : null;

    /* A textarea floating over the cell rather than inside it: cells clip
       their overflow, and a multi-line value (Shift+Enter) grows downwards. */
    const input = document.createElement('textarea');
    input.className = 'cell-editor'; input.spellcheck = false;
    input.value = typed != null ? typed : oldVal;
    const b = td.getBoundingClientRect();
    Object.assign(input.style, { left: b.left + 'px', top: b.top + 'px', width: Math.max(b.width, 240) + 'px', minHeight: b.height + 'px' });
    /* A text line (33-…): spaces kept, no wrapping, over the part of the line in view — the
       column may be thousands of pixels wide — and Ln / Col followed in the status bar. */
    const txt = !!t.lang;
    if (txt) {
        input.classList.add('tx'); input.wrap = 'off'; input.rows = 1;   // a textarea is two rows tall by default
        const cr = container.getBoundingClientRect(), left = Math.max(b.left, cr.left + idxColW);
        Object.assign(input.style, { left: left + 'px', width: Math.max(240, Math.min(b.right, cr.right - 26) - left) + 'px' });
        const pos = () => textPos(t, { input, line: rowObj.id });
        for (const ev of ['input', 'keyup', 'mouseup', 'select', 'focus']) input.addEventListener(ev, pos);
        document.addEventListener('selectionchange', pos);
        input.addEventListener('blur', () => document.removeEventListener('selectionchange', pos), { once: true });
    }
    document.body.appendChild(input);
    td.classList.add('editing');
    const grow = () => {                      // + 4: the 2 px border, top and bottom (border-box)
        const max = window.innerHeight - b.top - 12;
        input.style.height = 'auto';
        input.style.height = Math.min(input.scrollHeight + 4, max) + 'px';
        input.style.overflowY = input.scrollHeight + 4 > max ? 'auto' : 'hidden';
    };
    input.addEventListener('input', grow);
    grow(); input.focus();
    if (typed != null || caretEnd) input.setSelectionRange(input.value.length, input.value.length); else input.select();
    if (txt) textPos(t, { input, line: rowObj.id });
    const onScroll = () => finish('one');
    container.addEventListener('scroll', onScroll, { once: true });
    setStats(multi ? `${t.name} | Enter: this value in the ${fmt((multi.r1 - multi.r0 + 1) * visibleCols(t).filter(c => c >= multi.c0 && c <= multi.c1).length)} selected cells · Ctrl+Enter: a series from it · Shift+Enter: new line · Escape: cancel`
        : `${t.name} | Enter: validate · Shift+Enter: new line · Escape: cancel`);

    let done = false;
    const finish = mode => {                  // 'one' | 'all' | 'series' | 'cancel'
        if (done) return; done = true;
        closeEditor = null;
        const v = input.value;
        container.removeEventListener('scroll', onScroll);
        input.remove(); td.classList.remove('editing');
        if (txt) textPos(t);                  // back to the line's Ln
        if (mode === 'cancel') { paintSel(t); selStats(t); return; }
        if (mode === 'one' || !multi) {
            if (v !== oldVal) {
                const ed = rowEdits();
                ed.set(rowObj, colIdx, v);
                t.modificationsLog.push({ id: rowObj.id, col: t.headers[colIdx], old: oldVal, new: v, what: `edit in ${t.headers[colIdx]}`,
                    undo: () => ed.undo() });
                updateSaveBtn(); renderTabBar();
                redrawRows(t, viewIdx, viewIdx);   // that row alone
                if (!oldVal.trim() && v.trim()) ffNoteExample(t, rowObj, colIdx);   // an example for fill by example (48-…)
            }
            return;
        }
        /* The whole selection: the same value, or a series seeded by it — down
           each column, or along the row when the selection is a single row. */
        const cols = visibleCols(t).filter(c => c >= multi.c0 && c <= multi.c1), changes = [];
        const gen = mode === 'series' ? seriesFrom([v], false) : null;
        const horizontal = multi.r0 === multi.r1;
        for (let r = multi.r0; r <= multi.r1; r++) cols.forEach((c, ci) =>
            changes.push([t.filteredData[r], c, gen ? gen(horizontal ? ci : r - multi.r0) : v]));
        const n = setCells(t, changes, mode === 'series' ? 'filled with a series' : 'set');
        render(); if (!n) selStats(t);
    };
    input.addEventListener('blur', () => finish('one'));
    closeEditor = () => finish('one');
    input.addEventListener('keydown', ev => {
        ev.stopPropagation();                 // the grid's own keys stay out of the editor
        if (ev.key === 'Enter' && !ev.shiftKey) { ev.preventDefault(); finish(ev.ctrlKey || ev.metaKey ? 'series' : 'all'); }   // Shift+Enter: a line break
        else if (ev.key === 'Escape') { ev.preventDefault(); finish('cancel'); }
        else if (txt && ev.key === 'Tab' && !ev.shiftKey && !ev.ctrlKey && !ev.altKey) { ev.preventDefault(); input.setRangeText('\t', input.selectionStart, input.selectionEnd, 'end'); textPos(t, { input, line: rowObj.id }); }   // code is indented with tabs
    });
}

/* Writes [row, column, value] triples as one undoable edit; only the cells
   whose value changes are touched. Returns how many changed. */
function setCells(t, changes, verb) {
    const before = [], ed = rowEdits();
    for (const [row, c, v] of changes) {
        if (cellStr(row.data[c]) === v) continue;
        before.push([row, c]);
        ed.set(row, c, v);
    }
    if (!before.length) { setStats(`${t.name} | Nothing changed.`); return 0; }
    flash(before);
    t.modificationsLog.push({ id: '-', col: '---', old: verb, new: `${before.length} cells`, what: `${fmt(before.length)} cells ${verb}`, undo: () => ed.undo() });
    updateSaveBtn(); renderTabBar();
    setStats(`${t.name} | ${fmt(before.length)} cells ${verb} — not written yet, use Save.`);
    return before.length;
}

/* ---------------------------------------------------------------
   SERIES
   seriesFrom(seeds) returns k → value, where seeds sit at k = 0…n−1
   and k may run past them either way (a fill upwards uses k < 0).
   Recognised, all seeds alike: numbers (step = the seeds' trend, one
   seed = +1; decimals, decimal comma and leading zeros kept), dates
   (by day, month or year, in the seeds' own format), text ending in a
   number ("Lot 7", "A007"), French and English day and month names.
   Anything else — or copy — repeats the seeds in a loop.
----------------------------------------------------------------*/
const NAME_LISTS = [
    ['lundi', 'mardi', 'mercredi', 'jeudi', 'vendredi', 'samedi', 'dimanche'],
    ['lun', 'mar', 'mer', 'jeu', 'ven', 'sam', 'dim'],
    ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'],
    ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'],
    ['janvier', 'février', 'mars', 'avril', 'mai', 'juin', 'juillet', 'août', 'septembre', 'octobre', 'novembre', 'décembre'],
    ['janv', 'févr', 'mars', 'avr', 'mai', 'juin', 'juil', 'août', 'sept', 'oct', 'nov', 'déc'],
    ['january', 'february', 'march', 'april', 'may', 'june', 'july', 'august', 'september', 'october', 'november', 'december'],
    ['jan', 'feb', 'mar', 'apr', 'may', 'jun', 'jul', 'aug', 'sep', 'oct', 'nov', 'dec']
];
function seriesFrom(seeds, copy) {
    const vals = seeds.map(cellStr), n = vals.length;
    const loop = k => vals[((k % n) + n) % n];
    if (copy || !vals.some(v => v.trim())) return loop;
    const trim = vals.map(v => v.trim());
    const stepOf = nums => n === 1 ? 1 : (nums[n - 1] - nums[0]) / (n - 1);

    /* Numbers */
    if (trim.every(v => isNumericLike(v) && !isNaN(numKey(v)))) {
        const nums = trim.map(numKey), step = stepOf(nums);
        const comma = trim.some(v => /,\d+$/.test(v) && !/\./.test(v)), grouped = trim.some(v => /\d[\s  ]\d{3}/.test(v));
        const dec = Math.max(0, ...trim.map(v => { const m = v.match(/[.,](\d+)$/); return m && !(grouped && !comma) ? m[1].length : 0; }), step % 1 ? String(step).split('.')[1].length : 0);
        const lead = /^-?0\d/.test(trim[0]) ? trim[0].replace('-', '').length : 0;
        return k => {
            const x = nums[0] + step * k;
            if (grouped) return x.toLocaleString('fr-FR', { minimumFractionDigits: dec, maximumFractionDigits: dec });
            let s = Math.abs(x).toFixed(Math.min(dec, 10));
            if (lead && !dec) s = s.padStart(lead, '0');
            if (comma) s = s.replace('.', ',');
            return (x < 0 ? '-' : '') + s;
        };
    }
    /* Dates: yyyy-mm-dd or dd/mm/yyyy, the rest of the first seed (a time) kept as is */
    const DRE = /^(\d{4})([-\/.])(\d{1,2})\2(\d{1,2})((?:[ T].*)?)$|^(\d{1,2})([-\/.])(\d{1,2})\7(\d{4}|\d{2})((?:[ T].*)?)$/;   // 4-digit year tried first
    const ds = trim.map(v => {
        const m = v.match(DRE); if (!m) return null;
        const iso = !!m[1], y = +(iso ? m[1] : (m[9].length === 2 ? '20' + m[9] : m[9])), mo = +(iso ? m[3] : m[8]), d = +(iso ? m[4] : m[6]);
        const dt = new Date(Date.UTC(y, mo - 1, d));
        return dt.getUTCMonth() === mo - 1 ? { iso, sep: iso ? m[2] : m[7], y, mo, d, pad: (iso ? m[3] : m[6]).length === 2, yy: !iso && m[9].length === 2, rest: iso ? m[5] : m[10], t: dt.getTime() } : null;
    });
    if (ds.every(Boolean) && ds.every(x => x.iso === ds[0].iso && x.sep === ds[0].sep)) {
        const f = ds[0], DAY = 86400000;
        let unit = 'd', step = n === 1 ? 1 : Math.round((ds[n - 1].t - f.t) / DAY / (n - 1));
        /* Same day of the month — or every seed the last day of its month — steps by months. */
        const lastDay = (y, mo) => new Date(Date.UTC(y, mo, 0)).getUTCDate();
        const eom = n > 1 && ds.every(x => x.d === lastDay(x.y, x.mo)) && !ds.every(x => x.d === f.d);
        if (n > 1 && (eom || ds.every(x => x.d === f.d))) {
            const months = x => x.y * 12 + x.mo;
            const mStep = (months(ds[n - 1]) - months(f)) / (n - 1);
            if (mStep && Number.isInteger(mStep)) { unit = eom ? 'eom' : 'm'; step = mStep; }
        }
        const p2 = x => f.pad ? String(x).padStart(2, '0') : String(x);
        const out = (y, mo, d) => f.iso ? `${y}${f.sep}${p2(mo)}${f.sep}${p2(d)}${f.rest}`
            : `${p2(d)}${f.sep}${p2(mo)}${f.sep}${f.yy ? String(y).slice(-2) : y}${f.rest}`;
        return k => {
            if (unit !== 'd') {
                const tot = f.y * 12 + (f.mo - 1) + step * k, y = Math.floor(tot / 12), mo = tot - y * 12 + 1;
                return out(y, mo, unit === 'eom' ? lastDay(y, mo) : Math.min(f.d, lastDay(y, mo)));   // 31 Jan + 1 month = 28/29 Feb
            }
            const dt = new Date(f.t + step * k * DAY);
            return out(dt.getUTCFullYear(), dt.getUTCMonth() + 1, dt.getUTCDate());
        };
    }
    /* Day and month names, case kept from the first seed */
    const low = trim.map(v => v.toLocaleLowerCase('fr').replace(/\.$/, ''));
    const list = NAME_LISTS.find(l => low.every(v => l.includes(v)));
    if (list) {
        const idx = low.map(v => list.indexOf(v)), L = list.length;
        const step = n === 1 ? 1 : ((idx[1] - idx[0]) % L + L) % L || 1;
        const up = trim[0] === trim[0].toUpperCase(), cap = !up && trim[0][0] === trim[0][0].toUpperCase();
        const dot = /\.$/.test(trim[0]);
        return k => {
            const w = list[(((idx[0] + step * k) % L) + L) % L];
            return (up ? w.toUpperCase() : cap ? w[0].toUpperCase() + w.slice(1) : w) + (dot ? '.' : '');
        };
    }
    /* Text ending in a number: the number counts, zero padding kept */
    const TN = /^(.*?)(\d+)(\D*)$/;
    const tn = vals.map(v => v.match(TN));
    if (tn.every(Boolean) && tn.every(m => m[1] === tn[0][1] && m[3] === tn[0][3])) {
        const nums = tn.map(m => +m[2]), step = Math.round(stepOf(nums)) || 1, width = tn[0][2].length, zero = tn[0][2][0] === '0' && width > 1;
        return k => { const x = nums[0] + step * k; return tn[0][1] + (zero ? String(Math.max(0, x)).padStart(width, '0') : String(x)) + tn[0][3]; };
    }
    return loop;
}

/* Fill down (Ctrl+D, menu): in each selected column, the filled cells at
   the top are the seeds and the series runs through the rest of the range
   (a column filled throughout is re-seeded from its first cell). */
function fillDown() {
    const t = T(); if (!t || !t.loaded) return;
    const rg = selRange(t);
    if (!rg || rg.r1 === rg.r0) { uiAlert('Select the cells to fill first.\n\nThe filled cells at the top of each column are the start of the series; the empty ones below them are filled.'); return; }
    const changes = [];
    for (const c of visibleCols(t).filter(c => c >= rg.c0 && c <= rg.c1)) {
        let s = 0; while (rg.r0 + s <= rg.r1 && cellStr(t.filteredData[rg.r0 + s].data[c]).trim()) s++;
        if (!s) continue;
        if (rg.r0 + s > rg.r1) s = 1;
        const gen = seriesFrom(Array.from({ length: s }, (_, k) => t.filteredData[rg.r0 + k].data[c]), false);
        for (let r = rg.r0 + s; r <= rg.r1; r++) changes.push([t.filteredData[r], c, gen(r - rg.r0)]);
    }
    if (setCells(t, changes, 'filled with a series')) render();
}

/* Fill empty cells from above: the layout of exports and pivot tables,
   where a category is written once at the top of its group. In the
   selected columns (one selected cell: its whole column), in the order
   shown, each blank cell takes the nearest value above it — above the
   selection too; blanks before any value stay blank. */
function fillBlanks() {
    const t = T(); if (!t || !t.loaded) return;
    const rg = selRange(t);
    if (!rg) { uiAlert('Select the column to fill first.\n\nOne cell selects its whole column; a range limits the fill to those rows. Each empty cell then takes the value above it.'); return; }
    const one = rg.r0 === rg.r1, r0 = one ? 0 : rg.r0, r1 = one ? t.filteredData.length - 1 : rg.r1;
    const blank = v => !cellStr(v).trim(), changes = [];
    for (const c of visibleCols(t).filter(c => c >= rg.c0 && c <= rg.c1)) {
        let last = null;
        for (let r = r0 - 1; r >= 0 && last === null; r--) { const v = t.filteredData[r].data[c]; if (!blank(v)) last = v; }
        for (let r = r0; r <= r1; r++) {
            const row = t.filteredData[r], v = row.data[c];
            if (!blank(v)) last = v;
            else if (last !== null) changes.push([row, c, cellStr(last)]);
        }
    }
    if (!changes.length) { setStats(`${t.name} | No empty cell with a value above it.`); return; }
    if (setCells(t, changes, 'filled from above')) render();
}

/* Fill handle: the square at the selection's bottom-right corner. Dragged
   down, up, right or left, it extends the series of the selected cells
   (Ctrl held at release: copies them instead). */
let fillDrag = null;                      // { rg, r, c } — pointer cell while dragging
function fillRect() {
    if (!fillDrag) return null;
    const { rg, r, c } = fillDrag, t = T(), vis = visibleCols(t);
    const ci = vis.indexOf(c), c0i = vis.indexOf(rg.c0) < 0 ? 0 : vis.indexOf(rg.c0), c1i = vis.indexOf(rg.c1) < 0 ? vis.length - 1 : vis.indexOf(rg.c1);
    const dDown = r - rg.r1, dUp = rg.r0 - r, dRight = ci - c1i, dLeft = c0i - ci;
    const best = Math.max(dDown, dUp, dRight, dLeft);
    if (best <= 0) return null;
    if (best === dDown) return { dir: 'down', r0: rg.r1 + 1, r1: r, c0: rg.c0, c1: rg.c1 };
    if (best === dUp) return { dir: 'up', r0: r, r1: rg.r0 - 1, c0: rg.c0, c1: rg.c1 };
    if (best === dRight) return { dir: 'right', r0: rg.r0, r1: rg.r1, c0: vis[c1i + 1], c1: c };
    return { dir: 'left', r0: rg.r0, r1: rg.r1, c0: c, c1: vis[c0i - 1] };
}
function doFill(t, rg, fr, copy) {
    const vis = visibleCols(t), changes = [];
    if (fr.dir === 'down' || fr.dir === 'up') {
        for (const c of vis.filter(c => c >= rg.c0 && c <= rg.c1)) {
            const gen = seriesFrom(Array.from({ length: rg.r1 - rg.r0 + 1 }, (_, k) => t.filteredData[rg.r0 + k].data[c]), copy);
            for (let r = fr.r0; r <= fr.r1; r++) changes.push([t.filteredData[r], c, gen(r - rg.r0)]);
        }
        sel = { tab: t.id, ar: fr.dir === 'down' ? rg.r0 : rg.r1, ac: rg.c0, fr: fr.dir === 'down' ? fr.r1 : fr.r0, fc: rg.c1 };
    } else {
        const seedCols = vis.filter(c => c >= rg.c0 && c <= rg.c1), base = vis.indexOf(seedCols[0]);
        const targets = vis.filter(c => c >= fr.c0 && c <= fr.c1);
        for (let r = rg.r0; r <= rg.r1; r++) {
            const gen = seriesFrom(seedCols.map(c => t.filteredData[r].data[c]), copy);
            for (const c of targets) changes.push([t.filteredData[r], c, gen(vis.indexOf(c) - base)]);
        }
        sel = { tab: t.id, ar: rg.r0, ac: fr.dir === 'right' ? rg.c0 : rg.c1, fr: rg.r1, fc: fr.dir === 'right' ? fr.c1 : fr.c0 };
    }
    setCells(t, changes, copy ? 'filled (copy)' : 'filled with a series');
    render();
}
