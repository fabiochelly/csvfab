//! Typing a column before it is written to a workbook (xlsx_columns.py).
//!
//! Each column is read whole first: it becomes numeric, percent or date when
//! at least 90 % of its non-empty cells are, and only the cells that fit are
//! converted — the others stay text, as written. Safeguards: an integer with a
//! leading zero (postal code, phone, identifier) or of more than 15 digits
//! (past that, Excel rounds) leaves the whole column as text.
//!
//! The patterns are matched by hand rather than with regexes: this runs on
//! every cell of the file.

use super::{days_from_civil, days_in_month};

const THRESHOLD: f64 = 0.9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColKind {
    Text,
    /// 1 234,5
    Fr,
    /// 1,234.5
    En,
    Pct,
    Ymd,
    Dmy,
    Mdy,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ColumnStat {
    filled: u64,
    fr: u64,
    en: u64,
    comma_dec: u64,
    dot_dec: u64,
    decimals: usize,
    pct: u64,
    pct_dec: usize,
    ymd: u64,
    dmy: u64,
    first_gt12: bool,
    second_gt12: bool,
    time: bool,
    secs: bool,
    lead0: bool,
    long: bool,
    /// In characters.
    pub width: usize,
}

/// The spaces a French number groups its thousands with.
#[inline]
fn is_group_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{202f}')
}

#[inline]
fn chars(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().count() }
}

/// Digits at b[i..]: how many.
#[inline]
fn run(b: &[u8], i: usize) -> usize {
    b[i.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count()
}

fn num(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |n, &c| n.saturating_mul(10).saturating_add((c - b'0') as u32))
}

/// A date's parts as written: (first, second, third) number groups, and the time.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct DateParts {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub h: Option<u32>,
    pub mi: u32,
    pub s: Option<u32>,
}

/// `^(\d{4})-(\d{1,2})-(\d{1,2})(?:[T ](\d{1,2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$`
pub(crate) fn match_ymd(v: &str) -> Option<DateParts> {
    let b = v.as_bytes();
    if run(b, 0) != 4 || b.get(4) != Some(&b'-') { return None; }
    let n2 = run(b, 5);
    if !(1..=2).contains(&n2) || b.get(5 + n2) != Some(&b'-') { return None; }
    let i = 6 + n2;
    let n3 = run(b, i);
    if !(1..=2).contains(&n3) { return None; }
    let mut p = DateParts { a: num(&b[..4]), b: num(&b[5..5 + n2]), c: num(&b[i..i + n3]), ..Default::default() };
    let mut i = i + n3;
    if i == b.len() { return Some(p); }
    if !matches!(b[i], b'T' | b' ') { return None; }
    i += 1;
    i = time(b, i, &mut p)?;
    if b.get(i) == Some(&b':') && run(b, i + 1) >= 2 {
        p.s = Some(num(&b[i + 1..i + 3]));
        i += 3;
        if b.get(i) == Some(&b'.') && run(b, i + 1) > 0 { i += 1 + run(b, i + 1); }
    }
    match b.get(i) {
        Some(b'Z') => i += 1,
        Some(b'+' | b'-') => {
            if run(b, i + 1) < 2 { return None; }
            i += 3;
            if b.get(i) == Some(&b':') { i += 1; }
            if run(b, i) < 2 { return None; }
            i += 2;
        }
        _ => {}
    }
    (i == b.len()).then_some(p)
}

/// `(\d{1,2}):(\d{2})` at b[i..]; returns where it ends.
fn time(b: &[u8], i: usize, p: &mut DateParts) -> Option<usize> {
    let nh = run(b, i);
    if !(1..=2).contains(&nh) || b.get(i + nh) != Some(&b':') || run(b, i + nh + 1) < 2 { return None; }
    p.h = Some(num(&b[i..i + nh]));
    p.mi = num(&b[i + nh + 1..i + nh + 3]);
    Some(i + nh + 3)
}

