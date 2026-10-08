/* ---------------------------------------------------------------
   FILL BY EXAMPLE
   Type the result wanted in an empty cell of a column — twice — and
   csvfab looks for a rule taking it from the row's other cells: words,
   parts around a separator, initials, digits, case, accents, with
   constant text between them ("M. Jean DUPONT" → "Dupont"; "Jean" +
   "Dupont" → "jean.dupont@acme.fr"; "2024-03-15" → "15/03/2024"). The
   rule's values show in the column's empty cells as grey ghosts, a bar
   says the rule in words; Tab writes them (one undo entry), Escape
   forgets them. Ctrl+E asks for it on the selected column with a single
   example — or, when nothing was typed, with the column's filled cells as
   examples, which fills a partly empty column the way the rest was.
   The search (ffSynth): every way a piece of the first example can be
   read from its row ("atoms"), then the cheapest chain of atoms and
   constant text spelling every example at once — a shortest path whose
   nodes are the positions reached in each example, so a chain straying
   from any example is never followed. Examples beyond the first few
   check the rule; one it gets wrong joins the search, which runs again.
----------------------------------------------------------------*/
const FF_SYNTH_EX = 6;            // examples the search starts from
const FF_MAX_EX = 40;             // examples taken at most
const FF_LEN_MAX = 200;           // longer examples and source values are not searched
const FF_COLS_MAX = 200;          // source columns looked at
const FF_NODES = 20000;           // shortest-path nodes expanded at most
const FF_PARTS_MAX = 12;          // parts of a value split on a separator, at most
/* var, not let: updateSaveBtn() and updateStats() (15-…) read ffx through ffCheck(), and render()
   ffCol, during the boot run — before this file's top level, where a let would still throw. */
var ffx = null;                   // the rule on offer: { t, col, name, prog, stamp, top, n, auto }
var ffCol = -1;                   // its column (cellHtml's one test per cell), −1 without one

