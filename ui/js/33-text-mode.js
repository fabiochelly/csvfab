/* ---------------------------------------------------------------
   TEXT FILES
   Any file that is not a table (csv, tsv — workbooks and JSON are
   converted first, or opened as text on request) opens in "Raw text"
   mode, the delimiter menu's last choice: one line, one cell (the
   scan worker's `lines` mode, 21-…: no delimiter, no quote, empty lines
   kept), every space kept (.cell.tx: white-space pre), saved back byte
   for byte (no quoting, no line break added at the end of a file that
   had none — streamCSV). The column is titled by the language found
   from the file name and coloured by a tokenizer per line (synHtml()),
   with each palette's own VS Code theme colours (--sy-* in themes.css).
   The tokenizers know nothing of the lines before: a block comment or
   a multi-line string is coloured only on the lines where it opens
   (and on doc-comment lines starting with *), which keeps the view
   random-access on a file of any size.
   The status bar shows Ln / Col as VS Code does: the selected line, and
   while a line is edited the caret's column (tabs counted as 4).
----------------------------------------------------------------*/
const TABLE_RE = /\.(csv|tsv|tab|psv)$/i;
/* Read as a table by default: the extensions above, and the converted files (whose name is .csv). */
function isTableName(name) { return TABLE_RE.test(String(name || '')); }

const KW = s => new Set(s.split(' '));
const SYN_LANGS = [
    ['JavaScript', 'js', /\.(js|mjs|cjs|jsx)$/i], ['TypeScript', 'js', /\.(ts|mts|cts|tsx)$/i],
    ['JSON', 'json', /\.(json|jsonc|json5|jsonl|ndjson|geojson|webmanifest|ipynb)$/i],
    ['Python', 'py', /\.(py|pyw|pyi)$/i],
    ['Shell', 'sh', /(\.(sh|bash|zsh|fish|ksh)|^\.?(bashrc|zshrc|profile|bash_profile|zprofile|envrc))$/i],
    ['PowerShell', 'sh', /\.(ps1|psm1|psd1)$/i], ['Batch', 'bat', /\.(bat|cmd)$/i],
    ['Dockerfile', 'sh', /(^|\.)(dockerfile|containerfile)$/i], ['Makefile', 'sh', /^(gnu)?makefile$|\.mk$/i],
    ['SQL', 'sql', /\.(sql|ddl|dml)$/i],
    ['CSS', 'css', /\.(css|scss|sass|less)$/i],
    ['HTML', 'html', /\.(html?|xhtml|vue|svelte|astro)$/i],
    ['XML', 'html', /\.(xml|svg|xsd|xsl|xslt|plist|csproj|vbproj|props|targets|config|rss|atom|kml|gpx|xaml|resx|wsdl)$/i],
    ['YAML', 'yaml', /\.(ya?ml)$/i],
    ['TOML', 'ini', /\.toml$/i],
    ['INI', 'ini', /(\.(ini|cfg|conf|cnf|properties|env|editorconfig|desktop|service|timer|socket|mount|gitconfig|npmrc)|^\.(env(\..+)?|gitconfig|npmrc|editorconfig))$/i],
    ['Markdown', 'md', /\.(md|markdown|mdx|rst)$/i],
    ['C', 'c', /\.[ch]$/i], ['C++', 'c', /\.(cpp|cc|cxx|c\+\+|hpp|hh|hxx|ino)$/i], ['C#', 'c', /\.cs$/i],
    ['Java', 'c', /\.java$/i], ['Kotlin', 'c', /\.kts?$/i], ['Go', 'c', /\.go$/i], ['Rust', 'c', /\.rs$/i],
    ['Swift', 'c', /\.swift$/i], ['PHP', 'c', /\.php\d?$/i], ['Scala', 'c', /\.(scala|sc)$/i], ['Dart', 'c', /\.dart$/i],
    ['Groovy', 'c', /\.(groovy|gradle)$/i],
    ['Ruby', 'rb', /(\.(rb|rake|gemspec)|^(gemfile|rakefile))$/i], ['Lua', 'lua', /\.lua$/i], ['Perl', 'sh', /\.(pl|pm)$/i], ['R', 'py', /\.r$/i],
    ['Diff', 'diff', /\.(diff|patch|rej)$/i],
    ['Log', 'log', /\.(log|out|err)(\.\d+)?$|\.log\./i],
];
/* The language of a file name: [label, grammar]. Anything unknown is read as a log — dates,
   levels, quoted strings and numbers, which suits plain text too. */
