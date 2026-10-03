/* ---------------------------------------------------------------
   REGEX GUARD
   A regular expression typed by the user runs on the page's thread, over
   every row shown. One with catastrophic backtracking — (a+)+$, (.*)*x,
   (\w+\s?)*$ — can take minutes on a single value: the window freezes,
   and killing it loses the edits not saved yet. A running regex cannot be
   interrupted on this thread, but a worker can be killed: so a pattern is
   first tried there, on a sample of the values it will meet, and refused
   past REGEX_PROBE_MS. Its verdict is kept per tab, scope and pattern;
   until it is in, the caller says the pattern is being checked and is
   called back. A pattern without a repetition (* + ? {) cannot run away
   and is never held. The sample may miss the one value that would make a
   pattern explode: this catches the patterns that are slow on the file's
   values, not every pathological case.
----------------------------------------------------------------*/
const REGEX_PROBE_MS = 1000;
const REGEX_SAMPLE_ROWS = 2000, REGEX_SAMPLE_MAX = 30000;
const regexVerdicts = new Map();          // key → 'ok' | 'slow'
const regexWaiting = new Map();           // key → callbacks to run once its verdict is in
let regexWorker = null, regexQueue = Promise.resolve();

/* patterns: [[scope, source, flags]] — scope 'c<column>' for one column's values, anything
   else for every column's. null when they may all run; else {error, pending}: refused, or
   being checked (retry, if given, is called when the verdict is in). */
function regexGate(t, patterns, retry) {
    let pending = false;
    for (const [scope, src, flags] of patterns) {
        if (!src || !/[*+?{]/.test(src)) continue;
        const key = `${t.id}\u0001${scope}\u0001${(flags || '').replace('g', '')}\u0001${src}`, v = regexVerdicts.get(key);
        if (v === 'slow') return { error: `this regular expression takes too long on this file's values and would freeze the window — avoid nested repeats such as (a+)+ or (.*)*` };
        if (v !== 'ok') { pending = true; regexCheck(t, scope, key, src, flags || '', retry); }
    }
    return pending ? { error: 'checking the regular expression…', pending: true } : null;
}

function regexCheck(t, scope, key, src, flags, retry) {
    const waiting = regexWaiting.get(key);
    if (waiting) { if (retry && !waiting.includes(retry)) waiting.push(retry); return; }
    regexWaiting.set(key, retry ? [retry] : []);
    const samples = regexSamples(t, scope);
    regexQueue = regexQueue.then(() => regexProbe(src, flags, samples)).then(v => {
        regexVerdicts.set(key, v);
        const cbs = regexWaiting.get(key) || [];
        regexWaiting.delete(key);
        for (const f of cbs) { try { f(); } catch (e) { console.warn(e); } }
    });
}

/* The values a pattern will meet: one column's, or every column's, in rows spread over the file. */
function regexSamples(t, scope) {
    const rows = t.allData || [], n = rows.length, out = [];
    const col = scope[0] === 'c' ? +scope.slice(1) : -1, step = Math.max(1, Math.floor(n / REGEX_SAMPLE_ROWS));
    for (let i = 0; i < n && out.length < REGEX_SAMPLE_MAX; i += step) {
        if (col >= 0) { const v = cellOf(rows[i], col); if (v) out.push(String(v)); continue; }
        const d = rows[i].data;
        for (let c = 0; c < d.length; c++) if (d[c]) out.push(String(d[c]));
    }
    return out;
}

/* 'ok' once the worker has run the pattern over every sample, 'slow' if it has not within REGEX_PROBE_MS (it is then killed). */
function regexProbe(src, flags, samples) {
    return new Promise(resolve => {
        if (!regexWorker) regexWorker = new Worker(URL.createObjectURL(new Blob([`(${regexProbeWorker})()`], { type: 'text/javascript' })));
        const w = regexWorker;
        const timer = setTimeout(() => { w.terminate(); if (regexWorker === w) regexWorker = null; resolve('slow'); }, REGEX_PROBE_MS);
        w.onmessage = () => { clearTimeout(timer); resolve('ok'); };
        w.postMessage({ src, flags, samples });
    });
}

/* Runs in the worker. Every match of every sample, as a replace or a highlight would scan them;
   a pattern that does not compile is answered at once — its caller reports that itself. */
function regexProbeWorker() {
    onmessage = e => {
        const { src, flags, samples } = e.data;
        let re;
        try { re = new RegExp(src, flags.includes('g') ? flags : flags + 'g'); } catch (err) { postMessage(0); return; }
        for (const s of samples) { re.lastIndex = 0; s.replace(re, ''); }
        postMessage(1);
    };
}