/// `^(\d{1,2})[/.-](\d{1,2})[/.-](\d{4}|\d{2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?$`
pub(crate) fn match_dmy(v: &str) -> Option<DateParts> {
    let b = v.as_bytes();
    let sep = |c: Option<&u8>| matches!(c, Some(b'/' | b'.' | b'-'));
    let n1 = run(b, 0);
    if !(1..=2).contains(&n1) || !sep(b.get(n1)) { return None; }
    let i = n1 + 1;
    let n2 = run(b, i);
    if !(1..=2).contains(&n2) || !sep(b.get(i + n2)) { return None; }
    let j = i + n2 + 1;
    let n3 = run(b, j);
    if n3 != 4 && n3 != 2 { return None; }
    let mut p = DateParts { a: num(&b[..n1]), b: num(&b[i..i + n2]), c: num(&b[j..j + n3]), ..Default::default() };
    let mut k = j + n3;
    if k == b.len() { return Some(p); }
    if !matches!(b[k], b' ' | b'T') { return None; }
    k = time(b, k + 1, &mut p)?;
    if b.get(k) == Some(&b':') {
        if run(b, k + 1) < 2 { return None; }
        p.s = Some(num(&b[k + 1..k + 3]));
        k += 3;
    }
    (k == b.len()).then_some(p)
}

/// `^(-?\d+(?:[.,]\d+)?)\s?%$`: the number before the `%`.
pub(crate) fn match_pct(v: &str) -> Option<&str> {
    let b = v.as_bytes();
    let mut i = (b.first() == Some(&b'-')) as usize;
    let n = run(b, i);
    if n == 0 { return None; }
    i += n;
    if matches!(b.get(i), Some(b'.' | b',')) && run(b, i + 1) > 0 { i += 1 + run(b, i + 1); }
    let numb = &v[..i];
    let rest = v[i..].strip_suffix('%')?;
    let mut cs = rest.chars();
    match (cs.next(), cs.next()) {
        (None, _) => Some(numb),
        (Some(c), None) if c.is_whitespace() => Some(numb),
        _ => None,
    }
}

/// `^-?(?:\d{1,3}(?:G\d{3})+|\d+)(?:D\d+)?$` with G the group separator, D the
/// decimal one (`group` is a predicate: French numbers group with three kinds of space).
fn match_num(v: &str, group: impl Fn(char) -> bool, dec: u8) -> bool {
    let b = v.as_bytes();
    let mut i = (b.first() == Some(&b'-')) as usize;
    let n = run(b, i);
    if n == 0 { return false; }
    i += n;
    let grouped = v[i..].chars().next().is_some_and(&group);
    if grouped {
        if n > 3 { return false; }
        while let Some(c) = v[i..].chars().next().filter(|&c| group(c)) {
            let j = i + c.len_utf8();
            if run(b, j) != 3 { return false; }
            i = j + 3;
        }
    }
    if b.get(i) == Some(&dec) {
        let n = run(b, i + 1);
        if n == 0 { return false; }
        i += 1 + n;
    }
    i == b.len()
}

pub(crate) fn match_fr(v: &str) -> bool {
    match_num(v, is_group_space, b',')
}

pub(crate) fn match_en(v: &str) -> bool {
    match_num(v, |c| c == ',', b'.')
}

