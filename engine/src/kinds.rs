//! Cells as the JS version read them, shared by the insights, the schemas and
//! the diagnosis: a cell's kind (number, date, text), numbers and dates as
//! keys, column names as words, garbled accents, numbers written the French
//! way in the texts shown. Kept apart from `text.rs`, whose readings (filters,
//! sorts) are broader: these must find what the JS found, on the same values.

use crate::enc;
use crate::scan;
use crate::table::{RowId, State};
use std::sync::OnceLock;

pub type FxMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type FxSet<K> = rustc_hash::FxHashSet<K>;

/// JS `\s`: Unicode white space and the BOM (NEL aside).
#[inline]
pub fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// JS `String.prototype.trim`.
#[inline]
pub fn js_trim(s: &str) -> &str {
    if s.as_bytes().first().is_some_and(|b| b.is_ascii_graphic()) && s.as_bytes().last().is_some_and(|b| b.is_ascii_graphic()) {
        return s;
    }
    s.trim_matches(is_js_space)
}

/// Length in UTF-16 units: what JS calls a string's length.
#[inline]
pub fn utf16_len(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char::len_utf16).sum() }
}

/// The first `n` UTF-16 units of `s` (never half a character).
pub fn utf16_prefix(s: &str, n: usize) -> &str {
    if s.len() <= n { return s; }
    let mut u = 0;
    for (i, c) in s.char_indices() {
        u += c.len_utf16();
        if u > n { return &s[..i]; }
    }
    s
}

