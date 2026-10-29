//! Values of the formula language: JavaScript's, as far as a formula can reach
//! them — its coercions, comparisons and number formatting, so a formula gives
//! what it gave in the web app.

use super::date;
use super::jsre::Rx;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub enum Val<'a> {
    Undef,
    Null,
    Bool(bool),
    Num(f64),
    /// Borrowed from a cell or a literal whenever possible: most formulas never copy text.
    Str(Cow<'a, str>),
    /// Milliseconds since 1970 UTC; NaN: an Invalid Date.
    Date(f64),
    Re(&'a Rx),
    List(Vec<Val<'a>>),
}

pub type R<'a> = Result<Val<'a>, String>;

pub static UNDEF: Val<'static> = Val::Undef;

impl<'a> Val<'a> {
    pub fn str(s: &'a str) -> Val<'a> {
        Val::Str(Cow::Borrowed(s))
    }

    pub fn owned(s: String) -> Val<'a> {
        Val::Str(Cow::Owned(s))
    }

    pub fn nullish(&self) -> bool {
        matches!(self, Val::Undef | Val::Null)
    }

    /// A copy borrowing from this one (a constant of the compiled formula).
    pub fn reborrow(&self) -> Val<'_> {
        match self {
            Val::Str(s) => Val::Str(Cow::Borrowed(s)),
            Val::List(l) => Val::List(l.iter().map(|v| v.reborrow()).collect()),
            Val::Re(r) => Val::Re(r),
            Val::Undef => Val::Undef,
            Val::Null => Val::Null,
            Val::Bool(b) => Val::Bool(*b),
            Val::Num(n) => Val::Num(*n),
            Val::Date(d) => Val::Date(*d),
        }
    }

    /// The value with nothing borrowed, to be kept as a constant; None for a regex.
    pub fn into_static(self) -> Option<Val<'static>> {
        Some(match self {
            Val::Str(s) => Val::Str(Cow::Owned(s.into_owned())),
            Val::List(l) => Val::List(l.into_iter().map(|v| v.into_static()).collect::<Option<_>>()?),
            Val::Re(_) => return None,
            Val::Undef => Val::Undef,
            Val::Null => Val::Null,
            Val::Bool(b) => Val::Bool(b),
            Val::Num(n) => Val::Num(n),
            Val::Date(d) => Val::Date(d),
        })
    }

    /// `typeof`-like name, for messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Val::Undef => "undefined",
            Val::Null => "null",
            Val::Bool(_) => "boolean",
            Val::Num(_) => "number",
            Val::Str(_) => "string",
            Val::Date(_) => "date",
            Val::Re(_) => "regex",
            Val::List(_) => "list",
        }
    }
}

/// JavaScript's white space and line terminators (what \s and trim() take).
#[inline]
pub fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[inline]
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

pub fn truthy(v: &Val) -> bool {
    match v {
        Val::Undef | Val::Null => false,
        Val::Bool(b) => *b,
        Val::Num(n) => *n != 0.0 && !n.is_nan(),
        Val::Str(s) => !s.is_empty(),
        _ => true,
    }
}

/// ToNumber.
pub fn to_num(v: &Val) -> f64 {
    match v {
        Val::Num(n) => *n,
        Val::Undef => f64::NAN,
        Val::Null => 0.0,
        Val::Bool(b) => *b as u8 as f64,
        Val::Str(s) => str_to_num(s),
        Val::Date(t) => *t,
        Val::Re(_) => f64::NAN,
        Val::List(_) => str_to_num(&to_str(v)),
    }
}

/// ToString.
pub fn to_str<'b>(v: &'b Val) -> Cow<'b, str> {
    match v {
        Val::Str(s) => Cow::Borrowed(s),
        Val::Undef => Cow::Borrowed("undefined"),
        Val::Null => Cow::Borrowed("null"),
        Val::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        _ => {
            let mut out = String::new();
            write_str(v, &mut out);
            Cow::Owned(out)
        }
    }
}

