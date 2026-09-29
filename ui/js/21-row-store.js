/* ---------------------------------------------------------------
   ROW STORE
   A file is kept as its own bytes plus the offset of every record, and
   each row is a small object that decodes its cells only when they are
   read: parsing a file into one JS string per cell cost about five times
   its size in V8's heap, which is capped near 4 GB whatever the machine
   has — a 2 GB file could not open. ArrayBuffers live outside that cap.

   t.base = { u8, starts, n, width, enc, delim, eol, cmap, … }: the bytes
   as read (UTF-16 is transcoded to UTF-8 on the way in), records i =
   u8[starts[i], starts[i+1]) minus the line break. Records are found by
   csvScanWorker(), off the main thread.

   A row is a t.base.Row: { id, src, b, d }. b is its record in the base
   (-1: a row created here), src its line in the file on disk (Review
   changes; renumbered after a save while b stays), d null until an edit
   gives the row its own array. r.data is a getter: d, or the record
   decoded — through a small cache, and SHARED: never mutate what r.data
   returns. Every write goes through rowEdits() (or r.data = a new array),
   which replaces d rather than mutating it, so an undo only has to put
   the previous d back — null included.

   base.cmap maps the current columns to the record's fields (null =
   identity, -1 = a new, empty column): adding, deleting or moving a
   column re-maps the base instead of rewriting every row.
----------------------------------------------------------------*/

/* The record scanner, run as a worker (its source is this function).
   In: the file's bytes (transferred), the encoding, whether auto
   detection still has to check UTF-8, the delimiter and line-break byte.
   Out: the bytes (transcoded to UTF-8 for UTF-16), the start of every
   record, the fields of the first one, the records with another count,
   quote errors. Quotes follow Papa Parse: a field is quoted only if its
   first character is a quote; "" inside is a quote; a closing quote must
   be followed (spaces aside) by a delimiter or a line break, otherwise
   it is kept as a character and counted as an error. */
