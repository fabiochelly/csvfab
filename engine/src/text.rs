//! Text helpers: accent and case folding, numbers and dates as written in
//! French and English files.

use unicode_normalization::char::decompose_canonical;

/// Lower case without accents ("Émile" → "emile"), appended to `out`.
#[inline]
pub fn fold_into(s: &str, out: &mut String) {
    let b = s.as_bytes();
    // SAFETY (each as_mut_vec): only ASCII bytes and whole characters' folds are pushed.
    if s.is_ascii() {
        unsafe { out.as_mut_vec() }.extend(b.iter().map(u8::to_ascii_lowercase));
        return;
    }
    let mut i = 0;
    while i < b.len() {
        // ASCII runs in bulk; the rest one character at a time.
        let start = i;
        i += find_high(&b[i..]).unwrap_or(b.len() - i);
        unsafe { out.as_mut_vec() }.extend(b[start..i].iter().map(u8::to_ascii_lowercase));
        if i >= b.len() { break; }
        let c = s[i..].chars().next().unwrap();
        i += c.len_utf8();
        let u = c as u32;
        if u < LATIN_END {
            let (len, bytes) = &latin()[(u - 0x80) as usize];
            if *len > 0 {
                unsafe { out.as_mut_vec() }.extend_from_slice(&bytes[..*len as usize]);
                continue;
            }
        }
        fold_char(c, out);
    }
}

/// The first byte ≥ 0x80, 32 bytes at a time.
#[inline]
pub fn find_high(b: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 32 <= b.len() {
        let w = |k: usize| u64::from_le_bytes(b[i + k..i + k + 8].try_into().unwrap());
        if (w(0) | w(8) | w(16) | w(24)) & 0x8080_8080_8080_8080 != 0 { break; }
        i += 32;
    }
    b[i..].iter().position(|&c| c >= 0x80).map(|k| i + k)
}

/// The first byte from `i` on that starts a character of two bytes or more (≥ 0xC0), 8 bytes
/// at a time: in valid UTF-8, where the non-ASCII characters are.
#[inline]
pub fn find_lead(b: &[u8], mut i: usize) -> Option<usize> {
    while i + 8 <= b.len() {
        let w = u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        // Bits 7 and 6 of a byte both set (bit 6 shifted onto bit 7 of the same byte).
        let m = w & (w << 1) & 0x8080_8080_8080_8080;
        if m != 0 { return Some(i + (m.trailing_zeros() / 8) as usize); }
        i += 8;
    }
    b[i..].iter().position(|&c| c >= 0xC0).map(|k| i + k)
}

/// The fold of one non-ASCII character (the reference: `fold_into`'s table is built from it).
fn fold_char(c: char, out: &mut String) {
    match c {
        'œ' | 'Œ' => out.push_str("oe"),
        'æ' | 'Æ' => out.push_str("ae"),
        'ß' => out.push_str("ss"),
        '\u{a0}' | '\u{202f}' => out.push(' '),
        _ => {
            let mut first = None;
            decompose_canonical(c, |d| {
                if first.is_none() { first = Some(d) }
            });
            let base = first.unwrap_or(c);
            for l in base.to_lowercase() {
                out.push(l);
            }
        }
    }
}

/// Latin-1 and Latin Extended A/B: the characters of European files, folded
/// once (from `fold_char`, so identical) instead of decomposed each time.
const LATIN_END: u32 = 0x250;

fn latin() -> &'static [(u8, [u8; 7])] {
    static T: std::sync::OnceLock<Vec<(u8, [u8; 7])>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        (0x80..LATIN_END)
            .map(|u| {
                let mut s = String::new();
                if let Some(c) = char::from_u32(u) { fold_char(c, &mut s); }
                let mut b = [0u8; 7];
                if s.is_empty() || s.len() > 7 { return (0, b); }
                b[..s.len()].copy_from_slice(s.as_bytes());
                (s.len() as u8, b)
            })
            .collect()
    })
}

/// Whether some non-ASCII character of `s` folds to text holding an ASCII byte
/// of `mask` (bit b: byte b). If none does, an ASCII needle made of those bytes
/// is in fold(s) only where it is in s lower-cased.
pub fn folds_into_any(s: &str, mask: u128) -> bool {
    s.chars().any(|c| !c.is_ascii() && folds_into(c, mask))
}

