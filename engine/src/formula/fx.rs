//! The helpers a formula calls (num, date, contains…), the methods of its
//! values ({City}.upper(), the JavaScript string methods…) and Math. Each
//! keeps the web app's behaviour, coercions included: num("") is 0, a helper
//! reads null as "", `{Date}.date().year()` throws on a cell that is no date.

use super::eval::{Args, Cx, Scratch};
use super::jsre::{self, Rx};
use super::value::*;
use super::{date, hash, sound};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::Arc;

macro_rules! names {
    ($ty:ident { $($v:ident = $n:literal),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $ty { $($v),* }
        #[allow(dead_code)]
        impl $ty {
            pub const ALL: &'static [$ty] = &[$($ty::$v),*];
            pub fn name(self) -> &'static str { match self { $($ty::$v => $n),* } }
            pub fn by_name(n: &str) -> Option<$ty> { match n { $($n => Some($ty::$v),)* _ => None } }
        }
    };
}

// In the web app's order (Object.keys(FX)): the autocomplete lists them so.
names!(Fx {
    Num = "num", Date = "date", Today = "today", Days = "days", AddDays = "addDays", Year = "year", Month = "month", Day = "day",
    FmtDate = "fmtDate", Round = "round", Fixed = "fixed", Upper = "upper", Lower = "lower", Trim = "trim", Capitalize = "capitalize",
    Slug = "slug", Len = "len", Left = "left", Right = "right", Pad = "pad", Extract = "extract", Replace = "replace",
    Contains = "contains", Empty = "empty", First = "first", Join = "join", Digits = "digits", FirstNumber = "firstNumber",
    Abs = "abs", Min = "min", Max = "max", Sum = "sum", Avg = "avg", Split = "split", Mid = "mid", StartsWith = "startsWith",
    EndsWith = "endsWith", Matches = "matches", Weekday = "weekday", Week = "week", AddMonths = "addMonths", Months = "months",
    Years = "years", IsEmail = "isEmail", IsNumber = "isNumber", IsDate = "isDate", IsPhone = "isPhone", Ifs = "ifs", Cases = "cases",
    Phonetic = "phonetic", Similarity = "similarity", Uuid = "uuid", Uuid7 = "uuid7", Md5 = "md5", Sha1 = "sha1", Sha256 = "sha256",
    Sha3_256 = "sha3_256", Sha3_512 = "sha3_512", Blake2b = "blake2b", Blake3 = "blake3",
});

// JavaScript's own methods a formula may call (FX_SAFE_NATIVE): never one that
// returns a function or reaches a constructor.
names!(Nat {
    Length = "length", At = "at", CharAt = "charAt", CharCodeAt = "charCodeAt", CodePointAt = "codePointAt", EndsWith = "endsWith",
    Includes = "includes", IndexOf = "indexOf", LastIndexOf = "lastIndexOf", LocaleCompare = "localeCompare", Match = "match",
    Normalize = "normalize", PadEnd = "padEnd", PadStart = "padStart", Repeat = "repeat", Replace = "replace", ReplaceAll = "replaceAll",
    Search = "search", Slice = "slice", Split = "split", StartsWith = "startsWith", Substring = "substring", ToLowerCase = "toLowerCase",
    ToUpperCase = "toUpperCase", ToLocaleLowerCase = "toLocaleLowerCase", ToLocaleUpperCase = "toLocaleUpperCase", Trim = "trim",
    TrimStart = "trimStart", TrimEnd = "trimEnd", ToFixed = "toFixed", ToPrecision = "toPrecision", ToString = "toString", Join = "join",
    Concat = "concat", Reverse = "reverse", Test = "test", GetTime = "getTime", GetFullYear = "getFullYear", GetMonth = "getMonth",
    GetDate = "getDate", GetDay = "getDay", GetHours = "getHours", GetMinutes = "getMinutes", GetSeconds = "getSeconds",
    GetUTCFullYear = "getUTCFullYear", GetUTCMonth = "getUTCMonth", GetUTCDate = "getUTCDate", GetUTCDay = "getUTCDay",
    GetUTCHours = "getUTCHours", GetUTCMinutes = "getUTCMinutes", GetUTCSeconds = "getUTCSeconds", ToISOString = "toISOString",
});

names!(Mf {
    Abs = "abs", Ceil = "ceil", Floor = "floor", Round = "round", Trunc = "trunc", Sign = "sign", Sqrt = "sqrt", Cbrt = "cbrt",
    Pow = "pow", Min = "min", Max = "max", Log = "log", Log10 = "log10", Log2 = "log2", Exp = "exp", Random = "random",
});

/// Helpers that are also methods of texts — every cell is one: {City}.upper(), {Amount}.num().round(2).
pub const ON_TEXT: &[Fx] = &[Fx::Num, Fx::Date, Fx::Days, Fx::Months, Fx::Years, Fx::AddDays, Fx::AddMonths, Fx::Year, Fx::Month,
    Fx::Day, Fx::Weekday, Fx::Week, Fx::FmtDate, Fx::Round, Fx::Fixed, Fx::Abs, Fx::Upper, Fx::Lower, Fx::Capitalize, Fx::Slug,
    Fx::Len, Fx::Left, Fx::Right, Fx::Mid, Fx::Pad, Fx::Extract, Fx::Contains, Fx::Matches, Fx::Empty, Fx::IsEmail, Fx::IsPhone,
    Fx::IsNumber, Fx::IsDate, Fx::FirstNumber, Fx::Digits, Fx::Cases, Fx::Md5, Fx::Sha1, Fx::Sha256, Fx::Sha3_256, Fx::Sha3_512,
    Fx::Blake2b, Fx::Blake3, Fx::Phonetic, Fx::Similarity];
/// … of numbers (cases: {Date}.month().cases(1, "January", …))
pub const ON_NUMBER: &[Fx] = &[Fx::Round, Fx::Fixed, Fx::Abs, Fx::Cases];
/// … and of dates.
pub const ON_DATE: &[Fx] = &[Fx::Days, Fx::Months, Fx::Years, Fx::AddDays, Fx::AddMonths, Fx::Year, Fx::Month, Fx::Day, Fx::Weekday,
    Fx::Week, Fx::FmtDate];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meth {
    Nat(Nat),
    Fx(Fx),
}

impl Meth {
    /// A name after a dot: JavaScript's own method first (trim, split, replace stay JavaScript's), else a helper's.
    pub fn by_name(n: &str) -> Option<Meth> {
        Nat::by_name(n).map(Meth::Nat).or_else(|| Fx::by_name(n).filter(|f| ON_TEXT.contains(f)).map(Meth::Fx))
    }

    pub fn name(self) -> &'static str {
        match self { Meth::Nat(n) => n.name(), Meth::Fx(f) => f.name() }
    }
}

