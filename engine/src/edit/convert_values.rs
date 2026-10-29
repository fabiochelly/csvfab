//! Convert one column of the rows shown to one format: dates, numbers or
//! phone numbers. Done on the text, never through a float, so no digit is
//! lost (unless a number of decimals is imposed). A value not recognised is
//! left as it is, counted, and the rows holding one can be listed.

use super::series::{civil_from_days, last_day};
use super::{fmt_int, is_cf, js_space, js_trim, not_saved, par_chunks, slugify, to_fixed, CellOp, Outcome, Scan};
use crate::table::{RowId, State};
use std::borrow::Cow;
use std::fmt::Write;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOut {
    /// 2024-06-15
    Iso,
    /// 15/06/2024
    Dmy,
    /// 06/15/2024
    Mdy,
    /// 15.06.2024
    DmyDot,
    /// 15-06-2024
    DmyDash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumOut {
    /// 1234.5
    Point,
    /// 1234,5
    Comma,
    /// 1 234,5
    Fr,
    /// 1,234.5
    En,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneOut {
    /// +33612345678
    E164,
    /// +33 6 12 34 56 78
    Intl,
    /// 06 12 34 56 78
    Nat,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Convert {
    /// `month_first`: how 01/02/2024 reads (see `guess`).
    Date { month_first: bool, out: DateOut },
    /// `dec`: the decimal separator of the column, ',' or '.' (see `guess`);
    /// `decimals`: None keeps them as they are.
    Number { dec: char, out: NumOut, decimals: Option<usize> },
    /// `cc`: the country code given to national numbers (leading 0), as typed
    /// (non-digits are dropped; the dialog's default is "33").
    Phone { cc: String, out: PhoneOut },
}

/// A conversion, ready to run over cells.
pub struct Converter {
    kind: Convert,
    cc: String,
    /// Two-digit years up to this one are 20yy, the others 19yy.
    pivot: i64,
}

fn this_year() -> i64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_from_days((secs / 86400) as i64).0
}

impl Converter {
    pub fn new(kind: Convert) -> Converter {
        Converter::in_year(kind, this_year())
    }

    /// As of `year`: a two-digit year up to ten years ahead is this century,
    /// the rest the last one (birth dates).
    pub fn in_year(kind: Convert, year: i64) -> Converter {
        let cc = match &kind { Convert::Phone { cc, .. } => cc.chars().filter(char::is_ascii_digit).collect(), _ => String::new() };
        Converter { kind, cc, pivot: year % 100 + 10 }
    }

    /// The value converted into `out`; false when not recognised.
    pub fn convert(&self, v: &str, out: &mut String) -> bool {
        let v = clean(v);
        match &self.kind {
            Convert::Date { month_first, out: fmt } => {
                let Some(p) = parse_date(&v, *month_first, self.pivot) else { return false };
                let _ = match fmt {
                    DateOut::Iso => write!(out, "{}-{:02}-{:02}", p.y, p.mo, p.d),
                    DateOut::Dmy => write!(out, "{:02}/{:02}/{}", p.d, p.mo, p.y),
                    DateOut::Mdy => write!(out, "{:02}/{:02}/{}", p.mo, p.d, p.y),
                    DateOut::DmyDot => write!(out, "{:02}.{:02}.{}", p.d, p.mo, p.y),
                    DateOut::DmyDash => write!(out, "{:02}-{:02}-{}", p.d, p.mo, p.y),
                };
                // A time stays; its T becomes a space but in ISO.
                match p.rest.strip_prefix('T') {
                    Some(r) if *fmt != DateOut::Iso => { out.push(' '); out.push_str(r) }
                    _ => out.push_str(p.rest),
                }
                true
            }
            Convert::Number { dec, out: fmt, decimals } => match parse_number(&v, *dec) {
                Some(p) => { number_out(&p, *fmt, *decimals, out); true }
                None => false,
            },
            Convert::Phone { out: fmt, .. } => match parse_phone(&v, &self.cc) {
                Some(e) => { phone_out(&e, *fmt, &self.cc, out); true }
                None => false,
            },
        }
    }

    pub fn recognises(&self, v: &str) -> bool {
        self.convert(v, &mut String::new())
    }
}

impl CellOp for Converter {
    type Scratch = String;

    fn cell(&self, v: &str, _: usize, buf: &mut String) -> Outcome {
        if js_trim(v).is_empty() {
            return Outcome::Skip;
        }
        buf.clear();
        if !self.convert(v, buf) {
            Outcome::Bad
        } else if buf == v {
            Outcome::Same
        } else {
            Outcome::Change(buf.clone())
        }
    }
}

/// Invisible format characters (U+202D/U+202C around a number pasted from a
/// phone's contact card, zero-width spaces) are dropped before reading.
fn clean(v: &str) -> Cow<'_, str> {
    if v.is_ascii() || !v.chars().any(is_cf) {
        return Cow::Borrowed(js_trim(v));
    }
    let s: String = v.chars().filter(|&c| !is_cf(c)).collect();
    Cow::Owned(js_trim(&s).to_string())
}

fn digits_at(b: &[u8], i: usize) -> usize {
    b.get(i..).map_or(0, |s| s.iter().take_while(|c| c.is_ascii_digit()).count())
}

struct Ymd<'a> {
    y: i64,
    mo: u32,
    d: u32,
    rest: &'a str,
}

