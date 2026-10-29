//! Regular expressions written the JavaScript way (/…/flags in a formula, or a
//! text that extract(), replace() or matches() turn into one), run by the
//! `regex` crate.
//!
//! The web app ran each new pattern in a worker first and refused it past a
//! second: a backtracking engine can take minutes on (a+)+$ or (.*)*x. The
//! `regex` crate matches in time linear in the text, whatever the pattern, so
//! that probe is gone; what is left of the guard is a bound on the size of the
//! compiled pattern (a{1000}{1000} would take gigabytes), with a message the
//! user can act on. Patterns are translated to keep JavaScript's meaning where
//! the syntaxes differ (\d and \w are ASCII, `{` stands for itself when it is
//! not a repetition, [^] is any character…); look-around and back-references,
//! which need backtracking, are refused with a message saying so.

use super::value::Val;
use regex::{Regex, RegexBuilder};
use std::borrow::Cow;

/// Bounds of a compiled pattern, as the view's filter uses.
const SIZE_LIMIT: usize = 1 << 22;
const DFA_LIMIT: usize = 1 << 23;
const MAX_SOURCE: usize = 10_000;

pub const TOO_BIG: &str = "this regular expression is too large to run on every row — use smaller counted repeats such as {100} rather than {10000}";

#[derive(Debug)]
pub struct Rx {
    pub re: Regex,
    pub src: Box<str>,
    pub flags: Box<str>,
    pub global: bool,
    pub sticky: bool,
}

/// A pattern and its flags (d g i m s u v y), as `new RegExp(src, flags)` would take them.
pub fn build(src: &str, flags: &str) -> Result<Rx, String> {
    let bad = |m: &str| format!("Invalid regular expression: /{src}/{flags}: {m}");
    let mut seen = String::new();
    for c in flags.chars() {
        if !"dgimsuvy".contains(c) || seen.contains(c) {
            return Err(format!("Invalid flags supplied to RegExp constructor '{flags}'"));
        }
        seen.push(c);
    }
    if seen.contains('u') && seen.contains('v') {
        return Err(format!("Invalid flags supplied to RegExp constructor '{flags}'"));
    }
    if src.len() > MAX_SOURCE {
        return Err(TOO_BIG.into());
    }
    let unicode = seen.contains('u') || seen.contains('v');
    let body = translate(src, unicode, seen.contains('s')).map_err(|m| bad(&m))?;
    let mut pat = String::with_capacity(body.len() + 8);
    if seen.contains('i') || seen.contains('m') {
        pat.push_str("(?");
        if seen.contains('i') { pat.push('i') }
        if seen.contains('m') { pat.push('m') }
        pat.push(')');
    }
    pat.push_str(&body);
    let re = RegexBuilder::new(&pat).size_limit(SIZE_LIMIT).dfa_size_limit(DFA_LIMIT).build().map_err(|e| match e {
        regex::Error::CompiledTooBig(_) => TOO_BIG.to_string(),
        e => {
            let s = e.to_string();
            bad(s.lines().rev().find_map(|l| l.strip_prefix("error: ")).unwrap_or(&s))
        }
    })?;
    Ok(Rx { re, src: src.into(), flags: flags.into(), global: seen.contains('g'), sticky: seen.contains('y') })
}

/// A valid quantifier {n}, {n,}, {n,m} at the start of `s`: its length.
fn quantifier(s: &[char]) -> Option<usize> {
    let mut i = 1;
    let d0 = i;
    while i < s.len() && s[i].is_ascii_digit() { i += 1 }
    if i == d0 { return None }
    if i < s.len() && s[i] == ',' {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() { i += 1 }
    }
    (i < s.len() && s[i] == '}').then_some(i + 1)
}

fn hex(s: &[char], from: usize, n: usize) -> Option<u32> {
    if from + n > s.len() { return None }
    s[from..from + n].iter().try_fold(0, |v, c| c.to_digit(16).map(|d| v * 16 + d))
}

