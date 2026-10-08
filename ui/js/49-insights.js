/* ---------------------------------------------------------------
   INSIGHTS — "What I noticed"
   One pass over the file (an even sample past IN_SAMPLE rows) and a
   few detectors, each turning what it finds into a card — the cards
   arrive as they are found:
   - the key: a column filled and different on every row, or one that
     nearly is (the values that come back are the finding);
   - a column that decides another (postal code → city) but not on a few
     rows: those rows disagree with the rest of their group;
   - values written otherwise than the rest of their column (7501 among
     75011, 2024-03-15 among 15/03/2024, DUPONT among Dupont, 12.50 among
     10,50);
   - numbers or dates far from all the others, a few negatives among
     positives;
   - rows that are exact copies of an earlier one;
   - hidden arithmetic: TTC = HT × 1,2, TTC = HT + TVA — on every row, or
     on all but a few;
   - empty or constant columns, stray spaces and invisible characters,
     irregular rows, garbled accents.
   "Show the rows" counts again on every row (the sample only found it)
   and marks them (row marks, shown alone; the cell at fault tinted, with
   what was expected as its tooltip). Nothing in the file changes.
----------------------------------------------------------------*/
const IN_SAMPLE = 100000;         // rows read at most for the detection (evenly spaced past that)
const IN_CELLS = 2e6;             // …and cells
const IN_DISTINCT = 50000;        // distinct values counted per column
const IN_FD_ROWS = 30000;         // rows the dependencies are looked for in
const IN_FD_COLS = 30;            // columns tried on each side
const IN_REL_ROWS = 20000;        // rows the arithmetic between columns is looked for in
const IN_REL_COLS = 16;           // numeric columns tried (sums: 12)
let insights = null;              // { t, stamp, items, done, run }

const IN_ICONS = {
    key: '<circle cx="8" cy="15" r="4"></circle><path d="M10.8 12.2 20 3M17 6l3 3M14 9l2 2"></path>',
    dep: '<path d="M4 7h10M10 3l4 4-4 4"></path><path d="M20 17H10M14 13l-4 4 4 4"></path>',
    shape: '<path d="M4 7V5h16v2M9 19h6M12 5v14"></path>',
    far: '<path d="M3 17l5-5 4 4 8-8"></path><circle cx="20" cy="5" r="2"></circle>',
    dup: '<rect x="8" y="8" width="13" height="13" rx="2"></rect><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"></path>',
    rel: '<path d="M5 9h14M5 15h14"></path>',
    empty: '<rect x="4" y="4" width="16" height="16" rx="2" stroke-dasharray="3 3"></rect>',
    space: '<path d="M4 10v4h16v-4"></path>',
    warn: '<path d="M12 9v4M12 17h.01"></path><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"></path>'
};

function insightsDialog() {
    modalOnce('modal-insights', `<div id="modal-insights" class="modal-box">
    <div class="in-head"><svg class="ic in-logo" viewBox="0 0 24 24"><path d="M12 3l1.9 5.6L19.5 10.5l-5.6 1.9L12 18l-1.9-5.6L4.5 10.5l5.6-1.9z"></path><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"></path></svg>
        <h4>What I noticed</h4><span id="in-sum" class="muted"></span></div>
    <div id="in-prog"><i></i></div>
    <div id="in-list"></div>
    <div class="modal-actions"><button class="btn" onclick="closeAllModals()">Close</button></div>
</div>`);
}
async function openInsights() {
    const t = T(); if (!t || !t.loaded || t.lang) return;
    insightsDialog();
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-insights').style.display = 'block';
    if (insights && insights.t === t && insights.done && sameStamp(insights.stamp, dataStamp(t))) { inRender(true); return; }
    const run = {};
    insights = { t, stamp: dataStamp(t), items: [], done: false, run };
    document.getElementById('in-list').innerHTML = '';
    inProgress(0, 'Reading the rows…');
    try { await inRun(t, run); }
    catch (e) { if (e !== IN_STOP) throw e; return; }
    if (insights.run !== run) return;
    insights.done = true;
    inProgress(1);
    inRender(false);
}
const IN_STOP = {};
/* The run goes on while its dialog is open on the same rows; a yield every ~12 ms. */
function inOpen() { const b = document.getElementById('modal-insights'); return !!b && b.style.display === 'block'; }
/* scheduler.yield(): a nested setTimeout(0) waits 4 ms, a quarter of every 12 ms slice. */
const inPause = () => typeof scheduler !== 'undefined' && scheduler.yield ? scheduler.yield() : new Promise(r => setTimeout(r, 0));
async function inYield(run) {
    await inPause();
    const I = insights;
    if (!I || I.run !== run || T() !== I.t || !inOpen() || !sameStamp(I.stamp, dataStamp(I.t))) throw IN_STOP;
}
function inProgress(p, msg) {
    const bar = document.getElementById('in-prog');
    if (bar) { bar.classList.toggle('on', p < 1); bar.firstElementChild.style.width = Math.round(p * 100) + '%'; }
    if (msg != null) document.getElementById('in-sum').textContent = msg;
}

