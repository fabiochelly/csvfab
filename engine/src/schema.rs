//! Schemas: a file's rules, kept beside it as `<stem>.schema.json` in Table
//! Schema (https://specs.frictionlessdata.io/table-schema/), so other tools
//! can check the same file with it: fields [{name, type, format, constraints:
//! {required, unique, enum, pattern, minimum, maximum, minLength,
//! maxLength}}], primaryKey, missingValues.
//! - Create (`infer`): the rules a file already follows, read from every row
//!   and kept prudent (no observed min/max, which the next export would
//!   break), with hints for the rules a user adds by hand (`Draft`).
//! - Validate (`validate`): a file against such a schema — its own, the one
//!   of the export it is a later copy of (customers_2026-10-04.csv finds
//!   customers.schema.json: `find_beside`), or any. Each kind of error is
//!   counted with examples and its rows.
//!
//! Patterns are JavaScript regular expressions (what the JS version wrote and
//! other tools read): `js_regex` translates them. The regex crate runs in
//! linear time, so no pattern can freeze a pass — the JS version's regex
//! guard has nothing to guard here; a pattern needing look-arounds runs on
//! fancy-regex, whose backtracking is bounded, and is refused past it.

use crate::kinds::{self, utf16_len, FxMap};
use crate::table::{RowId, State};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const ENUM_MAX: usize = 15; // distinct values for a list to be proposed…
const ENUM_REPEAT: usize = 10; // …each seen this many times on average: a closed list, not a sample of free text
const MARK_MAX: usize = 200_000; // rows kept per kind of error (beyond, only counted)
const VALUES_MAX: usize = 50; // distinct values kept as hints for a list added by hand
const SHAPES_MAX: usize = 12; // value shapes kept per column, for a pattern added by hand
const SHAPE_ROWS: usize = 100_000; // rows whose shapes are read, spread over the file
const SLICE: usize = 1 << 15; // rows per slice of a parallel pass
const PRE_UNIQUE: usize = 1 << 16; // rows looked at for a repeat before a uniqueness pass over every row
pub const TYPES: [&str; 9] = ["string", "integer", "number", "boolean", "date", "datetime", "time", "year", "any"];
const TOO_SLOW: &str = "a pattern of the schema takes too long on this file's values and would freeze the window — avoid nested repeats such as (a+)+ or (.*)*";

// ---- The file ------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub fields: Vec<Field>,
    #[serde(rename = "missingValues", default, skip_serializing_if = "Option::is_none")]
    pub missing_values: Option<Value>,
    /// A name, or a list of names.
    #[serde(rename = "primaryKey", default, skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<Value>,
    /// Keys of Table Schema (or other tools) csvfab does not read, kept as they are.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(rename = "decimalChar", default, skip_serializing_if = "Option::is_none")]
    pub decimal_char: Option<String>,
    #[serde(rename = "groupChar", default, skip_serializing_if = "Option::is_none")]
    pub group_char: Option<String>,
    #[serde(rename = "trueValues", default, skip_serializing_if = "Option::is_none")]
    pub true_values: Option<Vec<String>>,
    #[serde(rename = "falseValues", default, skip_serializing_if = "Option::is_none")]
    pub false_values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Constraints>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Constraints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique: Option<bool>,
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub en: Option<Vec<Value>>,
    #[serde(rename = "minLength", default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    #[serde(rename = "maxLength", default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Constraints {
    fn is_empty(&self) -> bool {
        *self == Constraints::default()
    }
}

impl Field {
    pub fn ty(&self) -> &str {
        self.ty.as_deref().unwrap_or("")
    }
    fn k(&self) -> Constraints {
        self.constraints.clone().unwrap_or_default()
    }
}

/// A schema's text: JSON, a list of fields each with a name.
pub fn parse(text: &str) -> Result<Schema, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON ({e})"))?;
    let Some(fields) = v.get("fields").and_then(|f| f.as_array()) else { return Err("no \"fields\" list: not a Table Schema".into()) };
    if fields.iter().any(|f| !f.get("name").is_some_and(|n| n.is_string())) {
        return Err("a field has no \"name\"".into());
    }
    serde_json::from_value(v).map_err(|e| format!("not a Table Schema ({e})"))
}

/// The text written: indented by two, a line break at the end.
pub fn to_json(s: &Schema) -> String {
    serde_json::to_string_pretty(s).unwrap() + "\n"
}

/// A number as JS writes it in JSON: 360, not 360.0.
fn num_value(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9e15 { Value::from(x as i64) } else { Value::from(x) }
}

/// JS `String(v)` of a JSON value.
fn value_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), kinds::js_num_str),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        Value::Array(a) => a.iter().map(value_str).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

// ---- Where a schema lives -------------------------------------------------

/// The file name without its extension.
pub fn stem(name: &str) -> &str {
    match name.rfind('.') { Some(k) => &name[..k], None => name }
}

/// customers_2026-10-04 → customers_2026-10-04, then customers: a dated or
/// numbered export finds its family's schema.
pub fn stems(name: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"(?i)[ _.\-]*(?:[0-9]{4}[\-_.]?[0-9]{2}[\-_.]?[0-9]{2}(?:[T _\-]?[0-9]{2}[\-_.:h]?[0-9]{2}(?:[\-_.:]?[0-9]{2})?)?|[0-9]{6,14}|v[0-9]{1,4}|\([0-9]{1,3}\)|[0-9]{1,3})$").unwrap());
    let mut out = vec![stem(name).to_string()];
    loop {
        let s = out.last().unwrap();
        // The JS regex is leftmost: the first place from which a suffix runs to the end.
        let n = match (0..=s.len()).filter(|&i| s.is_char_boundary(i)).find_map(|i| re.find_at(s, i).filter(|m| m.start() == i)) {
            Some(m) => s[..m.start()].to_string(),
            None => break,
        };
        if n == *s || n.is_empty() { break; }
        out.push(n);
    }
    out
}

/// Where the schema of a file is written: beside it.
pub fn path_beside(csv: &Path) -> PathBuf {
    let name = csv.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    csv.with_file_name(format!("{}.schema.json", stem(&name)))
}

/// The schema beside a file, its own or its family's: (path, text).
pub fn find_beside(csv: &Path) -> Option<(PathBuf, String)> {
    let name = csv.file_name()?.to_string_lossy().into_owned();
    stems(&name).into_iter().find_map(|s| {
        let p = csv.with_file_name(format!("{s}.schema.json"));
        std::fs::read_to_string(&p).ok().map(|t| (p, t))
    })
}

// ---- Patterns ---------------------------------------------------------------

/// A JavaScript regular expression (u flag) as the regex crates read it:
/// `\d` and `\w` are ASCII there, and a class's `[`, `&&`, `--`, `~~` are literal.
pub fn js_regex(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 8);
    let mut it = src.chars().peekable();
    let (mut class, mut first) = (false, false);
    let mut prev = '\0';
    while let Some(c) = it.next() {
        let at_start = first;
        first = false;
        match c {
            '\\' => {
                let Some(n) = it.next() else { out.push('\\'); break };
                match (n, class) {
                    ('d', false) => out.push_str("[0-9]"),
                    ('d', true) => out.push_str("0-9"),
                    ('D', false) => out.push_str("[^0-9]"),
                    ('w', false) => out.push_str("[0-9A-Za-z_]"),
                    ('w', true) => out.push_str("0-9A-Za-z_"),
                    ('W', false) => out.push_str("[^0-9A-Za-z_]"),
                    ('/', _) => out.push('/'),
                    _ => { out.push('\\'); out.push(n); }
                }
                prev = '\0';
                continue;
            }
            '[' if !class => {
                class = true;
                first = true;
                out.push('[');
                if it.peek() == Some(&'^') { out.push(it.next().unwrap()); }
            }
            ']' if class && !at_start => { class = false; out.push(']'); }
            '[' | '&' | '~' if class => { out.push('\\'); out.push(c); }
            '-' if class && prev == '-' => out.push_str("\\-"),
            _ => out.push(c),
        }
        prev = c;
    }
    out
}

/// A compiled pattern: linear time, or bounded backtracking for look-arounds.
pub enum Re {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl Re {
    /// A JS source, as JS compiles it.
    pub fn new(js: &str) -> Result<Re, String> {
        let src = js_regex(js);
        match regex::Regex::new(&src) {
            Ok(r) => Ok(Re::Plain(r)),
            Err(e) => fancy_regex::RegexBuilder::new(&src).backtrack_limit(1_000_000).build().map(Re::Fancy).map_err(|f| if src.contains("(?") { f.to_string() } else { e.to_string() }),
        }
    }
    /// Whether `s` holds a match; Err when the backtracking went past its bound.
    pub fn test(&self, s: &str) -> Result<bool, ()> {
        match self {
            Re::Plain(r) => Ok(r.is_match(s)),
            Re::Fancy(r) => r.is_match(s).map_err(|_| ()),
        }
    }
}

/// A whole value against a pattern, as Table Schema means it: `^(?:p)$`.
pub fn whole(p: &str) -> Result<Re, String> {
    Re::new(&format!("^(?:{p})$"))
}

/// Checks of a text column that add up — no invisible characters AND no
/// double spaces AND a shape. Table Schema gives a field one pattern and
/// nothing else, so each check is written into that pattern as a lookahead,
/// `(?![\s\S]*(?:bad))` — "nowhere a bad sequence" —, ahead of the shape;
/// `split_pattern` recognises them when the schema is read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Check {
    NoInvisible,
    NoDoubleSpace,
    NoMojibake,
    UpperCase,
    Capitalised,
}

/// Bytes 80–BF as Windows-1252 shows them.
const C1: &str = r"\u0080-¿ŒœŠšŸŽžƒˆ˜–—‘-‚“-„†-•…‰‹›€™";

impl Check {
    pub const ALL: [Check; 5] = [Check::NoInvisible, Check::NoDoubleSpace, Check::NoMojibake, Check::UpperCase, Check::Capitalised];

    /// The key the JS version knew it by.
    pub fn key(self) -> &'static str {
        ["noInvisible", "noDoubleSpace", "noMojibake", "upperCase", "capitalised"][self as usize]
    }
    /// (rule shown, menu entry, its detail, what the report says).
    pub fn texts(self) -> [&'static str; 4] {
        match self {
            Check::NoInvisible => ["no invisible characters", "No invisible characters", "control characters, zero-width spaces, BOM, soft hyphens", "holds an invisible character"],
            Check::NoDoubleSpace => ["no double spaces", "No double spaces", "two spaces in a row", "holds two spaces in a row"],
            Check::NoMojibake => ["no garbled accents", "No garbled accents", "Ã© for é, â€™ for ’: UTF-8 read as Windows-1252", "holds garbled accents (mojibake)"],
            Check::UpperCase => ["upper case", "Upper case", "no lower-case letter: DUPONT, LE HAVRE", "holds a lower-case letter"],
            Check::Capitalised => ["capitalised", "Capitalised", "a capital first, then lower case: Dupont, Le Havre, Boulogne-sur-Mer", "not capitalised (a capital first, then lower case)"],
        }
    }
    /// What must be nowhere in the value: the same to JavaScript and Python's re.
    pub fn bad(self) -> String {
        match self {
            Check::NoInvisible => r"[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F\u00AD\u200B-\u200F\u2028-\u202E\u2060-\u2064\u2066-\u206F\uFEFF]".into(),
            Check::NoDoubleSpace => " {2}".into(),
            Check::NoMojibake => format!("[ÂÃÅ][{C1}]|â€"),
            Check::UpperCase => "[a-zß-öø-ÿœ]".into(),
            Check::Capitalised => "^[^A-ZÀ-ÖØ-ÞŒ]|[A-Za-zÀ-ÖØ-öø-ÿŒœ][A-ZÀ-ÖØ-ÞŒ]".into(),
        }
    }
    fn part(self) -> String {
        format!(r"(?![\s\S]*(?:{}))", self.bad())
    }
    fn re(self) -> Re {
        Re::new(&self.bad()).expect("a check's pattern")
    }
}

/// The pattern written for a shape (or none) and checks.
pub fn join_pattern(base: &str, checks: &[Check]) -> String {
    if checks.is_empty() { return base.to_string(); }
    let parts: String = checks.iter().map(|c| c.part()).collect();
    if base.is_empty() { format!(r"{parts}[\s\S]*") } else { format!("{parts}(?:{base})") }
}

/// The reverse: a pattern → its shape and its checks. A pattern csvfab did not write this way is a shape alone.
pub fn split_pattern(p: &str) -> (String, Vec<Check>) {
    let mut checks = Vec::new();
    let mut rest = p;
    let mut again = true;
    while again {
        again = false;
        for c in Check::ALL {
            let part = c.part();
            if !checks.contains(&c) && rest.starts_with(&part) {
                checks.push(c);
                rest = &rest[part.len()..];
                again = true;
            }
        }
    }
    if checks.is_empty() { return (p.to_string(), checks); }
    let base = if rest == r"[\s\S]*" { Some("") } else if rest.starts_with("(?:") && rest.ends_with(')') { Some(&rest[3..rest.len() - 1]) } else { None };
    match base {
        Some(b) if join_pattern(b, &checks) == p => (b.to_string(), checks),
        _ => (p.to_string(), Vec::new()),
    }
}

// ---- Shapes of values ---------------------------------------------------------

const SC_D: char = '\u{10}';
const SC_U: char = '\u{11}';
const SC_L: char = '\u{12}';
const SC_N: char = '\u{13}';
const SC_E: char = '\u{14}';
const SC_P: char = '\u{15}';