function csvScanWorker() {
    self.onmessage = e => {
        const { buf, enc, validate, delim, nl, bom } = e.data;
        const post = (m, tr) => self.postMessage(m, tr || []);
        try {
            let u8 = new Uint8Array(buf), outEnc = enc, lastPost = 0;
            const progress = (p, phase) => { const now = Date.now(); if (now - lastPost > 100) { lastPost = now; post({ progress: p, phase }); } };
            const S = 32 << 20;
            if (outEnc === 'utf-16le' || outEnc === 'utf-16be') {
                /* UTF-16 → UTF-8 once: the scanner and the row decoder then only
                   know byte encodings. Each UTF-16 unit takes at most 3 bytes. */
                const dec = new TextDecoder(outEnc), te = new TextEncoder(), out = new Uint8Array(Math.ceil(u8.length * 1.5) + 8);
                let w = 0;
                for (let p = 0; p < u8.length; p += S) {
                    const s = dec.decode(u8.subarray(p, p + S), { stream: p + S < u8.length });
                    w += te.encodeInto(s, out.subarray(w)).written;
                    progress(p / u8.length, 'transcode');
                }
                u8 = out.subarray(0, w);
            } else if (bom) u8 = u8.subarray(bom);
            const len = u8.length, Q = 34, D = delim, NL = nl;
            const Big = len > 0xFFFFFFF0 ? Float64Array : Uint32Array;
            let starts = new Big(Math.max(1024, Math.min(len / 32, 1 << 24) | 0)), n = 0;
            /* For UTF-8, each record's offset in characters (UTF-16 units) too, so the
               page can slice a decoded block without counting bytes again: adj =
               characters − bytes, over the bytes before i. Each multi-byte sequence
               met on the way is checked as the strict decoder would (overlongs,
               surrogates, past U+10FFFF, cut short): auto detection falls back to
               Windows-1252 on the first bad one, with no separate pass. */
            const wantChars = outEnc === 'utf-8' || outEnc.startsWith('utf-16');
            let chars = wantChars ? new Big(starts.length) : null, adj = 0, bad = false;
            const seq = k => {                 // length of the valid sequence at k, or -1
                const x = u8[k], y = u8[k + 1];
                if (x < 0xC2 || x > 0xF4) return -1;
                if (x < 0xE0) return (y & 0xC0) === 0x80 ? 2 : -1;
                if (x < 0xF0) {
                    if (x === 0xE0 ? (y < 0xA0 || y > 0xBF) : x === 0xED ? (y < 0x80 || y > 0x9F) : (y & 0xC0) !== 0x80) return -1;
                    return (u8[k + 2] & 0xC0) === 0x80 ? 3 : -1;
                }
                if (x === 0xF0 ? (y < 0x90 || y > 0xBF) : x === 0xF4 ? (y < 0x80 || y > 0x8F) : (y & 0xC0) !== 0x80) return -1;
                return (u8[k + 2] & 0xC0) === 0x80 && (u8[k + 3] & 0xC0) === 0x80 ? 4 : -1;
            };
            const walk = (a, b) => {           // a quoted record's bytes, jumped over by the field search
                for (let k = a; k < b; k++) if (u8[k] >= 0x80) { const w = seq(k); if (w < 0) bad = true; else { adj -= w === 2 ? 1 : 2; k += w - 1; } }
            };
            let odd = new Uint32Array(1024), no = 0;
            let width = -1, qerr = 0, i = 0, nn = -2, nd = -2, nq = -2;
            while (i < len) {
                const rs = i, adjAt = adj;
                let fields = 1;
                if (nn !== -1 && nn < i) nn = u8.indexOf(NL, i);
                if (nq !== -1 && nq < i) nq = u8.indexOf(Q, i);
                const eol = nn < 0 ? len : nn;
                if (nq < 0 || nq >= eol) {        // no quote on this line, the usual case: count the delimiters, done
                    if (chars) for (let k = i; k < eol; k++) {
                        const x = u8[k];
                        if (x === D) fields++;
                        else if (x >= 0x80) {
                            if (x >= 0xC2 && x < 0xE0 && (u8[k + 1] & 0xC0) === 0x80) { adj--; k++; }   // é, ç…: the common case, inline
                            else { const w = seq(k); if (w < 0) bad = true; else { adj -= w === 2 ? 1 : 2; k += w - 1; } }
                        }
                    }
                    else for (let k = i; k < eol; k++) if (u8[k] === D) fields++;
                    i = eol;
                } else for (;;) {
                    if (u8[i] === Q) {
                        let j = i + 1;
                        for (;;) {
                            const k = u8.indexOf(Q, j);
                            if (k < 0) { i = len; qerr++; break; }          // never closed: the rest of the file
                            if (u8[k + 1] === Q) { j = k + 2; continue; }
                            let m = k + 1;
                            while (m < len && (u8[m] === 32 || (u8[m] === 9 && D !== 9))) m++;
                            const b = u8[m];
                            if (m >= len || b === D || b === NL || (NL === 10 && b === 13 && u8[m + 1] === 10)) { i = m; break; }
                            qerr++; j = k + 1;                               // a stray quote: kept, search on
                        }
                    } else {
                        if (nn !== -1 && nn < i) nn = u8.indexOf(NL, i);
                        if (nd !== -1 && nd < i) nd = u8.indexOf(D, i);
                        i = nd >= 0 && (nn < 0 || nd < nn) ? nd : (nn < 0 ? len : nn);
                    }
                    if (i < len && u8[i] === D) { fields++; i++; if (i >= len) break; continue; }
                    break;
                }
                const e = i;
                if (chars && (nq >= 0 && nq < eol)) walk(rs, e);
                if (i < len) i += (u8[i] === 13 && NL === 10 && u8[i + 1] === 10) ? 2 : 1;
                let z = e; if (NL === 10 && z > rs && u8[z - 1] === 13) z--;
                if (z === rs) continue;   // an empty line is no record (as Papa's skipEmptyLines); it stays in the previous record's span
                if (n + 2 > starts.length) {
                    const g = new Big(starts.length * 2); g.set(starts); starts = g;
                    if (chars) { const h = new Big(starts.length); h.set(chars); chars = h; }
                }
                if (chars) chars[n] = rs + adjAt;
                starts[n++] = rs;
                if (width < 0) width = fields;
                else if (fields !== width) {
                    if (no + 2 > odd.length) { const g = new Uint32Array(odd.length * 2); g.set(odd); odd = g; }
                    odd[no++] = n - 1; odd[no++] = fields;
                }
                if ((n & 0xFFFF) === 0) progress(i / len, 'scan');
            }
            starts[n] = len;
            starts = starts.slice(0, n + 1); odd = odd.slice(0, no);
            if (validate && bad) { outEnc = 'windows-1252'; chars = null; }       // not UTF-8: one byte, one character
            if (chars) { chars[n] = len + adj; chars = adj ? chars.slice(0, n + 1) : null; }   // adj 0: ASCII throughout, characters = bytes
            post({ done: true, buf: u8.buffer, off: u8.byteOffset, len, enc: outEnc, starts, chars, n, width: Math.max(width, 0), odd, qerr },
                [u8.buffer, starts.buffer, odd.buffer].concat(chars ? [chars.buffer] : []));
        } catch (err) { post({ error: String(err && err.message || err) }); }
    };
}
let scanWorkerUrl = null;
function runScan(msg, onProgress) {
    if (!scanWorkerUrl) scanWorkerUrl = URL.createObjectURL(new Blob([`(${csvScanWorker.toString()})()`], { type: 'text/javascript' }));
    return new Promise((res, rej) => {
        const w = new Worker(scanWorkerUrl);
        w.onmessage = e => {
            const m = e.data;
            if (m.progress != null) { if (onProgress) onProgress(m.progress, m.phase); return; }
            w.terminate();
            if (m.error) rej(new Error(m.error)); else res(m);
        };
        w.onerror = e => { w.terminate(); rej(new Error(e.message || 'scan failed')); };
        w.postMessage(msg, [msg.buf]);
    });
}