impl ColumnStat {
    pub fn see(&mut self, v: &str) {
        self.width = self.width.max(chars(v));
        let v = v.trim();
        if v.is_empty() { return; }
        self.filled += 1;
        // A text that starts with neither a digit, a sign nor a currency is neither
        // number, percent nor date: names, cities, e-mails — most cells.
        let Some(c0) = v.chars().next() else { return };
        if !matches!(c0, '0'..='9' | '-' | '+' | '€' | '$' | '£' | '.') { return; }
        let b = v.as_bytes();
        if b.iter().all(u8::is_ascii_digit) {
            // A bare integer, the usual case: no pattern to try.
            self.fr += 1;
            self.en += 1;
            if b[0] == b'0' && b.len() > 1 { self.lead0 = true; }
            if b.len() > 15 { self.long = true; }
            return;
        }
        let digits = v.trim_start_matches('-');
        let d = digits.as_bytes();
        if d.first() == Some(&b'0') && d.len() > 1 && d[1].is_ascii_digit() {
            self.lead0 = true; // 007, 06 12 34 56 78, 01000
        }
        if v.contains('/') || digits.contains('-') || v.contains('T') {
            // A date, or nothing: no number holds these.
            if let Some(p) = match_ymd(v) {
                self.ymd += 1;
                self.t(&p);
            } else if let Some(p) = match_dmy(v) {
                self.dmy += 1;
                if p.a > 12 { self.first_gt12 = true; }
                if p.b > 12 { self.second_gt12 = true; }
                self.t(&p);
            }
            return;
        }
        if v.ends_with('%') {
            if let Some(n) = match_pct(v) {
                self.pct += 1;
                if let Some(k) = n.find(['.', ',']) {
                    self.pct_dec = self.pct_dec.max(n.len() - k - 1);
                }
            }
            return;
        }
        if match_fr(v) {
            self.fr += 1;
            if let Some(k) = v.find(',') {
                self.comma_dec += 1;
                self.decimals = self.decimals.max(v.len() - k - 1);
            }
        }
        if match_en(v) {
            self.en += 1;
            if let Some(k) = v.find('.') {
                self.dot_dec += 1;
                self.decimals = self.decimals.max(v.len() - k - 1);
            }
        }
        let int_part = digits.split([',', '.']).next().unwrap_or("");
        if int_part.chars().filter(|&c| !is_group_space(c)).count() > 15 {
            self.long = true;
        }
    }

    fn t(&mut self, p: &DateParts) {
        if p.h.is_some() {
            self.time = true;
            if p.s.is_some_and(|s| s != 0) { self.secs = true; }
        }
    }

    /// Adds what another slice of the rows saw.
    pub fn merge(&mut self, o: &ColumnStat) {
        self.filled += o.filled;
        self.fr += o.fr;
        self.en += o.en;
        self.comma_dec += o.comma_dec;
        self.dot_dec += o.dot_dec;
        self.decimals = self.decimals.max(o.decimals);
        self.pct += o.pct;
        self.pct_dec = self.pct_dec.max(o.pct_dec);
        self.ymd += o.ymd;
        self.dmy += o.dmy;
        self.first_gt12 |= o.first_gt12;
        self.second_gt12 |= o.second_gt12;
        self.time |= o.time;
        self.secs |= o.secs;
        self.lead0 |= o.lead0;
        self.long |= o.long;
        self.width = self.width.max(o.width);
    }

    /// The column's kind and number format (None: General); widens `width` to fit.
    pub fn decide(&mut self) -> (ColKind, Option<String>) {
        let n = self.filled;
        if n == 0 { return (ColKind::Text, None); }
        let need = n as f64 * THRESHOLD;
        if self.ymd as f64 >= need || self.dmy as f64 >= need {
            let iso = self.ymd >= self.dmy;
            let kind = if iso { ColKind::Ymd } else if self.second_gt12 && !self.first_gt12 { ColKind::Mdy } else { ColKind::Dmy };
            let date = match kind { ColKind::Ymd => "yyyy-mm-dd", ColKind::Dmy => "dd/mm/yyyy", _ => "mm/dd/yyyy" };
            let fmt = format!("{date}{}", if self.secs { " hh:mm:ss" } else if self.time { " hh:mm" } else { "" });
            self.width = self.width.max(fmt.len());
            return (kind, Some(fmt));
        }
        if self.lead0 || self.long { return (ColKind::Text, None); }
        if self.pct as f64 >= need {
            let fmt = if self.pct_dec > 0 { format!("0.{}%", "0".repeat(self.pct_dec.min(4))) } else { "0%".into() };
            return (ColKind::Pct, Some(fmt));
        }
        // Decimal comma or point: the convention that explains the most cells.
        let kind = if self.comma_dec > 0 && self.dot_dec == 0 && self.fr as f64 >= need {
            ColKind::Fr
        } else if self.en as f64 >= need && self.en >= self.fr {
            ColKind::En
        } else if self.fr as f64 >= need {
            ColKind::Fr
        } else {
            return (ColKind::Text, None);
        };
        let d = self.decimals.min(6);
        // Integers: General, no thousands separator (an identifier must not read 12,345).
        let fmt = (d > 0).then(|| format!("#,##0.{}", "0".repeat(d)));
        self.width += 1 + if d > 0 { self.width / 3 } else { 0 };
        (kind, fmt)
    }
}