/// A value's shape, exact and general: "AB-1234" → ("[A-Z]{2}-\d{4}",
/// "[A-Z]+-\d+") once written as a regex (`shape_regex`). Kept as tokens —
/// control characters for digit, upper, lower and the repeats — so a "+" or
/// "{" of the value itself stays a literal. Accented letters count as letters.
pub fn shape(s: &str) -> (String, String) {
    let (mut ex, mut gen) = (String::new(), String::new());
    let (mut prev, mut run) = ('\0', 0usize);
    let put = |prev: char, run: usize, ex: &mut String, gen: &mut String| {
        if prev == '\0' { return; }
        ex.push(prev);
        gen.push(prev);
        if run > 1 {
            ex.push(SC_N);
            ex.push_str(&run.to_string());
            ex.push(SC_E);
            gen.push(SC_P);
        }
    };
    for ch in s.chars() {
        let k = match ch {
            '0'..='9' => SC_D,
            'A'..='Z' => SC_U,
            'a'..='z' => SC_L,
            _ if ch.is_ascii() => ch,
            _ => {
                let (lo, up): (String, String) = (ch.to_lowercase().collect(), ch.to_uppercase().collect());
                if lo != up { if up.chars().eq(std::iter::once(ch)) { SC_U } else { SC_L } } else { ch }
            }
        };
        if k == prev { run += 1 } else { put(prev, run, &mut ex, &mut gen); prev = k; run = 1; }
    }
    put(prev, run, &mut ex, &mut gen);
    (ex, gen)
}

fn lit(c: char) -> String {
    match c {
        SC_D => r"\d".into(),
        SC_U => "[A-ZÀ-ÖØ-Þ]".into(),
        SC_L => "[a-zß-öø-ÿ]".into(),
        SC_P => "+".into(),
        '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' | '/' => format!("\\{c}"),
        _ => c.to_string(),
    }
}

/// A shape as a regex (in JS syntax, which Python's re reads too).
pub fn shape_regex(shape: &str) -> String {
    let mut out = String::new();
    let mut it = shape.chars();
    while let Some(c) = it.next() {
        if c == SC_N {
            let n: String = it.by_ref().take_while(|&x| x != SC_E).collect();
            out.push_str(&format!("{{{n}}}"));
        } else {
            out.push_str(&lit(c));
        }
    }
    out
}

/// An exact shape as runs: (token, count).
fn shape_runs(shape: &str) -> Vec<(char, usize)> {
    let mut out: Vec<(char, usize)> = Vec::new();
    let mut it = shape.chars();
    while let Some(c) = it.next() {
        if c == SC_N {
            let n: String = it.by_ref().take_while(|&x| x != SC_E).collect();
            if let Some(l) = out.last_mut() { l.1 = n.parse().unwrap_or(1); }
        } else {
            out.push((c, 1));
        }
    }
    out
}

struct Group {
    n: u32,
    runs: Vec<(char, usize)>,
    lo: Vec<usize>,
    hi: Vec<usize>,
}

/// Exact shapes that differ only by their counts, grouped: "Aa{4}", "Aa{9}"
/// → one group whose runs go from 1 to 1 and 4 to 9.
fn shape_groups(entries: &[(String, u32)]) -> Vec<Group> {
    let mut groups: Vec<(String, Group)> = Vec::new();
    for (sh, n) in entries {
        let runs = shape_runs(sh);
        let key: String = runs.iter().map(|r| r.0).collect();
        match groups.iter_mut().find(|g| g.0 == key) {
            Some((_, g)) => {
                for (k, r) in runs.iter().enumerate() { g.lo[k] = g.lo[k].min(r.1); g.hi[k] = g.hi[k].max(r.1); }
                g.n += n;
            }
            None => {
                let lo = runs.iter().map(|r| r.1).collect::<Vec<_>>();
                groups.push((key, Group { n: *n, hi: lo.clone(), lo, runs }));
            }
        }
    }
    let mut g: Vec<Group> = groups.into_iter().map(|g| g.1).collect();
    g.sort_by(|a, b| b.n.cmp(&a.n));
    g
}

fn ranges_regex(groups: &[Group], any_length: bool) -> String {
    groups.iter().map(|g| g.runs.iter().enumerate().map(|(k, r)| {
        let (lo, hi) = (g.lo[k], g.hi[k]);
        if any_length { return lit(r.0) + if hi > 1 { "+" } else { "" }; }
        lit(r.0) + &if lo == hi { if lo > 1 { format!("{{{lo}}}") } else { String::new() } } else { format!("{{{lo},{hi}}}") }
    }).collect::<String>()).collect::<Vec<_>>().join("|")
}

/// Patterns that go with any text, offered beside the ones read from the values.
pub const PATTERN_PRESETS: [(&str, &str); 7] = [
    (r"\d+", "digits only"), (r"[A-Z0-9_-]+", "upper-case code"), (r"\S(.*\S)?", "no space at either end"),
    (r"\d{5}", "French postal code"), (r"(0|\+33 ?)[1-9]([ .-]?\d{2}){4}", "French phone number"),
    (r"[A-Z]{2}", "two capital letters (country code)"), (r"[A-Z]{2}\d{2}[A-Z0-9 ]{10,30}", "IBAN"),
];

// ---- Reading values as Table Schema does ----------------------------------------

/// A cell as its field reads it.
#[derive(Clone, Copy, Debug)]
enum Val<'a> {
    Num(f64),
    Bool(bool),
    Str(&'a str),
}

#[derive(Clone, Debug, PartialEq)]
enum Owned {
    Num(f64),
    Bool(bool),
    Str(String),
}

impl Val<'_> {
    fn own(self) -> Owned {
        match self { Val::Num(x) => Owned::Num(x), Val::Bool(b) => Owned::Bool(b), Val::Str(s) => Owned::Str(s.into()) }
    }
    /// JS `===`.
    fn same(&self, o: &Owned) -> bool {
        match (self, o) {
            (Val::Num(a), Owned::Num(b)) => a == b,
            (Val::Bool(a), Owned::Bool(b)) => a == b,
            (Val::Str(a), Owned::Str(b)) => *a == b,
            _ => false,
        }
    }
    /// A key for "the same reading" (007 and 7 are one integer).
    fn hash(&self) -> u64 {
        let mut h = rustc_hash::FxHasher::default();
        match self {
            Val::Num(x) => { h.write_u8(0); h.write_u64(if *x == 0.0 { 0 } else { x.to_bits() }); }
            Val::Bool(b) => { h.write_u8(1); h.write_u8(*b as u8); }
            Val::Str(s) => { h.write_u8(2); h.write(s.as_bytes()); }
        }
        h.finish()
    }
}