function langOf(name) {
    const n = baseName(String(name || ''));
    const m = SYN_LANGS.find(l => l[2].test(n));
    return m ? { label: m[0], g: m[1] } : { label: 'Text', g: 'log' };
}

/* Grammars: rules tried left to right at each position (one regex of alternatives, a
   group each), then identifiers looked up in the keyword sets. Classes: k keyword,
   c control flow, s string, n number, m comment, f function, t type, v variable,
   p property, r regex, g tag, a attribute, e escape/entity, o constant,
   E W I D log levels, h heading, b bold, ins del (diff). */
const STR2 = String.raw`"(?:[^"\\]|\\.)*(?:"|$)`, STR1 = String.raw`'(?:[^'\\]|\\.)*'`, STR1O = String.raw`'(?:[^'\\]|\\.)*(?:'|$)`;
const NUM = String.raw`\b(?:0[xX][\da-fA-F_]+|0[bB][01_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)[a-zA-Z]{0,3}\b|\B\.\d+\b`;
const ID = String.raw`[A-Za-z_$][\w$]*`;
const SYN = {
    js: { rules: [[String.raw`\/\/.*`, 'm'], [String.raw`\/\*.*?(?:\*\/|$)`, 'm'], [String.raw`^\s*\*(?:\s.*|\/?$)`, 'm'], [STR2, 's'], [STR1O, 's'], ['`(?:[^`\\\\]|\\\\.)*(?:`|$)', 's'],
            [String.raw`(?<=[(,=:\[!&|?{};]\s*|^\s*|return\s+)\/(?![*/\s])(?:[^/\\\[]|\\.|\[(?:[^\]\\]|\\.)*\])+\/[dgimsuyv]*`, 'r'], [String.raw`@${ID}`, 'f'], [NUM, 'n']],
        kw: KW('var let const function class extends new delete typeof instanceof in of void this super static get set async await yield import export from as default interface type enum implements namespace declare abstract public private protected readonly keyof infer is satisfies accessor'),
        ctl: KW('if else for while do switch case break continue return throw try catch finally with debugger'),
        con: KW('true false null undefined NaN Infinity'), types: true },
    c: { rules: [[String.raw`\/\/.*`, 'm'], [String.raw`\/\*.*?(?:\*\/|$)`, 'm'], [String.raw`^\s*\*(?:\s.*|\/?$)`, 'm'], [String.raw`^\s*#\s*\w+`, 'k'], [String.raw`[@#]\[?${ID}`, 'f'],
            [STR2, 's'], [STR1, 's'], ['`[^`]*(?:`|$)', 's'], [String.raw`\$${ID}`, 'v'], [NUM, 'n']],
        kw: KW('auto char const double enum extern float int long register short signed sizeof static struct typedef union unsigned void volatile class namespace template typename using virtual public private protected friend operator new delete this inline explicit mutable constexpr override final abstract interface extends implements package import var val fun func fn let mut pub impl trait mod use crate self Self struct enum type where dyn ref move unsafe async await go defer chan map range select byte rune string bool object dynamic readonly sealed internal partial event delegate lateinit companion data object suspend function echo require include namespace global static local yield'),
        ctl: KW('if else for while do switch case default break continue return goto try catch finally throw throws match loop when in is as guard fallthrough elif foreach'),
        con: KW('true false null nullptr NULL nil None undefined'), types: true },
    py: { rules: [[String.raw`#.*`, 'm'], [String.raw`[rRbBuUfF]{0,2}(?:"""|''').*?(?:"""|'''|$)`, 's'], [String.raw`[rRbBuUfF]{0,2}` + STR2, 's'], [String.raw`[rRbBuUfF]{0,2}` + STR1O, 's'], [String.raw`^\s*@[\w.]+`, 'f'], [NUM, 'n']],
        kw: KW('def class lambda import from as global nonlocal del pass assert async await yield and or not in is function library'),
        ctl: KW('if elif else for while break continue return try except finally raise with match case repeat'),
        con: KW('True False None self cls TRUE FALSE NULL NA'), types: true },
    rb: { rules: [[String.raw`#.*`, 'm'], [STR2, 's'], [STR1O, 's'], [String.raw`(?<![\w:]):${ID}[?!]?`, 'o'], [String.raw`@@?${ID}|\$${ID}`, 'v'], [NUM, 'n']],
        kw: KW('def class module end do begin alias and or not defined? self super yield require require_relative include extend attr_accessor attr_reader attr_writer private protected public lambda proc then'),
        ctl: KW('if elsif else unless while until for in case when break next redo retry return rescue ensure raise loop'),
        con: KW('true false nil'), types: true },
    lua: { rules: [[String.raw`--.*`, 'm'], [STR2, 's'], [STR1O, 's'], [String.raw`\[\[.*?(?:\]\]|$)`, 's'], [NUM, 'n']],
        kw: KW('local function end and or not in do then self require'), ctl: KW('if elseif else for while repeat until break return goto'), con: KW('true false nil'), types: false },
    sh: { rules: [[String.raw`(?<=^|\s)#.*`, 'm'], [STR2, 's'], [String.raw`'[^']*(?:'|$)`, 's'], [String.raw`\$\{[^}]*\}|\$\(|\$[\w@#?$!*-]+`, 'v'], [String.raw`(?<=^|\s)--?[\w-]+`, 'a'], [NUM, 'n']],
        kw: KW('function local export declare readonly unset alias source set shift trap eval exec in echo printf read cd test FROM RUN CMD ENTRYPOINT COPY ADD ENV ARG WORKDIR EXPOSE USER VOLUME LABEL AS param begin process end my our sub use'),
        ctl: KW('if then else elif fi for while until do done case esac break continue return exit select switch foreach'),
        con: KW('true false'), types: false },
    bat: { rules: [[String.raw`^\s*(?:@?rem\b|::).*`, 'm', 'i'], [STR2, 's'], [String.raw`%[\w~]+%?|!\w+!`, 'v'], [String.raw`^\s*:\w+`, 'f'], [NUM, 'n']],
        kw: KW('echo set setlocal endlocal call start cd copy del move mkdir rmdir exist defined not shift pushd popd'), ctl: KW('if else for in do goto exit errorlevel'), con: KW('on off'), ci: true, types: false },
    sql: { rules: [[String.raw`--.*`, 'm'], [String.raw`\/\*.*?(?:\*\/|$)`, 'm'], [String.raw`'(?:[^']|'')*(?:'|$)`, 's'], [String.raw`"[^"]*"|\x60[^\x60]*\x60|\[[^\]]*\]`, 'v'], [String.raw`[:@$]${ID}`, 'v'], [NUM, 'n']],
        kw: KW('select from where and or not in is null like ilike between exists insert into values update set delete create alter drop table view index unique primary key foreign references constraint default join inner left right full outer cross on using group by order having limit offset union all distinct as asc desc with recursive returning database schema grant revoke begin commit rollback transaction trigger procedure function returns language declare if replace temporary temp cascade column add rename to over partition window fetch first next rows only top int integer bigint smallint varchar char text date timestamp boolean numeric decimal float real serial count sum avg min max coalesce cast'),
        ctl: KW('case when then else end return'), con: KW('true false null'), ci: true, types: false },
    css: { rules: [[String.raw`\/\*.*?(?:\*\/|$)`, 'm'], [String.raw`^\s*\*(?:\s.*|\/?$)`, 'm'], [String.raw`\/\/.*`, 'm'], [STR2, 's'], [STR1O, 's'], [String.raw`#[\da-fA-F]{3,8}\b`, 'n'],
            [String.raw`@[\w-]+`, 'c'], [String.raw`--[\w-]+|\$[\w-]+`, 'v'], [String.raw`[\w-]+(?=\s*:(?!:)[^{;]*(?:;|$))`, 'p'], [String.raw`[.#][A-Za-z_-][\w-]*|::?[\w-]+`, 't'],
            [String.raw`-?\b\d*\.?\d+(?:%|[a-z]+)?\b`, 'n'], [String.raw`[\w-]+(?=\()`, 'f'], [String.raw`!important`, 'c']],
        kw: KW('and not only from to'), ctl: new Set(), con: new Set(), types: false, noId: true },
    html: { rules: [[String.raw`<!--.*?(?:-->|$)`, 'm'], [String.raw`<!\[CDATA\[.*?(?:\]\]>|$)`, 's'], [String.raw`<[!?][\w-]*`, 'g'], [String.raw`<\/?[\w:.-]+`, 'g'], [String.raw`\/?>|\?>`, 'g'],
            [String.raw`[\w:.@-]+(?==)`, 'a'], [STR2, 's'], [String.raw`'[^']*(?:'|$)`, 's'], [String.raw`&#?\w+;`, 'e'], [String.raw`\{\{.*?(?:\}\}|$)`, 'v']],
        kw: new Set(), ctl: new Set(), con: new Set(), types: false, noId: true },
    json: { rules: [[String.raw`"(?:[^"\\]|\\.)*"(?=\s*:)`, 'p'], [STR2, 's'], [String.raw`-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b`, 'n'], [String.raw`\/\/.*`, 'm']],
        kw: new Set(), ctl: new Set(), con: KW('true false null'), types: false },
    yaml: { rules: [[String.raw`(?<=^|\s)#.*`, 'm'], [String.raw`^(?:---|\.\.\.)\s*$`, 'k'], [String.raw`^\s*(?:-\s+)?[^\s#:'"][^:#]*?(?=\s*:(?:\s|$))`, 'p'], [String.raw`^\s*(?:-\s+)?"[^"]*"(?=\s*:(?:\s|$))`, 'p'],
            [STR2, 's'], [String.raw`'(?:[^']|'')*(?:'|$)`, 's'], [String.raw`[&*][\w-]+`, 't'], [String.raw`!{1,2}[\w/:.-]*`, 't'], [String.raw`(?<=:\s*)[|>][-+]?\d*\s*$`, 'k'], [NUM, 'n']],
        kw: new Set(), ctl: new Set(), con: KW('true false null yes no on off True False Null Yes No TRUE FALSE NULL ~'), types: false },
    ini: { rules: [[String.raw`^\s*[;#].*`, 'm'], [String.raw`^\s*\[\[?[^\]]*\]\]?`, 't'], [String.raw`^\s*(?:export\s+)?[\w.\-"' ]+?(?=\s*[=:])`, 'p'], [STR2, 's'], [String.raw`'[^']*(?:'|$)`, 's'],
            [String.raw`\$\{[^}]*\}|%\([\w]+\)s|%[\w]+%`, 'v'], [NUM, 'n'], [String.raw`(?<=\s)[;#].*`, 'm']],
        kw: new Set(), ctl: new Set(), con: KW('true false yes no on off null none True False'), types: false },
    md: { rules: [[String.raw`^#{1,6}\s.*`, 'h'], [String.raw`^\s*(?:\x60{3,}|~{3,}).*`, 's'], [String.raw`^\s*>.*`, 'm'], [String.raw`^\s*(?:[-*+]|\d+[.)])(?=\s)`, 'k'], [String.raw`^\s*(?:[-*_]\s*){3,}$`, 'k'],
            [String.raw`\x60[^\x60]+\x60`, 's'], [String.raw`!?\[[^\]]*\]\([^)]*\)|<https?:[^>]+>|https?:\/\/\S+`, 'f'], [String.raw`\*\*[^*]+\*\*|__[^_]+__`, 'b'], [String.raw`(?<![\w*])\*[^*\s][^*]*\*|(?<!\w)_[^_\s][^_]*_(?!\w)`, 'i'], [String.raw`^\s*\|.*`, 'p']],
        kw: new Set(), ctl: new Set(), con: new Set(), types: false, noId: true },
    diff: { rules: [[String.raw`^(?:\+\+\+|---)\s.*`, 'b'], [String.raw`^@@.*`, 'k'], [String.raw`^(?:diff|index|new file|deleted file|similarity|rename|old mode|new mode)\b.*`, 'm'], [String.raw`^\+.*`, 'ins'], [String.raw`^-.*`, 'del'], [String.raw`^\\.*`, 'm']],
        kw: new Set(), ctl: new Set(), con: new Set(), types: false, noId: true },
    log: { rules: [[String.raw`\b\d{4}-\d\d-\d\d(?:[T ]\d\d:\d\d(?::\d\d)?(?:[.,]\d+)?(?:Z|[+-]\d\d:?\d\d)?)?\b|\b\d\d[/.]\d\d[/.]\d{2,4}(?:[ T]\d\d:\d\d(?::\d\d)?)?\b|\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}(?:\s+\d\d:\d\d:\d\d)?\b|\b\d\d:\d\d:\d\d(?:[.,]\d+)?\b`, 't'],
            [String.raw`\b(?:FATAL|CRITICAL|CRIT|ERROR|ERR|SEVERE|EMERG|ALERT|PANIC|FAIL(?:ED|URE)?|Error|Fatal|Failed|Exception)\b|\[(?:E|F)\]`, 'E'],
            [String.raw`\b(?:WARNING|WARN|Warning)\b|\[W\]`, 'W'], [String.raw`\b(?:INFO|NOTICE|Info|SUCCESS|OK)\b|\[I\]`, 'I'], [String.raw`\b(?:DEBUG|TRACE|VERBOSE|FINE|FINER|FINEST|Debug|Trace)\b|\[D\]`, 'D'],
            [String.raw`\b[\w.$]+(?:Exception|Error)\b`, 'c'], [String.raw`https?:\/\/[^\s"'<>]+|\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b`, 'f'], [STR2, 's'], [String.raw`(?<=\s|^)'[^'\n]*'`, 's'],
            [String.raw`\b[\w.-]+(?==)`, 'p'], [String.raw`\b0x[\da-fA-F]+\b|\b[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}\b|(?<![\w.-])-?\d+(?:[.,]\d+)?\b`, 'n']],
        kw: new Set(), ctl: new Set(), con: KW('true false null None nil'), types: false },
};
/* Each grammar compiled once: one regex, a capture group per rule, identifiers as the last group. */
function synGrammar(g) {
    const G = SYN[g] || SYN.log;
    if (!G.re) {
        const alts = G.rules.map(r => `(${r[0]})`);
        if (!G.noId) alts.push(String.raw`([A-Za-z_$][\w$]*[?!]?)`);
        G.re = new RegExp(alts.join('|'), 'g' + (G.ci || G.rules.some(r => r[2] === 'i') ? 'i' : ''));
        G.cls = G.rules.map(r => r[1]);
    }
    return G;
}
const SYN_MAX = 4000;           // a minified line of a megabyte: coloured over its first few thousand characters
function synEsc(s) { return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;'); }
function synHtml(line, g) {
    if (!line) return '';
    const G = synGrammar(g), re = G.re, cls = G.cls, head = line.length > SYN_MAX ? line.slice(0, SYN_MAX) : line;
    let out = '', last = 0, m;
    re.lastIndex = 0;
    while ((m = re.exec(head))) {
        const s = m[0];
        if (!s) { re.lastIndex++; continue; }
        let k = 1; while (m[k] === undefined) k++;
        let c = cls[k - 1];
        if (c === undefined) {                       // an identifier
            const w = G.ci ? s.toLowerCase() : s;
            c = G.ctl.has(w) ? 'c' : G.kw.has(w) ? 'k' : G.con.has(w) || G.con.has(s) ? 'o'
                : head.charCodeAt(m.index + s.length) === 40 ? 'f'                        // followed by "("
                : G.types && s.charCodeAt(0) >= 65 && s.charCodeAt(0) <= 90 && /[a-z]/.test(s) ? 't' : '';
            if (!c) continue;                         // a plain name: left in the text run
        }
        if (m.index > last) out += synEsc(head.slice(last, m.index));
        out += `<span class="sy-${c}">${synEsc(s)}</span>`;
        last = m.index + s.length;
    }
    out += synEsc(head.slice(last));
    return head === line ? out : out + synEsc(line.slice(SYN_MAX));
}

/* A text cell: the line coloured, then a filter's or the find's marks over its text. */
function textCellHtml(t, c, cIdx) {
    const s = c == null ? '' : String(c);
    return showBreaks(highlightCell(s, cIdx, t.hl, synHtml(s, t.lang.g)));
}
/* The text column's width: the longest line among the first and the shown ones (tabs as 4),
   at least the view's width — a wide column scrolls sideways as an editor does. */
function textColWidth(t) {
    const adv = cellAdv(), rows = t.filteredData, [v0] = viewRows(t);
    let n = 0;
    const see = r => { const s = cellStr(cellOf(r, 0)); if (s.length > n) { const x = s.indexOf('\t') < 0 ? s.length : s.replace(/[^\t]*\t/g, a => a.slice(0, -1).padEnd(Math.ceil(a.length / 4) * 4)).length; if (x > n) n = x; } };
    for (let i = 0; i < Math.min(rows.length, 5000); i++) see(rows[i]);
    for (let i = v0; i < Math.min(rows.length, v0 + 200); i++) see(rows[i]);
    const view = (container.offsetWidth || 800) - 24 - idxColW - 2;   // 24: the vertical scrollbar, absent while the first rows are measured
    return Math.max(view, Math.min(Math.ceil(n * adv + 40), 30000));
}

/* Ln / Col in the status bar, VS Code's way. */
function textPos(t, ed) {
    const el = document.getElementById('sb-pos'); if (!el) return;
    if (!t || !t.lang || !t.loaded) { el.style.display = 'none'; return; }
    let txt = '';
    if (ed) {
        const v = ed.input.value, a = ed.input.selectionStart, b = ed.input.selectionEnd;
        const nl = v.lastIndexOf('\n', a - 1), before = v.slice(nl + 1, a);
        let col = 0; for (const ch of before) col = ch === '\t' ? (Math.floor(col / 4) + 1) * 4 : col + 1;
        const extra = (v.slice(0, a).match(/\n/g) || []).length;
        txt = `Ln ${fmt(ed.line + extra)}, Col ${fmt(col + 1)}${b > a ? ` (${fmt(b - a)} selected)` : ''}`;
    } else {
        const rg = selRange(t);
        if (rg) {
            const r = t.filteredData[sel.fr];
            txt = r ? `Ln ${fmt(r.id)}, Col 1${rg.r1 > rg.r0 ? ` (${fmt(rg.r1 - rg.r0 + 1)} lines selected)` : ''}` : '';
        }
    }
    el.textContent = txt;
    el.style.display = txt ? '' : 'none';
}
