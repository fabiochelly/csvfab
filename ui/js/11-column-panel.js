/* ---------------------------------------------------------------
   COLUMN PANEL
   The ▾ of a column header: a profile of the column and a filter by
   value (sorting stays on a click of the title). Profile and value list come from one count over
   the rows the other filters let through (this column's own value
   filter excluded), so the numbers follow what is being looked at.
----------------------------------------------------------------*/
const CP_MAX = 300;                       // values listed at once; the search reaches the others
let colPanel = null;                      // { t, col, entries: [[value, count]], excluded: Set, shown: [] }

function closeColPanel() {
    document.getElementById('col-panel').classList.remove('open');
    document.querySelectorAll('.col-menu.open').forEach(b => b.classList.remove('open'));
    colPanel = null;
}

function openColPanel(e, col) {
    e.stopPropagation();
    const t = T(); if (!t || !t.loaded) return;
    if (colPanel && colPanel.col === col) return closeColPanel();
    closeColPanel(); closeDDs();

    /* Counted once per state: reopening the panel on the same rows, filters and column reuses the counts. */
    const key = [col, filterSig(t, col), t.globalQuery, JSON.stringify(t.colFilters)].join('\u0001');
    let counts, scope;
    const c = t.cpCache;
    if (c && c.key === key && sameStamp(c.stamp, dataStamp(t))) ({ counts, scope } = c);
    else {
        const tt = textFilterTest(t) || null, vt = valueFilterTest(t, col);
        counts = new Map(); scope = 0;
        visitRows(t, t.allData, r => {
            if ((tt && !tt(r)) || (vt && !vt(r))) return;
            scope++;
            const v = cellStr(cellOf(r, col));
            counts.set(v, (counts.get(v) || 0) + 1);
        });
        t.cpCache = { key, stamp: dataStamp(t), counts, scope };
    }
    /* By frequency, ties in collation order — past 20 000 distinct values (an ID or e-mail
       column, every count 1) ties keep their order of first appearance: the collator on
       600 k ties cost 1.1 s, and the list only ever shows CP_MAX of them. */
    const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' }), many = counts.size > 20000;
    const entries = [...counts].sort((a, b) => b[1] - a[1] || (many ? 0 : coll.compare(a[0], b[0])));

    /* Profile */
    let empty = 0, distinct = 0; const ty = { n: 0, d: 0, t: 0 };
    for (const [v, n] of entries) {
        const k = cellType(v);
        if (!k) { empty += n; continue; }
        distinct++; ty[k] += n;
    }
    const filled = scope - empty;
    const kind = filled && ty.n / filled >= 0.9 ? 'n' : (filled && ty.d / filled >= 0.9 ? 'd' : 't');
    let range = '';
    if (filled) {
        const key = kind === 'n' ? numKey : kind === 'd' ? dateKey : v => v.length;
        let lo = null, hi = null, sum = 0, cnt = 0;
        for (const [v, n] of entries) {
            if (!cellType(v)) continue;
            const k = key(v.trim()); if (typeof k !== 'number' || isNaN(k)) continue;
            if (!lo || k < lo[0]) lo = [k, v]; if (!hi || k > hi[0]) hi = [k, v];
            sum += k * n; cnt += n;
        }
        const num = x => esc(x.toLocaleString('fr-FR', { maximumFractionDigits: 2 }));
        /* The value may be cut (ellipsis, whole value in the tooltip); what follows it — a
           text value's length — sits outside that box, so it always shows in full. */
        const line = (a, b, full, after) => `<div class="cp-kv"><span>${a}</span><b title="${esc(full || b)}">${esc(b)}</b>${after ? `<i class="cnt">${after}</i>` : ''}</div>`;
        /* Text: the shortest and longest values themselves, with their length. */
        const txt = ([len, v]) => { const x = v.trim(); return [x, x, `(${fmt(len)} char${len === 1 ? '' : 's'})`]; };
        /* The median: the middle of the values in order (the mean of the two middle ones for an
           even count), from the counted values, no second pass; a click selects the first cell
           holding it, or the nearest value. */
        let med = NaN;
        if (lo && kind === 'n') {
            const ks = [];
            for (const [v, n] of entries) { if (!cellType(v)) continue; const k = numKey(v.trim()); if (!isNaN(k)) ks.push([k, n]); }
            ks.sort((a, b) => a[0] - b[0]);
            const at = p => { let s = 0; for (const [k, n] of ks) { s += n; if (s > p) return k; } return NaN; };
            med = cnt % 2 ? at((cnt - 1) / 2) : (at(cnt / 2 - 1) + at(cnt / 2)) / 2;
        }
        if (lo && kind === 'n') range = `<div class="cp-pair">${line('min', lo[1])}${line('max', hi[1])}</div>`
            + `<div class="cp-pair"><div class="cp-kv"><span>sum</span><b>${num(sum)}</b></div><div class="cp-kv"><span>avg</span><b>${num(sum / cnt)}</b></div></div>`
            + `<div class="cp-pair"><div class="cp-kv"><span>med</span><b class="cp-link" onclick="colPanelGo(${col}, ${med})" title="Click: go to it">${num(med)}</b></div></div>`;
        else if (lo && kind === 'd') range = line('min', lo[1]) + line('max', hi[1]);
        else if (lo) range = line('min', ...txt(lo)) + line('max', ...txt(hi));
        if (lo && (kind === 'n' || kind === 'd') && hi[0] > lo[0]) range += histogram(entries, key, lo, hi);
    }
    const total = t.allData.length;
    /* No empty count here: the value list below has it, as its (Empty) entry. */
    const notes = scope !== total ? [`${fmt(scope)} of ${fmt(total)} rows (filtered)`] : [];
    const prof = `<div class="cp-head"><span class="cp-type t-${kind}">${{ n: 'Numbers', d: 'Dates', t: 'Text' }[kind]}</span>`
        + `<span><b>${fmt(distinct)}</b> distinct value${distinct === 1 ? '' : 's'} on <b>${fmt(filled)}</b></span></div>`
        + (notes.length ? `<div class="cp-note">${notes.join(' · ')}</div>` : '')
        + range;

    colPanel = { t, col, entries, maxCount: Math.max(1, ...entries.slice(0, 1).map(e => e[1])), excluded: new Set(t.valFilters[col] || []), shown: [], query: '' };
    const panel = document.getElementById('col-panel');
    panel.innerHTML = `
        <div class="cp-tools">
            <span class="cp-grp">
                <button class="btn btn-outline ico" onclick="closeColPanel(); moveColumn(${col}, ${col - 1})" ${col === 0 ? 'disabled' : ''} title="Move left"><svg class="ic" viewBox="0 0 24 24"><path d="M15 6l-6 6 6 6"/></svg></button>
                <button class="btn btn-outline ico" onclick="closeColPanel(); moveColumn(${col}, ${col + 1})" ${col === t.headers.length - 1 ? 'disabled' : ''} title="Move right"><svg class="ic" viewBox="0 0 24 24"><path d="M9 6l6 6-6 6"/></svg></button>
            </span>
            <span class="cp-grp">
                <button class="btn btn-outline ico" onclick="closeColPanel(); openSplit(${col})" title="Split"><svg class="ic" viewBox="0 0 24 24"><path d="M12 3v18"/><path d="M8 8l-4 4 4 4M4 12h5"/><path d="M16 8l4 4-4 4M20 12h-5"/></svg></button>
                <button class="btn btn-outline ico" onclick="closeColPanel(); openMerge(${col})" title="Merge"><svg class="ic" viewBox="0 0 24 24"><path d="M12 3v18"/><path d="M5 8l4 4-4 4M3 12h6"/><path d="M19 8l-4 4 4 4M21 12h-6"/></svg></button>
            </span>
            <button class="btn btn-outline ico" onclick="closeColPanel(); openConvert(${col})" title="Convert: rewrite its dates, numbers or phones into one format (the values change)"><svg class="ic" viewBox="0 0 24 24"><polyline points="17 2 21 6 17 10"/><path d="M3 12v-2a4 4 0 0 1 4-4h14"/><polyline points="7 22 3 18 7 14"/><path d="M21 12v2a4 4 0 0 1-4 4H3"/></svg></button>
            <span class="cp-grp cp-add">
                <button class="btn btn-outline ico" onclick="this.parentElement.classList.toggle('open')" title="Insert a column">+</button>
                <button class="btn btn-outline ico del" onclick="closeColPanel(); deleteColumn(${col})" title="Delete this column">−</button>
                <span class="cp-add-menu">
                    <span class="dd-item" onclick="closeColPanel(); addColumn(${col - 1})">Insert a column before</span>
                    <span class="dd-item" onclick="closeColPanel(); addColumn(${col})">Insert a column after</span>
                </span>
            </span>
        </div>
        <div class="dd-sep"></div>
        <div class="cp-prof">${prof}</div>
        <div class="dd-sep"></div>
        <div class="cp-srow">
            <input type="text" class="bs-input cp-search" placeholder="Search values…" spellcheck="false" autocomplete="off" oninput="colPanel.query = this.value; renderColValues()">
            ${kind === 'n' ? `<span class="cp-grp">
                <button class="btn btn-outline ico${t.dataBars[col] ? ' on' : ''}" onclick="closeColPanel(); toggleDataBars(${col})" title="Data bars: each value drawn as a bar at the foot of its cell"><svg class="ic" viewBox="0 0 24 24"><path d="M4 3v18"/><rect x="4" y="5.5" width="11" height="3.5" rx="1" fill="currentColor" stroke="none"/><rect x="4" y="10.25" width="16" height="3.5" rx="1" fill="currentColor" stroke="none" opacity=".75"/><rect x="4" y="15" width="7" height="3.5" rx="1" fill="currentColor" stroke="none" opacity=".5"/></svg></button>
                <button class="btn btn-outline ico${t.heat[col] ? ' on' : ''}" onclick="closeColPanel(); toggleHeat(${col})" title="Colour scale: each cell tinted by its value"><svg class="ic" viewBox="0 0 24 24"><rect x="2.5" y="5" width="5" height="14" rx="1.5" fill="currentColor" fill-opacity=".12" stroke-width="1.6"/><rect x="9.5" y="5" width="5" height="14" rx="1.5" fill="currentColor" fill-opacity=".45" stroke-width="1.6"/><rect x="16.5" y="5" width="5" height="14" rx="1.5" fill="currentColor" stroke-width="1.6"/></svg></button>
            </span>` : ''}
        </div>
        <div class="cp-list" id="cp-list"></div>
        <div class="cp-actions">
            <button class="btn btn-outline" onclick="applyValueFilter(true)">Clear</button>
            <button class="btn" onclick="applyValueFilter(false)">Apply</button>
        </div>`;
    renderColValues();

    const btn = e.currentTarget, th = btn.closest('th'), rc = th.getBoundingClientRect();
    btn.classList.add('open');
    panel.classList.add('open');
    const w = panel.offsetWidth, h = panel.offsetHeight;
    panel.style.left = Math.max(8, Math.min(rc.right - w, window.innerWidth - w - 8)) + 'px';
    panel.style.top = Math.max(8, Math.min(rc.bottom + 4, window.innerHeight - h - 8)) + 'px';
    panel.querySelector('.cp-search').focus();
}

