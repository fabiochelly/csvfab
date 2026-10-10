/* ---------------------------------------------------------------
   FIND, WITHOUT FILTERING
   The search box filters: the rows that do not match disappear. The
   find field of the replace bar does the opposite — every match in the
   rows shown is highlighted (mark.fd, through t.hl), and ‹ › / Enter /
   F3 step through them, selecting the matching cell, the rows around
   it left in place. Hidden columns are skipped; the bar's column scope,
   Regex and Match case apply to find and replace alike.
   The matches are counted in the background (findCount: slices of
   ~12 ms in view order, so typing never waits) into a sorted list of
   cell positions, which then gives the position of the current match
   ("3 / 128") and the next one by binary search; until the count is
   done, or past FIND_LIST matches, a step scans forward from the
   selection instead. The list is dropped whenever the query, the scope,
   the rows shown or the data change (findKey).
----------------------------------------------------------------*/
const FIND_LIST = 2e6;
const find = { key: [], hlKey: '', list: null, n: 0, done: false, gen: 0, timer: 0, hit: null, tab: null, rows: null, upto: 0 };

/* What the bar asks for, or null (empty field) / {error} (a bad regex; guard: true when it is
   held by the regex guard, 40-…, as too slow or being checked). */
function findSpec(t) {
    const q = document.getElementById('sr-find').value;
    if (!q) return null;
    const col = document.getElementById('sr-col').value, only = col === '' ? -1 : +col;
    const cs = document.getElementById('sr-case').checked;
    let re = null;
    if (document.getElementById('sr-regex').checked) {
        try { re = new RegExp(q, cs ? '' : 'i'); } catch (e) { return { error: e.message }; }
        const g = regexGate(t, [[only < 0 ? 'g' : 'c' + only, q, cs ? '' : 'i']], () => { if (T() === t) findRefresh(t); });
        if (g) return { error: g.error, guard: true, pending: g.pending };
    }
    const cols = visibleCols(t).filter(c => only < 0 || c === only);
    const needle = cs ? q : q.toLowerCase();
    const test = re ? v => re.test(v) : cs ? v => v.indexOf(needle) >= 0 : v => v.toLowerCase().indexOf(needle) >= 0;
    /* Plain text: a record whose raw text does not hold it has no cell that does, so most rows are skipped without being split. */
    const raw = re ? null : (cs ? v => v.indexOf(needle) >= 0 : v => v.toLowerCase().indexOf(needle) >= 0);
    return { q, re, cols, only, cs, test, raw, hl: re ? q : buildSafePattern(q, false) };
}
function findBarOpen() { return srBar.style.display === 'flex'; }

/* The highlight context's find fields, from the bar (applyFilters rebuilds t.hl). */
function findHl(t) {
    const S = findBarOpen() ? findSpec(t) : null;
    if (S && !S.error) { t.hl.find = S.hl; t.hl.findCol = S.only; t.hl.findCase = S.cs; }
    else { delete t.hl.find; }
}

/* Keep the highlight and the count in step with the bar and the data:
   called from the bar's inputs and from updateStats() after any change. */
function findRefresh(t) {
    if (!t || !t.loaded || !t.hl) return;
    const S = findBarOpen() ? findSpec(t) : null, ok = S && !S.error;
    document.getElementById('sr-bar').classList.toggle('bad', !!(S && S.error && !S.pending));
    const hlKey = ok ? [S.hl, S.only, S.cs].join('\0') : '';
    if (hlKey !== find.hlKey || (t.hl.find || '') !== (ok ? S.hl : '')) {
        find.hlKey = hlKey;
        findHl(t); render();
    }
    const key = ok ? [hlKey, S.cols.join(','), t.id, t.filteredData, t.filteredData.length, ...dataStamp(t)] : [];   // S.cols: a column hidden or shown changes the count
    if (key.length !== find.key.length || key.some((x, i) => x !== find.key[i])) {
        find.key = key;
        clearTimeout(find.timer); find.gen++;
        find.list = null; find.n = 0; find.done = false; find.hit = null;
        if (ok) findCount(t, S); else { findPos(t); stripFind(t); }
    } else findPos(t);
}
function findChanged() { clearTimeout(findChanged.timer); findChanged.timer = setTimeout(() => findRefresh(T()), 150); }

