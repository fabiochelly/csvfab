//! Series: what the fill handle, Ctrl+D and Ctrl+Enter write. From seed
//! cells at k = 0…n−1, the value at any k (a fill upwards uses k < 0):
//! numbers (step = the seeds' trend, one seed: +1; decimals, decimal comma,
//! grouping and leading zeros kept), dates (by day, month or year, in the
//! seeds' own format), French and English day and month names, text ending
//! in a number ("Lot 7", "A007"). Anything else — or a copy — repeats the
//! seeds in a loop. Also: fill the empty cells from the value above.

use super::{fmt_int, js_num, js_round, js_space, js_trim, not_saved, par_chunks, round_digits, to_fixed, Change};
use crate::table::{RowId, State};
use std::fmt::Write;
use std::sync::atomic::AtomicBool;

/* ---- Calendar ------------------------------------------------------ */

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m as i64 + 9) % 12) + 2) / 5 + d as i64 - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub(crate) fn last_day(y: i64, m: u32) -> u32 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/* ---- Numbers as the web app reads them ----------------------------- */

/// `isNumericLike`: digits with separators, or a mantissa and an exponent,
/// once spaces, a sign and currency or % signs are set aside.
pub(crate) fn numeric_like(s: &str) -> bool {
    let s = js_trim(s);
    let mut c: String = s.chars().filter(|&ch| !js_space(ch)).collect();
    if c.starts_with(['+', '-']) {
        c.remove(0);
    }
    c.retain(|ch| !matches!(ch, '€' | '$' | '£' | '%'));
    let b = c.as_bytes();
    if b.is_empty() {
        return false;
    }
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    // ^\d+(?:[.,]\d+)*$
    let mut i = 0;
    let mut ok = digits(&mut i);
    while ok && i < b.len() && matches!(b[i], b'.' | b',') {
        i += 1;
        ok = digits(&mut i);
    }
    if ok && i == b.len() {
        return true;
    }
    // ^\d+(?:[.,]\d+)?[eE][+-]?\d+$
    let mut i = 0;
    if !digits(&mut i) {
        return false;
    }
    if i < b.len() && matches!(b[i], b'.' | b',') {
        i += 1;
        if !digits(&mut i) {
            return false;
        }
    }
    if i >= b.len() || !matches!(b[i], b'e' | b'E') {
        return false;
    }
    i += 1;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    digits(&mut i) && i == b.len()
}

/// `numKey`: spaces out, the decimal separator told from the grouping one, then `parseFloat`.
pub(crate) fn num_key(v: &str) -> f64 {
    let mut s: String = v.chars().filter(|&c| !js_space(c)).collect();
    match (s.rfind(','), s.rfind('.')) {
        (Some(c), Some(p)) if c > p => {
            s.retain(|ch| ch != '.');
            s = s.replacen(',', ".", 1);
        }
        (Some(_), Some(_)) => s.retain(|ch| ch != ','),
        _ if comma_decimal(&s) => s = s.replacen(',', ".", 1),
        _ => s.retain(|ch| ch != ','),
    }
    parse_float(&s)
}

/// ^[-+]?\d+,\d+$
fn comma_decimal(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    t.split_once(',').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()))
}