/* Distribution of a numeric or date column: 28 bins between min and max. Each bar tells, in a
   tooltip shown at once (CSS, not a <title> and its delay), the values it holds — its smallest
   and largest as written, so dates need no formatting back from their keys — and how many. */
function histogram(entries, key, lo, hi) {
    const B = 28, bins = new Array(B).fill(0), first = new Array(B), last = new Array(B), span = hi[0] - lo[0];
    for (const [v, n] of entries) {
        if (!cellType(v)) continue;
        const k = key(v.trim()); if (typeof k !== 'number' || isNaN(k)) continue;
        const b = Math.min(B - 1, Math.floor((k - lo[0]) / span * B));
        bins[b] += n;
        if (!first[b] || k < first[b][0]) first[b] = [k, v.trim()];
        if (!last[b] || k > last[b][0]) last[b] = [k, v.trim()];
    }
    const max = Math.max(...bins);
    return '<div class="cp-hist">' + bins.map((c, i) => {
        const h = c ? Math.max(4, c / max * 100) : 0;
        /* Anchored left, centred or right by the bar's place, so the tooltip stays in the panel. */
        const pos = i < B / 3 ? 'left:0' : i >= B * 2 / 3 ? 'right:0' : `left:${((i + .5) / B * 100).toFixed(1)}%;transform:translateX(-50%)`;
        const vals = !c ? '' : first[i][1] === last[i][1] ? esc(first[i][1]) : `${esc(first[i][1])} – ${esc(last[i][1])}`;
        return `<div class="hb${c ? '' : ' z'}"><i style="height:${h.toFixed(1)}%"></i>`
            + (c ? `<span class="tip" style="${pos}">${vals} <span>· ${fmt(c)} value${c === 1 ? '' : 's'}</span></span>` : '') + '</div>';
    }).join('') + '</div>';
}
/* The first row shown whose value in column col is closest to x (the median: with an even
   count it may be no cell's value), selected and scrolled to the middle of the view. */