/// JS `cutTxt`: at most n units, an ellipsis in place of the rest.
pub fn cut(v: &str, n: usize) -> String {
    if utf16_len(v) > n { format!("{}…", utf16_prefix(v, n - 1)) } else { v.to_string() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Empty,
    Num,
    Date,
    Text,
}

/// JS `cellType`: a number or a date starts with a digit, a sign, a decimal
/// mark or a currency; anything else is text at once.
pub fn cell_type(v: &str) -> Kind {
    let s = js_trim(v);
    let Some(c0) = s.chars().next() else { return Kind::Empty };
    if !(c0.is_ascii_digit() || matches!(c0, '+' | '-' | '.' | ',' | '€' | '$' | '£')) {
        return Kind::Text;
    }
    if is_numeric_like(s) { Kind::Num } else if is_date_like(s) { Kind::Date } else { Kind::Text }
}

/// JS `isNumericLike`: spaces, a leading sign and currencies aside, digits in
/// groups split by `.` or `,` (`1 234,50`, `1.234.567`), or a mantissa and an exponent.
pub fn is_numeric_like(v: &str) -> bool {
    let s = js_trim(v);
    let mut it = s.chars().filter(|&c| !is_js_space(c)).peekable();
    if matches!(it.peek(), Some('+' | '-')) { it.next(); }
    #[derive(PartialEq)]
    enum St { Start, Int, Sep, Frac, ESign, EDigit, Exp }
    let (mut st, mut groups) = (St::Start, 0);
    for c in it.filter(|c| !matches!(c, '€' | '$' | '£' | '%')) {
        let d = c.is_ascii_digit();
        st = match st {
            St::Start | St::Sep if d => if st == St::Start { St::Int } else { St::Frac },
            St::Int | St::Frac if d => st,
            St::Int | St::Frac if c == '.' || c == ',' => { groups += 1; St::Sep }
            St::Int | St::Frac if (c == 'e' || c == 'E') && groups <= 1 => St::ESign,
            St::ESign if c == '+' || c == '-' => St::EDigit,
            St::ESign | St::EDigit | St::Exp if d => St::Exp,
            _ => return false,
        };
    }
    matches!(st, St::Int | St::Frac | St::Exp)
}

/// The length of the run of ASCII digits at `i`.
#[inline]
fn digit_run(b: &[u8], i: usize) -> usize {
    b[i.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count()
}

/// JS `isDateLike`: `d/m/y`, `y-m-d` (any of - / .), with an optional time
/// and zone — `2025-12-10T17:26:15.000Z` included.
pub fn is_date_like(v: &str) -> bool {
    let b = js_trim(v).as_bytes();
    let mut i = 0;
    for (k, (lo, hi)) in [(1, 4), (1, 2), (1, 4)].into_iter().enumerate() {
        let r = digit_run(b, i);
        if r < lo || r > hi { return false; }
        i += r;
        if k < 2 {
            if !matches!(b.get(i), Some(b'-' | b'/' | b'.')) { return false; }
            i += 1;
        }
    }
    if i == b.len() { return true; }
    // The time: all of it to the end, or no match at all.
    if !matches!(b[i], b' ' | b'T') { return false; }
    i += 1;
    let r = digit_run(b, i);
    if !(1..=2).contains(&r) || b.get(i + r) != Some(&b':') { return false; }
    i += r + 1;
    if digit_run(b, i) != 2 { return false; }
    i += 2;
    if b.get(i) == Some(&b':') {
        if digit_run(b, i + 1) != 2 { return false; }
        i += 3;
        if b.get(i) == Some(&b'.') {
            let r = digit_run(b, i + 1);
            if r == 0 { return false; }
            i += 1 + r;
        }
    }
    match b.get(i) {
        None => true,
        Some(b'Z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            let r = digit_run(b, i + 1);
            (r == 4 && i + 5 == b.len()) || (r == 2 && b.get(i + 3) == Some(&b':') && digit_run(b, i + 4) == 2 && i + 6 == b.len())
        }
        _ => false,
    }
}

/// JS `parseFloat`: the longest number at the start, NaN if none.
pub fn js_parse_float(s: &str) -> f64 {
    let s = s.trim_start_matches(is_js_space);
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    if s[i..].starts_with("Infinity") {
        return if b[0] == b'-' { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let int = digit_run(b, i);
    i += int;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        frac = digit_run(b, i + 1);
        if int + frac > 0 { i += 1 + frac; }
    }
    if int + frac == 0 { return f64::NAN; }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let j = i + 1 + usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
        let e = digit_run(b, j);
        if e > 0 { i = j + e; }
    }
    s[..i].parse().unwrap_or(f64::NAN)
}

/// JS `numKey`: the decimal mark is the last of `.` and `,` when both appear,
/// a lone comma before decimals; the other one groups thousands.
pub fn num_key(v: &str) -> f64 {
    let s: String = v.chars().filter(|&c| !is_js_space(c)).collect();
    let (lc, ld) = (s.rfind(','), s.rfind('.'));
    let t = match (lc, ld) {
        (Some(c), Some(d)) => if c > d { s.replace('.', "").replacen(',', ".", 1) } else { s.replace(',', "") },
        (Some(_), None) if {
            let b = s.trim_start_matches(['+', '-']).as_bytes();
            s.len() - b.len() <= 1 && b.iter().filter(|&&c| c == b',').count() == 1
                && b.iter().all(|&c| c.is_ascii_digit() || c == b',') && b[0] != b',' && b[b.len() - 1] != b','
        } => s.replacen(',', ".", 1),
        _ => s.replace(',', ""),
    };
    js_parse_float(&t)
}

/// JS `dateKey`: YYYYMMDDhhmmss from `y-m-d`, else day first (`15/03/24` is 2024).
pub fn date_key(v: &str) -> f64 {
    let b = js_trim(v).as_bytes();
    let mut parts = [0f64; 6];
    let (mut i, mut lens) = (0, [0usize; 3]);
    for k in 0..3 {
        let r = digit_run(b, i);
        let max = if k == 1 { 2 } else { 4 };
        // A longer run fails at once, but for the third part, cut after 4 (the match is a prefix).
        if r == 0 || (r > max && k < 2) { return f64::NAN; }
        let r = r.min(max);
        parts[k] = std::str::from_utf8(&b[i..i + r]).unwrap().parse().unwrap();
        lens[k] = r;
        i += r;
        if k < 2 {
            if !matches!(b.get(i), Some(b'-' | b'/' | b'.')) { return f64::NAN; }
            i += 1;
        }
    }
    if matches!(b.get(i), Some(b' ' | b'T')) {
        let h = digit_run(b, i + 1);
        if (1..=2).contains(&h) && b.get(i + 1 + h) == Some(&b':') && digit_run(b, i + 2 + h) >= 2 {
            let num = |a: usize, n: usize| std::str::from_utf8(&b[a..a + n]).unwrap().parse::<f64>().unwrap();
            parts[3] = num(i + 1, h);
            let m = i + 2 + h;
            parts[4] = num(m, 2);
            if b.get(m + 2) == Some(&b':') && digit_run(b, m + 3) >= 2 { parts[5] = num(m + 3, 2); }
        }
    }
    let (y, mo, d) = if lens[0] == 4 {
        (parts[0], parts[1], parts[2])
    } else {
        (if lens[2] == 2 { 2000.0 + parts[2] } else { parts[2] }, parts[1], parts[0])
    };
    ((((y * 100.0 + mo) * 100.0 + d) * 100.0 + parts[3]) * 100.0 + parts[4]) * 100.0 + parts[5]
}

/// Days since 1970 of a YYYYMMDDhhmmss key, the time as a fraction (JS
/// `Date.UTC`: months and days past their end roll over).
pub fn key_days(k: f64) -> f64 {
    let y = (k / 1e10).floor() as i64;
    let mo = ((k / 1e8).floor() as i64).rem_euclid(100);
    let d = ((k / 1e6).floor() as i64).rem_euclid(100);
    let y = if (0..=99).contains(&y) { 1900 + y } else { y };
    let m0 = mo - 1;
    let (y, m) = (y + m0.div_euclid(12), m0.rem_euclid(12) + 1);
    // Howard Hinnant's days_from_civil.
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468 + d - 1;
    days as f64 + (k % 1e6) / 1e6
}

/// JS `slugify`: lower case, accents off, anything but letters and digits as one space.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    let mut push = |c: char, out: &mut String| {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() { out.push(' '); }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    };
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'œ' => { push('o', &mut out); push('e', &mut out) }
            'æ' => { push('a', &mut out); push('e', &mut out) }
            'ß' => { push('s', &mut out); push('s', &mut out) }
            'ø' => push('o', &mut out),
            'đ' | 'ð' => push('d', &mut out),
            'ł' => push('l', &mut out),
            'þ' => { push('t', &mut out); push('h', &mut out) }
            _ if c.is_ascii() => push(c, &mut out),
            _ => unicode_normalization::char::decompose_canonical(c, |d| {
                if !('\u{300}'..='\u{36f}').contains(&d) { push(d, &mut out) }
            }),
        }
    }
    out
}