pub fn write_str(v: &Val, out: &mut String) {
    match v {
        Val::Str(s) => out.push_str(s),
        Val::Num(n) => fmt_num(*n, out),
        Val::Undef => out.push_str("undefined"),
        Val::Null => out.push_str("null"),
        Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Val::Date(t) => date::to_string(*t, out),
        Val::Re(r) => {
            out.push('/');
            out.push_str(&r.src);
            out.push('/');
            out.push_str(&r.flags);
        }
        Val::List(l) => {
            for (i, x) in l.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if !x.nullish() {
                    write_str(x, out);
                }
            }
        }
    }
}

/// The helpers' reading of a value as text (cellStr): "" for null and undefined.
pub fn cell_str<'b>(v: &'b Val) -> Cow<'b, str> {
    if v.nullish() { Cow::Borrowed("") } else { to_str(v) }
}

/// The same, taking the value: a borrowed cell stays borrowed.
pub fn into_cell_str(v: Val) -> Cow<str> {
    match v {
        Val::Str(s) => s,
        Val::Undef | Val::Null => Cow::Borrowed(""),
        v => Cow::Owned(to_str(&v).into_owned()),
    }
}

/// ToPrimitive: a date reads as its text unless a number is wanted; a list or a regex as its text.
pub fn to_prim(v: Val, number: bool) -> Val {
    match v {
        Val::Date(t) if number => Val::Num(t),
        Val::Date(_) | Val::List(_) | Val::Re(_) => Val::Str(Cow::Owned(to_str(&v).into_owned())),
        v => v,
    }
}

/// StringToNumber: "" is 0, "0x1F" is 31, anything else not a whole number literal is NaN.
pub fn str_to_num(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let b = t.as_bytes();
    if b.len() > 2 && b[0] == b'0' && matches!(b[1] | 0x20, b'x' | b'o' | b'b') {
        let radix = match b[1] | 0x20 { b'x' => 16, b'o' => 8, _ => 2 };
        let mut v = 0f64;
        for &c in &b[2..] {
            match (c as char).to_digit(radix) {
                Some(d) => v = v * radix as f64 + d as f64,
                None => return f64::NAN,
            }
        }
        return v;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    // Once "inf", "nan" and other letters are ruled out, Rust's grammar is JavaScript's.
    if !b.iter().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return f64::NAN;
    }
    t.parse().unwrap_or(f64::NAN)
}

/// parseFloat: the longest number at the start of the text.
pub fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(is_space);
    let b = t.as_bytes();
    let mut i = 0;
    let neg = matches!(b.first(), Some(b'-'));
    if matches!(b.first(), Some(b'-' | b'+')) {
        i = 1;
    }
    if t[i..].starts_with("Infinity") {
        return if neg { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let d0 = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int = i - d0;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if int > 0 || j > i + 1 {
            i = j;
        }
    }
    if i == d0 || (int == 0 && i == d0 + 1) {
        return f64::NAN;
    }
    if i < b.len() && (b[i] | 0x20) == b'e' {
        let mut j = i + 1;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        let e0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > e0 {
            i = j;
        }
    }
    t[..i].parse().unwrap_or(f64::NAN)
}