/// `^(\d{1,4})[-/.](\d{1,2})[-/.](\d{1,4})((?:[ T]\d{1,2}:\d{2}.*)?)$`: year
/// first (4 digits), else day first or, `month_first`, month first.
fn parse_date(s: &str, month_first: bool, pivot: i64) -> Option<Ymd<'_>> {
    let b = s.as_bytes();
    let sep = |i: usize| matches!(b.get(i), Some(b'-' | b'/' | b'.'));
    let la = digits_at(b, 0);
    if !(1..=4).contains(&la) || !sep(la) { return None; }
    let lb = digits_at(b, la + 1);
    if !(1..=2).contains(&lb) || !sep(la + 1 + lb) { return None; }
    let pc = la + lb + 2;
    let lc = digits_at(b, pc);
    if !(1..=4).contains(&lc) { return None; }
    let rest = &s[pc + lc..];
    if !rest.is_empty() {
        let r = rest.as_bytes();
        let lh = digits_at(r, 1);
        let ok = matches!(r[0], b' ' | b'T') && (1..=2).contains(&lh) && r.get(1 + lh) == Some(&b':')
            && r.len() >= lh + 4 && r[lh + 2].is_ascii_digit() && r[lh + 3].is_ascii_digit()
            && !rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']);
        if !ok { return None; }
    }
    let n = |i: usize, l: usize| s[i..i + l].parse::<i64>().unwrap();
    let (a, bv, c) = (n(0, la), n(la + 1, lb), n(pc, lc));
    let (mut y, mo, d) = if la == 4 {
        if lc > 2 { return None; }
        (a, bv, c)
    } else {
        if la > 2 || (lc != 2 && lc != 4) { return None; }
        if month_first { (c, a, bv) } else { (c, bv, a) }
    };
    if la != 4 && lc == 2 {
        y += if y <= pivot { 2000 } else { 1900 };
    }
    if !(1..=12).contains(&mo) || d < 1 || d > last_day(y, mo as u32) as i64 {
        return None;
    }
    Some(Ymd { y, mo: mo as u32, d: d as u32, rest })
}

#[derive(Debug, PartialEq)]
struct Num<'a> {
    pre: &'a str,
    neg: bool,
    int: Cow<'a, str>,
    frac: Option<&'a str>,
    post: &'a str,
}