/// Whether `slug` holds one of `words` between non-letters (a-z): `(^|[^a-z])(w1|w2…)($|[^a-z])`.
fn has_word(slug: &str, words: &[&str]) -> bool {
    let b = slug.as_bytes();
    words.iter().any(|w| {
        slug.match_indices(w).any(|(i, _)| {
            (i == 0 || !b[i - 1].is_ascii_lowercase()) && b.get(i + w.len()).is_none_or(|c| !c.is_ascii_lowercase())
        })
    })
}

/// A name saying the column identifies its rows (a postal or town code does not, though it says "code").
pub fn id_like(name: &str) -> bool {
    let n = slugify(name);
    has_word(&n, &["id", "ids", "uuid", "guid", "code", "ref", "reference", "key", "cle", "num", "numero", "no", "matricule", "siret", "siren", "iban", "email", "e mail", "mail", "login"])
        && !["postal", "zip", "insee", "commune", "pays", "country", "ville", "city"].iter().any(|w| n.contains(w))
}

/// Digits that are a code, not a quantity: their shape matters (a postal code of 4 digits lost its zero).
pub fn code_like(name: &str) -> bool {
    has_word(&slugify(name), &["cp", "code", "codes", "zip", "postal", "postcode", "tel", "telephone", "phone", "mobile", "portable", "gsm", "fax", "siret", "siren", "insee", "iban", "bic", "id", "ref", "num", "numero", "no", "matricule", "compte"])
}

// ---- Garbled accents ---------------------------------------------------

/// Bytes 80–9F as Windows-1252 shows them (WHATWG: the five holes are C1 controls).
fn cp1252_high() -> &'static [char; 32] {
    static T: OnceLock<[char; 32]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = ['\0'; 32];
        for (k, c) in t.iter_mut().enumerate() {
            *c = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&[0x80 + k as u8]).0.chars().next().unwrap();
        }
        t
    })
}

/// The Windows-1252 byte that shows as `c`, if any.
#[inline]
pub fn cp1252_byte(c: char) -> Option<u8> {
    let u = c as u32;
    if u < 0x80 || (0xA0..=0xFF).contains(&u) { return Some(u as u8); }
    cp1252_high().iter().position(|&h| h == c).map(|k| 0x80 + k as u8)
}