/// JS `Number(s)` of a string.
fn js_number(s: &str) -> f64 {
    let t = s.trim_matches(kinds::is_js_space);
    if t.is_empty() { return 0.0; }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { return i64::from_str_radix(h, 16).map_or(f64::NAN, |v| v as f64); }
    match t { "Infinity" | "+Infinity" => f64::INFINITY, "-Infinity" => f64::NEG_INFINITY, _ if t.contains(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E') => f64::NAN, _ => t.parse().unwrap_or(f64::NAN) }
}

/// JS `a < b` between two readings.
fn js_lt(a: &Val, b: &Owned) -> bool {
    let num = |v: &Val| match v { Val::Num(x) => *x, Val::Bool(b) => *b as u8 as f64, Val::Str(s) => js_number(s) };
    match (a, b) {
        (Val::Str(x), Owned::Str(y)) => x.encode_utf16().lt(y.encode_utf16()),
        (_, Owned::Num(y)) => num(a) < *y,
        (_, Owned::Bool(y)) => num(a) < *y as u8 as f64,
        (_, Owned::Str(y)) => num(a) < js_number(y),
    }
}

fn js_gt(a: &Val, b: &Owned) -> bool {
    let num = |v: &Val| match v { Val::Num(x) => *x, Val::Bool(b) => *b as u8 as f64, Val::Str(s) => js_number(s) };
    match (a, b) {
        (Val::Str(x), Owned::Str(y)) => x.encode_utf16().gt(y.encode_utf16()),
        (_, Owned::Num(y)) => num(a) > *y,
        (_, Owned::Bool(y)) => num(a) > *y as u8 as f64,
        (_, Owned::Str(y)) => num(a) > js_number(y),
    }
}

fn all_digits(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(u8::is_ascii_digit)
}

/// `^[+-]?\d+$`
fn is_int(s: &str) -> bool {
    all_digits(s.strip_prefix(['+', '-']).unwrap_or(s).as_bytes())
}

/// `^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$`
fn is_num(s: &str) -> bool {
    let b = s.strip_prefix(['+', '-']).unwrap_or(s).as_bytes();
    let e = b.iter().position(|&c| c == b'e' || c == b'E').unwrap_or(b.len());
    let (m, x) = (&b[..e], &b[e..]);
    let ok_m = match m.iter().position(|&c| c == b'.') {
        Some(0) => all_digits(&m[1..]),
        Some(k) => all_digits(&m[..k]) && m[k + 1..].iter().all(u8::is_ascii_digit),
        None => all_digits(m),
    };
    ok_m && (x.is_empty() || all_digits(x[1..].strip_prefix(b"+").or_else(|| x[1..].strip_prefix(b"-")).unwrap_or(&x[1..])))
}

/// `^[+-]?\d{1,3}(?: \d{3})*(?:,\d+)?$|^[+-]?\d+(?:,\d+)?$`: a number with a decimal comma.
fn is_num_comma(s: &str) -> bool {
    let b = s.strip_prefix(['+', '-']).unwrap_or(s).as_bytes();
    let (int, dec) = match b.iter().position(|&c| c == b',') { Some(k) => (&b[..k], Some(&b[k + 1..])), None => (b, None) };
    if dec.is_some_and(|d| !all_digits(d)) { return false; }
    if all_digits(int) { return true; }
    let mut parts = int.split(|&c| c == b' ');
    let first = parts.next().unwrap_or(b"");
    all_digits(first) && first.len() <= 3 && parts.all(|p| p.len() == 3 && all_digits(p))
}

const MDAYS: [u32; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn valid_date(y: u32, m: u32, d: u32) -> bool {
    if !(1..=12).contains(&m) || d < 1 { return false; }
    d <= if m == 2 && ((y % 4 == 0 && y % 100 != 0) || y % 400 == 0) { 29 } else { MDAYS[m as usize] }
}

fn num_of(b: &[u8]) -> u32 {
    b.iter().fold(0, |a, &c| a * 10 + (c - b'0') as u32)
}

/// yyyy-mm-dd as a sortable number (YYYYMMDD000000).
fn iso_date(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' || !all_digits(&b[..4]) || !all_digits(&b[5..7]) || !all_digits(&b[8..]) { return None; }
    let (y, m, d) = (num_of(&b[..4]), num_of(&b[5..7]), num_of(&b[8..]));
    valid_date(y, m, d).then(|| (y as f64 * 10000.0 + m as f64 * 100.0 + d as f64) * 1e6)
}

/// `^(\d{1,2})/(\d{1,2})/(\d{4})$`
fn dmy(s: &str) -> Option<(u32, u32, u32)> {
    let mut p = s.split('/');
    let (a, b, c) = (p.next()?.as_bytes(), p.next()?.as_bytes(), p.next()?.as_bytes());
    if p.next().is_some() || !all_digits(a) || a.len() > 2 || !all_digits(b) || b.len() > 2 || !all_digits(c) || c.len() != 4 { return None; }
    Some((num_of(a), num_of(b), num_of(c)))
}

/// `^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?$`:
/// the date and time digits.
fn iso_dt(s: &str) -> Option<[u32; 6]> {
    let b = s.as_bytes();
    if b.len() < 16 || !all_digits(&b[..4]) || b[4] != b'-' || !all_digits(&b[5..7]) || b[7] != b'-' || !all_digits(&b[8..10])
        || !(b[10] == b'T' || b[10] == b' ') || !all_digits(&b[11..13]) || b[13] != b':' || !all_digits(&b[14..16]) { return None; }
    let mut i = 16;
    let mut sec = 0;
    if b.get(i) == Some(&b':') {
        if b.len() < i + 3 || !all_digits(&b[i + 1..i + 3]) { return None; }
        sec = num_of(&b[i + 1..i + 3]);
        i += 3;
        if b.get(i) == Some(&b'.') {
            let r = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if r == 0 { return None; }
            i += 1 + r;
        }
    }
    let zone_ok = match &b[i..] {
        [] | [b'Z'] => true,
        [b'+' | b'-', z @ ..] => (z.len() == 4 && all_digits(z)) || (z.len() == 5 && z[2] == b':' && all_digits(&z[..2]) && all_digits(&z[3..])),
        _ => false,
    };
    zone_ok.then(|| [num_of(&b[..4]), num_of(&b[5..7]), num_of(&b[8..10]), num_of(&b[11..13]), num_of(&b[14..16]), sec])
}

/// `^(\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?$`
fn time(s: &str) -> Option<(u32, u32, u32)> {
    let b = s.as_bytes();
    if b.len() < 5 || !all_digits(&b[..2]) || b[2] != b':' || !all_digits(&b[3..5]) { return None; }
    let mut sec = 0;
    if b.len() > 5 {
        if b.len() < 8 || b[5] != b':' || !all_digits(&b[6..8]) { return None; }
        sec = num_of(&b[6..8]);
        if b.len() > 8 && (b[8] != b'.' || !all_digits(&b[9..])) { return None; }
    }
    Some((num_of(&b[..2]), num_of(&b[3..5]), sec))
}

/// `^[^\s@]+@[^\s@]+\.[^\s@]+$`
/// `a` against `b` in UTF-16 code units (JavaScript's string order), without encoding
/// either: UTF-8 bytes sort as code points, which only differ from UTF-16 between a
/// character of U+E000–U+FFFF (lead byte EE, EF) and one of the supplementary planes
/// (lead byte F0 and up) — read as UTF-16 then.
fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let Some(i) = x.iter().zip(y).position(|(p, q)| p != q) else { return x.len().cmp(&y.len()) };
    // Same bytes before i: a continuation byte there means the same lead byte in both.
    if x[i] & 0xC0 != 0x80 && y[i] & 0xC0 != 0x80 {
        let (top, sup) = (|l: u8| l == 0xEE || l == 0xEF, |l: u8| l >= 0xF0);
        if (top(x[i]) && sup(y[i])) || (sup(x[i]) && top(y[i])) { return a.encode_utf16().cmp(b.encode_utf16()); }
    }
    x[i].cmp(&y[i])
}

fn is_email(s: &str) -> bool {
    if s.chars().any(kinds::is_js_space) { return false; }
    let Some((local, dom)) = s.split_once('@') else { return false };
    !local.is_empty() && !dom.contains('@') && dom.char_indices().any(|(i, c)| c == '.' && i > 0 && i + 1 < dom.len())
}

/// `^[a-z][a-z0-9+.-]*:\S+$` (any case)
fn is_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else { return false };
    let b = scheme.as_bytes();
    !b.is_empty() && b[0].is_ascii_alphabetic() && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
        && !rest.is_empty() && !rest.chars().any(kinds::is_js_space)
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

/// `^\+?[\d\s().\/-]{6,24}$` and 6 digits at least.
fn is_phone(s: &str) -> bool {
    let body = s.strip_prefix('+').unwrap_or(s);
    let n = utf16_len(body);
    (6..=24).contains(&n) && body.chars().all(|c| c.is_ascii_digit() || kinds::is_js_space(c) || "()./-".contains(c))
        && s.bytes().filter(u8::is_ascii_digit).count() >= 6
}

/// A strptime format (`%d/%m/%Y`) → a regex and the order of its parts.
struct Fmt {
    re: regex::Regex,
    parts: Vec<char>,
}

fn strptime(f: &str) -> Fmt {
    let mut re = String::from("^");
    let mut parts = Vec::new();
    let mut it = f.chars().peekable();
    while let Some(c) = it.next() {
        let tok = |t: char| Some(match t { 'Y' => r"(\d{4})", 'y' | 'M' | 'S' => r"(\d{2})", 'm' | 'd' | 'H' | 'I' => r"(\d{1,2})", 'p' => "([AaPp][Mm])", _ => return None });
        if c == '%' {
            if let Some(t) = it.peek().and_then(|&t| tok(t).map(|r| (t, r))) {
                it.next();
                parts.push(t.0);
                re.push_str(&js_regex(t.1));
                continue;
            }
            if it.peek() == Some(&'%') { it.next(); re.push('%'); continue; }
        }
        re.push_str(&regex::escape(&c.to_string()));
    }
    re.push('$');
    Fmt { re: regex::Regex::new(&re).unwrap_or_else(|_| regex::Regex::new("$^").unwrap()), parts }
}

fn parse_fmt(s: &str, f: &Fmt) -> Option<f64> {
    let m = f.re.captures(s)?;
    let (mut y, mut mo, mut d, mut h, mut mi, mut se, mut pm) = (1970u32, 1, 1, 0, 0, 0, false);
    for (i, p) in f.parts.iter().enumerate() {
        let t = m.get(i + 1)?.as_str();
        let n: u32 = t.parse().unwrap_or(0);
        match p {
            'y' => y = n + if n < 70 { 2000 } else { 1900 },
            'I' => h = n % 12,
            'p' => pm = t.eq_ignore_ascii_case("pm"),
            'Y' => y = n,
            'm' => mo = n,
            'd' => d = n,
            'H' => h = n,
            'M' => mi = n,
            'S' => se = n,
            _ => {}
        }
    }
    if pm { h += 12; }
    if !valid_date(y, mo, d) || h > 23 || mi > 59 || se > 59 { return None; }
    Some(((((y as f64 * 100.0 + mo as f64) * 100.0 + d as f64) * 100.0 + h as f64) * 100.0 + mi as f64) * 100.0 + se as f64)
}

/// A field, ready to read cells: its formats compiled once.
struct Reader {
    ty: String,
    format: Option<String>,
    group: Option<String>,
    decimal: Option<String>,
    trues: Vec<String>,
    falses: Vec<String>,
    fmt: Option<Fmt>,
    any_dmy: Fmt,
}

impl Reader {
    fn new(f: &Field) -> Reader {
        let ty = f.ty().to_string();
        let format = f.format.clone();
        let custom = matches!(ty.as_str(), "date" | "datetime") && format.as_deref().is_some_and(|x| x != "default" && x != "any");
        Reader {
            fmt: custom.then(|| strptime(format.as_deref().unwrap())),
            any_dmy: strptime("%d/%m/%Y"),
            group: f.group_char.clone().filter(|g| !g.is_empty()),
            decimal: f.decimal_char.clone().filter(|d| !d.is_empty()),
            trues: f.true_values.clone().unwrap_or_else(|| ["true", "True", "TRUE", "1"].map(String::from).to_vec()),
            falses: f.false_values.clone().unwrap_or_else(|| ["false", "False", "FALSE", "0"].map(String::from).to_vec()),
            ty,
            format,
        }
    }

    /// JS `scValue`: the field's reading of a value, or why it is not one.
    fn value<'a>(&self, s: &'a str) -> Result<Val<'a>, String> {
        let fm = self.format.as_deref();
        match self.ty.as_str() {
            "integer" => if is_int(s) { Ok(Val::Num(js_number(s))) } else { Err("not an integer".into()) },
            "number" => {
                let mut x = std::borrow::Cow::Borrowed(s);
                if let Some(g) = &self.group { x = x.replace(g.as_str(), "").into(); }
                if let Some(d) = self.decimal.as_deref().filter(|d| *d != ".") {
                    if x.contains('.') { return Err("not a number".into()); }
                    x = x.replacen(d, ".", 1).into();
                }
                if is_num(&x) { Ok(Val::Num(js_number(&x))) } else { Err("not a number".into()) }
            }
            "boolean" => if self.trues.iter().any(|t| t == s) { Ok(Val::Bool(true)) } else if self.falses.iter().any(|t| t == s) { Ok(Val::Bool(false)) } else { Err("not a boolean".into()) },
            "date" => match &self.fmt {
                None => iso_date(s).or_else(|| if fm == Some("any") { parse_fmt(s, &self.any_dmy) } else { None }).map(Val::Num).ok_or_else(|| "not a date (yyyy-mm-dd)".into()),
                Some(f) => parse_fmt(s, f).map(Val::Num).ok_or_else(|| format!("not a date ({})", fm.unwrap())),
            },
            "datetime" => match &self.fmt {
                None => match iso_dt(s) {
                    Some([y, m, d, h, mi, _]) if valid_date(y, m, d) && h < 24 && mi < 60 => {
                        let b = s.as_bytes();
                        let sec = if b.get(16) == Some(&b':') { &s[17..19] } else { "00" };
                        Ok(Val::Num(format!("{}{}{}{}{}{sec}", &s[..4], &s[5..7], &s[8..10], &s[11..13], &s[14..16]).parse().unwrap()))
                    }
                    _ => Err("not a date and time (yyyy-mm-ddThh:mm:ss)".into()),
                },
                Some(f) => parse_fmt(s, f).map(Val::Num).ok_or_else(|| format!("not a date and time ({})", fm.unwrap())),
            },
            "time" => match time(s) {
                Some((h, m, sec)) if h < 24 && m < 60 => Ok(Val::Num((h * 3600 + m * 60 + sec) as f64)),
                _ => Err("not a time (hh:mm:ss)".into()),
            },
            "year" => if s.len() == 4 && all_digits(s.as_bytes()) { Ok(Val::Num(num_of(s.as_bytes()) as f64)) } else { Err("not a year".into()) },
            "string" => match fm {
                Some("email") if !is_email(s) => Err("not an e-mail address".into()),
                Some("uri") if !is_uri(s) => Err("not a URI".into()),
                Some("uuid") if !is_uuid(s) => Err("not a UUID".into()),
                _ => Ok(Val::Str(s)),
            },
            // any, and the types csvfab does not check (object, array, geopoint…)
            _ => Ok(Val::Str(s)),
        }
    }

    /// A rule's value (a bound, a value of a list) as the cells read.
    fn bound(&self, v: &Value) -> Option<Owned> {
        if let Value::Number(n) = v { return n.as_f64().map(Owned::Num); }
        if v.is_null() { return None; }
        let s = value_str(v);
        self.value(&s).ok().map(Val::own)
    }
}

// ---- Inference ----------------------------------------------------------

/// What every filled value of a column has in common, over a slice of rows
/// (merged in file order). The type flags only ever go from true to false;
/// the rows where they did are kept, for uniqueness (see `unique_ok`).
#[derive(Clone)]
struct Acc {
    n: usize,
    empty: usize,
    int: bool,
    lead0: bool,
    /// Digits of every integer: -1 none yet, -2 not all the same.
    dlen: i64,
    num: bool,
    comma: bool,
    group: bool,
    neg: bool,
    iso: bool,
    dmy: bool,
    mdy: bool,
    first12: bool,
    second12: bool,
    isodt: bool,
    time: bool,
    email: bool,
    phone: bool,
    space: bool,
    /// The distinct values and their counts, while few.
    dv: Option<Vec<(String, u32)>>,
    lmin: usize,
    lmax: usize,
    nmin: (f64, String),
    nmax: (f64, String),
    dmin: (f64, String),
    dmax: (f64, String),
    dmy_min: (f64, String),
    dmy_max: (f64, String),
    mdy_min: (f64, String),
    mdy_max: (f64, String),
    smin: Option<String>,
    smax: Option<String>,
    shp: Option<Vec<(String, u32)>>,
    gshp: Option<Vec<(String, u32)>>,
    /// Rows where a flag went false (space: true); the first filled row.
    at: [Option<u32>; 10],
}

const AT_INT: usize = 0;
const AT_SPACE: usize = 8;
const AT_FIRST: usize = 9;

impl Acc {
    fn new() -> Acc {
        let lo = (f64::INFINITY, String::new());
        let hi = (f64::NEG_INFINITY, String::new());
        Acc {
            n: 0, empty: 0, int: true, lead0: false, dlen: -1, num: true, comma: true, group: false, neg: false,
            iso: true, dmy: true, mdy: true, first12: false, second12: false, isodt: true, time: true, email: true, phone: true, space: false,
            dv: Some(Vec::new()), lmin: usize::MAX, lmax: 0, nmin: lo.clone(), nmax: hi.clone(), dmin: lo.clone(), dmax: hi.clone(),
            dmy_min: lo.clone(), dmy_max: hi.clone(), mdy_min: lo, mdy_max: hi, smin: None, smax: None,
            shp: Some(Vec::new()), gshp: Some(Vec::new()), at: [None; 10],
        }
    }

    fn flags(&self) -> [bool; 8] {
        [self.int, self.num, self.comma, self.iso, self.dmy, self.mdy, self.isodt, self.time]
    }

    fn add(&mut self, s: &str, p: u32, shape_row: bool) {
        if s.is_empty() { self.empty += 1; return; }
        self.n += 1;
        self.at[AT_FIRST].get_or_insert(p);
        let before = self.flags();
        if !self.space && s.contains(' ') { self.space = true; self.at[AT_SPACE] = Some(p); }
        if let Some(dv) = &mut self.dv {
            match dv.iter().position(|e| e.0 == s) {
                Some(k) => dv[k].1 += 1,
                None if dv.len() < VALUES_MAX => dv.push((s.to_string(), 1)),
                None => self.dv = None,
            }
        }
        let len = utf16_len(s);
        self.lmin = self.lmin.min(len);
        self.lmax = self.lmax.max(len);
        if self.smin.as_deref().is_none_or(|m| utf16_cmp(s, m).is_lt()) { self.smin = Some(s.into()); }
        if self.smax.as_deref().is_none_or(|m| utf16_cmp(s, m).is_gt()) { self.smax = Some(s.into()); }
        if shape_row && (self.shp.is_some() || self.gshp.is_some()) {
            let (ex, gen) = shape(s);
            for (m, k) in [(&mut self.shp, ex), (&mut self.gshp, gen)] {
                if let Some(v) = m {
                    match v.iter_mut().find(|e| e.0 == k) { Some(e) => e.1 += 1, None => v.push((k, 1)) }
                    if v.len() > SHAPES_MAX { *m = None; }
                }
            }
        }
        let c0 = s.as_bytes()[0];
        let digitish = c0.is_ascii_digit() || matches!(c0, b'+' | b'-' | b'.');
        if self.int && !(digitish && is_int(s)) { self.int = false; }
        if self.int {
            let b = s.as_bytes();
            let body = s.strip_prefix(['+', '-']).unwrap_or(s);
            if body.len() >= 2 && body.as_bytes()[0] == b'0' { self.lead0 = true; }
            let l = body.len() as i64;
            self.dlen = if self.dlen == -1 || self.dlen == l { l } else { -2 };
            if b[0] == b'-' { self.neg = true; }
        }
        if self.num && !(digitish && is_num(s)) { self.num = false; }
        if self.comma && !(digitish && is_num_comma(s)) { self.comma = false; } else if self.comma && s.contains(' ') { self.group = true; }
        if (self.num || self.comma) && c0 == b'-' { self.neg = true; }
        if self.int || self.num || self.comma {
            let v = if self.int || self.num { js_number(s) } else { js_number(&s.replace(' ', "").replacen(',', ".", 1)) };
            if v < self.nmin.0 { self.nmin = (v, s.into()); }
            if v > self.nmax.0 { self.nmax = (v, s.into()); }
        }
        if self.iso {
            match if digitish { iso_date(s) } else { None } {
                None => self.iso = false,
                Some(v) => {
                    if v < self.dmin.0 { self.dmin = (v, s.into()); }
                    if v > self.dmax.0 { self.dmax = (v, s.into()); }
                }
            }
        }
        if self.dmy || self.mdy {
            match if digitish { dmy(s) } else { None } {
                None => { self.dmy = false; self.mdy = false; }
                Some((a, b, y)) => {
                    if a > 12 { self.first12 = true; }
                    if b > 12 { self.second12 = true; }
                    if self.dmy && !valid_date(y, b, a) { self.dmy = false; }
                    if self.mdy && !valid_date(y, a, b) { self.mdy = false; }
                    let (dv, mv) = ((y * 10000 + b * 100 + a) as f64, (y * 10000 + a * 100 + b) as f64);
                    if dv < self.dmy_min.0 { self.dmy_min = (dv, s.into()); }
                    if dv > self.dmy_max.0 { self.dmy_max = (dv, s.into()); }
                    if mv < self.mdy_min.0 { self.mdy_min = (mv, s.into()); }
                    if mv > self.mdy_max.0 { self.mdy_max = (mv, s.into()); }
                }
            }
        }
        if self.isodt && !(digitish && iso_dt(s).is_some_and(|d| valid_date(d[0], d[1], d[2]))) { self.isodt = false; }
        if self.time && !(digitish && time(s).is_some()) { self.time = false; }
        if self.email && !is_email(s) { self.email = false; }
        if self.phone && !is_phone(s) { self.phone = false; }
        for (k, (b, a)) in before.iter().zip(self.flags()).enumerate() {
            if *b && !a { self.at[if k == 0 { AT_INT } else { k }] = Some(p); }
        }
    }