fn lit(c: char, out: &mut String) {
    if c.is_ascii_alphanumeric() || !c.is_ascii() || c == '_' {
        out.push(c);
    } else {
        // the regex crate takes any escaped ASCII punctuation as itself
        out.push('\\');
        out.push(c);
    }
}

/// JavaScript pattern → `regex` crate pattern.
fn translate(src: &str, unicode: bool, dotall: bool) -> Result<String, String> {
    let s: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    let mut class = false;
    while i < s.len() {
        let c = s[i];
        if c == '\\' {
            let Some(&e) = s.get(i + 1) else { return Err("\\ at end of pattern".into()) };
            i += 2;
            match e {
                'd' => out.push_str(if class { "0-9" } else { "[0-9]" }),
                'D' => out.push_str("[^0-9]"),
                'w' => out.push_str(if class { "0-9A-Za-z_" } else { "[0-9A-Za-z_]" }),
                'W' => out.push_str("[^0-9A-Za-z_]"),
                's' | 'S' | 'n' | 'r' | 't' | 'f' | 'v' => { out.push('\\'); out.push(e) }
                'b' if class => out.push_str("\\x08"),
                'b' => out.push_str("(?-u:\\b)"),
                'B' => out.push_str("(?-u:\\B)"),
                '0' if !s.get(i).is_some_and(|c| c.is_ascii_digit()) => out.push_str("\\x00"),
                '1'..='9' if !class => return Err("back-references (\\1) are not supported".into()),
                '0'..='9' => {
                    // an octal escape, in a class or after \0
                    let mut v = e.to_digit(8).ok_or("invalid escape")?;
                    while i < s.len() && v * 8 < 256 && s[i].is_digit(8) { v = v * 8 + s[i].to_digit(8).unwrap(); i += 1 }
                    out.push_str(&format!("\\x{{{v:x}}}"));
                }
                'k' if s.get(i) == Some(&'<') => return Err("back-references (\\k<name>) are not supported".into()),
                'x' => match hex(&s, i, 2) {
                    Some(v) => { out.push_str(&format!("\\x{{{v:x}}}")); i += 2 }
                    None => out.push('x'),
                },
                'u' if unicode && s.get(i) == Some(&'{') => {
                    let end = s[i..].iter().position(|&c| c == '}').ok_or("invalid Unicode escape")? + i;
                    let v = hex(&s, i + 1, end - i - 1).ok_or("invalid Unicode escape")?;
                    out.push_str(&format!("\\x{{{v:x}}}"));
                    i = end + 1;
                }
                'u' => match hex(&s, i, 4) {
                    Some(v) => { out.push_str(&format!("\\x{{{v:x}}}")); i += 4 }
                    None => out.push('u'),
                },
                'c' => match s.get(i) {
                    Some(l) if l.is_ascii_alphabetic() => { out.push_str(&format!("\\x{{{:x}}}", *l as u32 % 32)); i += 1 }
                    _ => out.push_str("\\\\c"),
                },
                'p' | 'P' if unicode => { out.push('\\'); out.push(e) }
                _ => lit(e, &mut out),
            }
            continue;
        }
        if class {
            match c {
                ']' => { class = false; out.push(']') }
                '[' | '&' | '~' => { out.push('\\'); out.push(c) }
                _ => out.push(c),
            }
            i += 1;
            continue;
        }
        match c {
            '[' => {
                if s.get(i + 1) == Some(&'^') && s.get(i + 2) == Some(&']') {
                    out.push_str("(?s:.)");
                    i += 3;
                    continue;
                }
                if s.get(i + 1) == Some(&']') {
                    out.push_str("[^\\x00-\\x{10FFFF}]"); // [] matches nothing
                    i += 2;
                    continue;
                }
                class = true;
                out.push('[');
                if s.get(i + 1) == Some(&'^') { out.push('^'); i += 1 }
                // a ] right after [ or [^ closes nothing in Rust but ends the class in JavaScript: handled above
            }
            '(' if s.get(i + 1) == Some(&'?') => {
                let next: String = s[i + 2..].iter().take(2).collect();
                if next.starts_with('=') || next.starts_with('!') || next == "<=" || next == "<!" {
                    return Err("look-ahead and look-behind — (?= (?! (?<= (?<! — are not supported".into());
                }
                if !(next.starts_with(':') || next.starts_with('<')) {
                    return Err("invalid group".into());
                }
                out.push_str("(?");
                i += 1;
            }
            '{' => match quantifier(&s[i..]) {
                Some(n) => { out.extend(&s[i..i + n]); i += n; continue }
                None => out.push_str("\\{"),
            },
            '}' => out.push_str("\\}"),
            ']' => out.push_str("\\]"),
            '.' if !dotall => out.push_str("[^\\n\\r\\x{2028}\\x{2029}]"),
            '.' => out.push_str("(?s:.)"),
            c => out.push(c),
        }
        i += 1;
    }
    if class {
        return Err("Unterminated character class".into());
    }
    Ok(out)
}