impl Fx {
    /// Whether a call on constants can be computed once at compile time: not
    /// the random ones, not today() (a formula may outlive the day), not fixed(),
    /// whose decimal mark is set after compiling.
    pub fn pure(self) -> bool {
        !matches!(self, Fx::Uuid | Fx::Uuid7 | Fx::Today | Fx::Fixed)
    }

    /// Whether the helper always gives a text.
    pub fn gives_text(self) -> bool {
        matches!(self, Fx::Upper | Fx::Lower | Fx::Trim | Fx::Capitalize | Fx::Slug | Fx::Left | Fx::Right | Fx::Pad | Fx::Extract
            | Fx::Replace | Fx::Join | Fx::Digits | Fx::FirstNumber | Fx::Split | Fx::Mid | Fx::Fixed | Fx::FmtDate | Fx::Phonetic
            | Fx::Uuid | Fx::Uuid7 | Fx::Md5 | Fx::Sha1 | Fx::Sha256 | Fx::Sha3_256 | Fx::Sha3_512 | Fx::Blake2b | Fx::Blake3)
    }

    /// Flags of the regex a helper makes of a text pattern; None: the helper takes no pattern.
    pub fn pattern_flags(self) -> Option<&'static str> {
        match self { Fx::Extract => Some(""), Fx::Replace => Some("g"), Fx::Matches => Some("i"), _ => None }
    }
}

// ---- texts ------------------------------------------------------------------

/// Removes accents the way NFD and dropping U+0300–U+036F does.
pub fn remove_accents(s: &str, out: &mut String) {
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            unicode_normalization::char::decompose_canonical(c, |d| {
                if !('\u{300}'..='\u{36f}').contains(&d) { out.push(d) }
            });
        }
    }
}

/// Lower case, no accents, no symbols: "Émile  O'Brien" → "emile o brien" (slugify).
pub fn slugify(s: &str) -> String {
    let mut plain = String::with_capacity(s.len());
    remove_accents(&s.to_lowercase(), &mut plain);
    let mut out = String::with_capacity(plain.len());
    let mut gap = false;
    for c in plain.chars() {
        let lig = match c { 'œ' => "oe", 'æ' => "ae", 'ß' => "ss", 'ø' => "o", 'đ' => "d", 'ł' => "l", 'ð' => "d", 'þ' => "th", _ => "" };
        if !lig.is_empty() || c.is_alphanumeric() {
            if gap && !out.is_empty() { out.push(' ') }
            gap = false;
            if lig.is_empty() { out.push(c) } else { out.push_str(lig) }
        } else {
            gap = true;
        }
    }
    out
}

/// Lower case and accents off, into `out` (contains, startsWith…: removeAccents(toLowerCase)).
fn fold<'b>(s: &str, out: &'b mut String) -> &'b str {
    out.clear();
    crate::text::fold_into(s, out);
    out
}

/// UTF-16 length: JavaScript counts code units.
pub fn len16(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char::len_utf16).sum() }
}

/// The byte offset of UTF-16 index `i` (clamped; inside a surrogate pair, the character's start).
pub fn byte_of(s: &str, i: usize) -> usize {
    if s.is_ascii() {
        return i.min(s.len());
    }
    let mut u = 0;
    for (b, c) in s.char_indices() {
        if u >= i { return b }
        u += c.len_utf16();
        if u > i { return b }
    }
    s.len()
}

/// A relative index (slice, at): negative from the end, clamped to 0..=len.
fn rel(x: f64, len: usize) -> usize {
    let x = to_int(x);
    if x < 0.0 { (len as f64 + x).max(0.0) as usize } else { x.min(len as f64) as usize }
}

/// Part of a text, borrowed when the text is.
pub fn sub<'a>(s: Cow<'a, str>, a: usize, b: usize) -> Cow<'a, str> {
    let b = b.max(a);
    match s {
        Cow::Borrowed(t) => Cow::Borrowed(&t[a..b]),
        Cow::Owned(mut t) if a == 0 => { t.truncate(b); Cow::Owned(t) }
        Cow::Owned(t) => Cow::Owned(t[a..b].to_string()),
    }
}

/// slice(start, end) in UTF-16 units.
fn slice16<'a>(s: Cow<'a, str>, start: f64, end: Option<f64>) -> Cow<'a, str> {
    let n = len16(&s);
    let (a, b) = (rel(start, n), end.map_or(n, |e| rel(e, n)));
    if a >= b {
        return Cow::Borrowed("");
    }
    let (x, y) = (byte_of(&s, a), byte_of(&s, b));
    sub(s, x, y)
}

/// A value computed on a text, borrowing it when the text is borrowed.
fn on_text<'a>(s: Cow<'a, str>, f: impl for<'b> FnOnce(&'b str) -> Val<'b>) -> Val<'a> {
    match s {
        Cow::Borrowed(t) => f(t),
        Cow::Owned(t) => f(&t).into_static().unwrap_or(Val::Undef),
    }
}

fn upper(s: Cow<str>) -> Cow<str> {
    if s.is_ascii() {
        if !s.bytes().any(|c| c.is_ascii_lowercase()) { return s }
        return Cow::Owned(s.to_ascii_uppercase());
    }
    Cow::Owned(s.to_uppercase())
}

fn lower(s: Cow<str>) -> Cow<str> {
    if s.is_ascii() {
        if !s.bytes().any(|c| c.is_ascii_uppercase()) { return s }
        return Cow::Owned(s.to_ascii_lowercase());
    }
    Cow::Owned(s.to_lowercase())
}