/// UTF-8 read as Windows-1252 ("Ã©" for é, "â€™" for ’): the next
/// lead-and-continuations sequence from byte `from`, as (start, end, decodes).
/// A sequence decodes back when its character lies where Western text lands
/// ("voilà »" is also à + A0 + BB, a Samaritan letter: not kept).
fn next_mojibake(text: &str, from: usize) -> Option<(usize, usize, bool)> {
    let mut i = from;
    let b = text.as_bytes();
    while i < b.len() {
        // Every character of a sequence is ≥ 0x80 once decoded.
        match b[i..].iter().position(|&x| x >= 0x80) {
            None => return None,
            Some(k) => i += k,
        }
        let c = text[i..].chars().next().unwrap();
        let need = match cp1252_byte(c) {
            Some(0xC2..=0xDF) => 1,
            Some(0xE0..=0xEF) => 2,
            Some(0xF0..=0xF4) => 3,
            _ => 0,
        };
        if need > 0 {
            let mut bytes = vec![cp1252_byte(c).unwrap()];
            let mut end = i + c.len_utf8();
            for d in text[end..].chars().take(need) {
                match cp1252_byte(d) {
                    Some(x @ 0x80..=0xBF) => { bytes.push(x); end += d.len_utf8(); }
                    _ => break,
                }
            }
            if bytes.len() == need + 1 {
                let ok = std::str::from_utf8(&bytes).ok().and_then(|s| s.chars().next()).is_some_and(|ch| {
                    let cp = ch as u32;
                    (0xA0..=0x24F).contains(&cp) || (0x2000..=0x214F).contains(&cp) || cp >= 0x1F000
                });
                return Some((i, end, ok));
            }
        }
        i += c.len_utf8();
    }
    None
}

/// JS `hasMojibake`: a sequence that really decodes, among the first 1000.
pub fn has_mojibake(text: &str) -> bool {
    let mut at = 0;
    for _ in 0..=1000 {
        match next_mojibake(text, at) {
            None => return false,
            Some((_, _, true)) => return true,
            Some((_, e, false)) => at = e,
        }
    }
    false
}

/// The first sequence shaped like garbled accents, decoding or not: (start, end) in bytes.
pub fn find_mojibake(text: &str) -> Option<(usize, usize)> {
    next_mojibake(text, 0).map(|(s, e, _)| (s, e))
}

/// Garbled accents in the file as read: its first 16 MB are plenty to notice them.
pub fn file_mojibake(st: &State) -> bool {
    let b = st.base.bytes();
    if st.base.n == 0 { return false; }
    let mut z = b.len().min(16 << 20);
    if st.base.enc.mem_utf8() {
        while z < b.len() && z > 0 && b[z] & 0xC0 == 0x80 { z -= 1; }
    }
    has_mojibake(&enc::decode(st.base.enc, &b[..z]))
}

/// What an invisible character is called in the row card.
pub fn inv_name(cp: u32) -> &'static str {
    match cp {
        0xA0 => "no-break space",
        0x202F => "narrow no-break space",
        0x2007 => "figure space",
        0x2009 => "thin space",
        0x200B => "zero-width space",
        0x200C => "zero-width non-joiner",
        0x200D => "zero-width joiner",
        0x200E => "left-to-right mark",
        0x200F => "right-to-left mark",
        0x202A => "left-to-right embedding",
        0x202C => "pop directional formatting",
        0x202D => "left-to-right override",
        0xFEFF => "byte order mark",
        0xAD => "soft hyphen",
        _ if cp < 32 => "control character",
        _ => "format character",
    }
}

// ---- Rows --------------------------------------------------------------

/// The field count rows are expected to have: the header's (the first
/// record's) while the columns are the file's own.
pub fn expected_width(st: &State) -> usize {
    if st.cmap.is_none() { st.base.width.max(1) } else { st.ncols }
}

/// A row's field count: a record's from the scan, an edited row's own.
pub fn field_count(st: &State, id: RowId, row: &mut Vec<String>) -> usize {
    if st.pristine(id) {
        let b = &st.base;
        return match b.odd.binary_search_by_key(&id, |&(r, _)| r) {
            Ok(k) => b.odd[k].1 as usize,
            Err(_) => b.width.max(1),
        };
    }
    st.row(id, row);
    row.len()
}

