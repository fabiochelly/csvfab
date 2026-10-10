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
/* What a write uses: the tab's encoding unless overridden, and whether a BOM leads. */
function writeEnc(t, override) {
    const enc = override || currentEnc(t);
    /* Written as read: the BOM if the file had one, none if it had none (a UTF-16
       file without a BOM stays without). Converted to UTF-16: with one, the only
       way most readers tell UTF-16 from a single-byte encoding. */
    return { enc, bom: enc === currentEnc(t) ? !!t.bom : enc.startsWith('utf-16') };
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
        scanAhead(t.size || 0);           // the scan's workers start while the bytes travel (45-…)
        /* A big bridge file is scanned as it arrives, its slices leaving as their bytes come in (45-…, scanStream). */
        const how = { encoding: t.encoding, delimiter: t.delimiter, lines: t.delimiter === '\n' };
        const feed = t.path && t.size >= SCAN_PAR.min ? byteFeed() : null, stream = feed && scanStream(feed, how);
        let src = null;
        try { src = await tabBytes(t, active() ? setProgress : null, feed); }
        finally { if (feed) feed.end(src && feed.u8 && src.bytes === feed.u8.buffer); }
        t.size = src.bytes.byteLength;
        if (t.path) t.name = src.stat ? src.stat.name : (await srvStat(t.path)).name;
        t.stamp = await diskStamp(t, src.file, src.stat);
        /* The fingerprint is not taken here, ahead of the scan: crypto.subtle.digest hashes on the
           calling thread before it returns (Chromium, measured: 1.7 ms for 2 MB, 15 ms for 24 MB),
           so "hashed meanwhile" was a wait. afterShown() takes it from the bytes kept — except for a
           big file, whose fingerprint keys the reopen cache, looked up before any scan: of its first,
           middle and last MB only (~2 ms), which past 64 MB is the fingerprint itself. Up to 64 MB it
           was the whole file here, ~25 ms before the scan; the full one now waits for afterShown(). */
        const len = src.bytes.byteLength, fp = t.stamp && len >= IDX_MIN ? await fingerprint(src.bytes, true) : null;
        if (fp && len > 64 << 20) t.stamp.fp = fp;
        const phases = { transcode: 'Converting', scan: 'Reading' };
        let shown = '';
        /* A big file gets its scan kept for the next opening (21-…, REOPEN CACHE), keyed by what
           would make it wrong: the bytes (size, date, fingerprint) and how they are read. */
        const cache = fp
            ? { key: [len, t.stamp.mtime, fp, t.encoding || '', t.delimiter || '', t.delimiter === '\n' ? 'lines' : ''].join('|'), name: t.name } : null;
        base = await loadBase(src.bytes, { ...how, cache, stream }, (p, ph) => {
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
    t.mojibake = false;                   // afterShown()
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
        t.scrollTop = top;
        if (top) { container.vTop = top; render(); }   // a first opening is at the top, as applyFilters() just drew it
    }
    renderTabBar();
    afterShown(t, base);
    settleWaiters(t, t.allData);          // before eviction, which may release this very tab again
    evictIfNeeded();
}

/* What drawing the file does not need, once it is drawn: the fingerprint of the bytes as read
   (checkDisk's proof that a same-size file with another date is still ours) and the
   garbled-accents hint (the first 16 MB are plenty to notice it). After the next frame; a hidden
   window has none, hence the timer too. Both run in a worker (checkOff): on the page they held it
   right after the first frame — crypto.subtle.digest hashes on the calling thread, and the hint
   decodes up to 16 MB: 6 ms for 2.4 MB, ~30 ms for 24 MB, measured from a fresh page. The page only
   copies the bytes. checkDisk() runs it at once if a save comes first (t.afterShown), then waits
   for t.fpWait. */
function afterShown(t, base) {
    let done = false;
    const go = t.afterShown = () => {
        if (done) return;
        done = true;
        if (t.afterShown === go) t.afterShown = null;
        if (t.base !== base) return;      // read again, or released, meanwhile
        const stamp = t.stamp, fb = fileBytes(base), mlen = base.n > 0 ? Math.min(base.u8.length, 16 << 20) : 0;
        const hashIt = !!stamp && !stamp.fp && fb.length <= 64 << 20;   // past 64 MB it was taken before the scan
        const log = t.modificationsLog, n0 = log.length, last0 = log[n0 - 1];
        const apply = (moji, fp) => {
            /* An edit since (a clean-up repairing the accents) would make the hint stale: kept as it is. */
            if (t.base === base && log === t.modificationsLog && log.length === n0 && log[n0 - 1] === last0) { t.mojibake = moji; if (T() === t) updateMojiChip(t); }
            if (fp && t.stamp === stamp) stamp.fp = fp;
        };
        const here = () => {              // no worker, or it failed: as it was done before
            apply(mlen > 0 && hasMojibake(base.dec.decode(base.u8.subarray(0, mlen))), null);
            if (stamp && !stamp.fp) return fingerprint(fb).then(fp => apply(t.mojibake, fp), () => { });
        };
        if (typeof Worker !== 'function') { t.fpWait = here() || null; return; }
        /* One copy when the scanned bytes are the file's own (not UTF-16): the hint reads its part. */
        const one = hashIt && !base.orig, at = one ? base.u8.byteOffset - fb.byteOffset : 0;
        const hash = hashIt ? fb.slice().buffer : null, moji = one ? null : base.u8.slice(0, mlen).buffer;
        t.fpWait = checkOff({ hash, moji, from: at, to: at + mlen, enc: base.transcoded ? 'utf-8' : base.enc }, [hash, moji].filter(Boolean))
            .then(r => r.error ? here() : apply(!!r.moji, r.fp || null), () => here());
    };
    requestAnimationFrame(() => setTimeout(go, 0));
    setTimeout(go, 250);
}
/* The worker of afterShown(), kept once started; its answers matched to their request by id. */
let checkW = null, checkSeq = 0;
const checkWait = new Map();
function checkOff(msg, transfer) {
    if (!checkW) {
        const src = `let mojiRe = null; const sbTables = {}; const utf8Strict = new TextDecoder('utf-8', { fatal: true });\n${sbTable}\n${mojibakeRe}\n${unmoji}\n${hasMojibake}\n(${checkWorker})()`;
        checkW = new Worker(URL.createObjectURL(new Blob([src], { type: 'text/javascript' })));
        checkW.onmessage = e => { const f = checkWait.get(e.data.id); checkWait.delete(e.data.id); if (f) f.res(e.data); };
        checkW.onerror = () => { for (const f of checkWait.values()) f.rej(new Error('check worker')); checkWait.clear(); checkW.terminate(); checkW = null; };
    }
    return new Promise((res, rej) => { const id = ++checkSeq; checkWait.set(id, { res, rej }); checkW.postMessage({ ...msg, id }, transfer); });
}
function checkWorker() {
    onmessage = async e => {
        const { id, hash, moji, from, to, enc } = e.data, out = { id };
        try {
            const m = moji ? new Uint8Array(moji) : new Uint8Array(hash);
            if (to > from) out.moji = hasMojibake(new TextDecoder(enc, { ignoreBOM: true }).decode(m.subarray(from, to)));
            if (hash) out.fp = [...new Uint8Array(await crypto.subtle.digest('SHA-256', hash))].map(b => b.toString(16).padStart(2, '0')).join('');
        } catch (err) { out.error = String(err && err.message || err); }
        postMessage(out);
    };
}

/* --- Progress helpers --- */
/* progressAt: when the bar was started (0: ended) — a long operation awaiting the disk or the
   server is under way, which the column charts' background pass waits for (47-…). */
let progressAt = 0;
function startProgress() {
    progressAt = performance.now();
    progressBar.style.transition = 'none'; progressBar.style.width = '0%'; progressBar.style.opacity = '1';
    setTimeout(() => progressBar.style.transition = 'width 0.1s linear, opacity 0.5s ease-out', 10);
}
function setProgress(ratio) { progressBar.style.width = Math.min(ratio * 100, 100) + '%'; }
function endProgress() { progressAt = 0; progressBar.style.width = '100%'; setTimeout(() => { progressBar.style.opacity = '0'; }, 500); }
function setStats(txt) { document.getElementById('stats').innerText = txt; }
function setStatsHtml(html) { document.getElementById('stats').innerHTML = html; }

updateSaveBtn(); renderTabBar();
