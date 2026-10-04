/* ---------------------------------------------------------------
   FILTERS IN PARALLEL
   On a big file the text and value filters test every record on one
   core: 250 ms a keystroke on 147 MB, 700 ms with accents ignored. Here
   the records are cut into slices, one per worker, each holding a copy
   of its slice's bytes (sent once, at the first filtering of that file:
   TextDecoder refuses a SharedArrayBuffer view, so sharing the bytes
   would have meant copying them at every decode instead). A query goes
   to every worker, which runs the very same row test (textRowTest(),
   valueRowTest() and the row store's readers, their source sent as is)
   on its records and sends back one byte per record; the page then only
   tests the rows that are no longer their record (edited, added).
   The answer depends on the bytes, the queries and the column map, not
   on the rows: kept per base, it still holds after an edit, a sort or
   an undo. applyFilters() waits for it only when the rows have not
   changed since the view was last filtered — typing, ticking an option
   — and keeps that view meanwhile; any other caller, which may read
   the result at once, filters on the page's thread as before.
----------------------------------------------------------------*/
const PAR_MIN = 16 << 20;                  // bytes of file from which the filters go to workers
const PAR_MAX = 1 << 30;                   // …and up to which: the slices are a second copy of the file in RAM
const PAR_N = Math.max(2, Math.min(16, (navigator.hardwareConcurrency || 4) - 2));   // measured on 16 cores: 4 workers ×2.8, 8 ×3.5, 14 ×4.5
const PAR_KEEP = 4;                        // answers kept per file
let par = null;                            // { base: WeakRef, ws: [{w, b0, b1}], cache, run, next, waiters, broken }
const parGone = typeof FinalizationRegistry === 'function' ? new FinalizationRegistry(p => { if (par === p) parDrop(); }) : null;

function parUsable(t) {
    const B = t.base;
    return !!B && !B.lines && B.u8.length >= PAR_MIN && B.u8.length <= PAR_MAX && B.n >= 4 << BLK_BITS && typeof Worker === 'function' && !(par && par.broken === B);
}

/* The worker's script: the row store's readers and the row tests as they are
   in the page, so a filtered row is the same row whichever thread tested it. */
let parSrcUrl = null;
function parWorkerUrl() {
    if (parSrcUrl) return parSrcUrl;
    const fns = [recordText, decodeOne, loadBlock, splitRecord, recordFields, mapCells, baseCells, cellOf,
        loweredBlock, loweredRecord, blockMatches, removeAccents, cellStr, queryTerms, textRowTest, valueRowTest];
    const src = `const BLK_BITS = ${BLK_BITS}; let LOWER_CACHE = 0;\n${fns.map(String).join('\n')}\n(${parWorker})();`;
    return (parSrcUrl = URL.createObjectURL(new Blob([src], { type: 'text/javascript' })));
}
function parWorker() {
    let base = null, row = null, cmapKey = 'null';
    onmessage = e => {
        const m = e.data;
        if (m.init) {
            const i = m.init;
            LOWER_CACHE = i.lowerOk ? Infinity : 0;   // the page's rule, on the whole file: its lowered text kept up to LOWER_CACHE of file
            base = { u8: i.u8, starts: i.starts, chars: i.chars, n: i.n, width: i.width, delim: i.delim, lines: false, cmap: null,
                keys: new Int32Array(1024).fill(-1), vals: new Array(1024), blk: null, lastB: -2, run: 0, dec: new TextDecoder(i.enc, { ignoreBOM: true }) };
            row = { b: 0, d: null, base, get data() { return baseCells(base, this.b); } };
            return;
        }
        const q = m.query, key = JSON.stringify(q.cmap);
        if (key !== cmapKey) { cmapKey = key; base.cmap = q.cmap; base.keys.fill(-1); }
        const tt = q.text ? textRowTest(q.text, base, false) : null;
        const vt = q.vals.length ? valueRowTest(q.vals.map(([c, v]) => [c, new Set(v)])) : null;
        const bits = new Uint8Array(base.n);
        for (let b = 0; b < base.n; b++) { row.b = b; if ((!tt || tt(row)) && (!vt || vt(row))) bits[b] = 1; }
        postMessage({ id: m.id, bits }, [bits.buffer]);
    };
}

