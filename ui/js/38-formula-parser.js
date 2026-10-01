/* ---------------------------------------------------------------
   SAFE FORMULAS
   A formula used to be handed to new Function() as it was typed: any
   JavaScript at all, run inside the page — which holds the token that
   reads and writes every file of the user. Harmless while only the
   user's keyboard writes formulas; a formula pasted from someone else
   (or, later, loaded from a recipe or a file) could read ~/.ssh and send
   it away. So a formula is now parsed into a tree, checked against what
   the language allows, and JavaScript is written back from the tree:
   whatever was typed, the compiled code holds nothing else than
     literals (numbers, "strings", 'strings', /regexes/), {Column}, row
     (or v in the duplicates' cleaning), true false null undefined NaN
     Infinity, [lists], ( ), unary ! - +, ** * / % + -
     < > <= >= == != === !==, && || ??, … ? … : …,
     calls of the helpers in FX (by own name: 'constructor' in FX is true
     through Object.prototype), of Math's functions, and of the methods
     in FX_SAFE_METHODS — a list of names, which never holds constructor,
     __proto__, prototype, call, apply, bind or anything that returns a
     function —, and x[i] through fxAt(), which only indexes a string or
     a list by an integer (a name computed at run time — "constructor" —
     cannot be checked before).
   No assignment, no other name (window, fetch, CSVFAB_TOKEN, Function,
   $…), no new, no arrow function, no template literal, no comma
   sequence. The checks all happen once, at compile time: the code run
   on each row is what the formula says, as fast as before (V8 compiles
   it the same way); fxAt() is the only run-time guard, and only in a
   formula that indexes.
----------------------------------------------------------------*/
const FX_SAFE_NATIVE = [
    // strings
    'length', 'at', 'charAt', 'charCodeAt', 'codePointAt', 'endsWith', 'includes', 'indexOf', 'lastIndexOf', 'localeCompare',
    'match', 'normalize', 'padEnd', 'padStart', 'repeat', 'replace', 'replaceAll', 'search', 'slice', 'split', 'startsWith',
    'substring', 'toLowerCase', 'toUpperCase', 'toLocaleLowerCase', 'toLocaleUpperCase', 'trim', 'trimStart', 'trimEnd',
    // numbers, lists, regexes, dates
    'toFixed', 'toPrecision', 'toString', 'join', 'concat', 'reverse', 'test',
    'getTime', 'getFullYear', 'getMonth', 'getDate', 'getDay', 'getHours', 'getMinutes', 'getSeconds',
    'getUTCFullYear', 'getUTCMonth', 'getUTCDate', 'getUTCDay', 'getUTCHours', 'getUTCMinutes', 'getUTCSeconds', 'toISOString'
];
const FX_SAFE_MATH = new Set(['abs', 'ceil', 'floor', 'round', 'trunc', 'sign', 'sqrt', 'cbrt', 'pow', 'min', 'max', 'log', 'log10', 'log2', 'exp', 'random', 'PI', 'E']);
const FX_SAFE_CONST = new Set(['true', 'false', 'null', 'undefined', 'NaN', 'Infinity']);
let FX_SAFE_METHODS = null;               // built on first use: FX_METHOD (17-…) is a const of a file after nothing, but read lazily all the same
function fxSafeMethods() {
    if (!FX_SAFE_METHODS) FX_SAFE_METHODS = new Set([...FX_SAFE_NATIVE, ...FX_ON_TEXT, ...FX_ON_NUMBER, ...FX_ON_DATE]);
    return FX_SAFE_METHODS;
}
/* The one run-time guard: x[i] only reads a character of a string or an item of a list. */
function fxAt(x, i) {
    return (typeof x === 'string' || Array.isArray(x)) && Number.isInteger(i) ? x[i] : undefined;
}

class FxError extends Error {
    constructor(msg, at) { super(msg); this.at = at; }
}

/* Tokens: { k: kind, v: text, at: offset }. Strings, numbers and regexes keep their exact
   source text, written back as it is — the lexer reads them by JavaScript's own rules, so
   what it calls one literal is one literal to V8 too. */
