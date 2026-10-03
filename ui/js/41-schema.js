/* ---------------------------------------------------------------
   SCHEMAS
   A file's rules, kept beside it as <stem>.schema.json in Table Schema
   (https://specs.frictionlessdata.io/table-schema/), so other tools can
   check the same file with it: fields [{name, type, format, constraints:
   {required, unique, enum, pattern, minimum, maximum, minLength,
   maxLength}}], primaryKey, missingValues.
   - Create: the rules a file already follows, inferred from every row and
     kept prudent (no observed min/max, which the next export would break),
     shown one by one before they are written.
   - Validate: a file against such a schema — its own, the one of the
     export it is a later copy of (customers_2026-10-04.csv finds
     customers.schema.json), or any picked by hand. Each kind of error is
     counted with examples; a click shows its rows (row marks, 19-…).
   On demand only: nothing runs when a file is opened.
----------------------------------------------------------------*/
const SCHEMA_ENUM_MAX = 15;               // distinct values for a list to be proposed…
const SCHEMA_ENUM_REPEAT = 10;            // …each seen this many times on average: a closed list, not a sample of free text
const SCHEMA_SLICE = 20000;               // rows per slice between two yields
const SCHEMA_MARK_MAX = 200000;           // rows kept per kind of error (beyond, only counted)
const SCHEMA_VALUES_MAX = 50;             // distinct values kept as hints for a list added by hand
const SCHEMA_SHAPES_MAX = 12;
const SCHEMA_SHAPE_ROWS = 100000;         // rows whose shapes are read, spread over the file: a suggestion, checked on every row once added             // value shapes kept per column (\d{5}, [A-Z]{2}\d{3}…), for a pattern added by hand
const SCHEMA_TYPES = ['string', 'integer', 'number', 'boolean', 'date', 'datetime', 'time', 'year', 'any'];
let schemaDraft = null;                   // the create dialog: { t, schema, on: Map(field → Set of rule keys) }
let schemaReport = null;                  // the last validation: { t, name, items, rows, … }

/* ---- Where a schema lives ---------------------------------------- */
function schemaStem(name) { return name.replace(/\.[^.]*$/, ''); }
/* customers_2026-10-04 → customers_2026-10-04, then customers: a dated or numbered export finds its family's schema. */
function schemaStems(name) {
    const out = [schemaStem(name)];
    let s = out[0];
    for (;;) {
        const n = s.replace(/[ _.-]*(?:\d{4}[-_.]?\d{2}[-_.]?\d{2}(?:[T _-]?\d{2}[-_.:h]?\d{2}(?:[-_.:]?\d{2})?)?|\d{6,14}|v\d{1,4}|\(\d{1,3}\)|\d{1,3})$/i, '');
        if (n === s || !n) break;
        out.push(s = n);
    }
    return out;
}
/* The schema beside the tab's file: { name, text, where } or null. */
async function schemaFind(t) {
    for (const stem of schemaStems(t.name)) {
        const name = stem + '.schema.json';
        try {
            if (t.path) {
                const p = pathDir(t.path) + name, r = await srvFetch(srvFileUrl(p));
                if (r.ok) return { name, text: await r.text(), where: p };
            } else if (t.dirHandle) {
                const fh = await t.dirHandle.getFileHandle(name);
                return { name, text: await (await fh.getFile()).text(), where: name };
            }
        } catch (e) { }
    }
    return null;
}
/* Writes the schema beside the file — through the bridge, the folder's handle, or a picker. */
async function schemaWrite(t, text) {
    const name = schemaStem(t.name) + '.schema.json';
    if (t.path) {
        const p = pathDir(t.path) + name;
        const there = await srvFetch(`/api/stat?path=${encodeURIComponent(p)}`).then(r => r.ok).catch(() => false);
        if (there && !await uiConfirm(`Replace ${name}?\n\nA schema with this name is already beside the file.`, { ok: 'Replace' })) return null;
        const r = await srvFetch(srvFileUrl(p), { method: 'PUT', body: new Blob([text], { type: 'application/json' }) });
        if (!r.ok) { uiAlert(`Could not write ${name}.\n\n${(await r.json().catch(() => ({}))).error || 'HTTP ' + r.status}`); return null; }
        return p;
    }
    try {
        let fh;
        if (t.dirHandle && await ensureWritable(t.dirHandle)) fh = await t.dirHandle.getFileHandle(name, { create: true });
        else fh = await showSaveFilePicker({ suggestedName: name, types: [{ description: 'Table Schema', accept: { 'application/json': ['.json'] } }], startIn: t.handle || 'documents' });
        const w = await fh.createWritable();
        await w.write(text); await w.close();
        return fh.name;
    } catch (e) {
        if (e && e.name !== 'AbortError') uiAlert(`Could not write ${name}.\n\n${e.message || e}`);
        return null;
    }
}