/// Converts dates and numbers once the column is typed.
pub(crate) struct Converter {
    pivot: i64,
    epoch: i64,
}

impl Converter {
    pub fn new() -> Self {
        // Two-digit years: 20xx up to ten years past this one, 19xx beyond — the
        // page's rule (parseDateCell), so 31/12/99 is 1999 on export as on screen.
        Converter { pivot: super::this_year() % 100 + 10, epoch: days_from_civil(1899, 12, 30) }
    }

    /// A cell's Excel value (a date as its serial number), or None: written as text.
    pub fn value(&self, kind: ColKind, v: &str) -> Option<f64> {
        let v = v.trim();
        let x = match kind {
            ColKind::Text => return None,
            ColKind::Fr => {
                if !match_fr(v) { return None; }
                v.chars().filter(|&c| !is_group_space(c)).map(|c| if c == ',' { '.' } else { c }).collect::<String>().parse().ok()?
            }
            ColKind::En => {
                if !match_en(v) { return None; }
                if v.contains(',') { v.replace(',', "").parse().ok()? } else { v.parse().ok()? }
            }
            ColKind::Pct => match_pct(v)?.replace(',', ".").parse::<f64>().ok()? / 100.0,
            ColKind::Ymd | ColKind::Dmy | ColKind::Mdy => {
                let p = if kind == ColKind::Ymd { match_ymd(v)? } else { match_dmy(v)? };
                let (y, mo, d) = match kind {
                    ColKind::Ymd => (p.a, p.b, p.c),
                    ColKind::Dmy => (p.c, p.b, p.a),
                    _ => (p.c, p.a, p.b),
                };
                let mut y = y as i64;
                if y < 100 { y += if y <= self.pivot { 2000 } else { 1900 }; }
                let (h, mi, s) = (p.h.unwrap_or(0), p.mi, p.s.unwrap_or(0));
                if !(1..=9999).contains(&y) || !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || s > 59 {
                    return None;
                }
                (days_from_civil(y, mo, d) - self.epoch) as f64 + (h * 3600 + mi * 60 + s) as f64 / 86400.0
            }
        };
        x.is_finite().then_some(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(values: &[&str]) -> (ColKind, Option<String>) {
        let mut st = ColumnStat::default();
        for v in values { st.see(v); }
        st.decide()
    }

    fn f(s: &str) -> Option<String> {
        Some(s.into())
    }

    #[test]
    fn numbers() {
        assert_eq!(decide(&["1 234,50", "-5,25", "7"]), (ColKind::Fr, f("#,##0.00")));
        assert_eq!(decide(&["1,234.5", "2.25", "3"]), (ColKind::En, f("#,##0.00")));
        // Bare integers: both conventions read them, English wins a tie; General format.
        assert_eq!(decide(&["1", "2", "30"]), (ColKind::En, None));
        assert_eq!(decide(&["1\u{a0}234,5", "1\u{202f}000"]), (ColKind::Fr, f("#,##0.0")));
    }

    #[test]
    fn codes_stay_text() {
        assert_eq!(decide(&["01000", "75001", "13002"]), (ColKind::Text, None));
        assert_eq!(decide(&["1234567890123456", "1"]), (ColKind::Text, None));
        assert_eq!(decide(&["06 12 34 56 78", "07 00 00 00 00"]), (ColKind::Text, None));
        assert_eq!(decide(&["1 234 567 890 123 456,5", "1"]), (ColKind::Text, None));
    }

    #[test]
    fn threshold() {
        let mut v = vec!["1,5"; 9];
        v.push("x");
        assert_eq!(decide(&v).0, ColKind::Fr);
        let mut v = vec!["1,5"; 8];
        v.extend(["x", "y"]);
        assert_eq!(decide(&v).0, ColKind::Text);
        assert_eq!(decide(&["", " ", "1,5"]).0, ColKind::Fr);
    }

    #[test]
    fn dates() {
        assert_eq!(decide(&["15/06/2023", "01/01/2024"]), (ColKind::Dmy, f("dd/mm/yyyy")));
        assert_eq!(decide(&["06/15/2023", "01/01/2024"]), (ColKind::Mdy, f("mm/dd/yyyy")));
        assert_eq!(decide(&["2023-06-15", "2024-01-01"]), (ColKind::Ymd, f("yyyy-mm-dd")));
        assert_eq!(decide(&["2023-06-15 10:30", "2024-01-01"]), (ColKind::Ymd, f("yyyy-mm-dd hh:mm")));
        assert_eq!(decide(&["2023-06-15T10:30:15Z"]), (ColKind::Ymd, f("yyyy-mm-dd hh:mm:ss")));
        assert_eq!(decide(&["2023-06-15T10:30:00.5+02:00"]), (ColKind::Ymd, f("yyyy-mm-dd hh:mm")));
    }

    #[test]
    fn percent() {
        assert_eq!(decide(&["12%", "7,5 %"]), (ColKind::Pct, f("0.0%")));
        assert_eq!(decide(&["12%", "100%"]), (ColKind::Pct, f("0%")));
    }

    #[test]
    fn patterns() {
        assert!(match_ymd("2023-6-5").is_some());
        assert!(match_ymd("2023-06-15T10:30:00+0200").is_some());
        assert!(match_ymd("2023-06-15+02:00").is_none());     // a zone only after a time
        assert!(match_ymd("2023-06-155").is_none());
        assert!(match_ymd("2023-06-15 10:305").is_none());
        assert!(match_dmy("1/2/23").is_some());
        assert!(match_dmy("15/06-2023 8:05:09").is_some());
        assert!(match_dmy("1/2/123").is_none());
        assert!(match_dmy("1/2/20234").is_none());
        assert_eq!(match_pct("-7,5 %"), Some("-7,5"));
        assert_eq!(match_pct("7.%"), None);
        assert!(match_fr("-1 234 567,89") && match_fr("1234,5") && !match_fr("1234 567") && !match_fr("1 2345"));
        assert!(match_en("1,234,567.5") && match_en("12.5") && !match_en("12,34") && !match_en("1234,567"));
    }

    #[test]
    fn excel_values() {
        let c = Converter::new();
        assert_eq!(c.value(ColKind::Fr, "1 234,5"), Some(1234.5));
        assert_eq!(c.value(ColKind::Fr, "1\u{a0}234,5"), Some(1234.5));
        assert_eq!(c.value(ColKind::En, "1,234.5"), Some(1234.5));
        assert_eq!(c.value(ColKind::Fr, "abc"), None);
        assert_eq!(c.value(ColKind::Pct, "12,5%"), Some(0.125));
        assert_eq!(c.value(ColKind::Dmy, "15/06/2023"), Some(45092.0));
        assert_eq!(c.value(ColKind::Mdy, "06/15/2023"), Some(45092.0));
        assert_eq!(c.value(ColKind::Dmy, "15/06/23"), Some(45092.0));
        assert_eq!(c.value(ColKind::Ymd, "2023-06-15T12:00:00"), Some(45092.5));
        assert_eq!(c.value(ColKind::Dmy, "31/02/2023"), None);
        assert_eq!(c.value(ColKind::Ymd, "2023-06-15T10:30:00Z"), Some(45092.4375));
    }

    #[test]
    fn two_digit_years_pivot_like_the_page() {
        let c = Converter::new();
        let pivot = super::super::this_year() % 100 + 10;
        assert_eq!(c.value(ColKind::Dmy, "31/12/99"), Some(36525.0));
        assert_eq!(c.value(ColKind::Dmy, &format!("01/01/{pivot:02}")), c.value(ColKind::Ymd, &format!("20{pivot:02}-01-01")));
        if pivot < 99 {
            assert_eq!(c.value(ColKind::Dmy, &format!("01/01/{:02}", pivot + 1)), c.value(ColKind::Ymd, &format!("19{:02}-01-01", pivot + 1)));
        }
    }
}
