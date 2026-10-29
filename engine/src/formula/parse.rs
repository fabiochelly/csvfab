//! The formula language (38-formula-parser.js): a small, safe part of
//! JavaScript's expressions. Literals (numbers, "texts", 'texts', /regexes/),
//! {Column}, row (or v), true false null undefined NaN Infinity, [lists],
//! ( ), unary ! - +, ** * / % + -, < > <= >= == != === !==, && || ??,
//! … ? … : …, calls of the helpers, of Math's functions and of the methods
//! listed, and x[i]. Nothing else gets through, with the same messages as the
//! web app — they tell the user what to write instead.

use super::eval::{Bop, Node};
use super::fx::{Fx, Meth, Mf, Nat};
use super::jsre::{self, Rx};
use super::value::{is_space, Val};
use super::Error;
use std::borrow::Cow;

enum K {
    Num(f64),
    Str(String),
    Col(String),
    Name,
    Re(Option<Box<Rx>>),
    Op(&'static str),
}

struct Tok {
    k: K,
    at: usize,
    end: usize,
}

fn err(msg: impl Into<String>, at: usize, end: usize) -> Error {
    Error { msg: msg.into(), span: at..end }
}

const OPS: &[&str] = &["===", "!==", "**", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "-", "+", "*", "/", "%", "<", ">", "!", "?",
    ":", ".", ",", "(", ")", "[", "]"];

fn name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$' || ('\u{c0}'..='\u{ffff}').contains(&c)
}

fn name_char(c: char) -> bool {
    name_start(c) || c.is_ascii_digit()
}

/// The number literal at the start of `s` (JavaScript's grammar, at most 64 characters as the web app reads it).
fn number_len(s: &str) -> Option<usize> {
    let b = &s.as_bytes()[..s.len().min(64)];
    let digits = |i: usize| b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    if b.len() > 2 && b[0] == b'0' && (b[1] | 0x20) == b'x' {
        let h = b[2..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
        if h > 0 { return Some(2 + h) }
    }
    let int = digits(0);
    let mut i = int;
    if int > 0 {
        if b.get(i) == Some(&b'.') { i += 1 + digits(i + 1) }
    } else if b.first() == Some(&b'.') && digits(1) > 0 {
        i = 1 + digits(1);
    } else {
        return None;
    }
    if i < b.len() && (b[i] | 0x20) == b'e' {
        let s = if matches!(b.get(i + 1), Some(b'+' | b'-')) { 2 } else { 1 };
        let e = if i + s <= b.len() { digits(i + s) } else { 0 };
        if e > 0 { i += s + e }
    }
    Some(i)
}

/// A text literal's value, its escapes read as JavaScript does in strict mode.
fn unescape(body: &str, at: usize) -> Result<String, Error> {
    let mut out = String::with_capacity(body.len());
    let mut it = body.chars().peekable();
    let mut units: Vec<u16> = Vec::new();
    let flush = |units: &mut Vec<u16>, out: &mut String| {
        if !units.is_empty() {
            out.push_str(&String::from_utf16_lossy(units));
            units.clear();
        }
    };
    let hexn = |it: &mut std::iter::Peekable<std::str::Chars>, n: usize| -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = v * 16 + it.next()?.to_digit(16)?;
        }
        Some(v)
    };
    while let Some(c) = it.next() {
        if c != '\\' {
            flush(&mut units, &mut out);
            out.push(c);
            continue;
        }
        let Some(e) = it.next() else { break };
        let bad = |m: &str| Err(err(m, at, at + body.len() + 2));
        let unit = match e {
            'n' => '\n' as u32,
            't' => '\t' as u32,
            'r' => '\r' as u32,
            'b' => 8,
            'f' => 12,
            'v' => 11,
            '0' if !it.peek().is_some_and(|c| c.is_ascii_digit()) => 0,
            '0'..='7' => return bad("Octal escape sequences are not allowed in strict mode."),
            '8' | '9' => return bad("\\8 and \\9 are not allowed in strict mode."),
            'x' => match hexn(&mut it, 2) { Some(v) => v, None => return bad("Invalid hexadecimal escape sequence") },
            'u' if it.peek() == Some(&'{') => {
                it.next();
                let mut v = 0u32;
                let mut n = 0;
                loop {
                    match it.next() {
                        Some('}') if n > 0 => break,
                        Some(d) if d.is_ascii_hexdigit() && v <= 0x10ffff => { v = v * 16 + d.to_digit(16).unwrap(); n += 1 }
                        _ => return bad("Invalid Unicode escape sequence"),
                    }
                }
                if v > 0x10ffff { return bad("Undefined Unicode code-point") }
                flush(&mut units, &mut out);
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                continue;
            }
            'u' => match hexn(&mut it, 4) { Some(v) => v, None => return bad("Invalid Unicode escape sequence") },
            '\u{2028}' | '\u{2029}' => continue, // a line continuation
            c => c as u32,
        };
        // 😀 makes one character: units are gathered until a plain character.
        if (0xd800..0xe000).contains(&unit) {
            units.push(unit as u16);
        } else {
            flush(&mut units, &mut out);
            out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
        }
    }
    flush(&mut units, &mut out);
    Ok(out)
}