/// num(): a cell as a number the way people write it — 1 234,50 and 1,234.50
/// both read, currency and percent signs ignored, an empty cell is 0 (as in a
/// spreadsheet), trailing text ignored ("12 kg" is 12).
pub fn fx_num_str(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    if t.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'-' | b'+' | b'e' | b'E')) {
        return parse_float(t);
    }
    // Spaces and currency out; any other non-ASCII character becomes '#', which stops the number
    // where it stood. Short texts on the stack: a column of "1234,50" allocates nothing.
    let mut stack = [0u8; 64];
    let mut heap = Vec::new();
    let mut n = 0;
    {
        let mut push = |c: u8| {
            if n < stack.len() { stack[n] = c } else {
                if heap.is_empty() { heap.extend_from_slice(&stack) }
                heap.push(c)
            }
            n += 1;
        };
        for c in t.chars() {
            match c {
                '€' | '$' | '£' | '%' => {}
                c if is_space(c) => {}
                c if c.is_ascii() => push(c as u8),
                _ => push(b'#'),
            }
        }
    }
    let buf: &mut [u8] = if n > stack.len() { &mut heap[..] } else { &mut stack[..n] };
    let (lc, ld) = (buf.iter().rposition(|&c| c == b','), buf.iter().rposition(|&c| c == b'.'));
    let mut len = buf.len();
    let remove = |buf: &mut [u8], what: u8, len: &mut usize| {
        let mut k = 0;
        for i in 0..*len {
            if buf[i] != what { buf[k] = buf[i]; k += 1 }
        }
        *len = k;
    };
    match (lc, ld) {
        (Some(c), Some(d)) => {
            if c > d {
                remove(buf, b'.', &mut len);
                if let Some(p) = buf[..len].iter().position(|&x| x == b',') { buf[p] = b'.' }
            } else {
                remove(buf, b',', &mut len);
            }
        }
        (Some(c), None) if decimal_comma(buf, c) => buf[c] = b'.',
        _ => remove(buf, b',', &mut len),
    }
    // Only ASCII is left.
    parse_float(std::str::from_utf8(&buf[..len]).unwrap_or(""))
}

/// ^[-+]?\d+,\d+$ with the comma at `c`.
fn decimal_comma(b: &[u8], c: usize) -> bool {
    let s = if matches!(b[0], b'-' | b'+') { 1 } else { 0 };
    c > s && c + 1 < b.len() && b[s..c].iter().all(u8::is_ascii_digit) && b[c + 1..].iter().all(u8::is_ascii_digit)
}