/// trim(): white space removed at both ends, runs of it made one space.
fn clean_spaces(s: Cow<str>) -> Cow<str> {
    let t = trim(&s);
    let mut prev = false;
    let tidy = t.chars().all(|c| {
        let ok = c == ' ' && !prev || !is_space(c);
        prev = is_space(c);
        ok
    });
    if tidy {
        let (a, b) = (t.as_ptr() as usize - s.as_ptr() as usize, t.len());
        return sub(s, a, a + b);
    }
    let mut out = String::with_capacity(t.len());
    for w in t.split(is_space).filter(|w| !w.is_empty()) {
        if !out.is_empty() { out.push(' ') }
        out.push_str(w);
    }
    Cow::Owned(out)
}

/// Every Word Capitalised, the rest in lower case.
fn capitalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.to_lowercase().chars() {
        if start && c.is_alphabetic() { out.extend(c.to_uppercase()) } else { out.push(c) }
        start = is_space(c) || matches!(c, '-' | '\'' | '’' | '(');
    }
    out
}

fn pad_start(s: &str, n: f64, fill: &str, end: bool) -> Result<String, String> {
    let len = len16(s);
    let n = if n.is_nan() || n < 0.0 { 0.0 } else { n.trunc() };
    if n <= len as f64 || fill.is_empty() {
        return Ok(s.to_string());
    }
    if n > MAX_LEN as f64 {
        return Err("Invalid string length".into());
    }
    let need = n as usize - len;
    let fill16: Vec<u16> = fill.encode_utf16().collect();
    let pad: Vec<u16> = fill16.iter().copied().cycle().take(need).collect();
    let pad = String::from_utf16_lossy(&pad);
    Ok(if end { format!("{s}{pad}") } else { format!("{pad}{s}") })
}

/// The longest text a formula may build (V8 stops near 2^29 characters; one cell has no use for that much).
const MAX_LEN: usize = 1 << 24;

/// firstNumber(): the first number as written — a sign only when it starts a word: "68 - Rhin" is 68, "-3,75 €" is -3,75.
fn first_number(s: &str) -> &str {
    let b = s.as_bytes();
    let mut prev_space = true;
    let mut start = None;
    for (i, c) in s.char_indices() {
        let next_digit = b.get(i + 1).is_some_and(u8::is_ascii_digit);
        if c.is_ascii_digit() || (matches!(c, '-' | '+') && next_digit && (i == 0 || prev_space)) {
            start = Some(i);
            break;
        }
        prev_space = is_space(c);
    }
    let Some(a) = start else { return "" };
    let mut i = a + matches!(b[a], b'-' | b'+') as usize;
    while i < b.len() && b[i].is_ascii_digit() { i += 1 }
    if i + 1 < b.len() && matches!(b[i], b'.' | b',') && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() { i += 1 }
    }
    &s[a..i]
}

/// /^[^\s@]+@[^\s@]+\.[^\s@.]{2,}$/
pub fn is_email(s: &str) -> bool {
    let s = trim(s);
    if s.chars().any(is_space) { return false }
    let Some((local, dom)) = s.split_once('@') else { return false };
    if local.is_empty() || dom.contains('@') { return false }
    dom.rfind('.').is_some_and(|p| p >= 1 && dom[p + 1..].chars().count() >= 2)
}

/// isNumericLike: digits with . or , between groups, or with an exponent; sign, spaces and currency aside.
pub fn is_numeric_like(s: &str) -> bool {
    let s = trim(s);
    let mut c: String = s.chars().filter(|&c| !is_space(c)).collect();
    if c.starts_with(['+', '-']) { c.remove(0); }
    c.retain(|x| !matches!(x, '€' | '$' | '£' | '%'));
    let b = c.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() { return false }
    let groups = b.iter().enumerate().all(|(i, &x)| x.is_ascii_digit() || (matches!(x, b'.' | b',') && b.get(i + 1).is_some_and(u8::is_ascii_digit)));
    if groups { return true }
    // \d+(?:[.,]\d+)?[eE][+-]?\d+
    let Some(e) = c.find(['e', 'E']) else { return false };
    let (m, x) = (&c[..e], c[e + 1..].trim_start_matches(['+', '-']));
    let x_ok = !x.is_empty() && x.bytes().all(|d| d.is_ascii_digit()) && c[e + 1..].len() - x.len() <= 1;
    let mut parts = m.splitn(2, ['.', ',']);
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    x_ok && !int.is_empty() && int.bytes().all(|d| d.is_ascii_digit()) && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|d| d.is_ascii_digit()))
}

/// Invisible format characters (\p{Cf}): U+202D… around a number pasted from a contact card.
fn is_format(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{890}' | '\u{891}' | '\u{8e2}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}'
        | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}'
        | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

/// parsePhoneCell: E.164 (+33612345678), a national number (leading 0) taking the country code given.
pub fn parse_phone(s: &str, cc: &str) -> Option<String> {
    let digits = |x: &str| x.bytes().all(|c| c.is_ascii_digit());
    let mut x: String = s.replace("(0)", "").chars().filter(|&c| !(is_space(c) || matches!(c, '.' | '-' | '(' | ')' | '/'))).collect();
    if let Some(r) = x.strip_prefix("00") { x = format!("+{r}") }
    if !x.starts_with('+') {
        let n = x.len();
        let nz = |i: usize| x.as_bytes().get(i).is_some_and(|c| (b'1'..=b'9').contains(c));
        if x.starts_with('0') && (7..=15).contains(&n) && digits(&x) {
            x = format!("+{cc}{}", &x[1..]);
        } else if cc == "33" && n == 9 && nz(0) && digits(&x) {
            x = format!("+33{x}");
        } else if cc == "33" && digits(&x) && x.starts_with("33") && ((n == 11 && nz(2)) || (n == 12 && x.as_bytes()[2] == b'0' && nz(3))) {
            x = format!("+33{}", &x[n - 9..]);
        } else {
            return None;
        }
    }
    if x.len() == 13 && x.starts_with("+330") && x.as_bytes()[4] != b'0' && digits(&x[1..]) {
        x = format!("+33{}", &x[4..]);
    }
    let b = x.as_bytes();
    if !(9..=16).contains(&b.len()) || !(b'1'..=b'9').contains(&b[1]) || !digits(&x[1..]) {
        return None;
    }
    if x.starts_with("+33") && x.len() != 12 {
        return None;
    }
    Some(x)
}

// ---- helpers' coercions -------------------------------------------------------