fn lex(src: &str) -> Result<Vec<Tok>, Error> {
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        let c = rest.chars().next().unwrap();
        if is_space(c) {
            i += c.len_utf8();
            continue;
        }
        let at = i;
        if c == '{' {
            match rest.find('}') {
                Some(e) if e > 1 && !rest[1..e].contains(['{', '\n']) => {
                    out.push(Tok { k: K::Col(rest[1..e].to_string()), at, end: at + e + 1 });
                    i += e + 1;
                    continue;
                }
                _ => return Err(err("A { opens a column name: close it with }", at, at + 1)),
            }
        }
        if c == '"' || c == '\'' {
            let b = rest.as_bytes();
            let mut j = 1;
            loop {
                if j >= b.len() || b[j] == b'\n' || b[j] == b'\r' {
                    return Err(err("A text in quotes is not closed", at, at + 1));
                }
                if b[j] == b'\\' {
                    if matches!(b.get(j + 1), Some(b'\n' | b'\r')) {
                        return Err(err("A text in quotes is not closed", at, at + 1));
                    }
                    j += 1 + rest[j + 1..].chars().next().map_or(1, char::len_utf8);
                    continue;
                }
                if b[j] == c as u8 {
                    break;
                }
                j += 1;
            }
            out.push(Tok { k: K::Str(unescape(&rest[1..j], at)?), at, end: at + j + 1 });
            i += j + 1;
            continue;
        }
        if c == '`' {
            return Err(err("Use \"…\" or '…' for a text (no backquotes)", at, at + 1));
        }
        if let Some(n) = number_len(rest) {
            let text = &rest[..n];
            if let Some(next) = rest[n..].chars().next().filter(|&c| c.is_ascii_alphanumeric() || c == '_' || c == '$') {
                return Err(err(format!("“{}{next}” is not a number", text), at, at + n + next.len_utf8()));
            }
            let b = text.as_bytes();
            if b.len() > 1 && b[0] == b'0' && b[1].is_ascii_digit() {
                // what V8 says of these literals in strict mode
                let msg = if b.iter().all(|c| (b'0'..=b'7').contains(c)) { "Octal literals are not allowed in strict mode." } else { "Decimals with leading zeros are not allowed in strict mode." };
                return Err(err(msg, at, at + n));
            }
            let v = if b.len() > 2 && (b[1] | 0x20) == b'x' { super::value::str_to_num(text) } else { text.parse().unwrap_or(f64::NAN) };
            out.push(Tok { k: K::Num(v), at, end: at + n });
            i += n;
            continue;
        }
        if name_start(c) {
            let n = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
            out.push(Tok { k: K::Name, at, end: at + n });
            i += n;
            continue;
        }
        let value_ends = out.last().is_some_and(|p| matches!(p.k, K::Num(_) | K::Str(_) | K::Col(_) | K::Name | K::Re(_) | K::Op(")" | "]")));
        if c == '/' && !value_ends {
            let b = rest.as_bytes();
            let (mut j, mut class) = (1, false);
            loop {
                if j >= b.len() || b[j] == b'\n' {
                    return Err(err("A regular expression /…/ is not closed", at, at + 1));
                }
                match b[j] {
                    b'\\' => { j += 2; continue }
                    b'[' => class = true,
                    b']' => class = false,
                    b'/' if !class => break,
                    _ => {}
                }
                j += 1;
            }
            let fl = rest[j + 1..].bytes().take_while(|c| b"dgimsuyv".contains(c)).count();
            let (body, flags) = (&rest[1..j], &rest[j + 1..j + 1 + fl]);
            let rx = jsre::build(body, flags).map_err(|e| {
                let msg = if e == jsre::TOO_BIG || e.starts_with("Invalid regular expression") { e } else { format!("Invalid regular expression: {e}") };
                err(msg, at, at + j + 1 + fl)
            })?;
            out.push(Tok { k: K::Re(Some(Box::new(rx))), at, end: at + j + 1 + fl });
            i += j + 1 + fl;
            continue;
        }
        if let Some(op) = OPS.iter().find(|o| rest.starts_with(**o)) {
            if *op == "?." {
                return Err(err("Optional chaining ?. is not supported: test the value first", at, at + 2));
            }
            out.push(Tok { k: K::Op(op), at, end: at + op.len() });
            i += op.len();
            continue;
        }
        return Err(match c {
            '=' => err("Use === to compare (= would assign)", at, at + 1),
            ';' => err("A formula is one expression: no ;", at, at + 1),
            c => err(format!("Unexpected “{c}”"), at, at + c.len_utf8()),
        });
    }
    Ok(out)
}