/* ---- Cards ------------------------------------------------------ */
const inPl = (n, one, many) => `${fmt(n)} ${n === 1 ? one : (many || one + 's')}`;
const inV = v => `<span class="in-v">${esc(cutTxt(v === '' ? '(empty)' : v, 40))}</span>`;
const inC = (t, c) => `<span class="in-c" onclick="inGoCol(${c})" title="Go to this column">${esc(t.headers[c])}</span>`;
/* A count found on a sample, scaled to the file: ≈ in front. */
const inN = (S, n) => S.sampled ? '≈ ' + fmt(Math.max(1, Math.round(n * S.total / S.n))) : fmt(n);
const inNs = (S, n, one, many) => `${inN(S, n)} ${Math.round(n) === 1 && !S.sampled ? one : (many || one + 's')}`;
const IN_SEV = { warn: 0, info: 1 };
function inAdd(item) {
    const I = insights;
    item.id = I.items.length;
    I.items.push(item);
    inRender(false, item);
}
function inCard(it, k) {
    const acts = (it.show ? [`<button class="btn btn-outline in-show" onclick="inShow(${it.id})">${esc(it.showLabel || 'Show the rows')}</button>`] : [])
        .concat((it.acts || []).map((a, j) => `<button class="btn btn-outline" onclick="inAct(${it.id}, ${j})">${esc(a.label)}</button>`));
    return `<div class="in-card ${it.sev}" data-id="${it.id}" style="--i:${k}"><svg class="ic in-ic" viewBox="0 0 24 24">${IN_ICONS[it.icon] || IN_ICONS.warn}</svg>`
        + `<div class="in-b"><div class="in-t">${it.title}</div>${it.sub ? `<div class="in-s">${it.sub}</div>` : ''}</div>`
        + (acts.length ? `<div class="in-acts">${acts.join('')}</div>` : '') + '</div>';
}
/* The cards, data problems first, then by weight. One just found is inserted in its place
   (and animated); a full render (reopened, or done) redraws them all. */
function inRender(all, fresh) {
    const I = insights, list = document.getElementById('in-list');
    if (!I || !list) return;
    const items = I.items.slice().sort((a, b) => IN_SEV[a.sev] - IN_SEV[b.sev] || b.score - a.score);
    if (fresh) {
        const k = items.indexOf(fresh), next = items[k + 1];
        const el = document.createRange().createContextualFragment(inCard(fresh, 0));
        const before = next && list.querySelector(`.in-card[data-id="${next.id}"]`);
        list.insertBefore(el, before || null);
        return;
    }
    if (all || list.children.length !== items.length) list.innerHTML = items.map((it, k) => inCard(it, all ? k : 0)).join('');
    else for (const it of items) list.appendChild(list.querySelector(`.in-card[data-id="${it.id}"]`));   // in order, nothing redrawn
    const t = I.t, n = items.length, warn = items.filter(x => x.sev === 'warn').length;
    document.getElementById('in-sum').textContent = `${I.S ? (I.S.sampled ? `${fmt(I.S.n)} rows of ${fmt(I.S.total)} read` : inPl(I.S.total, 'row') + ' read') : ''}`
        + (I.done ? ` · ${n ? `${inPl(n, 'thing')} noticed${warn ? `, ${fmt(warn)} to look at` : ''}` : 'nothing stands out' }` : '');
    if (I.done && !n) list.innerHTML = `<div class="in-none"><svg class="ic" viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5"></path></svg>Nothing stands out in ${esc(t.name)}: no key broken, no value written otherwise, no outlier, no copy.</div>`;
}
function inGoCol(c) { closeAllModals(); goToColumn(c); }
function inAct(id, j) { const it = insights && insights.items[id]; if (!it) return; closeAllModals(); it.acts[j].run(); }
async function inShow(id) {
    const I = insights, it = I && I.items[id], t = I && I.t;
    if (!it || T() !== t) return;
    const btn = document.querySelector(`.in-card[data-id="${id}"] .in-show`);
    if (btn) { btn.disabled = true; btn.textContent = 'Counting…'; }
    const m = await it.show(t);
    if (T() !== t) return;
    closeAllModals();
    if (!m || !m.rows.size) { toast('Those rows are no longer there.', { kind: 'info' }); return; }
    setRowMark(t, { kind: 'insight', label: m.label, tip: m.tip || 'Expected', rows: m.rows, cells: m.cells || new Map(), only: true });
    doneMsg(`${t.name} | ${inPl(m.rows.size, 'row')} ${m.label} — shown alone; the chip in the status bar shows every row again.`);
}
/* Every row of the file through fn(r) in slices (Show counts on all rows, not the sample). */
async function inEvery(t, fn) {
    const all = t.allData;
    for (let i = 0; i < all.length; i += 50000) {
        visitRows(t, all.slice(i, i + 50000), fn);
        if (i + 50000 < all.length) await inPause();
    }
}
/* Marks: rows → the column's cell tinted, with what was expected as its tooltip. */
function inMarks(t, label, tip) {
    const rows = new Set(), cells = new Map();
    return { label, tip, rows, cells, add(r, c, v) { rows.add(r); if (c != null) { if (!cells.has(r)) cells.set(r, new Map()); cells.get(r).set(t.headers[c], v); } } };
}