/// fxNum
pub fn fx_num(v: &Val) -> f64 {
    match v {
        Val::Num(n) => *n,
        v => fx_num_str(&cell_str(v)),
    }
}

/// fxDate: a valid date, or a text read as one.
pub fn fx_date(v: &Val) -> Option<f64> {
    match v {
        Val::Date(t) => (!t.is_nan()).then_some(*t),
        v => date::from_text(trim(&cell_str(v))),
    }
}

/// The numbers among the arguments, empty cells left out (an average of 10 and nothing is 10).
fn fx_nums<'v>(a: &'v [Val<'v>]) -> impl Iterator<Item = f64> + 'v {
    a.iter().filter(|x| matches!(x, Val::Num(_)) || !trim(&cell_str(x)).is_empty()).map(fx_num).filter(|n| !n.is_nan())
}

/// A regex given as such, or made of a text (new RegExp(text, flags)) — compiled once per thread.
pub enum Pat<'a> {
    B(&'a Rx),
    A(Arc<Rx>),
}

impl std::ops::Deref for Pat<'_> {
    type Target = Rx;
    fn deref(&self) -> &Rx {
        match self { Pat::B(r) => r, Pat::A(r) => r }
    }
}

pub fn pattern<'a>(v: &Val<'a>, flags: &'static str, sc: &mut Scratch) -> Result<Pat<'a>, String> {
    if let Val::Re(r) = v {
        return Ok(Pat::B(r));
    }
    let src = if let Val::Undef = v { Cow::Borrowed("") } else { to_str(v) };
    sc.regex(&src, flags).map(Pat::A)
}

fn not_regex(v: &Val, m: &str) -> Result<(), String> {
    if let Val::Re(_) = v { Err(format!("First argument to String.prototype.{m} must not be a regular expression")) } else { Ok(()) }
}

fn nan_or(o: Option<f64>) -> Val<'static> {
    Val::Num(o.unwrap_or(f64::NAN))
}

