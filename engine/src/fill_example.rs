//! Fill by example: from one or a few results typed in a column, a rule
//! taking them from the row's other cells — words, parts around a
//! separator, initials, digits, case, accents, with constant text between
//! them ("M. Jean DUPONT" → "Dupont"; "Jean" + "Dupont" →
//! "jean.dupont@acme.fr"; "2024-03-15" → "15/03/2024") — then applied to
//! the column's empty cells.
//!
//! The search: every way a piece of the first example can be read from its
//! row ("atoms"), then the cheapest chain of atoms and constant text spelling
//! every example at once — a shortest path whose nodes are the positions
//! reached in each example, so a chain straying from any example is never
//! followed. Examples beyond the first few check the rule; one it gets
//! wrong joins the search, which runs again.
//!
//! Texts are compared as strings and positions are byte offsets; slices and
//! costs count characters (the JS counted UTF-16 units: the same outside
//! the astral planes).

use crate::split::{is_letter_or_number, remove_accents};
use crate::table::{RowId, State};
use rayon::prelude::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Examples the search starts from.
const SYNTH_EX: usize = 6;
/// Examples taken at most (the app keeps the last ones typed).
pub const MAX_EX: usize = 40;
/// Longer examples and source values are not searched.
const LEN_MAX: usize = 200;
/// Source columns looked at.
const COLS_MAX: usize = 200;
/// Shortest-path nodes expanded at most.
const NODES: usize = 20000;
/// Parts of a value split on a separator, at most.
const PARTS_MAX: usize = 12;

/// Token classes: non-space runs, words (letters), letters and digits, numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    S,
    W,
    A,
    N,
}

impl Tok {
    const ALL: [Tok; 4] = [Tok::S, Tok::W, Tok::A, Tok::N];

    #[inline]
    fn has(self, c: char) -> bool {
        match self {
            Tok::S => !c.is_whitespace(),
            Tok::W => c.is_alphabetic(),
            Tok::A => is_letter_or_number(c),
            Tok::N => c.is_numeric(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    As,
    Upper,
    Lower,
    /// Each word's first letter upper case, the rest lower.
    Capital,
    LowerNoAccents,
    UpperNoAccents,
}

impl Case {
    const ALL: [Case; 6] = [Case::As, Case::Upper, Case::Lower, Case::Capital, Case::LowerNoAccents, Case::UpperNoAccents];

    fn apply<'a>(self, s: &'a str) -> Cow<'a, str> {
        match self {
            Case::As => s.into(),
            Case::Upper => s.to_uppercase().into(),
            Case::Lower => s.to_lowercase().into(),
            Case::Capital => {
                let low = s.to_lowercase();
                let mut out = String::with_capacity(low.len());
                let mut prev: Option<char> = None;
                for c in low.chars() {
                    let start = prev.is_none_or(|p| !(is_letter_or_number(p) || p == '\'' || p == '’'));
                    if start && c.is_alphabetic() { out.extend(c.to_uppercase()) } else { out.push(c) }
                    prev = Some(c);
                }
                out.into()
            }
            Case::LowerNoAccents => remove_accents(s).to_lowercase().into(),
            Case::UpperNoAccents => remove_accents(s).to_uppercase().into(),
        }
    }
}

/// The piece an atom reads from a value. Indexes < 0 count from the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    /// The whole value.
    All,
    /// Its digits.
    Digits,
    /// The i-th token of a class.
    Tok(Tok, i32),
    /// The i-th part split on a character.
    Field(char, i32),
    /// The parts from the i-th on.
    From(char, i32),
    /// The parts up to the i-th.
    To(char, i32),
}

/// A piece of a column's value, in a case, then its first (slice > 0) or
/// last (slice < 0) characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Atom {
    pub col: usize,
    pub piece: Piece,
    pub case: Case,
    pub slice: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Text written as is.
    Text(String),
    Atom(Atom),
}

/// One chip of a rule spelled out: constant text, or a column and what is taken of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chip {
    Text(String),
    Column { name: String, detail: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The column filled.
    pub col: usize,
    pub parts: Vec<Part>,
}