/* Count the matches in the rows shown, a slice at a time, into a sorted list of row × width + column. */
function findCount(t, S) {
    const gen = ++find.gen, rows = t.filteredData, W = t.headers.length, list = [], hit = new Uint8Array(rows.length);
    let i = 0, n = 0, capped = false, drawn = 0;
    Object.assign(find, { hit, tab: t.id, rows, upto: 0 });   // the rows holding a match, for the scroll strip's ticks (28-…)
    const step = () => {
        if (gen !== find.gen || T() !== t) return;
        const t0 = performance.now();
        for (; i < rows.length && performance.now() - t0 < 12; i++) {
            const row = rows[i];
            if (S.raw && !row.d && row.b >= 0 && !S.raw(recordText(row.base, row.b))) continue;
            const d = row.data;
            for (const c of S.cols) {
                const v = d[c];
                if (v == null || v === '' || !S.test(String(v))) continue;
                n++; hit[i] = 1;
                if (list.length < FIND_LIST) list.push(i * W + c); else capped = true;
            }
        }
        find.n = n; find.upto = i;
        if (i >= rows.length || performance.now() - drawn > 100) { drawn = performance.now(); stripFind(t); }   // the strip's ticks as the count goes
        if (i < rows.length) { find.timer = setTimeout(step, 0); findPos(t); return; }
        find.list = capped ? null : list; find.done = true; findPos(t);
    };
    step();
}

/* "3 / 128" when the selection sits on a match, else the count. */
function findPos(t) {
    const el = document.getElementById('sr-pos');
    const S = findBarOpen() && t ? findSpec(t) : null;
    if (S && S.guard) { el.textContent = S.pending ? 'checking…' : 'too slow'; el.classList.toggle('none', !S.pending); return; }
    if (!findBarOpen() || !find.key.length) { el.textContent = ''; el.classList.remove('none'); return; }
    const n = find.n, more = find.done && !find.list && n >= FIND_LIST ? '+' : '';
    el.classList.toggle('none', find.done && n === 0);
    if (!find.done) { el.textContent = `${fmt(n)}…`; return; }
    if (n === 0) { el.textContent = 'No match'; return; }
    if (find.list && sel && sel.tab === t.id) {
        const k = findIndexOf(find.list, sel.fr * t.headers.length + sel.fc);
        if (k >= 0) { el.textContent = `${fmt(k + 1)} / ${fmt(n)}`; return; }
    }
    el.textContent = `${fmt(n)}${more} match${n === 1 ? '' : 'es'}`;
}
function findIndexOf(list, x) {   // binary search; -1 when absent
    let lo = 0, hi = list.length - 1;
    while (lo <= hi) { const m = (lo + hi) >> 1; if (list[m] === x) return m; if (list[m] < x) lo = m + 1; else hi = m - 1; }
    return -1;
}
function findLowerBound(list, x) {   // first index whose value is ≥ x
    let lo = 0, hi = list.length;
    while (lo < hi) { const m = (lo + hi) >> 1; if (list[m] < x) lo = m + 1; else hi = m; }
    return lo;
}

