//! A raw-text file's colours (the web's 33-text-mode.js): the language from the file name,
//! a grammar per family of languages, a tokenizer per line. The tokenizers know nothing of
//! the lines before (a block comment is coloured on the line where it opens, and on the
//! doc-comment lines starting with *): the view stays random-access on a file of any size.
//! Only the lines on screen are tokenized, once each: the grid keeps their galleys.

use crate::theme::Palette;
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId};
use std::collections::HashSet;
use std::ops::Range;
use std::sync::OnceLock;

/// The grammars (several languages share one, as on the web).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grammar { Js, C, Py, Rb, Lua, Sh, Bat, Sql, Css, Html, Json, Yaml, Ini, Md, Diff, Log }

/// What a token is: the web's `sy-*` classes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// k keyword, c control flow, s string, n number, m comment, f function, t type,
    /// v variable, p property, r regex, g tag, a attribute, e escape / entity, o constant.
    K, C, S, N, M, F, T, V, P, R, G, A, E, O,
    /// Log levels: E W I D.
    Err, Wrn, Inf, Dbg,
    /// Markdown: h heading, b bold, i italic. Diff: ins, del.
    H, B, I, Ins, Del,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Lang {
    pub label: &'static str,
    pub g: Grammar,
}

/// The language of a file name (the web's SYN_LANGS, in its order). Anything unknown is read
/// as a log — dates, levels, quoted strings and numbers, which suits plain text too.
pub fn lang_of(name: &str) -> Lang {
    use Grammar::*;
    let n = name.rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
    let ext = n.rfind('.').map_or("", |i| &n[i + 1..]);
    let is = |l: &[&str]| l.contains(&ext);
    let named = |l: &[&str]| l.contains(&n.as_str());
    // `.log.1`, `.out.2`: a rotated log.
    let rotated = || n.rsplit_once('.').is_some_and(|(a, d)| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) && [".log", ".out", ".err"].iter().any(|e| a.ends_with(e)));
    let found: Option<(&'static str, Grammar)> = if is(&["js", "mjs", "cjs", "jsx"]) { Some(("JavaScript", Js)) }
        else if is(&["ts", "mts", "cts", "tsx"]) { Some(("TypeScript", Js)) }
        else if is(&["json", "jsonc", "json5", "jsonl", "ndjson", "geojson", "webmanifest", "ipynb"]) { Some(("JSON", Json)) }
        else if is(&["py", "pyw", "pyi"]) { Some(("Python", Py)) }
        else if is(&["sh", "bash", "zsh", "fish", "ksh"]) || named(&["bashrc", "zshrc", "profile", "bash_profile", "zprofile", "envrc", ".bashrc", ".zshrc", ".profile", ".bash_profile", ".zprofile", ".envrc"]) { Some(("Shell", Sh)) }
        else if is(&["ps1", "psm1", "psd1"]) { Some(("PowerShell", Sh)) }
        else if is(&["bat", "cmd"]) { Some(("Batch", Bat)) }
        else if ["dockerfile", "containerfile"].iter().any(|d| n == *d || n.ends_with(&format!(".{d}"))) { Some(("Dockerfile", Sh)) }
        else if named(&["makefile", "gnumakefile"]) || ext == "mk" { Some(("Makefile", Sh)) }
        else if is(&["sql", "ddl", "dml"]) { Some(("SQL", Sql)) }
        else if is(&["css", "scss", "sass", "less"]) { Some(("CSS", Css)) }
        else if is(&["htm", "html", "xhtml", "vue", "svelte", "astro"]) { Some(("HTML", Html)) }
        else if is(&["xml", "svg", "xsd", "xsl", "xslt", "plist", "csproj", "vbproj", "props", "targets", "config", "rss", "atom", "kml", "gpx", "xaml", "resx", "wsdl"]) { Some(("XML", Html)) }
        else if is(&["yml", "yaml"]) { Some(("YAML", Yaml)) }
        else if ext == "toml" { Some(("TOML", Ini)) }
        else if is(&["ini", "cfg", "conf", "cnf", "properties", "env", "editorconfig", "desktop", "service", "timer", "socket", "mount", "gitconfig", "npmrc"])
            || named(&[".env", ".gitconfig", ".npmrc", ".editorconfig"]) || (n.starts_with(".env.") && n.len() > 5) { Some(("INI", Ini)) }
        else if is(&["md", "markdown", "mdx", "rst"]) { Some(("Markdown", Md)) }
        else if is(&["c", "h"]) { Some(("C", C)) }
        else if is(&["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "ino"]) { Some(("C++", C)) }
        else if ext == "cs" { Some(("C#", C)) }
        else if ext == "java" { Some(("Java", C)) }
        else if is(&["kt", "kts"]) { Some(("Kotlin", C)) }
        else if ext == "go" { Some(("Go", C)) }
        else if ext == "rs" { Some(("Rust", C)) }
        else if ext == "swift" { Some(("Swift", C)) }
        else if ext.strip_prefix("php").is_some_and(|d| d.len() <= 1 && d.bytes().all(|b| b.is_ascii_digit())) { Some(("PHP", C)) }
        else if is(&["scala", "sc"]) { Some(("Scala", C)) }
        else if ext == "dart" { Some(("Dart", C)) }
        else if is(&["groovy", "gradle"]) { Some(("Groovy", C)) }
        else if is(&["rb", "rake", "gemspec"]) || named(&["gemfile", "rakefile"]) { Some(("Ruby", Rb)) }
        else if ext == "lua" { Some(("Lua", Lua)) }
        else if is(&["pl", "pm"]) { Some(("Perl", Sh)) }
        else if ext == "r" { Some(("R", Py)) }
        else if is(&["diff", "patch", "rej"]) { Some(("Diff", Diff)) }
        else if is(&["log", "out", "err"]) || rotated() || n.contains(".log.") { Some(("Log", Log)) }
        else { None };
    let (label, g) = found.unwrap_or(("Text", Log));
    Lang { label, g }
}

/// A line is coloured over its first bytes only (a minified line of a megabyte).
pub const SYN_MAX: usize = 4000;

const STR2: &str = r#""(?:[^"\\]|\\.)*(?:"|$)"#;
const STR1: &str = r#"'(?:[^'\\]|\\.)*'"#;
const STR1O: &str = r#"'(?:[^'\\]|\\.)*(?:'|$)"#;
const NUM: &str = r#"\b(?:0[xX][\da-fA-F_]+|0[bB][01_]+|\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?)[a-zA-Z]{0,3}\b|\B\.\d+\b"#;
const ID: &str = r#"[A-Za-z_$][\w$]*"#;

/// A grammar as written: rules tried left to right at each position (one regex of
/// alternatives, a group each), then the identifiers looked up in the keyword sets.
struct Def {
    rules: Vec<(String, Class)>,
    kw: &'static str,
    ctl: &'static str,
    con: &'static str,
    /// Capitalized names (with a lower-case letter) are types.
    types: bool,
    /// Case-insensitive (the keywords are lower case).
    ci: bool,
    /// No identifier rule (markup, CSS).
    no_id: bool,
}

fn def(g: Grammar) -> Def {
    use Class::*;
    let r = |v: &[(&str, Class)]| v.iter().map(|&(s, c)| (s.to_owned(), c)).collect::<Vec<_>>();
    let d = |rules, kw, ctl, con, types| Def { rules, kw, ctl, con, types, ci: false, no_id: false };
    match g {
        Grammar::Js => d(r(&[(r"//.*", M), (r"/\*.*?(?:\*/|$)", M), (r"^\s*\*(?:\s.*|/?$)", M), (STR2, S), (STR1O, S), (r"`(?:[^`\\]|\\.)*(?:`|$)", S),
                (r"(?<=[(,=:\[!&|?{};]\s*|^\s*|return\s+)/(?![*/\s])(?:[^/\\\[]|\\.|\[(?:[^\]\\]|\\.)*\])+/[dgimsuyv]*", R), (&format!("@{ID}"), F), (NUM, N)]),
            "var let const function class extends new delete typeof instanceof in of void this super static get set async await yield import export from as default interface type enum implements namespace declare abstract public private protected readonly keyof infer is satisfies accessor",
            "if else for while do switch case break continue return throw try catch finally with debugger",
            "true false null undefined NaN Infinity", true),
        Grammar::C => d(r(&[(r"//.*", M), (r"/\*.*?(?:\*/|$)", M), (r"^\s*\*(?:\s.*|/?$)", M), (r"^\s*#\s*\w+", K), (&format!(r"[@#]\[?{ID}"), F),
                (STR2, S), (STR1, S), (r"`[^`]*(?:`|$)", S), (&format!(r"\${ID}"), V), (NUM, N)]),
            "auto char const double enum extern float int long register short signed sizeof static struct typedef union unsigned void volatile class namespace template typename using virtual public private protected friend operator new delete this inline explicit mutable constexpr override final abstract interface extends implements package import var val fun func fn let mut pub impl trait mod use crate self Self struct enum type where dyn ref move unsafe async await go defer chan map range select byte rune string bool object dynamic readonly sealed internal partial event delegate lateinit companion data object suspend function echo require include namespace global static local yield",
            "if else for while do switch case default break continue return goto try catch finally throw throws match loop when in is as guard fallthrough elif foreach",
            "true false null nullptr NULL nil None undefined", true),
        Grammar::Py => d(r(&[(r"#.*", M), (r#"[rRbBuUfF]{0,2}(?:"""|''').*?(?:"""|'''|$)"#, S), (&format!("[rRbBuUfF]{{0,2}}{STR2}"), S), (&format!("[rRbBuUfF]{{0,2}}{STR1O}"), S), (r"^\s*@[\w.]+", F), (NUM, N)]),
            "def class lambda import from as global nonlocal del pass assert async await yield and or not in is function library",
            "if elif else for while break continue return try except finally raise with match case repeat",
            "True False None self cls TRUE FALSE NULL NA", true),
        Grammar::Rb => d(r(&[(r"#.*", M), (STR2, S), (STR1O, S), (&format!(r"(?<![\w:]):{ID}[?!]?"), O), (&format!(r"@@?{ID}|\${ID}"), V), (NUM, N)]),
            "def class module end do begin alias and or not defined? self super yield require require_relative include extend attr_accessor attr_reader attr_writer private protected public lambda proc then",
            "if elsif else unless while until for in case when break next redo retry return rescue ensure raise loop",
            "true false nil", true),
        Grammar::Lua => d(r(&[(r"--.*", M), (STR2, S), (STR1O, S), (r"\[\[.*?(?:\]\]|$)", S), (NUM, N)]),
            "local function end and or not in do then self require", "if elseif else for while repeat until break return goto", "true false nil", false),
        Grammar::Sh => d(r(&[(r"(?:^|(?<=\s))#.*", M), (STR2, S), (r"'[^']*(?:'|$)", S), (r"\$\{[^}]*\}|\$\(|\$[\w@#?$!*-]+", V), (r"(?:^|(?<=\s))--?[\w-]+", A), (NUM, N)]),
            "function local export declare readonly unset alias source set shift trap eval exec in echo printf read cd test FROM RUN CMD ENTRYPOINT COPY ADD ENV ARG WORKDIR EXPOSE USER VOLUME LABEL AS param begin process end my our sub use",
            "if then else elif fi for while until do done case esac break continue return exit select switch foreach",
            "true false", false),
        Grammar::Bat => Def { ci: true, ..d(r(&[(r"^\s*(?:@?rem\b|::).*", M), (STR2, S), (r"%[\w~]+%?|!\w+!", V), (r"^\s*:\w+", F), (NUM, N)]),
            "echo set setlocal endlocal call start cd copy del move mkdir rmdir exist defined not shift pushd popd", "if else for in do goto exit errorlevel", "on off", false) },
        Grammar::Sql => Def { ci: true, ..d(r(&[(r"--.*", M), (r"/\*.*?(?:\*/|$)", M), (r"'(?:[^']|'')*(?:'|$)", S), (r#""[^"]*"|`[^`]*`|\[[^\]]*\]"#, V), (&format!("[:@$]{ID}"), V), (NUM, N)]),
            "select from where and or not in is null like ilike between exists insert into values update set delete create alter drop table view index unique primary key foreign references constraint default join inner left right full outer cross on using group by order having limit offset union all distinct as asc desc with recursive returning database schema grant revoke begin commit rollback transaction trigger procedure function returns language declare if replace temporary temp cascade column add rename to over partition window fetch first next rows only top int integer bigint smallint varchar char text date timestamp boolean numeric decimal float real serial count sum avg min max coalesce cast",
            "case when then else end return", "true false null", false) },
        Grammar::Css => Def { no_id: true, ..d(r(&[(r"/\*.*?(?:\*/|$)", M), (r"^\s*\*(?:\s.*|/?$)", M), (r"//.*", M), (STR2, S), (STR1O, S), (r"#[\da-fA-F]{3,8}\b", N),
                (r"@[\w-]+", C), (r"--[\w-]+|\$[\w-]+", V), (r"[\w-]+(?=\s*:(?!:)[^{;]*(?:;|$))", P), (r"[.#][A-Za-z_-][\w-]*|::?[\w-]+", T),
                (r"-?\b\d*\.?\d+(?:%|[a-z]+)?\b", N), (r"[\w-]+(?=\()", F), (r"!important", C)]), "", "", "", false) },
        Grammar::Html => Def { no_id: true, ..d(r(&[(r"<!--.*?(?:-->|$)", M), (r"<!\[CDATA\[.*?(?:\]\]>|$)", S), (r"<[!?][\w-]*", G), (r"</?[\w:.-]+", G), (r"/?>|\?>", G),
                (r"[\w:.@-]+(?==)", A), (STR2, S), (r"'[^']*(?:'|$)", S), (r"&#?\w+;", E), (r"\{\{.*?(?:\}\}|$)", V)]), "", "", "", false) },
        Grammar::Json => d(r(&[(r#""(?:[^"\\]|\\.)*"(?=\s*:)"#, P), (STR2, S), (r"-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b", N), (r"//.*", M)]), "", "", "true false null", false),
        Grammar::Yaml => d(r(&[(r"(?:^|(?<=\s))#.*", M), (r"^(?:---|\.\.\.)\s*$", K), (r#"^\s*(?:-\s+)?[^\s#:'"][^:#]*?(?=\s*:(?:\s|$))"#, P), (r#"^\s*(?:-\s+)?"[^"]*"(?=\s*:(?:\s|$))"#, P),
                (STR2, S), (r"'(?:[^']|'')*(?:'|$)", S), (r"[&*][\w-]+", T), (r"!{1,2}[\w/:.-]*", T), (r"(?<=:\s*)[|>][-+]?\d*\s*$", K), (NUM, N)]),
            "", "", "true false null yes no on off True False Null Yes No TRUE FALSE NULL ~", false),
        Grammar::Ini => d(r(&[(r"^\s*[;#].*", M), (r"^\s*\[\[?[^\]]*\]\]?", T), (r#"^\s*(?:export\s+)?[\w.\-"' ]+?(?=\s*[=:])"#, P), (STR2, S), (r"'[^']*(?:'|$)", S),
                (r"\$\{[^}]*\}|%\([\w]+\)s|%[\w]+%", V), (NUM, N), (r"(?<=\s)[;#].*", M)]),
            "", "", "true false yes no on off null none True False", false),
        Grammar::Md => Def { no_id: true, ..d(r(&[(r"^#{1,6}\s.*", H), (r"^\s*(?:`{3,}|~{3,}).*", S), (r"^\s*>.*", M), (r"^\s*(?:[-*+]|\d+[.)])(?=\s)", K), (r"^\s*(?:[-*_]\s*){3,}$", K),
                (r"`[^`]+`", S), (r"!?\[[^\]]*\]\([^)]*\)|<https?:[^>]+>|https?://\S+", F), (r"\*\*[^*]+\*\*|__[^_]+__", B), (r"(?<![\w*])\*[^*\s][^*]*\*|(?<!\w)_[^_\s][^_]*_(?!\w)", I), (r"^\s*\|.*", P)]), "", "", "", false) },
        Grammar::Diff => Def { no_id: true, ..d(r(&[(r"^(?:\+\+\+|---)\s.*", B), (r"^@@.*", K), (r"^(?:diff|index|new file|deleted file|similarity|rename|old mode|new mode)\b.*", M), (r"^\+.*", Ins), (r"^-.*", Del), (r"^\\.*", M)]), "", "", "", false) },
        Grammar::Log => d(r(&[(r"\b\d{4}-\d\d-\d\d(?:[T ]\d\d:\d\d(?::\d\d)?(?:[.,]\d+)?(?:Z|[+-]\d\d:?\d\d)?)?\b|\b\d\d[/.]\d\d[/.]\d{2,4}(?:[ T]\d\d:\d\d(?::\d\d)?)?\b|\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}(?:\s+\d\d:\d\d:\d\d)?\b|\b\d\d:\d\d:\d\d(?:[.,]\d+)?\b", T),
                (r"\b(?:FATAL|CRITICAL|CRIT|ERROR|ERR|SEVERE|EMERG|ALERT|PANIC|FAIL(?:ED|URE)?|Error|Fatal|Failed|Exception)\b|\[(?:E|F)\]", Err),
                (r"\b(?:WARNING|WARN|Warning)\b|\[W\]", Wrn), (r"\b(?:INFO|NOTICE|Info|SUCCESS|OK)\b|\[I\]", Inf), (r"\b(?:DEBUG|TRACE|VERBOSE|FINE|FINER|FINEST|Debug|Trace)\b|\[D\]", Dbg),
                (r"\b[\w.$]+(?:Exception|Error)\b", C), (r#"https?://[^\s"'<>]+|\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b"#, F), (STR2, S), (r"(?:^|(?<=\s))'[^'\n]*'", S),
                (r"\b[\w.-]+(?==)", P), (r"\b0x[\da-fA-F]+\b|\b[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}\b|(?<![\w.-])-?\d+(?:[.,]\d+)?\b", N)]),
            "", "", "true false null None nil", false),
    }
}

/// A grammar compiled. The rules without look-around are one plain regex (a group each, the
/// identifiers last): no backtracking. Each rule with look-around is a regex of its own, its
/// next match kept while the line is read: one run of a backtracking regex over the whole
/// alternation per token was 10× slower on a log.
struct Compiled {
    easy: fancy_regex::Regex,
    /// The easy regex's group k - 1 → its rule (`cls.len()`: an identifier).
    easy_rule: Vec<usize>,
    hard: Vec<(usize, fancy_regex::Regex)>,
    cls: Vec<Class>,
    kw: HashSet<&'static str>,
    ctl: HashSet<&'static str>,
    con: HashSet<&'static str>,
    types: bool,
    ci: bool,
}

const ALL: [Grammar; 16] = [Grammar::Js, Grammar::C, Grammar::Py, Grammar::Rb, Grammar::Lua, Grammar::Sh, Grammar::Bat, Grammar::Sql, Grammar::Css, Grammar::Html, Grammar::Json, Grammar::Yaml, Grammar::Ini, Grammar::Md, Grammar::Diff, Grammar::Log];

fn compiled(g: Grammar) -> &'static Compiled {
    static C: [OnceLock<Compiled>; 16] = [const { OnceLock::new() }; 16];
    C[ALL.iter().position(|&x| x == g).unwrap()].get_or_init(|| {
        let d = def(g);
        let ci = if d.ci { "(?i)" } else { "" };
        let re = |s: &str| fancy_regex::Regex::new(&format!("{ci}{s}")).unwrap_or_else(|e| panic!("grammar {g:?}: {e}"));
        let looks = |s: &str| ["(?=", "(?!", "(?<=", "(?<!"].iter().any(|l| s.contains(l));
        let (mut alts, mut easy_rule, mut hard) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (s, _)) in d.rules.iter().enumerate() {
            if looks(s) { hard.push((i, re(s))); } else { alts.push(format!("({s})")); easy_rule.push(i); }
        }
        if !d.no_id { alts.push(r"([A-Za-z_$][\w$]*[?!]?)".into()); easy_rule.push(d.rules.len()); }
        // No easy rule: a regex that never matches.
        let easy = re(&if alts.is_empty() { r"[^\s\S]".to_owned() } else { alts.join("|") });
        let set = |s: &'static str| s.split(' ').filter(|w| !w.is_empty()).collect::<HashSet<_>>();
        Compiled { easy, easy_rule, hard, cls: d.rules.iter().map(|r| r.1).collect(), kw: set(d.kw), ctl: set(d.ctl), con: set(d.con), types: d.types, ci: d.ci }
    })
}

/// The tokens of a line: byte ranges, in order, not overlapping; plain text between them.
/// Only the first `SYN_MAX` bytes are read. As the web's one regex of alternatives: at the
/// leftmost place where a rule matches, the first rule that does.
pub fn spans(line: &str, g: Grammar) -> Vec<(Range<usize>, Class)> {
    let head = &line[..line.floor_char_boundary(SYN_MAX)];
    let cg = compiled(g);
    let mut out = Vec::new();
    // Each hard rule's next match (start, end) from where it was last looked for; None: no more.
    let mut next: Vec<Option<(usize, usize)>> = vec![Some((0, 0)); cg.hard.len()];
    let mut searched = vec![false; cg.hard.len()];
    let mut pos = 0;
    while pos < head.len() {
        // (start, end, rule). An error is the backtracking limit (a pathological line): no match.
        let mut best = match cg.easy.captures_from_pos(head, pos) {
            Ok(Some(m)) => {
                let all = m.get(0).unwrap();
                let k = (1..m.len()).find(|&k| m.get(k).is_some()).unwrap_or(1);
                Some((all.start(), all.end(), cg.easy_rule[k - 1]))
            }
            _ => None,
        };
        for (j, (rule, re)) in cg.hard.iter().enumerate() {
            if !searched[j] || next[j].is_some_and(|(a, _)| a < pos) {
                searched[j] = true;
                next[j] = re.find_from_pos(head, pos).ok().flatten().map(|m| (m.start(), m.end()));
            }
            if let Some((a, b)) = next[j] {
                if best.is_none_or(|(ba, _, br)| (a, *rule) < (ba, br)) { best = Some((a, b, *rule)); }
            }
        }
        let Some((a, b, rule)) = best else { break };
        if a == b {
            pos = a + head[a..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        pos = b;
        let c = match cg.cls.get(rule) {
            Some(&c) => c,
            None => {
                // An identifier: a keyword, a constant, a call, a type, or a plain name.
                let s = &head[a..b];
                let low;
                let w = if cg.ci { low = s.to_ascii_lowercase(); low.as_str() } else { s };
                if cg.ctl.contains(w) { Class::C }
                else if cg.kw.contains(w) { Class::K }
                else if cg.con.contains(w) || cg.con.contains(s) { Class::O }
                else if head.as_bytes().get(b) == Some(&b'(') { Class::F }
                else if cg.types && s.as_bytes()[0].is_ascii_uppercase() && s.bytes().any(|c| c.is_ascii_lowercase()) { Class::T }
                else { continue }
            }
        };
        out.push((a..b, c));
    }
    out
}

/// A class's look: its colour (None: the text's), bold, italic (app.css's `.sy-*`).
pub fn style(c: Class, p: &Palette) -> (Option<Color32>, bool, bool) {
    use Class::*;
    match c {
        K => (Some(p.sy_kw), false, false),
        C => (Some(p.sy_ctl), false, false),
        S => (Some(p.sy_str), false, false),
        N => (Some(p.sy_num), false, false),
        M => (Some(p.sy_com), false, true),
        F => (Some(p.sy_fn), false, false),
        T => (Some(p.sy_type), false, false),
        V => (Some(p.sy_var), false, false),
        P => (Some(p.sy_prop), false, false),
        R => (Some(p.sy_re), false, false),
        G => (Some(p.sy_tag), false, false),
        A => (Some(p.sy_attr), false, false),
        E => (Some(p.sy_esc), false, false),
        O => (Some(p.sy_const), false, false),
        H => (Some(p.sy_kw), true, false),
        B => (None, true, false),
        I => (None, false, true),
        Err => (Some(p.sy_err), true, false),
        Wrn => (Some(p.sy_wrn), true, false),
        Inf => (Some(p.sy_inf), false, false),
        Dbg => (Some(p.sy_dbg), false, false),
        Ins => (Some(p.ok_txt), false, false),
        Del => (Some(p.danger_txt), false, false),
    }
}

/// Appends `text` to `job` coloured by grammar `g`, with the search / find marks
/// (byte ranges, `true`: a find's) over the colours; `plain` is the text's format, `bold`
/// the grid's bold font.
pub fn append(job: &mut LayoutJob, text: &str, g: Grammar, marks: &mut [(usize, usize, bool)], plain: &TextFormat, bold: &FontId, p: &Palette) {
    let sp = spans(text, g);
    marks.sort_unstable();
    // The cut points of both, then each piece in its span's look, under its mark.
    let mut cuts: Vec<usize> = Vec::with_capacity(2 * (sp.len() + marks.len()) + 2);
    cuts.push(0);
    for (r, _) in &sp { cuts.push(r.start); cuts.push(r.end); }
    let mut at = 0;
    let mut ms: Vec<(usize, usize, bool)> = Vec::with_capacity(marks.len());
    for &(a, b, fd) in marks.iter() {
        if a < at || b > text.len() || a >= b { continue; }
        ms.push((a, b, fd));
        cuts.push(a);
        cuts.push(b);
        at = b;
    }
    cuts.push(text.len());
    cuts.sort_unstable();
    cuts.dedup();
    let (mut si, mut mi) = (0, 0);
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        while si < sp.len() && sp[si].0.end <= a { si += 1; }
        while mi < ms.len() && ms[mi].1 <= a { mi += 1; }
        let mut f = plain.clone();
        if let Some((r, c)) = sp.get(si).filter(|(r, _)| r.start <= a) {
            debug_assert!(r.end >= b);
            let (col, bo, it) = style(*c, p);
            if let Some(col) = col { f.color = col; }
            if bo { f.font_id = bold.clone(); }
            f.italics = it;
        }
        if let Some(&(_, _, fd)) = ms.get(mi).filter(|m| m.0 <= a) {
            f.background = if fd { p.find_bg } else { p.match_bg };
            f.color = p.mark_txt;
        }
        job.append(&text[a..b], 0.0, f);
    }
}

/// A raw-text line as the grid shows it: tabs to the next multiple of 4 (the web's
/// `tab-size: 4`), line breaks as ↵ (an edit may hold one); cut past what a column of
/// 30 000 px shows. Whether it was cut.
pub fn shown(s: &str) -> (std::borrow::Cow<'_, str>, bool) {
    const CAP: usize = 8000;
    let cut = s.len() > CAP;
    let s = if cut { &s[..s.floor_char_boundary(CAP)] } else { s };
    if !s.bytes().any(|b| b == b'\t' || b == b'\n' || b == b'\r') { return (s.into(), cut); }
    let mut out = String::with_capacity(s.len() + 16);
    let mut col = 0;
    let mut it = s.chars().peekable();
    while let Some(ch) = it.next() {
        match ch {
            '\t' => { let n = 4 - col % 4; out.extend(std::iter::repeat_n(' ', n)); col += n; }
            '\r' if it.peek() == Some(&'\n') => {}
            '\r' | '\n' => { out.push('↵'); col += 1; }
            c => { out.push(c); col += 1; }
        }
    }
    (out.into(), cut)
}

/// A line's width in columns, tabs as 4 (the web's textColWidth).
pub fn columns(s: &str) -> usize {
    if !s.contains('\t') { return s.chars().count(); }
    s.chars().fold(0, |col, ch| if ch == '\t' { (col / 4 + 1) * 4 } else { col + 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Class::*;

    /// The tokens of a line as (text, class).
    fn toks(line: &str, g: Grammar) -> Vec<(&str, Class)> {
        spans(line, g).into_iter().map(|(r, c)| (&line[r], c)).collect()
    }

    fn has(line: &str, g: Grammar, want: &[(&str, Class)]) {
        let t = toks(line, g);
        for w in want { assert!(t.contains(w), "{line:?}: {w:?} not in {t:?}"); }
    }

    #[test]
    fn every_grammar_compiles() {
        for g in ALL { let _ = spans("x = 1 # a", g); }
    }

    #[test]
    fn languages_by_name() {
        let l = |n: &str| { let x = lang_of(n); (x.label, x.g) };
        assert_eq!(l("main.py"), ("Python", Grammar::Py));
        assert_eq!(l("/a/b/App.TSX"), ("TypeScript", Grammar::Js));
        assert_eq!(l("app.log"), ("Log", Grammar::Log));
        assert_eq!(l("syslog.log.1"), ("Log", Grammar::Log));
        assert_eq!(l("app.out.12"), ("Log", Grammar::Log));
        assert_eq!(l("x.log.gz"), ("Log", Grammar::Log));
        assert_eq!(l("README"), ("Text", Grammar::Log));
        assert_eq!(l("notes.txt"), ("Text", Grammar::Log));
        assert_eq!(l("Makefile"), ("Makefile", Grammar::Sh));
        assert_eq!(l("rules.mk"), ("Makefile", Grammar::Sh));
        assert_eq!(l("Dockerfile"), ("Dockerfile", Grammar::Sh));
        assert_eq!(l("web.dockerfile"), ("Dockerfile", Grammar::Sh));
        assert_eq!(l(".bashrc"), ("Shell", Grammar::Sh));
        assert_eq!(l("run.sh"), ("Shell", Grammar::Sh));
        assert_eq!(l(".env"), ("INI", Grammar::Ini));
        assert_eq!(l(".env.local"), ("INI", Grammar::Ini));
        assert_eq!(l("Cargo.toml"), ("TOML", Grammar::Ini));
        assert_eq!(l("ci.yml"), ("YAML", Grammar::Yaml));
        assert_eq!(l("lib.rs"), ("Rust", Grammar::C));
        assert_eq!(l("a.c++"), ("C++", Grammar::C));
        assert_eq!(l("x.cs"), ("C#", Grammar::C));
        assert_eq!(l("i.php7"), ("PHP", Grammar::C));
        assert_eq!(l("Gemfile"), ("Ruby", Grammar::Rb));
        assert_eq!(l("fix.patch"), ("Diff", Grammar::Diff));
        assert_eq!(l("page.svg"), ("XML", Grammar::Html));
        assert_eq!(l("a.R"), ("R", Grammar::Py));
        assert_eq!(l("q.SQL"), ("SQL", Grammar::Sql));
        assert_eq!(l("doc.md"), ("Markdown", Grammar::Md));
        assert_eq!(l("run.BAT"), ("Batch", Grammar::Bat));
        assert_eq!(l("data.ndjson"), ("JSON", Grammar::Json));
    }

    #[test]
    fn python() {
        has("def f(x):  # note", Grammar::Py, &[("def", K), ("f", F), ("# note", M)]);
        has("    return None if x else 'a\\'b'", Grammar::Py, &[("return", C), ("None", O), ("if", C), ("'a\\'b'", S)]);
        has("@dataclass", Grammar::Py, &[("@dataclass", F)]);
        has("x = f\"{y}\" + 0x1F + 1.5e3 + MyType", Grammar::Py, &[("f\"{y}\"", S), ("0x1F", N), ("1.5e3", N), ("MyType", T)]);
        // A plain name is no token.
        assert!(toks("value", Grammar::Py).is_empty());
    }

    #[test]
    fn javascript_and_c() {
        has("const re = /a[/]b/gi; // c", Grammar::Js, &[("const", K), ("/a[/]b/gi", R), ("// c", M)]);
        // A division is no regex.
        assert!(!toks("a = b / c / d", Grammar::Js).iter().any(|t| t.1 == R));
        has("return `t${x}` === undefined", Grammar::Js, &[("return", C), ("`t${x}`", S), ("undefined", O)]);
        has(" * @param x", Grammar::Js, &[(" * @param x", M)]);
        has("#include <stdio.h>", Grammar::C, &[("#include", K)]);
        has("fn main() { let mut v: Vec<u8> = Vec::new(); }", Grammar::C, &[("fn", K), ("main", F), ("let", K), ("mut", K), ("Vec", T), ("new", K)]);
        has("/* open", Grammar::C, &[("/* open", M)]);
    }

    #[test]
    fn shell_yaml_ini_sql() {
        has("echo \"$HOME\" # done --x", Grammar::Sh, &[("echo", K), ("\"$HOME\"", S), ("# done --x", M)]);
        has("ls -la ${DIR} a#b", Grammar::Sh, &[("-la", A), ("${DIR}", V)]);
        assert!(!toks("a#b", Grammar::Sh).iter().any(|t| t.1 == M));
        has("key: value # c", Grammar::Yaml, &[("key", P), ("# c", M)]);
        has("- name: \"x\"", Grammar::Yaml, &[("- name", P), ("\"x\"", S)]);
        has("run: |", Grammar::Yaml, &[("run", P), ("|", K)]);
        has("on: true", Grammar::Yaml, &[("true", O)]);
        assert_eq!(toks("---", Grammar::Yaml), vec![("---", K)]);
        has("[section]", Grammar::Ini, &[("[section]", T)]);
        has("name = \"csvfab\" ; c", Grammar::Ini, &[("name", P), ("\"csvfab\"", S), ("; c", M)]);
        has("SELECT a FROM t WHERE b = 'x''y' -- c", Grammar::Sql, &[("SELECT", K), ("FROM", K), ("WHERE", K), ("'x''y'", S), ("-- c", M)]);
    }

    #[test]
    fn markup_and_diff() {
        has("<a href=\"x\">&amp;</a>", Grammar::Html, &[("<a", G), ("href", A), ("\"x\"", S), (">", G), ("&amp;", E), ("</a", G)]);
        has("# Title", Grammar::Md, &[("# Title", H)]);
        has("a **b** and *c* `d`", Grammar::Md, &[("**b**", B), ("*c*", I), ("`d`", S)]);
        has(".cls { color: #fff; }", Grammar::Css, &[(".cls", T), ("color", P), ("#fff", N)]);
        has("{\"k\": [1, true, \"s\"]}", Grammar::Json, &[("\"k\"", P), ("1", N), ("true", O), ("\"s\"", S)]);
        assert_eq!(toks("+added", Grammar::Diff), vec![("+added", Ins)]);
        assert_eq!(toks("-gone", Grammar::Diff), vec![("-gone", Del)]);
        assert_eq!(toks("@@ -1,2 +1,3 @@", Grammar::Diff), vec![("@@ -1,2 +1,3 @@", K)]);
    }

    #[test]
    fn logs() {
        let l = "2024-05-01 12:00:03.120 ERROR [main] java.io.IOException at http://h/x id=42 took -3.5 ms";
        has(l, Grammar::Log, &[("2024-05-01 12:00:03.120", T), ("ERROR", Err), ("java.io.IOException", C), ("http://h/x", F), ("id", P), ("42", N), ("-3.5", N)]);
        has("Oct 29 08:00:01 host WARN disk 'sda' 10.0.0.1:22 [I] ok", Grammar::Log, &[("Oct 29 08:00:01", T), ("WARN", Wrn), ("'sda'", S), ("10.0.0.1:22", F), ("[I]", Inf)]);
        has("DEBUG null 0xff", Grammar::Log, &[("DEBUG", Dbg), ("null", O), ("0xff", N)]);
        // A number inside a word is no number.
        assert!(toks("v1.2 ab-3", Grammar::Log).is_empty());
    }

    #[test]
    fn long_lines_and_marks() {
        let long = format!("x = 1 {}", "a ".repeat(5000));
        let s = spans(&long, Grammar::Py);
        assert!(s.iter().all(|(r, _)| r.end <= SYN_MAX));
        // Marks over the colours (egui joins the pieces of one look); the job holds the text whole.
        let p = &crate::theme::themes()[0].p;
        let mut job = LayoutJob::default();
        let plain = TextFormat::simple(FontId::monospace(13.0), p.txt);
        append(&mut job, "def foo(): pass", Grammar::Py, &mut [(2, 6, false)], &plain, &FontId::monospace(13.0), p);
        assert_eq!(job.text, "def foo(): pass");
        let sec: Vec<_> = job.sections.iter().map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.color, s.format.background)).collect();
        let (n, mk) = (Color32::TRANSPARENT, p.match_bg);
        assert_eq!(sec, vec![("de", p.sy_kw, n), ("f fo", p.mark_txt, mk), ("o", p.sy_fn, n), ("(): ", p.txt, n), ("pass", p.sy_kw, n)]);
    }

    #[test]
    fn tabs_shown_as_four() {
        assert_eq!(shown("a\tb").0, "a   b");
        assert_eq!(shown("\tx").0, "    x");
        assert_eq!(columns("ab\tc"), 5);
        assert_eq!(columns("abcd\t"), 8);
    }
}