/* ---- The pass ---------------------------------------------------- */
async function inRun(t, run) {
    const S = await inSample(t, run);
    insights.S = S;
    const steps = [inStructure, inColumns, inShapes, inFar, inArithmetic, inDeps, inCopies, inKeys];
    for (let k = 0; k < steps.length; k++) {
        inProgress(.25 + .75 * k / steps.length, `Looking ${['at the file', 'at each column', 'at how values are written', 'for outliers', 'for hidden arithmetic', 'for columns that decide others', 'for copied rows', 'for the key'][k]}…`);
        await inYield(run);
        await steps[k](t, S, run);
    }
}
/* The rows read: all of them, or an even sample; each column's values trimmed, plus its counts. */
async function inSample(t, run) {
    const all = t.allData, C = t.headers.length;
    const max = Math.max(2000, Math.min(IN_SAMPLE, Math.floor(IN_CELLS / Math.max(1, C))));
    const sampled = all.length > max, step = sampled ? all.length / max : 1;
    const rows = sampled ? Array.from({ length: max }, (_, i) => all[Math.floor(i * step)]) : all;
    const n = rows.length, vals = t.headers.map(() => new Array(n)), raw = t.headers.map(() => 0);
    let last = performance.now();
    for (let i0 = 0; i0 < n; i0 += 20000) {
        visitRows(t, rows.slice(i0, i0 + 20000), (r, j) => {
            const d = r.data, i = i0 + j;
            for (let c = 0; c < C; c++) {
                const v = d[c] == null ? '' : String(d[c]), s = v.trim();
                vals[c][i] = s;
                if (s && (s.length !== v.length || IN_RAW.test(s))) raw[c]++;
            }
        });
        if (performance.now() - last > 12) { inProgress(.25 * Math.min(1, (i0 + 20000) / n)); await inYield(run); last = performance.now(); }
    }
    const cols = vals.map((v, c) => {
        const counts = new Map(); let filled = 0, num = 0, date = 0, over = false;
        for (const s of v) {
            if (!s) continue;
            filled++;
            const ty = cellType(s); if (ty === 'n') num++; else if (ty === 'd') date++;
            const k = counts.get(s);
            if (k) counts.set(s, k + 1); else if (counts.size < IN_DISTINCT) counts.set(s, 1); else over = true;
        }
        const kind = !filled ? '' : num / filled >= .9 ? 'n' : date / filled >= .9 ? 'd' : 't';
        return { c, v, counts, filled, kind, over, distinct: counts.size, raw: raw[c] };
    });
    return { rows, cols, n, sampled, total: all.length };
}
/* Spaces doubled, no-break and narrow ones, and the invisible format characters met in pasted
   text (soft hyphen, zero-width, direction marks, word joiner, BOM): a plain class, \p{Cf} cost more. */
const IN_RAW = /\s\s|[\u00a0\u202f\u00ad\u200b-\u200f\u202a-\u202e\u2060-\u2064\ufeff]/;
/* A name saying the column identifies its rows (a postal or town code does not, though it says "code"). */
const inIdLike = name => { const n = slugify(name); return /(^|[^a-z])(id|ids|uuid|guid|code|ref|reference|key|cle|num|numero|no|matricule|siret|siren|iban|email|e mail|mail|login)($|[^a-z])/.test(n) && !/postal|zip|insee|commune|pays|country|ville|city/.test(n); };
/* Digits that are a code, not a quantity: their shape matters (a postal code of 4 digits lost its zero). */
const inCodeLike = name => /(^|[^a-z])(cp|code|codes|zip|postal|postcode|tel|telephone|phone|mobile|portable|gsm|fax|siret|siren|insee|iban|bic|id|ref|num|numero|no|matricule|compte)($|[^a-z])/.test(slugify(name));

/* Irregular rows, quote errors, garbled accents: the file's own state, counted on every row. */
async function inStructure(t, S) {
    const irr = t.allData.reduce((n, r) => n + (r.len !== t.headers.length ? 1 : 0), 0);
    if (irr || t.quoteErrors) inAdd({ sev: 'warn', icon: 'warn', score: 90,
        title: irr ? `${inPl(irr, 'row')} ${irr === 1 ? 'has' : 'have'} more or fewer fields than the header` : `${inPl(t.quoteErrors, 'quote')} in the file ${t.quoteErrors === 1 ? 'is' : 'are'} not where a field starts or ends`,
        sub: (irr && t.quoteErrors ? `and ${inPl(t.quoteErrors, 'quote error')}. ` : '') + 'A field cut by an unquoted delimiter or line break shifts every value after it. The row card\'s <i>Why?</i> tells each one\'s story.',
        acts: irr ? [{ label: 'Show them', run: () => { if (!t.onlyIrregular) toggleIrregular(); } }] : [] });
    if (t.mojibake) inAdd({ sev: 'warn', icon: 'warn', score: 85, title: 'Accents are garbled: <span class="in-v">Ã©</span> where <span class="in-v">é</span> was meant',
        sub: 'The text was read as Windows-1252 somewhere upstream though it was UTF-8. Clean up repairs it.', acts: [{ label: 'Clean up…', run: openClean }] });
    const spaced = S.cols.filter(s => s.raw);
    if (spaced.length) {
        const n = spaced.reduce((a, s) => a + s.raw, 0);
        inAdd({ sev: 'info', icon: 'space', score: 20, title: `${inN(S, n)} cells hold stray spaces or invisible characters`,
            sub: `In ${spaced.slice(0, 4).map(s => inC(t, s.c)).join(', ')}${spaced.length > 4 ? ` and ${inPl(spaced.length - 4, 'other column')}` : ''}: spaces at the ends or doubled, no-break spaces, zero-width characters. Two values that look the same may then not be.`,
            acts: [{ label: 'Clean up…', run: openClean }] });
    }
}

/* Empty and constant columns. */
async function inColumns(t, S, run) {
    const exact = async (c, fn) => { const m = new Map(); await inEvery(t, r => { const v = cellStr(cellOf(r, c)).trim(); m.set(v, (m.get(v) || 0) + 1); }); return fn(m); };
    let empty = S.cols.filter(s => !s.filled).map(s => s.c);
    if (S.sampled && empty.length) { const still = []; for (const c of empty) if (await exact(c, m => m.size === 1 && m.has(''))) still.push(c); empty = still; }
    if (empty.length) inAdd({ sev: 'info', icon: 'empty', score: 30,
        title: empty.length === 1 ? `${inC(t, empty[0])} is empty on every row` : `${fmt(empty.length)} columns are empty on every row`,
        sub: empty.length > 1 ? empty.slice(0, 8).map(c => inC(t, c)).join(' ') + (empty.length > 8 ? ' …' : '') : 'Clean up can delete the empty columns.',
        acts: [{ label: 'Clean up…', run: openClean }] });
    const consts = [];
    for (const s of S.cols) if (s.filled === S.n && s.distinct === 1 && !s.over) {
        if (S.sampled && !await exact(s.c, m => m.size === 1)) continue;
        consts.push(s);
    }
    if (consts.length) inAdd({ sev: 'info', icon: 'rel', score: 10,
        title: consts.length === 1 ? `${inC(t, consts[0].c)} holds ${inV(consts[0].counts.keys().next().value)} on every row` : `${fmt(consts.length)} columns hold one same value on every row`,
        sub: consts.length > 1 ? consts.slice(0, 6).map(s => `${inC(t, s.c)} ${inV(s.counts.keys().next().value)}`).join(' · ') : 'It says nothing about any row: a filter or a sort on it does nothing.' });
}
/* The key: filled and different on every row (checked on every row when sampled); an id-like
   name first. Near keys: as good but for a few values that come back. Last of the steps: its
   passes over the whole file are the slowest. */