    /// `o` follows `self` in the file.
    fn merge(&mut self, o: Acc) {
        self.n += o.n;
        self.empty += o.empty;
        if o.n > 0 {
            if self.int && o.int { self.dlen = if self.dlen == -1 { o.dlen } else if o.dlen == -1 || o.dlen == self.dlen { self.dlen } else { -2 }; }
            self.lead0 |= o.lead0;
            self.int &= o.int;
            self.num &= o.num;
            self.comma &= o.comma;
            self.group |= o.group;
            self.neg |= o.neg;
            self.iso &= o.iso;
            self.dmy &= o.dmy;
            self.mdy &= o.mdy;
            self.first12 |= o.first12;
            self.second12 |= o.second12;
            self.isodt &= o.isodt;
            self.time &= o.time;
            self.email &= o.email;
            self.phone &= o.phone;
            self.space |= o.space;
        }
        self.dv = match (self.dv.take(), o.dv) {
            (Some(mut a), Some(b)) => {
                for (v, k) in b {
                    match a.iter_mut().find(|e| e.0 == v) { Some(e) => e.1 += k, None => a.push((v, k)) }
                }
                (a.len() <= VALUES_MAX).then_some(a)
            }
            _ => None,
        };
        for (m, b) in [(&mut self.shp, o.shp), (&mut self.gshp, o.gshp)] {
            *m = match (m.take(), b) {
                (Some(mut a), Some(b)) => {
                    for (v, k) in b {
                        match a.iter_mut().find(|e| e.0 == v) { Some(e) => e.1 += k, None => a.push((v, k)) }
                    }
                    (a.len() <= SHAPES_MAX).then_some(a)
                }
                _ => None,
            };
        }
        self.lmin = self.lmin.min(o.lmin);
        self.lmax = self.lmax.max(o.lmax);
        if let Some(s) = o.smin { if self.smin.as_deref().is_none_or(|m| utf16_cmp(&s, m).is_lt()) { self.smin = Some(s); } }
        if let Some(s) = o.smax { if self.smax.as_deref().is_none_or(|m| utf16_cmp(&s, m).is_gt()) { self.smax = Some(s); } }
        for (a, b) in [(&mut self.nmin, o.nmin), (&mut self.dmin, o.dmin), (&mut self.dmy_min, o.dmy_min), (&mut self.mdy_min, o.mdy_min)] {
            if b.0 < a.0 { *a = b; }
        }
        for (a, b) in [(&mut self.nmax, o.nmax), (&mut self.dmax, o.dmax), (&mut self.dmy_max, o.dmy_max), (&mut self.mdy_max, o.mdy_max)] {
            if b.0 > a.0 { *a = b; }
        }
        for k in 0..self.at.len() {
            if self.at[k].is_none() { self.at[k] = o.at[k]; }
        }
    }

    /// Whether the JS pass kept following uniqueness to the end — it gave
    /// up at the first row with a space, or once the column was no integer
    /// but could still read as a number or a date (or had been one).
    fn unique_ok(&self) -> bool {
        if self.at[AT_SPACE].is_some() { return false; }
        let Some(ri) = self.at[AT_INT] else { return true };
        // Flags still true at the row the integers ended (they flip in that same row before the check).
        let other = (1..8).any(|k| self.at[k].is_none_or(|r| r > ri));
        !(other || self.at[AT_FIRST].is_some_and(|f| f < ri))
    }
}

/// What the dialog suggests for a rule added by hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hints {
    /// Share of the rows where the column is filled.
    pub filled: f64,
    pub lmin: Option<usize>,
    pub lmax: Option<usize>,
    /// The distinct values, the most common first (while few).
    pub values: Option<Vec<String>>,
    /// Smallest and largest seen, as written.
    pub min: Option<String>,
    pub max: Option<String>,
    pub min_v: Option<f64>,
    pub max_v: Option<f64>,
    /// (pattern, what it is).
    pub patterns: Vec<(String, String)>,
}

fn hints(f: &Field, x: &Acc) -> Hints {
    let filled = x.n as f64 / (x.n + x.empty).max(1) as f64;
    let mut h = Hints { filled, lmin: (x.n > 0).then_some(x.lmin), lmax: (x.n > 0).then_some(x.lmax), ..Default::default() };
    if let Some(dv) = &x.dv {
        let mut v = dv.clone();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        h.values = Some(v.into_iter().map(|e| e.0).collect());
    }
    let nz = |s: &String| (!s.is_empty()).then(|| s.clone());
    match (f.ty(), f.format.as_deref()) {
        ("integer" | "number", _) => {
            (h.min, h.max) = (nz(&x.nmin.1), nz(&x.nmax.1));
            (h.min_v, h.max_v) = (x.nmin.0.is_finite().then_some(x.nmin.0), x.nmax.0.is_finite().then_some(x.nmax.0));
        }
        ("date", None) => (h.min, h.max) = (nz(&x.dmin.1), nz(&x.dmax.1)),
        ("date", Some(fm)) => (h.min, h.max) = if fm == "%d/%m/%Y" { (nz(&x.dmy_min.1), nz(&x.dmy_max.1)) } else { (nz(&x.mdy_min.1), nz(&x.mdy_max.1)) },
        ("datetime" | "time" | "year", _) => (h.min, h.max) = (x.smin.clone(), x.smax.clone()),
        _ => {}
    }
    // Best first: the shapes with the lengths seen as ranges, then each exact shape, the most common alone, any length.
    let sorted = |m: &Option<Vec<(String, u32)>>| m.clone().map(|mut v| { v.sort_by(|a, b| b.1.cmp(&a.1)); v }).unwrap_or_default();
    let (exact, general) = (sorted(&x.shp), sorted(&x.gshp));
    let seen: u32 = if exact.is_empty() { &general } else { &exact }.iter().map(|e| e.1).sum();
    let groups = (!exact.is_empty()).then(|| shape_groups(&exact));
    let mut add = |p: String, l: String| if !p.is_empty() && !h.patterns.iter().any(|e| e.0 == p) { h.patterns.push((p, l)) };
    if exact.len() == 1 {
        add(shape_regex(&exact[0].0), "every value has this shape".into());
    } else if let Some(g) = groups.as_ref().filter(|g| g.len() < exact.len()) {
        add(ranges_regex(g, false), if g.len() == 1 { "the shape of the values, with the lengths seen".into() } else { format!("the {} shapes of the values, with the lengths seen", g.len()) });
    } else if !exact.is_empty() {
        add(exact.iter().map(|e| shape_regex(&e.0)).collect::<Vec<_>>().join("|"), format!("the {} shapes of the values", exact.len()));
    }
    if exact.len() > 1 { add(shape_regex(&exact[0].0), format!("the most common shape, {} % of the values", (exact[0].1 as f64 / seen as f64 * 100.0).round())); }
    if let Some(g) = &groups {
        add(ranges_regex(g, true), "the same, any length".into());
    } else if !general.is_empty() {
        add(general.iter().map(|e| shape_regex(&e.0)).collect::<Vec<_>>().join("|"), if general.len() == 1 { "the shape of the values, any length".into() } else { format!("the {} shapes of the values, any length", general.len()) });
    }
    h
}

/// The field a column's values make, before uniqueness is known.
fn field_of(name: &str, x: &Acc) -> Field {
    let mut f = Field { name: name.to_string(), ..Default::default() };
    let mut k = Constraints::default();
    let bool_pair = x.dv.as_ref().filter(|dv| dv.len() <= 2 && x.n > 0).and_then(|dv| {
        let lower: Vec<String> = dv.iter().map(|e| e.0.to_lowercase()).collect();
        [("true", "false"), ("yes", "no"), ("oui", "non"), ("vrai", "faux")].into_iter().find(|(a, b)| lower.iter().all(|v| v == a || v == b))
    });
    let ty = if x.n == 0 {
        "string"
    } else if let Some((t, fa)) = bool_pair {
        let dv = x.dv.as_ref().unwrap();
        f.true_values = Some(dv.iter().filter(|e| e.0.to_lowercase() == t).map(|e| e.0.clone()).collect());
        f.false_values = Some(dv.iter().filter(|e| e.0.to_lowercase() == fa).map(|e| e.0.clone()).collect());
        "boolean"
    } else if x.int && !x.lead0 {
        if !x.neg { k.minimum = Some(0.into()); }
        "integer"
    } else if x.int {
        k.pattern = Some(if x.dlen > 0 { format!(r"\d{{{}}}", x.dlen) } else { r"\d+".into() });
        "string"
    } else if x.num {
        if !x.neg { k.minimum = Some(0.into()); }
        "number"
    } else if x.comma {
        f.decimal_char = Some(",".into());
        if x.group { f.group_char = Some(" ".into()); }
        if !x.neg { k.minimum = Some(0.into()); }
        "number"
    } else if x.iso {
        "date"
    } else if x.dmy && (!x.mdy || x.first12 || !x.second12) {
        f.format = Some("%d/%m/%Y".into());
        "date"
    } else if x.mdy {
        f.format = Some("%m/%d/%Y".into());
        "date"
    } else if x.isodt {
        "datetime"
    } else if x.time {
        "time"
    } else if x.email {
        f.format = Some("email".into());
        "string"
    } else if x.phone {
        k.pattern = Some(r"\+?[0-9 ().\/-]{6,24}".into());
        "string"
    } else {
        "string"
    };
    f.ty = Some(ty.into());
    if x.n > 0 && x.empty == 0 { k.required = Some(true); }
    f.constraints = Some(k);
    f
}

/// Unique only where it means an identity: always filled, an integer or a
/// text without spaces (ids, codes, e-mails). Amounts, dates or free text
/// all distinct in one file are a coincidence.
fn may_be_unique(f: &Field, x: &Acc) -> bool {
    x.n > 1 && x.unique_ok() && x.empty == 0 && (f.ty() == "integer" || (f.ty() == "string" && !x.space))
}