fn tokens(s: &str, x: Tok) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (x.has(c), start) {
            (true, None) => start = Some(i),
            (false, Some(a)) => { out.push(&s[a..i]); start = None; }
            _ => {}
        }
    }
    if let Some(a) = start { out.push(&s[a..]); }
    out
}

#[inline]
fn index(i: i32, n: usize) -> Option<usize> {
    let i = if i < 0 { n as i64 + i as i64 } else { i as i64 };
    if i < 0 || i >= n as i64 { None } else { Some(i as usize) }
}

/// The piece an atom reads from a (trimmed) value, before case and slice;
/// None when the value has no such piece (no third word, no "@") or it is empty.
fn base(s: &str, p: Piece) -> Option<Cow<'_, str>> {
    let b: Cow<str> = match p {
        Piece::All => s.into(),
        Piece::Digits => s.chars().filter(char::is_ascii_digit).collect::<String>().into(),
        Piece::Tok(x, i) => {
            let t = tokens(s, x);
            t[index(i, t.len())?].into()
        }
        Piece::Field(x, i) | Piece::From(x, i) | Piece::To(x, i) => {
            let parts: Vec<&str> = s.split(x).collect();
            let n = parts.len();
            let i = index(i, n)?;
            if n < 2 { return None; }    // the separator is not in the value
            // The parts are slices of s: the ones from (up to) i are s from (up to) there.
            let at = |q: &str| q.as_ptr() as usize - s.as_ptr() as usize;
            match p {
                Piece::Field(..) => parts[i].trim().into(),
                Piece::From(..) => { if i < 1 { return None; } s[at(parts[i])..].trim().into() }
                _ => { if i + 2 > n { return None; } s[..at(parts[i]) + parts[i].len()].trim().into() }
            }
        }
    };
    if b.is_empty() { None } else { Some(b) }
}

fn sliced(v: Cow<'_, str>, sl: i32) -> String {
    match sl {
        0 => v.into_owned(),
        n if n > 0 => v.chars().take(n as usize).collect(),
        n => {
            let k = v.chars().count().saturating_sub((-n) as usize);
            v.chars().skip(k).collect()
        }
    }
}

/// An atom's text from a source value.
fn atom_value(s: &str, a: &Atom) -> Option<String> {
    let b = base(s, a.piece)?;
    Some(sliced(a.case.apply(&b), a.slice))
}

fn src(st: &State, id: RowId, c: usize, tmp: &mut Vec<u8>) -> String {
    st.with_cell(id, c, tmp, |s| s.trim().to_string())
}

/// What a piece of the rule costs: the plainest reading wins (the whole
/// value, then a word, then a part…; a case or a slice costs more), and
/// constant text by its characters, a letter or a digit far more than
/// punctuation — so what the row holds is read from it rather than written
/// as a constant.
fn atom_cost(a: &Atom) -> f64 {
    let (mut k, i) = match a.piece {
        Piece::All => (1.0, None),
        Piece::Digits => (1.0 + 0.4, None),
        Piece::Tok(x, i) => (1.0 + 0.0 + match x { Tok::S => 0.1, Tok::W => 0.15, Tok::A => 0.2, Tok::N => 0.15 }, Some(i)),
        Piece::Field(_, i) => (1.0 + 0.25, Some(i)),
        Piece::From(_, i) | Piece::To(_, i) => (1.0 + 0.35, Some(i)),
    };
    if let Some(i) = i { k += 0.03 * (if i < 0 { -i - 1 } else { i }) as f64; }
    if a.case != Case::As { k += if matches!(a.case, Case::LowerNoAccents | Case::UpperNoAccents) { 0.3 } else { 0.2 }; }
    if a.slice != 0 { k += 0.4 + 0.05 * a.slice.abs() as f64; }
    k
}

fn const_cost(s: &str) -> f64 {
    let mut k = 1.0;
    for c in s.chars() { k += if is_letter_or_number(c) { 1.5 } else { 0.15 }; }
    k
}