async function inKeys(t, S, run) {
    const exact = async (c, fn) => { const m = new Map(); await inEvery(t, r => { const v = cellStr(cellOf(r, c)).trim(); m.set(v, (m.get(v) || 0) + 1); }); return fn(m); };
    const cands = S.cols.filter(s => s.filled >= S.n * .98 && (s.over || s.distinct >= s.filled * .9) && s.kind !== 'd' && S.n >= 20)
        .sort((a, b) => (inIdLike(t.headers[b.c]) - inIdLike(t.headers[a.c])) || a.c - b.c);
    /* Decided on the rows read. The whole file is read again — once at most — to confirm a key
       (a sample may miss the one value that comes back) or to count an identifier's repeats, which
       a sample all but hides: both rows of a pair must be in it. */
    let keyed = false, passes = 0;
    for (const s of cands.slice(0, 6)) {
        const name = t.headers[s.c], idLike = inIdLike(name);
        let all = s.counts, filled = s.filled, rows = S.n;
        if (s.over) { all = new Map(); for (const v of s.v) if (v) all.set(v, (all.get(v) || 0) + 1); }   // past IN_DISTINCT: counted for this one
        let rep = [...all].filter(([, k]) => k > 1);
        const unique = !rep.length && filled === rows;
        if (unique ? keyed : !rep.length) continue;
        /* Amounts, dates and free text are all different by chance; an identifier, a code or an
           e-mail is meant to be. */
        if (!unique && !idLike && (s.kind !== 't' || [...all.keys()].slice(0, 200).some(v => /\s/.test(v)))) continue;
        if (S.sampled) {
            if (passes >= 1 || !(unique || idLike)) continue;
            passes++;
            all = await exact(s.c, m => m); filled = S.total - (all.get('') || 0); all.delete(''); rows = S.total;
            rep = [...all].filter(([, k]) => k > 1);
        }
        const repRows = rep.reduce((a, [, k]) => a + k, 0);
        if (!rep.length && filled === rows) {
            inAdd({ sev: 'info', icon: 'key', score: 40, title: `${inC(t, s.c)} is filled and different on every row: it identifies them`,
                sub: 'A key: what to match rows on in Look up, Compare or Remove duplicates.' });
            keyed = true;
            continue;
        }
        if (!rep.length || repRows > Math.max(2, filled * (idLike ? .2 : .02))) continue;   // a contact list may hold one contact in ten twice
        rep.sort((a, b) => b[1] - a[1]);
        inAdd({ sev: 'warn', icon: 'key', score: 70,
            title: `${inC(t, s.c)} identifies every row but ${inPl(rep.length, 'value')} that come${rep.length === 1 ? 's' : ''} back`,
            sub: `${rep.slice(0, 3).map(([v, k]) => `${inV(v)} on ${fmt(k)} rows`).join(', ')}${rep.length > 3 ? ', …' : ''} — ${inPl(repRows, 'row')} in all.`,
            show: async () => {
                const m = new Map(); await inEvery(t, r => { const v = cellStr(cellOf(r, s.c)).trim(); if (v) m.set(v, (m.get(v) || 0) + 1); });
                const mk = inMarks(t, `sharing a ${name}`, 'Also on another row');
                await inEvery(t, r => { const v = cellStr(cellOf(r, s.c)).trim(); if (v && m.get(v) > 1) mk.add(r, s.c, v); });
                return mk;
            }, showLabel: 'Show the rows', acts: [{ label: 'Remove duplicates…', run: openDedupe }] });
    }
}

/* Values written otherwise than the rest of their column. A value's shape: a run of digits one 9
   (each digit one in a column of codes, where 7501 is not 75011; elsewhere 1/2/2024 and 01/02/2024
   are alike), a run of letters by its case, spaces as one, punctuation as is. A column whose shape is shared by
   most values (IN_SHAPE_MIN) and not by a few: those few. Letters only: just the case (names
   vary in shape, Jean-Pierre is not a misfit, DUPONT among Dupont is). Numbers: the decimal mark. */
