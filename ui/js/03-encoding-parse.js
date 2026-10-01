/* ---------------------------------------------------------------
   ENCODING
   The file is kept as bytes and decoded one record at a time (see the
   row store, 21-…), always with one decoder per record — never slices
   of the file decoded on their own, which split a multi-byte character
   in two U+FFFD that a save then wrote back.
   Auto detection: a BOM decides; otherwise zero bytes on every other
   position mean UTF-16 without BOM; otherwise the file is UTF-8 if it
   decodes strictly as such — the whole file, not a sample, since a single
   "é" in 1252 can sit on the last line — and Windows-1252 if it does not
   (checked by the scan worker, off the main thread).
----------------------------------------------------------------*/
const ENCODINGS = [['utf-8', 'UTF-8'], ['windows-1252', 'Windows-1252'], ['iso-8859-1', 'ISO-8859-1 (Latin-1)'],
    ['iso-8859-15', 'ISO-8859-15 (Latin-9, with €)'], ['macintosh', 'Mac Roman (Excel for Mac)'], ['utf-16le', 'UTF-16']];
const ENC_SHORT = { 'iso-8859-1': 'ISO-8859-1', 'iso-8859-15': 'ISO-8859-15', 'macintosh': 'Mac Roman', 'utf-16be': 'UTF-16 BE' };
/* One byte per character: encoded through a table built from the browser's own decoder. */
const SINGLE_BYTE = ['windows-1252', 'iso-8859-1', 'iso-8859-15', 'macintosh'];
function encName(e) { return ENC_SHORT[e] || (ENCODINGS.find(x => x[0] === e) || [e, e || '?'])[1]; }
function currentEnc(t) { return t.encoding || t.detectedEnc || 'utf-8'; }

function sniffEncoding(b) {
    if (b[0] === 0xEF && b[1] === 0xBB && b[2] === 0xBF) return { enc: 'utf-8', bom: true };
    if (b[0] === 0xFF && b[1] === 0xFE) return { enc: 'utf-16le', bom: true };
    if (b[0] === 0xFE && b[1] === 0xFF) return { enc: 'utf-16be', bom: true };
    const n = Math.min(b.length, 4096) & ~1;
    let zEven = 0, zOdd = 0;
    for (let i = 0; i < n; i++) if (b[i] === 0) { if (i & 1) zOdd++; else zEven++; }
    if (n && zOdd > n / 8 && zEven < n / 64) return { enc: 'utf-16le', bom: false };
    if (n && zEven > n / 8 && zOdd < n / 64) return { enc: 'utf-16be', bom: false };
    return { enc: 'utf-8', bom: false };
}

/* Reverse table of a single-byte encoding, built by decoding its 256 bytes
   with TextDecoder — so it matches exactly what reading the file gave.
   (WHATWG decodes "iso-8859-1" as windows-1252, a superset: writing it back
   through the same table restores the original bytes.) `bad` matches any
   character the encoding cannot hold. */
const sbTables = {};
function sbTable(enc) {
    if (sbTables[enc]) return sbTables[enc];
    const chars = new TextDecoder(enc).decode(Uint8Array.from({ length: 256 }, (_, i) => i));
    const map = new Map();
    for (let i = 0; i < chars.length; i++) if (!map.has(chars[i])) map.set(chars[i], i);
    const cls = [...map.keys()].map(c => '\\u' + c.charCodeAt(0).toString(16).padStart(4, '0')).join('');
    return sbTables[enc] = { map, bad: new RegExp('[^' + cls + ']', 'u') };
}

/* A chunk encoder for streamCSV: strings stay strings for UTF-8 (Blob and
   createWritable encode those themselves), the others become bytes. */