/// A helper called with its arguments.
pub fn call<'a>(f: Fx, mut a: Args<'a>, cx: &Cx<'a>, sc: &mut Scratch) -> R<'a> {
    let d0 = || fx_date(a.get(0));
    Ok(match f {
        Fx::Num => Val::Num(fx_num(a.get(0))),
        Fx::Date => d0().map_or(Val::Null, Val::Date),
        Fx::Today => Val::Date(cx.today),
        Fx::Days => nan_or(d0().zip(fx_date(a.get(1))).map(|(x, y)| js_round((y - x) / date::DAY))),
        Fx::AddDays => d0().map_or(Val::Null, |t| Val::Date(date::clip(t + fx_num(a.get(1)) * date::DAY))),
        Fx::Year => nan_or(d0().map(|t| date::parts(t).y as f64)),
        Fx::Month => nan_or(d0().map(|t| date::parts(t).mo as f64)),
        Fx::Day => nan_or(d0().map(|t| date::parts(t).d as f64)),
        Fx::Weekday => nan_or(d0().map(|t| match date::parts(t).wd { 0 => 7.0, w => w as f64 })),
        Fx::Week => nan_or(d0().map(date::week)),
        Fx::AddMonths => d0().map_or(Val::Null, |t| Val::Date(date::add_months(t, fx_num(a.get(1))))),
        Fx::Months | Fx::Years => {
            let m = d0().zip(fx_date(a.get(1))).map(|(x, y)| date::months(x, y));
            nan_or(if f == Fx::Years { m.map(|m| (m / 12.0).trunc()) } else { m })
        }
        Fx::FmtDate => match d0() {
            None => Val::str(""),
            Some(t) => match a.get(1) {
                Val::Undef => Val::owned(date::format(t, "dd/mm/yyyy")),
                Val::Str(p) => Val::owned(date::format(t, p)),
                Val::Null => return Err("Cannot read properties of null (reading 'replace')".into()),
                _ => return Err("pattern.replace is not a function".into()),
            },
        },
        Fx::Round => {
            let d = match a.get(1) { Val::Undef => 0.0, v => to_num(v) };
            let k = pow(10.0, d);
            Val::Num(js_round(fx_num(a.get(0)) * k) / k)
        }
        Fx::Fixed => {
            let n = fx_num(a.get(0));
            if n.is_nan() {
                return Ok(Val::str(""));
            }
            let d = match a.get(1) { Val::Undef => 2.0, v => to_int(to_num(v)) };
            let mut s = to_fixed(n, d)?;
            if cx.comma { comma_mark(&mut s, 0) }
            Val::owned(s)
        }
        Fx::Upper => Val::Str(upper(into_cell_str(a.take(0)))),
        Fx::Lower => Val::Str(lower(into_cell_str(a.take(0)))),
        Fx::Trim => Val::Str(clean_spaces(into_cell_str(a.take(0)))),
        Fx::Capitalize => Val::owned(capitalize(&cell_str(a.get(0)))),
        Fx::Slug => Val::owned(slugify(&cell_str(a.get(0)))),
        Fx::Len => Val::Num(len16(&cell_str(a.get(0))) as f64),
        Fx::Left => {
            let end = match a.get(1) { Val::Undef => None, v => Some(to_num(v)) };
            Val::Str(slice16(into_cell_str(a.take(0)), 0.0, end))
        }
        Fx::Right => {
            if compare(a.get(1).clone(), Val::Num(0.0)) != Some(Ordering::Greater) {
                return Ok(Val::str(""));
            }
            let n = to_num(a.get(1));
            Val::Str(slice16(into_cell_str(a.take(0)), -n, None))
        }
        Fx::Mid => {
            // Math.max(0, NaN) is NaN, which slice() reads as 0 and which makes the end NaN too
            let s0 = to_num(a.get(1));
            let start = if s0.is_nan() { s0 } else { (s0 - 1.0).max(0.0) };
            let end = if a.get(2).nullish() { None } else { Some(start + to_num(a.get(2))) };
            Val::Str(slice16(into_cell_str(a.take(0)), start, end))
        }
        Fx::Pad => {
            let fill = match a.get(2) { Val::Undef => Cow::Borrowed("0"), v => to_str(v) };
            Val::owned(pad_start(&cell_str(a.get(0)), to_num(a.get(1)), &fill, false)?)
        }
        Fx::Extract => {
            let g = match a.get(2) { Val::Undef => 1.0, v => to_num(v) };
            let rx = pattern(a.get(1), "", sc)?;
            match rx.match_in(into_cell_str(a.take(0))) {
                Val::List(mut l) => {
                    let pick = if g.fract() == 0.0 && g >= 0.0 && (g as usize) < l.len() && !l[g as usize].nullish() { g as usize } else { 0 };
                    l.swap_remove(pick)
                }
                _ => Val::str(""),
            }
        }
        Fx::Replace => {
            let by = match a.get(2) { Val::Undef => Cow::Borrowed(""), v => Cow::Owned(to_str(v).into_owned()) };
            let rx = pattern(a.get(1), "g", sc)?;
            Val::Str(rx.replace(into_cell_str(a.take(0)), &by))
        }
        Fx::Contains | Fx::StartsWith | Fx::EndsWith => {
            let mut hay = std::mem::take(&mut sc.fold);
            let mut needle = std::mem::take(&mut sc.fold2);
            fold(&cell_str(a.get(0)), &mut hay);
            fold(&cell_str(a.get(1)), &mut needle);
            let r = match f {
                Fx::Contains => hay.contains(needle.as_str()),
                Fx::StartsWith => hay.starts_with(needle.as_str()),
                _ => hay.ends_with(needle.as_str()),
            };
            (sc.fold, sc.fold2) = (hay, needle);
            Val::Bool(r)
        }
        Fx::Empty => Val::Bool(trim(&cell_str(a.get(0))).is_empty()),
        Fx::First => match (0..a.len()).find(|&i| !trim(&cell_str(a.get(i))).is_empty()) {
            Some(i) => a.take(i),
            None => Val::str(""),
        },
        Fx::Join => {
            let sep = match a.get(0) { Val::Undef => Cow::Borrowed(","), v => to_str(v) };
            let mut out = String::new();
            let mut first = true;
            for v in &a.slice()[1.min(a.len())..] {
                let s = cell_str(v);
                if trim(&s).is_empty() { continue }
                if !first { out.push_str(&sep) }
                first = false;
                out.push_str(&s);
            }
            Val::owned(out)
        }
        Fx::Digits => {
            let s = into_cell_str(a.take(0));
            if s.bytes().all(|c| c.is_ascii_digit()) { Val::Str(s) } else { Val::owned(s.chars().filter(char::is_ascii_digit).collect()) }
        }
        Fx::FirstNumber => on_text(into_cell_str(a.take(0)), |s| Val::str(first_number(s))),
        Fx::Abs => Val::Num(fx_num(a.get(0)).abs()),
        Fx::Min | Fx::Max | Fx::Sum | Fx::Avg => {
            let mut it = fx_nums(a.slice());
            match f {
                Fx::Sum => Val::Num(it.sum()),
                Fx::Avg => {
                    let (s, n) = it.fold((0.0, 0), |(s, n), x| (s + x, n + 1));
                    Val::Num(if n == 0 { f64::NAN } else { s / n as f64 })
                }
                _ => {
                    let first = it.next();
                    nan_or(first.map(|x| it.fold(x, |m, y| if (f == Fx::Min) == (y < m) { y } else { m })))
                }
            }
        }
        Fx::Split => {
            let n = match a.get(2) { Val::Undef => 1.0, v => to_num(v) };
            let sep = a.take(1);
            let sep_rx = if let Val::Re(r) = sep { Some(r) } else { None };
            let sep_s = cell_str(&sep).into_owned();
            on_text(into_cell_str(a.take(0)), |s| {
                let mut parts = match sep_rx {
                    Some(r) => r.split(s, usize::MAX),
                    None => jsre::split_str(s, &sep_s, usize::MAX),
                };
                let k = if n < 0.0 { parts.len() as f64 + n } else { n - 1.0 };
                let v = if k >= 0.0 && k.fract() == 0.0 && (k as usize) < parts.len() { parts.swap_remove(k as usize) } else { Val::Undef };
                if v.nullish() { Val::str("") } else { v }
            })
        }
        Fx::Matches => {
            let rx = pattern(a.get(1), "i", sc)?;
            Val::Bool(rx.test(&cell_str(a.get(0))))
        }
        Fx::IsEmail => Val::Bool(is_email(&cell_str(a.get(0)))),
        Fx::IsNumber => Val::Bool(is_numeric_like(&cell_str(a.get(0)))),
        Fx::IsDate => Val::Bool(d0().is_some()),
        Fx::IsPhone => {
            let x: String = cell_str(a.get(0)).chars().filter(|&c| !is_format(c)).collect();
            let x = trim(&x);
            Val::Bool(!x.is_empty() && parse_phone(x, "33").is_some()) // national numbers read as French, like Convert
        }
        Fx::Ifs => {
            let n = a.len();
            let mut i = 0;
            while i + 1 < n {
                if truthy(a.get(i)) { return Ok(a.take(i + 1)) }
                i += 2;
            }
            if n % 2 == 1 { a.take(n - 1) } else { Val::str("") }
        }
        Fx::Cases => {
            let n = a.len();
            let x = cell_str(a.get(0)).into_owned();
            let mut i = 1;
            while i + 1 < n {
                if *cell_str(a.get(i)) == *x { return Ok(a.take(i + 1)) }
                i += 2;
            }
            if n > 1 && (n - 1) % 2 == 1 { a.take(n - 1) } else { Val::str("") }
        }
        Fx::Phonetic => Val::owned(sound::phonetic(&cell_str(a.get(0)))),
        Fx::Similarity => Val::Num(sound::similarity(&cell_str(a.get(0)), &cell_str(a.get(1)))),
        Fx::Uuid => Val::owned(hash::uuid4()),
        Fx::Uuid7 => Val::owned(hash::uuid7()),
        Fx::Md5 | Fx::Sha1 | Fx::Sha256 | Fx::Sha3_256 | Fx::Sha3_512 | Fx::Blake2b | Fx::Blake3 => {
            let s = cell_str(a.get(0));
            if s.is_empty() {
                return Ok(Val::str(""));
            }
            let h = match f {
                Fx::Md5 => hash::Hash::Md5,
                Fx::Sha1 => hash::Hash::Sha1,
                Fx::Sha256 => hash::Hash::Sha256,
                Fx::Sha3_256 => hash::Hash::Sha3_256,
                Fx::Sha3_512 => hash::Hash::Sha3_512,
                Fx::Blake2b => hash::Hash::Blake2b,
                _ => hash::Hash::Blake3,
            };
            Val::owned(hash::hash(h, &s))
        }
    })
}