/// UTF-16 length of the text before byte `b` (JavaScript indexes in code units).
pub fn u16_index(s: &str, b: usize) -> usize {
    let head = &s[..b];
    if head.is_ascii() { b } else { head.chars().map(char::len_utf16).sum() }
}

/// Where a search from byte `i` continues after an empty match: the next character.
fn step(s: &str, i: usize) -> usize {
    i + s[i..].chars().next().map_or(1, char::len_utf8)
}

/// A match and its groups as a list (exec): [whole, group 1, …], undefined for a group not taken.
fn groups<'a>(caps: &regex::Captures<'a>) -> Vec<Val<'a>> {
    caps.iter().map(|m| m.map_or(Val::Undef, |m| Val::str(m.as_str()))).collect()
}

impl Rx {
    fn find_at<'h>(&self, s: &'h str, i: usize) -> Option<regex::Match<'h>> {
        let m = self.re.find_at(s, i)?;
        (!self.sticky || m.start() == i).then_some(m)
    }

    pub fn test(&self, s: &str) -> bool {
        if self.sticky { self.find_at(s, 0).is_some() } else { self.re.is_match(s) }
    }

    /// Every match from the start, as `match` with /g and `replace` with /g walk them:
    /// an empty match moves on one character.
    fn each<'h>(&self, s: &'h str, mut f: impl FnMut(regex::Match<'h>)) {
        let mut i = 0;
        while i <= s.len() {
            let Some(m) = self.find_at(s, i) else { break };
            f(m);
            i = if m.end() == m.start() { step(s, m.end()) } else { m.end() };
        }
    }

    /// String::match: the groups of the first match, or every match with /g; null when none.
    pub fn match_in<'a>(&self, s: Cow<'a, str>) -> Val<'a> {
        let out = match s {
            Cow::Borrowed(s) => self.matches_of(s),
            Cow::Owned(s) => self.matches_of(&s).into_iter().map(|v| v.into_static().unwrap_or(Val::Undef)).collect(),
        };
        if out.is_empty() { Val::Null } else { Val::List(out) }
    }

    fn matches_of<'h>(&self, s: &'h str) -> Vec<Val<'h>> {
        if self.global {
            let mut out = Vec::new();
            self.each(s, |m| out.push(Val::str(m.as_str())));
            return out;
        }
        if self.sticky {
            return match self.re.captures_at(s, 0) {
                Some(c) if c.get(0).unwrap().start() == 0 => groups(&c),
                _ => Vec::new(),
            };
        }
        self.re.captures(s).map(|c| groups(&c)).unwrap_or_default()
    }

    /// String::search: the UTF-16 index of the first match, -1 when none.
    pub fn search(&self, s: &str) -> f64 {
        self.find_at(s, 0).map_or(-1.0, |m| u16_index(s, m.start()) as f64)
    }

    /// String::replace with this pattern (every match with /g), `$1 $& $<name>…` expanded.
    pub fn replace<'a>(&self, s: Cow<'a, str>, by: &str) -> Cow<'a, str> {
        let plain = !by.contains('$');
        let mut out = String::new();
        let mut last = 0;
        let mut any = false;
        let mut one = |m_start: usize, m_end: usize, out: &mut String| {
            out.push_str(&s[last..m_start]);
            if plain {
                out.push_str(by);
            } else if let Some(c) = self.re.captures_at(&s, m_start) {
                expand(by, &s, m_start, m_end, Some((&c, &self.re)), out);
            }
            last = m_end;
            any = true;
        };
        if self.global {
            let mut spans = Vec::new();
            self.each(&s, |m| spans.push((m.start(), m.end())));
            for (a, b) in spans { one(a, b, &mut out) }
        } else if let Some(m) = self.find_at(&s, 0) {
            one(m.start(), m.end(), &mut out);
        }
        if !any {
            return s;
        }
        out.push_str(&s[last..]);
        Cow::Owned(out)
    }

    /// String::split with this pattern; its groups go into the list, as in JavaScript.
    pub fn split<'a>(&self, s: &'a str, limit: usize) -> Vec<Val<'a>> {
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        if s.is_empty() {
            if self.find_at(s, 0).is_none() { out.push(Val::str("")) }
            return out;
        }
        let (mut p, mut q) = (0, 0);
        while q < s.len() {
            let Some(c) = self.re.captures_at(s, q) else { break };
            let m = c.get(0).unwrap();
            if m.end() == p || m.start() >= s.len() {
                // an empty match where the last piece ends: move on
                q = step(s, m.start().max(q));
                continue;
            }
            out.push(Val::str(&s[p..m.start()]));
            if out.len() == limit { return out }
            for g in c.iter().skip(1) {
                out.push(g.map_or(Val::Undef, |g| Val::str(g.as_str())));
                if out.len() == limit { return out }
            }
            p = m.end();
            q = if m.end() == m.start() { step(s, p) } else { p };
        }
        out.push(Val::str(&s[p..]));
        out
    }
}