function encoderFor(enc) {
    if (enc === 'utf-16le' || enc === 'utf-16be') {
        const le = enc === 'utf-16le';
        return c => {
            const u = new Uint8Array(c.length * 2);
            for (let i = 0; i < c.length; i++) {
                const k = c.charCodeAt(i);
                u[2 * i + (le ? 0 : 1)] = k & 0xFF; u[2 * i + (le ? 1 : 0)] = k >> 8;
            }
            return u;
        };
    }
    if (SINGLE_BYTE.includes(enc)) {
        const { map } = sbTable(enc);
        return c => {
            const u = new Uint8Array(c.length);
            for (let i = 0; i < c.length; i++) u[i] = map.get(c[i]) ?? 0x3F;   // '?' — checked beforehand, see confirmEncodable()
            return u;
        };
    }
    return c => c;
}
function bomFor(enc) {
    return enc === 'utf-8' ? '﻿' : enc === 'utf-16le' ? new Uint8Array([0xFF, 0xFE]) : enc === 'utf-16be' ? new Uint8Array([0xFE, 0xFF]) : null;
}
/* What a write uses: the tab's encoding unless overridden. UTF-16 always
   carries its BOM; UTF-8 keeps one only if the source file had it. */
function writeEnc(t, override) {
    const enc = override || currentEnc(t);
    return { enc, bom: enc.startsWith('utf-16') || (enc === 'utf-8' && t.bom && currentEnc(t) === 'utf-8') };
}

/* A single-byte encoding holds ~250 characters: say what would be lost before writing '?'. */
async function confirmEncodable(t, rows, enc) {
    const { bad } = sbTable(enc);
    let n = 0, first = '';
    const test = v => { if (v && bad.test(v)) { if (!n) first = (String(v).match(bad) || [''])[0]; n++; } };
    t.headers.forEach(test);
    const same = t.base && !t.base.transcoded && t.base.enc === enc;   // a record read in this encoding holds nothing it cannot write
    visitRows(t, rows, r => { if (!(same && !r.d)) for (const v of r.data) test(v); });
    return !n || await uiConfirm(`${fmt(n)} cells contain characters ${encName(enc)} cannot represent (e.g. "${first}").\n\nThey will be written as "?".`, { ok: 'Write them as "?"' });
}

/* The rows of any tab, active or not, read back from disk if RAM eviction
   released them (null if the file cannot be read). The array is handed
   over before evictIfNeeded() runs: unloadTab() replaces t.allData rather
   than emptying it, so the caller's reference stays whole even if the tab
   is released again at once. */
function tabRows(t) {
    if (t.loaded) return Promise.resolve(t.allData);
    return new Promise(res => { (t.waiters = t.waiters || []).push(res); if (!t.loading) parseTab(t); });
}
function settleWaiters(t, rows) { if (t.waiters) t.waiters.splice(0).forEach(f => f(rows)); }