const IN_SHAPE_MIN = .85, IN_SHAPE_ODD = .05, IN_SHAPE_CASE = .12, IN_SHAPE_ROWS = 20000;
function inShape(v, runs) {
    /* Letters by runs: Aa (a capital then lower case), A, a, x (other scripts, mixed) — one
       control character each, as the shape is only ever compared. */
    let s = (v.length > 60 ? v.slice(0, 60) : v).replace(/\p{Lu}\p{Ll}+/gu, '\x01').replace(/\p{Lu}+/gu, '\x02').replace(/\p{Ll}+/gu, '\x03').replace(/\p{L}+/gu, '\x04');
    s = runs ? s.replace(/\d+/g, '9') : s.replace(/\d/g, '9');
    return s.replace(/\s+/g, ' ');
}
const inCaseless = s => s.replace(/[\x01-\x04]/g, 'a');
const IN_DEC = v => /^[-+]?[\d\s\u00a0\u202f.]*\d,(\d{1,2}|\d{4,})$/.test(v) ? ',' : /^[-+]?[\d\s\u00a0\u202f,]*\d\.(\d{1,2}|\d{4,})$/.test(v) ? '.' : '';
async function inShapes(t, S, run) {
    for (const s of S.cols) {
        if (s.filled < 20 || s.distinct < 3) continue;
        const name = t.headers[s.c];
        const codes = s.kind === 'n' && (inCodeLike(name) || s.v.some(v => /^0\d/.test(v))) && !s.v.some(v => /[.,]\d/.test(v) && !/^0\d/.test(v));
        if (s.kind === 'n' && !codes) {             // a quantity: only its decimal mark
            const k = { ',': [], '.': [] };
            for (const v of s.v) if (v) { const d = IN_DEC(v); if (d) k[d].push(v); }
            const [maj, min] = k[','].length >= k['.'].length ? [',', '.'] : ['.', ','];
            if (!k[min].length || k[min].length > (k[maj].length + k[min].length) * .1) continue;
            inAdd({ sev: 'warn', icon: 'shape', score: 55,
                title: `${inC(t, s.c)}: ${inNs(S, k[min].length, 'number')} written with a ${min === ',' ? 'comma' : 'point'} where the others use a ${maj === ',' ? 'comma' : 'point'}`,
                sub: `${inV(k[min][0])} among values like ${inV(k[maj][0])}. A program reading one convention misreads the other — 12.50 may become 1250.`,
                show: async () => { const mk = inMarks(t, `with a ${min === ',' ? 'decimal comma' : 'decimal point'} in ${name}`, 'Usual form'); await inEvery(t, r => { const v = cellStr(cellOf(r, s.c)).trim(); if (v && IN_DEC(v) === min) mk.add(r, s.c, k[maj][0]); }); return mk; } });
            continue;
        }
        let len = 0, n = 0;
        for (const v of s.v) if (v) { len += v.length; if (++n >= 2000) break; }
        if (len / n > 40) continue;                 // free text
        /* Digits one by one only in codes (75011 vs 7501): elsewhere a run is a run — e-mails, lot
           numbers and dates hold as many digits as they need. */
        const runs = !codes, shapes = new Map(), ex = new Map();
        /* The usual shape from IN_SHAPE_ROWS values; the rest are read only for a column that has one. */
        const count = vs => { for (const v of vs) if (v) { const sh = inShape(v, runs); shapes.set(sh, (shapes.get(sh) || 0) + 1); if (!ex.has(sh)) ex.set(sh, v); } };
        const head = s.v.length > IN_SHAPE_ROWS ? s.v.slice(0, IN_SHAPE_ROWS) : s.v;
        count(head);
        let dom = '', dn = 0, seen = 0;
        for (const [sh, k] of shapes) { seen += k; if (k > dn) { dom = sh; dn = k; } }
        if (dn / seen < IN_SHAPE_MIN || dn === seen && head === s.v) continue;
        if (head !== s.v) {
            shapes.clear(); count(s.v);
            dn = shapes.get(dom) || 0;
            if (dn / s.filled < IN_SHAPE_MIN || dn === s.filled) continue;
        }
        const digits = /9/.test(dom) || s.kind === 'd';
        const odd = sh => sh !== dom && (digits || inCaseless(sh) === inCaseless(dom));
        const odds = [...shapes].filter(([sh]) => odd(sh)).sort((a, b) => b[1] - a[1]);
        const m = odds.reduce((a, [, k]) => a + k, 0);
        if (!m || m > s.filled * (digits ? IN_SHAPE_ODD : IN_SHAPE_CASE)) continue;
        if (codes && s.distinct === s.filled) continue;   // numbered rows (1 … 10 000), not codes of one width
        const caseOnly = !digits;
        inAdd({ sev: 'warn', icon: 'shape', score: 60 - 20 * caseOnly,
            title: caseOnly ? `${inC(t, s.c)}: ${inNs(S, m, 'value')} in another case than the others` : `${inC(t, s.c)}: ${inNs(S, m, 'value')} written otherwise than the others`,
            sub: `${odds.slice(0, 3).map(([sh]) => inV(ex.get(sh))).join(', ')}${odds.length > 3 ? ', …' : ''} where most look like ${inV(ex.get(dom))}.`
                + (digits && /^9+$/.test(dom) && odds.some(([sh]) => /^9+$/.test(sh) && sh.length < dom.length) ? ' Leading zeros lost on the way?' : ''),
            show: async () => {
                const mk = inMarks(t, `written otherwise in ${name}`, 'Usual form');
                await inEvery(t, r => { const v = cellStr(cellOf(r, s.c)).trim(); if (v && odd(inShape(v, runs))) mk.add(r, s.c, ex.get(dom)); });
                return mk;
            } });
        await inYield(run);
    }
}

/* Numbers and dates far from all the others: beyond 3 interquartile ranges of the quartiles
   (an extreme outlier for Tukey), a handful at most — a long tail of amounts is no finding. */
