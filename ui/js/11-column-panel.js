/* ---------------------------------------------------------------
   COLUMN PANEL
   The ▾ of a column header: a profile of the column and a filter by
   value (sorting stays on a click of the title). Profile and value list come from one count over
   the rows the other filters let through (this column's own value
   filter excluded), so the numbers follow what is being looked at.
----------------------------------------------------------------*/
const CARET_SVG = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6l4 4 4-4"/></svg>';
const FUNNEL_SVG = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M3 4h18l-7 8.5V19l-4 2v-8.5z"/></svg>';
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

    const tt = textFilterTest(t) || null, vt = valueFilterTest(t, col);
    const counts = new Map();
    let scope = 0;
    for (const r of t.allData) {
        if ((tt && !tt(r)) || (vt && !vt(r))) continue;
        scope++;
        const v = cellStr(r.data[col]);
        counts.set(v, (counts.get(v) || 0) + 1);
    }
    const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' });
    const entries = [...counts].sort((a, b) => b[1] - a[1] || coll.compare(a[0], b[0]));

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
        const line = (a, b, full) => `<div class="cp-kv"><span>${a}</span><b title="${esc(full || b)}">${esc(b)}</b></div>`;
        /* Text: the shortest and longest values themselves, cut at 24 characters (whole value in the tooltip). */
        const txt = ([len, v]) => { const x = v.trim(); return [(x.length > 24 ? x.slice(0, 24) + '…' : x) + ` (${fmt(len)} char${len === 1 ? '' : 's'})`, x]; };
        if (lo && kind === 'n') range = `<div class="cp-pair">${line('min', lo[1])}${line('max', hi[1])}</div>`
            + `<div class="cp-pair"><div class="cp-kv"><span>sum</span><b>${num(sum)}</b></div><div class="cp-kv"><span>avg</span><b>${num(sum / cnt)}</b></div></div>`;
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
            <button class="btn btn-outline" onclick="closeColPanel(); moveColumn(${col}, ${col - 1})" ${col === 0 ? 'disabled' : ''} title="Move left">◀</button>
            <button class="btn btn-outline" onclick="closeColPanel(); moveColumn(${col}, ${col + 1})" ${col === t.headers.length - 1 ? 'disabled' : ''} title="Move right">▶</button>
            <button class="btn btn-outline" onclick="closeColPanel(); openSplit(${col})">Split…</button>
            <button class="btn btn-outline" onclick="closeColPanel(); openMerge(${col})">Merge…</button>
            ${kind === 'n' ? `<button class="btn btn-outline${t.dataBars[col] ? ' on' : ''}" onclick="closeColPanel(); toggleDataBars(${col})" title="Draw each value as a bar in its cell">Bars</button>` : ''}
        </div>
        <div class="dd-sep"></div>
        <div class="cp-prof">${prof}</div>
        <div class="dd-sep"></div>
        <input type="text" class="bs-input cp-search" placeholder="Search values…" spellcheck="false" oninput="colPanel.query = this.value; renderColValues()">
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

/* Distribution of a numeric or date column: 28 bins between min and max. */
function histogram(entries, key, lo, hi) {
    const B = 28, bins = new Array(B).fill(0), span = hi[0] - lo[0];
    for (const [v, n] of entries) {
        if (!cellType(v)) continue;
        const k = key(v.trim()); if (typeof k !== 'number' || isNaN(k)) continue;
        bins[Math.min(B - 1, Math.floor((k - lo[0]) / span * B))] += n;
    }
    const max = Math.max(...bins), W = 100 / B;
    return `<svg class="cp-hist" viewBox="0 0 100 40" preserveAspectRatio="none">`
        + bins.map((c, i) => c ? `<rect x="${(i * W + .3).toFixed(2)}" y="${(40 - Math.max(1.5, c / max * 38)).toFixed(2)}" width="${(W - .6).toFixed(2)}" height="${Math.max(1.5, c / max * 38).toFixed(2)}" rx=".6"><title>${fmt(c)} value${c === 1 ? '' : 's'}</title></rect>` : '').join('')
        + '</svg>';
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
