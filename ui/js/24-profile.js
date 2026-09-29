/* ---------------------------------------------------------------
   FILE PROFILE
   Every column at a glance: kind, filled and empty cells, distinct
   values, min / max (or shortest / longest text), sum and average —
   the column panel's profile, for all columns in one table. Rows are
   visited in slices with a yield between them, so a million rows do
   not freeze the window; past PROFILE_SAMPLE rows an evenly spaced
   sample is read instead and the counts are shown as percentages.
----------------------------------------------------------------*/
const PROFILE_SAMPLE = 200000, PROFILE_DISTINCT = 20000;
let profile = null;      // { t, stamp, cols, sampled, rows }

async function openProfile() {
    const t = T(); if (!t || !t.loaded) return;
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-profile').style.display = 'block';
    document.getElementById('pf-sum').innerHTML = profileSummary(t);
    if (profile && profile.t === t && sameStamp(profile.stamp, dataStamp(t))) { renderProfile(); return; }
    const list = document.getElementById('pf-list');
    list.innerHTML = '<tr><td class="muted">Profiling…</td></tr>';
    const res = await profileTab(t, p => { list.innerHTML = `<tr><td class="muted">Profiling… ${Math.round(p * 100)} %</td></tr>`; });
    if (T() !== t || !res) return;
    profile = { t, stamp: dataStamp(t), ...res };
    if (document.getElementById('modal-profile').style.display === 'block') renderProfile();
}

function profileSummary(t) {
    const irr = t.allData.reduce((n, r) => n + (r.len !== t.headers.length ? 1 : 0), 0);
    const parts = [`<b>${fmt(t.allData.length)}</b> rows`, `<b>${fmt(t.headers.length)}</b> columns`, fmtBytes(t.size),
        esc(encName(currentEnc(t))) + (t.bom ? ' BOM' : ''), { ';': 'semicolon', ',': 'comma', '\t': 'tab', '|': 'pipe' }[currentDelim(t)] || esc(currentDelim(t)),
        t.detectedEol === '\r\n' ? 'CRLF' : t.detectedEol === '\r' ? 'CR' : 'LF'];
    if (t.syntheticHeader) parts.push('no header line');
    if (irr) parts.push(`<span class="warn">${fmt(irr)} irregular row${irr === 1 ? '' : 's'}</span>`);
    if (t.quoteErrors) parts.push(`<span class="warn">${fmt(t.quoteErrors)} quote error${t.quoteErrors === 1 ? '' : 's'}</span>`);
    if (t.mojibake) parts.push('<span class="warn">garbled accents</span>');
    return parts.join(' · ');
}

/* One pass over the rows (or a stride sample of them), per column: type
   counts, empties, distinct values (capped), min / max, sum. */
async function profileTab(t, onProgress) {
    const n = t.headers.length, all = t.allData;
    const sampled = all.length > PROFILE_SAMPLE, step = sampled ? all.length / PROFILE_SAMPLE : 1;
    const rows = sampled ? Array.from({ length: PROFILE_SAMPLE }, (_, i) => all[Math.floor(i * step)]) : all;
    const cols = t.headers.map(() => ({ filled: 0, empty: 0, n: 0, d: 0, t: 0, set: new Set(), more: false, lo: null, hi: null, sum: 0, cnt: 0, short: null, long: null }));
    const stamp = dataStamp(t);
    let last = performance.now();
    for (let i = 0; i < rows.length; i += 20000) {
        visitRows(t, rows.slice(i, i + 20000), r => {
            const d = r.data;
            for (let c = 0; c < n; c++) {
                const s = cols[c], v = cellStr(d[c]).trim();
                if (!v) { s.empty++; continue; }
                s.filled++;
                if (s.set.size < PROFILE_DISTINCT) s.set.add(v); else if (!s.set.has(v)) s.more = true;
                const k = isNumericLike(v) ? 'n' : isDateLike(v) ? 'd' : 't';
                s[k]++;
                if (k === 'n') { const x = numKey(v); if (!isNaN(x)) { s.sum += x; s.cnt++; if (s.lo === null || x < s.lo[0]) s.lo = [x, v]; if (s.hi === null || x > s.hi[0]) s.hi = [x, v]; } }
                else if (k === 'd') { const x = dateKey(v); if (!isNaN(x)) { if (s.lo === null || x < s.lo[0]) s.lo = [x, v]; if (s.hi === null || x > s.hi[0]) s.hi = [x, v]; } }
                if (s.short === null || v.length < s.short.length) s.short = v;
                if (s.long === null || v.length > s.long.length) s.long = v;
            }
        });
        if (performance.now() - last > 30) {
            onProgress(Math.min(1, (i + 20000) / rows.length));
            await new Promise(r => setTimeout(r, 0));
            last = performance.now();
            if (T() !== t || !sameStamp(stamp, dataStamp(t))) return null;   // the rows changed meanwhile
        }
    }
    return { cols: cols.map(s => ({ ...s, distinct: s.set.size, set: undefined })), sampled, rows: rows.length };
}