/// Sign, integer digits, fraction digits, and a currency or % kept around
/// them: `^([€$£]\s?)?([+-])?\s*(\d[\d\s.,']*?)(\s?(?:[€$£%]|EUR|USD))?$`.
fn parse_number(s: &str, dec: char) -> Option<Num<'_>> {
    let mut i = 0;
    if let Some(c) = s.chars().next().filter(|c| matches!(c, '€' | '$' | '£')) {
        i = c.len_utf8();
        if let Some(w) = s[i..].chars().next().filter(|&w| js_space(w)) {
            i += w.len_utf8();
        }
    }
    let pre = &s[..i];
    let neg = s[i..].starts_with('-');
    if s[i..].starts_with(['+', '-']) {
        i += 1;
    }
    i = s.len() - s[i..].trim_start_matches(js_space).len();
    let tail = &s[i..];
    if !tail.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let in_body = |c: char| c.is_ascii_digit() || js_space(c) || matches!(c, '.' | ',' | '\'');
    // The body is lazy: the shortest that leaves a currency (with one space before it) or nothing.
    let (body, post) = if tail.chars().all(in_body) {
        (tail, "")
    } else {
        let x = ["€", "$", "£", "%", "EUR", "USD"].iter().find(|x| tail.ends_with(**x))?;
        let pe = tail.len() - x.len();
        let ws = tail[..pe].chars().next_back().filter(|&c| js_space(c)).map_or(0, char::len_utf8);
        if ws > 0 && pe - ws > 0 && tail[..pe - ws].chars().all(in_body) {
            (&tail[..pe - ws], &tail[pe - ws..])
        } else if tail[..pe].chars().all(in_body) {
            (&tail[..pe], &tail[pe..])
        } else {
            return None;
        }
    };
    let mut parts = body.split(dec);
    let int = parts.next().unwrap();
    let frac = parts.next();
    if parts.next().is_some() || frac.is_some_and(|f| f.is_empty() || !f.bytes().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let int: Cow<str> = if int.bytes().all(|c| c.is_ascii_digit()) {
        Cow::Borrowed(int)
    } else {
        // Only well-formed thousands groups: 1 234 567, 1'234, 1.234 (decimal comma).
        let other = if dec == ',' { '.' } else { ',' };
        let gs: Vec<&str> = int.split(|c: char| js_space(c) || c == '\'' || c == other).collect();
        let ok = gs.len() > 1 && (1..=3).contains(&gs[0].len()) && gs[1..].iter().all(|g| g.len() == 3)
            && gs.iter().all(|g| g.bytes().all(|c| c.is_ascii_digit()));
        if !ok { return None; }
        Cow::Owned(gs.concat())
    };
    // 0612…, 01000: a code, not a quantity.
    if int.len() > 1 && int.starts_with('0') {
        return None;
    }
    Some(Num { pre, neg, int, frac, post })
}

fn number_out(p: &Num, fmt: NumOut, decimals: Option<usize>, out: &mut String) {
    let fixed;
    let (int, frac) = match decimals {
        Some(n) => {
            let x: f64 = format!("{}.{}", p.int, p.frac.unwrap_or("0")).parse().unwrap_or(0.0);
            fixed = to_fixed(x, n);
            match fixed.split_once('.') { Some((i, f)) => (i, Some(f)), None => (&fixed[..], None) }
        }
        None => (&p.int[..], p.frac),
    };
    let (grp, dec) = match fmt { NumOut::Point => ("", '.'), NumOut::Comma => ("", ','), NumOut::Fr => (" ", ','), NumOut::En => (",", '.') };
    out.push_str(p.pre);
    if p.neg {
        out.push('-');
    }
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push_str(grp);
        }
        out.push(c);
    }
    if let Some(f) = frac.filter(|f| !f.is_empty()) {
        out.push(dec);
        out.push_str(f);
    }
    out.push_str(p.post);
}

/// Which character is the decimal separator in these values: one followed by
/// other than three digits is, the last of two different ones is, a repeated
/// one is not. A tie goes to the comma.
pub fn guess_decimal<S: AsRef<str>>(values: &[S]) -> char {
    let (mut comma, mut point) = (0, 0);
    for v in values {
        let s: String = v.as_ref().chars().filter(|&c| !js_space(c)).collect();
        let (c, p) = (s.matches(',').count(), s.matches('.').count());
        // A separator followed by exactly three digits: a thousands one, maybe.
        let three = |sep: char| s.match_indices(sep).any(|(i, _)| digits_at(s.as_bytes(), i + 1) == 3);
        if c > 0 && p > 0 {
            if s.rfind(',') > s.rfind('.') { comma += 1 } else { point += 1 }
        } else if c > 1 {
            point += 1;
        } else if p > 1 {
            comma += 1;
        } else if c == 1 && !three(',') {
            comma += 1;
        } else if p == 1 && !three('.') {
            point += 1;
        }
    }
    if point > comma { '.' } else { ',' }
}