/// Whether no two filled cells of `col` are the same — as numbers for a
/// column of integers (007 and 7 are one), as text otherwise. Hashes are
/// sorted, not kept in a set; equal hashes of text are read again.
fn all_different(st: &State, col: usize, ints: bool, cancel: &AtomicBool) -> Option<bool> {
    let n = st.len();
    // Most columns that pass the cheap tests (names, codes, counts) repeat a value within
    // their first rows: found there, the pass over every row is not needed.
    let mut seen: FxMap<u64, u32> = FxMap::default();
    let (mut tmp, mut tmp2) = (Vec::new(), Vec::new());
    for p in 0..n.min(PRE_UNIQUE) {
        let hit = st.with_cell(st.data_id(p), col, &mut tmp, |s| {
            if s.is_empty() { return false; }
            let h = if ints { Val::Num(js_number(s)).hash() } else { Val::Str(s).hash() };
            match seen.insert(h, p as u32) {
                None => false,
                Some(q) => ints || st.with_cell(st.data_id(q as usize), col, &mut tmp2, |t| t == s),
            }
        });
        if hit { return Some(false); }
    }
    let parts: Vec<Vec<(u64, u32)>> = (0..n.div_ceil(SLICE)).into_par_iter().map(|k| {
        let mut out = Vec::with_capacity(SLICE);
        let mut tmp = Vec::new();
        if cancel.load(Ordering::Relaxed) { return out; }
        for p in k * SLICE..((k + 1) * SLICE).min(n) {
            st.with_cell(st.data_id(p), col, &mut tmp, |s| {
                if s.is_empty() { return; }
                let h = if ints { Val::Num(js_number(s)).hash() } else { Val::Str(s).hash() };
                out.push((h, p as u32));
            });
        }
        out
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut hs: Vec<(u64, u32)> = parts.into_iter().flatten().collect();
    hs.par_sort_unstable();
    let mut tmp = Vec::new();
    for g in hs.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1) {
        if ints { return Some(false); }
        let vals: Vec<String> = g.iter().map(|&(_, p)| st.with_cell(st.data_id(p as usize), col, &mut tmp, str::to_string)).collect();
        for i in 1..vals.len() {
            if vals[..i].contains(&vals[i]) { return Some(false); }
        }
    }
    Some(true)
}

/// A schema and the hints of the create dialog.
#[derive(Clone, Debug)]
pub struct Inferred {
    pub schema: Schema,
    pub hints: Vec<Hints>,
}

/// The rules a file already follows, read from every row. `progress`, if
/// given, counts the rows read. None if cancelled.
pub fn infer(st: &State, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Inferred> {
    let headers = st.headers();
    let w = headers.len();
    let n = st.len();
    let shape_step = (n / SHAPE_ROWS).max(1);
    let accs: Vec<Vec<Acc>> = (0..n.div_ceil(SLICE)).into_par_iter().map(|k| {
        let mut acc = vec![Acc::new(); w];
        if cancel.load(Ordering::Relaxed) { return acc; }
        let (mut tmp, mut row) = (Vec::new(), Vec::new());
        let end = ((k + 1) * SLICE).min(n);
        for p in k * SLICE..end {
            let shape_row = p % shape_step == 0;
            let mut upto = 0;
            kinds::visit_row(st, st.data_id(p), &mut tmp, &mut row, |c, s| {
                if c < w { acc[c].add(s, p as u32, shape_row); upto = c + 1; }
            });
            for a in &mut acc[upto..] { a.add("", p as u32, shape_row); }
        }
        if let Some(pr) = progress { pr.fetch_add(end - k * SLICE, Ordering::Relaxed); }
        acc
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut cols = vec![Acc::new(); w];
    for slice in accs {
        for (a, b) in cols.iter_mut().zip(slice) { a.merge(b); }
    }
    let mut fields = Vec::with_capacity(w);
    for (c, x) in cols.iter().enumerate() {
        let mut f = field_of(&headers[c], x);
        let mut k = f.constraints.take().unwrap_or_default();
        if may_be_unique(&f, x) && all_different(st, c, x.int, cancel)? { k.unique = Some(true); }
        if f.ty() == "string" && f.format.is_none() && k.pattern.is_none() && k.unique.is_none() {
            if let Some(dv) = x.dv.as_ref().filter(|dv| dv.len() > 1 && dv.len() <= ENUM_MAX && x.n >= dv.len() * ENUM_REPEAT) {
                let mut v: Vec<String> = dv.iter().map(|e| e.0.clone()).collect();
                v.sort_by(|a, b| locale_cmp(a, b));
                k.en = Some(v.into_iter().map(Value::from).collect());
            }
        }
        if !k.is_empty() { f.constraints = Some(k); }
        fields.push(f);
    }
    let hints = fields.iter().zip(&cols).map(|(f, x)| hints(f, x)).collect();
    let keyish = regex::Regex::new(r"(?i)^(id|.*[_ \-]id|.*id|code|ref|reference|key|uuid|siret|siren)$").unwrap();
    let pk: Vec<&Field> = fields.iter().filter(|f| f.constraints.as_ref().is_some_and(|k| k.unique == Some(true) && k.required == Some(true))).collect();
    let best = pk.iter().find(|f| keyish.is_match(&f.name)).or(pk.first()).map(|f| f.name.clone());
    let schema = Schema { fields, missing_values: Some(Value::Array(vec!["".into()])), primary_key: best.map(Value::from), other: Map::new() };
    Some(Inferred { schema, hints })
}

/// `localeCompare`, roughly: accents and case second.
fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    crate::text::fold(a).cmp(&crate::text::fold(b)).then_with(|| a.to_lowercase().cmp(&b.to_lowercase())).then_with(|| b.cmp(a))
}

// ---- Validation ---------------------------------------------------------

/// One kind of error: a rule of a field (or of the rows), how often, a few
/// values, its rows (MARK_MAX at most).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The field's name, empty for a rule of the rows (field count, primary key).
    pub field: String,
    /// "required", "type", "enum", "pattern", "minimum", "maximum", "minLength",
    /// "maxLength", "unique", "fields", "primaryKey", or a check's key.
    pub rule: String,
    pub label: String,
    pub count: usize,
    pub examples: Vec<String>,
    pub rows: Vec<RowId>,
}

#[derive(Clone, Debug, Default)]
pub struct Validation {
    /// Missing or unexpected columns, columns out of order, patterns that do not compile.
    pub structure: Vec<String>,
    /// The most frequent first.
    pub items: Vec<Item>,
    /// Every row at fault, in file order.
    pub rows: Vec<RowId>,
    /// The cells at fault (rows: MARK_MAX at most) and why.
    pub cells: Vec<(RowId, usize, String)>,
    pub checked: usize,
}

impl Validation {
    /// The dialog's summary.
    pub fn summary(&self) -> String {
        let bad = self.rows.len();
        let cols = self.items.iter().filter(|i| !i.field.is_empty()).map(|i| &i.field).collect::<std::collections::HashSet<_>>().len();
        if bad == 0 && self.structure.is_empty() { return format!("All {} rows follow the schema.", fmt_int(self.checked)); }
        let mut s = if bad > 0 { format!("{} of {} rows break the schema", fmt_int(bad), fmt_int(self.checked)) } else { "The rows follow the schema".into() };
        if cols > 0 { s += &format!(", in {} column{}", fmt_int(cols), if cols == 1 { "" } else { "s" }); }
        let k = self.structure.len();
        if k > 0 { s += &format!(" — {} problem{} of structure", fmt_int(k), if k == 1 { "" } else { "s" }); }
        s + "."
    }
}

use kinds::fmt_int;

/// A field's checks, compiled.
struct Ck {
    i: usize,
    c: usize,
    k: Constraints,
    r: Reader,
    re: Option<Re>,
    base: String,
    cks: Vec<(Check, Re)>,
    en: Option<Vec<Owned>>,
    min: Option<Owned>,
    max: Option<Owned>,
}

/// Errors met on a slice of rows, in the order met.
#[derive(Default)]
struct Hits {
    /// (key, label, count, examples, rows, first (position, order in the row)).
    items: Vec<(u32, String, usize, Vec<String>, Vec<RowId>, (u32, u32))>,
    idx: FxMap<u32, usize>,
    rows: Vec<RowId>,
    cells: Vec<(RowId, usize, u32)>,
    slow: bool,
}

impl Hits {
    fn hit(&mut self, key: u32, label: impl FnOnce() -> String, value: &str, id: RowId, p: u32, cell: Option<usize>) {
        let k = *self.idx.entry(key).or_insert_with(|| { self.items.push((key, label(), 0, Vec::new(), Vec::new(), (p, key))); self.items.len() - 1 });
        let it = &mut self.items[k];
        it.2 += 1;
        if it.3.len() < 3 && !it.3.iter().any(|e| e == value) { it.3.push(value.to_string()); }
        if it.4.len() < MARK_MAX { it.4.push(id); }
        if self.rows.last() != Some(&id) { self.rows.push(id); }
        if let Some(c) = cell { self.cells.push((id, c, key)); }
    }
}

const R_REQUIRED: u32 = 0;
const R_TYPE: u32 = 1;
const R_ENUM: u32 = 2;
const R_PATTERN: u32 = 3;
const R_CHECK: u32 = 4;
const R_MIN: u32 = 10;
const R_MAX: u32 = 11;
const R_MINLEN: u32 = 12;
const R_MAXLEN: u32 = 13;
const R_UNIQUE: u32 = 14;

/// A file against a schema. None if cancelled; Err when the schema cannot
/// be run on it (a pattern too slow on its values). `progress` counts rows.
pub fn validate(st: &State, schema: &Schema, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Result<Validation, String>> {
    let w = kinds::expected_width(st);
    let mut h = st.headers();
    h.truncate(w);
    let fields = &schema.fields;
    let mut out = Validation::default();
    let norm = |s: &str| s.trim().to_lowercase();
    let at: Vec<Option<usize>> = fields.iter().map(|f| h.iter().position(|x| *x == f.name).or_else(|| h.iter().position(|x| norm(x) == norm(&f.name)))).collect();
    for (f, a) in fields.iter().zip(&at) {
        if a.is_none() { out.structure.push(format!("missing column “{}”", f.name)); }
    }
    for (c, x) in h.iter().enumerate() {
        if !at.contains(&Some(c)) { out.structure.push(format!("unexpected column “{x}”")); }
    }
    let present: Vec<usize> = at.iter().flatten().copied().collect();
    if present.windows(2).any(|p| p[1] < p[0]) { out.structure.push("the columns are not in the schema's order".into()); }
    let missing: Vec<String> = match &schema.missing_values { Some(Value::Array(a)) => a.iter().map(value_str).collect(), _ => vec![String::new()] };
    let checks: Vec<Ck> = fields.iter().enumerate().filter_map(|(i, f)| {
        let c = at[i]?;
        let k = f.k();
        let (base, ids) = split_pattern(k.pattern.as_deref().unwrap_or(""));
        let re = if base.is_empty() { None } else {
            whole(&base).map_err(|_| out.structure.push(format!("the pattern of “{}” is not a valid regular expression", f.name))).ok()
        };
        let r = Reader::new(f);
        let en = k.en.as_ref().map(|e| e.iter().map(|v| r.bound(v).unwrap_or_else(|| Owned::Str(value_str(v)))).collect());
        let (min, max) = (k.minimum.as_ref().and_then(|v| r.bound(v)), k.maximum.as_ref().and_then(|v| r.bound(v)));
        Some(Ck { i, c, cks: ids.iter().map(|&x| (x, x.re())).collect(), re, base, en, min, max, r, k })
    }).collect();
    let pk_names: Vec<String> = match &schema.primary_key { None | Some(Value::Null) => vec![], Some(Value::Array(a)) => a.iter().map(value_str).collect(), Some(v) => vec![value_str(v)] };
    let pk_cols: Vec<usize> = pk_names.iter().filter_map(|n| fields.iter().position(|f| f.name == *n).and_then(|i| at[i])).collect();
    // A key of one column already checked as unique would report each duplicate twice.
    let pk_unique = pk_cols.len() == 1 && checks.iter().any(|ck| ck.c == pk_cols[0] && ck.k.unique == Some(true));
    let pk_on = !pk_cols.is_empty() && pk_cols.len() == pk_names.len() && !pk_unique;
    let key_of = |ck: &Ck, rule: u32| 1 + ck.i as u32 * 32 + rule;
    let pk_key = 1 + fields.len() as u32 * 32;
    let label_of = |key: u32| -> String {
        if key == 0 { return String::new(); }
        if key == pk_key { return format!("duplicate primary key ({})", pk_names.join(", ")); }
        let (i, rule) = ((key - 1) / 32, (key - 1) % 32);
        let ck = checks.iter().find(|c| c.i == i as usize).unwrap();
        match rule {
            R_REQUIRED => "required: the cell is empty".into(),
            R_ENUM => "not one of the allowed values".into(),
            R_PATTERN => format!("does not match {}", ck.base),
            R_MIN => format!("below the minimum {}", value_str(ck.k.minimum.as_ref().unwrap())),
            R_MAX => format!("above the maximum {}", value_str(ck.k.maximum.as_ref().unwrap())),
            R_MINLEN => format!("shorter than {} characters", ck.k.min_length.unwrap()),
            R_MAXLEN => format!("longer than {} characters", ck.k.max_length.unwrap()),
            R_UNIQUE => "duplicate value".into(),
            r if r >= R_CHECK && r < R_MIN => ck.cks[(r - R_CHECK) as usize].0.texts()[3].into(),
            _ => String::new(),
        }
    };
    let n = st.len();
    // A pattern that backtracks is first tried on values spread over the file (as the JS regex
    // guard did): one that goes past its bound there is refused before the pass.
    let step = (n / 2000).max(1);
    let mut tmp = Vec::new();
    for ck in checks.iter().filter(|ck| matches!(ck.re, Some(Re::Fancy(_)))) {
        let re = ck.re.as_ref().unwrap();
        if (0..n).step_by(step).any(|p| st.with_cell(st.data_id(p), ck.c, &mut tmp, |s| re.test(s).is_err())) { return Some(Err(TOO_SLOW.into())); }
    }
    let slow = AtomicBool::new(false);
    let parts: Vec<Hits> = (0..n.div_ceil(SLICE)).into_par_iter().map(|k| {
        let mut hits = Hits::default();
        if cancel.load(Ordering::Relaxed) || slow.load(Ordering::Relaxed) { return hits; }
        let (mut tmp, mut row) = (Vec::new(), Vec::new());
        let mut cells: Vec<String> = vec![String::new(); w];
        let end = ((k + 1) * SLICE).min(n);
        for p in k * SLICE..end {
            if hits.slow { break; }
            let id = st.data_id(p);
            let p32 = p as u32;
            let mut len = 0;
            kinds::visit_row(st, id, &mut tmp, &mut row, |c, s| {
                if c < w { cells[c].clear(); cells[c].push_str(s); }
                len = c + 1;
            });
            for x in cells.iter_mut().skip(len) { x.clear(); }
            if len != w {
                hits.hit(0, || format!("row of {len} fields instead of {w}"), &format!("row {}", p + 1), id, p32, None);
            }
            for ck in &checks {
                let s = cells[ck.c].as_str();
                let hit = |rule: u32, label: &dyn Fn() -> String, hits: &mut Hits| hits.hit(key_of(ck, rule), label, s, id, p32, Some(ck.c));
                if missing.iter().any(|m| m == s) {
                    if ck.k.required == Some(true) { hit(R_REQUIRED, &|| label_of(key_of(ck, R_REQUIRED)), &mut hits); }
                    continue;
                }
                let v = match ck.r.value(s) {
                    Ok(v) => v,
                    Err(e) => { hit(R_TYPE, &|| e.clone(), &mut hits); continue; }
                };
                let lab = |r: u32| move || label_of(key_of(ck, r));
                if ck.en.as_ref().is_some_and(|en| !en.iter().any(|o| v.same(o))) { hit(R_ENUM, &lab(R_ENUM), &mut hits); }
                if let Some(re) = &ck.re {
                    match re.test(s) { Ok(true) => {}, Ok(false) => hit(R_PATTERN, &lab(R_PATTERN), &mut hits), Err(()) => { hits.slow = true; slow.store(true, Ordering::Relaxed); } }
                }
                for (j, (_, re)) in ck.cks.iter().enumerate() {
                    if re.test(s) == Ok(true) { hit(R_CHECK + j as u32, &lab(R_CHECK + j as u32), &mut hits); }
                }
                if ck.min.as_ref().is_some_and(|m| js_lt(&v, m)) { hit(R_MIN, &lab(R_MIN), &mut hits); }
                if ck.max.as_ref().is_some_and(|m| js_gt(&v, m)) { hit(R_MAX, &lab(R_MAX), &mut hits); }
                if ck.k.min_length.is_some_and(|l| (utf16_len(s) as u64) < l) { hit(R_MINLEN, &lab(R_MINLEN), &mut hits); }
                if ck.k.max_length.is_some_and(|l| utf16_len(s) as u64 > l) { hit(R_MAXLEN, &lab(R_MAXLEN), &mut hits); }
            }
        }
        if let Some(pr) = progress { pr.fetch_add(end - k * SLICE, Ordering::Relaxed); }
        hits
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    if slow.load(Ordering::Relaxed) { return Some(Err(TOO_SLOW.into())); }
    // Duplicates (unique fields, the primary key): the same reading as an earlier row.
    let mut dups: Vec<(u32, u32, String, Option<usize>)> = Vec::new();   // (position, key, value, column)
    for ck in checks.iter().filter(|ck| ck.k.unique == Some(true)) {
        let d = duplicates(st, cancel, |s: &[&str], _| {
            let v = s[0];
            if missing.iter().any(|m| m == v) { return None; }
            ck.r.value(v).ok().map(|x| (x.hash(), v.to_string()))
        }, &[ck.c])?;
        dups.extend(d.into_iter().map(|(p, v)| (p, key_of(ck, R_UNIQUE), v, Some(ck.c))));
    }
    if pk_on {
        let d = duplicates(st, cancel, |s: &[&str], _| {
            let mut hh = rustc_hash::FxHasher::default();
            for v in s { hh.write(v.as_bytes()); hh.write_u8(1); }
            Some((hh.finish(), s.join(" · ")))
        }, &pk_cols)?;
        dups.extend(d.into_iter().map(|(p, v)| (p, pk_key, v, None)));
    }
    dups.sort_by_key(|d| (d.0, d.1));
    // Everything merged in file order.
    let mut all = Hits::default();
    for part in parts {
        for (key, label, count, ex, rows, first) in part.items {
            let k = *all.idx.entry(key).or_insert_with(|| { all.items.push((key, label, 0, Vec::new(), Vec::new(), first)); all.items.len() - 1 });
            let it = &mut all.items[k];
            it.2 += count;
            for e in ex { if it.3.len() < 3 && !it.3.contains(&e) { it.3.push(e); } }
            let room = MARK_MAX - it.4.len();
            it.4.extend(rows.into_iter().take(room));
        }
        all.rows.extend(part.rows);
        all.cells.extend(part.cells);
    }
    for (p, key, v, c) in &dups {
        let id = st.data_id(*p as usize);
        let k = *all.idx.entry(*key).or_insert_with(|| { all.items.push((*key, label_of(*key), 0, Vec::new(), Vec::new(), (*p, *key))); all.items.len() - 1 });
        let it = &mut all.items[k];
        it.2 += 1;
        if it.3.len() < 3 && !it.3.contains(v) { it.3.push(v.clone()); }
        if it.4.len() < MARK_MAX { it.4.push(id); }
        all.rows.push(id);
        if let Some(c) = c { all.cells.push((id, *c, *key)); }
    }
    if !dups.is_empty() {
        let pos: FxMap<RowId, usize> = (0..n).map(|p| (st.data_id(p), p)).collect();
        all.rows.sort_by_key(|id| pos[id]);
        all.rows.dedup();
        for it in &mut all.items { it.4.sort_by_key(|id| pos[id]); }
        all.cells.sort_by_key(|c| (pos[&c.0], c.2));
    }
    // A cell keeps the first reason found; MARK_MAX rows at most.
    let labels: FxMap<u32, String> = all.items.iter().map(|it| (it.0, it.1.clone())).collect();
    let mut seen_rows = 0;
    let mut last: Option<RowId> = None;
    let mut cells = Vec::new();
    for (id, c, key) in all.cells {
        if last != Some(id) {
            if seen_rows >= MARK_MAX { break; }
            seen_rows += 1;
            last = Some(id);
        }
        if !cells.iter().rev().take_while(|x: &&(RowId, usize, String)| x.0 == id).any(|x| x.1 == c) { cells.push((id, c, labels[&key].clone())); }
    }
    all.items.sort_by(|a, b| b.2.cmp(&a.2).then(a.5.cmp(&b.5)));
    out.items = all.items.into_iter().map(|(key, label, count, examples, rows, _)| {
        let (field, rule) = if key == 0 { (String::new(), "fields".to_string()) } else if key == pk_key { (String::new(), "primaryKey".into()) } else {
            let (i, r) = (((key - 1) / 32) as usize, (key - 1) % 32);
            let ck = checks.iter().find(|c| c.i == i).unwrap();
            let rule = match r { R_REQUIRED => "required", R_TYPE => "type", R_ENUM => "enum", R_PATTERN => "pattern", R_MIN => "minimum", R_MAX => "maximum", R_MINLEN => "minLength", R_MAXLEN => "maxLength", R_UNIQUE => "unique", r => ck.cks[(r - R_CHECK) as usize].0.key() };
            (fields[i].name.clone(), rule.to_string())
        };
        Item { field, rule, label, count, examples, rows }
    }).collect();
    out.rows = all.rows;
    out.cells = cells;
    out.checked = n;
    Some(Ok(out))
}

/// Rows whose key (from the cells of `cols`) is the same as an earlier
/// row's: (position, value shown), in file order. Equal hashes are read
/// again and compared by their values.
fn duplicates(st: &State, cancel: &AtomicBool, key: impl Fn(&[&str], ()) -> Option<(u64, String)> + Sync, cols: &[usize]) -> Option<Vec<(u32, String)>> {
    let n = st.len();
    let read = |p: usize, tmp: &mut Vec<u8>, row: &mut Vec<String>, vals: &mut Vec<String>| -> Option<(u64, String)> {
        vals.clear();
        vals.resize(cols.len(), String::new());
        kinds::visit_row(st, st.data_id(p), tmp, row, |c, s| {
            for (k, _) in cols.iter().enumerate().filter(|e| *e.1 == c) { vals[k] = s.to_string(); }
        });
        let refs: Vec<&str> = vals.iter().map(String::as_str).collect();
        key(&refs, ())
    };
    let parts: Vec<Vec<(u64, u32)>> = (0..n.div_ceil(SLICE)).into_par_iter().map(|k| {
        let (mut tmp, mut row, mut vals) = (Vec::new(), Vec::new(), Vec::new());
        let mut out = Vec::new();
        if cancel.load(Ordering::Relaxed) { return out; }
        for p in k * SLICE..((k + 1) * SLICE).min(n) {
            if let Some((h, _)) = read(p, &mut tmp, &mut row, &mut vals) { out.push((h, p as u32)); }
        }
        out
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut hs: Vec<(u64, u32)> = parts.into_iter().flatten().collect();
    hs.par_sort_unstable();
    let (mut tmp, mut row, mut vals) = (Vec::new(), Vec::new(), Vec::new());
    let mut out = Vec::new();
    for g in hs.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1) {
        // The readings of one hash are compared by the text the key gives (the shown value of the reading).
        let mut firsts: Vec<String> = Vec::new();
        for &(_, p) in g {
            let (_, v) = read(p as usize, &mut tmp, &mut row, &mut vals)?;
            let canon = canon_of(&vals);
            if firsts.contains(&canon) { out.push((p, v)); } else { firsts.push(canon); }
        }
    }
    out.sort_by_key(|d| d.0);
    Some(out)
}

/// What decides that two keys of one hash are the same: their cells. (A
/// reading's hash already merges 007 and 7; texts that collide differ here.)
fn canon_of(vals: &[String]) -> String {
    // Numbers that read the same keep one form: the hash said so already; their texts may differ.
    let all_num = vals.iter().all(|v| !v.is_empty() && v.trim().parse::<f64>().is_ok());
    if all_num { return vals.iter().map(|v| js_number(v).to_string()).collect::<Vec<_>>().join("\u{1}"); }
    vals.join("\u{1}")
}

// ---- The create dialog ---------------------------------------------------------

/// A rule of a field, as the dialog lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    Required,
    Unique,
    Minimum,
    Maximum,
    MinLength,
    MaxLength,
    Pattern,
    Enum,
    Format,
    Check(Check),
}

const ORDERED: [&str; 6] = ["integer", "number", "date", "datetime", "time", "year"];

impl Rule {
    pub const ALL: [Rule; 14] = [Rule::Required, Rule::Unique, Rule::Minimum, Rule::Maximum, Rule::MinLength, Rule::MaxLength, Rule::Pattern, Rule::Enum, Rule::Format,
        Rule::Check(Check::NoInvisible), Rule::Check(Check::NoDoubleSpace), Rule::Check(Check::NoMojibake), Rule::Check(Check::UpperCase), Rule::Check(Check::Capitalised)];

    pub fn label(self) -> &'static str {
        match self {
            Rule::Required => "required", Rule::Unique => "unique", Rule::Minimum => "minimum", Rule::Maximum => "maximum",
            Rule::MinLength => "at least", Rule::MaxLength => "at most", Rule::Pattern => "pattern", Rule::Enum => "one of", Rule::Format => "format",
            Rule::Check(c) => c.texts()[0],
        }
    }
    /// Whether a field of this type can take the rule.
    pub fn fits(self, f: &Field) -> bool {
        match self {
            Rule::Minimum | Rule::Maximum => ORDERED.contains(&f.ty()),
            Rule::Pattern | Rule::Format | Rule::Check(_) => f.ty() == "string",
            Rule::Enum => f.ty() != "boolean",
            _ => true,
        }
    }
    fn has(self, f: &Field, checks: &[Check]) -> bool {
        let k = f.constraints.as_ref();
        match self {
            Rule::Required => k.is_some_and(|k| k.required.is_some()),
            Rule::Unique => k.is_some_and(|k| k.unique.is_some()),
            Rule::Minimum => k.is_some_and(|k| k.minimum.is_some()),
            Rule::Maximum => k.is_some_and(|k| k.maximum.is_some()),
            Rule::MinLength => k.is_some_and(|k| k.min_length.is_some()),
            Rule::MaxLength => k.is_some_and(|k| k.max_length.is_some()),
            Rule::Pattern => k.is_some_and(|k| k.pattern.is_some()),
            Rule::Enum => k.is_some_and(|k| k.en.is_some()),
            Rule::Format => f.format.is_some(),
            Rule::Check(c) => checks.contains(&c),
        }
    }
}

pub const FORMATS: [(&str, &str); 3] = [("email", "e-mail"), ("uri", "URI"), ("uuid", "UUID")];

/// The create dialog's state: the inferred schema, the rules ticked, the
/// checks added (kept apart, joined into the pattern when written).
#[derive(Clone, Debug)]
pub struct Draft {
    pub schema: Schema,
    pub hints: Vec<Hints>,
    pub on: Vec<Vec<Rule>>,
    pub checks: Vec<Vec<Check>>,
    pub pk: bool,
}

impl Draft {
    pub fn new(inf: Inferred) -> Draft {
        let on = inf.schema.fields.iter().map(|f| Rule::ALL.into_iter().filter(|r| !matches!(r, Rule::Check(_)) && r.has(f, &[]) && (*r != Rule::Format || f.ty() == "string")).collect()).collect();
        let n = inf.schema.fields.len();
        Draft { pk: inf.schema.primary_key.is_some(), schema: inf.schema, hints: inf.hints, on, checks: vec![Vec::new(); n] }
    }

    /// The rules field i has (ticked or not).
    pub fn rules(&self, i: usize) -> Vec<Rule> {
        let f = &self.schema.fields[i];
        Rule::ALL.into_iter().filter(|r| r.has(f, &self.checks[i]) && (*r != Rule::Format || f.ty() == "string")).collect()
    }

    /// The rules "+ rule" offers for field i: not there yet, fit for its type
    /// (upper case and capitalised exclude each other).
    pub fn addable(&self, i: usize) -> Vec<Rule> {
        let f = &self.schema.fields[i];
        let has = |r: Rule| r.has(f, &self.checks[i]);
        Rule::ALL.into_iter().filter(|&r| !has(r) && r.fits(f)
            && !(r == Rule::Enum && self.hints[i].values.is_none())
            && !(r == Rule::Check(Check::UpperCase) && has(Rule::Check(Check::Capitalised)))
            && !(r == Rule::Check(Check::Capitalised) && has(Rule::Check(Check::UpperCase)))).collect()
    }

    /// Adds a rule, filled in from what the values show; `arg`: a pattern or a format.
    pub fn add_rule(&mut self, i: usize, rule: Rule, arg: &str) {
        let h = self.hints[i].clone();
        let f = &mut self.schema.fields[i];
        let r = Reader::new(f);
        // A bound typed as the field reads it: a number stays a number, a date its text.
        let numeric = matches!(f.ty(), "integer" | "number");
        let typed = |s: &str| -> Value { match r.value(s) { Ok(Val::Num(x)) if numeric => num_value(x), Ok(Val::Bool(b)) => b.into(), _ => s.into() } };
        let k = f.constraints.get_or_insert_with(Default::default);
        let mut on = vec![rule];
        match rule {
            Rule::Required => k.required = Some(true),
            Rule::Unique => k.unique = Some(true),
            Rule::Minimum => k.minimum = Some(h.min.as_deref().map_or(0.into(), typed)),
            Rule::Maximum => k.maximum = Some(h.max.as_deref().map_or(0.into(), typed)),
            Rule::MinLength | Rule::MaxLength => {
                k.min_length = Some(h.lmin.unwrap_or(0) as u64);
                k.max_length = Some(h.lmax.unwrap_or(0) as u64);
                on = vec![Rule::MinLength, Rule::MaxLength];
            }
            Rule::Pattern => k.pattern = Some(arg.into()),
            Rule::Enum => k.en = Some(h.values.unwrap_or_default().into_iter().map(Value::from).collect()),
            Rule::Format => f.format = Some(arg.into()),
            Rule::Check(c) => if !self.checks[i].contains(&c) { self.checks[i].push(c) },
        }
        for r in on { if !self.on[i].contains(&r) { self.on[i].push(r); } }
    }

    /// A value typed for a rule: kept if it reads as one (Err: shown in red until it does).
    pub fn set_value(&mut self, i: usize, rule: Rule, text: &str) -> Result<(), ()> {
        let f = &mut self.schema.fields[i];
        let v: Value = match rule {
            Rule::Minimum | Rule::Maximum => match Reader::new(f).value(text.trim()) {
                Err(_) => return Err(()),
                Ok(Val::Num(x)) if matches!(f.ty(), "integer" | "number") => num_value(x),
                Ok(_) => text.trim().into(),
            },
            Rule::MinLength | Rule::MaxLength => text.trim().parse::<u64>().map_err(|_| ())?.into(),
            Rule::Pattern => { whole(text).map_err(|_| ())?; text.into() }
            Rule::Enum => Value::Array(parse_list(text).into_iter().map(Value::from).collect()),
            Rule::Format => text.into(),
            _ => return Err(()),
        };
        let k = f.constraints.get_or_insert_with(Default::default);
        match rule {
            Rule::Minimum => k.minimum = Some(v),
            Rule::Maximum => k.maximum = Some(v),
            Rule::MinLength => k.min_length = v.as_u64(),
            Rule::MaxLength => k.max_length = v.as_u64(),
            Rule::Pattern => k.pattern = v.as_str().map(String::from),
            Rule::Enum => k.en = v.as_array().cloned(),
            _ => f.format = v.as_str().map(String::from),
        }
        if !self.on[i].contains(&rule) { self.on[i].push(rule); }
        Ok(())
    }

    /// A rule ticked or not.
    pub fn toggle(&mut self, i: usize, rule: Rule, on: bool) {
        self.on[i].retain(|&r| r != rule);
        if on { self.on[i].push(rule); }
    }

    /// Another type: the formats and rules it cannot take go.
    pub fn set_type(&mut self, i: usize, ty: &str) {
        let f = &mut self.schema.fields[i];
        f.ty = Some(ty.into());
        if !matches!(ty, "date" | "datetime") && !(ty == "string" && FORMATS.iter().any(|x| Some(x.0) == f.format.as_deref())) { f.format = None; }
        if ty != "number" { f.decimal_char = None; f.group_char = None; }
        if ty != "boolean" { f.true_values = None; f.false_values = None; }
        let f = f.clone();
        if let Some(k) = &mut self.schema.fields[i].constraints {
            for r in Rule::ALL.into_iter().filter(|r| !r.fits(&f)) {
                match r {
                    Rule::Minimum => k.minimum = None,
                    Rule::Maximum => k.maximum = None,
                    Rule::Pattern => k.pattern = None,
                    Rule::Enum => k.en = None,
                    _ => {}
                }
                self.on[i].retain(|&x| x != r);
            }
        }
        if !f.ty().eq("string") { self.checks[i].clear(); }
        if self.schema.fields[i].format.is_none() { self.on[i].retain(|&x| x != Rule::Format); }
    }

    /// The schema as ticked: only the rules kept, in Table Schema's terms.
    pub fn to_schema(&self) -> Schema {
        let mut out = Schema { fields: Vec::new(), missing_values: self.schema.missing_values.clone(), primary_key: None, other: self.schema.other.clone() };
        for (i, f) in self.schema.fields.iter().enumerate() {
            let on = &self.on[i];
            let mut g = f.clone();
            if f.ty() == "string" && !on.contains(&Rule::Format) { g.format = None; }
            let k = f.k();
            let mut c = Constraints { other: k.other.clone(), ..Default::default() };
            let pick = |r: Rule| on.contains(&r);
            if pick(Rule::Minimum) { c.minimum = k.minimum.clone(); }
            if pick(Rule::Maximum) { c.maximum = k.maximum.clone(); }
            if pick(Rule::Pattern) { c.pattern = k.pattern.clone(); }
            if pick(Rule::Required) { c.required = k.required; }
            if pick(Rule::Unique) { c.unique = k.unique; }
            if pick(Rule::Enum) { c.en = k.en.clone(); }
            if pick(Rule::MinLength) { c.min_length = k.min_length; }
            if pick(Rule::MaxLength) { c.max_length = k.max_length; }
            let checks: Vec<Check> = Check::ALL.into_iter().filter(|x| self.checks[i].contains(x) && pick(Rule::Check(*x))).collect();
            if !checks.is_empty() { c.pattern = Some(join_pattern(c.pattern.as_deref().unwrap_or(""), &checks)); }
            g.constraints = (!c.is_empty()).then_some(c);
            out.fields.push(g);
        }
        if self.pk { out.primary_key = self.schema.primary_key.clone(); }
        out
    }

    /// How many rows of this file break rule `rule` of field i (shown beside
    /// it); None if cancelled or if the rule cannot be counted (no column, a
    /// bound that does not read).
    pub fn count_breaking(&self, st: &State, i: usize, rule: Rule, cancel: &AtomicBool) -> Option<usize> {
        let f = &self.schema.fields[i];
        let col = st.headers().iter().position(|h| *h == f.name)?;
        let k = f.k();
        let r = Reader::new(f);
        let test: Box<dyn Fn(&str) -> bool + Sync> = match rule {
            Rule::Required => Box::new(|s| s.is_empty()),
            Rule::Unique => {
                let d = duplicates(st, cancel, |s: &[&str], _| {
                    if s[0].is_empty() { return None; }
                    Some(match r.value(s[0]) { Ok(v) => (v.hash(), String::new()), Err(_) => (Val::Str(s[0]).hash(), String::new()) })
                }, &[col])?;
                return Some(d.len());
            }
            Rule::Minimum | Rule::Maximum => {
                let lim = if rule == Rule::Minimum { k.minimum.as_ref() } else { k.maximum.as_ref() }.and_then(|v| r.bound(v))?;
                let min = rule == Rule::Minimum;
                Box::new(move |s| !s.is_empty() && r.value(s).is_ok_and(|v| if min { js_lt(&v, &lim) } else { js_gt(&v, &lim) }))
            }
            Rule::MinLength => { let m = k.min_length?; Box::new(move |s| !s.is_empty() && (utf16_len(s) as u64) < m) }
            Rule::MaxLength => { let m = k.max_length?; Box::new(move |s| utf16_len(s) as u64 > m) }
            Rule::Enum => { let all: Vec<String> = k.en.iter().flatten().map(value_str).collect(); Box::new(move |s| !s.is_empty() && !all.iter().any(|a| a == s)) }
            Rule::Format => {
                let fr = Reader::new(&Field { ty: Some("string".into()), format: f.format.clone(), ..Default::default() });
                Box::new(move |s| !s.is_empty() && fr.value(s).is_err())
            }
            Rule::Check(c) => { let re = c.re(); Box::new(move |s| re.test(s) == Ok(true)) }
            Rule::Pattern => { let re = whole(k.pattern.as_deref().filter(|p| !p.is_empty())?).ok()?; Box::new(move |s| !s.is_empty() && re.test(s) != Ok(true)) }
        };
        let n = st.len();
        let counts: Vec<usize> = (0..n.div_ceil(SLICE)).into_par_iter().map(|k| {
            let mut tmp = Vec::new();
            if cancel.load(Ordering::Relaxed) { return 0; }
            (k * SLICE..((k + 1) * SLICE).min(n)).filter(|&p| st.with_cell(st.data_id(p), col, &mut tmp, |s| test(s))).count()
        }).collect();
        if cancel.load(Ordering::Relaxed) { None } else { Some(counts.iter().sum()) }
    }
}

/// A rule's list as the dialog shows it: quoted as in a CSV when a value holds a comma.
pub fn list_text(values: &[Value]) -> String {
    values.iter().map(|v| {
        let s = value_str(v);
        if s.contains([',', '"']) || s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
    }).collect::<Vec<_>>().join(", ")
}

/// `a, b, "c, d"` → [a, b, c, d]: quotes as in a CSV for a value holding a comma.
pub fn parse_list(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let t = rest.trim_start();
        if let Some(q) = t.strip_prefix('"') {
            // A quoted value: up to a quote not doubled.
            let mut v = String::new();
            let mut it = q.char_indices().peekable();
            let mut end = q.len();
            while let Some((i, c)) = it.next() {
                if c == '"' {
                    if it.peek().map(|e| e.1) == Some('"') { v.push('"'); it.next(); continue; }
                    end = i + 1;
                    break;
                }
                v.push(c);
            }
            out.push(v);
            let after = q[end..].trim_start();
            rest = after.strip_prefix(',').unwrap_or(if after.is_empty() { "" } else { after });
            if after.is_empty() { break; }
        } else {
            let (v, r) = t.split_once(',').map_or((t, None), |(a, b)| (a, Some(b)));
            if !v.trim().is_empty() { out.push(v.trim().to_string()); }
            match r { Some(r) => rest = r, None => break }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_order_without_encoding() {
        let words = ["", "a", "ab", "b", "é", "e\u{301}", "\u{e000}", "\u{ffff}", "\u{10000}", "a\u{10000}", "a\u{ffff}", "\u{7ff}", "\u{800}", "z\u{1f600}x", "z\u{fe0f}"];
        for a in words {
            for b in words {
                assert_eq!(utf16_cmp(a, b), a.encode_utf16().cmp(b.encode_utf16()), "{a:?} {b:?}");
            }
        }
    }
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use serde_json::json;
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn good() -> String {
        let mut s = String::from("id;code;montant;date;actif;email;statut;note\r\n");
        for i in 1..121usize {
            s += &format!("{i};{:05};{},{}0;{:02}/{:02}/2024;{};client{i}@exemple.fr;{};{}\r\n", i % 7, i * 3, i % 10, i % 28 + 1, i % 12 + 1,
                if i % 3 != 0 { "oui" } else { "non" }, ["actif", "inactif", "vip"][i % 3], if i % 4 != 0 { format!("texte libre {i}") } else { String::new() });
        }
        s
    }

    const BAD: &str = "id;code;montant;date;actif;email;statut;extra\r\n\
        1;00001;10,50;01/01/2024;oui;a@b.fr;actif;x\r\n\
        1;00002;-3,00;31/02/2024;peut-être;pas-un-mail;inconnu;x\r\n\
        3;123;abc;01/01/2024;non;c@d.fr;vip;x\r\n\
        ;00004;1,00;01/01/2024;oui;e@f.fr;actif\r\n";

    fn no() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn inferred() -> Inferred {
        infer(&table(&good()).st, &no(), None).unwrap()
    }

    fn field(s: &Schema, name: &str) -> Value {
        serde_json::to_value(s.fields.iter().find(|f| f.name == name).unwrap()).unwrap()
    }

    #[test]
    fn inference() {
        let s = inferred().schema;
        assert_eq!(field(&s, "id"), json!({"name": "id", "type": "integer", "constraints": {"minimum": 0, "required": true, "unique": true}}));
        assert_eq!(field(&s, "code"), json!({"name": "code", "type": "string", "constraints": {"pattern": "\\d{5}", "required": true}}));
        assert_eq!(field(&s, "montant"), json!({"name": "montant", "type": "number", "decimalChar": ",", "constraints": {"minimum": 0, "required": true}}));
        assert_eq!(field(&s, "date"), json!({"name": "date", "type": "date", "format": "%d/%m/%Y", "constraints": {"required": true}}));
        assert_eq!(field(&s, "actif"), json!({"name": "actif", "type": "boolean", "trueValues": ["oui"], "falseValues": ["non"], "constraints": {"required": true}}));
        assert_eq!(field(&s, "email"), json!({"name": "email", "type": "string", "format": "email", "constraints": {"required": true, "unique": true}}));
        assert_eq!(field(&s, "statut")["constraints"], json!({"required": true, "enum": ["actif", "inactif", "vip"]}));
        assert_eq!(field(&s, "note"), json!({"name": "note", "type": "string"}));
        assert_eq!(s.primary_key, Some(json!("id")));
        // The text written, read back the same.
        assert_eq!(parse(&to_json(&s)).unwrap(), s);
        assert!(to_json(&s).starts_with("{\n  \"fields\": [\n    {\n      \"name\": \"id\",\n      \"type\": \"integer\""));
    }

    #[test]
    fn own_file_is_valid() {
        let s = inferred().schema;
        let v = validate(&table(&good()).st, &s, &no(), None).unwrap().unwrap();
        assert_eq!((v.rows.len(), v.structure.len(), v.checked), (0, 0, 120));
        assert_eq!(v.summary(), "All 120 rows follow the schema.");
    }

    #[test]
    fn bad_file() {
        let s = inferred().schema;
        let t = table(BAD);
        let v = validate(&t.st, &s, &no(), None).unwrap().unwrap();
        assert_eq!(v.structure, vec!["missing column “note”", "unexpected column “extra”"]);
        let mut items: Vec<(String, usize, Vec<String>)> = v.items.iter().map(|i| (format!("{} {}", if i.field.is_empty() { "-" } else { &i.field }, i.rule), i.count, i.examples.clone())).collect();
        items.sort();
        let mut want: Vec<(String, usize, Vec<String>)> = [("id unique", "1"), ("id required", ""), ("montant minimum", "-3,00"), ("montant type", "abc"), ("date type", "31/02/2024"),
            ("actif type", "peut-être"), ("email type", "pas-un-mail"), ("statut enum", "inconnu"), ("code pattern", "123"), ("- fields", "row 4")]
            .iter().map(|(k, e)| (k.to_string(), 1, vec![e.to_string()])).collect();
        want.sort();
        assert_eq!(items, want);
        assert_eq!(v.rows.len(), 3);
        // The report's marks: the cell at fault says why.
        let h = t.st.headers();
        assert!(v.cells.iter().any(|(_, c, l)| h[*c] == "actif" && l == "not a boolean"));
        assert_eq!(v.items.iter().find(|i| i.rule == "unique").unwrap().rows, vec![t.st.data_id(1)]);
    }

    #[test]
    fn primary_key_of_two_columns() {
        let t = table("a;b\n1;x\n1;y\n1;x\n");
        let s = Schema { fields: vec![Field { name: "a".into(), ..Default::default() }, Field { name: "b".into(), ..Default::default() }], primary_key: Some(json!(["a", "b"])), ..Default::default() };
        let v = validate(&t.st, &s, &no(), None).unwrap().unwrap();
        assert_eq!(v.items.len(), 1);
        assert_eq!((v.items[0].rule.as_str(), v.items[0].label.as_str(), v.items[0].examples[0].as_str()), ("primaryKey", "duplicate primary key (a, b)", "1 · x"));
        assert_eq!(v.rows, vec![t.st.data_id(2)]);
    }

    #[test]
    fn stems_of_dated_exports() {
        let r: Vec<Vec<String>> = ["clients_2026-10-04.csv", "clients-20261004.csv", "clients v3.csv", "clients (2).csv", "export_2026-10-04T08-30.csv", "clients.csv"].iter().map(|n| stems(n)).collect();
        let want: Vec<Vec<&str>> = vec![vec!["clients_2026-10-04", "clients"], vec!["clients-20261004", "clients"], vec!["clients v3", "clients"], vec!["clients (2)", "clients"], vec!["export_2026-10-04T08-30", "export"], vec!["clients"]];
        assert_eq!(r, want);
    }

    #[test]
    fn written_beside_and_found_for_a_dated_export() {
        let dir = std::env::temp_dir().join(format!("csvfab-schema-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = inferred().schema;
        let p = path_beside(&dir.join("clients.csv"));
        assert!(p.ends_with("clients.schema.json"));
        std::fs::write(&p, to_json(&s)).unwrap();
        let (found, text) = find_beside(&dir.join("clients_2026-10-04.csv")).unwrap();
        assert_eq!(found, p);
        assert_eq!(parse(&text).unwrap(), s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn catastrophic_pattern_runs_in_linear_time() {
        // The JS version refused (a+)+ (it would freeze the page); the regex crate runs it at once.
        // Two columns: one alone would be read as lines of text.
        let mut csv = String::from("id;mot\r\n");
        for i in 0..50 { csv += &format!("{i};{}b\r\n", "a".repeat(30)); }
        let s = Schema { fields: vec![Field { name: "mot".into(), ty: Some("string".into()), constraints: Some(Constraints { pattern: Some("(a+)+".into()), ..Default::default() }), ..Default::default() }], ..Default::default() };
        let t0 = std::time::Instant::now();
        let v = validate(&table(&csv).st, &s, &no(), None).unwrap().unwrap();
        assert!(t0.elapsed().as_millis() < 500);
        assert_eq!(v.items[0].count, 50);
        // A look-around pattern that backtracks without end is refused.
        let mut s2 = s.clone();
        s2.fields[0].constraints.as_mut().unwrap().pattern = Some(r"(a|aa)+\1c".into());
        let r = validate(&table(&csv).st, &s2, &no(), None).unwrap();
        assert!(r.as_ref().is_err_and(|e| e.contains("too long")), "{:?}", r.map(|v| v.items));
    }

    #[test]
    fn rules_added_by_hand() {
        let t = table(&good());
        let mut d = Draft::new(inferred());
        let col = |n: &str| d.schema.fields.iter().position(|f| f.name == n).unwrap();
        let (code, note, montant, date, id, statut) = (col("code"), col("note"), col("montant"), col("date"), col("id"), col("statut"));
        assert_eq!(d.hints[code].patterns.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), vec![r"\d{5}", r"\d+"]);
        assert_eq!(d.hints[note].patterns[0], ("[a-zß-öø-ÿ]{5} [a-zß-öø-ÿ]{5} \\d{1,3}".to_string(), "the shape of the values, with the lengths seen".to_string()));
        assert_eq!((d.hints[montant].min.as_deref(), d.hints[montant].max.as_deref()), (Some("3,10"), Some("360,00")));
        assert_eq!((d.hints[date].min.as_deref(), d.hints[date].max.as_deref()), (Some("01/01/2024"), Some("28/12/2024")));
        assert_eq!((d.hints[note].lmin, d.hints[note].lmax), (Some(13), Some(15)));
        d.add_rule(montant, Rule::Maximum, "");
        d.set_value(id, Rule::Minimum, "100").unwrap();
        d.add_rule(note, Rule::Pattern, r"texte libre \d+");
        d.set_value(statut, Rule::Enum, r#"actif, "vip, or""#).unwrap();
        assert!(d.set_value(montant, Rule::Minimum, "abc").is_err());
        let c = |i: usize, r: Rule| d.count_breaking(&t.st, i, r, &no());
        assert_eq!((c(montant, Rule::Maximum), c(id, Rule::Minimum), c(note, Rule::Pattern), c(statut, Rule::Enum)), (Some(0), Some(99), Some(0), Some(80)));
        let s = d.to_schema();
        let k = |n: &str| field(&s, n)["constraints"].clone();
        assert_eq!(k("montant"), json!({"minimum": 0, "required": true, "maximum": 360}));
        assert_eq!(k("id"), json!({"minimum": 100, "required": true, "unique": true}));
        assert_eq!(k("note"), json!({"pattern": "texte libre \\d+"}));
        assert_eq!(k("statut"), json!({"required": true, "enum": ["actif", "vip, or"]}));
        let en = s.fields[statut].constraints.as_ref().unwrap().en.clone().unwrap();
        assert_eq!(parse_list(&list_text(&en)), vec!["actif", "vip, or"]);
    }

    #[test]
    fn shapes() {
        let want = [r"[A-ZÀ-ÖØ-Þ]{2}-\d{4}", r"\+\d{2} \d \d{2} \d{2} \d{2} \d{2}", r"[a-zß-öø-ÿ]\{[a-zß-öø-ÿ]\}\+[a-zß-öø-ÿ]", r"[A-ZÀ-ÖØ-Þ][a-zß-öø-ÿ]{5}", r"[a-zß-öø-ÿ]\.[a-zß-öø-ÿ]\/[a-zß-öø-ÿ]"];
        for (v, w) in ["AB-1234", "+33 6 12 34 56 78", "a{b}+c", "Élodie", "x.y/z"].iter().zip(want) {
            let (e, g) = shape(v);
            assert_eq!(shape_regex(&e), w);
            assert_eq!(whole(&shape_regex(&e)).unwrap().test(v), Ok(true));
            assert_eq!(whole(&shape_regex(&g)).unwrap().test(v), Ok(true));
        }
    }

    #[test]
    fn text_checks() {
        let noms = ["Dupont", "DUPONT", "dupont", "Le  Havre", "Ã©cole", "Zoé\u{200b}", "Boulogne-sur-Mer", "Jean-Pierre"];
        let mut csv = String::from("id;nom\r\n");
        for (i, n) in noms.iter().enumerate() { csv += &format!("{};{n}\r\n", i + 1); }
        let t = table(&csv);
        let mut d = Draft::new(infer(&t.st, &no(), None).unwrap());
        let c = 1;
        let has = |d: &Draft, r: Rule| d.addable(c).contains(&r);
        assert!(has(&d, Rule::Check(Check::UpperCase)) && has(&d, Rule::Check(Check::Capitalised)) && has(&d, Rule::Check(Check::NoMojibake)));
        for k in [Check::NoInvisible, Check::NoDoubleSpace, Check::NoMojibake, Check::Capitalised] { d.add_rule(c, Rule::Check(k), ""); }
        assert!(!has(&d, Rule::Check(Check::UpperCase)) && !has(&d, Rule::Check(Check::Capitalised)));
        d.add_rule(c, Rule::Pattern, r"[\s\S]{1,20}");
        let counts: Vec<Option<usize>> = [Rule::Check(Check::NoInvisible), Rule::Check(Check::NoDoubleSpace), Rule::Check(Check::NoMojibake), Rule::Check(Check::Capitalised), Rule::Pattern]
            .iter().map(|&r| d.count_breaking(&t.st, c, r, &no())).collect();
        assert_eq!(counts, vec![Some(1), Some(1), Some(1), Some(2), Some(0)]);
        let s = d.to_schema();
        let f = serde_json::to_value(&s.fields[c]).unwrap();
        assert_eq!(f.as_object().unwrap().keys().collect::<Vec<_>>(), vec!["constraints", "name", "type"]);
        let p = s.fields[c].constraints.as_ref().unwrap().pattern.clone().unwrap();
        assert_eq!(split_pattern(&p), (r"[\s\S]{1,20}".to_string(), vec![Check::NoInvisible, Check::NoDoubleSpace, Check::NoMojibake, Check::Capitalised]));
        assert_eq!(split_pattern(r"(?![\s\S]*(?: {2}))x|y"), (r"(?![\s\S]*(?: {2}))x|y".to_string(), vec![]));
        let v = validate(&t.st, &s, &no(), None).unwrap().unwrap();
        let mut items: Vec<(String, usize)> = v.items.iter().map(|i| (i.rule.clone(), i.count)).collect();
        items.sort();
        assert_eq!(items, vec![("capitalised".into(), 2), ("noDoubleSpace".into(), 1), ("noInvisible".into(), 1), ("noMojibake".into(), 1)]);
        let cap = v.items.iter().find(|i| i.rule == "capitalised").unwrap();
        let mut ex = cap.examples.clone();
        ex.sort();
        assert_eq!(ex, vec!["DUPONT", "dupont"]);
        assert_eq!(v.items.iter().find(|i| i.rule == "noMojibake").unwrap().label, "holds garbled accents (mojibake)");
        // The whole pattern (look-aheads included) says the same.
        let re = whole(&p).unwrap();
        assert_eq!(noms.iter().filter(|n| re.test(n) == Ok(true)).copied().collect::<Vec<_>>(), vec!["Dupont", "Boulogne-sur-Mer", "Jean-Pierre"]);
    }

    #[test]
    fn readings() {
        let r = Reader::new(&Field { ty: Some("number".into()), decimal_char: Some(",".into()), group_char: Some(" ".into()), ..Default::default() });
        assert!(matches!(r.value("1 234,5"), Ok(Val::Num(x)) if x == 1234.5));
        assert!(r.value("1.5").is_err());
        let d = Reader::new(&Field { ty: Some("date".into()), format: Some("%m/%d/%y".into()), ..Default::default() });
        assert!(matches!(d.value("12/31/99"), Ok(Val::Num(x)) if x == 19991231000000.0));
        let dt = Reader::new(&Field { ty: Some("datetime".into()), ..Default::default() });
        assert!(matches!(dt.value("2024-03-15T10:20:30Z"), Ok(Val::Num(x)) if x == 20240315102030.0));
        assert!(dt.value("2024-02-30T10:20").is_err());
        assert!(is_email("a.b@c.fr") && !is_email("a@b") && !is_email("a@.b"));
        assert!(is_num_comma("1 234,50") && is_num_comma("-12") && !is_num_comma("1234 567"));
        assert!(is_num("1.") && is_num(".5") && is_num("1e-3") && !is_num("."));
        assert_eq!(js_regex(r"[\d-]\w"), "[0-9-][0-9A-Za-z_]");
        assert_eq!(parse("{}"), Err("no \"fields\" list: not a Table Schema".into()));
    }
}
