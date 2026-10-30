//! The filter and sort as they were before the column caches and radix sorts:
//! the reference the property tests compare the fast paths with.
#![allow(dead_code)]

use crate::table::{RowId, State};
pub use super::{FilterSpec, Kind, SortKey};
use crate::text::{fold_into, parse_date, parse_number};
use memchr::memmem;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

enum Pat {
    Sub { needle: String, finder: memmem::Finder<'static>, raw_ok: bool },
    Re(regex::Regex),
}

impl Pat {
    fn new(text: &str, regex: bool, case: bool) -> Result<Pat, String> {
        if regex {
            let re = regex::RegexBuilder::new(text).case_insensitive(!case).size_limit(1 << 22).build().map_err(|e| e.to_string())?;
            return Ok(Pat::Re(re));
        }
        let needle = if case { text.to_string() } else { crate::text::fold(text) };
        let finder = memmem::Finder::new(needle.as_bytes()).into_owned();
        // Searchable in a record's raw bytes when it cannot straddle two fields.
        let raw_ok = needle.is_ascii() && !needle.bytes().any(|b| matches!(b, b'"' | b'\n' | b'\r' | b';' | b',' | b'\t' | b'|'));
        Ok(Pat::Sub { needle, finder, raw_ok })
    }

    #[inline]
    fn test(&self, s: &str, case: bool, buf: &mut String) -> bool {
        match self {
            Pat::Re(re) => re.is_match(s),
            Pat::Sub { finder, needle, .. } => {
                if needle.is_empty() { return true; }
                if case { return finder.find(s.as_bytes()).is_some(); }
                buf.clear();
                fold_into(s, buf);
                finder.find(buf.as_bytes()).is_some()
            }
        }
    }
}

enum Cmp { Lt, Le, Gt, Ge }

enum ColTest {
    Contains(Pat),
    NotContains(Pat),
    Exact(String),
    NotExact(String),
    Num(Cmp, f64),
}

fn col_test(text: &str, regex: bool, case: bool) -> Result<ColTest, String> {
    if regex {
        return Ok(ColTest::Contains(Pat::new(text, true, case)?));
    }
    let norm = |s: &str| if case { s.to_string() } else { crate::text::fold(s) };
    let num = |s: &str| parse_number(s).or_else(|| parse_date(s)).ok_or_else(|| format!("not a number: {s}"));
    Ok(if let Some(r) = text.strip_prefix(">=") {
        ColTest::Num(Cmp::Ge, num(r)?)
    } else if let Some(r) = text.strip_prefix("<=") {
        ColTest::Num(Cmp::Le, num(r)?)
    } else if let Some(r) = text.strip_prefix('>') {
        ColTest::Num(Cmp::Gt, num(r)?)
    } else if let Some(r) = text.strip_prefix('<') {
        ColTest::Num(Cmp::Lt, num(r)?)
    } else if let Some(r) = text.strip_prefix("!=") {
        ColTest::NotExact(norm(r))
    } else if let Some(r) = text.strip_prefix('!') {
        if r.is_empty() { ColTest::NotExact(String::new()) } else { ColTest::NotContains(Pat::new(r, false, case)?) }
    } else if let Some(r) = text.strip_prefix('=') {
        ColTest::Exact(norm(r))
    } else {
        ColTest::Contains(Pat::new(text, false, case)?)
    })
}

impl ColTest {
    fn test(&self, s: &str, case: bool, buf: &mut String) -> bool {
        let eq = |want: &str, buf: &mut String| {
            if case { return s.trim() == want; }
            buf.clear();
            fold_into(s.trim(), buf);
            buf == want
        };
        match self {
            ColTest::Contains(p) => p.test(s, case, buf),
            ColTest::NotContains(p) => !p.test(s, case, buf),
            ColTest::Exact(w) => eq(w, buf),
            ColTest::NotExact(w) => !eq(w, buf),
            ColTest::Num(c, v) => match parse_number(s).or_else(|| parse_date(s)) {
                None => false,
                Some(x) => match c { Cmp::Lt => x < *v, Cmp::Le => x <= *v, Cmp::Gt => x > *v, Cmp::Ge => x >= *v },
            },
        }
    }
}

pub struct Filter {
    global: Vec<Pat>,
    invert: bool,
    case: bool,
    cols: Vec<(usize, ColTest)>,
    hidden: Vec<(usize, std::sync::Arc<std::collections::HashSet<String>>)>,
}

impl Filter {
    pub fn compile(spec: &FilterSpec) -> Result<Filter, String> {
        let global = if spec.global.is_empty() {
            Vec::new()
        } else if spec.regex {
            vec![Pat::new(&spec.global, true, spec.match_case)?]
        } else {
            super::terms(&spec.global).iter().map(|t| Pat::new(t, false, spec.match_case)).collect::<Result<_, _>>()?
        };
        let mut cols = Vec::new();
        for (c, t) in &spec.cols {
            if t.is_empty() { continue; }
            // Plain text (no operator): every word in the cell, any order; "a phrase" whole.
            if !spec.regex && !t.starts_with(['>', '<', '!', '=']) {
                for w in super::terms(t) { cols.push((*c, col_test(&w, false, spec.match_case)?)); }
            } else {
                cols.push((*c, col_test(t, spec.regex, spec.match_case)?));
            }
        }
        let hidden = spec.hidden.iter().filter(|(_, h)| !h.is_empty()).cloned().collect();
        Ok(Filter { global, invert: spec.invert, case: spec.match_case, cols, hidden })
    }

    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.cols.is_empty() && self.hidden.is_empty()
    }