/* The workers of a base, started and sent their slices on first use. */
function parPool(base) {
    if (par && par.base.deref() === base && !par.broken) return par;
    parDrop();
    const n = base.n, N = Math.min(PAR_N, n >> BLK_BITS), per = Math.ceil(n / N / (1 << BLK_BITS)) << BLK_BITS, ws = [];
    const lowerOk = base.u8.length <= LOWER_CACHE, enc = base.transcoded ? 'utf-8' : base.enc, url = parWorkerUrl();
    for (let b0 = 0; b0 < n; b0 += per) {
        const b1 = Math.min(n, b0 + per), nb = b1 - b0;
        const rebase = o => { if (!o) return null; const r = new o.constructor(nb + 1), z = o[b0]; for (let i = 0; i <= nb; i++) r[i] = o[b0 + i] - z; return r; };
        const u8 = base.u8.slice(base.starts[b0], base.starts[b1]), starts = rebase(base.starts), chars = rebase(base.chars);
        const w = new Worker(url);
        w.postMessage({ init: { u8, starts, chars, n: nb, width: base.width, delim: base.delim, enc, lowerOk } }, [u8.buffer, starts.buffer, ...(chars ? [chars.buffer] : [])]);
        ws.push({ w, b0, b1 });
    }
    par = { base: new WeakRef(base), ws, cache: new Map(), run: null, next: null, waiters: [], seq: 0 };
    if (parGone) parGone.register(base, par);
    return par;
}
function parDrop() {
    if (!par) return;
    for (const x of par.ws) x.w.terminate();
    const w = par.waiters; par = null;
    w.forEach(f => f());
}
/* A base going away (tab closed, released, re-read): its copies go with it. */
function parRelease(base) { if (par && par.base.deref() === base) parDrop(); }

/* What the workers are asked: the text spec (without the expression, which stays
   on the page), the value filters as lists, the column map. */
function parQuery(t, spec) {
    const vals = Object.keys(t.valFilters).map(Number).map(c => [c, [...t.valFilters[c]]]);
    return { text: spec ? { g: spec.g, cols: spec.cols, isRegex: spec.isRegex, isReverse: spec.isReverse, useSlug: spec.useSlug } : null,
        vals, cmap: t.base.cmap ? Array.from(t.base.cmap) : null };
}
function parKey(q) { return JSON.stringify(q); }
/* The answer for q if it is known: one byte per record, 1 = kept. */
function parCached(base, q) {
    if (!par || par.base.deref() !== base) return null;
    const k = parKey(q), bits = par.cache.get(k);
    if (bits) { par.cache.delete(k); par.cache.set(k, bits); }   // most recently used last
    return bits || null;
}
/* Ask for q; done() is called once it is known (or the workers failed — the
   page then filters on its own thread). A query asked while another runs
   waits for it, and only the last one asked does: the ones typed in between
   are dropped. */
function parRequest(base, q, done) {
    const p = parPool(base), k = parKey(q);
    if (p.run && p.run.k === k) { p.run.done = done; return; }
    if (p.run) { p.next = { q, k, done }; return; }
    parStart(p, base, q, k, done);
}
function parStart(p, base, q, k, done) {
    const id = ++p.seq, parts = new Array(p.ws.length);
    let left = p.ws.length;
    p.run = { k, done, id };
    const finish = ok => {
        if (par !== p || !p.run || p.run.id !== id) return;
        const run = p.run; p.run = null;
        if (ok) {
            const bits = new Uint8Array(base.n);
            p.ws.forEach((x, i) => bits.set(parts[i], x.b0));
            p.cache.set(k, bits);
            while (p.cache.size > PAR_KEEP) p.cache.delete(p.cache.keys().next().value);
        } else { p.broken = base; for (const x of p.ws) x.w.terminate(); }
        const next = p.next; p.next = null;
        if (ok && next) parStart(p, base, next.q, next.k, next.done);
        else { const w = p.waiters; p.waiters = []; w.forEach(f => f()); }
        run.done();
        if (!ok && next) next.done();
    };
    p.ws.forEach((x, i) => {
        x.w.onmessage = e => { if (e.data.id !== id) return; parts[i] = e.data.bits; if (--left === 0) finish(true); };
        x.w.onerror = e => { console.warn('filter worker:', e.message); finish(false); };
        x.w.postMessage({ id, query: q });
    });
}
/* Resolved once no filtering is under way in the workers (the page bench waits on it). The
   waiters are resolved after the run's done(), whose re-filtering is synchronous. */
async function whenFiltered() {
    while (par && (par.run || par.next)) { const p = par; await new Promise(r => p.waiters.push(r)); }
}