/// Number::toString: the shortest digits that read back the same, JavaScript's layout.
pub fn fmt_num(x: f64, out: &mut String) {
    if x.is_nan() {
        return out.push_str("NaN");
    }
    if x == 0.0 {
        return out.push('0');
    }
    if x.is_infinite() {
        return out.push_str(if x > 0.0 { "Infinity" } else { "-Infinity" });
    }
    if x.fract() == 0.0 && x.abs() < 1e15 {
        let _ = write!(out, "{}", x as i64);
        return;
    }
    // Rust's {:e} gives the shortest round-trip digits: d.ddde±n.
    let mut buf = [0u8; 40];
    let len = {
        let mut c = std::io::Cursor::new(&mut buf[..]);
        let _ = std::io::Write::write_fmt(&mut c, format_args!("{:e}", x.abs()));
        c.position() as usize
    };
    let s = std::str::from_utf8(&buf[..len]).unwrap_or("0e0");
    let (mant, exp) = s.split_once('e').unwrap_or((s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut digits = [0u8; 24];
    let mut k = 0;
    for &c in mant.as_bytes() {
        if c != b'.' && k < digits.len() {
            digits[k] = c;
            k += 1;
        }
    }
    let d = std::str::from_utf8(&digits[..k]).unwrap_or("0");
    let n = exp + 1;
    let k = k as i32;
    if x < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(d);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&d[..n as usize]);
        out.push('.');
        out.push_str(&d[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(d);
    } else {
        out.push_str(&d[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&d[1..]);
        }
        let _ = write!(out, "e{}{}", if n > 0 { '+' } else { '-' }, (n - 1).abs());
    }
}

pub fn num_string(x: f64) -> String {
    let mut s = String::new();
    fmt_num(x, &mut s);
    s
}

/// The number of binary digits after the point: a double's exact decimal
/// expansion has that many decimals, the last a 5.
fn frac_bits(a: f64) -> usize {
    if a == 0.0 || !a.is_finite() {
        return 0;
    }
    let bits = a.to_bits();
    let ex = ((bits >> 52) & 0x7ff) as i32;
    let mut m = bits & ((1u64 << 52) - 1);
    let e = if ex == 0 { -1074 } else { m |= 1 << 52; ex - 1075 };
    let e2 = e + m.trailing_zeros() as i32;
    if e2 >= 0 { 0 } else { (-e2) as usize }
}

/// Adds one unit in the last digit of a decimal text ("1.29" → "1.30", "9.9" → "10.0").
fn bump(s: &mut String) {
    let mut b = std::mem::take(s).into_bytes();
    let mut i = b.len();
    loop {
        if i == 0 {
            b.insert(0, b'1');
            break;
        }
        i -= 1;
        match b[i] {
            b'.' => continue,
            b'9' => b[i] = b'0',
            c => {
                b[i] = c + 1;
                break;
            }
        }
    }
    *s = String::from_utf8(b).unwrap_or_default();
}

/// Number::toFixed. Rust rounds exact ties to even, JavaScript up: (2.5).toFixed(0) is "3".
pub fn to_fixed(x: f64, d: f64) -> Result<String, String> {
    if !(0.0..=100.0).contains(&d) {
        return Err("toFixed() digits argument must be between 0 and 100".into());
    }
    let d = d as usize;
    if !x.is_finite() || x.abs() >= 1e21 {
        return Ok(num_string(x));
    }
    let a = x.abs();
    let mut s = if frac_bits(a) == d + 1 {
        let mut s = format!("{:.*}", d + 1, a);
        s.pop();
        if s.ends_with('.') {
            s.pop();
        }
        bump(&mut s);
        s
    } else {
        format!("{:.*}", d, a)
    };
    if x < 0.0 {
        s.insert(0, '-');
    }
    Ok(s)
}

/// Number::toPrecision, from the exact decimal expansion.
pub fn to_precision(x: f64, p: f64) -> Result<String, String> {
    if !x.is_finite() {
        return Ok(num_string(x));
    }
    if !(1.0..=100.0).contains(&p) {
        return Err("toPrecision() argument must be between 1 and 100".into());
    }
    let p = p as usize;
    let a = x.abs();
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    if a == 0.0 {
        out.push('0');
        if p > 1 {
            out.push('.');
            out.extend(std::iter::repeat_n('0', p - 1));
        }
        return Ok(out);
    }
    let exact = format!("{:.*}", frac_bits(a), a);
    let (ip, fp) = exact.split_once('.').unwrap_or((&exact, ""));
    let all: Vec<u8> = ip.bytes().chain(fp.bytes()).collect();
    let lead = all.iter().position(|&c| c != b'0').unwrap_or(0);
    let mut e = ip.len() as i32 - 1 - lead as i32;
    let mut dg: Vec<u8> = all[lead..].iter().copied().chain(std::iter::repeat(b'0')).take(p).collect();
    if all.len() > lead + p && all[lead + p] >= b'5' {
        // half up: a 5 followed by anything rounds up, ties included
        let mut i = p;
        loop {
            if i == 0 {
                dg.insert(0, b'1');
                dg.pop();
                e += 1;
                break;
            }
            i -= 1;
            if dg[i] == b'9' { dg[i] = b'0' } else { dg[i] += 1; break }
        }
    }
    let d = String::from_utf8(dg).unwrap_or_default();
    if e < -6 || e >= p as i32 {
        out.push_str(&d[..1]);
        if p > 1 {
            out.push('.');
            out.push_str(&d[1..]);
        }
        let _ = write!(out, "e{}{}", if e >= 0 { '+' } else { '-' }, e.abs());
    } else if e >= 0 {
        let e = e as usize;
        out.push_str(&d[..e + 1]);
        if p > e + 1 {
            out.push('.');
            out.push_str(&d[e + 1..]);
        }
    } else {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-e - 1) as usize));
        out.push_str(&d);
    }
    Ok(out)
}

