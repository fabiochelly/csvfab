/* ---------------------------------------------------------------
   NOISE WORDS
   A list of words (or phrases) taken out of one column or of every
   column, in the rows shown: "SARL", "Monsieur", "et Fils"… Whole words
   by default, case and accents ignored, and the gap each one leaves is
   tidied (doubled spaces, a comma left dangling). One undo entry.
----------------------------------------------------------------*/

const NOISE_KEY = 'csvfab-noise-words';
const NOISE_PRESETS = [
    ['Legal forms', 'SARL, S.A.R.L., SAS, S.A.S., SASU, SA, S.A., EURL, SCI, SNC, SCOP, SELARL, EIRL, Ltd, LLC, Inc, GmbH'],
    ['Titles', 'Monsieur, Madame, Mademoiselle, M., Mme, Mlle, Mr, Mrs, Ms, Dr'],
    ['French small words', "le, la, les, l', de, du, des, d', et, un, une, au, aux"]
];
let noiseDraft = null;                    // the dialog's state: { words: text, col }

/* Every precomposed letter whose NFD base is a given ASCII letter, so an
   accent-blind pattern can match "Société" from "societe" without
   rewriting the cell (its own spelling must stay byte for byte). */
let accentCls = null;
function accentClasses() {
    if (accentCls) return accentCls;
    accentCls = {};
    for (let cp = 0xC0; cp <= 0x24F; cp++) {
        const ch = String.fromCodePoint(cp), base = removeAccents(ch);
        if (base.length === 1 && base !== ch && /[a-z]/i.test(base)) (accentCls[base] = accentCls[base] || [base]).push(ch);
    }
    return accentCls;
}
const reEsc = s => s.replace(/[.*+?^${}()|[\]\\\/]/g, '\\$&');   // no "-": a u-flag regex refuses "\-" outside a class

/* The list as typed: one per line, or separated by commas / semicolons. */
function noiseWords(text) {
    const seen = new Set();
    return text.split(/[\n,;]+/).map(w => w.trim().replace(/\s+/g, ' ')).filter(w => {
        const k = w.toLocaleLowerCase('fr');
        if (!w || seen.has(k)) return false;
        seen.add(k); return true;
    });
}

/* One regex for the whole list, longest first ("et Cie" before "et").
   Whole words: a boundary only on a side where the word ends in a letter
   or digit, so "M." and "d'" match before what follows them. Hyphens
   count as boundaries: "Saint-Martin-de-Ré" loses its "de" if "de" is
   listed. */
function noiseRegex(words, o) {
    if (!words.length) return null;
    const cls = o.accents ? accentClasses() : null, W = '[\\p{L}\\p{N}_]';
    const part = w => [...w].map(ch => {
        if (/\s/.test(ch)) return '\\s+';
        const set = cls && cls[removeAccents(o.caseless ? ch.toLowerCase() : ch)];   // case-blind: the i flag does the rest
        if (!set) return reEsc(ch) + (cls && /\p{L}/u.test(ch) ? '\\p{M}*' : '');
        return '[' + set.join('') + ']\\p{M}*';
    }).join('');
    const alt = [...words].sort((a, b) => b.length - a.length).map(w => {
        const p = part(w);
        if (!o.whole) return p;
        return (/^[\p{L}\p{N}_]/u.test(w) ? `(?<!${W})` : '') + p + (/[\p{L}\p{N}_]$/u.test(w) ? `(?!${W})` : '');
    });
    return new RegExp('(?:' + alt.join('|') + ')', 'gu' + (o.caseless ? 'i' : ''));
}

/* The gaps a removal leaves: "Dupont SARL, Paris" → "Dupont, Paris",
   "SARL - Dupont" → "Dupont", "Dupont, SARL, Paris" → "Dupont, Paris",
   "(SARL) Dupont" → "Dupont".
   Only around what was removed: the rest of the cell is not touched. */
const NZ = '\u0000', NZ_SEP = '[,;:/|·\\-–—]';
const nzRuns = new RegExp(`${NZ}(?:[ \\t]*${NZ})+`, 'g');
const nzDup = new RegExp(`(${NZ_SEP})([ \\t]*)${NZ}[ \\t]*\\1`, 'g');
const nzPair = new RegExp(`\\([ \\t]*${NZ}[ \\t]*\\)|\\[[ \\t]*${NZ}[ \\t]*\\]|\\{[ \\t]*${NZ}[ \\t]*\\}`, 'g');
const nzHead = new RegExp(`^\\s*${NZ}\\s*(?:${NZ_SEP}[ \\t]*)?`);
const nzTail = new RegExp(`(?:\\s*${NZ_SEP})?\\s*${NZ}\\s*$`);
const nzMid = new RegExp(`([ \\t]*)${NZ}([ \\t]*)`, 'g');
function noiseStrip(v, re, tidy) {
    re.lastIndex = 0;
    if (!re.test(v)) return v;
    if (!tidy || v.includes(NZ)) return v.replace(re, '');
    const s = v.replace(re, NZ).replace(nzRuns, NZ).replace(nzPair, NZ).replace(nzRuns, NZ).replace(nzDup, `$1$2${NZ}`).replace(nzHead, '').replace(nzTail, '');
    return s.replace(nzMid, (m, a, b, off, str) => {
        const prev = str[off - 1], next = str[off + m.length];
        if (prev === undefined || next === undefined) return '';
        if (/[,;:.!?)\]}»]/.test(next)) return b;     // "Dupont SARL, Paris"; a French " ;" keeps its space
        if (/[(\[{«]/.test(prev)) return a;
        return a || b ? ' ' : '';
    });
}

function noiseOpts() {
    const on = id => document.getElementById(id).checked;
    return { caseless: on('nz-case'), accents: on('nz-acc'), whole: on('nz-whole'), tidy: on('nz-tidy') };
}