/// E.164 (+33612345678). A national number (leading 0) takes the country code
/// given; with 33, a French mobile whose 0 a spreadsheet ate (612345678) and
/// 33612345678 without its + are recognised too.
fn parse_phone(s: &str, cc: &str) -> Option<String> {
    let mut x: String = s.replace("(0)", "").chars().filter(|&c| !(js_space(c) || matches!(c, '.' | '-' | '(' | ')' | '/'))).collect();
    if let Some(r) = x.strip_prefix("00") {
        x = format!("+{r}");
    }
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    let fr9 = |s: &str| s.len() == 9 && digits(s) && !s.starts_with('0');
    if !x.starts_with('+') {
        if cc.is_empty() {
            return None;
        }
        x = if x.starts_with('0') && (7..=15).contains(&x.len()) && digits(&x) {
            format!("+{cc}{}", &x[1..])
        } else if cc == "33" && fr9(&x) {
            format!("+33{x}")
        } else if cc == "33" && x.starts_with("33") && (fr9(&x[2..]) || (x[2..].starts_with('0') && fr9(&x[3..]))) {
            format!("+33{}", &x[x.len() - 9..])
        } else {
            return None;
        };
    }
    // +33 06…: the national 0 kept after the code.
    if x.len() == 13 && x.starts_with("+330") && fr9(&x[4..]) {
        x = format!("+33{}", &x[4..]);
    }
    let n = &x[1..];
    if !(8..=15).contains(&n.len()) || !digits(n) || n.starts_with('0') || (x.starts_with("+33") && x.len() != 12) {
        return None;
    }
    Some(x)
}

/// "612345678" → "61 23 45 67 8": a space after each pair followed by a digit.
fn pairs(s: &str, out: &mut String) {
    for (i, c) in s.chars().enumerate() {
        if i > 0 && i % 2 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
}

fn phone_out(e: &str, fmt: PhoneOut, cc: &str, out: &mut String) {
    let fr = e.starts_with("+33") && e.len() == 12;
    match fmt {
        PhoneOut::Intl if fr => {
            out.push_str("+33 ");
            out.push_str(&e[3..4]);
            out.push(' ');
            pairs(&e[4..], out);
        }
        PhoneOut::Nat if !cc.is_empty() && e[1..].starts_with(cc) => {
            let n = format!("0{}", &e[1 + cc.len()..]);
            if fr { pairs(&n, out) } else { out.push_str(&n) }
        }
        _ => out.push_str(e),
    }
}

/* ---- The dialog ----------------------------------------------------- */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Date,
    Number,
    Phone,
}

/// What a column seems to hold, and how it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Guess {
    pub kind: Kind,
    pub month_first: bool,
    pub dec: char,
    /// The line under the date order.
    pub note: String,
}

/// From the first 2 000 non-empty values of the rows shown: the kind that
/// recognises most of them (a phone-like title breaking ties), the order of
/// day and month, the decimal separator. `cc`: the country code field.
pub fn guess(st: &State, ids: &[RowId], col: usize, title: &str, cc: &str) -> Guess {
    let mut vals: Vec<String> = Vec::new();
    let mut tmp = Vec::new();
    for &id in ids {
        st.with_cell(id, col, &mut tmp, |v| {
            let c = clean(v);
            if !c.is_empty() {
                vals.push(c.into_owned());
            }
        });
        if vals.len() >= 2000 {
            break;
        }
    }
    let (mut dm, mut md) = (0, 0);
    for v in &vals {
        let b = v.as_bytes();
        let la = digits_at(b, 0);
        let sep = |i: usize| matches!(b.get(i), Some(b'-' | b'/' | b'.'));
        if !(1..=2).contains(&la) || !sep(la) { continue; }
        let lb = digits_at(b, la + 1);
        if !(1..=2).contains(&lb) || !sep(la + 1 + lb) || digits_at(b, la + lb + 2) < 2 { continue; }
        dm += (v[..la].parse::<u32>().unwrap() > 12) as usize;
        md += (v[la + 1..la + 1 + lb].parse::<u32>().unwrap() > 12) as usize;
    }
    let month_first = md > 0 && dm == 0;
    let dec = guess_decimal(&vals);
    let cc: String = cc.chars().filter(char::is_ascii_digit).collect();
    let cc = if cc.is_empty() { "33".to_string() } else { cc };
    let note = if dm > 0 || md > 0 {
        format!("Detected: {} first ({} values could only be read that way).", if month_first { "month" } else { "day" }, fmt_int(if month_first { md } else { dm }))
    } else {
        "No value tells day from month here: day first is assumed.".into()
    };
    let slug = slugify(title).replace("number", " ");
    let phone_title = ["tel", "phone", "mobile", "portable", "fax", "gsm", "cell"].iter().any(|w| {
        slug.match_indices(w).any(|(i, _)| i == 0 || !slug.as_bytes()[i - 1].is_ascii_alphanumeric())
    });
    let y = this_year() % 100 + 10;
    let date = vals.iter().filter(|v| parse_date(v, month_first, y).is_some()).count() as f64;
    let num = vals.iter().filter(|v| parse_number(v, dec).is_some()).count() as f64;
    let phone = vals.iter().filter(|v| parse_phone(v, &cc).is_some()).count() as f64 + if phone_title { 0.5 } else { 0.0 };
    // The kind that reads most values, the first one listed on a tie.
    let mut best = (Kind::Date, date);
    for (k, n) in [(Kind::Number, num), (Kind::Phone, phone)] {
        if n > best.1 {
            best = (k, n);
        }
    }
    let kind = if phone_title && phone >= num { Kind::Phone } else { best.0 };
    Guess { kind, month_first, dec, note }
}