/// Number::toString(radix).
pub fn to_radix(x: f64, radix: u32) -> String {
    if radix == 10 || !x.is_finite() {
        return num_string(x);
    }
    let digit = |d: u32| std::char::from_digit(d, radix).unwrap_or('0');
    let a = x.abs();
    let mut ip = a.trunc();
    let mut fp = a - ip;
    let mut int = Vec::new();
    loop {
        int.push(digit((ip % radix as f64) as u32));
        ip = (ip / radix as f64).trunc();
        if ip < 1.0 { break }
    }
    let mut out: String = if x < 0.0 { "-".into() } else { String::new() };
    out.extend(int.iter().rev());
    if fp > 0.0 {
        out.push('.');
        for _ in 0..52 {
            fp *= radix as f64;
            let d = fp.trunc();
            out.push(digit(d as u32));
            fp -= d;
            if fp <= 0.0 { break }
        }
    }
    out
}

/// Math.round: halves go up, toward +∞ (Math.round(-2.5) is -2).
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// ToIntegerOrInfinity.
pub fn to_int(x: f64) -> f64 {
    if x.is_nan() { 0.0 } else { x.trunc() }
}

/// Strings compare by UTF-16 code units: byte order is the same except
/// between U+E000..U+FFFF and the astral planes.
pub fn cmp_str(a: &str, b: &str) -> Ordering {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    match x.iter().zip(y).position(|(p, q)| p != q) {
        None => x.len().cmp(&y.len()),
        Some(i) if x[i] < 0xee && y[i] < 0xee => x[i].cmp(&y[i]),
        Some(_) => a.encode_utf16().cmp(b.encode_utf16()),
    }
}

/// a < b, a > b … (None: undefined, a NaN met — every comparison is then false).
pub fn compare(a: Val, b: Val) -> Option<Ordering> {
    if let (Val::Num(x), Val::Num(y)) = (&a, &b) {
        return x.partial_cmp(y);
    }
    let (a, b) = (to_prim(a, true), to_prim(b, true));
    if let (Val::Str(x), Val::Str(y)) = (&a, &b) {
        return Some(cmp_str(x, y));
    }
    to_num(&a).partial_cmp(&to_num(&b))
}

/// ===
pub fn strict_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Undef, Val::Undef) | (Val::Null, Val::Null) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Num(x), Val::Num(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        // Dates, lists and regexes are objects: each evaluation makes a new one, never the same.
        _ => false,
    }
}

/// SameValueZero (Array::includes): NaN finds NaN.
pub fn same_value_zero(a: &Val, b: &Val) -> bool {
    matches!((a, b), (Val::Num(x), Val::Num(y)) if x.is_nan() && y.is_nan()) || strict_eq(a, b)
}

/// ==
pub fn loose_eq(a: Val, b: Val) -> bool {
    let obj = |v: &Val| matches!(v, Val::Date(_) | Val::List(_) | Val::Re(_));
    match (&a, &b) {
        (x, y) if x.nullish() || y.nullish() => x.nullish() && y.nullish(),
        (Val::Num(x), Val::Str(s)) | (Val::Str(s), Val::Num(x)) => *x == str_to_num(s),
        (Val::Bool(x), _) => loose_eq(Val::Num(*x as u8 as f64), b),
        (_, Val::Bool(y)) => loose_eq(a, Val::Num(*y as u8 as f64)),
        (x, y) if obj(x) && obj(y) => false,
        (x, _) if obj(x) => loose_eq(to_prim(a, false), b),
        (_, y) if obj(y) => loose_eq(a, to_prim(b, false)),
        (x, y) => strict_eq(x, y),
    }
}