/* Counts the cells that would change in the rows shown (a few kept as
   examples), or calls each(r, c, old, new) to apply. */
function noiseScan(t, each) {
    const col = document.getElementById('nz-col').value, o = noiseOpts();
    const re = noiseRegex(noiseWords(document.getElementById('nz-words').value), o);
    const out = { cells: 0, rows: 0, examples: [], col, re };
    if (!re) return out;
    const one = col === '' ? -1 : +col;
    visitRows(t, t.filteredData, r => {
        const d = r.data;
        let hit = false;
        for (let c = one < 0 ? 0 : one, end = one < 0 ? d.length : one + 1; c < end; c++) {
            const v = d[c];
            if (v == null || v === '') continue;
            const s = String(v), nv = noiseStrip(s, re, o.tidy);
            if (nv === s) continue;
            out.cells++; hit = true;
            if (each) each(r, c, nv);
            else if (out.examples.length < 8) out.examples.push([c, s, nv]);
        }
        if (hit) out.rows++;
    });
    return out;
}

function openNoise(col) {
    const t = T(); if (!t || !t.loaded) return;
    if (!noiseDraft) { let w = ''; try { w = localStorage.getItem(NOISE_KEY) || ''; } catch (e) { } noiseDraft = { words: w, col: '' }; }
    const rg = selRange(t);
    if (col == null && rg && rg.c0 === rg.c1) col = rg.c0;   // one column selected: that one
    const sel = document.getElementById('nz-col');
    sel.innerHTML = '<option value="">All columns</option>' + t.headers.map((h, i) => `<option value="${i}">${esc(h)}</option>`).join('');
    sel.value = col != null ? String(col) : (noiseDraft.col !== '' && +noiseDraft.col < t.headers.length ? noiseDraft.col : '');
    document.getElementById('nz-words').value = noiseDraft.words;
    document.getElementById('nz-chips').innerHTML = NOISE_PRESETS.map(([l, w], i) =>
        `<span class="dk" title="${esc(w)}" onclick="noisePreset(${i})">+ ${esc(l)}</span>`).join('');
    noiseRefresh();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-noise').style.display = 'block';
    document.getElementById('nz-words').focus();
}
function noisePreset(i) {
    const box = document.getElementById('nz-words'), cur = box.value.replace(/\s+$/, '');
    box.value = (cur ? cur + '\n' : '') + NOISE_PRESETS[i][1];
    noiseRefresh();
}
const noiseRefreshSoon = debounce(() => noiseRefresh(), 250);

function noiseRefresh() {
    const t = T(); if (!t || !noiseDraft) return;
    noiseDraft.words = document.getElementById('nz-words').value;
    noiseDraft.col = document.getElementById('nz-col').value;
    const n = noiseWords(noiseDraft.words).length, scan = noiseScan(t), go = document.getElementById('nz-go');
    const rows = `${fmt(t.filteredData.length)} ${hasFilter(t) ? 'filtered ' : ''}rows`;
    document.getElementById('nz-stats').innerHTML = !n ? 'Type the words to remove, or add a list below.'
        : (scan.cells ? `<b>${fmt(scan.cells)}</b> cell${scan.cells === 1 ? '' : 's'} will change, in ${fmt(scan.rows)} row${scan.rows === 1 ? '' : 's'}` : 'No cell would change')
          + ` — ${fmt(n)} word${n === 1 ? '' : 's'}, in the ${rows}.`;
    const mark = s => { let h = '', at = 0; scan.re.lastIndex = 0;
        for (const m of s.matchAll(scan.re)) { h += esc(s.slice(at, m.index)) + `<mark class="nz">${esc(m[0])}</mark>`; at = m.index + m[0].length; }
        return h + esc(s.slice(at)); };
    document.getElementById('nz-pv').innerHTML = scan.examples.length
        ? `<tr>${scan.col === '' ? '<th style="padding: 4px 8px;">column</th>' : ''}<th style="padding: 4px 8px;">${scan.col === '' ? 'before' : esc(t.headers[+scan.col])}</th><th style="padding: 4px 8px; color: var(--prim);">after</th></tr>`
            + scan.examples.map(([c, o, v]) => `<tr>${scan.col === '' ? `<td class="src">${esc(t.headers[c] ?? '')}</td>` : ''}<td class="src">${mark(o)}</td><td>${v === '' ? '<span class="empty">(empty)</span>' : esc(v)}</td></tr>`).join('')
        : `<tr><td class="empty">${n ? 'Nothing to remove with these settings.' : 'The cells that change will show here.'}</td></tr>`;
    go.disabled = !scan.cells;
}

function applyNoise() {
    const t = T(); if (!t || !noiseDraft) return;
    const ed = rowEdits(), cells = [];
    const scan = noiseScan(t, (r, c, nv) => { ed.set(r, c, nv); if (cells.length < 5000) cells.push([r, c]); });
    try { localStorage.setItem(NOISE_KEY, noiseDraft.words); } catch (e) { }
    closeAllModals();
    if (!scan.cells) return;
    const where = scan.col === '' ? 'every column' : t.headers[+scan.col];
    const what = `noise words removed from ${fmt(scan.cells)} cell${scan.cells === 1 ? '' : 's'} in ${where}`;
    t.modificationsLog.push({ id: '-', col: scan.col === '' ? '---' : t.headers[+scan.col], old: 'noise words', new: `${scan.cells} cells`, what,
        undo: () => ed.undo() });
    updateSaveBtn(); renderTabBar(); render();
    flash(cells);
    setStats(`${t.name} | ${what[0].toUpperCase() + what.slice(1)} — not written yet, use Save.`);
}