function profileKind(s) { return !s.filled ? '' : s.n / s.filled >= 0.9 ? 'n' : s.d / s.filled >= 0.9 ? 'd' : 't'; }
const PF_KIND = { n: ['Numbers', 't-n'], d: ['Dates', 't-d'], t: ['Text', 't-t'], '': ['Empty', 't-t'] };
const cutTxt = (v, n = 28) => v.length > n ? v.slice(0, n - 1) + '…' : v;

function renderProfile() {
    const { t, cols, sampled, rows } = profile;
    const num = x => x.toLocaleString('fr-FR', { maximumFractionDigits: 2 });
    const pct = k => `${(k / rows * 100).toFixed(k && k < rows * 0.01 ? 1 : 0)} %`;
    const count = k => sampled ? pct(k) : fmt(k);
    let html = `<tr class="sec"><th>Column</th><th>Kind</th><th>Filled</th><th>Empty</th><th>Distinct</th><th>Min</th><th>Max</th><th>Sum · avg</th></tr>`;
    t.headers.forEach((h, c) => {
        const s = cols[c], k = profileKind(s), [label, cls] = PF_KIND[k];
        const cell = v => v == null ? '<td class="empty">—</td>' : `<td title="${esc(v)}">${esc(cutTxt(v))}</td>`;
        let lo, hi, sum = '';
        if (k === 'n' && s.lo) { lo = s.lo[1]; hi = s.hi[1]; sum = s.cnt ? `${num(s.sum)} · ${num(s.sum / s.cnt)}` : ''; }
        else if (k === 'd' && s.lo) { lo = s.lo[1]; hi = s.hi[1]; }
        else if (s.filled) { lo = s.short ? `${s.short} (${fmt(s.short.length)})` : null; hi = s.long ? `${s.long} (${fmt(s.long.length)})` : null; }
        const dist = !s.filled ? '' : (s.more ? `${fmt(s.distinct)}+` : fmt(s.distinct)) + (s.filled && s.distinct === s.filled && !s.more && !sampled ? ' <span class="muted" title="Every filled cell is different: a key?">unique</span>' : '');
        html += `<tr class="go" onclick="closeAllModals(); openColPanelFor(${c})" title="Open this column's panel">`
            + `<td class="pf-name"><span class="pf-bar" style="--f: ${(s.filled / (rows || 1) * 100).toFixed(1)}%"></span>${esc(h)}${t.hiddenCols.has(c) ? ' <span class="muted">(hidden)</span>' : ''}</td>`
            + `<td><span class="cp-type ${cls}">${label}</span></td><td class="k">${count(s.filled)}</td><td class="k${s.empty ? '' : ' muted'}">${count(s.empty)}</td><td class="k">${dist}</td>`
            + cell(lo) + cell(hi) + `<td class="k">${sum}</td></tr>`;
    });
    document.getElementById('pf-list').innerHTML = html;
    document.getElementById('pf-note').textContent = sampled ? `Read from an evenly spaced sample of ${fmt(rows)} rows: filled and empty as percentages, distinct values within the sample.` : '';
}

/* The table as tab-separated text, for a note or a message. */
async function copyProfile() {
    if (!profile) return;
    const { t, cols, sampled, rows } = profile;
    const lines = [['Column', 'Kind', 'Filled', 'Empty', 'Distinct', 'Min', 'Max', 'Sum', 'Average'].join('\t')];
    t.headers.forEach((h, c) => {
        const s = cols[c], k = profileKind(s);
        const lo = (k === 'n' || k === 'd') && s.lo ? s.lo[1] : s.short || '', hi = (k === 'n' || k === 'd') && s.hi ? s.hi[1] : s.long || '';
        lines.push([h, PF_KIND[k][0], s.filled, s.empty, (s.more ? '≥' : '') + s.distinct, lo, hi, k === 'n' && s.cnt ? s.sum : '', k === 'n' && s.cnt ? s.sum / s.cnt : ''].join('\t'));
    });
    if (sampled) lines.push(`(sample of ${rows} rows)`);
    try { await navigator.clipboard.writeText(lines.join('\n')); doneMsg(`${t.name} | Profile copied — ${fmt(t.headers.length)} columns.`); }
    catch (e) { uiAlert('The clipboard is not available here.'); }
}