function inDays(k) { const y = Math.floor(k / 1e10), mo = Math.floor(k / 1e8) % 100, d = Math.floor(k / 1e6) % 100; return Date.UTC(y, mo - 1, d) / 864e5 + (k % 1e6) / 1e6; }
async function inFar(t, S, run) {
    for (const s of S.cols) {
        if ((s.kind !== 'n' && s.kind !== 'd') || s.filled < 30) continue;
        const name = t.headers[s.c], isD = s.kind === 'd';
        const key = v => { if (!v) return NaN; if (isD) { const k = dateKey(v); return isNaN(k) ? NaN : inDays(k); } return isNumericLike(v) ? numKey(v) : NaN; };
        const xs = [];
        for (const v of s.v) { const x = key(v); if (!isNaN(x)) xs.push(x); }
        if (xs.length < 30) continue;
        const ints = !isD && xs.every(Number.isInteger);
        if (ints && s.distinct === s.filled) continue;   // identifiers
        const q = Float64Array.from(xs).sort(), at = p => q[Math.min(q.length - 1, Math.floor(p * (q.length - 1)))];
        const q1 = at(.25), q3 = at(.75), iqr = q3 - q1;
        if (iqr > 0) {
            const lo = q1 - 3 * iqr, hi = q3 + 3 * iqr, far = [];
            s.v.forEach(v => { const x = key(v); if (!isNaN(x) && (x < lo || x > hi)) far.push([x, v]); });
            if (far.length && far.length <= Math.max(1, xs.length * .01)) {
                far.sort((a, b) => Math.abs(b[0] - (q1 + q3) / 2) - Math.abs(a[0] - (q1 + q3) / 2));
                const p05 = q[Math.floor(.05 * (q.length - 1))], p95 = q[Math.floor(.95 * (q.length - 1))];
                const wrote = x => { const v = s.v.find(v => key(v) === x); return v == null ? String(x) : v; };
                inAdd({ sev: 'warn', icon: 'far', score: 50,
                    title: `${inC(t, s.c)}: ${inNs(S, far.length, isD ? 'date' : 'value')} far from all the others`,
                    sub: `${far.slice(0, 3).map(([, v]) => inV(v)).join(', ')}${far.length > 3 ? ', …' : ''}, where most lie between ${inV(wrote(p05))} and ${inV(wrote(p95))}.`,
                    show: async () => { const mk = inMarks(t, `far from the others in ${name}`, 'Most lie between'); const range = `${wrote(p05)} and ${wrote(p95)}`;
                        await inEvery(t, r => { const v = cellStr(cellOf(r, s.c)).trim(); const x = key(v); if (!isNaN(x) && (x < lo || x > hi)) mk.add(r, s.c, range); }); return mk; } });
            }
        }
        if (!isD) {
            const neg = xs.filter(x => x < 0).length;
            if (neg && neg <= Math.max(1, xs.length * .01) && xs.length - neg >= 50) inAdd({ sev: 'warn', icon: 'far', score: 45,
                title: `${inC(t, s.c)}: ${inNs(S, neg, 'negative number')} among positive ones`,
                sub: `${s.v.filter(v => key(v) < 0).slice(0, 3).map(inV).join(', ')} — a credit note, or a sign typed by mistake?`,
                show: async () => { const mk = inMarks(t, `below zero in ${name}`, 'The others are'); await inEvery(t, r => { const x = key(cellStr(cellOf(r, s.c)).trim()); if (x < 0) mk.add(r, s.c, '0 or more'); }); return mk; } });
        }
        await inYield(run);
    }
}

/* Rows that are exact copies of an earlier one. A row still its record is hashed from its bytes
   (a Set of 600 000 decoded records took 1.6 s) and compared byte for byte
   with the first record of the same hash; an edited or added row by its cells. Blank rows are
   Clean up's business, not copies. */
const IN_BLANK = new Uint8Array(256);
for (const ch of ' \t\r\n";,|') IN_BLANK[ch.charCodeAt(0)] = 1;
function inCopyKey(base) {
    const u8 = base.u8, st = base.starts, first = new Map(), cells = new Map();
    const span = b => { let e = st[b + 1]; while (e > st[b] && (u8[e - 1] === 10 || u8[e - 1] === 13)) e--; return [st[b], e]; };
    const same = (a, b) => { const [a0, a1] = span(a), [b0, b1] = span(b); if (a1 - a0 !== b1 - b0) return false; for (let i = 0; i < a1 - a0; i++) if (u8[a0 + i] !== u8[b0 + i]) return false; return true; };
    /* true: a copy of a row seen before; false: the first of its kind or blank. */
    return r => {
        if (r.d || r.b < 0) { const k = r.data.join('\x1f'); if (!k.replace(/[\x1f\s]/g, '')) return false; if (cells.has(k)) return true; cells.set(k, 1); return false; }
        const [a, e] = span(r.b);
        let h = 0x811c9dc5, blank = true;
        for (let i = a; i < e; i++) { const x = u8[i]; if (blank && !IN_BLANK[x]) blank = false; h = Math.imul(h ^ x, 16777619); }
        if (blank) return false;
        h = (h >>> 0) * 2097152 + ((e - a) & 2097151);   // FNV-1a and the length
        const prev = first.get(h);
        if (prev == null) { first.set(h, r.b); return false; }
        if (same(prev, r.b)) return true;
        return false;                         // another record of the same hash: too rare to keep a list for
    };
}
async function inCopies(t, S, run) {
    if (!t.base || !t.base.u8) return;
    const isCopy = inCopyKey(t.base), all = t.allData;
    let n = 0, last = performance.now();
    for (let i = 0; i < all.length; i++) {
        if (isCopy(all[i])) n++;
        if ((i & 8191) === 8191 && performance.now() - last > 12) { inProgress(.25 + .75 * (6 + i / all.length) / 8); await inYield(run); last = performance.now(); }
    }
    if (!n) return;
    inAdd({ sev: 'warn', icon: 'dup', score: 75, title: `${inPl(n, 'row')} ${n === 1 ? 'is an exact copy' : 'are exact copies'} of an earlier row`,
        sub: 'Every field the same. Remove duplicates keeps one of each, or finds the rows that differ only by case, spaces or spelling.',
        show: async () => { const mk = inMarks(t, 'copying an earlier row'), is = inCopyKey(t.base); for (const r of t.allData) if (is(r)) mk.add(r); return mk; },
        showLabel: 'Show the copies', acts: [{ label: 'Remove duplicates…', run: openDedupe }] });
}

/* Hidden arithmetic between numeric columns: B = A × k (k from the most common ratio, written
   with 4 significant digits), C = A + B. Checked to half a unit of the result's last decimal, so
   amounts rounded to the cent still agree. On every row, or on ≥ 90 % of them: the others. */