/* Delimiter and line break, guessed by Papa on the first 64 KB (or taken
   from the tab), before the scan that needs them. */
function sniffFormat(u8, enc, bom, delimiter) {
    const head = u8.subarray(bom, bom + (64 << 10));
    const text = new TextDecoder(enc.startsWith('utf-16') ? enc : enc === 'utf-8' ? 'utf-8' : enc).decode(head);
    const r = Papa.parse(text, { preview: 50, delimiter: delimiter || '', skipEmptyLines: true });   // an empty last line would sink the guess
    return { delim: delimiter || r.meta.delimiter || ',', eol: r.meta.linebreak || '\n' };
}

/* Bytes → base. The ArrayBuffer is handed over to the worker (not copied)
   and comes back as the base's bytes: the caller must not use it after. */
async function loadBase(ab, o, onProgress) {
    const u8 = new Uint8Array(ab);
    const sn = sniffEncoding(u8.subarray(0, 4096));
    const enc = o.encoding || sn.enc;
    const bom = sn.bom && sn.enc === enc ? (enc === 'utf-8' ? 3 : 2) : 0;
    const { delim, eol } = o.delim ? { delim: o.delim, eol: o.eol || '\n' } : sniffFormat(u8, enc, bom, o.delimiter);
    const m = await runScan({ buf: ab, enc, validate: !o.encoding && enc === 'utf-8' && !sn.bom, delim: delim.charCodeAt(0), nl: eol === '\r' ? 13 : 10, bom }, onProgress);
    const base = {
        u8: new Uint8Array(m.buf, m.off, m.len), starts: m.starts, chars: m.chars, n: m.n, width: m.width, qerr: m.qerr,
        enc: m.enc, bom: bom > 0, delim, eol, transcoded: m.enc.startsWith('utf-16'),
        cmap: null, odd: new Map(), irr: null,
        keys: new Int32Array(1024).fill(-1), vals: new Array(1024), blk: null, lastB: -2
    };
    base.dec = new TextDecoder(base.transcoded ? 'utf-8' : base.enc, { ignoreBOM: true });   // a U+FEFF inside a record is data
    if (m.odd.length) {
        base.irr = new Uint8Array(m.n);
        for (let k = 0; k < m.odd.length; k += 2) { base.odd.set(m.odd[k], m.odd[k + 1]); base.irr[m.odd[k]] = 1; }
    }
    base.Row = makeRowClass(base);
    return base;
}