/// Visits a row's cells in column order — a record still as read is split once, not once per column.
#[inline]
pub fn visit_row(st: &State, id: RowId, tmp: &mut Vec<u8>, row: &mut Vec<String>, mut f: impl FnMut(usize, &str)) {
    if st.pristine(id) {
        let b = &st.base;
        let rec = b.record(id as usize);
        if b.dialect.lines {
            f(0, &enc::decode(b.enc, rec));
            return;
        }
        let e = b.enc;
        scan::visit_fields(rec, b.dialect.delim, tmp, |i, v| {
            f(i, &enc::decode(e, v));
            true
        });
        return;
    }
    st.row(id, row);
    for (i, s) in row.iter().enumerate() {
        f(i, s);
    }
}

/// JS `columnKinds`: each column's kind from its first 400 rows (≥ 90 % of the filled cells).
pub fn column_kinds(st: &State) -> Vec<Kind> {
    let w = st.ncols;
    let mut k = vec![[0usize; 3]; w];
    let (mut tmp, mut row) = (Vec::new(), Vec::new());
    for p in 0..st.len().min(400) {
        visit_row(st, st.data_id(p), &mut tmp, &mut row, |c, v| {
            if c >= w { return; }
            match cell_type(v) {
                Kind::Num => k[c][0] += 1,
                Kind::Date => k[c][1] += 1,
                Kind::Text => k[c][2] += 1,
                Kind::Empty => {}
            }
        });
    }
    k.iter()
        .map(|&[n, d, t]| {
            let f = (n + d + t) as f64;
            if f == 0.0 { Kind::Empty } else if n as f64 / f >= 0.9 { Kind::Num } else if d as f64 / f >= 0.9 { Kind::Date } else { Kind::Text }
        })
        .collect()
}

// ---- Numbers written the French way (fr-FR toLocaleString) -------------

fn group(int: &str) -> String {
    let n = int.len();
    let mut out = String::with_capacity(n + n / 3 * 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 { out.push('\u{202f}'); }
        out.push(c);
    }
    out
}

/// A count: `12 345` (narrow no-break space).
pub fn fmt_int(n: usize) -> String {
    group(&n.to_string())
}

/// A number with at most `frac` decimals, trailing zeros dropped: `1 234,5`.
pub fn fmt_frac(x: f64, frac: usize) -> String {
    if !x.is_finite() { return if x.is_nan() { "NaN".into() } else if x > 0.0 { "∞".into() } else { "-∞".into() }; }
    // Half away from zero, as Intl rounds.
    let p = 10f64.powi(frac as i32);
    let r = (x.abs() * p).round() / p;
    let s = format!("{r:.frac$}");
    let (int, dec) = s.split_once('.').unwrap_or((&s, ""));
    let dec = dec.trim_end_matches('0');
    let sign = if x < 0.0 && r != 0.0 { "-" } else { "" };
    if dec.is_empty() { format!("{sign}{}", group(int)) } else { format!("{sign}{},{dec}", group(int)) }
}

/// JS `fmt`: three decimals at most.
pub fn fmt_num(x: f64) -> String {
    fmt_frac(x, 3)
}

/// JS `Number(x.toPrecision(4))`.
pub fn round_sig(x: f64, digits: usize) -> f64 {
    if x == 0.0 || !x.is_finite() { return x; }
    format!("{:.*e}", digits - 1, x).parse().unwrap_or(x)
}

/// A number with at most `digits` significant digits: `1,2`, `0,2`, `1 235`.
pub fn fmt_sig(x: f64, digits: usize) -> String {
    let r = round_sig(x, digits);
    if r == 0.0 || !r.is_finite() { return fmt_frac(r, 0); }
    let mag = r.abs().log10().floor() as i32;
    let frac = (digits as i32 - 1 - mag).max(0) as usize;
    fmt_frac(r, frac)
}

/// A JS number as `String(x)` writes it (enough for the values met here).
pub fn js_num_str(x: f64) -> String {
    if x.is_nan() { return "NaN".into(); }
    if x.is_infinite() { return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() }; }
    if x == x.trunc() && x.abs() < 1e21 { return format!("{}", x as i128); }
    format!("{x}")
}

// ---- Phone countries (the duplicates' phone cross-check) --------------