async function inArithmetic(t, S, run) {
    const n = Math.min(S.n, IN_REL_ROWS);
    const cols = S.cols.filter(s => s.kind === 'n' && s.filled >= 20 && s.distinct >= 5 && !(s.distinct === s.filled && inIdLike(t.headers[s.c]))).slice(0, IN_REL_COLS);
    if (cols.length < 2) return;
    const dec = v => { const m = v.match(/[.,](\d+)$/); return m && !/^\d{3}$/.test(m[1]) ? m[1].length : 0; };
    const X = cols.map(s => Float64Array.from({ length: n }, (_, i) => isNumericLike(s.v[i]) ? numKey(s.v[i]) : NaN));
    const TOL = cols.map(s => Float64Array.from({ length: n }, (_, i) => .5 * 10 ** -dec(s.v[i] || '') + 1e-9));
    const found = [], kTxt = k => k.toLocaleString('fr-FR', { maximumSignificantDigits: 4 });
    const report = (rel, ok, both, cs) => {
        if (both < 20 || ok / both < .9) return;
        found.push({ rel, bad: both - ok, both, cs });
    };
    /* B after A in the file: a derived column usually comes after what it is computed from (HT,
       TVA, TTC), so TVA = HT × 0,2 rather than HT = TVA × 5. */
    for (let a = 0; a < cols.length; a++) for (let b = a + 1; b < cols.length; b++) {
        const A = X[a], B = X[b], ratios = new Map();
        let both = 0;
        for (let i = 0; i < n; i++) if (A[i] && B[i] && isFinite(A[i]) && isFinite(B[i])) { both++; const r = Number((B[i] / A[i]).toPrecision(4)); ratios.set(r, (ratios.get(r) || 0) + 1); }
        let k = 0, kn = 0;
        for (const [r, m] of ratios) if (m > kn) { k = r; kn = m; }
        if (!both || kn / both < .5 || k <= 0) continue;
        let ok = 0;
        for (let i = 0; i < n; i++) if (A[i] && B[i] && isFinite(A[i]) && isFinite(B[i]) && Math.abs(B[i] - A[i] * k) <= TOL[b][i] + 1e-9 * Math.abs(B[i])) ok++;
        report({ kind: 'mul', a: cols[a].c, b: cols[b].c, k, kt: kTxt(k) }, ok, both, [cols[a].c, cols[b].c]);
    }
    await inYield(run);
    if (cols.length <= 12) for (let a = 0; a < cols.length; a++) for (let b = a + 1; b < cols.length; b++) for (let c = 0; c < cols.length; c++) {
        if (c === a || c === b) continue;
        const A = X[a], B = X[b], C = X[c];
        let both = 0, ok = 0, nzA = 0, nzB = 0;
        for (let i = 0; i < n; i++) {
            if (!isFinite(A[i]) || !isFinite(B[i]) || !isFinite(C[i]) || isNaN(A[i]) || isNaN(B[i]) || isNaN(C[i])) continue;
            both++; if (A[i]) nzA++; if (B[i]) nzB++;
            if (Math.abs(C[i] - A[i] - B[i]) <= TOL[c][i] + 1e-9 * Math.abs(C[i])) ok++;
        }
        if (nzA < both / 2 || nzB < both / 2) continue;   // C = A + 0 is no sum
        report({ kind: 'add', a: cols[a].c, b: cols[b].c, c: cols[c].c }, ok, both, [cols[a].c, cols[b].c, cols[c].c]);
    }
    /* The surest first, a sum before a product (TTC = HT + TVA says more than TTC = TVA × 6); a
       rule whose columns are all in rules already kept says nothing new. */
    found.sort((x, y) => (x.bad / x.both) - (y.bad / y.both) || (x.rel.kind === 'add' ? 0 : 1) - (y.rel.kind === 'add' ? 0 : 1) || (x.rel.kt || '').length - (y.rel.kt || '').length);
    const covered = new Set(), kept = [];
    for (const f of found) { if (f.cs.every(c => covered.has(c))) continue; f.cs.forEach(c => covered.add(c)); kept.push(f); if (kept.length >= 4) break; }
    kept.forEach(f => {
        const r = f.rel, H = c => inC(t, c);
        const what = r.kind === 'mul' ? (r.k === 1 ? `${H(r.b)} holds the same number as ${H(r.a)}` : `${H(r.b)} = ${H(r.a)} × ${esc(r.kt)}`) : `${H(r.c)} = ${H(r.a)} + ${H(r.b)}`;
        const plain = r.kind === 'mul' ? (r.k === 1 ? `${t.headers[r.b]} = ${t.headers[r.a]}` : `${t.headers[r.b]} = ${t.headers[r.a]} × ${r.kt}`) : `${t.headers[r.c]} = ${t.headers[r.a]} + ${t.headers[r.b]}`;
        const target = r.kind === 'mul' ? r.b : r.c;
        const expect = rw => {
            const g = c => { const v = cellStr(cellOf(rw, c)).trim(); return v && isNumericLike(v) ? numKey(v) : NaN; };
            const A = g(r.a), B = g(r.b);
            if (r.kind === 'mul') { if (!A || !B || isNaN(A) || isNaN(B)) return null; const want = A * r.k; return Math.abs(B - want) <= .5 * 10 ** -dec(cellStr(cellOf(rw, r.b)).trim()) + 1e-9 * Math.abs(B) + 1e-9 ? null : want; }
            const C = g(r.c); if ([A, B, C].some(isNaN)) return null;
            return Math.abs(C - A - B) <= .5 * 10 ** -dec(cellStr(cellOf(rw, r.c)).trim()) + 1e-9 * Math.abs(C) + 1e-9 ? null : A + B;
        };
        inAdd({ sev: f.bad ? 'warn' : 'info', icon: 'rel', score: f.bad ? 65 : 35,
            title: f.bad ? `${what} — except on ${inNs(S, f.bad * S.n / n, 'row')}` : `${what}, on every row`,
            sub: f.bad ? `True on ${fmt(f.both - f.bad)} rows of ${fmt(f.both)} read; the others break the rule: a typo, a rate that changed, a line edited by hand?`
                : `A rule the file keeps without saying it: ${esc(plain)}${S.sampled || n < S.n ? ' on the rows read' : ''}.`,
            show: f.bad ? async () => { const mk = inMarks(t, `breaking ${plain}`, 'The rule gives'); await inEvery(t, rw => { const w = expect(rw); if (w != null) mk.add(rw, target, w.toLocaleString('fr-FR', { maximumFractionDigits: 4 })); }); return mk; } : null });
    });
}