const FF_TOK = { s: /\S+/gu, w: /\p{L}+/gu, a: /[\p{L}\p{N}]+/gu, n: /\p{N}+/gu };
const FF_CASES = ['as', 'up', 'low', 'cap', 'nal', 'nau'];
function ffCase(s, cs) {
    switch (cs) {
        case 'up': return s.toUpperCase();
        case 'low': return s.toLowerCase();
        case 'cap': return s.toLowerCase().replace(/(^|[^\p{L}\p{N}'’])(\p{L})/gu, (m, a, b) => a + b.toUpperCase());
        case 'nal': return removeAccents(s).toLowerCase();
        case 'nau': return removeAccents(s).toUpperCase();
        default: return s;
    }
}
/* The piece an atom reads from a value (trimmed), before case and slice; null when the value
   has no such piece (no third word, no "@"). Kinds: all — the value; dig — its digits; t — the
   i-th token of class x (from the end when i < 0); f — the i-th part split on x; fr — the parts
   from the i-th on; fl — the parts up to the i-th. */
function ffBase(s, a) {
    if (a.k === 'all') return s;
    if (a.k === 'dig') return s.replace(/\D+/g, '');
    const p = a.k === 't' ? (s.match(FF_TOK[a.x]) || []) : s.split(a.x), n = p.length, i = a.i < 0 ? n + a.i : a.i;
    if (i < 0 || i >= n) return null;
    if (a.k === 't') return p[i];
    if (n < 2) return null;                   // the separator is not in the value
    if (a.k === 'f') return p[i].trim();
    if (a.k === 'fr') return i < 1 ? null : p.slice(i).join(a.x).trim();
    return i > n - 2 ? null : p.slice(0, i + 1).join(a.x).trim();   // fl
}
/* An atom's text from a source value: the piece, in its case, then its first or last sl characters. */
function ffAtom(s, a) {
    const b = ffBase(s, a);
    if (!b) return null;
    const v = ffCase(b, a.cs);
    return a.sl ? (a.sl > 0 ? v.slice(0, a.sl) : v.slice(a.sl)) : v;
}
const ffSrc = (r, c) => cellStr(cellOf(r, c)).trim();
/* A rule's value for a row: null when one of its atoms finds nothing there. */
function ffRun(prog, r) {
    let out = '';
    for (const p of prog) {
        if (p.txt != null) { out += p.txt; continue; }
        const v = ffAtom(ffSrc(r, p.c), p);
        if (!v) return null;
        out += v;
    }
    return out;
}

/* Every atom of row r whose text appears somewhere in the example O. */
function ffAtoms(t, col, r, O) {
    const atoms = [];
    for (let c = 0; c < Math.min(t.headers.length, FF_COLS_MAX); c++) {
        if (c === col) continue;
        const s = ffSrc(r, c);
        if (!s || s.length > FF_LEN_MAX) continue;
        const descs = [{ k: 'all' }];
        if (/\d/.test(s) && /\D/.test(s)) descs.push({ k: 'dig' });
        for (const x of ['s', 'w', 'a', 'n']) {
            const p = s.match(FF_TOK[x]) || [];
            if (p.length > FF_PARTS_MAX * 2) continue;
            for (let i = 0; i < p.length; i++) { descs.push({ k: 't', x, i }); descs.push({ k: 't', x, i: i - p.length }); }
        }
        for (const x of new Set(s.match(/[^\p{L}\p{N}]/gu) || [])) {
            const n = s.split(x).length;
            if (n < 2 || n > FF_PARTS_MAX) continue;
            for (let i = 0; i < n; i++) {
                if (!/\s/.test(x)) { descs.push({ k: 'f', x, i }); descs.push({ k: 'f', x, i: i - n }); }
                if (i >= 1) { descs.push({ k: 'fr', x, i }); descs.push({ k: 'fr', x, i: i - n }); }
                if (i <= n - 2) { descs.push({ k: 'fl', x, i }); descs.push({ k: 'fl', x, i: i - n }); }
            }
        }
        for (const d of descs) {
            const b = ffBase(s, d);
            if (!b) continue;
            /* Every case, even one that changes nothing here: "DUPONT" as is and in upper case are
               the same text, but not on the next row's "Martin". */
            for (const cs of FF_CASES) {
                const v = ffCase(b, cs);
                if (O.includes(v)) atoms.push({ c, ...d, cs, sl: 0 });
                for (let n = 1; n <= Math.min(4, v.length - 1); n++) {
                    if (O.includes(v.slice(0, n))) atoms.push({ c, ...d, cs, sl: n });
                    if (O.includes(v.slice(-n))) atoms.push({ c, ...d, cs, sl: -n });
                }
            }
        }
    }
    return atoms;
}
/* What a piece of the rule costs: the plainest reading wins (the whole value, then a word, then
   a part, …; a case or a slice costs more), and constant text by its characters, a letter or a
   digit far more than punctuation — so what the row holds is read from it rather than written as
   a constant: with one example, "15/03/" + the year would otherwise beat three parts of a date. */
function ffCost(a) {
    let k = 1 + ({ all: 0, dig: .4, t: 0, f: .25, fr: .35, fl: .35 })[a.k];
    if (a.k === 't') k += ({ s: .1, w: .15, a: .2, n: .15 })[a.x];
    if (a.i != null) k += .03 * (a.i < 0 ? -a.i - 1 : a.i);
    if (a.cs !== 'as') k += a.cs === 'nal' || a.cs === 'nau' ? .3 : .2;
    if (a.sl) k += .4 + .05 * Math.abs(a.sl);
    return k;
}
function ffConstCost(s) {
    let k = 1;
    for (const ch of s) k += /[\p{L}\p{N}]/u.test(ch) ? 1.5 : .15;
    return k;
}

/* The cheapest rule spelling every example: Dijkstra over the positions reached in each example
   (plus: an atom used yet, the last piece a constant — two constants in a row are one). */
function ffSearch(t, col, exs) {
    const O = exs.map(x => x.out), E = O.length, L0 = O[0].length;
    const atoms = [];
    for (const a of ffAtoms(t, col, exs[0].r, O[0])) {
        const v = exs.map(x => ffAtom(ffSrc(x.r, a.c), a));
        if (v.every((s, e) => s && O[e].includes(s))) { a.v = v; a.cost = ffCost(a); atoms.push(a); }
    }
    const at = new Map();                     // position in the first example → the atoms that fit there
    for (const a of atoms) for (let p = O[0].indexOf(a.v[0]); p >= 0; p = O[0].indexOf(a.v[0], p + 1)) {
        if (!at.has(p)) at.set(p, []);
        at.get(p).push(a);
    }
    const heap = [], best = new Map();
    const push = n => {
        const key = n.pos.join(',') + (n.atom ? '+' : '') + (n.k ? 'k' : '');
        if (best.has(key) && best.get(key) <= n.cost) return;
        best.set(key, n.cost);
        heap.push(n);
        for (let i = heap.length - 1; i > 0;) { const j = (i - 1) >> 1; if (heap[j].cost <= heap[i].cost) break; [heap[i], heap[j]] = [heap[j], heap[i]]; i = j; }
    };
    const pop = () => {
        const top = heap[0], last = heap.pop();
        if (heap.length) {
            heap[0] = last;
            for (let i = 0; ;) {
                const l = 2 * i + 1, r = l + 1; let m = i;
                if (l < heap.length && heap[l].cost < heap[m].cost) m = l;
                if (r < heap.length && heap[r].cost < heap[m].cost) m = r;
                if (m === i) break;
                [heap[i], heap[m]] = [heap[m], heap[i]]; i = m;
            }
        }
        return top;
    };
    push({ pos: new Array(E).fill(0), atom: false, k: false, cost: 0, prev: null, part: null });
    for (let seen = 0; heap.length && seen < FF_NODES; seen++) {
        const n = pop();
        const key = n.pos.join(',') + (n.atom ? '+' : '') + (n.k ? 'k' : '');
        if (best.get(key) < n.cost) continue;     // a cheaper way here was expanded already
        if (n.atom && n.pos.every((p, e) => p === O[e].length)) {
            const prog = [];
            for (let x = n; x.prev; x = x.prev) prog.unshift(x.part);
            return prog;
        }
        const p0 = n.pos[0];
        for (const a of at.get(p0) || []) {
            if (!a.v.every((v, e) => O[e].startsWith(v, n.pos[e]))) continue;
            push({ pos: n.pos.map((p, e) => p + a.v[e].length), atom: true, k: false, cost: n.cost + a.cost, prev: n, part: a });
        }
        if (n.k) continue;
        for (let len = 1; p0 + len <= L0; len++) {
            const s = O[0].substr(p0, len);
            if (!O.every((o, e) => o.startsWith(s, n.pos[e]))) break;   // longer ones fail too
            push({ pos: n.pos.map(p => p + len), atom: n.atom, k: true, cost: n.cost + ffConstCost(s), prev: n, part: { txt: s } });
        }
    }
    return null;
}
/* The rule for examples [{r, out}]: searched on the first few, checked on the others; one it
   gets wrong joins the search (a few rounds at most). */
function ffSynth(t, col, exs) {
    exs = exs.filter(x => x.out.length <= FF_LEN_MAX);
    if (!exs.length) return null;
    let use = exs.slice(0, FF_SYNTH_EX);
    for (let round = 0; round < 5; round++) {
        const prog = ffSearch(t, col, use);
        if (!prog) return null;
        const bad = exs.find(x => !use.includes(x) && ffRun(prog, x.r) !== x.out);
        if (!bad) return prog.map(p => p.txt != null ? p : { c: p.c, k: p.k, x: p.x, i: p.i, cs: p.cs, sl: p.sl });
        use = use.concat(bad);
    }
    return null;
}

/* ---- In the grid ---------------------------------------------- */
const ffKey = t => [t.modificationsLog.length, t.modificationsLog[t.modificationsLog.length - 1], t.headers, t.allData];
function ffLive() {
    const t = ffx && ffx.t;
    return !!t && T() === t && t.loaded && t.headers[ffx.col] === ffx.name && sameStamp(ffx.stamp, ffKey(t));
}
/* A ghost: the rule's value in an empty cell of its column (from cellHtml). */
function ffGhostHtml(t, r, c, i) {
    if (!ffx || t !== ffx.t || cellStr(c).trim() !== '') return '';
    const v = ffRun(ffx.prog, r);
    return v ? `<span class="ffg" style="--ffi:${Math.max(0, Math.min(40, i - ffx.top))}">${esc(cellShown(v, CELL_SHOWN))}</span>` : '';
}
/* An example typed: a cell of the column that was empty now holds a value (startEdit's commit). */
function ffNoteExample(t, r, c) {
    if (t.lang) return;
    const name = t.headers[c];
    if (!t.ffEx || t.ffEx.c !== c || t.ffEx.name !== name) t.ffEx = { c, name, rows: [] };
    const ex = t.ffEx.rows;
    if (!ex.includes(r)) ex.push(r);
    if (ex.length > FF_MAX_EX) ex.shift();
    clearTimeout(ffNoteExample.timer);
    ffNoteExample.timer = setTimeout(() => ffOffer(t, c, true), 0);   // after the edit's own frame
}
/* The column's filled cells shown, as examples (Ctrl+E with nothing typed). */
function ffColumnExamples(t, c) {
    const out = [];
    for (const r of t.filteredData) {
        const v = cellStr(cellOf(r, c)).trim();
        if (v) { out.push({ r, out: v }); if (out.length >= FF_MAX_EX) break; }
    }
    return out;
}
/* Looks for a rule and offers it. auto: after a typed example — needs two, and stays silent. */
function ffOffer(t, c, auto) {
    if (T() !== t || !t.loaded || t.lang || c == null || c >= t.headers.length) return;
    const typed = t.ffEx && t.ffEx.c === c && t.ffEx.name === t.headers[c] ? t.ffEx.rows : [];
    let ex = typed.map(r => ({ r, out: cellStr(cellOf(r, c)).trim() })).filter(x => x.out);   // an undo empties them
    if (auto && ex.length < 2) return;
    if (!ex.length) ex = ffColumnExamples(t, c);
    const name = t.headers[c];
    if (!ex.length) {
        toast(`Type the result you want in one or two cells of "${name}", then Ctrl+E: the rest of the column follows.`, { kind: 'info' });
        return;
    }
    const prog = ffSynth(t, c, ex);
    if (!prog) {
        if (!auto) toast(`No rule found that gives ${ex.length === 1 ? 'this example' : 'these examples'} from the other columns of the row. Type one more example and try again.`, { kind: 'warn' });
        return;
    }
    ffx = { t, col: c, name, prog, stamp: ffKey(t), top: viewRows(t)[0], n: null, auto };
    ffCol = c;
    tbody.classList.add('ff-in');
    clearTimeout(ffOffer.timer); ffOffer.timer = setTimeout(() => tbody.classList.remove('ff-in'), 1600);
    if (drawn && drawn.t === t) redrawRows(t, drawn.r0, drawn.r1);
    ffBar(); ffCount(ffx);
}
/* Counts, in slices, the empty cells shown that the rule fills. */
async function ffCount(f) {
    const { t, col, prog } = f, rows = t.filteredData;
    let n = 0, last = performance.now();
    for (let i = 0; i < rows.length; i++) {
        const r = rows[i];
        if (cellStr(cellOf(r, col)).trim() === '' && ffRun(prog, r)) n++;
        if ((i & 1023) === 1023 && performance.now() - last > 12) {
            await new Promise(res => setTimeout(res, 0));
            if (ffx !== f) return;
            last = performance.now();
        }
    }
    if (ffx !== f) return;
    f.n = n;
    if (!n) {
        ffClear();
        if (!f.auto) toast(`The rule found gives nothing for the empty cells of "${f.name}" shown.`, { kind: 'warn' });
        return;
    }
    ffBar();
}
function ffClear() {
    const f = ffx;
    ffx = null; ffCol = -1;
    const bar = document.getElementById('ff-bar');
    if (bar) bar.classList.remove('on');
    if (f && drawn && drawn.t === f.t && T() === f.t) redrawRows(f.t, drawn.r0, drawn.r1);
}
/* Called wherever the tab or its rows may have changed (updateSaveBtn, updateStats): a rule
   offered for other rows, another column or another tab goes. */
function ffCheck() { if (ffx && !ffLive()) ffClear(); }
function ffDismiss() {
    const t = ffx && ffx.t;
    if (t) t.ffEx = null;                     // forgotten: two new examples bring a new offer
    ffClear();
}
function ffAccept() {
    if (!ffLive()) { ffClear(); return; }
    const { t, col, prog } = ffx, changes = [];
    for (const r of t.filteredData) {
        if (cellStr(cellOf(r, col)).trim() !== '') continue;
        const v = ffRun(prog, r);
        if (v) changes.push([r, col, v]);
    }
    ffx = null; ffCol = -1;
    t.ffEx = null;
    document.getElementById('ff-bar').classList.remove('on');
    const n = setCells(t, changes, 'filled by example');
    render(); if (!n) selStats(t);
}

/* The rule in words: one chip per piece. */
function ffOrd(i) {
    if (i < 0) return i === -1 ? 'last' : i === -2 ? 'second to last' : `${-i}th from last`;
    return ['first', 'second', 'third'][i] || `${i + 1}th`;
}
function ffDesc(a) {
    const q = x => `“${x === '\t' ? 'tab' : x === ' ' ? 'space' : x}”`, parts = [];
    if (a.k === 'dig') parts.push('digits');
    else if (a.k === 't') parts.push(`${ffOrd(a.i)} ${a.x === 'n' ? 'number' : 'word'}`);
    else if (a.k === 'f') parts.push(a.i === 0 ? `before the first ${q(a.x)}` : a.i === -1 ? `after the last ${q(a.x)}` : `${ffOrd(a.i)} part between ${q(a.x)}`);
    else if (a.k === 'fr') parts.push(`after the ${ffOrd(a.i < 0 ? a.i : a.i - 1)} ${q(a.x)}`);
    else if (a.k === 'fl') parts.push(`before the ${ffOrd(a.i < 0 ? a.i + 1 : a.i)} ${q(a.x)}`);
    if (a.sl) parts.push(a.sl === 1 ? 'initial' : a.sl > 0 ? `first ${a.sl} characters` : a.sl === -1 ? 'last character' : `last ${-a.sl} characters`);
    const cs = { up: 'UPPER', low: 'lower', cap: 'Capitalised', nal: 'lower, no accents', nau: 'UPPER, no accents' }[a.cs];
    if (cs) parts.push(cs);
    return parts.join(' · ');
}
function ffRuleHtml(t, prog) {
    return prog.map(p => p.txt != null
        ? `<span class="ff-k" title="Text written as is">${esc(p.txt).replace(/ /g, '<i>␣</i>')}</span>`
        : `<span class="ff-a"><b>${esc(t.headers[p.c])}</b>${ffDesc(p) ? `<small>${esc(ffDesc(p))}</small>` : ''}</span>`).join('');
}
function ffBar() {
    let bar = document.getElementById('ff-bar');
    if (!bar) {
        bar = document.createElement('div');
        bar.id = 'ff-bar';
        document.body.appendChild(bar);
    }
    if (!ffx) { bar.classList.remove('on'); return; }
    const { t, name, prog, n } = ffx;
    bar.innerHTML = `<svg class="ic ff-ic" viewBox="0 0 24 24"><path d="M12 3l1.9 5.6L19.5 10.5l-5.6 1.9L12 18l-1.9-5.6L4.5 10.5l5.6-1.9z"></path><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"></path></svg>`
        + `<span class="ff-t">Fill <b>${esc(name)}</b> by example</span>`
        + `<span class="ff-rule">${ffRuleHtml(t, prog)}</span>`
        + `<span class="ff-n">${n == null ? 'counting…' : `${fmt(n)} empty cell${n === 1 ? '' : 's'}`}</span>`
        + `<button class="ff-go" onclick="ffAccept()" title="Write these values (Tab)">Fill<kbd>Tab</kbd></button>`
        + `<span class="ff-x" onclick="ffDismiss()" title="Dismiss (Esc)">×</span>`;
    bar.classList.add('on');
}

/* Ctrl+E, the ☰ menu and the palette: a rule for the selected cell's column. */
function fillByExample() {
    const t = T(); if (!t || !t.loaded) return;
    if (t.lang) { toast('Fill by example works on the columns of a CSV file.', { kind: 'info' }); return; }
    if (!sel || sel.tab !== t.id) { toast('Select a cell of the column to fill first.', { kind: 'info' }); return; }
    ffOffer(t, sel.fc, false);
}
/* Ctrl+E on the grid: a rule for the selected column. Tab takes the rule on offer, Escape drops
   it — before the other Escape handlers (it is the innermost thing open), only on the grid. */
window.addEventListener('keydown', e => {
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'e') {
        const t = T();
        if (!t || !t.loaded || !gridKeysAllowed(e)) return;
        e.preventDefault();
        fillByExample();
        return;
    }
    if (!ffx || ctrl || e.altKey || !gridKeysAllowed(e)) return;
    if (!ffLive()) { ffClear(); return; }
    if (e.key === 'Tab' && !e.shiftKey) { e.preventDefault(); e.stopImmediatePropagation(); ffAccept(); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopImmediatePropagation(); ffDismiss(); }
}, true);
