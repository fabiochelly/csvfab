/* ---------------------------------------------------------------
   SCAN IN PARALLEL
   Opening a file is mostly the scan that finds its records (~80 ms on
   38 MB, ~500 ms on 147 MB, ~12 s on 2 GB), one pass over every byte.
   Here the bytes are cut into slices, each scanned by its own worker
   with the same csvScanCore() as the single scan, and the page stitches
   the results. A slice but the first starts at the first line start in
   it, as if a record started there — which it may not: the line may be
   inside a quoted field. So each slice also scans a little past its end
   (an overflow, on a copy that overlaps the next slice), from where the
   true scan reaches, and the stitching looks for the first record start
   both the overflow and the next slice found: from there the scanner is
   in the same state (it holds no other), so the next slice's records are
   the true ones. Before it, the next slice's records are dropped and
   the overflow's kept; the character offsets and the counters (quote
   errors, invalid UTF-8) are re-based at that point. No common start —
   a quoted field running past the overflow, a slice without a line
   break — and the file is scanned whole, as before: the result is never
   different, at worst no faster.
----------------------------------------------------------------*/
const SCAN_PAR = {
    min: 8 << 20,           // bytes of file from which the scan is split
    slice: 0,               // bytes per slice (0: one slice per worker)
    minSlice: 2 << 20,      // …but no smaller than this
    over: 1 << 20,          // bytes of the next slice copied with a slice, for its overflow and its last record
    scanOver: 64 << 10,     // how far past its end a slice's overflow looks for record starts
    checkpoints: 1024,      // first records of a slice where the stitching may join it
    workers: Math.max(2, Math.min(16, (navigator.hardwareConcurrency || 4) - 2)),
};
let scanPool = null;        // workers kept between files: starting them costs more than a small scan
let scanParWhy = '';      // why the last split scan gave up (tests)
let scanBusy = false;       // one split scan at a time: a second one meanwhile (a lookup source) scans whole
let scanParUrl = null;
function scanParWorkers(k) {
    if (!scanParUrl) scanParUrl = URL.createObjectURL(new Blob([`${csvScanCore}\n(${scanParWorker})()`], { type: 'text/javascript' }));
    scanPool = scanPool || [];
    while (scanPool.length < k) scanPool.push(new Worker(scanParUrl));
    return scanPool.slice(0, k);
}
function scanParWorker() {
    onmessage = e => {
        const { id, buf, o } = e.data;
        try {
            const r = csvScanCore(new Uint8Array(buf), o);
            postMessage({ id, r }, [r.starts.buffer, r.odd.buffer].concat(r.chars ? [r.chars.buffer] : []));
        } catch (err) { postMessage({ id, error: String(err && err.message || err) }); }
    };
}

/* The scan's result for msg (as runScan() gives it), or null when the file is not worth
   splitting or the stitching gave up — the caller then scans it whole. The buffer stays the
   caller's: only copies of its slices go to the workers. */
async function scanParallel(msg, onProgress) {
    const { buf, enc, validate, delim, nl, bom, lines } = msg;
    scanParWhy = '';
    if (enc.startsWith('utf-16') || typeof Worker !== 'function') return (scanParWhy = 'utf-16'), null;
    const u8 = new Uint8Array(buf).subarray(bom), len = u8.length;
    if (len < SCAN_PAR.min) return (scanParWhy = 'small'), null;
    const NL = nl, wantChars = enc === 'utf-8', base = { delim, nl, lines, wantChars };
    const head = csvScanCore(u8, { ...base, wantChars: false, limit: 1 });   // the file's width: its first record's field count
    if (!head.n) return (scanParWhy = 'empty'), null;
    const per = Math.max(SCAN_PAR.slice || Math.ceil(len / SCAN_PAR.workers), SCAN_PAR.slice ? 1 : SCAN_PAR.minSlice);
    /* The cuts are line starts, the first one at or after every per bytes: a line longer than a
       slice merges the slices it spans, instead of leaving one with no place to start. */
    const cuts = [0];
    for (let p = per; p < len; p += per) {
        const from = u8.indexOf(NL, p - 1) + 1;
        if (from <= 0 || from >= len) break;
        if (from > cuts[cuts.length - 1]) cuts.push(from);
    }
    cuts.push(len);
    const jobs = [];
    for (let k = 0; k + 1 < cuts.length; k++) {
        const from = cuts[k], next = cuts[k + 1], end = Math.min(len, next + SCAN_PAR.over);
        jobs.push({ from, end, o: { ...base, width: head.width, mainEnd: next - from, stop: Math.min(end, next + SCAN_PAR.scanOver) - from, atEnd: end === len, checkpoints: SCAN_PAR.checkpoints } });
    }
    if (jobs.length < 2 || scanBusy) return (scanParWhy = scanBusy ? 'busy' : 'one slice'), null;
    scanBusy = true;
    const ws = scanParWorkers(Math.min(jobs.length, SCAN_PAR.workers));
    const res = await new Promise(resolve => {
        const out = new Array(jobs.length);
        let next = 0, left = jobs.length, failed = false;
        const give = w => {
            if (next >= jobs.length || failed) return;
            const k = next++, j = jobs[k], copy = u8.slice(j.from, j.end);
            w.onmessage = e => {
                if (e.data.error) { failed = true; scanParWhy = e.data.error; return resolve(null); }
                out[k] = e.data.r;
                left--;
                if (onProgress) onProgress(1 - left / jobs.length);
                if (!left) resolve(out); else give(w);
            };
            w.onerror = () => { failed = true; resolve(null); };
            w.postMessage({ id: k, buf: copy.buffer, o: j.o }, [copy.buffer]);
        };
        ws.forEach(give);
    }).finally(() => { scanBusy = false; });
    const st = res && scanStitch(res, jobs, len, enc, validate, wantChars, head.width);
    return st && { ...st, buf, off: new Uint8Array(buf).byteOffset + bom, len, orig: null };
}