/// Every atom of a row whose text appears somewhere in the example `o`.
fn atoms(st: &State, col: usize, id: RowId, o: &str) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    let mut descs = Vec::new();
    for c in 0..st.ncols.min(COLS_MAX) {
        if c == col { continue; }
        let s = src(st, id, c, &mut tmp);
        if s.is_empty() || s.chars().count() > LEN_MAX { continue; }
        descs.clear();
        descs.push(Piece::All);
        if s.bytes().any(|b| b.is_ascii_digit()) && s.chars().any(|c| !c.is_ascii_digit()) { descs.push(Piece::Digits); }
        for x in Tok::ALL {
            let n = tokens(&s, x).len() as i32;
            if n as usize > PARTS_MAX * 2 { continue; }
            for i in 0..n { descs.push(Piece::Tok(x, i)); descs.push(Piece::Tok(x, i - n)); }
        }
        let mut seps: Vec<char> = Vec::new();
        for ch in s.chars() {
            if !is_letter_or_number(ch) && !seps.contains(&ch) { seps.push(ch); }
        }
        for x in seps {
            let n = s.split(x).count() as i32;
            if n < 2 || n as usize > PARTS_MAX { continue; }
            for i in 0..n {
                if !x.is_whitespace() { descs.push(Piece::Field(x, i)); descs.push(Piece::Field(x, i - n)); }
                if i >= 1 { descs.push(Piece::From(x, i)); descs.push(Piece::From(x, i - n)); }
                if i <= n - 2 { descs.push(Piece::To(x, i)); descs.push(Piece::To(x, i - n)); }
            }
        }
        for &d in &descs {
            let Some(b) = base(&s, d) else { continue };
            // Every case, even one that changes nothing here: "DUPONT" as is and
            // in upper case are the same text, but not on the next row's "Martin".
            for cs in Case::ALL {
                let v = cs.apply(&b);
                let mk = |slice| Atom { col: c, piece: d, case: cs, slice };
                if o.contains(v.as_ref()) { out.push(mk(0)); }
                let chars: Vec<(usize, char)> = v.char_indices().collect();
                let len = chars.len();
                for n in 1..=4.min(len.saturating_sub(1)) {
                    if o.contains(&v[..chars[n].0]) { out.push(mk(n as i32)); }
                    if o.contains(&v[chars[len - n].0..]) { out.push(mk(-(n as i32))); }
                }
            }
        }
    }
    out
}

struct Cand {
    atom: Atom,
    /// Its text in each example.
    v: Vec<String>,
    cost: f64,
}

#[derive(Clone, Copy)]
enum Step {
    Start,
    Atom(usize),
    /// Constant text: a slice of the first example.
    Text(usize, usize),
}

struct Node {
    pos: Vec<usize>,
    atom: bool,
    k: bool,
    cost: f64,
    prev: usize,
    step: Step,
}