/// GetSubstitution: $$ $& $` $' $n $nn $<name> in a replacement text.
pub fn expand(by: &str, s: &str, start: usize, end: usize, caps: Option<(&regex::Captures, &Regex)>, out: &mut String) {
    let b = by.as_bytes();
    let n = caps.map_or(0, |(c, _)| c.len() - 1);
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' || i + 1 >= b.len() {
            let ch = by[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        let group = |k: usize, out: &mut String| {
            if let Some(m) = caps.and_then(|(c, _)| c.get(k)) { out.push_str(m.as_str()) }
        };
        match b[i + 1] {
            b'$' => { out.push('$'); i += 2 }
            b'&' => { out.push_str(&s[start..end]); i += 2 }
            b'`' => { out.push_str(&s[..start]); i += 2 }
            b'\'' => { out.push_str(&s[end..]); i += 2 }
            d @ b'0'..=b'9' => {
                let one = (d - b'0') as usize;
                let two = b.get(i + 2).filter(|c| c.is_ascii_digit()).map(|c| one * 10 + (c - b'0') as usize);
                match two {
                    Some(k) if k >= 1 && k <= n => { group(k, out); i += 3 }
                    _ if one >= 1 && one <= n => { group(one, out); i += 2 }
                    _ => { out.push('$'); i += 1 }
                }
            }
            b'<' if caps.is_some_and(|(_, re)| re.capture_names().flatten().next().is_some()) => match by[i + 2..].find('>') {
                Some(e) => {
                    let name = &by[i + 2..i + 2 + e];
                    if let Some(m) = caps.and_then(|(c, _)| c.name(name)) { out.push_str(m.as_str()) }
                    i += e + 3;
                }
                None => { out.push('$'); i += 1 }
            },
            _ => { out.push('$'); i += 1 }
        }
    }
}