function fxTokens(src) {
    const out = [], L = src.length;
    let i = 0;
    const valueEnds = () => {                 // a / after a value divides; elsewhere it opens a regex
        const p = out[out.length - 1];
        return p && (p.k === 'num' || p.k === 'str' || p.k === 'col' || p.k === 'name' || p.k === 're' || (p.k === 'op' && (p.v === ')' || p.v === ']')));
    };
    while (i < L) {
        const c = src[i];
        if (/\s/.test(c)) { i++; continue; }
        const at = i;
        if (c === '{') {
            const e = src.indexOf('}', i);
            const name = e < 0 ? '' : src.slice(i + 1, e);
            if (e < 0 || !name || /[{\n]/.test(name)) throw new FxError('A { opens a column name: close it with }', at);
            out.push({ k: 'col', v: name, at }); i = e + 1; continue;
        }
        if (c === '"' || c === "'") {
            let j = i + 1;
            for (;;) {
                if (j >= L || src[j] === '\n' || src[j] === '\r') throw new FxError('A text in quotes is not closed', at);
                if (src[j] === '\\') { if (src[j + 1] === '\n' || src[j + 1] === '\r') throw new FxError('A text in quotes is not closed', at); j += 2; continue; }
                if (src[j] === c) break;
                j++;
            }
            out.push({ k: 'str', v: src.slice(i, j + 1), at }); i = j + 1; continue;
        }
        if (c === '`') throw new FxError('Use "…" or \'…\' for a text (no backquotes)', at);
        const num = /^(?:0[xX][0-9a-fA-F]+|(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)/.exec(src.slice(i, i + 64));
        if (num && !(c === '.' && !/\d/.test(src[i + 1] || ''))) {
            if (/[A-Za-z_$\d]/.test(src[i + num[0].length] || '')) throw new FxError(`“${src.slice(i, i + num[0].length + 1)}” is not a number`, at);
            out.push({ k: 'num', v: num[0], at }); i += num[0].length; continue;
        }
        if (/[A-Za-z_$À-￿]/.test(c)) {
            const m = /^[A-Za-z_$À-￿][\w$À-￿]*/.exec(src.slice(i));
            out.push({ k: 'name', v: m[0], at }); i += m[0].length; continue;
        }
        if (c === '/' && !valueEnds()) {
            let j = i + 1, cls = false;
            for (;;) {
                if (j >= L || src[j] === '\n') throw new FxError('A regular expression /…/ is not closed', at);
                if (src[j] === '\\') { j += 2; continue; }
                if (src[j] === '[') cls = true; else if (src[j] === ']') cls = false;
                else if (src[j] === '/' && !cls) break;
                j++;
            }
            const flags = /^[dgimsuyv]*/.exec(src.slice(j + 1))[0];
            try { new RegExp(src.slice(i + 1, j), flags); } catch (e) { throw new FxError(`Invalid regular expression: ${e.message}`, at); }
            out.push({ k: 're', v: src.slice(i, j + 1 + flags.length), at }); i = j + 1 + flags.length; continue;
        }
        const op = /^(?:===|!==|\*\*|==|!=|<=|>=|&&|\|\||\?\?|\?\.|[-+*/%<>!?:.,()[\]])/.exec(src.slice(i, i + 3));
        if (op) {
            if (op[0] === '?.') throw new FxError('Optional chaining ?. is not supported: test the value first', at);
            out.push({ k: 'op', v: op[0], at }); i += op[0].length; continue;
        }
        if (c === '=') throw new FxError('Use === to compare (= would assign)', at);
        if (c === ';') throw new FxError('A formula is one expression: no ;', at);
        throw new FxError(`Unexpected “${c}”`, at);
    }
    return out;
}

/* Recursive descent, JavaScript's precedence, straight to the code written back: each
   rule returns the JavaScript of its subtree, parenthesised, so precedence can never be
   reinterpreted. vars: the plain names allowed besides the constants (row, or v). */
function fxCompileSafe(src, colRef, vars) {
    const toks = fxTokens(src);
    let p = 0;
    const peek = () => toks[p], next = () => toks[p++];
    const is = v => toks[p] && toks[p].k === 'op' && toks[p].v === v;
    const expect = v => { if (!is(v)) fail(`Expected “${v}”`); p++; };
    const fail = msg => { const t = toks[p]; throw new FxError(t ? `${msg} near “${t.v}”` : `${msg} at the end`, t ? t.at : src.length); };

    function expr() { return ternary(); }
    function ternary() {
        const c = binary(0);
        if (!is('?')) return c;
        p++; const a = expr(); expect(':'); const b = expr();
        return `(${c} ? ${a} : ${b})`;
    }
    // Lowest first; ** is right-associative and handled in power().
    const LEVELS = [['??'], ['||'], ['&&'], ['==', '!=', '===', '!=='], ['<', '>', '<=', '>='], ['+', '-'], ['*', '/', '%']];
    function binary(level) {
        if (level === LEVELS.length) return power();
        let left = binary(level + 1);
        while (toks[p] && toks[p].k === 'op' && LEVELS[level].includes(toks[p].v)) {
            const op = next().v;
            left = `(${left} ${op} ${binary(level + 1)})`;
        }
        return left;
    }
    function power() {
        const base = unary();
        if (!is('**')) return base;
        p++;
        return `(${base} ** ${power()})`;
    }
    function unary() {
        if (is('!') || is('-') || is('+')) { const op = next().v; return `(${op}${unary()})`; }
        return postfix(primary());
    }
    function args() {
        const out = [];
        expect('(');
        if (!is(')')) { for (;;) { out.push(expr()); if (is(',')) { p++; continue; } break; } }
        expect(')');
        return out.join(', ');
    }
    function postfix(code) {
        for (;;) {
            if (is('.')) {
                p++;
                const t = next();
                if (!t || t.k !== 'name') fail('A method name is expected after “.”');
                if (!fxSafeMethods().has(t.v)) throw new FxError(`.${t.v} is not available in a formula`, t.at);
                /* Called, always — but .length: a method left uncalled would be a function value. */
                if (!is('(') && t.v !== 'length') throw new FxError(`.${t.v} is a method: write .${t.v}(…)`, t.at);
                code = is('(') ? `(${code}).${t.v}(${args()})` : `(${code}).${t.v}`;
            } else if (is('[')) {
                p++; const i = expr(); expect(']');
                code = `fxAt(${code}, ${i})`;
            } else if (is('(')) fail('Only a helper or a method can be called');
            else return code;
        }
    }
    function primary() {
        const t = next();
        if (!t) { p--; fail('A value is missing'); }
        if (t.k === 'num' || t.k === 'str' || t.k === 're') return t.v;
        if (t.k === 'col') return colRef(t.v, t.at);
        if (t.k === 'op' && t.v === '(') { const e = expr(); expect(')'); return `(${e})`; }
        if (t.k === 'op' && t.v === '[') {
            const items = [];
            if (!is(']')) { for (;;) { items.push(expr()); if (is(',')) { p++; continue; } break; } }
            expect(']');
            return `[${items.join(', ')}]`;
        }
        if (t.k === 'name') {
            if (Object.hasOwn(FX, t.v)) {
                if (!is('(')) throw new FxError(`${t.v} is a function: write ${t.v}(…)`, t.at);
                return `${t.v}(${args()})`;
            }
            if (t.v === 'Math') {
                expect('.');
                const m = next();
                if (!m || m.k !== 'name' || !FX_SAFE_MATH.has(m.v)) throw new FxError(`Math.${m ? m.v : ''} is not available in a formula`, m ? m.at : t.at);
                if (m.v === 'PI' || m.v === 'E') return `Math.${m.v}`;
                if (!is('(')) throw new FxError(`Math.${m.v} is a function: write Math.${m.v}(…)`, m.at);
                return `Math.${m.v}(${args()})`;
            }
            if (FX_SAFE_CONST.has(t.v) || vars.includes(t.v)) return t.v;
            throw new FxError(`Unknown name: ${t.v} — a column is written {${t.v}}, a helper is called with ( )`, t.at);
        }
        p--; fail('Unexpected');
    }

    const code = expr();
    if (p < toks.length) fail('Unexpected');
    return code;
}
