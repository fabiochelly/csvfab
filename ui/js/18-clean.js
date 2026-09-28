/* ---------------------------------------------------------------
   CLEAN UP
   One pass over every cell (filtered or not) and the header line:
   garbled accents, invisible characters, odd spaces, line breaks,
   then empty rows and columns — one undo entry for all of it.
----------------------------------------------------------------*/

/* Mojibake: UTF-8 bytes that were read as Windows-1252 somewhere upstream
   ("Ã©" for é, "â€™" for ’). The regex finds a lead byte followed by its
   continuation bytes, as 1252 characters; each match is decoded back on
   its own, so a cell mixing sound and garbled accents is repaired too.
   A decoded character is kept only in the ranges mojibake of Western text
   lands in: "voilà »" with its no-break space is also à + A0 + BB, a
   valid UTF-8 sequence for a Samaritan letter. */
let mojiRe = null;
function mojibakeRe() {
    if (mojiRe) return mojiRe;
    const { map } = sbTable('windows-1252');
    const cls = (lo, hi) => '[' + [...map].filter(([, b]) => b >= lo && b <= hi).map(([c]) => '\\u' + c.charCodeAt(0).toString(16).padStart(4, '0')).join('') + ']';
    const C = cls(0x80, 0xBF);
    return mojiRe = new RegExp(`${cls(0xC2, 0xDF)}${C}|${cls(0xE0, 0xEF)}${C}{2}|${cls(0xF0, 0xF4)}${C}{3}`, 'gu');
}
const utf8Strict = new TextDecoder('utf-8', { fatal: true });
function unmoji(m) {
    const map = sbTable('windows-1252').map;
    let ch; try { ch = utf8Strict.decode(Uint8Array.from(m, c => map.get(c))); } catch (e) { return m; }
    const cp = ch.codePointAt(0);
    return (cp >= 0xA0 && cp <= 0x24F) || (cp >= 0x2000 && cp <= 0x214F) || cp >= 0x1F000 ? ch : m;
}
/* Up to three passes: text garbled twice ("ÃƒÂ©") comes back one layer at a time. */
function fixMojibake(s) {
    for (let pass = 0; pass < 3; pass++) {
        const out = s.replace(mojibakeRe(), unmoji);
        if (out === s) break;
        s = out;
    }
    return s;
}
/* On the decoded text, before parsing: one scan, stopped at the first
   match that really decodes. */
function hasMojibake(text) {
    if (!text) return false;
    let tries = 0;
    for (const m of text.matchAll(mojibakeRe())) { if (unmoji(m[0]) !== m[0]) return true; if (++tries > 1000) break; }
    return false;
}

/* In the order they are applied: mojibake before the spaces, since "à"
   garbled is "Ã" + a no-break space. `test` counts the cells an option
   would touch, without rewriting them. */
const CLEAN_OPS = [
    { id: 'moji', label: 'Repair garbled accents (Ã© → é, â€™ → ’)', on: true, fn: fixMojibake, test: v => fixMojibake(v) !== v },
    { id: 'invis', label: 'Remove invisible characters (zero-width, direction marks, stray BOM)', on: true, fn: v => v.replace(/\p{Cf}/gu, ''), test: v => /\p{Cf}/u.test(v) },
    { id: 'nbsp', label: 'No-break and narrow spaces → plain spaces', on: true, fn: v => v.replace(/[   ]/g, ' '), test: v => /[   ]/.test(v) },
    { id: 'breaks', label: 'Line breaks inside cells → spaces', on: false, fn: v => v.replace(/[ \t]*[\r\n]+[ \t]*/g, ' '), test: v => /[\r\n]/.test(v) },
    { id: 'collapse', label: 'Collapse repeated spaces', on: false, fn: v => v.replace(/ {2,}/g, ' '), test: v => / {2}/.test(v) },
    { id: 'trim', label: 'Trim spaces at both ends of each cell', on: true, fn: v => v.trim(), test: v => /^\s|\s$/.test(v) },
    { id: 'rows', label: 'Delete empty rows', on: true },
    { id: 'cols', label: 'Delete empty columns (header aside)', on: false }
];
const BLANK = /^[\s\p{Cf}]*$/u;
const isBlank = v => v == null || v === '' || BLANK.test(v);
let cleanOn = null, cleanScan = null;

/* Counts once per opening: which cells each option would change, which
   rows and columns hold nothing but spaces or invisible characters. */
function scanClean(t) {
    const cellOps = CLEAN_OPS.filter(o => o.fn), counts = {};
    cellOps.forEach(o => counts[o.id] = 0);
    const filled = new Array(t.headers.length).fill(false);
    let emptyRows = 0;
    const visit = v => { if (v == null || v === '') return; const s = String(v); for (const o of cellOps) if (o.test(s)) counts[o.id]++; };
    for (const r of t.allData) {
        let blank = true;
        for (let c = 0; c < r.data.length; c++) {
            const v = r.data[c];
            visit(v);
            if (!isBlank(v)) { blank = false; if (c < filled.length) filled[c] = true; }
        }
        if (blank) emptyRows++;
    }
    if (!t.syntheticHeader) t.headers.forEach(visit);
    counts.rows = emptyRows;
    const emptyCols = filled.map((f, c) => f ? -1 : c).filter(c => c >= 0);
    counts.cols = emptyCols.length;
    return { counts, emptyCols };
}