pub enum Mode<'h> {
    /// {Column} names one of these; `row` is the row's number.
    Cols(&'h [String]),
    /// `v` is the value; a {Column} is refused.
    Value,
}

pub struct Parsed {
    pub root: Node,
    /// slot → column
    pub cols: Vec<usize>,
    pub uses_row: bool,
    pub uses: Vec<Fx>,
}

struct P<'s, 'h> {
    src: &'s str,
    toks: Vec<Tok>,
    p: usize,
    mode: Mode<'h>,
    slugs: Vec<String>,
    out: Parsed,
}

const CONSTS: [&str; 6] = ["true", "false", "null", "undefined", "NaN", "Infinity"];

pub fn parse(src: &str, mode: Mode) -> Result<Parsed, Error> {
    let toks = lex(src)?;
    let slugs = match &mode { Mode::Cols(h) => h.iter().map(|h| super::fx::slugify(h)).collect(), Mode::Value => Vec::new() };
    let mut p = P { src, toks, p: 0, mode, slugs, out: Parsed { root: Node::Const(Val::Undef), cols: Vec::new(), uses_row: false, uses: Vec::new() } };
    let root = p.expr()?;
    if p.p < p.toks.len() {
        return Err(p.fail("Unexpected"));
    }
    p.out.root = root;
    Ok(p.out)
}

impl P<'_, '_> {
    fn text(&self, t: &Tok) -> &str {
        &self.src[t.at..t.end]
    }

    fn fail(&self, msg: &str) -> Error {
        match self.toks.get(self.p) {
            Some(t) => err(format!("{msg} near “{}”", self.text(t)), t.at, t.end),
            None => err(format!("{msg} at the end"), self.src.len(), self.src.len()),
        }
    }

    fn is(&self, op: &str) -> bool {
        matches!(self.toks.get(self.p), Some(Tok { k: K::Op(o), .. }) if *o == op)
    }

    fn expect(&mut self, op: &str) -> Result<(), Error> {
        if !self.is(op) {
            return Err(self.fail(&format!("Expected “{op}”")));
        }
        self.p += 1;
        Ok(())
    }

    fn expr(&mut self) -> Result<Node, Error> {
        let c = self.binary(0)?;
        if !self.is("?") {
            return Ok(c);
        }
        self.p += 1;
        let a = self.expr()?;
        self.expect(":")?;
        let b = self.expr()?;
        Ok(Node::Cond(Box::new((c, a, b))))
    }