/// x ** y, Math.pow: NaN for 1 ** ±Infinity and anything ** NaN, where C's pow says 1.
pub fn pow(x: f64, y: f64) -> f64 {
    if y.is_nan() || (x.abs() == 1.0 && y.is_infinite()) {
        return f64::NAN;
    }
    if y.fract() == 0.0 && y.abs() < 1024.0 {
        return x.powi(y as i32);
    }
    x.powf(y)
}

pub fn math<'a>(f: Mf, a: &Args<'a>) -> f64 {
    let n = |i| to_num(a.get(i));
    match f {
        Mf::Abs => n(0).abs(),
        Mf::Ceil => n(0).ceil(),
        Mf::Floor => n(0).floor(),
        Mf::Round => js_round(n(0)),
        Mf::Trunc => n(0).trunc(),
        Mf::Sign => {
            let x = n(0);
            if x.is_nan() || x == 0.0 { x } else { x.signum() }
        }
        Mf::Sqrt => n(0).sqrt(),
        Mf::Cbrt => n(0).cbrt(),
        Mf::Pow => pow(n(0), n(1)),
        Mf::Min | Mf::Max => {
            let mut acc = if f == Mf::Min { f64::INFINITY } else { f64::NEG_INFINITY };
            for v in a.slice() {
                let x = to_num(v);
                if x.is_nan() { return f64::NAN }
                if (f == Mf::Min && (x < acc || (x == 0.0 && x.is_sign_negative()))) || (f == Mf::Max && (x > acc || (x == 0.0 && acc == 0.0 && x.is_sign_positive()))) {
                    acc = x;
                }
            }
            acc
        }
        Mf::Log => n(0).ln(),
        Mf::Log10 => n(0).log10(),
        Mf::Log2 => n(0).log2(),
        Mf::Exp => n(0).exp(),
        Mf::Random => hash::unit(),
    }
}

// ---- methods ------------------------------------------------------------------

/// recv.m(args): JavaScript's own method, or a helper with the value as its first argument.
pub fn method<'a>(recv: Val<'a>, m: Meth, a: Args<'a>, text: &str, cx: &Cx<'a>, sc: &mut Scratch) -> R<'a> {
    let not_fn = || Err(format!("{text}.{} is not a function", m.name()));
    match (&recv, m) {
        (Val::Undef | Val::Null, _) => Err(format!("Cannot read properties of {} (reading '{}')", recv.kind(), m.name())),
        (Val::Str(_), Meth::Fx(f)) if ON_TEXT.contains(&f) => call(f, a.prepend(recv), cx, sc),
        (Val::Num(_), Meth::Fx(f)) if ON_NUMBER.contains(&f) => call(f, a.prepend(recv), cx, sc),
        (Val::Date(_), Meth::Fx(f)) if ON_DATE.contains(&f) => call(f, a.prepend(recv), cx, sc),
        (_, Meth::Fx(_)) => not_fn(),
        (_, Meth::Nat(n)) => native(recv, n, a, sc).unwrap_or_else(not_fn),
    }
}

/// `.length`: of a text or a list; undefined on other values, a TypeError on null.
pub fn length<'a>(v: &Val<'a>) -> R<'a> {
    Ok(match v {
        Val::Str(s) => Val::Num(len16(s) as f64),
        Val::List(l) => Val::Num(l.len() as f64),
        Val::Undef | Val::Null => return Err(format!("Cannot read properties of {} (reading 'length')", v.kind())),
        _ => Val::Undef,
    })
}

/// x[i]: a character of a text or an item of a list, by an integer — anything else is undefined
/// (a name computed at run time, "constructor", never reaches a property).
pub fn at<'a>(x: Val<'a>, i: &Val) -> Val<'a> {
    let Val::Num(i) = *i else { return Val::Undef };
    if i.fract() != 0.0 || i < 0.0 {
        return Val::Undef;
    }
    match x {
        Val::Str(s) => unit_at(s, i as usize).unwrap_or(Val::Undef),
        Val::List(mut l) if (i as usize) < l.len() => l.swap_remove(i as usize),
        _ => Val::Undef,
    }
}

/// The character at UTF-16 index i, borrowed when the text is.
fn unit_at(s: Cow<str>, i: usize) -> Option<Val> {
    if s.is_ascii() {
        return (i < s.len()).then(|| Val::Str(sub(s, i, i + 1)));
    }
    let mut u = 0;
    let mut hit = None;
    for (b, c) in s.char_indices() {
        if u == i { hit = Some((b, b + c.len_utf8())); break }
        u += c.len_utf16();
        if u > i { return Some(Val::str("\u{fffd}")) } // the second half of a surrogate pair
    }
    let (a, b) = hit?;
    Some(Val::Str(sub(s, a, b)))
}

fn unit_code(s: &str, i: f64) -> Option<(u32, Option<u32>)> {
    let i = to_int(i);
    if i < 0.0 { return None }
    let units: Vec<u16> = s.encode_utf16().skip(i as usize).take(2).collect();
    let first = *units.first()? as u32;
    Some((first, units.get(1).map(|&x| x as u32)))
}

fn find16(s: &str, needle: &str, from: usize) -> f64 {
    let b = byte_of(s, from);
    s[b..].find(needle).map_or(-1.0, |k| jsre::u16_index(s, b + k) as f64)
}