/* Record b as text, its line break left out.
   A TextDecoder call costs ~1 µs whatever its size: decoding a million
   records one by one took 900 ms, the same bytes in one call 60. So a
   run of consecutive records (a scan, visitRows(), the rows on screen)
   is decoded by blocks of 1024 records and sliced; an isolated record
   (a sorted view read in its own order) alone. */
const BLK_BITS = 10;
function recordText(base, b) {
    let blk = base.blk;
    if (!blk || blk.k !== b >> BLK_BITS) {
        const seq = b === base.lastB + 1;
        base.lastB = b;
        if (!seq) return decodeOne(base, b);
        blk = base.blk = loadBlock(base, b >> BLK_BITS);
    }
    base.lastB = b;
    if (!blk.ok) return decodeOne(base, b);
    const o = blk.o, t = blk.text;
    let s = o[b] - blk.c0, e = o[b + 1] - blk.c0;
    while (e > s && (t.charCodeAt(e - 1) === 10 || t.charCodeAt(e - 1) === 13)) e--;
    return t.slice(s, e);
}
function decodeOne(base, b) {
    const u8 = base.u8, s = base.starts[b];
    let e = base.starts[b + 1];
    while (e > s && (u8[e - 1] === 10 || u8[e - 1] === 13)) e--;
    return base.dec.decode(u8.subarray(s, e));
}
/* Records k·1024 … as one string, sliced at each record's offset in
   characters (base.chars, counted by the scan worker; the byte offsets
   when characters and bytes are the same). Bytes that are not valid
   UTF-8 decode to fewer characters than counted: such a block is then
   decoded record by record (ok = false). */
function loadBlock(base, k) {
    const st = base.starts, b0 = k << BLK_BITS, b1 = Math.min(base.n, b0 + (1 << BLK_BITS));
    const text = base.dec.decode(base.u8.subarray(st[b0], st[b1]));
    const o = base.chars || st;
    return { k, b0, text, o, c0: o[b0], ok: text.length === o[b1] - o[b0] };
}
/* The same field rules as the scanner, on one record's text. */
function splitRecord(s, D) {
    if (s.indexOf('"') < 0) return s.split(D);
    const out = [], L = s.length;
    let i = 0;
    for (;;) {
        if (s.charCodeAt(i) === 34) {
            let j = i + 1, val = '';
            for (;;) {
                const k = s.indexOf('"', j);
                if (k < 0) { val += s.slice(j); i = L; break; }
                if (s.charCodeAt(k + 1) === 34) { val += s.slice(j, k + 1); j = k + 2; continue; }
                let m = k + 1;
                while (m < L && (s[m] === ' ' || (s[m] === '\t' && D !== '\t'))) m++;
                if (m >= L || s[m] === D) { val += s.slice(j, k); i = m; break; }
                val += s.slice(j, k + 1); j = k + 1;
            }
            out.push(val);
        } else {
            const k = s.indexOf(D, i);
            if (k < 0) { out.push(s.slice(i)); i = L; } else { out.push(s.slice(i, k)); i = k; }
        }
        if (i < L && s[i] === D) { i++; if (i >= L) { out.push(''); break; } continue; }
        return out;
    }
    return out;
}
function recordFields(base, b) { return splitRecord(recordText(base, b), base.delim); }

/* A row's cells in new columns: map[c] = its old column (-1: a new, empty
   one). A long row keeps its extra fields; a short one stays short — the
   columns it never had are only filled up to its last real field, so an
   irregular row is still flagged as one (Save pads it anyway). */
function mapCells(f, map, width) {
    const out = new Array(map.length);
    let last = -1;
    for (let c = 0; c < map.length; c++) {
        const s = map[c];
        if (s >= 0 && s < f.length) { out[c] = f[s]; last = c; } else out[c] = '';
    }
    if (f.length < width) out.length = last + 1;
    return f.length > width ? out.concat(f.slice(width)) : out;
}
/* Record b in the current columns, through a direct-mapped cache (a row is
   typically read several times in a row: filter, then highlight, then…). */