    /// Lowest first; ** is right-associative and handled in power().
    fn binary(&mut self, level: usize) -> Result<Node, Error> {
        const LEVELS: [&[&str]; 7] = [&["??"], &["||"], &["&&"], &["==", "!=", "===", "!=="], &["<", ">", "<=", ">="], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() {
            return self.power();
        }
        let mut left = self.binary(level + 1)?;
        while let Some(Tok { k: K::Op(op), .. }) = self.toks.get(self.p) {
            let op = *op;
            if !LEVELS[level].contains(&op) {
                break;
            }
            self.p += 1;
            let right = self.binary(level + 1)?;
            let ab = Box::new((left, right));
            left = match op {
                "??" => Node::Nullish(ab),
                "||" => Node::Or(ab),
                "&&" => Node::And(ab),
                op => Node::Bin(Bop::by_name(op).unwrap(), ab),
            };
        }
        Ok(left)
    }

    fn power(&mut self) -> Result<Node, Error> {
        let base = self.unary()?;
        if !self.is("**") {
            return Ok(base);
        }
        self.p += 1;
        Ok(Node::Bin(Bop::Pow, Box::new((base, self.power()?))))
    }

    fn unary(&mut self) -> Result<Node, Error> {
        for (op, f) in [("!", Node::Not as fn(Box<Node>) -> Node), ("-", Node::Neg), ("+", Node::Pos)] {
            if self.is(op) {
                self.p += 1;
                return Ok(f(Box::new(self.unary()?)));
            }
        }
        let start = self.toks.get(self.p).map_or(self.src.len(), |t| t.at);
        let prim = self.primary()?;
        self.postfix(prim, start)
    }

    fn args(&mut self) -> Result<Vec<Node>, Error> {
        self.expect("(")?;
        let out = self.items(")")?;
        self.expect(")")?;
        Ok(out)
    }

    fn items(&mut self, close: &str) -> Result<Vec<Node>, Error> {
        let mut out = Vec::new();
        if !self.is(close) {
            loop {
                out.push(self.expr()?);
                if self.is(",") {
                    self.p += 1;
                    continue;
                }
                break;
            }
        }
        Ok(out)
    }

    fn postfix(&mut self, mut node: Node, start: usize) -> Result<Node, Error> {
        let src = self.src;
        loop {
            if self.is(".") {
                let recv_end = self.toks[self.p - 1].end;
                self.p += 1;
                let t = self.toks.get(self.p);
                self.p += 1;
                let Some(t) = t.filter(|t| matches!(t.k, K::Name)) else { return Err(self.fail("A method name is expected after “.”")) };
                let (at, end) = (t.at, t.end);
                let name = &src[at..end];
                let Some(m) = Meth::by_name(name) else { return Err(err(format!(".{name} is not available in a formula"), at, end)) };
                // Called, always — but .length: a method left uncalled would be a function value.
                if m == Meth::Nat(Nat::Length) && !self.is("(") {
                    node = Node::Length(Box::new(node));
                    continue;
                }
                if !self.is("(") {
                    return Err(err(format!(".{name} is a method: write .{name}(…)"), at, end));
                }
                if let Meth::Fx(f) = m { self.out.uses.push(f) }
                let a = self.args()?;
                node = Node::Method(Box::new(node), m, a, src[start..recv_end].into());
            } else if self.is("[") {
                self.p += 1;
                let i = self.expr()?;
                self.expect("]")?;
                node = Node::Index(Box::new((node, i)));
            } else if self.is("(") {
                return Err(self.fail("Only a helper or a method can be called"));
            } else {
                return Ok(node);
            }
        }
    }

    fn column(&mut self, name: &str, at: usize, end: usize) -> Result<Node, Error> {
        let Mode::Cols(headers) = self.mode else {
            return Err(err(format!("Write v for the cell's value (not {{{name}}})"), at, end));
        };
        // exactly, then trimmed, then ignoring case, accents and symbols
        let c = headers.iter().position(|h| h == name)
            .or_else(|| headers.iter().position(|h| h == name.trim()))
            .or_else(|| { let s = super::fx::slugify(name); self.slugs.iter().position(|h| *h == s) })
            .ok_or_else(|| err(format!("Unknown column: {{{name}}}"), at, end))?;
        let slot = match self.out.cols.iter().position(|&x| x == c) {
            Some(s) => s,
            None => { self.out.cols.push(c); self.out.cols.len() - 1 }
        };
        Ok(Node::Col(slot))
    }

    fn primary(&mut self) -> Result<Node, Error> {
        let src = self.src;
        let Some(t) = self.toks.get_mut(self.p) else { return Err(self.fail("A value is missing")) };
        self.p += 1;
        let (at, end) = (t.at, t.end);
        match &mut t.k {
            K::Num(n) => return Ok(Node::Const(Val::Num(*n))),
            K::Str(s) => return Ok(Node::Const(Val::Str(Cow::Owned(std::mem::take(s))))),
            K::Re(r) => return Ok(Node::Re(r.take().expect("a regex token is read once"))),
            K::Col(name) => {
                let name = name.clone();
                return self.column(&name, at, end);
            }
            K::Op("(") => {
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(e);
            }
            K::Op("[") => {
                let items = self.items("]")?;
                self.expect("]")?;
                return Ok(Node::List(items));
            }
            K::Name => {}
            K::Op(_) => {
                self.p -= 1;
                return Err(self.fail("Unexpected"));
            }
        }
        let name = &src[at..end];
        if let Some(f) = Fx::by_name(name) {
            if !self.is("(") {
                return Err(err(format!("{name} is a function: write {name}(…)"), at, end));
            }
            self.out.uses.push(f);
            return Ok(Node::Fx(f, self.args()?));
        }
        if name == "Math" {
            self.expect(".")?;
            let m = self.toks.get(self.p).map(|m| (matches!(m.k, K::Name), m.at, m.end));
            self.p += 1;
            let (is_name, mat, mend) = m.unwrap_or((false, at, end));
            let mname = if m.is_some() { &src[mat..mend] } else { "" };
            match mname {
                "PI" if is_name => return Ok(Node::Const(Val::Num(std::f64::consts::PI))),
                "E" if is_name => return Ok(Node::Const(Val::Num(std::f64::consts::E))),
                _ => {}
            }
            let Some(f) = Mf::by_name(mname).filter(|_| is_name) else {
                return Err(err(format!("Math.{mname} is not available in a formula"), mat, mend));
            };
            if !self.is("(") {
                return Err(err(format!("Math.{mname} is a function: write Math.{mname}(…)"), mat, mend));
            }
            return Ok(Node::Math(f, self.args()?));
        }
        if CONSTS.contains(&name) {
            return Ok(Node::Const(match name {
                "true" => Val::Bool(true),
                "false" => Val::Bool(false),
                "null" => Val::Null,
                "undefined" => Val::Undef,
                "NaN" => Val::Num(f64::NAN),
                _ => Val::Num(f64::INFINITY),
            }));
        }
        match (&self.mode, name) {
            (Mode::Cols(_), "row") => {
                self.out.uses_row = true;
                return Ok(Node::Row);
            }
            (Mode::Value, "v") => return Ok(Node::V),
            _ => {}
        }
        Err(err(format!("Unknown name: {name} — a column is written {{{name}}}, a helper is called with ( )"), at, end))
    }
}

/// The patterns a formula holds — its /regexes/ and the texts that are valid
/// patterns — as (source, flags). The web app tried them in a worker before
/// running a formula; kept for whoever wants to show or check them.
pub fn regex_sources(src: &str) -> Vec<(String, String)> {
    let Ok(toks) = lex(src) else { return Vec::new() };
    toks.into_iter().filter_map(|t| match t.k {
        K::Re(Some(r)) => Some((r.src.to_string(), r.flags.to_string())),
        K::Str(s) => jsre::build(&s, "i").is_ok().then(|| (s, "i".to_string())),
        _ => None,
    }).collect()
}