/// What a formula's value becomes in a cell (fxOut): numbers rounded to 10
/// decimals with the file's decimal mark, a date as ISO (with its time when it
/// has one), nothing for null, undefined, NaN and the infinities.
pub fn write_out(v: &Val, comma: bool, out: &mut String) {
    match v {
        Val::Undef | Val::Null => {}
        Val::Num(n) if !n.is_finite() => {}
        Val::Num(n) => {
            let at = out.len();
            fmt_num(js_round(n * 1e10) / 1e10, out);
            if comma {
                comma_mark(out, at);
            }
        }
        Val::Date(t) if t.is_nan() => {}
        Val::Date(t) => {
            let at = out.len();
            date::iso(*t, out);
            let p = date::parts(*t);
            if p.h == 0 && p.mi == 0 && p.s == 0 {
                out.truncate(at + 10);
            } else if out.ends_with(".000Z") {
                out.truncate(out.len() - 5);
                out.push('Z');
            }
        }
        v => write_str(v, out),
    }
}

/// The value as a cell's text, taking it: a computed text is moved, not copied.
pub fn into_out(v: Val, comma: bool) -> String {
    match v {
        Val::Str(s) => s.into_owned(),
        v => {
            let mut s = String::new();
            write_out(&v, comma, &mut s);
            s
        }
    }
}

/// The first '.' from `at` on becomes ',' (fxNumText).
pub fn comma_mark(s: &mut String, at: usize) {
    if let Some(p) = s[at..].find('.') {
        s.replace_range(at + p..at + p + 1, ",");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_as_in_javascript() {
        let f = num_string;
        assert_eq!(f(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(f(1e21), "1e+21");
        assert_eq!(f(1e-7), "1e-7");
        assert_eq!(f(123456789012345680000.0), "123456789012345680000");
        assert_eq!(f(0.000001), "0.000001");
        assert_eq!(f(-13.01), "-13.01");
        assert_eq!(f(1.5e300), "1.5e+300");
        assert_eq!(f(2f64.powi(53)), "9007199254740992");
        assert_eq!(to_fixed(2.5, 0.0).unwrap(), "3");
        assert_eq!(to_fixed(1.005, 2.0).unwrap(), "1.00");
        assert_eq!(to_fixed(1.125, 2.0).unwrap(), "1.13");
        assert_eq!(to_fixed(-1.5, 0.0).unwrap(), "-2");
        assert_eq!(to_fixed(9.995, 2.0).unwrap(), "9.99");
        assert_eq!(to_fixed(99.5, 0.0).unwrap(), "100");
        assert_eq!(to_precision(123.456, 4.0).unwrap(), "123.5");
        assert_eq!(to_precision(0.000123, 2.0).unwrap(), "0.00012");
        assert_eq!(to_precision(123456.0, 2.0).unwrap(), "1.2e+5");
        assert_eq!(to_radix(255.0, 16), "ff");
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
    }

    #[test]
    fn texts_read_as_numbers() {
        assert_eq!(str_to_num(" 0x1F "), 31.0);
        assert_eq!(str_to_num(""), 0.0);
        assert!(str_to_num("12a").is_nan());
        assert!(str_to_num("inf").is_nan());
        assert_eq!(str_to_num("1e3"), 1000.0);
        assert_eq!(parse_float("12.5kg"), 12.5);
        assert_eq!(parse_float("-.5"), -0.5);
        assert!(parse_float(".").is_nan());
        assert_eq!(fx_num_str("1 234,50 €"), 1234.5);
        assert_eq!(fx_num_str("1,234.50"), 1234.5);
        assert_eq!(fx_num_str("1.234,5"), 1234.5);
        assert_eq!(fx_num_str("13,01"), 13.01);
        assert_eq!(fx_num_str("1,234,567"), 1234567.0);
        assert_eq!(fx_num_str("  "), 0.0);
        assert_eq!(fx_num_str("12 %"), 12.0);
        assert!(fx_num_str("abc").is_nan());
    }
}