/// Whether this (non-ASCII) character folds to text holding an ASCII byte of `mask`.
#[inline]
pub fn folds_into(c: char, mask: u128) -> bool {
    let u = c as u32;
    if (0x80..LATIN_END).contains(&u) {
        let (len, bytes) = &latin()[(u - 0x80) as usize];
        if *len > 0 {
            return bytes[..*len as usize].iter().any(|&x| x < 0x80 && mask >> x & 1 == 1);
        }
    }
    let mut tmp = String::new();
    fold_char(c, &mut tmp);
    tmp.bytes().any(|x| x < 0x80 && mask >> x & 1 == 1)
}

pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    fold_into(s, &mut out);
    out
}

/// Marks a digit run in a sort key: after spaces and punctuation, before letters.
const DIGITS: u8 = 0x41;

/// The sort key of a folded, trimmed text, appended to `out`: byte order of keys is the order
/// the web app's French numeric collator gave — accents and case ignored (the fold), digit runs
/// compared as numbers ("lot 2" < "lot 10", leading zeros aside), spaces and punctuation before
/// digits, digits before letters. A digit run is the marker, its length, its digits. No key
/// byte is 0xFF (`radix::text_order` keeps that for the empty key).
pub fn sort_key_into(f: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    while i < f.len() {
        let c = f[i];
        if c.is_ascii_digit() {
            let s = i;
            while i < f.len() && f[i].is_ascii_digit() { i += 1; }
            let mut d = &f[s..i];
            while d.len() > 1 && d[0] == b'0' { d = &d[1..]; }
            out.push(DIGITS);
            out.push(d.len().min(0xFE) as u8);
            out.extend_from_slice(d);
            continue;
        }
        // The ASCII punctuation above the digits moved below them (the fold leaves no capital
        // letter: 0x41-0x5A are free for the marker).
        out.push(match c {
            0x3A..=0x40 => c - 0x3A + 0x30,
            0x5B..=0x60 => c - 0x5B + 0x37,
            0x7B..=0x7E => c - 0x7B + 0x3D,
            _ => c,
        });
        i += 1;
    }
}

/// What tells apart texts of equal sort keys, as the web's collator did (ICU, French): accents
/// first ("e" < "é" < "è" < "ê" < "ë"), then case (lower first), position by position; appended
/// to `out` for a trimmed cell as written. One accent byte per character (1: none), 0, one case
/// byte per character.
pub fn tie_key_into(s: &str, out: &mut Vec<u8>) {
    if s.is_ascii() {
        out.extend(std::iter::repeat_n(1, s.len()));
        out.push(0);
        out.extend(s.bytes().map(|c| c.is_ascii_uppercase() as u8));
        return;
    }
    // ICU's order of the marks; other marks after them, letters that fold to others (œ, ß) last.
    const MARKS: [char; 13] = ['\u{301}', '\u{300}', '\u{306}', '\u{302}', '\u{30c}', '\u{30a}', '\u{308}', '\u{30b}', '\u{303}', '\u{307}', '\u{327}', '\u{328}', '\u{304}'];
    for c in s.chars() {
        out.push(if c.is_ascii() {
            1
        } else {
            let mut mark = None;
            decompose_canonical(c, |d| if mark.is_none() && d != c && !d.is_alphanumeric() { mark = Some(d) });
            match (mark, c) {
                (Some(m), _) => 2 + MARKS.iter().position(|&k| k == m).unwrap_or(MARKS.len()) as u8,
                (None, 'œ' | 'Œ' | 'æ' | 'Æ' | 'ß' | '\u{a0}' | '\u{202f}') => 0x40,
                (None, _) => 1,
            }
        });
    }
    out.push(0);
    out.extend(s.chars().map(|c| c.is_uppercase() as u8));
}

/// `sort_key_into` of a cell as written: trimmed and folded first.
pub fn sort_key(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 4);
    sort_key_into(fold(s.trim()).as_bytes(), &mut out);
    out
}