    fn row_matches(&self, st: &State, id: RowId, s: &mut Scratch) -> bool {
        for (c, h) in &self.hidden {
            if st.with_cell(id, *c, &mut s.tmp, |v| h.contains(v)) {
                return false;
            }
        }
        for (c, t) in &self.cols {
            let case = self.case;
            if !st.with_cell(id, *c, &mut s.tmp, |v| t.test(v, case, &mut s.buf)) {
                return false;
            }
        }
        if self.global.is_empty() { return true; }
        self.global.iter().all(|g| self.global_matches(st, id, g, s)) != self.invert
    }

    fn global_matches(&self, st: &State, id: RowId, g: &Pat, s: &mut Scratch) -> bool {
        let b = &st.base;
        if st.pristine(id) {
            let rec = b.record(id as usize);
            // Fast path: an ASCII record and a needle that cannot span two fields —
            // a match in the raw bytes is a match in one cell, and the other way round.
            // (Not for spaces alone: they may sit after a closing quote, outside any cell.)
            if let Pat::Sub { finder, raw_ok: true, needle } = g {
                if rec.is_ascii() && !(!needle.is_empty() && needle.bytes().all(|c| c == b' ' || c == b'\t')) {
                    if needle.is_empty() { return true; }
                    if self.case { return finder.find(rec).is_some(); }
                    s.low.clear();
                    s.low.extend(rec.iter().map(|c| c.to_ascii_lowercase()));
                    return finder.find(&s.low).is_some();
                }
            }
            let (enc, case) = (b.enc, self.case);
            if b.dialect.lines {
                return g.test(&crate::enc::decode(enc, rec), case, &mut s.buf);
            }
            let mut hit = false;
            let buf = &mut s.buf;
            crate::scan::visit_fields(rec, b.dialect.delim, &mut s.tmp, |_, f| {
                hit = g.test(&crate::enc::decode(enc, f), case, buf);
                !hit
            });
            return hit;
        }
        let mut row = Vec::new();
        st.row(id, &mut row);
        row.iter().any(|v| g.test(v, self.case, &mut s.buf))
    }
}

#[derive(Default)]
struct Scratch {
    tmp: Vec<u8>,
    buf: String,
    low: Vec<u8>,
}