/// Splits by a text (String::split with a string separator), in code units for "".
pub fn split_str<'a>(s: &'a str, sep: &str, limit: usize) -> Vec<Val<'a>> {
    let mut out = Vec::new();
    if limit == 0 {
        return out;
    }
    if sep.is_empty() {
        for (i, c) in s.char_indices() {
            if out.len() == limit { break }
            out.push(Val::str(&s[i..i + c.len_utf8()]));
        }
        return out;
    }
    for p in s.split(sep) {
        if out.len() == limit { break }
        out.push(Val::str(p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(src: &str, flags: &str, s: &str, by: &str) -> String {
        build(src, flags).unwrap().replace(Cow::Borrowed(s), by).into_owned()
    }

    #[test]
    fn translation_keeps_javascript_meaning() {
        assert!(build(r"^\d+$", "").unwrap().test("123"));
        assert!(!build(r"^\d+$", "").unwrap().test("١٢٣"));
        assert!(build(r"a{", "").unwrap().test("a{"));
        assert!(build(r"[^]", "").unwrap().test("\n"));
        assert!(!build(r"[]", "").unwrap().test("x"));
        assert!(build(r"\bé", "").is_ok());
        assert!(build(r"[\d.]+", "").unwrap().test("1.5"));
        assert!(build(r"[a&&b]", "").unwrap().test("&"));
        assert!(build(r"\/x\/", "").unwrap().test("/x/"));
        assert!(build(r"\u00e9", "").unwrap().test("é"));
        assert!(build("abc", "i").unwrap().test("ABC"));
        assert!(build("(?<y>\\d{4})", "").is_ok());
        assert!(build(r"(a)\1", "").unwrap_err().contains("back-references"));
        assert!(build(r"a(?=b)", "").unwrap_err().contains("look-ahead"));
        assert!(build(r"(", "").unwrap_err().starts_with("Invalid regular expression: /(/"));
        assert!(build("a", "gg").is_err());
    }

    #[test]
    fn guard() {
        // Catastrophic for a backtracking engine, linear here: 30 a's then b.
        let r = build("^(a+)+$", "").unwrap();
        let t = std::time::Instant::now();
        for _ in 0..1000 {
            assert!(!r.test(&format!("{}b", "a".repeat(30))));
        }
        assert!(t.elapsed().as_millis() < 1000);
        assert!(build("(.*)*x", "").is_ok());
        assert_eq!(build("(a{1000}){1000}", "").unwrap_err(), TOO_BIG);
    }

    #[test]
    fn replace_and_split_as_javascript() {
        assert_eq!(rep(r"\d+", "g", "Nom12 Dupont3", "#"), "Nom# Dupont#");
        assert_eq!(rep(r"\d+", "", "a1b2", "#"), "a#b2");
        assert_eq!(rep(r"(\w+)@(\w+)", "", "x ab@cd y", "$2 at $1 [$&] $$ $3"), "x cd at ab [ab@cd] $ $3 y");
        assert_eq!(rep("x*", "g", "abc", "-"), "-a-b-c-");
        assert_eq!(rep("a*", "g", "baaac", "-"), "-b--c-");
        let parts = |v: Vec<Val>| v.iter().map(|x| match x { Val::Str(s) => s.to_string(), _ => "?".into() }).collect::<Vec<_>>();
        assert_eq!(parts(build("x*", "").unwrap().split("abc", usize::MAX)), ["a", "b", "c"]);
        assert_eq!(parts(build("(-)", "").unwrap().split("a-b", usize::MAX)), ["a", "-", "b"]);
        assert_eq!(parts(build(",", "").unwrap().split("", usize::MAX)), [""]);
        assert_eq!(parts(build("", "").unwrap().split("ab", usize::MAX)), ["a", "b"]);
        let m = build("a(x)?", "g").unwrap().match_in(Cow::Borrowed("a ab a"));
        assert!(matches!(m, Val::List(ref l) if l.len() == 3));
    }
}