/// A number as people write it: `1234.5`, `1 234,50 €`, `-12,5 %`, `1,234,567`,
/// `(42)` for -42. None if the text is not one.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t.len() > 40 {
        return None;
    }
    let b = t.as_bytes();
    // Fast path: plain ASCII number.
    if b.iter().all(|&c| c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+') {
        if !b.iter().any(|c| c.is_ascii_digit()) {
            return None;
        }
        return t.parse::<f64>().ok();
    }
    let mut neg = false;
    // The digits, separators and exponent kept, on the stack (at most one byte per byte of `t`).
    let mut digits = Ascii::default();
    let (mut commas, mut dots, mut last_sep) = (0, 0, ' ');
    let mut seen_digit = false;
    let mut chars = t.chars().peekable();
    let mut paren = false;
    while let Some(c) = chars.next() {
        match c {
            '0'..='9' => { digits.push(c); seen_digit = true; }
            ',' => { commas += 1; last_sep = ','; digits.push(','); }
            '.' => { dots += 1; last_sep = '.'; digits.push('.'); }
            '-' | '\u{2212}' if !seen_digit => neg = !neg,
            '+' if !seen_digit => {}
            '(' if !seen_digit => paren = true,
            ')' if paren => paren = false,
            ' ' | '\u{a0}' | '\u{202f}' | '\'' | '_' => {
                if seen_digit { if let Some(n) = chars.peek() { if !n.is_ascii_digit() && !matches!(n, '€' | '$' | '£' | '%' | ' ') { return None } } }
            }
            '€' | '$' | '£' | '%' | '¥' => {}
            'e' | 'E' if seen_digit => {
                digits.push('e');
                if let Some(&n) = chars.peek() { if n == '-' || n == '+' { digits.push(n); chars.next(); } }
            }
            _ => return None,
        }
    }
    if !seen_digit || paren {
        return None;
    }
    if t.starts_with('(') && t.ends_with(')') {
        neg = !neg;
    }
    // Which separator is the decimal one: the last one when both appear; a lone
    // comma is decimal unless it groups thousands more than once (1,234,567).
    let dec = if commas > 0 && dots > 0 {
        last_sep
    } else if commas > 1 {
        '.'
    } else if commas == 1 {
        ','
    } else if dots > 1 {
        ','
    } else {
        '.'
    };
    let mut clean = Ascii::default();
    for &c in &digits.b[..digits.n] {
        match c {
            b',' | b'.' if c as char == dec => clean.push('.'),
            b',' | b'.' => {}
            _ => clean.push(c as char),
        }
    }
    let v: f64 = clean.as_str().parse().ok()?;
    Some(if neg { -v } else { v })
}

/// A few ASCII characters on the stack (`parse_number`'s 40 bytes and an exponent's sign).
struct Ascii {
    b: [u8; 48],
    n: usize,
}

impl Default for Ascii {
    fn default() -> Ascii {
        Ascii { b: [0; 48], n: 0 }
    }
}

impl Ascii {
    #[inline]
    fn push(&mut self, c: char) {
        self.b[self.n] = c as u8;
        self.n += 1;
    }

    fn as_str(&self) -> &str {
        // Only ASCII is pushed.
        std::str::from_utf8(&self.b[..self.n]).unwrap_or("")
    }
}