/* A column that decides another: every value of A comes with one value of B (postal code →
   city, product → price), but for a few rows that disagree with the rest of their group. Tried
   on the rows read (IN_FD_ROWS) over pairs of columns: A repeats (2 to n/2 values), B has no
   more values than A, mostly filled; the group's B is its most common one. */
async function inDeps(t, S, run) {
    const n = Math.min(S.n, IN_FD_ROWS);
    if (n < 30) return;
    const ids = new Map();
    const idOf = s => {
        if (ids.has(s.c)) return ids.get(s.c);
        const m = new Map(), a = new Int32Array(n);
        for (let i = 0; i < n; i++) { const v = s.v[i]; if (!v) { a[i] = -1; continue; } let k = m.get(v); if (k == null) { k = m.size; m.set(v, k); } a[i] = k; }
        const r = { a, d: m.size, vals: [...m.keys()] };
        ids.set(s.c, r); return r;
    };
    const As = S.cols.filter(s => !s.over && s.distinct >= 2 && s.distinct <= n / 2 && s.filled >= S.n * .5).slice(0, IN_FD_COLS);
    const Bs = S.cols.filter(s => !s.over && s.distinct >= 2 && s.distinct < S.n * .9).slice(0, IN_FD_COLS + 10);
    const found = [], done = new Set();
    let last = performance.now();
    for (const sa of As) {
        const A = idOf(sa);
        if (A.d > n / 2 || A.d < 2) continue;
        for (const sb of Bs) {
            if (sb === sa || done.has(sb.c + ',' + sa.c)) continue;
            const B = idOf(sb);
            if (B.d > A.d * 1.1 + 2) continue;   // more values than the column deciding it: no
            const W = B.d + 1, cnt = new Map();
            for (let i = 0; i < n; i++) { const g = A.a[i]; if (g < 0) continue; const b = B.a[i] < 0 ? B.d : B.a[i], k = g * W + b; cnt.set(k, (cnt.get(k) || 0) + 1); }
            const size = new Int32Array(A.d), best = new Int32Array(A.d), bestB = new Int32Array(A.d).fill(-1);
            for (const [k, m] of cnt) { const g = Math.floor(k / W), b = k - g * W; size[g] += m; if (m > best[g]) { best[g] = m; bestB[g] = b; } }
            let cover = 0, bad = 0, groups = 0, emptyBest = 0;
            const modes = new Set();
            for (let g = 0; g < A.d; g++) if (size[g] >= 2) { groups++; cover += size[g]; bad += size[g] - best[g]; modes.add(bestB[g]); if (bestB[g] === B.d) emptyBest += size[g]; }
            /* Groups of 3 rows on average at least: with pairs, any column "decides" another by chance. */
            if (groups >= 5 && cover >= groups * 3 && cover >= n * .3 && bad > 0 && bad <= cover * .02 && bad <= groups && modes.size >= 2 && emptyBest <= cover * .2) {
                done.add(sa.c + ',' + sb.c);
                found.push({ sa, sb, A, B, size, bestB, bad, cover });
            }
            if (performance.now() - last > 12) { await inYield(run); last = performance.now(); }
        }
    }
    found.sort((x, y) => x.bad / x.cover - y.bad / y.cover).slice(0, 3).forEach(f => {
        const { sa, sb, A, B, size, bestB } = f, an = t.headers[sa.c], bn = t.headers[sb.c];
        const bv = b => b === B.d ? '' : B.vals[b];
        /* An example: the biggest group with a dissenting row. */
        let eg = -1, ei = -1;
        for (let i = 0; i < n; i++) { const g = A.a[i]; if (g < 0 || size[g] < 2) continue; const b = B.a[i] < 0 ? B.d : B.a[i]; if (b !== bestB[g] && (eg < 0 || size[g] > size[eg])) { eg = g; ei = i; } }
        const sub = eg >= 0 ? `${inV(A.vals[eg])} comes with ${inV(bv(bestB[eg]))} on ${fmt(size[eg] - 1)} rows, but ${inV(sb.v[ei])} on another.` : '';
        inAdd({ sev: 'warn', icon: 'dep', score: 68,
            title: `${inC(t, sa.c)} decides ${inC(t, sb.c)} — except on ${inNs(S, f.bad * S.n / n, 'row')}`,
            sub: sub + ' A typo, or a value updated in one place only?',
            show: async () => {
                const groups = new Map();
                await inEvery(t, r => { const a = cellStr(cellOf(r, sa.c)).trim(); if (!a) return; const b = cellStr(cellOf(r, sb.c)).trim(); let g = groups.get(a); if (!g) groups.set(a, g = new Map()); g.set(b, (g.get(b) || 0) + 1); });
                const mode = new Map();
                for (const [a, g] of groups) { let best = null, bn2 = 0, tot = 0; for (const [b, k] of g) { tot += k; if (k > bn2) { best = b; bn2 = k; } } if (tot >= 2) mode.set(a, best); }
                const mk = inMarks(t, `where ${bn} disagrees with ${an}`, `Usually, for this ${an}`);
                await inEvery(t, r => { const a = cellStr(cellOf(r, sa.c)).trim(); if (!mode.has(a)) return; const b = cellStr(cellOf(r, sb.c)).trim(); if (b !== mode.get(a)) mk.add(r, sb.c, mode.get(a)); });
                return mk;
            } });
    });
}