/* The next match after (r, c) in direction dir, wrapping around: [r, c, wrapped] or null. */
function findFrom(t, S, r, c, dir) {
    const rows = t.filteredData, W = t.headers.length;
    if (!rows.length || !S.cols.length) return null;
    const pos = r * W + c;
    if (find.done && find.list) {
        const L = find.list; if (!L.length) return null;
        let k = dir > 0 ? findLowerBound(L, pos + 1) : findLowerBound(L, pos) - 1, wrapped = false;
        if (k >= L.length) { k = 0; wrapped = true; } else if (k < 0) { k = L.length - 1; wrapped = true; }
        return [Math.floor(L[k] / W), L[k] % W, wrapped];
    }
    /* No list yet: scan from the selection, the rest of its row first. */
    const cols = S.cols, n = rows.length;
    const test = (i, from, to) => {
        const row = rows[i];
        if (S.raw && !row.d && row.b >= 0 && !S.raw(recordText(row.base, row.b))) return -1;
        const d = row.data;
        for (let k = from; k !== to; k += dir) { const v = d[cols[k]]; if (v != null && v !== '' && S.test(String(v))) return cols[k]; }
        return -1;
    };
    if (r >= 0 && r < n) {                    // r may sit just outside the rows: a step with no selection starts before the view
        const k = dir > 0 ? findLowerBound(cols, c + 1) : findLowerBound(cols, c) - 1;
        const first = test(r, k, dir > 0 ? cols.length : -1);
        if (first >= 0) return [r, first, false];
    }
    for (let s = 1; s <= n; s++) {
        const i = ((r + dir * s) % n + n) % n, wrapped = dir > 0 ? i <= r : i >= r;
        const hit = test(i, dir > 0 ? 0 : cols.length - 1, dir > 0 ? cols.length : -1);
        if (hit >= 0) return [i, hit, wrapped];
        if (i === r) break;
    }
    return null;
}

function findStep(dir) {
    const t = T(); if (!t || !t.loaded) return;
    if (!findBarOpen()) toggleSRBar();
    const S = findSpec(t);
    const box = document.getElementById('sr-find');
    if (!S) { box.focus(); return; }
    if (S.error) { setStats(`${t.name} | ${S.guard ? 'Regex: ' : 'This regular expression is not valid: '}${S.error}`); return; }
    findRefresh(t);
    /* From the selection, else from the top of the view (backwards: its bottom). */
    let r, c;
    if (sel && sel.tab === t.id) { r = sel.fr; c = sel.fc; }
    else { const [v0, v1] = viewRows(t); r = dir > 0 ? v0 - 1 : Math.min(t.filteredData.length, v1 + 1); c = dir > 0 ? t.headers.length : -1; }
    const m = findFrom(t, S, r, c, dir);
    if (!m) { setStats(`${t.name} | No match for "${S.q}" in the rows shown.`); findPos(t); return; }
    const [mr, mc, wrapped] = m;
    /* A row outside the view lands in the middle of it. */
    const top = thead.offsetHeight, y = top + mr * ROW_H;
    if (y < container.vTop + top || y + ROW_H > container.vTop + container.clientHeight) container.vTop = Math.max(0, y - container.clientHeight / 2);
    setSel(t, mr, mc, mr, mc); revealCell(mr, mc);
    if (wrapped) setStats(`${t.name} | Wrapped to the ${dir > 0 ? 'top' : 'bottom'} of the rows shown.`);
    findPos(t);
}
function findKey(e) {
    if (e.key === 'Enter') { e.preventDefault(); findStep(e.shiftKey ? -1 : 1); }
}
/* Ctrl+F opens the bar on the find field — seeded with the selected cell's
   value when the field is empty; F3 / Shift+F3 step from anywhere. */
window.addEventListener('keydown', e => {
    const t = T();
    if (e.key === 'F3' && t && t.loaded && !document.getElementById('dlg')) { e.preventDefault(); findStep(e.shiftKey ? -1 : 1); return; }
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.altKey || e.key.toLowerCase() !== 'f') return;
    if (!t || document.getElementById('dlg') || document.getElementById('cmdk').classList.contains('open')) return;
    e.preventDefault();
    if (!findBarOpen()) toggleSRBar();
    const box = document.getElementById('sr-find');
    if (!box.value && sel && sel.tab === t.id && sel.ar === sel.fr && sel.ac === sel.fc && t.filteredData[sel.fr]) {
        const v = cellStr(t.filteredData[sel.fr].data[sel.fc]).trim();
        if (v && v.length <= 80 && v.indexOf('\n') < 0) { box.value = v; findRefresh(t); }
    }
    box.focus(); box.select();
});
document.getElementById('sr-find').addEventListener('input', findChanged);