/// How many digits the country calling code at the start of `d` (digits of an
/// E.164 number, after the +) takes: ITU zones 1 and 7 one digit, the others
/// two or three. 0 when `d` starts with no code.
pub fn calling_code_len(d: &[u8]) -> usize {
    let (a, b) = (d.first().copied().unwrap_or(b'0'), d.get(1).copied().unwrap_or(b'0'));
    match a {
        b'1' | b'7' => 1,
        b'2' => if matches!(b, b'0' | b'7') { 2 } else { 3 },
        b'3' => if matches!(b, b'0'..=b'4' | b'6' | b'9') { 2 } else { 3 },
        b'4' => if b == b'2' { 3 } else { 2 },
        b'5' => if matches!(b, b'1'..=b'8') { 2 } else { 3 },
        b'6' => if matches!(b, b'0'..=b'6') { 2 } else { 3 },
        b'8' => if matches!(b, b'1' | b'2' | b'4' | b'6') { 2 } else { 3 },
        b'9' => if matches!(b, b'0'..=b'5' | b'8') { 2 } else { 3 },
        _ => 0,
    }
}

/// The calling code of a country as a column names it, in French or English,
/// or as its ISO code ("France", "fr", "FRA", "États-Unis", "Belgique"…).
pub fn calling_code(country: &str) -> Option<u16> {
    const T: &[(&str, u16)] = &[
        ("france", 33), ("fr", 33), ("fra", 33), ("metropole", 33), ("france metropolitaine", 33),
        ("belgique", 32), ("belgium", 32), ("be", 32), ("bel", 32), ("belgie", 32),
        ("suisse", 41), ("switzerland", 41), ("schweiz", 41), ("ch", 41), ("che", 41),
        ("luxembourg", 352), ("lu", 352), ("lux", 352), ("monaco", 377), ("mc", 377),
        ("united states", 1), ("united states of america", 1), ("usa", 1), ("us", 1), ("etats unis", 1), ("etats unis d amerique", 1),
        ("canada", 1), ("ca", 1), ("can", 1), ("quebec", 1),
        ("united kingdom", 44), ("uk", 44), ("gb", 44), ("gbr", 44), ("great britain", 44), ("royaume uni", 44), ("england", 44), ("angleterre", 44),
        ("germany", 49), ("allemagne", 49), ("deutschland", 49), ("de", 49), ("deu", 49),
        ("spain", 34), ("espagne", 34), ("espana", 34), ("es", 34), ("esp", 34),
        ("italy", 39), ("italie", 39), ("italia", 39), ("it", 39), ("ita", 39),
        ("portugal", 351), ("pt", 351), ("prt", 351), ("netherlands", 31), ("pays bas", 31), ("nederland", 31), ("nl", 31), ("holland", 31), ("hollande", 31),
        ("ireland", 353), ("irlande", 353), ("ie", 353), ("austria", 43), ("autriche", 43), ("at", 43),
        ("sweden", 46), ("suede", 46), ("se", 46), ("norway", 47), ("norvege", 47), ("no", 47), ("denmark", 45), ("danemark", 45), ("dk", 45),
        ("finland", 358), ("finlande", 358), ("fi", 358), ("poland", 48), ("pologne", 48), ("pl", 48), ("greece", 30), ("grece", 30), ("gr", 30),
        ("czech republic", 420), ("czechia", 420), ("republique tcheque", 420), ("tchequie", 420), ("cz", 420), ("hungary", 36), ("hongrie", 36), ("hu", 36),
        ("romania", 40), ("roumanie", 40), ("ro", 40), ("russia", 7), ("russie", 7), ("ru", 7), ("turkey", 90), ("turquie", 90), ("tr", 90),
        ("morocco", 212), ("maroc", 212), ("ma", 212), ("algeria", 213), ("algerie", 213), ("dz", 213), ("tunisia", 216), ("tunisie", 216), ("tn", 216),
        ("senegal", 221), ("sn", 221), ("cote d ivoire", 225), ("ivory coast", 225), ("ci", 225), ("cameroun", 237), ("cameroon", 237), ("cm", 237),
        ("reunion", 262), ("la reunion", 262), ("re", 262), ("mayotte", 262), ("guadeloupe", 590), ("gp", 590), ("martinique", 596), ("mq", 596), ("guyane", 594), ("french guiana", 594),
        ("nouvelle caledonie", 687), ("new caledonia", 687), ("polynesie francaise", 689), ("french polynesia", 689),
        ("australia", 61), ("australie", 61), ("au", 61), ("new zealand", 64), ("nouvelle zelande", 64), ("nz", 64),
        ("japan", 81), ("japon", 81), ("jp", 81), ("china", 86), ("chine", 86), ("cn", 86), ("india", 91), ("inde", 91), ("in", 91),
        ("brazil", 55), ("bresil", 55), ("br", 55), ("mexico", 52), ("mexique", 52), ("mx", 52), ("argentina", 54), ("argentine", 54), ("ar", 54),
        ("israel", 972), ("il", 972), ("lebanon", 961), ("liban", 961), ("lb", 961), ("united arab emirates", 971), ("emirats arabes unis", 971), ("uae", 971), ("ae", 971),
        ("singapore", 65), ("singapour", 65), ("sg", 65), ("hong kong", 852), ("hk", 852), ("south africa", 27), ("afrique du sud", 27), ("za", 27),
    ];
    let s = slugify(country);
    if s.is_empty() { return None; }
    T.iter().find(|(n, _)| *n == s).map(|t| t.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(cell_type(" 1 234,50 € "), Kind::Num);
        assert_eq!(cell_type("1.5e3"), Kind::Num);
        assert_eq!(cell_type("15/03/2024"), Kind::Date);
        assert_eq!(cell_type("2025-12-10T17:26:15.000Z"), Kind::Date);
        assert_eq!(cell_type("2024-03-15 10:20+01:00"), Kind::Date);
        assert_eq!(cell_type("7; rue Haute"), Kind::Text);
        assert_eq!(cell_type("abc"), Kind::Text);
        assert_eq!(cell_type("  "), Kind::Empty);
        assert!(!is_numeric_like("1..2"));
        assert!(!is_numeric_like("1.2.3e4"));
    }

    #[test]
    fn keys() {
        assert_eq!(num_key("1 234,50"), 1234.5);
        assert_eq!(num_key("1.234,5"), 1234.5);
        assert_eq!(num_key("1,234.5"), 1234.5);
        assert_eq!(num_key("-0,5"), -0.5);
        assert_eq!(num_key("1,234,567"), 1234567.0);
        assert_eq!(num_key("12.5€"), 12.5);
        assert!(num_key("abc").is_nan());
        assert_eq!(date_key("15/03/2024"), 20240315000000.0);
        assert_eq!(date_key("2024-03-15T10:20"), 20240315102000.0);
        assert_eq!(date_key("15/03/24"), 20240315000000.0);
        assert_eq!(key_days(19700102000000.0), 1.0);
        assert_eq!(key_days(20000301000000.0) - key_days(20000228000000.0), 2.0);
    }

    #[test]
    fn calling_codes() {
        assert_eq!(calling_code(" États-Unis "), Some(1));
        assert_eq!(calling_code("FR"), Some(33));
        assert_eq!(calling_code("Royaume-Uni"), Some(44));
        assert_eq!(calling_code("Atlantis"), None);
        assert_eq!(calling_code_len(b"33612345678"), 2);
        assert_eq!(calling_code_len(b"12038933004"), 1);
        assert_eq!(calling_code_len(b"352621123456"), 3);
        assert_eq!(calling_code_len(b"420601123456"), 3);
    }

    #[test]
    fn names() {
        assert_eq!(slugify("Code postal (CP)"), "code postal cp");
        assert_eq!(slugify("N° Société"), "n societe");
        assert!(id_like("id"));
        assert!(id_like("E-mail"));
        assert!(id_like("customer_id"));
        assert!(!id_like("code postal"));
        assert!(!id_like("identity"));
        assert!(code_like("cp"));
        assert!(code_like("Tél."));
        assert!(!code_like("montant"));
    }

    #[test]
    fn mojibake() {
        assert!(has_mojibake("Ã©cole"));
        assert!(has_mojibake("lâ€™Ã©tÃ©"));
        assert!(!has_mojibake("école, voilà »"));
        assert_eq!(find_mojibake("x Ã© y"), Some((2, 6)));
        assert_eq!(cp1252_byte('€'), Some(0x80));
    }

    #[test]
    fn french_numbers() {
        assert_eq!(fmt_int(1234567), "1\u{202f}234\u{202f}567");
        assert_eq!(fmt_num(2.5), "2,5");
        assert_eq!(fmt_sig(0.2, 4), "0,2");
        assert_eq!(fmt_sig(1.2, 4), "1,2");
        assert_eq!(fmt_sig(1234.56, 4), "1\u{202f}235");
        assert_eq!(fmt_frac(-3.14159, 4), "-3,1416");
        assert_eq!(round_sig(1.19999, 4), 1.2);
        assert_eq!(js_num_str(5000.0), "5000");
    }
}