function openClean() {
    const t = T(); if (!t || !t.loaded) return;
    if (!cleanOn) cleanOn = new Set(CLEAN_OPS.filter(o => o.on).map(o => o.id));
    cleanScan = scanClean(t);
    const { counts, emptyCols } = cleanScan;
    document.getElementById('clean-opts').innerHTML = CLEAN_OPS.map(o => {
        const n = counts[o.id], unit = o.id === 'rows' ? 'row' : o.id === 'cols' ? 'column' : 'cell';
        const tip = o.id === 'cols' && n ? ` title="${esc(emptyCols.map(c => t.headers[c]).join(', '))}"` : '';
        return `<label class="col-label${n ? '' : ' none'}"${tip}><span><input type="checkbox" value="${o.id}"${cleanOn.has(o.id) ? ' checked' : ''} onchange="cleanToggle(this)"> ${esc(o.label)}</span>`
            + `<span class="k">${n ? `${fmt(n)} ${unit}${n === 1 ? '' : 's'}` : '—'}</span></label>`;
    }).join('');
    updateCleanNote();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-clean').style.display = 'block';
}
function cleanToggle(box) { box.checked ? cleanOn.add(box.value) : cleanOn.delete(box.value); updateCleanNote(); }
function updateCleanNote() {
    const any = CLEAN_OPS.some(o => cleanOn.has(o.id) && cleanScan.counts[o.id]);
    document.getElementById('clean-note').textContent = any
        ? 'Counts are per option: a cell can need several. Nothing is written until Save.'
        : 'Nothing to clean with these options.';
    document.getElementById('clean-go').disabled = !any;
}

function applyClean() {
    const t = T(); if (!t || !cleanScan) return;
    closeAllModals();
    const fns = CLEAN_OPS.filter(o => o.fn && cleanOn.has(o.id)).map(o => o.fn);
    const clean = v => fns.reduce((s, f) => f(s), v);
    const prevAll = t.allData, prevHeaders = t.headers, prevMoji = t.mojibake, view = viewSnap(t), changes = [];
    if (fns.length) for (const r of t.allData) {
        const d = r.data;
        for (let c = 0; c < d.length; c++) {
            const old = d[c];
            if (old == null || old === '') continue;
            const nv = clean(String(old));
            if (nv !== old) { changes.push([d, c, old]); d[c] = nv; }
        }
    }
    if (fns.length && !t.syntheticHeader) t.headers = t.headers.map(h => clean(h));
    const heads = t.headers.filter((h, c) => h !== prevHeaders[c]).length;

    let rows = t.allData, dropped = 0;
    if (cleanOn.has('rows')) { rows = rows.filter(r => !r.data.every(isBlank)); dropped = t.allData.length - rows.length; }
    /* The columns found blank at opening: blank means spaces and invisible
       characters only, so no cell option can have filled one since. */
    const gone = new Set(cleanOn.has('cols') ? cleanScan.emptyCols : []);
    let prevData = null;
    if (gone.size) {
        prevData = prevAll.map(r => r.data);
        for (const r of rows) r.data = r.data.filter((v, c) => !gone.has(c));
        t.headers = t.headers.filter((h, c) => !gone.has(c));
        const map = []; let k = 0;
        for (let c = 0; c < prevHeaders.length; c++) map[c] = gone.has(c) ? -1 : k++;
        remapCols(t, c => c < map.length ? map[c] : c - gone.size);
    }
    const cells = changes.length + heads;
    if (!cells && !dropped && !gone.size) { setStats(`${t.name} | Nothing to clean.`); return; }

    t.allData = rows;
    rows.forEach((r, i) => r.id = i + 1);
    t.rowCount = rows.length;
    if (cleanOn.has('moji')) t.mojibake = false;
    const n = (k, w) => `${fmt(k)} ${w}${k === 1 ? '' : 's'}`;
    const parts = [cells && n(cells, 'cell') + ' cleaned', dropped && n(dropped, 'empty row') + ' deleted', gone.size && n(gone.size, 'empty column') + ' deleted'].filter(Boolean);
    const what = parts.join(', ');
    t.modificationsLog.push({ id: '-', col: '---', old: 'clean-up', new: what, what, undo: t => {
        t.allData = prevAll;
        if (prevData) prevAll.forEach((r, i) => r.data = prevData[i]);
        for (let k = changes.length - 1; k >= 0; k--) { const [d, c, old] = changes[k]; d[c] = old; }
        t.headers = prevHeaders; viewRestore(t, view); t.mojibake = prevMoji;
    } });
    cleanScan = null;
    updateSaveBtn(); renderHeader(); applyColStyles(); applyFilters(); renderTabBar(); updateStats();
    setStats(`${t.name} | ${what} — not written yet, use Save.`);
}

function updateMojiChip(t) {
    const chip = document.getElementById('moji-chip');
    chip.style.display = t && t.loaded && t.mojibake ? '' : 'none';
}