/// The dialog's summary of a scan (a preview): what would change, what is
/// already right, what is not recognised (a second line).
pub fn convert_note(s: &Scan, shown: usize, filtered: bool) -> String {
    let mut out = if s.changed > 0 { format!("{} cells will change", fmt_int(s.changed)) } else { "No cell would change".into() };
    if s.same > 0 {
        let _ = write!(out, " · {} already in this format", fmt_int(s.same));
    }
    let _ = write!(out, " — in the {} {}rows.", fmt_int(shown), if filtered { "filtered " } else { "" });
    if s.bad > 0 {
        let eg: Vec<String> = s.bad_examples.iter().take(3).map(|v| {
            if v.chars().count() > 30 { format!("\"{}…\"", v.chars().take(29).collect::<String>()) } else { format!("\"{v}\"") }
        }).collect();
        let _ = write!(out, "\n{} cells not recognised, left as they are: {}{}", fmt_int(s.bad), eg.join(", "), if s.bad_examples.len() > 3 { "…" } else { "" });
    }
    out
}

pub const NOTHING_TO_CONVERT: &str = "Nothing to convert with these settings.";

pub fn convert_done(changed: usize, title: &str, bad: usize) -> String {
    let bad = if bad > 0 { format!(", {} not recognised", fmt_int(bad)) } else { String::new() };
    not_saved(&format!("{} cells converted in {title}{bad}", fmt_int(changed)))
}

/// "Show them": the rows among `ids` whose cell is not empty and not recognised.
pub fn unrecognised(st: &State, ids: &[RowId], col: usize, conv: &Converter, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, String)| {
        ids[r].iter().copied().filter(|&id| st.with_cell(id, col, &mut s.0, |v| {
            s.1.clear();
            !js_trim(v).is_empty() && !conv.convert(v, &mut s.1)
        })).collect::<Vec<_>>()
    })?;
    Some(parts.concat())
}