/// A date (and time) as a sortable number YYYYMMDDhhmmss: `2024-03-15`,
/// `15/03/2024`, `15.03.2024`, `15-03-2024`, with an optional `hh:mm[:ss]`.
pub fn parse_date(s: &str) -> Option<f64> {
    let t = s.trim().as_bytes();
    if t.len() < 8 || t.len() > 32 || !t[0].is_ascii_digit() {
        return None;
    }
    let mut nums = [0u32; 6];
    let mut lens = [0usize; 6];
    let mut seps = [0u8; 6];
    let mut k = 0;
    for &c in t {
        if c.is_ascii_digit() {
            if lens[k] >= 4 { return None; }
            nums[k] = nums[k] * 10 + (c - b'0') as u32;
            lens[k] += 1;
        } else {
            if lens[k] == 0 { if c == b' ' || c == b'Z' { continue; } return None; }
            if k == 5 { if c == b'.' || c == b'Z' || c == b'+' { break } return None; }
            seps[k] = c;
            k += 1;
        }
    }
    let n = if lens[k] > 0 { k + 1 } else { k };
    if n < 3 {
        return None;
    }
    let (y, m, d) = if lens[0] == 4 && matches!(seps[0], b'-' | b'/' | b'.') && seps[1] == seps[0] {
        (nums[0], nums[1], nums[2])
    } else if lens[2] == 4 && matches!(seps[0], b'/' | b'.' | b'-') && seps[1] == seps[0] {
        (nums[2], nums[1], nums[0])
    } else {
        return None;
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1000..=9999).contains(&y) {
        return None;
    }
    let (hh, mi, ss) = (nums[3], nums[4], nums[5]);
    if n > 3 && (hh > 23 || mi > 59 || ss > 60) {
        return None;
    }
    Some(((((y as f64 * 100.0 + m as f64) * 100.0 + d as f64) * 100.0 + hh as f64) * 100.0 + mi as f64) * 100.0 + ss as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_keys_read_numbers_and_ignore_accents() {
        let mut v = vec!["Lot 10", "lot 2", "Lot 1", "lot 02b", "Émile", "emma", "zoé", "Zoe", "lot", "#3", "_x", "10", "9", "a-b", "a b", "ab"];
        v.sort_by_key(|s| sort_key(s));
        assert_eq!(v, ["#3", "_x", "9", "10", "a b", "a-b", "ab", "Émile", "emma", "lot", "Lot 1", "lot 2", "lot 02b", "Lot 10", "zoé", "Zoe"]);
        assert!(!sort_key(&"9".repeat(400)).contains(&0xFF));
        assert_eq!(sort_key("007"), sort_key("7"));
        assert!(sort_key("0") < sort_key("1") && sort_key("00") == sort_key("0"));
    }

    #[test]
    fn equal_keys_by_accents_then_case() {
        let key = |s: &str| {
            let mut t = Vec::new();
            tie_key_into(s, &mut t);
            (sort_key(s), t)
        };
        let mut v = vec!["Émile", "émile", "Emile", "emile", "èmile", "EMILE", "œuvre", "oeuvre", "Oeuvre"];
        v.sort_by_key(|s| key(s));
        assert_eq!(v, ["emile", "Emile", "EMILE", "émile", "Émile", "èmile", "oeuvre", "Oeuvre", "œuvre"]);
    }

    #[test]
    fn lead_bytes_found() {
        let s = "abcdefghé€ijklmnopqrstuvwxyz日本 x😀";
        let want: Vec<usize> = s.char_indices().filter(|(_, c)| !c.is_ascii()).map(|(i, _)| i).collect();
        let (b, mut got, mut i) = (s.as_bytes(), Vec::new(), 0);
        while let Some(p) = find_lead(b, i) { got.push(p); i = p + 1; }
        assert_eq!(got, want);
    }

    #[test]
    fn folds() {
        assert_eq!(fold("Émile ÇA Œuvre"), "emile ca oeuvre");
        assert_eq!(fold("Müller"), "muller");
    }

    /// The old fold, character by character: the fast one must agree on every character.
    fn fold_ref(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            if c.is_ascii() { out.push(c.to_ascii_lowercase()) } else { fold_char(c, &mut out) }
        }
        out
    }

    #[test]
    fn fold_every_char() {
        for u in 0..0x110000u32 {
            let Some(c) = char::from_u32(u) else { continue };
            let s = format!("aB{c}Zé{c}");
            assert_eq!(fold(&s), fold_ref(&s), "U+{u:04X}");
            // The column caches keep fold(cell) and trim it later: fold(s.trim()) == fold(s).trim()
            // holds when every whitespace folds to whitespace and nothing else folds to a
            // string with whitespace at either end.
            let f = fold(&c.to_string());
            if c.is_whitespace() {
                assert!(!f.is_empty() && f.chars().all(char::is_whitespace), "U+{u:04X}");
            } else {
                assert!(!f.starts_with(char::is_whitespace) && !f.ends_with(char::is_whitespace) && !f.is_empty(), "U+{u:04X}");
            }
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("1234.5"), Some(1234.5));
        assert_eq!(parse_number("1 234,50 €"), Some(1234.5));
        assert_eq!(parse_number("-12,5 %"), Some(-12.5));
        assert_eq!(parse_number("1,234,567"), Some(1234567.0));
        assert_eq!(parse_number("1.234,5"), Some(1234.5));
        assert_eq!(parse_number("(42)"), Some(-42.0));
        assert_eq!(parse_number("75011"), Some(75011.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("12 rue"), None);
        assert_eq!(parse_number("-"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("2024-03-15"), Some(20240315000000.0));
        assert_eq!(parse_date("15/03/2024"), Some(20240315000000.0));
        assert_eq!(parse_date("2024-03-15T10:20:30Z"), Some(20240315102030.0));
        assert_eq!(parse_date("15/03/2024 10:20"), Some(20240315102000.0));
        assert_eq!(parse_date("13/13/2024"), None);
        assert_eq!(parse_date("12345678"), None);
    }
}