/// The cheapest rule spelling every example: Dijkstra over the positions
/// reached in each example (plus: an atom used yet, the last piece a
/// constant — two constants in a row are one).
fn search(st: &State, col: usize, exs: &[(RowId, &str)]) -> Option<Vec<Part>> {
    let o: Vec<&str> = exs.iter().map(|x| x.1).collect();
    let o0 = o[0];
    let mut tmp = Vec::new();
    // The source values of the examples, read once.
    let mut srcs: HashMap<(usize, usize), String> = HashMap::new();
    let mut cands: Vec<Cand> = Vec::new();
    for a in atoms(st, col, exs[0].0, o0) {
        let mut v = Vec::with_capacity(exs.len());
        let mut ok = true;
        for (e, x) in exs.iter().enumerate() {
            let s = srcs.entry((e, a.col)).or_insert_with(|| src(st, x.0, a.col, &mut tmp));
            match atom_value(s, &a) {
                Some(t) if o[e].contains(&t) => v.push(t),
                _ => { ok = false; break; }
            }
        }
        if ok { cands.push(Cand { cost: atom_cost(&a), atom: a, v }); }
    }
    // Position in the first example → the atoms that fit there.
    let mut at: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, a) in cands.iter().enumerate() {
        let v0 = &a.v[0];
        let mut from = 0;
        while let Some(p) = o0[from..].find(v0.as_str()) {
            let p = from + p;
            at.entry(p).or_default().push(k);
            from = p + o0[p..].chars().next().map_or(1, char::len_utf8);
        }
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut heap: Vec<usize> = Vec::new();
    let mut best: HashMap<(Vec<usize>, bool, bool), f64> = HashMap::new();
    // The JS binary heap, sift for sift: ties between equal costs break the same way.
    let push = |n: Node, nodes: &mut Vec<Node>, heap: &mut Vec<usize>, best: &mut HashMap<(Vec<usize>, bool, bool), f64>| {
        let key = (n.pos.clone(), n.atom, n.k);
        if best.get(&key).is_some_and(|&b| b <= n.cost) { return; }
        best.insert(key, n.cost);
        nodes.push(n);
        heap.push(nodes.len() - 1);
        let mut i = heap.len() - 1;
        while i > 0 {
            let j = (i - 1) >> 1;
            if nodes[heap[j]].cost <= nodes[heap[i]].cost { break; }
            heap.swap(i, j);
            i = j;
        }
    };
    let pop = |nodes: &Vec<Node>, heap: &mut Vec<usize>| -> usize {
        let top = heap[0];
        let last = heap.pop().unwrap();
        if !heap.is_empty() {
            heap[0] = last;
            let mut i = 0;
            loop {
                let (l, r) = (2 * i + 1, 2 * i + 2);
                let mut m = i;
                if l < heap.len() && nodes[heap[l]].cost < nodes[heap[m]].cost { m = l; }
                if r < heap.len() && nodes[heap[r]].cost < nodes[heap[m]].cost { m = r; }
                if m == i { break; }
                heap.swap(i, m);
                i = m;
            }
        }
        top
    };
    push(Node { pos: vec![0; o.len()], atom: false, k: false, cost: 0.0, prev: usize::MAX, step: Step::Start }, &mut nodes, &mut heap, &mut best);
    let mut seen = 0;
    while !heap.is_empty() && seen < NODES {
        seen += 1;
        let ni = pop(&nodes, &mut heap);
        let (cost, atom, k) = (nodes[ni].cost, nodes[ni].atom, nodes[ni].k);
        let pos = nodes[ni].pos.clone();
        if best.get(&(pos.clone(), atom, k)).is_some_and(|&b| b < cost) { continue; }   // a cheaper way here was expanded already
        if atom && pos.iter().zip(&o).all(|(&p, s)| p == s.len()) {
            let mut parts = Vec::new();
            let mut x = ni;
            while nodes[x].prev != usize::MAX {
                parts.push(match nodes[x].step {
                    Step::Atom(c) => Part::Atom(cands[c].atom),
                    Step::Text(a, b) => Part::Text(o0[a..b].to_string()),
                    Step::Start => unreachable!(),
                });
                x = nodes[x].prev;
            }
            parts.reverse();
            return Some(parts);
        }
        let p0 = pos[0];
        if let Some(list) = at.get(&p0) {
            for &c in list {
                let a = &cands[c];
                if !a.v.iter().enumerate().all(|(e, v)| o[e][pos[e]..].starts_with(v.as_str())) { continue; }
                let np = pos.iter().zip(&a.v).map(|(p, v)| p + v.len()).collect();
                push(Node { pos: np, atom: true, k: false, cost: cost + a.cost, prev: ni, step: Step::Atom(c) }, &mut nodes, &mut heap, &mut best);
            }
        }
        if k { continue; }
        for (off, ch) in o0[p0..].char_indices() {
            let end = p0 + off + ch.len_utf8();
            let s = &o0[p0..end];
            if !o.iter().enumerate().all(|(e, x)| x[pos[e]..].starts_with(s)) { break; }   // longer ones fail too
            let np = pos.iter().map(|p| p + s.len()).collect();
            push(Node { pos: np, atom, k: true, cost: cost + const_cost(s), prev: ni, step: Step::Text(p0, end) }, &mut nodes, &mut heap, &mut best);
        }
    }
    None
}

/// The text a rule gives for a row: None when one of its atoms finds nothing there.
fn run_parts(st: &State, parts: &[Part], id: RowId, tmp: &mut Vec<u8>) -> Option<String> {
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Text(t) => out.push_str(t),
            Part::Atom(a) => out.push_str(&st.with_cell(id, a.col, tmp, |s| atom_value(s.trim(), a))?),
        }
    }
    Some(out)
}