/// The data rows (file order) that pass the filter; None if cancelled.
pub fn filter(st: &State, f: &Filter, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    let n = st.len();
    if f.is_empty() {
        return Some((0..n).map(|p| st.data_id(p)).collect());
    }
    let out: Vec<RowId> = (0..n)
        .into_par_iter()
        .with_min_len(4096)
        .map_init(Scratch::default, |s, p| {
            if cancel.load(Ordering::Relaxed) { return None; }
            let id = st.data_id(p);
            if f.row_matches(st, id, s) { Some(id) } else { None }
        })
        .flatten()
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

/// Of these rows (in this order), those that pass the filter; None if cancelled.
pub fn filter_ids(st: &State, f: &Filter, ids: &[RowId], cancel: &AtomicBool) -> Option<Vec<RowId>> {
    if f.is_empty() {
        return Some(ids.to_vec());
    }
    let out: Vec<RowId> = ids
        .par_iter()
        .with_min_len(4096)
        .map_init(Scratch::default, |s, &id| {
            if cancel.load(Ordering::Relaxed) { return None; }
            if f.row_matches(st, id, s) { Some(id) } else { None }
        })
        .flatten()
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

/// What a column holds, from a sample of its non-empty cells: numbers or dates
/// when 95 % of them read as such.
pub fn detect_kinds(st: &State) -> Vec<Kind> {
    let n = st.len();
    let sample: Vec<usize> = if n <= 4000 { (0..n).collect() } else { (0..4000).map(|k| k * n / 4000).collect() };
    (0..st.ncols)
        .into_par_iter()
        .map(|c| {
            let (mut filled, mut nums, mut dates) = (0, 0, 0);
            let mut tmp = Vec::new();
            for &p in &sample {
                st.with_cell(st.data_id(p), c, &mut tmp, |v| {
                    let v = v.trim();
                    if v.is_empty() { return; }
                    filled += 1;
                    if parse_date(v).is_some() { dates += 1 } else if parse_number(v).is_some() { nums += 1 }
                });
            }
            if filled == 0 { Kind::Text }
            else if dates * 100 >= filled * 95 { Kind::Date }
            else if (nums + dates) * 100 >= filled * 95 && nums > 0 { Kind::Number }
            else { Kind::Text }
        })
        .collect()
}

enum Keys {
    /// NaN: empty or not a number — always last.
    Num(Vec<f64>),
    /// Folded text, in arenas of CH rows; empty — always last. Then each row's tie key.
    Text(Vec<(Vec<u8>, Vec<u32>)>, Vec<Vec<u8>>),
}

const CH: usize = 1 << 14;

impl Keys {
    #[inline]
    fn text(&self, p: usize) -> &[u8] {
        let Keys::Text(ch, _) = self else { unreachable!() };
        let (arena, offs) = &ch[p / CH];
        let k = p % CH;
        &arena[offs[k] as usize..offs[k + 1] as usize]
    }

    fn tie(&self, p: usize) -> &[u8] {
        let Keys::Text(_, ties) = self else { unreachable!() };
        &ties[p]
    }
}

fn build_keys(st: &State, ids: &[RowId], col: usize, kind: Kind, cancel: &AtomicBool) -> Keys {
    match kind {
        Kind::Number | Kind::Date => Keys::Num(
            ids.par_iter()
                .with_min_len(4096)
                .map_init(Vec::new, |tmp, &id| {
                    if cancel.load(Ordering::Relaxed) { return f64::NAN; }
                    st.with_cell(id, col, tmp, |v| {
                        let v = v.trim();
                        let x = if kind == Kind::Date { parse_date(v) } else { parse_number(v) };
                        x.unwrap_or(f64::NAN)
                    })
                })
                .collect(),
        ),
        Kind::Text => Keys::Text(
            ids.par_chunks(CH)
                .map(|chunk| {
                    let mut arena = Vec::with_capacity(chunk.len() * 12);
                    let mut offs = Vec::with_capacity(chunk.len() + 1);
                    let (mut tmp, mut buf) = (Vec::new(), String::new());
                    offs.push(0u32);
                    for &id in chunk {
                        if cancel.load(Ordering::Relaxed) { break; }
                        st.with_cell(id, col, &mut tmp, |v| {
                            buf.clear();
                            fold_into(v.trim(), &mut buf);
                        });
                        crate::text::sort_key_into(buf.as_bytes(), &mut arena);
                        offs.push(arena.len() as u32);
                    }
                    offs.resize(chunk.len() + 1, arena.len() as u32);
                    (arena, offs)
                })
                .collect(),
            ids.par_iter()
                .map_init(Vec::new, |tmp, &id| {
                    let mut out = Vec::new();
                    st.with_cell(id, col, tmp, |v| crate::text::tie_key_into(v.trim(), &mut out));
                    out
                })
                .collect(),
        ),
    }
}

/// Sorts `ids` on the keys, in parallel (stable: ties keep the file order).
/// Empty and unreadable values go last, ascending or descending.
pub fn sort(st: &State, ids: &mut Vec<RowId>, keys: &[SortKey], kinds: &[Kind], cancel: &AtomicBool) -> bool {
    if keys.is_empty() || ids.len() < 2 {
        return true;
    }
    let cols: Vec<(Keys, bool)> = keys
        .iter()
        .map(|k| (build_keys(st, ids, k.col, kinds.get(k.col).copied().unwrap_or(Kind::Text), cancel), k.desc))
        .collect();
    if cancel.load(Ordering::Relaxed) { return false; }
    let mut perm: Vec<u32> = (0..ids.len() as u32).collect();
    perm.par_sort_by(|&a, &b| {
        let (a, b) = (a as usize, b as usize);
        for (k, desc) in &cols {
            let o = match k {
                Keys::Num(v) => {
                    let (x, y) = (v[a], v[b]);
                    match (x.is_nan(), y.is_nan()) {
                        (true, true) => std::cmp::Ordering::Equal,
                        (true, false) => return std::cmp::Ordering::Greater,
                        (false, true) => return std::cmp::Ordering::Less,
                        _ => { let o = x.partial_cmp(&y).unwrap(); if *desc { o.reverse() } else { o } }
                    }
                }
                Keys::Text(..) => {
                    let (x, y) = (k.text(a), k.text(b));
                    match (x.is_empty(), y.is_empty()) {
                        (true, true) => std::cmp::Ordering::Equal,
                        (true, false) => return std::cmp::Ordering::Greater,
                        (false, true) => return std::cmp::Ordering::Less,
                        _ => { let o = x.cmp(y).then_with(|| k.tie(a).cmp(k.tie(b))); if *desc { o.reverse() } else { o } }
                    }
                }
            };
            if o != std::cmp::Ordering::Equal { return o; }
        }
        std::cmp::Ordering::Equal
    });
    if cancel.load(Ordering::Relaxed) { return false; }
    let sorted: Vec<RowId> = perm.par_iter().map(|&p| ids[p as usize]).collect();
    *ids = sorted;
    true
}