/* ---- Reading values as Table Schema does -------------------------- */
const SC_INT = /^[+-]?\d+$/, SC_LEAD0 = /^[+-]?0\d/;
const SC_EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/, SC_URI = /^[a-z][a-z0-9+.-]*:\S+$/i;
const SC_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SC_ISO_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;
const SC_ISO_DT = /^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?$/;
const SC_TIME = /^(\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?$/;
const SC_PHONE = /^\+?[\d\s().\/-]{6,24}$/;
const SC_MDAYS = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
/* A value's shape, exact and general: "AB-1234" → ["[A-Z]{2}-\\d{4}", "[A-Z]+-\\d+"] once written as
   a regex (scShapeRegex). Kept as tokens — control characters for digit, upper, lower and the
   repeats, so a "+" or "{" of the value itself stays a literal. Letters with accents count as
   letters; the classes are written so that Python's re reads them too (no \\p{…}): other tools
   use the schema. */
const SC_D = '\u0010', SC_U = '\u0011', SC_L = '\u0012', SC_N = '\u0013', SC_E = '\u0014', SC_P = '\u0015';
function scShape(s) {
    let ex = '', gen = '', prev = '', run = 0;
    const put = () => { if (!prev) return; ex += prev + (run > 1 ? SC_N + run + SC_E : ''); gen += prev + (run > 1 ? SC_P : ''); };
    for (let i = 0; i < s.length; i++) {
        const c = s.charCodeAt(i);
        let k;
        if (c >= 48 && c <= 57) k = SC_D;
        else if (c >= 65 && c <= 90) k = SC_U;
        else if (c >= 97 && c <= 122) k = SC_L;
        else if (c < 128) k = s[i];
        else { const ch = s[i]; k = ch.toLowerCase() !== ch.toUpperCase() ? (ch === ch.toUpperCase() ? SC_U : SC_L) : ch; }
        if (k === prev) run++; else { put(); prev = k; run = 1; }
    }
    put();
    return [ex, gen];
}
const SC_SHAPE_CLASS = { [SC_D]: '\\d', [SC_U]: '[A-ZÀ-ÖØ-Þ]', [SC_L]: '[a-zß-öø-ÿ]', [SC_P]: '+' };
const scLit = c => SC_SHAPE_CLASS[c] || c.replace(/[.*+?^${}()|[\]\\\/]/g, '\\$&');
function scShapeRegex(shape) {
    let out = '';
    for (let i = 0; i < shape.length; i++) {
        const c = shape[i];
        if (c === SC_N) { const j = shape.indexOf(SC_E, i); out += `{${shape.slice(i + 1, j)}}`; i = j; continue; }
        out += scLit(c);
    }
    return out;
}
/* An exact shape as runs: [[token, count]]. */
function scShapeRuns(shape) {
    const out = [];
    for (let i = 0; i < shape.length; i++) {
        if (shape[i] === SC_N) { const j = shape.indexOf(SC_E, i); out[out.length - 1][1] = +shape.slice(i + 1, j); i = j; }
        else out.push([shape[i], 1]);
    }
    return out;
}
/* Exact shapes that differ only by their counts, grouped: "Aa{4}", "Aa{9}" → one group whose
   runs go from 1 to 1 and 4 to 9 — [A-Z][a-z]{4,9}, or with any length [A-Z][a-z]+. */
function scShapeGroups(entries) {
    const groups = new Map();
    for (const [shape, n] of entries) {
        const runs = scShapeRuns(shape), key = runs.map(r => r[0]).join('');
        let g = groups.get(key);
        if (!g) groups.set(key, g = { n: 0, runs, lo: runs.map(r => r[1]), hi: runs.map(r => r[1]) });
        else runs.forEach((r, k) => { g.lo[k] = Math.min(g.lo[k], r[1]); g.hi[k] = Math.max(g.hi[k], r[1]); });
        g.n += n;
    }
    return [...groups.values()].sort((a, b) => b.n - a.n);
}
function scRangesRegex(groups, anyLength) {
    return groups.map(g => g.runs.map((r, k) => {
        const lo = g.lo[k], hi = g.hi[k];
        if (anyLength) return scLit(r[0]) + (hi > 1 ? '+' : '');
        return scLit(r[0]) + (lo === hi ? (lo > 1 ? `{${lo}}` : '') : `{${lo},${hi}}`);
    }).join('')).join('|');
}
/* Patterns that go with any text, offered beside the ones read from the values. */
const SC_PATTERN_PRESETS = [
    ['\\d+', 'digits only'], ['[A-Z0-9_-]+', 'upper-case code'], ['\\S(.*\\S)?', 'no space at either end'],
    ['\\d{5}', 'French postal code'], ['(0|\\+33 ?)[1-9]([ .-]?\\d{2}){4}', 'French phone number'],
    ['[A-Z]{2}', 'two capital letters (country code)'], ['[A-Z]{2}\\d{2}[A-Z0-9 ]{10,30}', 'IBAN'],
];
/* Checks of a text column that add up — no invisible characters AND no double spaces AND a shape.
   Table Schema gives a field one pattern and nothing else (no key of csvfab's own: the file must
   stay a valid schema for any tool), so each check is written into that pattern as a lookahead,
   (?![\s\S]*(?:bad)) — "nowhere a bad sequence" —, ahead of the shape. scSplitPattern recognises
   them when the schema is read back, to report each check in its own words. `bad` is searched
   anywhere in the value, and must mean the same to JavaScript (u flag) and Python's re. */
const SC_C1 = '\\u0080-\\u00BF\\u0152\\u0153\\u0160\\u0161\\u0178\\u017D\\u017E\\u0192\\u02C6\\u02DC\\u2013\\u2014\\u2018-\\u201A\\u201C-\\u201E\\u2020-\\u2022\\u2026\\u2030\\u2039\\u203A\\u20AC\\u2122';   // bytes 80–BF as Windows-1252 shows them
const SC_CHECKS = {        // key → [rule shown, menu entry, its detail, what the report says, bad]
    noInvisible: ['no invisible characters', 'No invisible characters', 'control characters, zero-width spaces, BOM, soft hyphens', 'holds an invisible character',
        '[\\u0000-\\u0008\\u000B\\u000C\\u000E-\\u001F\\u007F-\\u009F\\u00AD\\u200B-\\u200F\\u2028-\\u202E\\u2060-\\u2064\\u2066-\\u206F\\uFEFF]'],
    noDoubleSpace: ['no double spaces', 'No double spaces', 'two spaces in a row', 'holds two spaces in a row', ' {2}'],
    noMojibake: ['no garbled accents', 'No garbled accents', 'Ã© for é, â€™ for ’: UTF-8 read as Windows-1252', 'holds garbled accents (mojibake)', `[ÂÃÅ][${SC_C1}]|â€`],
    upperCase: ['upper case', 'Upper case', 'no lower-case letter: DUPONT, LE HAVRE', 'holds a lower-case letter', '[a-zß-öø-ÿœ]'],
    capitalised: ['capitalised', 'Capitalised', 'a capital first, then lower case: Dupont, Le Havre, Boulogne-sur-Mer', 'not capitalised (a capital first, then lower case)',
        '^[^A-ZÀ-ÖØ-ÞŒ]|[A-Za-zÀ-ÖØ-öø-ÿŒœ][A-ZÀ-ÖØ-ÞŒ]'],
};
const scCheckPart = key => `(?![\\s\\S]*(?:${SC_CHECKS[key][4]}))`;
/* The pattern written for a shape (or none) and checks. */
function scJoinPattern(base, checks) {
    if (!checks.length) return base || '';
    return checks.map(scCheckPart).join('') + (base ? `(?:${base})` : '[\\s\\S]*');
}
/* The reverse: a pattern → its checks and its shape. A pattern csvfab did not write this way is a shape alone. */
function scSplitPattern(p) {
    const checks = [];
    let rest = p || '';
    for (let again = true; again;) {
        again = false;
        for (const key of Object.keys(SC_CHECKS)) {
            const part = scCheckPart(key);
            if (!checks.includes(key) && rest.startsWith(part)) { checks.push(key); rest = rest.slice(part.length); again = true; }
        }
    }
    if (!checks.length) return { base: p || '', checks };
    const base = rest === '[\\s\\S]*' ? '' : rest.startsWith('(?:') && rest.endsWith(')') ? rest.slice(3, -1) : null;
    return base !== null && scJoinPattern(base, checks) === p ? { base, checks } : { base: p, checks: [] };
}
/* The suggestions of the rules a column can be given by hand, for its inferred type. */
function schemaHints(f, x) {
    const h = { filled: x.n / Math.max(1, x.n + x.empty), lmin: x.n ? x.lmin : null, lmax: x.n ? x.lmax : null,
        values: x.dv ? x.dv.map((v, i) => [v, x.dc[i]]).sort((a, b) => b[1] - a[1]).map(e => e[0]) : null, patterns: [] };
    if (f.type === 'integer' || f.type === 'number') { h.min = x.nminS; h.max = x.nmaxS; h.minV = x.nmin; h.maxV = x.nmax; }
    else if (f.type === 'date' && !f.format) { h.min = x.dminS; h.max = x.dmaxS; }
    else if (f.type === 'date') { const d = f.format === '%d/%m/%Y'; h.min = d ? x.dmyMinS : x.mdyMinS; h.max = d ? x.dmyMaxS : x.mdyMaxS; }
    else if (['datetime', 'time', 'year'].includes(f.type)) { h.min = x.smin; h.max = x.smax; }
    /* Best first: the shapes with the lengths seen as ranges ([A-Z][a-z]{2,9} rather than one
       alternative per length), then each exact shape, the most common one alone, any length.
       Shapes are read on a sample (SCHEMA_SHAPE_ROWS): the shares are the sample's. */
    const exact = x.shp ? [...x.shp.entries()].sort((a, b) => b[1] - a[1]) : [];
    const general = x.gshp ? [...x.gshp.entries()].sort((a, b) => b[1] - a[1]) : [];
    const seen = (exact.length ? exact : general).reduce((a, e) => a + e[1], 0);
    const groups = exact.length ? scShapeGroups(exact) : null;
    const add = (p, l) => { if (p && !h.patterns.some(e => e[0] === p)) h.patterns.push([p, l]); };
    if (exact.length === 1) add(scShapeRegex(exact[0][0]), 'every value has this shape');
    else if (groups && groups.length < exact.length) add(scRangesRegex(groups, false), groups.length === 1 ? 'the shape of the values, with the lengths seen' : `the ${groups.length} shapes of the values, with the lengths seen`);
    else if (exact.length) add(exact.map(e => scShapeRegex(e[0])).join('|'), `the ${exact.length} shapes of the values`);
    if (exact.length > 1) add(scShapeRegex(exact[0][0]), `the most common shape, ${Math.round(exact[0][1] / seen * 100)} % of the values`);
    if (groups) add(scRangesRegex(groups, true), 'the same, any length');
    else if (general.length) add(general.map(e => scShapeRegex(e[0])).join('|'), general.length === 1 ? 'the shape of the values, any length' : `the ${general.length} shapes of the values, any length`);
    return h;
}
function scDigits(s) { let n = 0; for (let i = 0; i < s.length; i++) { const c = s.charCodeAt(i); if (c >= 48 && c <= 57) n++; } return n; }
function scValidDate(y, m, d) {
    if (m < 1 || m > 12 || d < 1) return false;
    return d <= (m === 2 && ((y % 4 === 0 && y % 100 !== 0) || y % 400 === 0) ? 29 : SC_MDAYS[m]);
}
/* yyyy-mm-dd read by character: no match array, no string built — run on every date cell. */
function scIsoDate(s) {
    if (s.length !== 10 || s.charCodeAt(4) !== 45 || s.charCodeAt(7) !== 45) return NaN;
    let y = 0, m = 0, d = 0;
    for (let i = 0; i < 10; i++) {
        if (i === 4 || i === 7) continue;
        const c = s.charCodeAt(i) - 48;
        if (c < 0 || c > 9) return NaN;
        if (i < 4) y = y * 10 + c; else if (i < 7) m = m * 10 + c; else d = d * 10 + c;
    }
    return scValidDate(y, m, d) ? (y * 10000 + m * 100 + d) * 1e6 : NaN;
}
const SC_NUM = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/, SC_YEAR = /^\d{4}$/;
const SC_TRUE = ['true', 'True', 'TRUE', '1'], SC_FALSE = ['false', 'False', 'FALSE', '0'];

/* A strptime format (Table Schema's "%d/%m/%Y") → a regex and the order of its parts. */
const scFormats = new Map();
function scFormat(f) {
    let c = scFormats.get(f);
    if (c) return c;
    const parts = [], TOK = { Y: '(\\d{4})', y: '(\\d{2})', m: '(\\d{1,2})', d: '(\\d{1,2})', H: '(\\d{1,2})', M: '(\\d{2})', S: '(\\d{2})', I: '(\\d{1,2})', p: '([AaPp][Mm])' };
    let re = '';
    for (let i = 0; i < f.length; i++) {
        if (f[i] === '%' && TOK[f[i + 1]]) { parts.push(f[i + 1]); re += TOK[f[++i]]; }
        else if (f[i] === '%' && f[i + 1] === '%') { re += '%'; i++; }
        else re += f[i].replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    }
    scFormats.set(f, c = { re: new RegExp('^' + re + '$'), parts });
    return c;
}
/* A date or datetime against a strptime format: a sortable number, or NaN. */
function scParseFormat(s, f) {
    const { re, parts } = scFormat(f), m = re.exec(s);
    if (!m) return NaN;
    const v = { Y: 1970, m: 1, d: 1, H: 0, M: 0, S: 0 };
    parts.forEach((p, i) => { const n = +m[i + 1]; if (p === 'y') v.Y = n + (n < 70 ? 2000 : 1900); else if (p === 'I') v.H = n % 12; else if (p === 'p') v.pm = /p/i.test(m[i + 1]); else v[p === 'Y' ? 'Y' : p] = n; });
    if (v.pm) v.H += 12;
    if (!scValidDate(v.Y, v.m, v.d) || v.H > 23 || v.M > 59 || v.S > 59) return NaN;
    return ((((v.Y * 100 + v.m) * 100 + v.d) * 100 + v.H) * 100 + v.M) * 100 + v.S;
}
/* The field's reading of a value: a string, number, boolean or sortable date — or SC_BAD, with
   the reason in scErr. No object per call: validation runs this on every cell, and an object a
   cell (75 M of them on 5 M rows) made the collector the bottleneck — 228 s instead of 30. */
const SC_BAD = Symbol('bad');
let scErr = '';
const scBad = msg => { scErr = msg; return SC_BAD; };
function scValue(f, s) {
    switch (f.type) {
        case 'integer':
            return SC_INT.test(s) ? Number(s) : scBad('not an integer');
        case 'number': {
            let x = s;
            if (f.groupChar) x = x.split(f.groupChar).join('');
            if (f.decimalChar && f.decimalChar !== '.') { if (x.includes('.')) return scBad('not a number'); x = x.replace(f.decimalChar, '.'); }
            return SC_NUM.test(x) ? Number(x) : scBad('not a number');
        }
        case 'boolean': {
            const tv = f.trueValues || SC_TRUE, fv = f.falseValues || SC_FALSE;
            return tv.includes(s) ? true : fv.includes(s) ? false : scBad('not a boolean');
        }
        case 'date': {
            if (!f.format || f.format === 'default' || f.format === 'any') {
                const v = scIsoDate(s);
                if (v === v) return v;
                if (f.format === 'any') { const v = scParseFormat(s, '%d/%m/%Y'); if (!isNaN(v)) return v; }
                return scBad('not a date (yyyy-mm-dd)');
            }
            const v = scParseFormat(s, f.format);
            return isNaN(v) ? scBad(`not a date (${f.format})`) : v;
        }
        case 'datetime': {
            if (!f.format || f.format === 'default' || f.format === 'any') {
                const m = SC_ISO_DT.exec(s);
                if (m && scValidDate(+m[1], +m[2], +m[3]) && +m[4] < 24 && +m[5] < 60) return +(m[1] + m[2] + m[3] + m[4] + m[5] + (m[6] || '00'));
                return scBad('not a date and time (yyyy-mm-ddThh:mm:ss)');
            }
            const v = scParseFormat(s, f.format);
            return isNaN(v) ? scBad(`not a date and time (${f.format})`) : v;
        }
        case 'time': {
            const m = SC_TIME.exec(s);
            return m && +m[1] < 24 && +m[2] < 60 ? +m[1] * 3600 + +m[2] * 60 + +(m[3] || 0) : scBad('not a time (hh:mm:ss)');
        }
        case 'year':
            return SC_YEAR.test(s) ? +s : scBad('not a year');
        case 'string':
            if (f.format === 'email' && !SC_EMAIL.test(s)) return scBad('not an e-mail address');
            if (f.format === 'uri' && !SC_URI.test(s)) return scBad('not a URI');
            if (f.format === 'uuid' && !SC_UUID.test(s)) return scBad('not a UUID');
            return s;
        default:
            return s;                       // any, and the types csvfab does not check (object, array, geopoint…)
    }
}

/* ---- Inference ------------------------------------------------------
   One pass over every row; per column, what every filled value has in
   common. Prudent: a rule is proposed only when no value breaks it, and
   bounds are kept to "never negative". */
async function schemaInfer(t, progress) {
    const W = t.headers.length, rows = t.allData;
    const S = t.headers.map(() => ({ n: 0, empty: 0, int: true, lead0: false, dlen: -1, num: true, comma: true, group: false, neg: false,
        iso: true, dmy: true, mdy: true, first12: false, second12: false, isodt: true, time: true, email: true, phone: true, space: false,
        dv: [], dc: [], uniq: new Set(), uniqOk: true,        // dv/dc: the distinct values and their counts, while few (an array, compared by ===: see isMissing in schemaValidate)
        lmin: Infinity, lmax: 0, nmin: Infinity, nmax: -Infinity, nminS: '', nmaxS: '', dmin: Infinity, dmax: -Infinity, dminS: '', dmaxS: '',
        dmyMin: Infinity, dmyMax: -Infinity, dmyMinS: '', dmyMaxS: '', mdyMin: Infinity, mdyMax: -Infinity, mdyMinS: '', mdyMaxS: '', smin: null, smax: null,
        shp: new Map(), gshp: new Map() }));                  // the hints of the rules added by hand: extremes, lengths, shapes
    const NUMC = /^[+-]?\d{1,3}(?: \d{3})*(?:,\d+)?$|^[+-]?\d+(?:,\d+)?$/, NUMD = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
    const DMY = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/;
    const shapeStep = Math.max(1, Math.floor(rows.length / SCHEMA_SHAPE_ROWS));
    for (let i0 = 0; i0 < rows.length; i0 += SCHEMA_SLICE) {
        visitRows(t, rows.slice(i0, i0 + SCHEMA_SLICE), (r, i) => {
            const d = r.data, shapeRow = (i0 + i) % shapeStep === 0;
            for (let c = 0; c < W; c++) {
                const s = d[c] == null ? '' : String(d[c]), x = S[c];
                if (s === '') { x.empty++; continue; }
                x.n++;
                if (!x.space && s.includes(' ')) x.space = true;
                if (x.dv) { const j = x.dv.indexOf(s); if (j >= 0) x.dc[j]++; else if (x.dv.length < SCHEMA_VALUES_MAX) { x.dv.push(s); x.dc.push(1); } else x.dv = null; }
                if (s.length < x.lmin) x.lmin = s.length;
                if (s.length > x.lmax) x.lmax = s.length;
                if (x.smin === null || s < x.smin) x.smin = s;           // ISO dates and times sort as text
                if (x.smax === null || s > x.smax) x.smax = s;
                if (shapeRow && (x.shp || x.gshp)) {
                    const [ex, gen] = scShape(s);
                    if (x.shp) { x.shp.set(ex, (x.shp.get(ex) || 0) + 1); if (x.shp.size > SCHEMA_SHAPES_MAX) x.shp = null; }
                    if (x.gshp) { x.gshp.set(gen, (x.gshp.get(gen) || 0) + 1); if (x.gshp.size > SCHEMA_SHAPES_MAX) x.gshp = null; }
                }
                const c0 = s.charCodeAt(0), digitish = (c0 >= 48 && c0 <= 57) || c0 === 43 || c0 === 45 || c0 === 46;
                if (x.int && !(digitish && SC_INT.test(s))) x.int = false;
                if (x.int) { if (SC_LEAD0.test(s)) x.lead0 = true; const L = s.replace(/^[+-]/, '').length; x.dlen = x.dlen === -1 ? L : x.dlen === L ? L : -2; if (s[0] === '-') x.neg = true; }
                if (x.num && !(digitish && NUMD.test(s))) x.num = false;
                if (x.comma && !(digitish && NUMC.test(s))) x.comma = false; else if (x.comma && s.includes(' ')) x.group = true;
                if ((x.num || x.comma) && s[0] === '-') x.neg = true;
                if (x.int || x.num || x.comma) {
                    const v = x.int || x.num ? Number(s) : Number(s.replace(/ /g, '').replace(',', '.'));
                    if (v < x.nmin) { x.nmin = v; x.nminS = s; }
                    if (v > x.nmax) { x.nmax = v; x.nmaxS = s; }
                }
                if (x.iso) {
                    const v = digitish ? scIsoDate(s) : NaN;
                    if (v !== v) x.iso = false;
                    else { if (v < x.dmin) { x.dmin = v; x.dminS = s; } if (v > x.dmax) { x.dmax = v; x.dmaxS = s; } }
                }
                if (x.dmy || x.mdy) {
                    const m = digitish && DMY.exec(s);
                    if (!m) x.dmy = x.mdy = false;
                    else {
                        if (+m[1] > 12) x.first12 = true;
                        if (+m[2] > 12) x.second12 = true;
                        if (x.dmy && !scValidDate(+m[3], +m[2], +m[1])) x.dmy = false;
                        if (x.mdy && !scValidDate(+m[3], +m[1], +m[2])) x.mdy = false;
                        const dv = +m[3] * 10000 + +m[2] * 100 + +m[1], mv = +m[3] * 10000 + +m[1] * 100 + +m[2];
                        if (dv < x.dmyMin) { x.dmyMin = dv; x.dmyMinS = s; } if (dv > x.dmyMax) { x.dmyMax = dv; x.dmyMaxS = s; }
                        if (mv < x.mdyMin) { x.mdyMin = mv; x.mdyMinS = s; } if (mv > x.mdyMax) { x.mdyMax = mv; x.mdyMaxS = s; }
                    }
                }
                if (x.isodt) { const m = digitish && SC_ISO_DT.exec(s); if (!m || !scValidDate(+m[1], +m[2], +m[3])) x.isodt = false; }
                if (x.time && !(digitish && SC_TIME.test(s))) x.time = false;
                if (x.email && !SC_EMAIL.test(s)) x.email = false;
                if (x.phone && !(SC_PHONE.test(s) && scDigits(s) >= 6)) x.phone = false;
                /* Uniqueness is followed only while the column can still be proposed unique (an integer,
                   or a text without spaces — schemaField): a set of millions of phones or amounts cost
                   most of the pass. Integers are kept as numbers, which a Set does not hash as text. */
                if (x.uniqOk) {
                    if (x.space || (!x.int && (x.num || x.comma || x.iso || x.dmy || x.mdy || x.isodt || x.time)) || (x.intKeys && !x.int)) { x.uniqOk = false; x.uniq = null; }
                    else {
                        const key = x.int ? Number(s) : s;
                        if (x.int) x.intKeys = true;
                        if (x.uniq.has(key)) { x.uniqOk = false; x.uniq = null; } else x.uniq.add(key);
                    }
                }
            }
        });
        if (progress) { if (progress(Math.min(1, (i0 + SCHEMA_SLICE) / rows.length)) === false) return null; await new Promise(r => setTimeout(r, 0)); }
    }
    const fields = t.headers.map((name, c) => schemaField(name, S[c], rows.length));
    const schema = { fields, missingValues: [''] };
    /* What the dialog suggests for a rule added by hand; not enumerable, so never written. */
    Object.defineProperty(schema, 'hints', { value: fields.map((f, c) => schemaHints(f, S[c])) });
    const keyish = /^(id|.*[_ -]id|.*id$|code|ref|reference|key|uuid|siret|siren)$/i;
    const pk = fields.filter(f => f.constraints && f.constraints.unique && f.constraints.required);
    const best = pk.find(f => keyish.test(f.name)) || pk[0];
    if (best) schema.primaryKey = best.name;
    return schema;
}
function schemaField(name, x, rows) {
    const f = { name }, k = {};
    const lower = x.dv && x.dv.length <= 2 ? x.dv.map(v => v.toLowerCase()) : null;
    const bool = lower && x.n && lower.length <= 2 && [['true', 'false'], ['yes', 'no'], ['oui', 'non'], ['vrai', 'faux']].find(([a, b]) => lower.every(v => v === a || v === b));
    if (!x.n) f.type = 'string';
    else if (bool) {
        f.type = 'boolean';
        const vals = x.dv;
        f.trueValues = vals.filter(v => v.toLowerCase() === bool[0]);
        f.falseValues = vals.filter(v => v.toLowerCase() === bool[1]);
    }
    else if (x.int && !x.lead0) { f.type = 'integer'; if (!x.neg) k.minimum = 0; }
    else if (x.int && x.lead0) { f.type = 'string'; k.pattern = x.dlen > 0 ? `\\d{${x.dlen}}` : '\\d+'; }
    else if (x.num) { f.type = 'number'; if (!x.neg) k.minimum = 0; }
    else if (x.comma) { f.type = 'number'; f.decimalChar = ','; if (x.group) f.groupChar = ' '; if (!x.neg) k.minimum = 0; }
    else if (x.iso) f.type = 'date';
    else if (x.dmy && (!x.mdy || x.first12 || !x.second12)) { f.type = 'date'; f.format = '%d/%m/%Y'; }
    else if (x.mdy) { f.type = 'date'; f.format = '%m/%d/%Y'; }
    else if (x.isodt) f.type = 'datetime';
    else if (x.time) f.type = 'time';
    else if (x.email) { f.type = 'string'; f.format = 'email'; }
    else if (x.phone) { f.type = 'string'; k.pattern = '\\+?[0-9 ().\\/-]{6,24}'; }
    else f.type = 'string';
    if (x.n && !x.empty) k.required = true;
    /* Unique only where it means an identity: always filled, an integer or a text without spaces
       (ids, codes, e-mails). Amounts, dates or free text all distinct in one file are a coincidence. */
    if (x.n > 1 && x.uniqOk && !x.empty && (f.type === 'integer' || (f.type === 'string' && !x.space))) k.unique = true;
    if (f.type === 'string' && !f.format && !k.pattern && !k.unique && x.dv && x.dv.length > 1 && x.dv.length <= SCHEMA_ENUM_MAX && x.n >= x.dv.length * SCHEMA_ENUM_REPEAT)
        k.enum = x.dv.slice().sort((a, b) => a.localeCompare(b));
    if (Object.keys(k).length) f.constraints = k;
    return f;
}

/* ---- Validation ------------------------------------------------------ */
function schemaParse(text) {
    let s;
    try { s = JSON.parse(text); } catch (e) { return { error: `not valid JSON (${e.message})` }; }
    if (!s || !Array.isArray(s.fields)) return { error: 'no "fields" list: not a Table Schema' };
    if (s.fields.some(f => !f || typeof f.name !== 'string')) return { error: 'a field has no "name"' };
    return { schema: s };
}
/* items: one per kind of error — { field, rule, label, count, examples, rows: Set } —, plus the structure problems. */
async function schemaValidate(t, schema, progress) {
    const fields = schema.fields, H = t.headers, out = { structure: [], items: [], rows: new Set(), cells: new Map(), checked: 0 };
    const norm = s => String(s).trim().toLowerCase();
    const at = fields.map(f => { let c = H.indexOf(f.name); if (c < 0) c = H.findIndex(h => norm(h) === norm(f.name)); return c; });
    fields.forEach((f, i) => { if (at[i] < 0) out.structure.push(`missing column “${f.name}”`); });
    H.forEach((h, c) => { if (!at.includes(c)) out.structure.push(`unexpected column “${h}”`); });
    const present = at.filter(c => c >= 0);
    if (present.some((c, i) => i && c < present[i - 1])) out.structure.push('the columns are not in the schema\'s order');
    /* A Set of strings hashes each fresh cell string: 370 ns a lookup, 5.6 of 6.7 s on 1 M rows
       for missing values alone. A short list compares by === (length first), nearly free. */
    const missingList = Array.isArray(schema.missingValues) ? schema.missingValues.map(String) : [''];
    const isMissing = missingList.length === 1 ? (m => s => s === m)(missingList[0]) : s => missingList.includes(s);
    const items = new Map();
    const hit = (r, f, rule, label, value) => {
        const key = f ? f.name + '\u0001' + rule : '\u0001' + rule;
        let it = items.get(key);
        if (!it) items.set(key, it = { field: f ? f.name : '', rule, label, count: 0, examples: [], rows: new Set() });
        it.count++;
        if (it.examples.length < 3 && !it.examples.includes(value)) it.examples.push(value);
        if (it.rows.size < SCHEMA_MARK_MAX) it.rows.add(r);
        out.rows.add(r);
        if (f && out.cells.size < SCHEMA_MARK_MAX) { let m = out.cells.get(r); if (!m) out.cells.set(r, m = new Map()); if (!m.has(H[at[fields.indexOf(f)]])) m.set(H[at[fields.indexOf(f)]], label); }
    };
    /* Patterns are tried apart first (40-…): one that backtracks without end would freeze the window. */
    const pats = [], split = fields.map(f => scSplitPattern(f.constraints && f.constraints.pattern));
    fields.forEach((f, i) => { const p = split[i].base; if (p && at[i] >= 0) pats.push(['c' + at[i], `^(?:${p})$`, 'u']); });
    for (;;) {
        const g = regexGate(t, pats, null);
        if (!g) break;
        if (!g.pending) return { error: `a pattern of the schema ${g.error.replace(/^this regular expression /, '')}` };
        await new Promise(r => setTimeout(r, 50));
    }
    const checks = fields.map((f, i) => {
        if (at[i] < 0) return null;
        const k = f.constraints || {}, { base, checks: ids } = split[i];
        let re = null;
        if (base) { try { re = new RegExp(`^(?:${base})$`, 'u'); } catch (e) { out.structure.push(`the pattern of “${f.name}” is not a valid regular expression`); } }
        const cks = ids.map(key => [key, new RegExp(SC_CHECKS[key][4], 'u'), SC_CHECKS[key][3]]);
        /* The allowed values as the cells read: strings compare as they are, the others by their reading. */
        const enAll = Array.isArray(k.enum) ? k.enum.map(v => { const c = scValue(f, String(v)); return c === SC_BAD ? String(v) : c; }) : null;
        const en = enAll && (enAll.length <= 32 ? { has: v => enAll.includes(v) } : new Set(enAll));   // short lists by ===, as above
        const bound = v => { if (v == null) return null; if (typeof v === 'number') return v; const c = scValue(f, String(v)); return c === SC_BAD ? null : c; };
        return { f, c: at[i], k, re, base, cks, en, min: bound(k.minimum), max: bound(k.maximum), seen: k.unique ? new Set() : null };
    }).filter(Boolean);
    const pkNames = schema.primaryKey == null ? [] : [].concat(schema.primaryKey);
    const pkCols = pkNames.map(n => at[fields.findIndex(f => f.name === n)]).filter(c => c >= 0);
    /* A key of one column already checked as unique would report each duplicate twice. */
    const pkUnique = pkCols.length === 1 && checks.some(ck => ck.c === pkCols[0] && ck.seen);
    const pkSeen = pkCols.length && pkCols.length === pkNames.length && !pkUnique ? new Set() : null;
    const W = H.length, rows = t.allData;
    for (let i0 = 0; i0 < rows.length; i0 += SCHEMA_SLICE) {
        visitRows(t, rows.slice(i0, i0 + SCHEMA_SLICE), r => {
            out.checked++;
            if (r.len !== W) hit(r, null, 'fields', `row of ${r.len} fields instead of ${W}`, `row ${r.id}`);
            const d = r.data;
            for (const ck of checks) {
                const raw = d[ck.c], s = raw == null ? '' : String(raw), f = ck.f, k = ck.k;
                if (isMissing(s)) { if (k.required) hit(r, f, 'required', 'required: the cell is empty', s); continue; }
                const v = scValue(f, s);
                if (v === SC_BAD) { hit(r, f, 'type', scErr, s); continue; }
                if (ck.en && !ck.en.has(v)) hit(r, f, 'enum', 'not one of the allowed values', s);
                if (ck.re && !ck.re.test(s)) hit(r, f, 'pattern', `does not match ${ck.base}`, s);
                for (let j = 0; j < ck.cks.length; j++) if (ck.cks[j][1].test(s)) hit(r, f, ck.cks[j][0], ck.cks[j][2], s);
                if (ck.min != null && v < ck.min) hit(r, f, 'minimum', `below the minimum ${k.minimum}`, s);
                if (ck.max != null && v > ck.max) hit(r, f, 'maximum', `above the maximum ${k.maximum}`, s);
                if (k.minLength != null && s.length < k.minLength) hit(r, f, 'minLength', `shorter than ${k.minLength} characters`, s);
                if (k.maxLength != null && s.length > k.maxLength) hit(r, f, 'maxLength', `longer than ${k.maxLength} characters`, s);
                if (ck.seen) { if (ck.seen.has(v)) hit(r, f, 'unique', 'duplicate value', s); else ck.seen.add(v); }   // the reading itself: 007 and 7 are the same integer
            }
            if (pkSeen) {
                const key = pkCols.map(c => cellStr(d[c])).join('\u0001');
                if (pkSeen.has(key)) hit(r, null, 'primaryKey', `duplicate primary key (${pkNames.join(', ')})`, pkCols.map(c => cellStr(d[c])).join(' · '));
                else pkSeen.add(key);
            }
        });
        if (progress) { if (progress(Math.min(1, (i0 + SCHEMA_SLICE) / rows.length)) === false) return { error: 'cancelled' }; await new Promise(r => setTimeout(r, 0)); }
    }
    out.items = [...items.values()].sort((a, b) => b.count - a.count);
    return out;
}

/* ---- Create: the dialog ----------------------------------------------
   Each column: its type (a select) and its rules — the inferred ones ticked, any other added
   with "+ rule", pre-filled from what the values show (smallest and largest seen, lengths,
   their shapes as a pattern, their distinct values). A rule's value is edited in place, and a
   rule added or edited says at once how many rows of this file break it. */
const SC_ORDERED = ['integer', 'number', 'date', 'datetime', 'time', 'year'];
const SCHEMA_RULES = {      // key → [label, for which fields, how its value is edited]
    required: ['required', () => true, null],
    unique: ['unique', () => true, null],
    minimum: ['minimum', f => SC_ORDERED.includes(f.type), 'value'],
    maximum: ['maximum', f => SC_ORDERED.includes(f.type), 'value'],
    minLength: ['at least', () => true, 'length'],
    maxLength: ['at most', () => true, 'length'],
    pattern: ['pattern', f => f.type === 'string', 'pattern'],
    enum: ['one of', f => f.type !== 'boolean', 'list'],
    format: ['format', f => f.type === 'string', 'format'],
    /* The checks of a text, kept apart in the dialog and joined into the pattern when written (SC_CHECKS). */
    ...Object.fromEntries(Object.entries(SC_CHECKS).map(([key, c]) => [key, [c[0], f => f.type === 'string', null]])),
};
const SC_FORMATS = { email: 'e-mail', uri: 'URI', uuid: 'UUID' };

async function openSchemaCreate() {
    const t = T(); if (!t || !t.loaded) return;
    document.getElementById('sc-title').textContent = 'Create a schema';
    document.getElementById('sc-sum').textContent = `The rules ${t.name} already follows, to be written beside it as ${schemaStem(t.name)}.schema.json. Add your own with “+ rule”.`;
    document.getElementById('sc-list').innerHTML = '<tr><td class="muted">Reading every row…</td></tr>';
    document.getElementById('sc-note').innerHTML = '';
    schemaButtons('create');
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-schema').style.display = 'block';
    const list = document.getElementById('sc-list');
    /* progress() answers false once the dialog is closed: the pass stops at its next slice. */
    const schema = await schemaInfer(t, p => { list.innerHTML = `<tr><td class="muted">Reading every row… ${Math.round(p * 100)} %</td></tr>`; return schemaOpen(); });
    if (!schema || T() !== t || !schemaOpen()) return;
    schemaDraft = { t, schema, hints: schema.hints, pk: !!schema.primaryKey, counts: new Map(), raw: new Map(),
        on: new Map(schema.fields.map(f => [f.name, new Set([...Object.keys(f.constraints || {}), ...(f.format && f.type === 'string' ? ['format'] : [])])])) };
    renderSchemaDraft();
}

/* A rule's value as the dialog shows it. */
function scRuleText(f, key) {
    const k = f.constraints || {}, v = key === 'format' ? f.format : k[key];
    if (key === 'enum') return (v || []).map(x => /[,"]|^\s|\s$/.test(String(x)) ? `"${String(x).replace(/"/g, '""')}"` : String(x)).join(', ');
    return v == null ? '' : String(v);
}
/* "a, b, "c, d"" → ['a', 'b', 'c, d']: quotes as in a CSV for a value holding a comma. */
function scParseList(text) {
    const out = [], re = /\s*(?:"((?:[^"]|"")*)"|([^,]*))\s*(?:,|$)/g;
    let m;
    while (re.lastIndex < text.length && (m = re.exec(text))) {
        const v = m[1] != null ? m[1].replace(/""/g, '"') : m[2].trim();
        if (v !== '' || m[1] != null) out.push(v);
        if (!m[0]) break;
    }
    return out;
}
function renderSchemaDraft() {
    const D = schemaDraft; if (!D) return;
    const { schema } = D;
    const typeSel = (f, i) => `<select class="bs-input bs-select sc-type" onchange="schemaSetType(${i}, this.value)">${SCHEMA_TYPES.map(ty => `<option${ty === f.type ? ' selected' : ''}>${ty}</option>`).join('')}</select>`;
    const extra = f => [f.format && f.type !== 'string' && f.format !== 'default' ? `format ${f.format}` : '', f.decimalChar === ',' ? 'decimal comma' : '', f.groupChar ? 'spaces between thousands' : '',
        f.trueValues ? `${f.trueValues.join('/')} = true` : ''].filter(Boolean).map(esc).join(' · ');
    const count = (i, key) => {
        const n = D.counts.get(i + '|' + key);
        if (n === undefined) return '';
        if (typeof n === 'string') return `<span class="sc-cnt muted">${esc(n)}</span>`;
        return n ? `<span class="sc-cnt bad" title="Rows of ${esc(D.t.name)} that break this rule">${fmt(n)} row${n === 1 ? '' : 's'} break it</span>` : '<span class="sc-cnt good" title="Every row of this file follows it">✓</span>';
    };
    const editor = (f, i, key, kind) => {
        const id = `sc-${i}-${key}`, raw = D.raw.get(i + '|' + key), val = raw != null ? raw : scRuleText(f, key), bad = raw != null ? ' bad' : '';
        if (kind === 'format') return `<select class="bs-input bs-select sc-val" style="width: 12ch" onchange="schemaSetValue(${i}, '${key}', this.value)">${Object.entries(SC_FORMATS).map(([k, l]) => `<option value="${k}"${k === f.format ? ' selected' : ''}>${l}</option>`).join('')}</select>`;
        const size = kind === 'length' ? 3 : kind === 'list' ? 36 : kind === 'pattern' ? Math.min(40, Math.max(14, val.length + 2)) : Math.min(14, Math.max(6, val.length + 2));
        const list = kind === 'pattern' ? ` list="${id}-l"` : '';
        const opts = kind === 'pattern' ? `<datalist id="${id}-l">${[...(D.hints[i] ? D.hints[i].patterns : []), ...SC_PATTERN_PRESETS.map(([p, l]) => [p, l])].map(([p, l]) => `<option value="${esc(p)}">${esc(l)}</option>`).join('')}</datalist>` : '';
        return `<input type="text" id="${id}" class="bs-input sc-val${bad}" style="width: ${size + 2}ch" value="${esc(val)}"${list} spellcheck="false" autocomplete="off" onchange="schemaSetValue(${i}, '${key}', this.value)"${kind === 'list' ? ` title="${esc(val)}"` : ''}>${opts}`;
    };
    const rule = (f, i, key) => {
        const [label, , kind] = SCHEMA_RULES[key], on = D.on.get(f.name).has(key);
        return `<span class="sc-rule${on ? ' on' : ''}"><label><input type="checkbox"${on ? ' checked' : ''} onchange="schemaToggle(${i}, '${key}', this.checked)"> ${esc(label)}</label>${kind ? ' ' + editor(f, i, key, kind) : ''} ${count(i, key)}</span>`;
    };
    const listEl = document.getElementById('sc-list');
    listEl.classList.add('create');
    listEl.innerHTML = '<tr><th>Column</th><th>Type</th><th>Rules</th></tr>' + schema.fields.map((f, i) => {
        const keys = Object.keys(SCHEMA_RULES).filter(k => k === 'format' ? f.type === 'string' && f.format : f.constraints && f.constraints[k] !== undefined);
        const rules = keys.map(k => rule(f, i, k)).join('');
        const pk = schema.primaryKey === f.name ? `<span class="sc-rule${D.pk ? ' on' : ''}"><label><input type="checkbox"${D.pk ? ' checked' : ''} onchange="schemaDraft.pk = this.checked; renderSchemaDraft()"> primary key</label></span>` : '';
        const ex = extra(f);
        return `<tr><td>${esc(f.name)}</td><td>${typeSel(f, i)}${ex ? `<div class="muted sc-ex">${ex}</div>` : ''}</td><td>${rules}${pk}<span class="sc-add" onclick="schemaAddMenu(event, ${i})" title="Add a rule, filled in from the values">+ rule</span></td></tr>`;
    }).join('');
    const n = [...D.on.values()].reduce((a, s) => a + s.size, 0) + (D.pk && schema.primaryKey ? 1 : 0);
    document.getElementById('sc-note').textContent = `${fmt(schema.fields.length)} columns, ${fmt(n)} rules ticked. Untick a rule the next files may break; change a type that was guessed too narrowly.`;
}

/* The "+ rule" menu: the rules this column does not have yet, each with what the values suggest. */
function schemaAddMenu(e, i) {
    e.stopPropagation();
    const D = schemaDraft, f = D.schema.fields[i], h = D.hints[i] || {}, k = f.constraints || {}, m = document.getElementById('sc-menu');
    const again = m.classList.contains('open') && m.dataset.i === String(i);
    closeDDs();
    if (again) return;
    const has = key => key === 'format' ? !!f.format : k[key] !== undefined;
    const item = (call, label, sub) => `<div class="dd-item" onclick="closeDDs(); ${call}"><span class="lbl">${esc(label)}${sub ? `<span class="sc-sub">${esc(sub)}</span>` : ''}</span></div>`;
    let html = `<div class="dd-head">${esc(f.name)}</div>`;
    const ok = key => !has(key) && SCHEMA_RULES[key][1](f);
    if (ok('required')) html += item(`schemaAddRule(${i}, 'required')`, 'Required', `filled in ${Math.round((h.filled || 0) * 1000) / 10} % of the rows`);
    if (ok('unique')) html += item(`schemaAddRule(${i}, 'unique')`, 'Unique', 'no value twice');
    if (ok('minimum')) html += item(`schemaAddRule(${i}, 'minimum')`, 'Minimum', h.min ? `smallest seen: ${h.min}` : '');
    if (ok('maximum')) html += item(`schemaAddRule(${i}, 'maximum')`, 'Maximum', h.max ? `largest seen: ${h.max}` : '');
    if (ok('minLength') || ok('maxLength')) html += item(`schemaAddRule(${i}, 'length')`, 'Length', h.lmin != null ? `from ${h.lmin} to ${h.lmax} characters seen` : '');
    if (ok('pattern')) {
        for (const [p, l] of (h.patterns || []).slice(0, 3)) html += item(`schemaAddRule(${i}, 'pattern', ${JSON.stringify(p).replace(/"/g, '&quot;')})`, 'Pattern', `${l}: ${p.length > 48 ? p.slice(0, 47) + '…' : p}`);
        html += item(`schemaAddRule(${i}, 'pattern', '')`, 'Pattern…', 'type your own, or pick a common one');
    }
    if (ok('enum') && h.values) html += item(`schemaAddRule(${i}, 'enum')`, 'List of values', `the ${fmt(h.values.length)} seen`);
    if (ok('format')) for (const [fk, l] of Object.entries(SC_FORMATS)) html += item(`schemaAddRule(${i}, 'format', '${fk}')`, `Format: ${l}`, '');
    /* Upper case and capitalised exclude each other: only one is offered once the other is there. */
    for (const key of Object.keys(SC_CHECKS))
        if (ok(key) && !(key === 'upperCase' && has('capitalised')) && !(key === 'capitalised' && has('upperCase'))) html += item(`schemaAddRule(${i}, '${key}')`, SC_CHECKS[key][1], SC_CHECKS[key][2]);
    if (html.indexOf('dd-item') < 0) html += '<div class="dd-item disabled"><span class="lbl">Every rule of this type is there</span></div>';
    m.innerHTML = html; m.dataset.i = i;
    m.classList.add('open');
    const r = e.currentTarget.getBoundingClientRect(), w = m.offsetWidth, hgt = m.offsetHeight;
    m.style.left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8)) + 'px';
    m.style.top = (r.bottom + 4 + hgt > window.innerHeight - 8 ? Math.max(8, r.top - hgt - 4) : r.bottom + 4) + 'px';
}
function schemaAddRule(i, key, arg) {
    const D = schemaDraft, f = D.schema.fields[i], h = D.hints[i] || {}, on = D.on.get(f.name);
    f.constraints = f.constraints || {};
    const typed = s => { const v = scValue(f, String(s)); return v === SC_BAD ? s : (typeof v === 'number' && ['date', 'datetime', 'time', 'year'].includes(f.type) ? s : v); };
    const add = (k, v) => { f.constraints[k] = v; on.add(k); };
    if (key === 'required' || key === 'unique' || SC_CHECKS[key]) add(key, true);
    else if (key === 'minimum') add(key, h.min ? typed(h.min) : 0);
    else if (key === 'maximum') add(key, h.max ? typed(h.max) : 0);
    else if (key === 'length') { add('minLength', h.lmin != null ? h.lmin : 0); add('maxLength', h.lmax != null ? h.lmax : 0); }
    else if (key === 'pattern') add(key, arg || '');
    else if (key === 'enum') add(key, (h.values || []).slice());
    else if (key === 'format') { f.format = arg; on.add('format'); }
    renderSchemaDraft();
    for (const k of key === 'length' ? ['minLength', 'maxLength'] : [key]) schemaRecount(i, k);
    if (key === 'pattern' && !arg) { const box = document.getElementById(`sc-${i}-pattern`); if (box) box.focus(); }   // its own: the presets are in the field's list
}
/* A value typed in a rule: kept if it reads as one, else shown in red until it does. */
function schemaSetValue(i, key, text) {
    const D = schemaDraft, f = D.schema.fields[i], kind = SCHEMA_RULES[key][2], id = i + '|' + key;
    let v, okv = true;
    text = String(text);
    if (kind === 'value') {
        const r = scValue(f, text.trim());
        if (r === SC_BAD) okv = false;
        else v = typeof r === 'number' && (f.type === 'integer' || f.type === 'number') ? r : text.trim();
    } else if (kind === 'length') { v = parseInt(text, 10); okv = /^\s*\d+\s*$/.test(text); }
    else if (kind === 'pattern') { try { new RegExp(`^(?:${text})$`, 'u'); v = text; } catch (e) { okv = false; } }
    else if (kind === 'list') v = scParseList(text);
    else if (kind === 'format') v = text;
    if (!okv) { D.raw.set(id, text); D.counts.set(id, 'not valid'); renderSchemaDraft(); return; }
    D.raw.delete(id);
    if (key === 'format') f.format = v; else (f.constraints = f.constraints || {})[key] = v;
    D.on.get(f.name).add(key);
    renderSchemaDraft();
    schemaRecount(i, key);
}
/* How many rows of this file break rule `key` of column i, shown beside it. Patterns go
   through the regex guard first (40-…) and are counted once it lets them run. */
function schemaRecount(i, key) {
    const D = schemaDraft, t = D.t, f = D.schema.fields[i], k = f.constraints || {}, id = i + '|' + key;
    if (T() !== t) return;
    const col = t.headers.indexOf(f.name); if (col < 0) return;
    let test;
    if (key === 'required') test = s => s === '';
    else if (key === 'unique') { const seen = new Set(); test = s => { if (s === '') return false; const v = scValue(f, s), x = v === SC_BAD ? s : v; if (seen.has(x)) return true; seen.add(x); return false; }; }
    else if (key === 'minimum' || key === 'maximum') {
        const lim = typeof k[key] === 'number' ? k[key] : scValue(f, String(k[key]));
        if (lim === SC_BAD) return;
        test = s => { if (s === '') return false; const v = scValue(f, s); return v !== SC_BAD && (key === 'minimum' ? v < lim : v > lim); };
    }
    else if (key === 'minLength') test = s => s !== '' && s.length < k.minLength;
    else if (key === 'maxLength') test = s => s.length > k.maxLength;
    else if (key === 'enum') { const all = (k.enum || []).map(String); test = s => s !== '' && !all.includes(s); }
    else if (key === 'format') test = s => s !== '' && scValue({ type: 'string', format: f.format }, s) === SC_BAD;
    else if (SC_CHECKS[key]) { const re = new RegExp(SC_CHECKS[key][4], 'u'); test = s => re.test(s); }
    else if (key === 'pattern') {
        if (!k.pattern) { D.counts.delete(id); return; }
        const g = regexGate(t, [['c' + col, `^(?:${k.pattern})$`, 'u']], () => { if (schemaDraft === D && schemaOpen()) { schemaRecount(i, key); renderSchemaDraft(); } });
        if (g) { D.counts.set(id, g.pending ? 'checking…' : 'too slow for this file'); renderSchemaDraft(); return; }
        const re = new RegExp(`^(?:${k.pattern})$`, 'u');
        test = s => s !== '' && !re.test(s);
    }
    else return;
    let n = 0;
    visitRows(t, t.allData, r => { const v = cellOf(r, col); if (test(v == null ? '' : String(v))) n++; });
    D.counts.set(id, n);
    renderSchemaDraft();
}
function schemaSetType(i, ty) {
    const D = schemaDraft, f = D.schema.fields[i];
    f.type = ty;
    if (!['date', 'datetime'].includes(ty) && !(ty === 'string' && SC_FORMATS[f.format])) delete f.format;
    if (ty !== 'number') { delete f.decimalChar; delete f.groupChar; }
    if (ty !== 'boolean') { delete f.trueValues; delete f.falseValues; }
    /* Rules the new type cannot take go; the others are counted again, its reading has changed. */
    for (const key of Object.keys(f.constraints || {})) if (!SCHEMA_RULES[key] || !SCHEMA_RULES[key][1](f)) { delete f.constraints[key]; D.on.get(f.name).delete(key); }
    if (!f.format) D.on.get(f.name).delete('format');
    renderSchemaDraft();
    for (const key of D.on.get(f.name)) schemaRecount(i, key);
}
function schemaToggle(i, key, on) {
    const s = schemaDraft.on.get(schemaDraft.schema.fields[i].name);
    if (on) s.add(key); else s.delete(key);
    renderSchemaDraft();
}
/* The schema as ticked: only the rules kept, in Table Schema's terms. */
function schemaFromDraft() {
    const D = schemaDraft, out = { fields: [], missingValues: D.schema.missingValues };
    for (const f of D.schema.fields) {
        const g = { ...f }, on = D.on.get(f.name), k = {};
        delete g.constraints;
        if (f.type === 'string' && !on.has('format')) delete g.format;
        const checks = [];
        for (const key of Object.keys(f.constraints || {})) if (on.has(key) && SCHEMA_RULES[key]) { if (SC_CHECKS[key]) checks.push(key); else k[key] = f.constraints[key]; }
        if (checks.length) k.pattern = scJoinPattern(k.pattern, Object.keys(SC_CHECKS).filter(c => checks.includes(c)));
        if (Object.keys(k).length) g.constraints = k;
        out.fields.push(g);
    }
    if (D.pk && D.schema.primaryKey) out.primaryKey = D.schema.primaryKey;
    return out;
}
async function saveSchemaDraft() {
    const D = schemaDraft; if (!D) return;
    const where = await schemaWrite(D.t, JSON.stringify(schemaFromDraft(), null, 2) + '\n');
    if (!where) return;
    closeAllModals();
    doneMsg(`${D.t.name} | Schema written: ${baseName(where)} — check other files with "Validate with a schema".`);
}

/* ---- Validate: the dialog -------------------------------------------- */
async function openSchemaValidate() {
    const t = T(); if (!t || !t.loaded) return;
    document.getElementById('sc-title').textContent = 'Validate with a schema';
    document.getElementById('sc-sum').textContent = 'Looking for a schema beside the file…';
    document.getElementById('sc-list').innerHTML = '';
    document.getElementById('sc-note').innerHTML = '';
    schemaButtons('validate');
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-schema').style.display = 'block';
    const found = await schemaFind(t);
    if (!found) {
        document.getElementById('sc-sum').innerHTML = `No <code>${esc(schemaStem(t.name))}.schema.json</code> beside the file. Choose a schema, or create one from a file that is right with "Create a schema".`;
        return;
    }
    schemaRun(t, found.name, found.text);
}
async function pickSchema() {
    const t = T(); if (!t || !t.loaded) return;
    let fh;
    try { [fh] = await showOpenFilePicker({ types: [{ description: 'Table Schema', accept: { 'application/json': ['.json'] } }], startIn: t.handle || t.dirHandle || 'documents' }); }
    catch (e) { return; }
    schemaRun(t, fh.name, await (await fh.getFile()).text());
}
async function schemaRun(t, name, text) {
    schemaButtons('validate');
    const sum = document.getElementById('sc-sum'), list = document.getElementById('sc-list');
    const p = schemaParse(text);
    if (p.error) { sum.innerHTML = `<span class="warn">${esc(name)}: ${esc(p.error)}.</span>`; list.innerHTML = ''; return; }
    sum.textContent = `Checking ${t.name} against ${name}…`;
    const res = await schemaValidate(t, p.schema, q => { list.innerHTML = `<tr><td class="muted">Checking… ${Math.round(q * 100)} %</td></tr>`; return schemaOpen(); });
    if (T() !== t || !schemaOpen()) return;
    if (res.error) { sum.innerHTML = `<span class="warn">${esc(name)}: ${esc(res.error)}.</span>`; list.innerHTML = ''; return; }
    schemaReport = { t, name, ...res };
    renderSchemaReport();
}
function renderSchemaReport() {
    const R = schemaReport; if (!R) return;
    document.getElementById('sc-list').classList.remove('create');
    const bad = R.rows.size, cols = new Set(R.items.filter(i => i.field).map(i => i.field)).size;
    document.getElementById('sc-sum').innerHTML = (bad || R.structure.length
        ? `<span class="warn">${bad ? `<b>${fmt(bad)}</b> of ${fmt(R.checked)} rows break the schema${cols ? `, in ${fmt(cols)} column${cols === 1 ? '' : 's'}` : ''}` : 'The rows follow the schema'}${R.structure.length ? ` — ${fmt(R.structure.length)} problem${R.structure.length === 1 ? '' : 's'} of structure` : ''}.</span>`
        : `<span class="ok-txt">All ${fmt(R.checked)} rows follow the schema.</span>`) + ` <span class="muted">${esc(R.t.name)} · ${esc(R.name)}</span>`;
    const struct = R.structure.map(s => `<tr><td colspan="4" class="warn">${esc(s)}</td></tr>`).join('');
    document.getElementById('sc-list').innerHTML = (struct ? '<tr class="sec"><th colspan="4">Structure</th></tr>' + struct : '')
        + (R.items.length ? '<tr class="sec"><th>Column</th><th>Problem</th><th class="k">Rows</th><th>For example</th></tr>'
            + R.items.map((it, i) => `<tr class="go" onclick="schemaShow(${i})" title="Show these rows"><td>${esc(it.field || '(row)')}</td><td>${esc(it.label)}</td><td class="k">${fmt(it.count)}</td><td>${it.examples.map(v => `<code>${esc(v === '' ? '(empty)' : v)}</code>`).join(' ')}</td></tr>`).join('') : '');
    document.getElementById('sc-note').textContent = R.items.length ? 'Click a line to show its rows; the cells at fault are tinted, their tooltip says why.' : '';
    document.getElementById('sc-mark').disabled = !bad;
}
/* The rows of one kind of error (i), or of all (no i), as row marks shown alone. */
function schemaShow(i) {
    const R = schemaReport; if (!R || T() !== R.t) return;
    const rows = i == null ? R.rows : R.items[i].rows;
    const label = i == null ? 'rows breaking the schema' : `${R.items[i].field ? R.items[i].field + ': ' : ''}${R.items[i].label}`;
    closeAllModals();
    setRowMark(R.t, { kind: 'schema', label, tip: 'Schema', rows: new Set(rows), cells: R.cells, only: true });
    setStats(`${R.t.name} | ${fmt(rows.size)} ${label} — the chip in the status bar shows all rows again.`);
}
function schemaOpen() { return document.getElementById('modal-schema').style.display === 'block'; }
function schemaButtons(mode) {
    document.getElementById('sc-actions').innerHTML = mode === 'create'
        ? `<button class="btn btn-outline" onclick="closeAllModals()">Cancel</button><button class="btn" onclick="saveSchemaDraft()">Write the schema</button>`
        : `<button class="btn btn-outline" onclick="pickSchema()">Choose a schema…</button><button class="btn btn-outline" id="sc-mark" disabled onclick="schemaShow()">Show every row at fault</button><button class="btn" onclick="closeAllModals()">Close</button>`;
}