/// JavaScript's `parseFloat`: the longest number at the start, NaN if none.
pub(crate) fn parse_float(s: &str) -> f64 {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && matches!(b[i], b'+' | b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return if b.first() == Some(&b'-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut any = i > start;
    if i < b.len() && b[i] == b'.' {
        let j = i + 1;
        let mut k = j;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if any || k > j {
            any = true;
            i = k;
        }
    }
    if !any {
        return f64::NAN;
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        let mut k = i + 1;
        if k < b.len() && matches!(b[k], b'+' | b'-') {
            k += 1;
        }
        let d = k;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if k > d {
            i = k;
        }
    }
    s[..i].parse().unwrap_or(f64::NAN)
}

/// `x.toLocaleString('fr-FR', {minimumFractionDigits: dec, maximumFractionDigits: dec})`:
/// "1 234,50", grouped with narrow no-break spaces, rounded half away from zero.
fn fr_locale(x: f64, dec: usize, out: &mut String) {
    let r = round_digits(&format!("{}", x.abs()), dec);
    let (int, frac) = r.split_once('.').unwrap_or((&r, ""));
    if x < 0.0 {
        out.push('-');
    }
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(ch);
    }
    if !frac.is_empty() {
        out.push(',');
        out.push_str(frac);
    }
}

/* ---- Series --------------------------------------------------------- */

const NAME_LISTS: [&[&str]; 8] = [
    &["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"],
    &["lun", "mar", "mer", "jeu", "ven", "sam", "dim"],
    &["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"],
    &["mon", "tue", "wed", "thu", "fri", "sat", "sun"],
    &["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    &["janv", "févr", "mars", "avr", "mai", "juin", "juil", "août", "sept", "oct", "nov", "déc"],
    &["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"],
    &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"],
];

#[derive(Clone, Debug, PartialEq)]
struct DateSeed {
    iso: bool,
    sep: char,
    y: i64,
    mo: u32,
    d: u32,
    /// Two-digit month (ISO) or day (day first) in the first seed.
    pad: bool,
    yy: bool,
    /// What follows the date (a time), kept as is.
    rest: String,
    days: i64,
}

/// `yyyy-mm-dd` or `dd/mm/yyyy` (`-`, `/` or `.`, the same twice; a 2-digit
/// year is 20yy), then nothing or a space / T and anything on that line.
fn date_seed(v: &str) -> Option<DateSeed> {
    let b = v.as_bytes();
    let run = |i: usize| b[i.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
    let num = |i: usize, n: usize| v[i..i + n].parse::<i64>().unwrap();
    let a = run(0);
    let sep = *b.get(a).filter(|c| matches!(c, b'-' | b'/' | b'.'))?;
    let p = a + 1;
    let (iso, y, mo, d, end, yy, pad);
    if a == 4 {
        let lm = run(p);
        if !(1..=2).contains(&lm) || b.get(p + lm) != Some(&sep) { return None; }
        let q = p + lm + 1;
        let ld = run(q);
        if !(1..=2).contains(&ld) { return None; }
        (iso, y, mo, d, end, yy, pad) = (true, num(0, 4), num(p, lm), num(q, ld), q + ld, false, lm == 2);
    } else if (1..=2).contains(&a) {
        let lm = run(p);
        if !(1..=2).contains(&lm) || b.get(p + lm) != Some(&sep) { return None; }
        let q = p + lm + 1;
        let ly = run(q);
        if ly != 4 && ly != 2 { return None; }
        let yv = num(q, ly);
        (iso, y, mo, d, end, yy, pad) = (false, if ly == 2 { 2000 + yv } else { yv }, num(p, lm), num(0, a), q + ly, ly == 2, a == 2);
    } else {
        return None;
    }
    let rest = &v[end..];
    if !(rest.is_empty() || rest.starts_with([' ', 'T'])) || rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let (mo, d) = (mo as u32, d as u32);
    if !(1..=12).contains(&mo) || d < 1 || d > last_day(y, mo) {
        return None;
    }
    Some(DateSeed { iso, sep: sep as char, y, mo, d, pad, yy, rest: rest.to_string(), days: days_from_civil(y, mo, d) })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Day,
    Month,
    /// Every seed the last day of its month: so is every value.
    MonthEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Case {
    Lower,
    Upper,
    Capital,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Loop(Vec<String>),
    Num { first: f64, step: f64, dec: usize, grouped: bool, comma: bool, lead: usize },
    Date { f: DateSeed, unit: Unit, step: i64 },
    Names { list: &'static [&'static str], first: usize, step: usize, case: Case, dot: bool },
    TextNum { pre: String, post: String, first: f64, step: f64, width: usize, zero: bool },
}

/// What seeds extend to; `write(k)` gives the value at k.
#[derive(Clone, Debug, PartialEq)]
pub struct Series(Kind);

impl Series {
    /// The series the seeds start (in order: k = 0, 1…); `copy`: repeat them.
    pub fn new<S: AsRef<str>>(seeds: &[S], copy: bool) -> Series {
        let vals: Vec<&str> = seeds.iter().map(|s| s.as_ref()).collect();
        let n = vals.len();
        let lp = || Series(Kind::Loop(vals.iter().map(|s| s.to_string()).collect()));
        if n == 0 || copy || vals.iter().all(|v| js_trim(v).is_empty()) {
            return lp();
        }
        let trim: Vec<&str> = vals.iter().map(|v| js_trim(v)).collect();
        let step_of = |nums: &[f64]| if n == 1 { 1.0 } else { (nums[n - 1] - nums[0]) / (n - 1) as f64 };

        // A date is not a number: the web app read "15.03.2024" as 15.03 and
        // filled 16.0300, 17.0300…
        if trim.iter().all(|v| numeric_like(v) && !num_key(v).is_nan() && date_seed(v).is_none()) {
            let nums: Vec<f64> = trim.iter().map(|v| num_key(v)).collect();
            let step = step_of(&nums);
            let ends_digits = |v: &str, seps: &[char]| -> Option<usize> {
                let d = v.bytes().rev().take_while(u8::is_ascii_digit).count();
                (d > 0 && v[..v.len() - d].ends_with(seps)).then_some(d)
            };
            let comma = trim.iter().any(|v| ends_digits(v, &[',']).is_some() && !v.contains('.'));
            let grouped = trim.iter().any(|v| {
                let c: Vec<char> = v.chars().collect();
                c.windows(5).any(|w| w[0].is_ascii_digit() && js_space(w[1]) && w[2..].iter().all(char::is_ascii_digit))
            });
            let mut dec = trim.iter().map(|v| match ends_digits(v, &['.', ',']) {
                Some(d) if !(grouped && !comma) => d,
                _ => 0,
            }).max().unwrap_or(0);
            if step % 1.0 != 0.0 {
                dec = dec.max(js_num(step).split_once('.').map_or(0, |(_, f)| f.len()));
            }
            let t0 = trim[0];
            // A leading zero (^-?0\d): the width is kept, as for a code.
            let u = t0.strip_prefix('-').unwrap_or(t0).as_bytes();
            let lead = if u.len() > 1 && u[0] == b'0' && u[1].is_ascii_digit() { t0.replacen('-', "", 1).chars().count() } else { 0 };
            return Series(Kind::Num { first: nums[0], step, dec, grouped, comma, lead });
        }

        let ds: Vec<Option<DateSeed>> = trim.iter().map(|v| date_seed(v)).collect();
        if ds.iter().all(Option::is_some) {
            let ds: Vec<DateSeed> = ds.into_iter().map(Option::unwrap).collect();
            let f = &ds[0];
            if ds.iter().all(|x| x.iso == f.iso && x.sep == f.sep) {
                let mut unit = Unit::Day;
                let mut step = if n == 1 { 1 } else { js_round((ds[n - 1].days - f.days) as f64 / (n - 1) as f64) as i64 };
                // Same day of the month — or every seed its month's last — steps by months.
                let eom = n > 1 && ds.iter().all(|x| x.d == last_day(x.y, x.mo)) && !ds.iter().all(|x| x.d == f.d);
                if n > 1 && (eom || ds.iter().all(|x| x.d == f.d)) {
                    let months = |x: &DateSeed| x.y * 12 + x.mo as i64;
                    let m = (months(&ds[n - 1]) - months(f)) as f64 / (n - 1) as f64;
                    if m != 0.0 && m.fract() == 0.0 {
                        unit = if eom { Unit::MonthEnd } else { Unit::Month };
                        step = m as i64;
                    }
                }
                return Series(Kind::Date { f: f.clone(), unit, step });
            }
        }

        let low: Vec<String> = trim.iter().map(|v| {
            let l = v.to_lowercase();
            l.strip_suffix('.').map(str::to_string).unwrap_or(l)
        }).collect();
        if let Some(list) = NAME_LISTS.iter().find(|l| low.iter().all(|v| l.contains(&v.as_str()))) {
            let idx: Vec<usize> = low.iter().map(|v| list.iter().position(|w| w == v).unwrap()).collect();
            let len = list.len();
            let step = if n == 1 { 1 } else { match (idx[1] + len - idx[0]) % len { 0 => 1, s => s } };
            let t0 = trim[0];
            let first = t0.chars().next().unwrap();
            let case = if t0 == t0.to_uppercase() {
                Case::Upper
            } else if first.to_uppercase().eq(std::iter::once(first)) {
                Case::Capital
            } else {
                Case::Lower
            };
            return Series(Kind::Names { list, first: idx[0], step, case, dot: t0.ends_with('.') });
        }

        // Text ending in a number: the last run of digits counts.
        let tn: Vec<Option<(&str, &str, &str)>> = vals.iter().map(|v| text_num(v)).collect();
        if let Some(Some((pre, _, post))) = tn.first() {
            if tn.iter().all(|m| m.is_some_and(|(p, _, s)| p == *pre && s == *post)) {
                let nums: Vec<f64> = tn.iter().map(|m| m.unwrap().1.parse().unwrap_or(f64::NAN)).collect();
                let step = match js_round(step_of(&nums)) { s if s == 0.0 || s.is_nan() => 1.0, s => s };
                let digits = tn[0].unwrap().1;
                return Series(Kind::TextNum {
                    pre: pre.to_string(), post: post.to_string(), first: nums[0], step,
                    width: digits.len(), zero: digits.starts_with('0') && digits.len() > 1,
                });
            }
        }
        lp()
    }

    /// The value at `k`, appended to `out`.
    pub fn write(&self, k: i64, out: &mut String) {
        match &self.0 {
            Kind::Loop(v) => out.push_str(&v[k.rem_euclid(v.len() as i64) as usize]),
            Kind::Num { first, step, dec, grouped, comma, lead } => {
                let x = first + step * k as f64;
                if *grouped {
                    return fr_locale(x, *dec, out);
                }
                let mut s = to_fixed(x.abs(), (*dec).min(10));
                if *lead > 0 && *dec == 0 && s.len() < *lead {
                    s.insert_str(0, &"0".repeat(lead - s.len()));
                }
                if *comma {
                    s = s.replacen('.', ",", 1);
                }
                if x < 0.0 {
                    out.push('-');
                }
                out.push_str(&s);
            }
            Kind::Date { f, unit, step } => {
                let (y, mo, d) = match unit {
                    Unit::Day => civil_from_days(f.days + step * k),
                    _ => {
                        let tot = f.y * 12 + (f.mo as i64 - 1) + step * k;
                        let (y, mo) = (tot.div_euclid(12), (tot.rem_euclid(12) + 1) as u32);
                        (y, mo, if *unit == Unit::MonthEnd { last_day(y, mo) } else { f.d.min(last_day(y, mo)) })
                    }
                };
                let p2 = |x: u32, out: &mut String| {
                    let _ = if f.pad { write!(out, "{x:02}") } else { write!(out, "{x}") };
                };
                if f.iso {
                    let _ = write!(out, "{y}{}", f.sep);
                    p2(mo, out);
                    out.push(f.sep);
                    p2(d, out);
                } else {
                    p2(d, out);
                    out.push(f.sep);
                    p2(mo, out);
                    out.push(f.sep);
                    let at = out.len();
                    let _ = write!(out, "{y}");
                    if f.yy && out.len() - at > 2 {
                        out.replace_range(at..out.len() - 2, "");
                    }
                }
                out.push_str(&f.rest);
            }
            Kind::Names { list, first, step, case, dot } => {
                let len = list.len() as i64;
                let w = list[(*first as i64 + *step as i64 * k).rem_euclid(len) as usize];
                match case {
                    Case::Upper => out.push_str(&w.to_uppercase()),
                    Case::Capital => {
                        let mut c = w.chars();
                        out.extend(c.next().into_iter().flat_map(char::to_uppercase));
                        out.push_str(c.as_str());
                    }
                    Case::Lower => out.push_str(w),
                }
                if *dot {
                    out.push('.');
                }
            }
            Kind::TextNum { pre, post, first, step, width, zero } => {
                let x = first + step * k as f64;
                out.push_str(pre);
                if *zero {
                    let s = js_num(x.max(0.0));
                    out.extend(std::iter::repeat_n('0', width.saturating_sub(s.len())));
                    out.push_str(&s);
                } else {
                    out.push_str(&js_num(x));
                }
                out.push_str(post);
            }
        }
    }

    pub fn at(&self, k: i64) -> String {
        let mut s = String::new();
        self.write(k, &mut s);
        s
    }
}

/// `^(.*?)(\d+)(\D*)$`: (before, digits, after) around the last run of digits.
fn text_num(v: &str) -> Option<(&str, &str, &str)> {
    let b = v.as_bytes();
    let e = b.iter().rposition(u8::is_ascii_digit)? + 1;
    let s = b[..e].iter().rposition(|c| !c.is_ascii_digit()).map_or(0, |i| i + 1);
    let pre = &v[..s];
    // `.` stops at a line break: a number after one is not "text ending in a number".
    if pre.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    Some((pre, &v[s..e], &v[e..]))
}

/* ---- Fills ---------------------------------------------------------- */

/// A block of the grid: rows `r0..=r1` of the rows shown, data columns
/// `c0..=c1` (hidden ones skipped through `vis`, the visible columns in order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub r0: usize,
    pub r1: usize,
    pub c0: usize,
    pub c1: usize,
}

impl Rect {
    fn cols(&self, vis: &[usize]) -> Vec<usize> {
        vis.iter().copied().filter(|c| (self.c0..=self.c1).contains(c)).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Down,
    Up,
    Right,
    Left,
}

pub const FILL_DOWN_HELP: &str = "Select the cells to fill first.\n\nThe filled cells at the top of each column are the start of the series; the empty ones below them are filled.";
pub const FILL_BLANKS_HELP: &str = "Select the column to fill first.\n\nOne cell selects its whole column; a range limits the fill to those rows. Each empty cell then takes the value above it.";
pub const NO_BLANK_TO_FILL: &str = "No empty cell with a value above it.";

/// The status line after a fill or any block write: `verb` is "filled with
/// a series", "filled (copy)", "filled from above", "set"…
pub fn cells_done(n: usize, verb: &str) -> String {
    if n == 0 { "Nothing changed.".into() } else { not_saved(&format!("{} cells {verb}", fmt_int(n))) }
}

fn blank(v: &str) -> bool {
    js_trim(v).is_empty()
}

/// Writes `value(k, out)` in column `col` of the rows shown `range`, at
/// k = position − `base`, keeping the cells that change.
fn fill_col(
    st: &State, rows: &[RowId], col: usize, range: std::ops::RangeInclusive<usize>, base: usize,
    series: &Series, cancel: &AtomicBool,
) -> Option<Vec<Change>> {
    let (lo, n) = (*range.start(), range.end() + 1 - range.start());
    let parts = par_chunks(n, cancel, |r, s: &mut (Vec<u8>, String)| {
        let mut out = Vec::new();
        for p in r {
            let pos = lo + p;
            s.1.clear();
            series.write(pos as i64 - base as i64, &mut s.1);
            let id = rows[pos];
            if !st.with_cell(id, col, &mut s.0, |v| v == s.1) {
                out.push((id, col, s.1.clone()));
            }
        }
        out
    })?;
    Some(parts.concat())
}

fn seeds(st: &State, rows: &[RowId], col: usize, r: std::ops::RangeInclusive<usize>) -> Vec<String> {
    r.map(|p| st.cell(rows[p], col)).collect()
}

/// Ctrl+D: in each selected column, the filled cells at the top are the seeds
/// and the series runs through the rest (a column filled throughout is
/// re-seeded from its first cell). Err: the help shown when one row is selected.
pub fn fill_down(st: &State, rows: &[RowId], vis: &[usize], sel: Rect, cancel: &AtomicBool) -> Option<Result<Vec<Change>, &'static str>> {
    if sel.r1 <= sel.r0 {
        return Some(Err(FILL_DOWN_HELP));
    }
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    for c in sel.cols(vis) {
        let mut s = 0;
        while sel.r0 + s <= sel.r1 && !st.with_cell(rows[sel.r0 + s], c, &mut tmp, blank) {
            s += 1;
        }
        if s == 0 {
            continue;
        }
        if sel.r0 + s > sel.r1 {
            s = 1;
        }
        let series = Series::new(&seeds(st, rows, c, sel.r0..=sel.r0 + s - 1), false);
        out.extend(fill_col(st, rows, c, sel.r0 + s..=sel.r1, sel.r0, &series, cancel)?);
    }
    Some(Ok(out))
}

/// Where the fill handle, dragged from `sel` to the cell (`r`, `c`), fills:
/// the side the pointer went furthest out of the selection. None: still inside.
pub fn fill_target(sel: Rect, r: usize, c: usize, vis: &[usize]) -> Option<(Dir, Rect)> {
    let pos = |x: usize| vis.iter().position(|&v| v == x);
    let ci = pos(c)? as i64;
    let c0i = pos(sel.c0).unwrap_or(0) as i64;
    let c1i = pos(sel.c1).unwrap_or(vis.len().saturating_sub(1)) as i64;
    let (r, r0, r1) = (r as i64, sel.r0 as i64, sel.r1 as i64);
    let (down, up, right, left) = (r - r1, r0 - r, ci - c1i, c0i - ci);
    let best = down.max(up).max(right).max(left);
    if best <= 0 {
        return None;
    }
    let rect = |r0: i64, r1: i64, c0: usize, c1: usize| Rect { r0: r0 as usize, r1: r1 as usize, c0, c1 };
    Some(if best == down {
        (Dir::Down, rect(r1 + 1, r, sel.c0, sel.c1))
    } else if best == up {
        (Dir::Up, rect(r, r0 - 1, sel.c0, sel.c1))
    } else if best == right {
        (Dir::Right, rect(r0, r1, vis[(c1i + 1) as usize], c))
    } else {
        (Dir::Left, rect(r0, r1, c, vis[(c0i - 1) as usize]))
    })
}

/// The fill handle let go: the selection's cells extend as a series (`copy`:
/// repeated) over `target`, down or up each column, or along each row.
pub fn fill_handle(st: &State, rows: &[RowId], vis: &[usize], sel: Rect, dir: Dir, target: Rect, copy: bool, cancel: &AtomicBool) -> Option<Vec<Change>> {
    let mut out = Vec::new();
    if matches!(dir, Dir::Down | Dir::Up) {
        for c in sel.cols(vis) {
            let series = Series::new(&seeds(st, rows, c, sel.r0..=sel.r1), copy);
            out.extend(fill_col(st, rows, c, target.r0..=target.r1, sel.r0, &series, cancel)?);
        }
        return Some(out);
    }
    let seed_cols = sel.cols(vis);
    let base = vis.iter().position(|&v| Some(&v) == seed_cols.first())? as i64;
    let targets: Vec<(usize, i64)> = target.cols(vis).into_iter().map(|c| (c, vis.iter().position(|&v| v == c).unwrap() as i64 - base)).collect();
    let n = sel.r1 + 1 - sel.r0;
    let parts = par_chunks(n, cancel, |r, s: &mut (Vec<u8>, String)| {
        let mut out = Vec::new();
        for p in r {
            let id = rows[sel.r0 + p];
            let series = Series::new(&seed_cols.iter().map(|&c| st.cell(id, c)).collect::<Vec<_>>(), copy);
            for &(c, k) in &targets {
                s.1.clear();
                series.write(k, &mut s.1);
                if !st.with_cell(id, c, &mut s.0, |v| v == s.1) {
                    out.push((id, c, s.1.clone()));
                }
            }
        }
        out
    })?;
    out.extend(parts.into_iter().flatten());
    Some(out)
}

/// The cell editor's Enter on a block (`series` false: the value in every
/// cell) or Ctrl+Enter (a series from it, down each column — along the row
/// when a single row is selected).
pub fn fill_value(st: &State, rows: &[RowId], vis: &[usize], sel: Rect, value: &str, series: bool, cancel: &AtomicBool) -> Option<Vec<Change>> {
    let gen = Series::new(&[value], !series);
    let cols = sel.cols(vis);
    let horizontal = sel.r0 == sel.r1;
    let parts = par_chunks(sel.r1 + 1 - sel.r0, cancel, |r, s: &mut (Vec<u8>, String)| {
        let mut out = Vec::new();
        for p in r {
            let id = rows[sel.r0 + p];
            for (ci, &c) in cols.iter().enumerate() {
                s.1.clear();
                gen.write(if horizontal { ci as i64 } else { p as i64 }, &mut s.1);
                if !st.with_cell(id, c, &mut s.0, |v| v == s.1) {
                    out.push((id, c, s.1.clone()));
                }
            }
        }
        out
    })?;
    Some(parts.concat())
}

/// Fill empty cells from above — exports and pivot tables write a category
/// once at the top of its group. In the selected columns (one cell selected:
/// its whole column), in the order shown, each blank cell takes the nearest
/// value above it, above the selection too; blanks before any value stay.
/// Ok(empty): say NO_BLANK_TO_FILL.
pub fn fill_blanks(st: &State, rows: &[RowId], vis: &[usize], sel: Rect, cancel: &AtomicBool) -> Option<Result<Vec<Change>, &'static str>> {
    if rows.is_empty() {
        return Some(Err(FILL_BLANKS_HELP));
    }
    let one = sel.r0 == sel.r1;
    let (r0, r1) = if one { (0, rows.len() - 1) } else { (sel.r0, sel.r1) };
    let n = r1 + 1 - r0;
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    for c in sel.cols(vis) {
        // Pass 1: each chunk's last filled position, so that pass 2 can start
        // every chunk with the value carried into it.
        let lasts = par_chunks(n, cancel, |r, tmp: &mut Vec<u8>| {
            r.rev().map(|p| r0 + p).find(|&pos| !st.with_cell(rows[pos], c, tmp, blank))
        })?;
        let mut carry = (0..r0).rev().find(|&pos| !st.with_cell(rows[pos], c, &mut tmp, blank));
        let mut starts = Vec::with_capacity(lasts.len());
        for l in &lasts {
            starts.push(carry);
            carry = l.or(carry);
        }
        let parts = par_chunks(n, cancel, |r, s: &mut (Vec<u8>, Option<(usize, String)>)| {
            let mut last = starts[r.start / super::CHUNK];
            let mut out = Vec::new();
            for p in r {
                let pos = r0 + p;
                let id = rows[pos];
                if !st.with_cell(id, c, &mut s.0, blank) {
                    last = Some(pos);
                } else if let Some(l) = last {
                    if s.1.as_ref().is_none_or(|(at, _)| *at != l) {
                        s.1 = Some((l, st.with_cell(rows[l], c, &mut s.0, str::to_string)));
                    }
                    let v = &s.1.as_ref().unwrap().1;
                    // A blank of spaces gets the value; an empty one equal to it can't be.
                    if !st.with_cell(id, c, &mut s.0, |x| x == v) {
                        out.push((id, c, v.clone()));
                    }
                }
            }
            out
        })?;
        out.extend(parts.into_iter().flatten());
    }
    Some(Ok(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::all_rows;
    use crate::edit::tests::{col, table, NO};

    fn run(seeds: &[&str], ks: std::ops::Range<i64>) -> Vec<String> {
        let s = Series::new(seeds, false);
        ks.map(|k| s.at(k)).collect()
    }

    #[test]
    fn calendar() {
        for z in [-800_000i64, -1, 0, 59, 11_016, 19_782, 2_932_896] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(last_day(2100, 2), 28);
    }

    #[test]
    fn numbers() {
        assert_eq!(run(&["1"], 0..4), vec!["1", "2", "3", "4"]);
        assert_eq!(run(&["2", "4"], 0..4), vec!["2", "4", "6", "8"]);
        assert_eq!(run(&["1,5", "2"], 0..3), vec!["1,5", "2,0", "2,5"]);
        assert_eq!(run(&["007"], 0..2), vec!["007", "008"]);
        assert_eq!(run(&["10", "7"], 0..5), vec!["10", "7", "4", "1", "-2"]);
        assert_eq!(run(&["0.1", "0.2"], 2..4), vec!["0.3", "0.4"]);
        assert_eq!(run(&["1 000", "2 000"], 2..4), vec!["3\u{202f}000", "4\u{202f}000"]);
        assert_eq!(run(&["1 234,5"], 0..2), vec!["1\u{202f}234,5", "1\u{202f}235,5"]);
        assert_eq!(run(&["5"], -2..0), vec!["3", "4"]);
        assert_eq!(run(&["1.5e3"], 0..1), vec!["1500"]);
        assert!(numeric_like(" -12,5 € ") && !numeric_like("12a") && !numeric_like("1..2"));
        assert_eq!(num_key("1.234,5"), 1234.5);
        assert_eq!(num_key("1,234,567"), 1234567.0);
        assert!(num_key("€12").is_nan());
    }

    #[test]
    fn dates() {
        assert_eq!(run(&["2024-01-30"], 0..3), vec!["2024-01-30", "2024-01-31", "2024-02-01"]);
        assert_eq!(run(&["31/01/2024", "29/02/2024"], 0..4), vec!["31/01/2024", "29/02/2024", "31/03/2024", "30/04/2024"]);
        assert_eq!(run(&["31/01/2024", "31/03/2024"], 0..3), vec!["31/01/2024", "31/03/2024", "31/05/2024"]);
        assert_eq!(run(&["15/01/24", "15/02/24"], 2..4), vec!["15/03/24", "15/04/24"]);
        assert_eq!(run(&["2024-01-31", "2024-02-29"], 2..3), vec!["2024-03-31"]);
        assert_eq!(run(&["2024-1-5 10:00", "2024-1-12 08:00"], 2..3), vec!["2024-1-19 10:00"]);
        assert_eq!(run(&["2023-06-15", "2024-06-15"], 1..3), vec!["2024-06-15", "2025-06-15"]);
        assert_eq!(run(&["1.2.2024"], 0..2), vec!["1.2.2024", "2.2.2024"]);
        // Different formats: not a date series — the trailing number counts.
        assert_eq!(run(&["2024-01-01", "02/01/2024"], 2..3), vec!["2024-01-01"]);
        assert_eq!(date_seed("2024-02-30"), None);
    }

    #[test]
    fn names_and_text() {
        assert_eq!(run(&["Lundi"], 0..3), vec!["Lundi", "Mardi", "Mercredi"]);
        assert_eq!(run(&["JAN", "MAR"], 2..4), vec!["MAY", "JUL"]);
        assert_eq!(run(&["déc."], 0..2), vec!["déc.", "janv."]);
        assert_eq!(run(&["sunday"], 0..2), vec!["sunday", "monday"]);
        assert_eq!(run(&["Lot 7"], 0..3), vec!["Lot 7", "Lot 8", "Lot 9"]);
        assert_eq!(run(&["A007-x", "A009-x"], 2..4), vec!["A011-x", "A013-x"]);
        assert_eq!(run(&["A002"], -3..0), vec!["A000", "A000", "A001"]);
        assert_eq!(run(&["x", "y"], 0..5), vec!["x", "y", "x", "y", "x"]);
        assert_eq!(run(&["a1", "b2"], 2..3), vec!["a1"]);
        assert_eq!(Series::new(&["1", "2"], true).at(2), "1");
        assert_eq!(Series::new(&["", " "], false).at(3), " ");
    }

    #[test]
    fn fills() {
        let mut t = table("a,b\n1,x\n,y\n,\n,\n7,z\n");
        let rows = all_rows(&t.st);
        let vis = [0, 1];
        let sel = Rect { r0: 0, r1: 3, c0: 0, c1: 1 };
        let ch = fill_down(&t.st, &rows, &vis, sel, &NO).unwrap().unwrap();
        t.set_cells(&ch);
        assert_eq!(col(&t, 0), vec!["1", "2", "3", "4", "7"]);
        assert_eq!(col(&t, 1), vec!["x", "y", "x", "y", "z"]);
        t.undo();
        assert_eq!(fill_down(&t.st, &rows, &vis, Rect { r1: 0, ..sel }, &NO).unwrap(), Err(FILL_DOWN_HELP));

        let ch = fill_blanks(&t.st, &rows, &vis, Rect { r0: 2, r1: 2, c0: 0, c1: 1 }, &NO).unwrap().unwrap();
        t.set_cells(&ch);
        assert_eq!(col(&t, 0), vec!["1", "1", "1", "1", "7"]);
        assert_eq!(col(&t, 1), vec!["x", "y", "y", "y", "z"]);
        t.undo();
        // A range: only its rows, the value above the range carried in.
        let ch = fill_blanks(&t.st, &rows, &vis, Rect { r0: 2, r1: 2 + 1, c0: 0, c1: 0 }, &NO).unwrap().unwrap();
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].2, "1");

        let (dir, target) = fill_target(Rect { r0: 0, r1: 0, c0: 0, c1: 0 }, 0, 1, &vis).unwrap();
        assert_eq!((dir, target), (Dir::Right, Rect { r0: 0, r1: 0, c0: 1, c1: 1 }));
        let ch = fill_handle(&t.st, &rows, &vis, Rect { r0: 0, r1: 0, c0: 0, c1: 0 }, dir, target, false, &NO).unwrap();
        assert_eq!(ch, vec![(rows[0], 1, "2".to_string())]);
        let (dir, target) = fill_target(Rect { r0: 4, r1: 4, c0: 0, c1: 0 }, 2, 0, &vis).unwrap();
        assert_eq!(dir, Dir::Up);
        let ch = fill_handle(&t.st, &rows, &vis, Rect { r0: 4, r1: 4, c0: 0, c1: 0 }, dir, target, false, &NO).unwrap();
        assert_eq!(ch.iter().map(|c| c.2.as_str()).collect::<Vec<_>>(), vec!["5", "6"]);

        let ch = fill_value(&t.st, &rows, &vis, Rect { r0: 1, r1: 3, c0: 0, c1: 0 }, "Lot 1", true, &NO).unwrap();
        assert_eq!(ch.iter().map(|c| c.2.as_str()).collect::<Vec<_>>(), vec!["Lot 1", "Lot 2", "Lot 3"]);
        assert_eq!(cells_done(0, "set"), "Nothing changed.");
    }
}