fn native<'a>(recv: Val<'a>, n: Nat, mut a: Args<'a>, sc: &mut Scratch) -> Option<R<'a>> {
    let num = |a: &Args, i| to_num(a.get(i));
    let opt = |a: &Args<'a>, i| match a.get(i) { Val::Undef => None, v => Some(to_num(v)) };
    Some(Ok(match recv {
        Val::Str(s) => match n {
            Nat::At => {
                let len = len16(&s) as f64;
                let i = to_int(num(&a, 0));
                let k = if i < 0.0 { len + i } else { i };
                if k < 0.0 || k >= len { Val::Undef } else { unit_at(s, k as usize).unwrap_or(Val::Undef) }
            }
            Nat::CharAt => {
                let i = to_int(num(&a, 0));
                if i < 0.0 { Val::str("") } else { unit_at(s, i as usize).unwrap_or(Val::str("")) }
            }
            Nat::CharCodeAt => nan_or(unit_code(&s, num(&a, 0)).map(|(c, _)| c as f64)),
            Nat::CodePointAt => match unit_code(&s, num(&a, 0)) {
                None => Val::Undef,
                Some((h @ 0xd800..=0xdbff, Some(l @ 0xdc00..=0xdfff))) => Val::Num((((h - 0xd800) << 10) + (l - 0xdc00) + 0x10000) as f64),
                Some((c, _)) => Val::Num(c as f64),
            },
            Nat::EndsWith | Nat::StartsWith | Nat::Includes => {
                if let Err(e) = not_regex(a.get(0), n.name()) { return Some(Err(e)) }
                let needle = to_str(a.get(0));
                let len = len16(&s);
                Val::Bool(match n {
                    Nat::EndsWith => s[..byte_of(&s, opt(&a, 1).map_or(len, |e| rel(e.max(0.0), len)))].ends_with(&*needle),
                    Nat::StartsWith => s[byte_of(&s, opt(&a, 1).map_or(0, |p| rel(p.max(0.0), len)))..].starts_with(&*needle),
                    _ => s[byte_of(&s, opt(&a, 1).map_or(0, |p| rel(p.max(0.0), len)))..].contains(&*needle),
                })
            }
            Nat::IndexOf => {
                let pos = rel(num(&a, 1).max(0.0), len16(&s));
                Val::Num(find16(&s, &to_str(a.get(0)), pos))
            }
            Nat::LastIndexOf => {
                let needle = to_str(a.get(0));
                let p = num(&a, 1);
                let len = len16(&s);
                let pos = if p.is_nan() { len } else { rel(p.max(0.0), len) };
                let end = (byte_of(&s, pos) + needle.len()).min(s.len());
                let end = (0..=end).rev().find(|&e| s.is_char_boundary(e)).unwrap_or(0);
                Val::Num(s[..end].rfind(&*needle).map_or(-1.0, |k| jsre::u16_index(&s, k) as f64))
            }
            Nat::LocaleCompare => {
                let other = to_str(a.get(0));
                let (x, y) = (crate::text::fold(&s), crate::text::fold(&other));
                Val::Num(match x.cmp(&y).then_with(|| cmp_str(&s, &other)) { Ordering::Less => -1.0, Ordering::Equal => 0.0, Ordering::Greater => 1.0 })
            }
            Nat::Match => match pattern(a.get(0), "", sc) {
                Ok(rx) => rx.match_in(s),
                Err(e) => return Some(Err(e)),
            },
            Nat::Search => match pattern(a.get(0), "", sc) {
                Ok(rx) => Val::Num(rx.search(&s)),
                Err(e) => return Some(Err(e)),
            },
            Nat::Normalize => {
                use unicode_normalization::UnicodeNormalization;
                let form = match a.get(0) { Val::Undef => Cow::Borrowed("NFC"), v => to_str(v) };
                Val::owned(match &*form {
                    "NFC" => s.nfc().collect(),
                    "NFD" => s.nfd().collect(),
                    "NFKC" => s.nfkc().collect(),
                    "NFKD" => s.nfkd().collect(),
                    _ => return Some(Err("The normalization form should be one of NFC, NFD, NFKC, NFKD.".into())),
                })
            }
            Nat::PadEnd | Nat::PadStart => {
                let fill = match a.get(1) { Val::Undef => Cow::Borrowed(" "), v => to_str(v) };
                match pad_start(&s, num(&a, 0), &fill, n == Nat::PadEnd) {
                    Ok(p) => Val::owned(p),
                    Err(e) => return Some(Err(e)),
                }
            }
            Nat::Repeat => {
                let k = to_int(num(&a, 0));
                if k < 0.0 || k.is_infinite() { return Some(Err(format!("Invalid count value: {}", num_string(k)))) }
                if k * s.len() as f64 > MAX_LEN as f64 { return Some(Err("Invalid string length".into())) }
                Val::owned(s.repeat(k as usize))
            }
            Nat::Replace | Nat::ReplaceAll => {
                let by = to_str(a.get(1)).into_owned();
                if let Val::Re(r) = a.get(0) {
                    if n == Nat::ReplaceAll && !r.global {
                        return Some(Err("replaceAll must be called with a global RegExp".into()));
                    }
                    return Some(Ok(Val::Str(r.replace(s, &by))));
                }
                let pat = to_str(a.get(0)).into_owned();
                let mut hits = Vec::new();
                if n == Nat::Replace {
                    hits.extend(s.find(&pat));
                } else if pat.is_empty() {
                    hits.extend(s.char_indices().map(|(i, _)| i).chain([s.len()]));
                } else {
                    hits.extend(s.match_indices(&pat).map(|(i, _)| i));
                }
                if hits.is_empty() {
                    return Some(Ok(Val::Str(s)));
                }
                let mut out = String::with_capacity(s.len());
                let mut last = 0;
                for h in hits {
                    out.push_str(&s[last..h]);
                    jsre::expand(&by, &s, h, h + pat.len(), None, &mut out);
                    last = h + pat.len();
                }
                out.push_str(&s[last..]);
                Val::owned(out)
            }
            Nat::Slice => Val::Str(slice16(s, num(&a, 0), opt(&a, 1))),
            Nat::Substring => {
                let len = len16(&s) as f64;
                let clamp = |x: f64| if x.is_nan() { 0.0 } else { x.trunc().clamp(0.0, len) };
                let (x, y) = (clamp(num(&a, 0)), opt(&a, 1).map_or(len, clamp));
                let (x, y) = if x > y { (y, x) } else { (x, y) };
                let (bx, by) = (byte_of(&s, x as usize), byte_of(&s, y as usize));
                Val::Str(sub(s, bx, by))
            }
            Nat::Split => {
                let limit = match a.get(1) { Val::Undef => u32::MAX as usize, v => to_num(v).rem_euclid(4_294_967_296.0) as usize };
                let sep = a.take(0);
                on_text(s, |s| Val::List(match &sep {
                    Val::Undef => if limit == 0 { vec![] } else { vec![Val::str(s)] },
                    Val::Re(r) => r.split(s, limit),
                    v => jsre::split_str(s, &to_str(v), limit),
                }))
            }
            Nat::ToLowerCase | Nat::ToLocaleLowerCase => Val::Str(lower(s)),
            Nat::ToUpperCase | Nat::ToLocaleUpperCase => Val::Str(upper(s)),
            Nat::Trim | Nat::TrimStart | Nat::TrimEnd => {
                let t = match n { Nat::Trim => trim(&s), Nat::TrimStart => s.trim_start_matches(is_space), _ => s.trim_end_matches(is_space) };
                let at = t.as_ptr() as usize - s.as_ptr() as usize;
                let end = at + t.len();
                Val::Str(sub(s, at, end))
            }
            Nat::Concat => {
                let mut out = s.into_owned();
                for v in a.slice() { write_str(v, &mut out) }
                Val::owned(out)
            }
            Nat::ToString => Val::Str(s),
            _ => return None,
        },
        Val::Num(x) => match n {
            Nat::ToFixed => match to_fixed(x, to_int(num(&a, 0))) {
                Ok(s) => Val::owned(s),
                Err(e) => return Some(Err(e)),
            },
            Nat::ToPrecision => match opt(&a, 0) {
                None => Val::owned(num_string(x)),
                Some(p) => match to_precision(x, to_int(p)) {
                    Ok(s) => Val::owned(s),
                    Err(e) => return Some(Err(e)),
                },
            },
            Nat::ToString => {
                let r = opt(&a, 0).map_or(10.0, to_int);
                if !(2.0..=36.0).contains(&r) { return Some(Err("toString() radix must be between 2 and 36".into())) }
                Val::owned(to_radix(x, r as u32))
            }
            _ => return None,
        },
        Val::Bool(b) => match n {
            Nat::ToString => Val::str(if b { "true" } else { "false" }),
            _ => return None,
        },
        Val::Date(tm) => {
            let local = |f: fn(&date::Parts) -> f64| if tm.is_nan() { f64::NAN } else { f(&date::parts(tm + date::local_offset(tm).0)) };
            let utc = |f: fn(&date::Parts) -> f64| if tm.is_nan() { f64::NAN } else { f(&date::parts(tm)) };
            Val::Num(match n {
                Nat::GetTime => tm,
                Nat::GetFullYear => local(|p| p.y as f64),
                Nat::GetMonth => local(|p| p.mo as f64 - 1.0),
                Nat::GetDate => local(|p| p.d as f64),
                Nat::GetDay => local(|p| p.wd as f64),
                Nat::GetHours => local(|p| p.h as f64),
                Nat::GetMinutes => local(|p| p.mi as f64),
                Nat::GetSeconds => local(|p| p.s as f64),
                Nat::GetUTCFullYear => utc(|p| p.y as f64),
                Nat::GetUTCMonth => utc(|p| p.mo as f64 - 1.0),
                Nat::GetUTCDate => utc(|p| p.d as f64),
                Nat::GetUTCDay => utc(|p| p.wd as f64),
                Nat::GetUTCHours => utc(|p| p.h as f64),
                Nat::GetUTCMinutes => utc(|p| p.mi as f64),
                Nat::GetUTCSeconds => utc(|p| p.s as f64),
                Nat::ToISOString => {
                    if tm.is_nan() { return Some(Err("Invalid time value".into())) }
                    let mut s = String::new();
                    date::iso(tm, &mut s);
                    return Some(Ok(Val::owned(s)));
                }
                Nat::ToString => return Some(Ok(Val::owned(to_str(&Val::Date(tm)).into_owned()))),
                _ => return None,
            })
        }
        Val::Re(r) => match n {
            Nat::Test => Val::Bool(r.test(&to_str(a.get(0)))),
            Nat::ToString => Val::owned(to_str(&Val::Re(r)).into_owned()),
            _ => return None,
        },
        Val::List(mut l) => match n {
            Nat::At => {
                let len = l.len() as f64;
                let i = to_int(num(&a, 0));
                let k = if i < 0.0 { len + i } else { i };
                if k < 0.0 || k >= len { Val::Undef } else { l.swap_remove(k as usize) }
            }
            Nat::Includes | Nat::IndexOf => {
                let from = rel(num(&a, 1), l.len());
                let x = a.get(0);
                let hit = l.iter().skip(from).position(|v| if n == Nat::Includes { same_value_zero(v, x) } else { strict_eq(v, x) });
                if n == Nat::Includes { Val::Bool(hit.is_some()) } else { Val::Num(hit.map_or(-1.0, |k| (k + from) as f64)) }
            }
            Nat::LastIndexOf => {
                let len = l.len();
                let to = match a.get(1) { Val::Undef => len as f64 - 1.0, v => { let k = to_int(to_num(v)); if k < 0.0 { len as f64 + k } else { k.min(len as f64 - 1.0) } } };
                let x = a.get(0);
                Val::Num(if to < 0.0 { -1.0 } else { (0..=to as usize).rev().find(|&k| strict_eq(&l[k], x)).map_or(-1.0, |k| k as f64) })
            }
            Nat::Slice => {
                let len = l.len();
                let (x, y) = (rel(num(&a, 0), len), opt(&a, 1).map_or(len, |e| rel(e, len)));
                Val::List(if x < y { l.drain(x..y).collect() } else { Vec::new() })
            }
            Nat::Join | Nat::ToString => {
                let sep = match (n, a.get(0)) { (Nat::Join, Val::Undef) | (Nat::ToString, _) => Cow::Borrowed(","), (_, v) => to_str(v) };
                let mut out = String::new();
                for (i, v) in l.iter().enumerate() {
                    if i > 0 { out.push_str(&sep) }
                    if !v.nullish() { write_str(v, &mut out) }
                }
                Val::owned(out)
            }
            Nat::Concat => {
                for k in 0..a.len() {
                    match a.take(k) {
                        Val::List(x) => l.extend(x),
                        v => l.push(v),
                    }
                }
                Val::List(l)
            }
            Nat::Reverse => {
                l.reverse();
                Val::List(l)
            }
            _ => return None,
        },
        Val::Undef | Val::Null => return None,
    }))
}
