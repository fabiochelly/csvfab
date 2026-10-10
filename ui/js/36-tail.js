/* ---------------------------------------------------------------
   LIVE TAIL (tail -f) — raw text files only (33-…, a log)
   "Follow" in the status bar (or the palette) checks the file every
   TAIL_MS: what was appended since is read alone (the bridge's
   /api/file?from=, a handle's File.slice), its complete lines added to
   the row store in place (baseAppend: the bytes and the record offsets
   grow, nothing is scanned again) and as rows at the end of t.allData —
   not edits: the tab stays clean, Save has nothing to write. An
   unfinished last line waits for its line break. The view follows the
   end when it was at the bottom; the filters apply to the new lines.
   Paused while edits are pending (the rows would mix with them), and the
   file re-read once they are saved or discarded; re-read too when it
   shrinks (truncated, rotated) or did not end with a line break. Only
   the active tab is followed; another catches up when activated.
----------------------------------------------------------------*/
const TAIL_MS = 1000;
function toggleTail(e) {
    if (e && e.stopPropagation) e.stopPropagation();
    const t = T(); if (!t || !t.loaded) return;
    if (!t.lang) { uiAlert('Follow works on raw text files.\n\nRead the file as Raw text first (the delimiter menu of the status bar).'); return; }
    if (!t.path && !t.handle) { uiAlert('This tab is a read-only copy.\n\nOpen the file itself to follow it.'); return; }
    if (t.base && t.base.transcoded) { uiAlert('A UTF-16 file cannot be followed.'); return; }
    t.tail = !t.tail; t.tailPaused = ''; t.tailDirty = false;
    if (t.tail) { container.vTop = container.vHeight; tailStart(); }
    refreshParseOpts();
    setStats(`${t.name} | ${t.tail ? 'Following the end of the file: new lines appear as they are written.' : 'No longer following the file.'}`);
}
function tailStart() { if (!tailStart.timer) tailStart.timer = setInterval(tailTick, TAIL_MS); }
async function tailTick() {
    if (!tabs.some(t => t.tail)) { clearInterval(tailStart.timer); tailStart.timer = 0; return; }
    const t = T();
    if (!t || !t.tail || t.tailBusy || !t.loaded || t.loading || !t.base) return;
    t.tailBusy = true;
    try { await tailStep(t); }
    catch (err) { if (t.tailPaused !== 'error') { t.tailPaused = 'error'; refreshParseOpts(); setStats(`${t.name} | Follow: ${err.message || err}`); } }
    finally { t.tailBusy = false; }
}
async function tailStep(t) {
    const B = t.base, bomLen = B.bom && B.enc === 'utf-8' ? 3 : 0, have = bomLen + B.u8.length;
    if (isDirty(t)) {                                     // edits pending: nothing added under them
        t.tailDirty = true;
        if (t.tailPaused !== 'edits') { t.tailPaused = 'edits'; refreshParseOpts(); }
        return;
    }
    if (t.tailDirty) { t.tailDirty = false; return tailReload(t); }   // saved or discarded: the bytes held may no longer be the file's
    let size, mtime, file = null;
    if (t.path) { const st = await srvStat(t.path); size = st.size; mtime = st.mtime; }
    else { file = await t.handle.getFile(); size = file.size; mtime = file.lastModified / 1000; }
    if (t.tailPaused) { t.tailPaused = ''; refreshParseOpts(); }
    if (size === have) return;
    const NL = B.eol === '\r' ? 13 : 10, last = B.u8.length ? B.u8[B.u8.length - 1] : NL;
    if (size < have || (last !== 10 && last !== 13)) return tailReload(t);   // truncated or rotated; or the last line was unfinished
    let bytes;
    if (t.path) {
        const r = await srvFetch(srvFileUrl(t.path) + '&from=' + have);
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        bytes = new Uint8Array(await r.arrayBuffer());
    } else bytes = new Uint8Array(await file.slice(have).arrayBuffer());
    const cut = bytes.lastIndexOf(NL); if (cut < 0) return;               // no complete line yet
    const n0 = B.n, k = baseAppend(B, bytes.subarray(0, cut + 1), NL);
    if (k == null) return tailReload(t);
    t.stamp = { size: have + cut + 1, mtime, fp: await tailFingerprint(B, bomLen) };   // what this tab holds now
    if (!k) return;
    const Row = B.Row, id0 = t.allData.length, rows = new Array(k);
    for (let j = 0; j < k; j++) { const r = new Row(n0 + j, n0 + j, null); r.id = id0 + j + 1; rows[j] = r; }
    t.allData = t.allData.concat(rows); t.rowCount = t.allData.length;      // a new array: an undo closure may hold the old one
    const bottom = container.vTop + container.clientHeight >= container.vHeight - 2 * ROW_H, st = container.vTop;
    const keep = keepSel; keepSel = true;
    try { applyFilters(); } finally { keepSel = keep; }                       // the filters apply to the new lines too
    container.vTop = bottom ? container.vHeight : st;
    render(); renderTabBar();
}
function tailReload(t) {
    const follow = container.vTop + container.clientHeight >= container.vHeight - 2 * ROW_H;
    reloadKeepingView(t);
    if (follow) tabRows(t).then(() => { if (T() === t) { container.vTop = container.vHeight; render(); } });
}
/* The fingerprint the file now has (as fingerprint() computes it, 14-…), from the bytes held — no
   re-read: the BOM put back in front, the first, middle and last MB past 64 MB. */
