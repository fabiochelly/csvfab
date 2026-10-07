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
    min: 2 << 20,           // bytes of file from which the scan is split
    slice: 0,               // bytes per slice (0: one slice per worker)
    minSlice: 2 << 20,      // …but no smaller than this
    smallSlice: 512 << 10,  // …or this, below 8 MB (2.4 MB in ~5 slices: −9 ms, all cores; 1.2 MB: no gain)
    over: 1 << 20,          // bytes of the next slice copied with a slice, for its overflow and its last record
    scanOver: 64 << 10,     // how far past its end a slice's overflow looks for record starts
    checkpoints: 1024,      // first records of a slice where the stitching may join it
    head: 1 << 20,          // bytes of a file still arriving its first record is looked for in (else: all of them)
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
/* The workers a file of `size` bytes will scan in, started while its bytes travel (parseTab, 03-…):
   started when the scan begins, they made a first file wait for them — measured from a fresh page,
   5–13 ms for the single worker (the one kept is only started 1.5 s after load), ~30 ms for the
   split scan's. Once started they are kept, as before. */
function scanAhead(size) {
    if (typeof Worker !== 'function') return;
    if (size < SCAN_PAR.min) { if (!idleWorker) idleWorker = scanWorker(); return; }
    const per = scanPer(size);
    if (!scanBusy) scanParWorkers(Math.min(Math.ceil(size / per), SCAN_PAR.workers));
}
/* Bytes per slice for a file of len bytes. */
function scanPer(len) {
    if (SCAN_PAR.slice) return SCAN_PAR.slice;
    return Math.max(Math.ceil(len / SCAN_PAR.workers), len < 8 << 20 ? SCAN_PAR.smallSlice : SCAN_PAR.minSlice);
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
   splitting, false when the stitching (or a worker) gave up — the caller then scans it whole.
   The buffer stays the caller's: only copies of its slices go to the workers. feed: the bytes
   are still arriving (scanStream below) — each step waits for the ones it reads, and a slice
   leaves as soon as its own are in, rather than after the file's last byte. */
async function scanParallel(msg, onProgress, feed) {
    const { buf, enc, validate, delim, nl, bom, lines } = msg;
    scanParWhy = '';
    if (enc.startsWith('utf-16') || typeof Worker !== 'function') return (scanParWhy = 'utf-16'), null;
    const u8 = new Uint8Array(buf).subarray(bom), len = u8.length;
    if (len < SCAN_PAR.min) return (scanParWhy = 'small'), null;
    if (scanBusy) return (scanParWhy = 'busy'), null;
    const have = feed ? n => feed.until(bom + Math.min(n, len)) : null;   // the first n bytes of u8 are in
    const got = () => (feed ? feed.seen - bom : len);
    const NL = nl, wantChars = enc === 'utf-8', base = { delim, nl, lines, wantChars };
    scanBusy = true;
    try {
        /* The file's width: its first record's field count — on the first MB if it holds that record. */
        let head = null;
        if (have) {
            const k = Math.min(len, SCAN_PAR.head);
            await have(k);
            head = csvScanCore(u8.subarray(0, k), { ...base, wantChars: false, limit: 1, atEnd: k === len });
            if (head.fail || !head.n) { await have(len); head = null; }
        }
        if (!head) head = csvScanCore(u8, { ...base, wantChars: false, limit: 1 });
        if (!head.n) return (scanParWhy = 'empty'), null;
        const per = scanPer(len);
        const ws = scanParWorkers(Math.min(Math.ceil(len / per), SCAN_PAR.workers));
        const jobs = [], seq = ++scanJobs;
        const res = await new Promise(resolve => {
            const out = [], idle = [];
            let next = 0, done = 0, planned = false, failed = false;
            const fail = why => { if (!failed) { failed = true; if (why) scanParWhy = why; resolve(false); } };
            const give = w => {
                if (failed) return;
                if (next >= jobs.length) { if (!planned) idle.push(w); return; }
                const k = next++, j = jobs[k];
                const send = () => {
                    if (failed) return;
                    const copy = u8.slice(j.from, j.end);
                    w.onmessage = e => {
                        if (e.data.id !== seq + ':' + k) return;          // a job of a scan given up (its worker was busy)
                        if (e.data.error) return fail(e.data.error);
                        out[k] = e.data.r; done++;
                        if (onProgress && planned) onProgress(done / jobs.length);
                        if (planned && done === jobs.length) resolve(out); else give(w);
                    };
                    w.onerror = () => fail('a worker failed');
                    w.postMessage({ id: seq + ':' + k, buf: copy.buffer, o: j.o }, [copy.buffer]);
                };
                if (have) have(j.end).then(send, () => fail('the read failed')); else send();
            };
            const addJob = (from, next) => {
                const end = Math.min(len, next + SCAN_PAR.over);
                jobs.push({ from, end, o: { ...base, width: head.width, mainEnd: next - from, stop: Math.min(end, next + SCAN_PAR.scanOver) - from, atEnd: end === len, checkpoints: SCAN_PAR.checkpoints } });
                if (have && idle.length) give(idle.shift());       // arriving: a slice leaves as soon as it is known
            };
            /* The cuts are line starts, the first one at or after every per bytes: a line longer than a
               slice merges the slices it spans, instead of leaving one with no place to start. */
            const plan = async () => {
                let prev = 0;
                for (let p = per; p < len; p += per) {
                    let from = 0;
                    for (let at = p - 1; ;) {                     // the first line break from p - 1, once its bytes are in
                        const n = got(), k = (n >= len ? u8 : u8.subarray(0, n)).indexOf(NL, at);
                        if (k >= 0) { from = k + 1; break; }
                        if (n >= len) break;
                        at = Math.max(at, n);
                        await have(n + 1);
                    }
                    if (from <= 0 || from >= len) break;
                    if (from > prev) { addJob(prev, from); prev = from; }
                }
                addJob(prev, len);
                planned = true;
                if (jobs.length < 2) return fail('one slice');
                if (!have) ws.forEach(give);                     // whole: every slice known before any leaves, as before
                else { while (idle.length && next < jobs.length) give(idle.shift()); idle.length = 0; }
                if (done === jobs.length) resolve(out);
            };
            if (have) ws.forEach(give);                          // they wait, idle, for the first slices
            plan().catch(() => fail('the read failed'));
        });
        if (!res) return false;
        const st = scanStitch(res, jobs, len, enc, validate, wantChars, head.width);
        return st ? { ...st, buf, off: new Uint8Array(buf).byteOffset + bom, len, orig: null } : false;
    } finally { scanBusy = false; }
}
let scanJobs = 0;

/* A response read into its buffer as it arrives (tabBytes, 02-…), watched from elsewhere: the bytes
   so far, and until(n), a wait for the first n — rejected if the read fails or comes up short. */
function byteFeed() {
    let waits = [];
    const f = {
        u8: null, len: 0, seen: 0, ended: false, ok: false, stat: null,
        start(u8, stat) { f.u8 = u8; f.len = u8.length; f.stat = stat; wake(); },
        got(n) { f.seen = n; wake(); },
        end(ok) { if (f.ended) return; f.ended = true; f.ok = !!ok && !!f.u8 && f.seen === f.len; wake(); },
        until(n) { return new Promise((res, rej) => { const w = () => test(n, res, rej); if (!w()) waits.push(w); }); },
    };
    const test = (n, res, rej) => {
        if (f.ended && !f.ok) { rej(new Error('read failed')); return true; }
        if (f.u8 && f.seen >= Math.min(n, f.len)) { res(); return true; }
        return false;
    };
    const wake = () => { if (waits.length) waits = waits.filter(w => !w()); };
    return f;
}

/* The split scan of a bridge file while it is read (parseTab, 03-…): planned from its first bytes as
   loadBase() plans it (scanPlan), its slices scanned as they arrive. Measured (147 MB, no reopen cache, opening to
   the first frame): 513 → 488 ms. For loadBase's
   o.stream: { buf, plan, m } — m the scan, false if it gave up — or null when not for this file:
   small, UTF-16, another split scan running, or a big file the reopen cache may already hold
   (same size and date: no scan at all is likelier than this one). */
async function scanStream(feed, o) {
    try {
        await feed.until(PLAN_BYTES);
        if (feed.len < SCAN_PAR.min || scanBusy) return null;
        if (feed.len >= IDX_MIN && (!feed.stat || await idxMaybe(feed.len, feed.stat.mtime))) return null;
        const plan = scanPlan(feed.u8.subarray(0, feed.seen), o), buf = feed.u8.buffer;
        if (plan.enc.startsWith('utf-16')) return null;
        const m = await scanParallel(scanMsg(buf, plan, o), null, feed);
        return m === null ? null : { buf, plan, m };
    } catch (e) { return null; }
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

/* A file the launcher queued is coming (viewer.htm named it and asked for its bytes): its scan's
   workers start now, while this script ends and the page settles, not once its tab is made. */
if (window.CSVFAB_FILE && window.CSVFAB_QUEUED && window.CSVFAB_QUEUED.stat) scanAhead(window.CSVFAB_QUEUED.stat.size || 0);
