/* ---------------------------------------------------------------
   COMPUTED COLUMN
   A JavaScript expression evaluated once per row, where {Column} stands
   for that row's cell (always a string, "" when empty). The helpers
   below cover what a spreadsheet formula usually does — numbers read the
   French way, dates day first, text cleaning — and are plain arguments
   of the compiled function, so a formula calls them by name. It is the
   user's own code on their own data, compiled with new Function(); an
   error in one row leaves that cell empty and is counted, never thrown.
   The result goes into a new column, or replaces a column's values in
   the rows the filters show.
----------------------------------------------------------------*/
const DAY_MS = 864e5;
function fxDate(v) {
    if (v instanceof Date) return isNaN(v) ? null : v;
    const s = cellStr(v).trim(); if (!s) return null;
    const p = parseDateCell(s, false);
    if (p) {
        const m = p.rest.match(/(\d{1,2}):(\d{2})(?::(\d{2}))?/);
        return new Date(Date.UTC(p.y, p.mo - 1, p.d, m ? +m[1] : 0, m ? +m[2] : 0, m && m[3] ? +m[3] : 0));
    }
    return null;
}
function fxNum(v) {
    if (typeof v === 'number') return v;
    const s = cellStr(v).trim();
    return s ? numKey(s.replace(/[€$£%]/g, '')) : 0;       // an empty cell counts as 0, as in a spreadsheet
}
function fxFmtDate(d, pattern = 'dd/mm/yyyy') {
    d = fxDate(d); if (!d) return '';
    const map = { yyyy: d.getUTCFullYear(), yy: pad2(d.getUTCFullYear() % 100), mm: pad2(d.getUTCMonth() + 1), dd: pad2(d.getUTCDate()),
        hh: pad2(d.getUTCHours()), mi: pad2(d.getUTCMinutes()), ss: pad2(d.getUTCSeconds()) };
    return pattern.replace(/yyyy|yy|mm|dd|hh|mi|ss/g, k => map[k]);
}
const FX = {
    num: fxNum,
    date: fxDate,
    today: () => { const n = new Date(); return new Date(Date.UTC(n.getFullYear(), n.getMonth(), n.getDate())); },
    days: (a, b) => { a = fxDate(a); b = fxDate(b); return a && b ? Math.round((b - a) / DAY_MS) : NaN; },
    addDays: (d, n) => { d = fxDate(d); return d ? new Date(+d + fxNum(n) * DAY_MS) : null; },
    year: d => { d = fxDate(d); return d ? d.getUTCFullYear() : NaN; },
    month: d => { d = fxDate(d); return d ? d.getUTCMonth() + 1 : NaN; },
    day: d => { d = fxDate(d); return d ? d.getUTCDate() : NaN; },
    fmtDate: fxFmtDate,
    round: (n, d = 0) => { const k = 10 ** d; return Math.round(fxNum(n) * k) / k; },
    fixed: (n, d = 2) => { n = fxNum(n); return isNaN(n) ? '' : fxNumText(n.toFixed(d)); },
    upper: s => cellStr(s).toLocaleUpperCase('fr'),
    lower: s => cellStr(s).toLocaleLowerCase('fr'),
    trim: s => cellStr(s).replace(/[\s ]+/g, ' ').trim(),
    capitalize: s => cellStr(s).toLocaleLowerCase('fr').replace(/(^|[\s\-'’(])(\p{L})/gu, (m, a, b) => a + b.toLocaleUpperCase('fr')),
    slug: s => slugify(cellStr(s)),
    len: s => cellStr(s).length,
    left: (s, n) => cellStr(s).slice(0, n),
    right: (s, n) => n > 0 ? cellStr(s).slice(-n) : '',
    pad: (s, n, ch = '0') => cellStr(s).padStart(n, ch),
    extract: (s, re, g = 1) => { const m = cellStr(s).match(re instanceof RegExp ? re : new RegExp(re)); return m ? (m[g] ?? m[0]) : ''; },
    replace: (s, re, by = '') => cellStr(s).replace(re instanceof RegExp ? re : new RegExp(re, 'g'), by),
    contains: (s, part) => removeAccents(cellStr(s).toLowerCase()).includes(removeAccents(cellStr(part).toLowerCase())),
    empty: s => cellStr(s).trim() === '',
    first: (...v) => { for (const x of v) if (cellStr(x).trim() !== '') return x; return ''; },
    join: (sep, ...v) => v.map(cellStr).filter(x => x.trim() !== '').join(sep),
    /* Text, not numbers: a code keeps its leading zeros ("01 - Ain" → "01"); num() reads them after. */
    digits: s => cellStr(s).replace(/\D+/g, ''),
    firstNumber: s => { const m = cellStr(s).match(/(?:^|(?<=\s))[-+](?=\d)|\d/); if (!m) return ''; return cellStr(s).slice(m.index).match(/^[-+]?\d+(?:[.,]\d+)?/)[0]; },   // a sign only when it starts a word: "68 - Rhin" is 68, "-3,75 €" is -3,75
    abs: n => Math.abs(fxNum(n)),
    min: (...v) => { const n = fxNums(v); return n.length ? Math.min(...n) : NaN; },
    max: (...v) => { const n = fxNums(v); return n.length ? Math.max(...n) : NaN; },
    sum: (...v) => fxNums(v).reduce((a, b) => a + b, 0),
    avg: (...v) => { const n = fxNums(v); return n.length ? n.reduce((a, b) => a + b, 0) / n.length : NaN; },
    split: (s, sep, n = 1) => { const p = cellStr(s).split(sep instanceof RegExp ? sep : cellStr(sep)); return p[n < 0 ? p.length + n : n - 1] ?? ''; },
    mid: (s, start, n) => { const a = Math.max(0, start - 1); return cellStr(s).slice(a, n == null ? undefined : a + n); },
    startsWith: (s, part) => fxFold(s).startsWith(fxFold(part)),
    endsWith: (s, part) => fxFold(s).endsWith(fxFold(part)),
    matches: (s, re) => { re = re instanceof RegExp ? re : new RegExp(re, 'i'); re.lastIndex = 0; return re.test(cellStr(s)); },
    weekday: d => { d = fxDate(d); return d ? d.getUTCDay() || 7 : NaN; },
    week: d => {
        d = fxDate(d); if (!d) return NaN;
        const t = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate() + 4 - (d.getUTCDay() || 7)));   // the Thursday of its ISO week
        return Math.ceil(((t - Date.UTC(t.getUTCFullYear(), 0, 1)) / DAY_MS + 1) / 7);
    },
    addMonths: (d, n) => {
        d = fxDate(d); if (!d) return null;
        const y = d.getUTCFullYear(), m = d.getUTCMonth() + Math.trunc(fxNum(n)), last = new Date(Date.UTC(y, m + 1, 0)).getUTCDate();   // 31 January + 1 month: 28/29 February
        return new Date(Date.UTC(y, m, Math.min(d.getUTCDate(), last), d.getUTCHours(), d.getUTCMinutes(), d.getUTCSeconds()));
    },
    months: (a, b) => fxMonths(a, b),
    years: (a, b) => { const m = fxMonths(a, b); return isNaN(m) ? NaN : Math.trunc(m / 12); },
    isEmail: s => /^[^\s@]+@[^\s@]+\.[^\s@.]{2,}$/.test(cellStr(s).trim()),
    isNumber: s => isNumericLike(cellStr(s)),
    isDate: s => fxDate(s) !== null,
    isPhone: s => { const x = cellStr(s).replace(/\p{Cf}/gu, '').trim(); return !!x && parsePhoneCell(x, '33') !== null; },   // national numbers read as French, like Convert
    ifs: (...a) => { for (let i = 0; i + 1 < a.length; i += 2) if (a[i]) return a[i + 1]; return a.length % 2 ? a[a.length - 1] : ''; },
    cases: (v, ...a) => { const x = cellStr(v); for (let i = 0; i + 1 < a.length; i += 2) if (x === cellStr(a[i])) return a[i + 1]; return a.length % 2 ? a[a.length - 1] : ''; }
};
/* The numbers among a helper's arguments, empty cells left out (an average of 10 and nothing is 10). */
function fxNums(v) { const out = []; for (const x of v) { if (typeof x !== 'number' && cellStr(x).trim() === '') continue; const n = fxNum(x); if (!isNaN(n)) out.push(n); } return out; }
function fxFold(s) { return removeAccents(cellStr(s).toLowerCase()); }
/* Whole months from a to b (negative when b is earlier): 31/01 → 28/02 is 0, as a birthday not reached. */
function fxMonths(a, b) {
    a = fxDate(a); b = fxDate(b); if (!a || !b) return NaN;
    const sign = b < a ? -1 : 1; if (sign < 0) [a, b] = [b, a];
    let m = (b.getUTCFullYear() - a.getUTCFullYear()) * 12 + b.getUTCMonth() - a.getUTCMonth();
    if (b.getUTCDate() < a.getUTCDate()) m--;
    return sign * m;
}

/* ---------------------------------------------------------------
   FX_DOC: the one description of what a formula can use, read by the
   function picker (30-fx-picker.js), the autocomplete of the expression
   filter and the help of the computed column. A function listed here
   must exist in FX; Methods and Operators are plain JavaScript, listed
   so that nobody has to know it. In examples, {N} {D} {T} {T2} stand
   for a number, date, text and second text column of the file, so an
   example runs as it is (fxExample()).
   f: family · s: signature (its first argument is selected on insert)
   · d: what it does · ex: [formula, what it shows] · pick: 'email' or
   'phone', the column to prefer — by its values — over the first text one
----------------------------------------------------------------*/
const FX_DOC = [
    { f: 'Numbers', n: 'num', s: 'num(v)', d: 'The value as a number: reads 1 234,50 as well as 1,234.50; an empty cell counts as 0.', ex: [['num({N}) * 1.2', 'plus 20 %']] },
    { f: 'Numbers', n: 'round', s: 'round(n, digits)', d: 'Rounded to that many decimals (none by default).', ex: [['round(num({N}), 1)', 'one decimal']] },
    { f: 'Numbers', n: 'fixed', s: 'fixed(n, digits)', d: 'Text with exactly that many decimals, written with the file\'s decimal mark.', ex: [['fixed(num({N}), 2)', 'two decimals']] },
    { f: 'Numbers', n: 'abs', s: 'abs(n)', d: 'The value without its sign.', ex: [['abs(num({N}))', 'absolute value']] },
    { f: 'Numbers', n: 'sum', s: 'sum(a, b, …)', d: 'The sum of the values given; empty cells are left out.', ex: [['sum({N}, 100)', 'add two values']] },
    { f: 'Numbers', n: 'avg', s: 'avg(a, b, …)', d: 'The average of the values given; empty cells are left out.', ex: [['avg({N}, 0)', 'average']] },
    { f: 'Numbers', n: 'min', s: 'min(a, b, …)', d: 'The smallest of the values given.', ex: [['min({N}, 1000)', 'capped at 1000']] },
    { f: 'Numbers', n: 'max', s: 'max(a, b, …)', d: 'The largest of the values given.', ex: [['max({N}, 0)', 'no negatives']] },
    { f: 'Dates', n: 'date', s: 'date(v)', d: 'The value as a date: yyyy-mm-dd or day first (dd/mm/yyyy), a time kept.', ex: [['date({D})', 'read a date']] },
    { f: 'Dates', n: 'today', s: 'today()', d: 'Today\'s date.', ex: [['today()', 'today']] },
    { f: 'Dates', n: 'days', s: 'days(from, to)', d: 'Days from one date to the other, negative when the second is earlier.', ex: [['days({D}, today())', 'days since']] },
    { f: 'Dates', n: 'months', s: 'months(from, to)', d: 'Whole months from one date to the other.', ex: [['months({D}, today())', 'months since']] },
    { f: 'Dates', n: 'years', s: 'years(from, to)', d: 'Whole years from one date to the other: an age.', ex: [['years({D}, today())', 'age']] },
    { f: 'Dates', n: 'addDays', s: 'addDays(date, n)', d: 'The date n days later (earlier when n is negative).', ex: [['addDays({D}, 30)', 'thirty days later']] },
    { f: 'Dates', n: 'addMonths', s: 'addMonths(date, n)', d: 'The date n months later; the 31st becomes the month\'s last day when needed.', ex: [['addMonths({D}, 1)', 'a month later']] },
    { f: 'Dates', n: 'year', s: 'year(date)', d: 'The year.', ex: [['year({D})', 'year'], ['year({D}) === 2024', 'dated 2024']] },
    { f: 'Dates', n: 'month', s: 'month(date)', d: 'The month, 1 to 12.', ex: [['month({D})', 'month']] },
    { f: 'Dates', n: 'day', s: 'day(date)', d: 'The day of the month.', ex: [['day({D})', 'day']] },
    { f: 'Dates', n: 'weekday', s: 'weekday(date)', d: 'The day of the week, 1 for Monday to 7 for Sunday.', ex: [['weekday({D}) >= 6', 'on a weekend']] },
    { f: 'Dates', n: 'week', s: 'week(date)', d: 'The ISO week number.', ex: [['week({D})', 'week number']] },
    { f: 'Dates', n: 'fmtDate', s: 'fmtDate(date, "dd/mm/yyyy")', d: 'The date written with a pattern: yyyy yy mm dd hh mi ss.', ex: [['fmtDate({D}, "yyyy-mm-dd")', 'ISO date']] },
    { f: 'Text', n: 'upper', s: 'upper(s)', d: 'In capitals.', ex: [['upper({T})', 'capitals']] },
    { f: 'Text', n: 'lower', s: 'lower(s)', d: 'In lower case.', ex: [['lower({T})', 'lower case']] },
    { f: 'Text', n: 'capitalize', s: 'capitalize(s)', d: 'Every Word Capitalised, the rest in lower case.', ex: [['capitalize({T})', 'capitalised']] },
    { f: 'Text', n: 'trim', s: 'trim(s)', d: 'Spaces removed at both ends, repeated spaces made one.', ex: [['trim({T})', 'clean spaces']] },
    { f: 'Text', n: 'slug', s: 'slug(s)', d: 'Lower case without accents or symbols: for comparing loosely.', ex: [['slug({T})', 'slug']] },
    { f: 'Text', n: 'len', s: 'len(s)', d: 'The number of characters.', ex: [['len({T}) > 20', 'long values']] },
    { f: 'Text', n: 'left', s: 'left(s, n)', d: 'The first n characters.', ex: [['left({T}, 3)', 'first three']] },
    { f: 'Text', n: 'right', s: 'right(s, n)', d: 'The last n characters.', ex: [['right({T}, 3)', 'last three']] },
    { f: 'Text', n: 'mid', s: 'mid(s, start, n)', d: 'n characters from position start (the first is 1).', ex: [['mid({T}, 2, 3)', 'three from the 2nd']] },
    { f: 'Text', n: 'split', s: 'split(s, sep, n)', d: 'The nth part of the text cut at sep (the first is 1, -1 the last).', ex: [['split({T}, " ", 1)', 'first word'], ['split({T}, " ", -1)', 'last word']] },
    { f: 'Text', n: 'pad', s: 'pad(s, n, "0")', d: 'Filled on the left up to n characters.', ex: [['pad({N}, 8, "0")', 'eight digits']] },
    { f: 'Text', n: 'firstNumber', s: 'firstNumber(s)', d: 'The first number in the text, as written: "68 - Rhin" and "Rhin 68" give 68, "01 - Ain" keeps 01; a decimal part kept. Empty when there is none.', ex: [['firstNumber({T})', 'first number'], ['num(firstNumber({T})) > 50', 'as a number']] },
    { f: 'Text', n: 'digits', s: 'digits(s)', d: 'Every digit of the text, the rest removed: "06 12-34" gives 061234.', ex: [['digits({T})', 'digits only']] },
    { f: 'Text', n: 'extract', pick: 'email', s: 'extract(s, regex, group)', d: 'The part a regular expression captures (group 1 by default).', ex: [['extract({T}, "@(.+)$")', 'e-mail domain']] },
    { f: 'Text', n: 'replace', s: 'replace(s, regex, by)', d: 'Every match of a regular expression replaced; $1… reuse its groups.', ex: [['replace({T}, "\\s+", "-")', 'spaces to dashes']] },
    { f: 'Tests', n: 'contains', s: 'contains(s, part)', d: 'True when the text contains the part, ignoring case and accents.', ex: [['contains({T}, "a")', 'contains an a']] },
    { f: 'Tests', n: 'startsWith', s: 'startsWith(s, part)', d: 'True when the text starts with the part, ignoring case and accents.', ex: [['startsWith({T}, "a")', 'starts with a']] },
    { f: 'Tests', n: 'endsWith', s: 'endsWith(s, part)', d: 'True when the text ends with the part, ignoring case and accents.', ex: [['endsWith({T}, "e")', 'ends with e']] },
    { f: 'Tests', n: 'matches', s: 'matches(s, regex)', d: 'True when a regular expression matches (case ignored when given as text).', ex: [['matches({T}, "^[a-m]")', 'starts with a to m']] },
    { f: 'Tests', n: 'empty', s: 'empty(v)', d: 'True when the cell is empty or only spaces.', ex: [['empty({T})', 'empty'], ['!empty({T})', 'filled']] },
    { f: 'Tests', n: 'isEmail', pick: 'email', s: 'isEmail(v)', d: 'True when the value looks like an e-mail address.', ex: [['!isEmail({T})', 'not an e-mail']] },
    { f: 'Tests', n: 'isPhone', pick: 'phone', s: 'isPhone(v)', d: 'True when the value reads as a phone number (national numbers as French).', ex: [['!isPhone({T})', 'not a phone']] },
    { f: 'Tests', n: 'isNumber', s: 'isNumber(v)', d: 'True when the value reads as a number.', ex: [['isNumber({N})', 'a number']] },
    { f: 'Tests', n: 'isDate', s: 'isDate(v)', d: 'True when the value reads as a date.', ex: [['!isDate({D})', 'not a date']] },
    { f: 'Values', n: 'ifs', s: 'ifs(test1, value1, test2, value2, …, otherwise)', d: 'The value of the first test that holds, else the last argument.', ex: [['ifs(num({N}) > 1000, "big", num({N}) > 100, "medium", "small")', 'three sizes']] },
    { f: 'Values', n: 'cases', s: 'cases(v, value1, result1, …, otherwise)', d: 'The result paired with the value v equals, else the last argument.', ex: [['cases({T}, "Paris", "75", "Lyon", "69", "other")', 'a code per value'], ['cases(month({D}), 1, "January", 2, "February", "other")', 'month names']] },
    { f: 'Values', n: 'first', s: 'first(a, b, …)', d: 'The first value that is not empty.', ex: [['first({T}, {T2}, "none")', 'first filled']] },
    { f: 'Values', n: 'join', s: 'join(sep, a, b, …)', d: 'The values joined with sep, empty ones skipped.', ex: [['join(" ", {T}, {T2})', 'join two columns']] },
    { f: 'Values', n: 'row', s: 'row', d: 'The row\'s number in the file.', ex: [['row % 2 === 0', 'even rows']] },
    { f: 'Methods', n: '.startsWith', s: '.startsWith(text)', d: 'A column is a string: its own methods work too. True when it starts with the text, case sensitive.', ex: [['{T}.startsWith("A")', 'starts with A']] },
    { f: 'Methods', n: '.endsWith', s: '.endsWith(text)', d: 'True when the string ends with the text, case sensitive.', ex: [['{T}.endsWith("e")', 'ends with e']] },
    { f: 'Methods', n: '.includes', s: '.includes(text)', d: 'True when the string contains the text, case sensitive.', ex: [['{T}.includes("-")', 'contains a dash']] },
    { f: 'Methods', n: '.length', s: '.length', d: 'The number of characters.', ex: [['{T}.length', 'length']] },
    { f: 'Methods', n: '.slice', s: '.slice(start, end)', d: 'The characters from start to end (counted from 0; negative from the end).', ex: [['{T}.slice(-4)', 'last four']] },
    { f: 'Methods', n: '.split', s: '.split(sep)', d: 'The parts as a list; [0] is the first.', ex: [['{T}.split(" ")[0]', 'first word']] },
    { f: 'Methods', n: '.replaceAll', s: '.replaceAll(text, by)', d: 'Every occurrence of the text replaced.', ex: [['{T}.replaceAll(" ", "")', 'without spaces']] },
    { f: 'Methods', n: '.toUpperCase', s: '.toUpperCase()', d: 'In capitals.', ex: [['{T}.toUpperCase()', 'capitals']] },
    { f: 'Methods', n: '.trim', s: '.trim()', d: 'Spaces removed at both ends.', ex: [['{T}.trim()', 'trimmed']] },
    { f: 'Methods', n: '.padStart', s: '.padStart(n, "0")', d: 'Filled on the left up to n characters.', ex: [['{N}.padStart(6, "0")', 'six digits']] },
    { f: 'Operators', n: '===', s: ' === ', d: 'Equal (the value as written: compare text with text). !== for different.', ex: [['{T} === ""', 'empty cell']] },
    { f: 'Operators', n: '> < >= <=', s: ' > ', d: 'Compare numbers or dates: read them with num() or date() first.', ex: [['num({N}) >= 100', 'at least 100'], ['date({D}) < today()', 'in the past']] },
    { f: 'Operators', n: '&&', s: ' && ', d: 'And: both must hold.', ex: [['!empty({T}) && num({N}) > 0', 'both']] },
    { f: 'Operators', n: '||', s: ' || ', d: 'Or: either may hold.', ex: [['empty({T}) || empty({T2})', 'one of them empty']] },
    { f: 'Operators', n: '!', s: '!', d: 'Not: reverses a test.', ex: [['!contains({T}, "test")', 'without "test"']] },
    { f: 'Operators', n: '? :', s: ' ? "yes" : "no"', d: 'If … then … else.', ex: [['num({N}) > 1000 ? "big" : "small"', 'a condition']] },
    { f: 'Operators', n: '+ - * /', s: ' + ', d: 'Arithmetic on numbers; + also joins text.', ex: [['{T} + " " + {T2}', 'join text'], ['num({N}) / 2', 'half']] }
];
/* Numbers are written the way the file most likely writes them: a decimal
   comma in a ;-separated file (the French spreadsheet convention), a point otherwise. */
let fxComma = false;
function fxNumText(s) { return fxComma ? s.replace('.', ',') : s; }
function fxOut(v) {
    if (v == null || (typeof v === 'number' && !isFinite(v))) return '';
    if (typeof v === 'number') return fxNumText(String(Math.round(v * 1e10) / 1e10));
    if (v instanceof Date) return isNaN(v) ? '' : v.getUTCHours() || v.getUTCMinutes() || v.getUTCSeconds() ? v.toISOString().replace('.000Z', 'Z') : v.toISOString().slice(0, 10);
    return String(v);
}

/* {Name} → $[i]. A name is matched exactly, then ignoring case, accents and symbols. */
function compileFormula(t, src) {
    const slugs = t.headers.map(h => slugify(h)), unknown = [], used = [];
    const body = src.replace(/\{([^{}\n]+)\}/g, (m, name) => {
        let c = t.headers.indexOf(name);
        if (c < 0) c = t.headers.indexOf(name.trim());
        if (c < 0) c = slugs.indexOf(slugify(name));
        if (c < 0) { unknown.push(name); return 'undefined'; }
        if (!used.includes(c)) used.push(c);
        return `$[${c}]`;
    });
    if (unknown.length) return { error: `Unknown column: {${unknown[0]}}` };
    if (!body.trim()) return { error: '' };
    try { return { fn: new Function('$', 'row', ...Object.keys(FX), `"use strict"; return (${body}\n);`), used }; }
    catch (e) { return { error: e.message }; }
}
/* The $ array a formula reads: only the columns it names are filled (cellOf,
   which reads one field without splitting the record — most formulas use one
   or two of 20 columns), the rest stay undefined; a fresh array each time, so
   a formula assigning to $[i] cannot touch the shared cache. */
function rowArgs(t, r, used) {
    const d = new Array(t.headers.length);
    for (let k = 0; k < used.length; k++) { const c = used[k]; d[c] = cellStr(cellOf(r, c)); }
    return d;
}
function evalRow(fn, t, r, used) {
    return fxOut(fn(rowArgs(t, r, used), r.id, ...Object.values(FX)));
}

const FX_EXAMPLES = [   // {N} {D} {T} {T2}: a number, date, text and second text column of the file (fxExample(), 30-…)
    ['{T} + " " + {T2}', 'join two columns'],
    ['join(" ", {T}, {T2})', 'join, skipping empty values'],
    ['num({N}) * 1.2', 'arithmetic'],
    ['fixed(num({N}) / 3, 2)', 'two decimals'],
    ['days({D}, today())', 'days since a date'],
    ['year({D})', 'year of a date'],
    ['fmtDate({D}, "yyyy-mm-dd")', 'reformat a date'],
    ['extract({T}, "@(.+)$")', 'regex group (e-mail domain)'],
    ['num({N}) > 1000 ? "big" : "small"', 'condition'],
    ['first({T}, {T2}, "none")', 'first non-empty value']
];

function openFormula(col) {
    const t = T(); if (!t || !t.loaded) return;
    fxComma = t.detectedDelim === ';';
    document.getElementById('fx-cols').innerHTML = t.headers.map(h =>
        `<span class="dk" title="Insert this column" onclick="fxInsert(${esc(JSON.stringify('{' + h + '}'))})">${esc(h)}</span>`).join('');
    /* Examples use columns of the file of the right type, so they run as they are. */
    document.getElementById('fx-ex').innerHTML = FX_EXAMPLES.map(([x, l]) => {
        const s = fxExample(t, x);
        return `<div class="fx-ex" onclick="fxSet(${esc(JSON.stringify(s))})"><code>${esc(s)}</code><span>${esc(l)}</span></div>`;
    }).join('');
    document.getElementById('fx-fns').innerHTML = FX_FAMS.filter(f => f !== 'Operators' && f !== 'Methods').map(f => `<b>${f}</b> `
        + FX_DOC.filter(x => x.f === f).map(x => `<span class="fx-fn" onclick="openFxPicker('formula', ${esc(JSON.stringify(x.n))})" title="${esc(x.s + ' — ' + x.d)}">${esc(x.n)}</span>`).join(' · ')).join('<br>')
        + '<br><span class="fx-fn" onclick="openFxPicker(\'formula\', \'.startsWith\')">String methods</span> · <span class="fx-fn" onclick="openFxPicker(\'formula\', \'&&\')">operators</span>';
    const dest = document.getElementById('fx-dest'), prev = dest.value;
    dest.innerHTML = `<optgroup label="New column">` + t.headers.map((h, i) => `<option value="new:${i}">after ${esc(h)}</option>`).join('') + '</optgroup>'
        + `<optgroup label="Replace the values of (rows shown)">` + t.headers.map((h, i) => `<option value="set:${i}">${esc(h)}</option>`).join('') + '</optgroup>';
    dest.value = col != null ? `new:${col}` : (prev && dest.querySelector(`option[value="${prev}"]`) ? prev : `new:${t.headers.length - 1}`);
    document.getElementById('modal-bg').style.display = 'block';
    document.getElementById('modal-formula').style.display = 'block';
    fxRefresh();
    document.getElementById('fx-expr').focus();
}
function fxInsert(text) {
    const ta = document.getElementById('fx-expr');
    ta.setRangeText(text, ta.selectionStart, ta.selectionEnd, 'end');
    ta.focus(); fxRefresh();
}
function fxSet(text) { const ta = document.getElementById('fx-expr'); ta.value = text; ta.focus(); fxRefresh(); }

/* Every row the formula would write, with the error count; the preview shows the first ones. */
function fxRun(t, fn, rows, used) {
    const out = new Array(rows.length); let errors = 0, firstErr = '';
    const fx = Object.values(FX);
    visitRows(t, rows, (r, i) => {
        try { out[i] = fxOut(fn(rowArgs(t, r, used), r.id, ...fx)); }
        catch (e) { errors++; if (!firstErr) firstErr = e.message; out[i] = ''; }
    });
    return { out, errors, firstErr };
}
let fxTimer = 0;
function fxRefresh() {
    clearTimeout(fxTimer);
    fxTimer = setTimeout(fxRefreshNow, 120);             // a formula runs over every row: not at each keystroke
    const dest = document.getElementById('fx-dest').value;
    document.getElementById('fx-name').disabled = dest.startsWith('set:');
}
function fxRefreshNow() {
    const t = T(); if (!t) return;
    const src = document.getElementById('fx-expr').value, stats = document.getElementById('fx-stats'), pv = document.getElementById('fx-pv');
    const go = document.getElementById('fx-go');
    go.disabled = true;
    const c = compileFormula(t, src);
    if (c.error !== undefined) {
        stats.innerHTML = c.error ? `<span class="warn">${esc(c.error)}</span>` : 'Type a formula, or pick an example below.';
        pv.innerHTML = ''; return;
    }
    const dest = document.getElementById('fx-dest').value, set = dest.startsWith('set:'), rows = set ? t.filteredData : t.allData;
    const { out, errors, firstErr } = fxRun(t, c.fn, rows, c.used);
    const filled = out.filter(v => v !== '').length;
    let changed = 0;
    if (set) { const col = +dest.slice(4); rows.forEach((r, i) => { if (out[i] !== cellStr(r.data[col])) changed++; }); }
    stats.innerHTML = (set ? `<b>${fmt(changed)}</b> cells will change in the ${fmt(rows.length)} ${hasFilter(t) ? 'filtered ' : ''}rows`
            : `<b>${fmt(filled)}</b> of ${fmt(rows.length)} rows get a value`)
        + (errors ? `<br><span class="warn">${fmt(errors)} rows raise an error (their cell stays empty): ${esc(firstErr)}</span>` : '');
    const show = c.used.slice(0, 4), name = set ? t.headers[+dest.slice(4)] : (document.getElementById('fx-name').value.trim() || 'Computed');
    pv.innerHTML = `<tr>${show.map(i => `<th style="padding: 4px 8px;">${esc(t.headers[i])}</th>`).join('')}<th style="padding: 4px 8px; color: var(--prim);">${esc(name)}</th></tr>`
        + rows.slice(0, 8).map((r, i) => `<tr>${show.map(c => `<td class="src">${esc(cellStr(r.data[c]))}</td>`).join('')}`
            + (out[i] ? `<td title="${esc(out[i])}">${esc(out[i])}</td>` : '<td class="empty">empty</td>') + '</tr>').join('');
    go.disabled = set ? !changed : !rows.length;
}

function applyFormula() {
    const t = T(); if (!t) return;
    clearTimeout(fxTimer);
    const src = document.getElementById('fx-expr').value, c = compileFormula(t, src);
    if (!c.fn) return;
    const dest = document.getElementById('fx-dest').value, at = +dest.slice(4);
    closeAllModals();
    if (dest.startsWith('set:')) {
        const rows = t.filteredData, { out, errors } = fxRun(t, c.fn, rows, c.used), ch = [];
        rows.forEach((r, i) => { if (out[i] !== cellStr(r.data[at])) ch.push([r, r.data[at], out[i]]); });
        if (!ch.length) return;
        const ed = rowEdits();
        for (const [r, , nv] of ch) ed.set(r, at, nv);
        t.modificationsLog.push({ id: '-', col: t.headers[at], old: 'formula', new: `${ch.length} cells`, what: `${fmt(ch.length)} cells computed in ${t.headers[at]}`,
            undo: () => ed.undo() });
        updateSaveBtn(); renderTabBar(); render();
        flash(ch.map(([r]) => [r, at]));
        setStats(`${t.name} | ${fmt(ch.length)} cells computed in ${t.headers[at]}${errors ? `, ${fmt(errors)} errors left empty` : ''} — not written yet, use Save.`);
        return;
    }
    const name = document.getElementById('fx-name').value.trim() || 'Computed';
    const { out, errors } = fxRun(t, c.fn, t.allData, c.used);
    const headers = [...t.headers.slice(0, at + 1), name, ...t.headers.slice(at + 1)];
    restructure(t, headers, (d, r, i) => { const row = pad(d, at + 1); return [...row.slice(0, at + 1), out[i], ...row.slice(at + 1)]; },
        c => c <= at ? c : c + 1, `column "${name}" computed`);
    if (errors) setStats(`${t.name} | Column "${name}" computed, ${fmt(errors)} rows with an error left empty — not written yet, use Save.`);
}