async function tailFingerprint(B, bomLen) {
    const u8 = B.u8, n = bomLen + u8.length, M = 1 << 20;
    const at = (a, b) => {                                // bytes a…b of the file, from the BOM and the bytes held
        const out = new Uint8Array(b - a);
        for (let i = a; i < Math.min(b, bomLen); i++) out[i - a] = [0xEF, 0xBB, 0xBF][i];
        const s = Math.max(a, bomLen);
        if (s < b) out.set(u8.subarray(s - bomLen, b - bomLen), s - a);
        return out;
    };
    let part;
    if (n <= 64 * M) part = bomLen ? at(0, n) : u8;
    else {
        const mid = Math.floor(n / 2) - M / 2;
        part = new Uint8Array(3 * M); part.set(at(0, M), 0); part.set(at(mid, mid + M), M); part.set(at(n - M, n), 2 * M);
    }
    const d = await crypto.subtle.digest('SHA-256', part);
    return [...new Uint8Array(d)].map(b => b.toString(16).padStart(2, '0')).join('');
}

/* Complete lines appended to a raw text file's row store (21-…), in place: the bytes and the record
   offsets grow inside buffers kept with room to spare, the characters' offsets follow for UTF-8.
   Returns how many records were added, or null when it cannot (not a text file, offsets past the
   32-bit range) — the caller then re-reads the file. */
function baseAppend(B, chunk, NL) {
    if (!B.lines || B.transcoded) return null;
    const old = B.u8.length, len = old + chunk.length, n = B.n;
    if (len > 0xFFFFFFF0 && !(B.starts instanceof Float64Array)) return null;
    let k = 0; for (let i = 0; i < chunk.length; i++) if (chunk[i] === NL) k++;
    if (!k) return 0;
    const grow = (cur, cap, size, Ctor, copy) => {         // a view of `size` items, in a buffer with room past it
        if (cap && cap.length >= size) return [cap.subarray(0, size), cap];
        const nb = new Ctor(Math.ceil(size * 1.25) + 1024); nb.set(copy); return [nb.subarray(0, size), nb];
    };
    [B.u8, B.u8cap] = grow(B.u8, B.u8cap && B.u8cap.buffer === B.u8.buffer ? B.u8cap : null, len, Uint8Array, B.u8);
    B.u8.set(chunk, old);
    const prevStarts = B.starts;
    [B.starts, B.startsCap] = grow(B.starts, B.startsCap && B.startsCap.buffer === prevStarts.buffer ? B.startsCap : null, n + 1 + k, prevStarts.constructor, prevStarts);
    const S = B.starts;
    let j = n, from = 0;
    const ascii = B.enc !== 'utf-8' || chunk.every(b => b < 128);
    if (B.enc === 'utf-8' && !B.chars && !ascii) {         // the first non-ASCII character: offsets in characters from now on
        const c = new prevStarts.constructor(prevStarts.length); c.set(prevStarts); B.chars = c; B.charsCap = null;
    }
    if (B.chars) { const pc = B.chars; [B.chars, B.charsCap] = grow(pc, B.charsCap && B.charsCap.buffer === pc.buffer ? B.charsCap : null, n + 1 + k, pc.constructor, pc); }
    for (let i = 0; i < chunk.length; i++) {
        if (chunk[i] !== NL) continue;
        S[j + 1] = old + i + 1;
        if (B.chars) B.chars[j + 1] = B.chars[j] + (ascii ? i + 1 - from : B.dec.decode(chunk.subarray(from, i + 1)).length);
        j++; from = i + 1;
    }
    B.n = n + k;
    B.blk = null; B.lastB = -2; B.lower = null;           // the last block and the filters' lowered text had the old end
    return k;
}