function baseCells(base, b) {
    const slot = b & 1023;
    if (base.keys[slot] === b) return base.vals[slot];
    let f = recordFields(base, b);
    if (base.cmap) f = mapCells(f, base.cmap, base.width);
    base.keys[slot] = b; base.vals[slot] = f;
    return f;
}
function setCmap(base, cm) { base.cmap = cm; base.keys.fill(-1); }

/* Which records of block k contain every query, lower() applied to the
   block's text once (a million toLowerCase calls cost more than the
   search itself). null when lower() changes the text's length (a few
   Unicode letters do): the caller then tests record by record.
   Up to LOWER_CACHE bytes of file, the lowered blocks are kept (per
   mode: lower case, or lower case without accents): typing a search runs
   one filtering per keystroke, and the bytes never change. */
const LOWER_CACHE = 128 << 20;
function loweredBlock(base, k, lower, mode) {
    const cache = base.u8.length <= LOWER_CACHE ? (base.lower = base.lower || {})[mode] = (base.lower[mode] || new Map()) : null;
    let text = cache ? cache.get(k) : undefined;
    if (text === undefined) {
        if (!base.blk || base.blk.k !== k) { base.blk = loadBlock(base, k); base.lastB = -2; }
        const blk = base.blk;
        text = blk.ok ? lower(blk.text) : null;
        if (text !== null && text.length !== blk.text.length) text = null;
        if (cache) cache.set(k, text);
    }
    return text;
}
/* One record's lowered text, sliced from its lowered block when that one is
   cached — for a search narrowing down rows already found, which are too
   few per block to be worth searching whole blocks for. */
function loweredRecord(base, b, lower, mode) {
    const k = b >> BLK_BITS, cache = base.lower && base.lower[mode], text = cache ? cache.get(k) : undefined;
    if (text == null) return lower(recordText(base, b));
    const o = base.chars || base.starts, c0 = o[k << BLK_BITS];
    let s = o[b] - c0, e = o[b + 1] - c0;
    while (e > s && (text.charCodeAt(e - 1) === 10 || text.charCodeAt(e - 1) === 13)) e--;
    return text.slice(s, e);
}
function blockMatches(base, k, lower, queries, mode) {
    const text = loweredBlock(base, k, lower, mode);
    if (text === null) return null;
    const b0 = k << BLK_BITS, nb = Math.min(base.n, b0 + (1 << BLK_BITS)) - b0, o = base.chars || base.starts, c0 = o[b0];
    const off = i => o[b0 + i] - c0;
    let hits = null;
    for (const q of queries) {
        const h = new Uint8Array(nb);
        let r = 0, p = text.indexOf(q);
        while (p >= 0) {
            while (r + 1 < nb && off(r + 1) <= p) r++;
            h[r] = 1;
            p = text.indexOf(q, Math.max(p + 1, r + 1 < nb ? off(r + 1) : text.length));   // on to the next record
        }
        if (hits) for (let i = 0; i < nb; i++) hits[i] &= h[i]; else hits = h;
    }
    return hits;
}

/* One cell, without splitting the whole record when no quote is in it
   (column filters, sorting, a column's profile read one field of a
   million rows). Missing fields read as undefined, like r.data[c]. */
function cellOf(r, c) {
    if (r.d) return r.d[c];
    const base = r.base, b = r.b, slot = b & 1023;
    if (base.keys[slot] === b) return base.vals[slot][c];
    let s = c;
    if (base.cmap) { if (c >= base.cmap.length) return baseCells(base, b)[c]; s = base.cmap[c]; if (s < 0) return ''; }
    const text = recordText(base, b), D = base.delim, q = text.indexOf('"');
    let i = 0;
    for (let k = 0; k < s; k++) { i = text.indexOf(D, i); if (i < 0) break; i++; }
    const e = i < 0 ? -1 : text.indexOf(D, i);
    if (q >= 0 && (i < 0 || e < 0 || q < e)) return baseCells(base, b)[c];   // a quote before the field ends: the fields may not be where they seem
    return i < 0 ? undefined : e < 0 ? text.slice(i) : text.slice(i, e);
}