async function parseTab(t) {
    t.loading = true; t.error = null; t.loaded = false;
    t.allData = []; t.filteredData = []; t.quoteErrors = 0; t.base = null; t.redoStack = [];
    const active = () => t.id === activeTabId, top = t.scrollTop;   // where the tab was (evicted): the scroll events of the emptied grid zero t.scrollTop meanwhile
    t.rowMark = null;

    if (active()) { setStats(`Reading ${t.name}…`); startProgress(); }
    renderTabBar();

    /* Every kind of tab is read the same way: its bytes as a File, handed
       to the scan worker, which finds the records; rows decode themselves
       on demand. A bridge tab buffers its bytes through tabFile() rather
       than streaming them anywhere: the raw bytes are what we keep. */
    let base;
    try {
        if (t.path) t.name = (await srvStat(t.path)).name;
        const src = await tabBytes(t, active() ? setProgress : null); t.size = src.bytes.byteLength;
        t.stamp = await diskStamp(t, src.file);
        if (t.stamp) t.stamp.fp = await fingerprint(src.bytes);   // before the bytes go to the worker
        const phases = { transcode: 'Converting', scan: 'Reading' };
        let shown = '';
        base = await loadBase(src.bytes, { encoding: t.encoding, delimiter: t.delimiter, lines: t.delimiter === '\n' }, (p, ph) => {
            if (!active()) return;
            if (ph !== shown) { shown = ph; setStats(`${phases[ph] || 'Reading'} ${t.name}…`); startProgress(); }
            setProgress(p);
        });
    }
    catch (err) {                       // handle revoked, file moved or deleted
        t.loading = false; t.error = err;
        if (active()) { endProgress(); setStats(`Cannot read ${t.name} (${err.message || err})`); }
        settleWaiters(t, null); renderTabBar(); return;
    }

    t.base = base;
    t.detectedEnc = base.enc; t.bom = base.bom;
    /* Auto mode leaves t.delimiter empty: keep what was sniffed, and the
       file's own line ending, so an in-place save rewrites the file in its
       original shape instead of imposing ";" + LF. */
    t.detectedDelim = base.delim; t.detectedEol = base.eol;
    t.quoteErrors = base.qerr;
    /* The garbled-accents hint: the first 16 MB are plenty to notice it. */
    t.mojibake = base.n > 0 && hasMojibake(base.dec.decode(base.u8.subarray(0, Math.min(base.u8.length, 16 << 20))));
    /* Raw text (33-…): one column, titled by the language, every line a row — line 1 included. */
    t.lang = base.lines ? langOf(t.name) : null;
    if (base.lines) { t.syntheticHeader = true; t.headers = [t.lang.label]; }
    else if (t.headers.length === 0 && base.n) {    // first read: decide what line 1 is
        const mode = t.headerMode, row0 = recordFields(base, 0);
        const sample = []; for (let b = 1; b < Math.min(base.n, 1 + HEADER_SAMPLE); b++) sample.push(recordFields(base, b));
        const isHeader = mode === 'first' ? true : (mode === 'index' ? false : looksLikeHeader(row0, sample));
        t.syntheticHeader = !isHeader;
        t.headers = isHeader ? row0.slice() : numberedHeaders(row0.length);
    }
    const skip = t.syntheticHeader || !base.n ? 0 : 1;   // on a re-read the decision is already known
    t.headerSrc = skip ? 0 : null;

    /* One small object per row, in slices so a 20-million-row file keeps the page alive. */
    const Row = base.Row, n = base.n - skip, rows = new Array(n);
    for (let i = 0; i < n; i += 1 << 20) {
        const e = Math.min(n, i + (1 << 20));
        for (let k = i; k < e; k++) { const r = new Row(k + skip, k + skip, null); r.id = k + 1; rows[k] = r; }
        if (e < n) { if (active()) setProgress(e / n); await new Promise(r => setTimeout(r, 0)); }
    }
    t.allData = rows;
    t.loading = false; t.loaded = true; t.rowCount = n; t.lastUsed = Date.now();
    t.colSrc = t.headers.map((_, i) => i);
    if (!tabs.some(x => x.loading)) endProgress();
    if (active()) {
        if (!base.lines) convertHeader(t, wantsSynthetic(t, t.headerMode));
        renderHeader(); applyColStyles(); refreshParseOpts();
        applyFilters();                   // zeroes t.scrollTop: back to where the tab was, now that the extent is known
        t.scrollTop = top; container.scrollTop = top; render();
    }
    renderTabBar();
    settleWaiters(t, t.allData);          // before eviction, which may release this very tab again
    evictIfNeeded();
}

/* --- Progress helpers --- */
function startProgress() {
    progressBar.style.transition = 'none'; progressBar.style.width = '0%'; progressBar.style.opacity = '1';
    setTimeout(() => progressBar.style.transition = 'width 0.1s linear, opacity 0.5s ease-out', 10);
}
function setProgress(ratio) { progressBar.style.width = Math.min(ratio * 100, 100) + '%'; }
function endProgress() { progressBar.style.width = '100%'; setTimeout(() => { progressBar.style.opacity = '0'; }, 500); }
function setStats(txt) { document.getElementById('stats').innerText = txt; }
function setStatsHtml(html) { document.getElementById('stats').innerHTML = html; }

updateSaveBtn(); renderTabBar();