/// The rule for examples (row, result wanted) in column `col`: searched on
/// the first few, checked on the others; one it gets wrong joins the search
/// (a few rounds at most). Results are trimmed; empty or overlong ones are
/// left out. None: no rule gives them from the other columns of the rows.
pub fn infer(examples: &[(RowId, String)], col: usize, st: &State) -> Option<Rule> {
    let exs: Vec<(RowId, &str)> = examples
        .iter()
        .map(|(id, s)| (*id, s.trim()))
        .filter(|(_, s)| !s.is_empty() && s.chars().count() <= LEN_MAX)
        .collect();
    if exs.is_empty() { return None; }
    let mut use_: Vec<usize> = (0..exs.len().min(SYNTH_EX)).collect();
    let mut tmp = Vec::new();
    for _round in 0..5 {
        let set: Vec<(RowId, &str)> = use_.iter().map(|&k| exs[k]).collect();
        let parts = search(st, col, &set)?;
        let bad = (0..exs.len()).find(|k| !use_.contains(k) && run_parts(st, &parts, exs[*k].0, &mut tmp).as_deref() != Some(exs[*k].1));
        match bad {
            None => return Some(Rule { col, parts }),
            Some(k) => use_.push(k),
        }
    }
    None
}

/// The column's filled cells among `rows`, as examples (Ctrl+E with nothing
/// typed: a partly empty column is filled the way the rest was).
pub fn column_examples(st: &State, rows: &[RowId], col: usize) -> Vec<(RowId, String)> {
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    for &id in rows {
        let v = src(st, id, col, &mut tmp);
        if !v.is_empty() {
            out.push((id, v));
            if out.len() >= MAX_EX { break; }
        }
    }
    out
}

impl Rule {
    /// The rule's value for a row (whatever its cell holds); None when an atom finds nothing there.
    pub fn run(&self, st: &State, id: RowId, tmp: &mut Vec<u8>) -> Option<String> {
        run_parts(st, &self.parts, id, tmp)
    }

    /// Its value for a row whose cell is empty — the cells a fill writes.
    #[inline]
    pub fn fill(&self, st: &State, id: RowId, tmp: &mut Vec<u8>) -> Option<String> {
        if !st.with_cell(id, self.col, tmp, |s| s.trim().is_empty()) { return None; }
        self.run(st, id, tmp).filter(|v| !v.is_empty())
    }

    /// The rule in words: one chip per piece.
    pub fn chips(&self, headers: &[String]) -> Vec<Chip> {
        self.parts.iter().map(|p| match p {
            Part::Text(t) => Chip::Text(t.clone()),
            Part::Atom(a) => Chip::Column { name: headers.get(a.col).cloned().unwrap_or_else(|| format!("Column {}", a.col + 1)), detail: describe_atom(a) },
        }).collect()
    }

    /// The chips on one line: `“M. ” + nom (last word · Capitalised)`.
    pub fn describe(&self, headers: &[String]) -> String {
        self.chips(headers).into_iter().map(|c| match c {
            Chip::Text(t) => format!("“{t}”"),
            Chip::Column { name, detail } if detail.is_empty() => name,
            Chip::Column { name, detail } => format!("{name} ({detail})"),
        }).collect::<Vec<_>>().join(" + ")
    }
}

fn ord(i: i32) -> String {
    if i < 0 {
        return match i { -1 => "last".into(), -2 => "second to last".into(), _ => format!("{}th from last", -i) };
    }
    match i { 0 => "first".into(), 1 => "second".into(), 2 => "third".into(), _ => format!("{}th", i + 1) }
}

fn describe_atom(a: &Atom) -> String {
    let q = |x: char| format!("“{}”", match x { '\t' => "tab".to_string(), ' ' => "space".to_string(), _ => x.to_string() });
    let mut parts = Vec::new();
    match a.piece {
        Piece::All => {}
        Piece::Digits => parts.push("digits".to_string()),
        Piece::Tok(x, i) => parts.push(format!("{} {}", ord(i), if x == Tok::N { "number" } else { "word" })),
        Piece::Field(x, i) => parts.push(match i { 0 => format!("before the first {}", q(x)), -1 => format!("after the last {}", q(x)), _ => format!("{} part between {}", ord(i), q(x)) }),
        Piece::From(x, i) => parts.push(format!("after the {} {}", ord(if i < 0 { i } else { i - 1 }), q(x))),
        Piece::To(x, i) => parts.push(format!("before the {} {}", ord(if i < 0 { i + 1 } else { i }), q(x))),
    }
    match a.slice {
        0 => {}
        1 => parts.push("initial".into()),
        n if n > 0 => parts.push(format!("first {n} characters")),
        -1 => parts.push("last character".into()),
        n => parts.push(format!("last {} characters", -n)),
    }
    let cs = match a.case { Case::Upper => "UPPER", Case::Lower => "lower", Case::Capital => "Capitalised", Case::LowerNoAccents => "lower, no accents", Case::UpperNoAccents => "UPPER, no accents", Case::As => "" };
    if !cs.is_empty() { parts.push(cs.into()); }
    parts.join(" · ")
}