/* The slices' results joined into the file's, or null if a slice cannot be joined to the next. */
function scanStitch(res, jobs, len, enc, validate, wantChars, width) {
    const before = (r, idx, which) => {        // a slice's counter before its record idx
        if (idx === 0) return 0;
        if (idx === r.nMain) return which ? r.badMain : r.qerrMain;
        for (let k = 0; k < r.ck.length; k += 3) if (r.ck[k] === idx) return r.ck[k + 1 + which];
        return null;
    };
    const segs = [];
    let qerr = 0, bad = 0, a = 0, posFix = 0, charFix = 0;
    for (let k = 0; k < res.length; k++) {
        const r = res[k];
        if (r.fail) return (scanParWhy = `slice ${k}: a record runs past its copy`), null;
        const q0 = before(r, a, 0), b0 = before(r, a, 1);
        if (q0 == null || b0 == null) return (scanParWhy = `slice ${k}: joined past its checkpoints`), null;
        segs.push({ r, a, b: r.nMain, posFix, charFix });
        qerr += r.qerrMain - q0; bad += r.badMain - b0;
        if (k === res.length - 1) break;
        /* The overflow (true records past the slice's end) meets the next slice's records. */
        const nx = res[k + 1], nxFrom = jobs[k + 1].from, lim = Math.min(nx.nMain, SCAN_PAR.checkpoints);
        let o = r.nMain, j = 0, found = false;
        while (o < r.n && j < lim) {
            const x = r.starts[o] + posFix, y = nx.starts[j] + nxFrom;
            if (x === y) { found = true; break; }
            if (x < y) o++; else j++;
        }
        if (!found) return (scanParWhy = `slices ${k}/${k + 1}: no common record start`), null;
        const qo = before(r, o, 0), bo = before(r, o, 1);
        if (qo == null || bo == null) return (scanParWhy = `slice ${k}: overflow without checkpoint`), null;
        segs.push({ r, a: r.nMain, b: o, posFix, charFix });
        qerr += qo - r.qerrMain; bad += bo - r.badMain;
        if (wantChars) charFix = r.chars[o] + charFix - nx.chars[j];
        posFix = nxFrom; a = j;
    }
    const last = segs[segs.length - 1];
    const adjTotal = wantChars ? last.r.chars[last.r.n] + last.charFix - len : 0;   // characters − bytes, whole file
    let outEnc = enc, keepChars = wantChars;
    if (validate && bad) { outEnc = 'windows-1252'; keepChars = false; }   // not UTF-8: one byte, one character
    if (keepChars && !adjTotal) keepChars = false;                          // ASCII throughout
    let n = 0, no = 0;
    for (const s of segs) { n += s.b - s.a; for (let k = 0; k < s.r.odd.length; k += 2) if (s.r.odd[k] >= s.a && s.r.odd[k] < s.b) no += 2; }
    const Big = len > 0xFFFFFFF0 ? Float64Array : Uint32Array;
    const starts = new Big(n + 1), chars = keepChars ? new Big(n + 1) : null, odd = new Uint32Array(no);
    let w = 0, wo = 0;
    for (const s of segs) {
        const r = s.r;
        for (let k = 0; k < r.odd.length; k += 2) if (r.odd[k] >= s.a && r.odd[k] < s.b) { odd[wo++] = r.odd[k] - s.a + w; odd[wo++] = r.odd[k + 1]; }
        for (let i = s.a; i < s.b; i++, w++) {
            starts[w] = r.starts[i] + s.posFix;
            if (chars) chars[w] = r.chars[i] + s.charFix;
        }
    }
    starts[n] = len;
    if (chars) chars[n] = len + adjTotal;
    return { starts, chars, odd, n, width: Math.max(width, 0), qerr, enc: outEnc };
}