/* Rows visited in the order of their records in the file, so a sorted
   view is still decoded by blocks: fn(r, i), i its index in rows. Rows
   with their own array come first. Callers that need rows' order write
   into their own arrays at i. */
function visitRows(t, rows, fn) {
    const base = t.base;
    let inOrder = true, prev = -1;
    for (let i = 0; i < rows.length; i++) { const r = rows[i]; if (r.d || r.b < 0) continue; if (r.b < prev) { inOrder = false; break; } prev = r.b; }
    if (!base || inOrder) { for (let i = 0; i < rows.length; i++) fn(rows[i], i); return; }
    const at = new Int32Array(base.n).fill(-1);
    for (let i = 0; i < rows.length; i++) { const r = rows[i]; if (r.d || r.b < 0) fn(r, i); else at[r.b] = i; }
    for (let b = 0; b < base.n; b++) { const i = at[b]; if (i >= 0) fn(rows[i], i); }
}

function makeRowClass(base) {
    return class Row {
        constructor(b, src, d) { this.id = 0; this.src = src; this.b = b; this.d = d; }
        get data() { return this.d || baseCells(base, this.b); }
        set data(v) { this.d = v; }
        /* Field count without decoding. */
        get len() {
            if (this.d) return this.d.length;
            if (!(base.irr && base.irr[this.b])) return base.cmap ? base.cmap.length : base.width;
            return base.cmap ? baseCells(base, this.b).length : base.odd.get(this.b);   // an irregular row: few, decode it
        }
        get base() { return base; }
    };
}
/* A row born here (inserted, pasted, duplicated, appended): no record, no line. */
function newRow(t, cells) { return new t.base.Row(-1, null, cells); }

/* Edits of one operation. set() gives the row its own copy on first touch
   (never mutating an array r.data may share), undo() puts every touched
   row's previous d back — null for a row that was still its record. */
function rowEdits() {
    const rows = [], prev = [], seen = new Set();
    return {
        set(r, c, v) {
            if (!seen.has(r)) { seen.add(r); rows.push(r); prev.push(r.d); r.d = r.data.slice(); }
            const d = r.d;
            while (d.length <= c) d.push('');
            d[c] = v;
        },
        /* The whole array at once (a row of a structural change). */
        put(r, d) { if (!seen.has(r)) { seen.add(r); rows.push(r); prev.push(r.d); } r.d = d; },
        get count() { return rows.length; },
        undo() { for (let k = rows.length - 1; k >= 0; k--) rows[k].d = prev[k]; }
    };
}

/* Re-map the columns: newToOld[c] = the old column now at c (-1: a new,
   empty one). Base rows follow through cmap, rows with their own array
   are rebuilt; the returned function undoes both. */
function remapRows(t, newToOld) {
    const base = t.base, oldW = t.headers.length, prevCmap = base.cmap;
    const old = prevCmap || Array.from({ length: base.width }, (_, i) => i);
    let cm = newToOld.map(o => o < 0 ? -1 : (o < old.length ? old[o] : -1));
    if (cm.length === base.width && cm.every((s, i) => s === i)) cm = null;
    setCmap(base, cm);
    const mats = [];
    for (const r of t.allData) {
        if (!r.d) continue;
        mats.push([r, r.d]);
        r.d = mapCells(r.d, newToOld, oldW);
    }
    return () => { setCmap(base, prevCmap); for (const [r, d] of mats) r.d = d; };
}

/* The rows of a file on disk, for Review changes: an accessor over a
   freshly scanned base, with the tab's own encoding and delimiter. */
async function readDiskRecords(t) {
    const base = await loadBase((await tabBytes(t, null)).bytes, { encoding: currentEnc(t), delim: currentDelim(t), eol: t.base ? t.base.eol : t.detectedEol });
    return { length: base.n, get: i => recordFields(base, i) };
}