function colPanelGo(col, x) {
    const t = T(); if (!t || !t.loaded) return;
    closeColPanel();
    let best = -1, gap = Infinity;
    const rows = t.filteredData;
    for (let i = 0; i < rows.length; i++) {
        const v = cellStr(cellOf(rows[i], col)).trim(); if (!v || !isNumericLike(v)) continue;
        const d = Math.abs(numKey(v) - x);
        if (d < gap) { gap = d; best = i; if (!d) break; }
    }
    if (best < 0) return setStats('No row shown holds that value.');
    if (t.hiddenCols.has(col)) { t.hiddenCols.delete(col); applyColStyles(); render(); }
    container.vTop = Math.max(0, thead.offsetHeight + best * ROW_H - container.clientHeight / 2);
    setSel(t, best, col, best, col); revealCell(best, col);
}

function renderColValues() {
    const p = colPanel; if (!p) return;
    const q = removeAccents(p.query.trim().toLowerCase());
    const match = q ? p.entries.filter(([v]) => removeAccents(v.toLowerCase()).includes(q)) : p.entries;
    p.matching = match;
    p.shown = match.slice(0, CP_MAX);
    const allOn = match.every(([v]) => !p.excluded.has(v));
    document.getElementById('cp-list').innerHTML = !match.length ? '<div class="cp-more">No value matches.</div>'
        : `<label class="cp-val"><input type="checkbox" ${allOn ? 'checked' : ''} onchange="colPanelAll(this.checked)"><span class="v">${q ? '(Select all matches)' : '(Select all)'}</span><span class="n">${fmt(match.length)}</span></label>`
        + p.shown.map(([v, n], i) => `<label class="cp-val" style="--f: ${(n / p.maxCount * 100).toFixed(1)}%"><input type="checkbox" ${p.excluded.has(v) ? '' : 'checked'} onchange="colPanelToggle(${i}, this.checked)">`
            + `<span class="v${v.trim() ? '' : ' empty'}" title="${esc(v)}">${v.trim() ? esc(v) : (v ? '(Blank)' : '(Empty)')}</span><span class="n">${fmt(n)}</span></label>`).join('')
        + (match.length > CP_MAX ? `<div class="cp-more">${fmt(match.length - CP_MAX)} more values — search to reach them.</div>` : '');
}
function colPanelToggle(i, on) {
    const v = colPanel.shown[i][0];
    if (on) colPanel.excluded.delete(v); else colPanel.excluded.add(v);
    renderColValues();
}
function colPanelAll(on) {
    for (const [v] of colPanel.matching) { if (on) colPanel.excluded.delete(v); else colPanel.excluded.add(v); }
    renderColValues();
}
function applyValueFilter(clear) {
    const p = colPanel; if (!p) return;
    const { t, col } = p;
    if (clear || !p.excluded.size) delete t.valFilters[col];
    else t.valFilters[col] = new Set(p.excluded);
    closeColPanel();
    if (t !== T()) return;
    renderHeader(); applyColStyles(); applyFilters();
}
/* composedPath, not target.closest: ticking a value re-renders the list, so the target may be detached by now. */
document.addEventListener('click', (e) => {
    if (colPanel && !e.composedPath().some(n => n.id === 'col-panel' || (n.classList && n.classList.contains('col-menu')))) closeColPanel();
});
window.addEventListener('resize', () => { if (colPanel) closeColPanel(); });