/// The cells a fill writes among `rows`: (row, column, value) for each empty
/// cell of the rule's column the rule gives a value for — in parallel, in
/// the order of `rows`. None if cancelled. `progress` counts rows.
pub fn apply(st: &State, rule: &Rule, rows: &[RowId], cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Vec<(RowId, usize, String)>> {
    const CH: usize = 1 << 14;
    let out: Vec<(RowId, usize, String)> = rows
        .par_chunks(CH)
        .flat_map_iter(|chunk| {
            let mut tmp = Vec::new();
            let mut v = Vec::new();
            if !cancel.load(Ordering::Relaxed) {
                for &id in chunk {
                    if let Some(s) = rule.fill(st, id, &mut tmp) { v.push((id, rule.col, s)); }
                }
                if let Some(p) = progress { p.fetch_add(chunk.len(), Ordering::Relaxed); }
            }
            v
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

/// How many empty cells among `rows` the rule fills. None if cancelled.
pub fn count(st: &State, rule: &Rule, rows: &[RowId], cancel: &AtomicBool) -> Option<usize> {
    let n = rows
        .par_iter()
        .with_min_len(4096)
        .map_init(Vec::new, |tmp, &id| if cancel.load(Ordering::Relaxed) { 0 } else { rule.fill(st, id, tmp).is_some() as usize })
        .sum();
    if cancel.load(Ordering::Relaxed) { None } else { Some(n) }
}

/// The first `n` fills among `rows`, in their order (the ghosts shown).
pub fn preview(st: &State, rule: &Rule, rows: &[RowId], n: usize) -> Vec<(RowId, String)> {
    let mut tmp = Vec::new();
    rows.iter().filter_map(|&id| rule.fill(st, id, &mut tmp).map(|v| (id, v))).take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    const CLIENTS: &str = "civilite;prenom;nom;client;naissance;tel;ville;cible\r\n\
        M.;Jean;DUPONT;M. Jean DUPONT;1984-03-15;06 12 34 56 78;Paris;\r\n\
        Mme;Élodie;Martin;Mme Élodie Martin;1990-11-02;07 98 76 54 32;Lyon;\r\n\
        M.;Jean-Pierre;LE GALL;M. Jean-Pierre LE GALL;1975-07-30;01 23 45 67 89;Brest;\r\n\
        Mme;Anne;ROUX;Mme Anne ROUX;2001-01-09;06 00 00 00 01;Nantes;\r\n\
        M.;Paul;Petit;M. Paul Petit;1968-12-24;;Paris;\r\n";

    fn table() -> Table {
        Table::new(Arc::new(Base::from_bytes(CLIENTS.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn rule(t: &Table, ex: &[(usize, &str)]) -> Option<Vec<Option<String>>> {
        let st = &t.st;
        let exs: Vec<(RowId, String)> = ex.iter().map(|&(p, v)| (st.data_id(p), v.to_string())).collect();
        let r = infer(&exs, 7, st)?;
        let mut tmp = Vec::new();
        Some((0..st.len()).map(|p| r.run(st, st.data_id(p), &mut tmp)).collect())
    }

    fn some(v: &[&str]) -> Option<Vec<Option<String>>> {
        Some(v.iter().map(|s| Some(s.to_string())).collect())
    }

    #[test]
    fn word_in_another_case() {
        let t = table();
        assert_eq!(rule(&t, &[(0, "Dupont"), (1, "Martin")]), some(&["Dupont", "Martin", "Le Gall", "Roux", "Petit"]));
        let exs = vec![(t.st.data_id(0), "Dupont".to_string()), (t.st.data_id(1), "Martin".to_string())];
        let r = infer(&exs, 7, &t.st).unwrap();
        assert!(r.describe(&t.st.headers()).contains("Capitalised"), "{}", r.describe(&t.st.headers()));
    }

    #[test]
    fn email_from_two_columns() {
        let t = table();
        assert_eq!(rule(&t, &[(0, "jean.dupont@acme.fr"), (1, "elodie.martin@acme.fr")]),
            some(&["jean.dupont@acme.fr", "elodie.martin@acme.fr", "jean-pierre.le gall@acme.fr", "anne.roux@acme.fr", "paul.petit@acme.fr"]));
    }

    #[test]
    fn date_turned_around() {
        let t = table();
        assert_eq!(rule(&t, &[(0, "15/03/1984")]), some(&["15/03/1984", "02/11/1990", "30/07/1975", "09/01/2001", "24/12/1968"]));
    }

    #[test]
    fn initials_and_digits() {
        let t = table();
        assert_eq!(rule(&t, &[(0, "J. DUPONT"), (1, "É. MARTIN")]), some(&["J. DUPONT", "É. MARTIN", "J. LE GALL", "A. ROUX", "P. PETIT"]));
        let mut want = some(&["0612345678", "0798765432", "0123456789", "0600000001"]).unwrap();
        want.push(None);
        assert_eq!(rule(&t, &[(0, "0612345678"), (1, "0798765432")]), Some(want));
        assert_eq!(rule(&t, &[(0, "J.D."), (1, "É.M.")]), some(&["J.D.", "É.M.", "J.L.", "A.R.", "P.P."]));
    }

    #[test]
    fn no_rule_for_unrelated_values() {
        assert_eq!(rule(&table(), &[(0, "rouge"), (1, "vert")]), None);
    }

    #[test]
    fn fill_empty_cells_only() {
        let mut t = table();
        let ids: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        t.set_cells(&[(ids[0], 7, "Dupont".into()), (ids[1], 7, "Martin".into())]);
        // Nothing typed: the column's filled cells are the examples.
        let exs = column_examples(&t.st, &ids, 7);
        assert_eq!(exs.len(), 2);
        let r = infer(&exs, 7, &t.st).unwrap();
        let no = AtomicBool::new(false);
        assert_eq!(count(&t.st, &r, &ids, &no), Some(3));
        assert_eq!(preview(&t.st, &r, &ids, 2), vec![(ids[2], "Le Gall".to_string()), (ids[3], "Roux".to_string())]);
        let ch = apply(&t.st, &r, &ids, &no, None).unwrap();
        assert_eq!(ch.iter().map(|c| c.2.as_str()).collect::<Vec<_>>(), ["Le Gall", "Roux", "Petit"]);
        t.set_cells(&ch);
        assert_eq!((0..5).map(|p| t.st.cell(ids[p], 7)).collect::<Vec<_>>(), ["Dupont", "Martin", "Le Gall", "Roux", "Petit"]);
        t.undo();
        assert_eq!(t.st.cell(ids[4], 7), "");
    }

    #[test]
    fn pieces_and_words() {
        assert_eq!(base("a-b-c", Piece::From('-', 1)).as_deref(), Some("b-c"));
        assert_eq!(base("a-b-c", Piece::To('-', -2)).as_deref(), Some("a-b"));
        assert_eq!(base("a-b-c", Piece::To('-', 2)), None);
        assert_eq!(base("abc", Piece::Field('-', 0)), None);
        assert_eq!(base("x 12 y 7", Piece::Tok(Tok::N, -1)).as_deref(), Some("7"));
        assert_eq!(Case::Capital.apply("jean-pierre o'neil LE GALL"), "Jean-Pierre O'neil Le Gall");
        assert_eq!(Case::UpperNoAccents.apply("Élodie"), "ELODIE");
        let a = Atom { col: 0, piece: Piece::Tok(Tok::W, -1), case: Case::Capital, slice: 0 };
        assert_eq!(describe_atom(&a), "last word · Capitalised");
        let a = Atom { col: 0, piece: Piece::From('@', 1), case: Case::As, slice: 1 };
        assert_eq!(describe_atom(&a), "after the first “@” · initial");
    }
}