pub fn unrecognised_note(rows: usize, title: &str) -> String {
    format!("{} rows whose \"{title}\" was not recognised — clear the filter from the column's ▾.", fmt_int(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::tests::{table, NO};
    use crate::edit::{all_rows, scan};

    fn conv(kind: Convert, v: &str) -> Option<String> {
        let mut s = String::new();
        Converter::in_year(kind, 2026).convert(v, &mut s).then_some(s)
    }

    fn date(v: &str, mf: bool, out: DateOut) -> Option<String> {
        conv(Convert::Date { month_first: mf, out }, v)
    }

    fn num(v: &str, dec: char, out: NumOut, d: Option<usize>) -> Option<String> {
        conv(Convert::Number { dec, out, decimals: d }, v)
    }

    fn phone(v: &str, out: PhoneOut) -> Option<String> {
        conv(Convert::Phone { cc: "+33".into(), out }, v)
    }

    #[test]
    fn dates() {
        assert_eq!(date("15/06/2024", false, DateOut::Iso).as_deref(), Some("2024-06-15"));
        assert_eq!(date("06/15/2024", true, DateOut::Dmy).as_deref(), Some("15/06/2024"));
        assert_eq!(date("2024-6-5T10:20:00Z", false, DateOut::DmyDot).as_deref(), Some("05.06.2024 10:20:00Z"));
        assert_eq!(date("2024-6-5T10:20", false, DateOut::Iso).as_deref(), Some("2024-06-05T10:20"));
        assert_eq!(date("1/2/36", false, DateOut::Iso).as_deref(), Some("2036-02-01"));
        assert_eq!(date("1/2/37", false, DateOut::DmyDash).as_deref(), Some("01-02-1937"));
        assert_eq!(date("\u{202d}31.12.2023\u{202c} ", false, DateOut::Mdy).as_deref(), Some("12/31/2023"));
        assert_eq!(date("29/02/2023", false, DateOut::Iso), None);
        assert_eq!(date("2024-01-2024", false, DateOut::Iso), None);
        assert_eq!(date("15/06/2024 noon", false, DateOut::Iso), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(num("1 234,50 €", ',', NumOut::Point, None).as_deref(), Some("1234.50 €"));
        assert_eq!(num("€ -1.234.567,5", ',', NumOut::En, None).as_deref(), Some("€ -1,234,567.5"));
        assert_eq!(num("1,234.5", '.', NumOut::Fr, Some(2)).as_deref(), Some("1 234,50"));
        assert_eq!(num("12,345", ',', NumOut::Point, Some(2)).as_deref(), Some("12.35"));
        assert_eq!(num("42%", '.', NumOut::Comma, None).as_deref(), Some("42%"));
        assert_eq!(num("1'000 USD", '.', NumOut::En, None).as_deref(), Some("1,000 USD"));
        assert_eq!(num("0612", ',', NumOut::Point, None), None);
        assert_eq!(num("1,2,3", ',', NumOut::Point, None), None);
        assert_eq!(num("12 34", ',', NumOut::Point, None), None);
        assert_eq!(num("abc", ',', NumOut::Point, None), None);
        assert_eq!(num("0,5", ',', NumOut::Point, Some(0)).as_deref(), Some("1"));
        assert_eq!(guess_decimal(&["1,5", "2,25", "1.000"]), ',');
        assert_eq!(guess_decimal(&["1,234", "2.5", "3.25"]), '.');
        assert_eq!(guess_decimal(&["1,234"]), ',');
    }

    #[test]
    fn phones() {
        assert_eq!(phone("06 12 34 56 78", PhoneOut::E164).as_deref(), Some("+33612345678"));
        assert_eq!(phone("612345678", PhoneOut::Intl).as_deref(), Some("+33 6 12 34 56 78"));
        assert_eq!(phone("33612345678", PhoneOut::Nat).as_deref(), Some("06 12 34 56 78"));
        assert_eq!(phone("+33 (0)6 12 34 56 78", PhoneOut::E164).as_deref(), Some("+33612345678"));
        assert_eq!(phone("0033 6.12.34.56.78", PhoneOut::E164).as_deref(), Some("+33612345678"));
        assert_eq!(phone("+1 (203) 893-3004", PhoneOut::Nat).as_deref(), Some("+12038933004"));
        assert_eq!(phone("+33 6 12", PhoneOut::E164), None);
        assert_eq!(phone("12345", PhoneOut::E164), None);
        assert_eq!(conv(Convert::Phone { cc: "".into(), out: PhoneOut::E164 }, "0612345678"), None);
        assert_eq!(conv(Convert::Phone { cc: "44".into(), out: PhoneOut::Nat }, "020 7946 0018").as_deref(), Some("02079460018"));
    }

    #[test]
    fn column_scan_and_guess() {
        let t = table("Téléphone;when;amount\n06 12 34 56 78;12/31/2024;1 234,5\n+33612345678;01/02/2024;2,5\nn/a;;1.000,25\n");
        let ids = all_rows(&t.st);
        let g = guess(&t.st, &ids, 0, "Téléphone", "");
        assert_eq!(g.kind, Kind::Phone);
        let g = guess(&t.st, &ids, 1, "when", "33");
        assert_eq!((g.kind, g.month_first), (Kind::Date, true));
        assert_eq!(g.note, "Detected: month first (1 values could only be read that way).");
        let g = guess(&t.st, &ids, 2, "amount", "33");
        assert_eq!((g.kind, g.dec), (Kind::Number, ','));

        let c = Converter::new(Convert::Phone { cc: "33".into(), out: PhoneOut::E164 });
        let s = scan(&t.st, &ids, &[0], &c, false, &NO).unwrap();
        assert_eq!((s.changed, s.same, s.bad), (1, 1, 1));
        assert_eq!(convert_note(&s, 3, false), "1 cells will change · 1 already in this format — in the 3 rows.\n1 cells not recognised, left as they are: \"n/a\"");
        assert_eq!(unrecognised(&t.st, &ids, 0, &c, &NO).unwrap(), vec![ids[2]]);
    }
}
