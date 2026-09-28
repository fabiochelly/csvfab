/* ---------------------------------------------------------------
   ENCODING
   Papa decodes a File in 10 MB slices, each on its own, so a multi-byte
   character straddling a slice boundary came out as two U+FFFD — and a
   save wrote them back. The whole file is therefore decoded here, in one
   go, and Papa gets text. Only a file too large for one JS string still
   goes to Papa as a File (with the detected encoding), boundary risk and all.
   Auto detection: a BOM decides; otherwise zero bytes on every other
   position mean UTF-16 without BOM; otherwise the file is UTF-8 if it
   decodes strictly as such — the whole file, not a sample, since a single
   "é" in 1252 can sit on the last line — and Windows-1252 if it does not.
----------------------------------------------------------------*/
const ENCODINGS = [['utf-8', 'UTF-8'], ['windows-1252', 'Windows-1252'], ['iso-8859-1', 'ISO-8859-1 (Latin-1)'],
    ['iso-8859-15', 'ISO-8859-15 (Latin-9, with €)'], ['macintosh', 'Mac Roman (Excel for Mac)'], ['utf-16le', 'UTF-16']];
const ENC_SHORT = { 'iso-8859-1': 'ISO-8859-1', 'iso-8859-15': 'ISO-8859-15', 'macintosh': 'Mac Roman', 'utf-16be': 'UTF-16 BE' };
/* One byte per character: encoded through a table built from the browser's own decoder. */
const SINGLE_BYTE = ['windows-1252', 'iso-8859-1', 'iso-8859-15', 'macintosh'];
const MAX_DECODE = 400 * 1024 * 1024;     // V8 strings stop at ~512 M chars
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

/* Sets t.detectedEnc / t.bom; returns the text, or null when too large to decode here. */
async function decodeTab(t, file) {
    const sn = sniffEncoding(new Uint8Array(await file.slice(0, 4096).arrayBuffer()));
    let enc = t.encoding || sn.enc;
    t.bom = sn.bom && sn.enc === enc;
    if (file.size > MAX_DECODE) { t.detectedEnc = enc; return null; }
    const buf = await file.arrayBuffer();
    let text;
    if (!t.encoding && enc === 'utf-8' && !sn.bom) {
        try { text = new TextDecoder('utf-8', { fatal: true }).decode(buf); }
        catch (e) { enc = 'windows-1252'; }
    }
    if (text == null) text = new TextDecoder(enc).decode(buf);   // strips a matching BOM
    t.detectedEnc = enc;
    t.mojibake = hasMojibake(text);
    return text;
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
    for (const r of rows) for (const v of r.data) test(v);
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
    t.allData = []; t.filteredData = []; t.quoteErrors = 0;
    let headerDone = false;
    const active = () => t.id === activeTabId;

    if (active()) { setStats(`Reading ${t.name}…`); startProgress(); }
    renderTabBar();

    /* Every kind of tab is parsed the same way: bytes as a File, decoded
       here, then handed to the worker as text. A bridge tab buffers its
       bytes rather than letting Papa download them — Papa's worker silently
       never calls back when `download: true` is combined with `worker:
       true` (it has no script path to work from), and dropping the worker
       would parse a large file on the main thread. The raw bytes are a
       fraction of what the parsed rows cost in RAM anyway. */
    let src, text;
    try {
        if (t.path) t.name = (await srvStat(t.path)).name;
        src = await tabFile(t, active() ? setProgress : null); t.size = src.size;
        t.stamp = await diskStamp(t, src);
        if (t.stamp) t.stamp.fp = await fingerprint(src);
        text = await decodeTab(t, src);
    }
    catch (err) {                       // handle revoked, file moved or deleted
        t.loading = false; t.error = err;
        if (active()) { endProgress(); setStats(`Cannot read ${t.name} (${err.message || err})`); }
        settleWaiters(t, null); renderTabBar(); return;
    }

    const total = text != null ? text.length : t.size;
    Papa.parse(text != null ? text : src, {
        worker: true, delimiter: t.delimiter, skipEmptyLines: true,
        chunkSize: 4 << 20, encoding: t.detectedEnc,   // chunkSize counts chars for text, bytes for a File
        chunk: function (results) {
            let rows = results.data;
            t.quoteErrors += results.errors.filter(e => e.type === 'Quotes').length;
            if (!headerDone) {
                headerDone = true;
                /* Auto mode leaves t.delimiter empty: keep what Papa sniffed,
                   and the file's own line ending, so an in-place save rewrites
                   the file in its original shape instead of imposing ";" + LF. */
                t.detectedDelim = results.meta.delimiter || '';
                t.detectedEol = results.meta.linebreak || '\n';
                if (t.headers.length === 0 && rows.length) {   // first read: decide what line 1 is
                    const mode = t.headerMode;
                    const isHeader = mode === 'first' ? true : (mode === 'index' ? false : looksLikeHeader(rows[0], rows.slice(1, 1 + HEADER_SAMPLE)));
                    t.syntheticHeader = !isHeader;
                    t.headers = isHeader ? rows[0].slice() : numberedHeaders(rows[0].length);
                }
                if (!t.syntheticHeader) rows = rows.slice(1);  // on a re-read the decision is already known
                if (active()) { renderHeader(); applyColStyles(); refreshParseOpts(); }
            }
            const start = t.allData.length;
            for (let i = 0; i < rows.length; i++) t.allData.push({ id: start + i + 1, data: rows[i] });
            if (active() && total && results.meta.cursor) setProgress(results.meta.cursor / total);
        },
        complete: function () {
            t.loading = false; t.loaded = true; t.rowCount = t.allData.length; t.lastUsed = Date.now();
            if (!tabs.some(x => x.loading)) endProgress();
            if (active()) {
                convertHeader(t, wantsSynthetic(t, t.headerMode));
                renderHeader();                   // again now that the rows are in: the type icons need them
                refreshParseOpts();
                applyFilters();
                container.scrollTop = t.scrollTop; render();
            }
            renderTabBar();
            settleWaiters(t, t.allData);          // before eviction, which may release this very tab again
            evictIfNeeded();
        },
        error: function (err) {
            t.loading = false; t.error = err;
            if (active()) { endProgress(); setStats(`Cannot read ${t.name} (${err})`); }
            settleWaiters(t, null); renderTabBar();
        }
    });
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
