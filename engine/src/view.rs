//! The rows shown: filtered and sorted in parallel over a snapshot of the
//! table, cancellable. The result is a list of row ids.
//!
//! A filter runs over chunks of rows, one test at a time, cheapest first: a
//! column test reads the column's cache when there is one (`colcache`: a
//! verdict per distinct value, numbers parsed once, folded text), else the
//! cell; the search in every cell scans runs of untouched records as one block
//! of the file. A sort turns each key into a u64 per row — a number's bits, a
//! text's rank — and radix sorts them (`radix`). Rows edited here are always
//! read live, so the caches never need invalidating.

use crate::colcache::{self, Bits, Col, Need};
use crate::table::{RowId, State};
use crate::text::{fold, fold_into, parse_date, parse_number};
use memchr::memmem;
use rayon::prelude::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterSpec {
    /// Searched in every cell.
    pub global: String,
    pub regex: bool,
    pub match_case: bool,
    pub invert: bool,
    /// Per column: (column, text). The text may start with an operator:
    /// `=x` exactly x (`=` alone: empty), `!x` not containing x (`!` alone:
    /// not empty), `!=x`, `>n`, `>=n`, `<n`, `<=n` (numbers or dates).
    pub cols: Vec<(usize, String)>,
    /// Per column, values hidden (from the column panel or a cell's menu): a row whose cell is
    /// one of them is out. Exact text, as written ("Paris " is not "Paris").
    pub hidden: Vec<(usize, Arc<HashSet<String>>)>,
    /// Values kept: a chart's part picked (a bar, a range of bars, a value, the empty cells),
    /// a value from a cell's menu. With `hidden`, the value filters: Invert leaves them alone.
    pub picks: Vec<ValuePick>,
}

/// A value filter that keeps rows: those whose cell in `col` passes `keep`.
#[derive(Clone)]
pub struct ValuePick {
    pub col: usize,
    pub keep: Keeps,
}

#[derive(Clone)]
pub enum Keeps {
    /// A chart's part: its test (`sparkline::Spark::keep`, the one its hover lights).
    Part(Arc<crate::sparkline::Spark>, crate::sparkline::Pick),
    /// These values exactly, as written (a cell's menu: "Only the rows with …").
    Values(Arc<HashSet<String>>),
    /// A stretch of a histogram: the numbers or dates (in seconds) from `lo` to `hi`, both
    /// kept, and how its chip says it.
    Range { kind: Kind, lo: f64, hi: f64, label: Arc<str> },
}

impl PartialEq for ValuePick {
    fn eq(&self, o: &ValuePick) -> bool {
        self.col == o.col && match (&self.keep, &o.keep) {
            (Keeps::Part(a, x), Keeps::Part(b, y)) => x == y && Arc::ptr_eq(a, b),
            (Keeps::Values(a), Keeps::Values(b)) => a == b,
            (Keeps::Range { kind: k, lo: a, hi: z, .. }, Keeps::Range { kind: l, lo: b, hi: y, .. }) => k == l && a == b && z == y,
            _ => false,
        }
    }
}

impl std::fmt::Debug for ValuePick {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self.keep {
            Keeps::Part(_, p) => write!(f, "ValuePick({}, {p:?})", self.col),
            Keeps::Values(v) => write!(f, "ValuePick({}, {v:?})", self.col),
            Keeps::Range { lo, hi, .. } => write!(f, "ValuePick({}, {lo}..={hi})", self.col),
        }
    }
}

impl ValuePick {
    #[inline]
    pub fn test(&self, v: &str) -> bool {
        match &self.keep {
            Keeps::Part(s, p) => s.keep(self.col, *p).test(v),
            Keeps::Values(set) => set.contains(v),
            Keeps::Range { kind, lo, hi, .. } => {
                let v = v.trim();
                let x = if *kind == Kind::Date { crate::sparkline::time_key(v) } else { crate::text::parse_number(v) };
                x.is_some_and(|x| x >= *lo && x <= *hi)
            }
        }
    }
}

impl FilterSpec {
    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.cols.iter().all(|(_, s)| s.is_empty()) && !self.has_values()
    }

    /// Whether value filters (values hidden, chart picks) are set.
    pub fn has_values(&self) -> bool {
        self.hidden.iter().any(|(_, h)| !h.is_empty()) || !self.picks.is_empty()
    }

    /// The value filters alone (`hidden`, `picks`).
    pub fn values(&self) -> FilterSpec {
        FilterSpec { hidden: self.hidden.clone(), picks: self.picks.clone(), ..Default::default() }
    }

    /// The text tests alone (the search, the column filters): what Invert flips.
    pub fn texts(&self) -> FilterSpec {
        FilterSpec { hidden: Vec::new(), picks: Vec::new(), ..self.clone() }
    }

    /// Whether every row this filter keeps, `prev` keeps too (on the same
    /// table): then this one need only look at `prev`'s rows — typing one more
    /// letter, adding a column filter or hiding one more value.
    pub fn refines(&self, prev: &FilterSpec) -> bool {
        if self.regex != prev.regex || self.match_case != prev.match_case {
            return false;
        }
        let norm = |s: &str| if self.match_case { s.to_string() } else { fold(s) };
        let op = |s: &str| s.starts_with(['>', '<', '!', '=']);
        // A plain text (no operator) keeps the cells containing it: a longer one keeps fewer.
        let narrower = |new: &str, old: &str| new == old || (!self.regex && !op(new) && !op(old) && norm(new).contains(&norm(old)));
        if !prev.global.is_empty() {
            if self.global.is_empty() || self.invert != prev.invert {
                return false;
            }
            let ok = self.global == prev.global
                || (!self.regex && if self.invert {
                    // Rows without "lyo" are among those without "lyon": the other way round.
                    norm(&prev.global).contains(&norm(&self.global))
                } else {
                    norm(&self.global).contains(&norm(&prev.global))
                });
            if !ok {
                return false;
            }
        }
        let cols_ok = prev.cols.iter().filter(|(_, t)| !t.is_empty()).all(|(c, old)| self.cols.iter().any(|(k, new)| k == c && narrower(new, old)));
        let hidden_ok = prev
            .hidden
            .iter()
            .filter(|(_, h)| !h.is_empty())
            .all(|(c, old)| self.hidden.iter().any(|(k, new)| k == c && (Arc::ptr_eq(new, old) || new.is_superset(old))));
        cols_ok && hidden_ok && prev.picks.iter().all(|p| self.picks.contains(p))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Number,
    Date,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    pub col: usize,
    pub desc: bool,
}

/// Bytes a substring may not hold to be searched in a record's raw bytes: it
/// could straddle two fields, or a quote that is not in the cell.
const SEPARATORS: &[u8] = b"\"\n\r;,\t|";

enum Pat {
    Sub {
        needle: String,
        finder: memmem::Finder<'static>,
        /// Searchable in a record's raw bytes: ASCII and no separator.
        raw_ok: bool,
        /// No separator: a cell holding it means the whole record (decoded, folded) does.
        rec_ok: bool,
        /// Only spaces and tabs: could be found between a closing quote and a delimiter.
        blank: bool,
        /// The needle's bytes (bit b: byte b).
        mask: u128,
        /// An ASCII needle, case ignored: found in the raw bytes as they are (SIMD, both cases
        /// at once), not in a lower-cased copy of them.
        ci: Option<crate::cifind::CiFinder>,
        /// Bit `u - 0x80`: the two-byte character U+u (up to U+07FF) folds into one of the
        /// needle's letters (`text::folds_into`) — the accents of a text told by a table.
        fold2: Box<[u64; 30]>,
    },
    Re {
        re: crate::re::Re,
        pre: Option<Pre>,
        /// No anchor or word boundary: a match in a cell is a match in any text holding the cell.
        local: bool,
    },
}

/// Literals one of which starts every match of a regex: a record without any
/// of them holds no match.
struct Pre {
    ac: aho_corasick::AhoCorasick,
    ascii: bool,
}

fn literals(pattern: &str, case_insensitive: bool) -> Option<Pre> {
    use regex_syntax::hir::literal::{ExtractKind, Extractor};
    let hir = regex_syntax::ParserBuilder::new().case_insensitive(case_insensitive).build().parse(pattern).ok()?;
    let mut ex = Extractor::new();
    ex.kind(ExtractKind::Prefix);
    let seq = ex.extract(&hir);
    let lits = seq.literals()?;
    // A quote is unescaped in the cell, U+FFFD replaces bad bytes: not in the raw record as such.
    let bad = |l: &[u8]| l.is_empty() || l.iter().any(|b| matches!(b, b'"' | b'\n' | b'\r')) || memmem::find(l, "\u{FFFD}".as_bytes()).is_some();
    if lits.is_empty() || lits.iter().any(|l| bad(l.as_bytes())) {
        return None;
    }
    let ascii = lits.iter().all(|l| l.as_bytes().is_ascii());
    let ac = aho_corasick::AhoCorasick::new(lits.iter().map(|l| l.as_bytes())).ok()?;
    Some(Pre { ac, ascii })
}

impl Pat {
    fn new(text: &str, regex: bool, case: bool) -> Result<Pat, String> {
        if regex {
            let re = crate::re::Re::new(text, !case, 1 << 22)?;
            // Look-around and back-references: the records told by the pattern relaxed.
            let plain = re.plain_pattern().unwrap_or("").to_string();
            let local = !plain.is_empty() && regex_syntax::ParserBuilder::new().case_insensitive(!case).build().parse(&plain).is_ok_and(|h| h.properties().look_set().is_empty());
            let pre = if plain.is_empty() { None } else { literals(&plain, !case) };
            return Ok(Pat::Re { re, pre, local });
        }
        let needle = if case { text.to_string() } else { fold(text) };
        let finder = memmem::Finder::new(needle.as_bytes()).into_owned();
        let rec_ok = !needle.bytes().any(|b| SEPARATORS.contains(&b));
        let raw_ok = rec_ok && needle.is_ascii();
        let blank = needle.bytes().all(|b| b == b' ' || b == b'\t');
        let mask = needle.bytes().filter(|&b| b < 0x80).fold(0u128, |m, b| m | 1 << b);
        let ci = (!case && raw_ok).then(|| crate::cifind::CiFinder::new(needle.as_bytes())).flatten();
        let mut fold2 = Box::new([0u64; 30]);
        if !case && raw_ok {
            for u in 0x80..0x800u32 {
                if char::from_u32(u).is_some_and(|c| crate::text::folds_into(c, mask)) { fold2[(u - 0x80) as usize / 64] |= 1 << ((u - 0x80) % 64); }
            }
        }
        Ok(Pat::Sub { needle, finder, raw_ok, rec_ok, blank, mask, ci, fold2 })
    }

    #[inline]
    fn test(&self, s: &str, case: bool, buf: &mut String) -> bool {
        match self {
            Pat::Re { re, .. } => re.is_match(s),
            Pat::Sub { finder, needle, .. } => {
                if needle.is_empty() { return true; }
                if case { return finder.find(s.as_bytes()).is_some(); }
                buf.clear();
                fold_into(s, buf);
                finder.find(buf.as_bytes()).is_some()
            }
        }
    }

    /// On a cell already folded.
    #[inline]
    fn test_folded(&self, folded: &str) -> bool {
        match self {
            Pat::Sub { finder, needle, .. } => needle.is_empty() || finder.find(folded.as_bytes()).is_some(),
            Pat::Re { .. } => unreachable!(),
        }
    }
}

/// The terms of a search: words, and "phrases in quotes" kept whole (an unclosed quote
/// runs to the end). Each must be found in some cell of a row.
pub fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    // Nothing but quotes or spaces: the text as typed (spaces are searched too).
    if out.is_empty() { out.push(text.to_string()); }
    out
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
    let norm = |s: &str| if case { s.to_string() } else { fold(s) };
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
            ColTest::Num(c, v) => cmp(c, parse_number(s).or_else(|| parse_date(s)).unwrap_or(f64::NAN), *v),
        }
    }

    /// Whether the test reads fold(cell) only (case-insensitive substring or exact text).
    fn on_folded(&self, case: bool) -> bool {
        !case && match self {
            ColTest::Contains(p) | ColTest::NotContains(p) => matches!(p, Pat::Sub { .. }),
            ColTest::Exact(_) | ColTest::NotExact(_) => true,
            ColTest::Num(..) => false,
        }
    }

    /// On a cell already folded (`on_folded`). fold(s.trim()) == fold(s).trim(): text.rs tests it.
    #[inline]
    fn test_folded(&self, f: &str) -> bool {
        match self {
            ColTest::Contains(p) => p.test_folded(f),
            ColTest::NotContains(p) => !p.test_folded(f),
            ColTest::Exact(w) => f.trim() == w,
            ColTest::NotExact(w) => f.trim() != w,
            ColTest::Num(..) => unreachable!(),
        }
    }
}

/// NaN (not a number) never passes.
#[inline]
fn cmp(c: &Cmp, x: f64, v: f64) -> bool {
    match c { Cmp::Lt => x < v, Cmp::Le => x <= v, Cmp::Gt => x > v, Cmp::Ge => x >= v }
}

pub struct Filter {
    /// The search in every cell: each term must be in some cell of the row (any order,
    /// any cells); one regex is one term.
    global: Vec<Pat>,
    invert: bool,
    case: bool,
    cols: Vec<(usize, ColTest)>,
    hidden: Vec<(usize, Arc<HashSet<String>>)>,
    picks: Vec<ValuePick>,
}

impl Filter {
    pub fn compile(spec: &FilterSpec) -> Result<Filter, String> {
        let global = if spec.global.is_empty() {
            Vec::new()
        } else if spec.regex {
            vec![Pat::new(&spec.global, true, spec.match_case)?]
        } else {
            terms(&spec.global).iter().map(|t| Pat::new(t, false, spec.match_case)).collect::<Result<_, _>>()?
        };
        let mut cols = Vec::new();
        for (c, t) in &spec.cols {
            if t.is_empty() { continue; }
            // Plain text (no operator): every word in the cell, any order; "a phrase" whole.
            if !spec.regex && !t.starts_with(['>', '<', '!', '=']) {
                for w in terms(t) { cols.push((*c, col_test(&w, false, spec.match_case)?)); }
            } else {
                cols.push((*c, col_test(t, spec.regex, spec.match_case)?));
            }
        }
        let hidden = spec.hidden.iter().filter(|(_, h)| !h.is_empty()).cloned().collect();
        Ok(Filter { global, invert: spec.invert, case: spec.match_case, cols, hidden, picks: spec.picks.clone() })
    }

    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.cols.is_empty() && self.hidden.is_empty() && self.picks.is_empty()
    }

    /// Builds in the background (`colcache::warm`) the column caches this
    /// filter would read: the next filter on these columns uses them.
    pub fn warm(&self, st: &State, kinds: &[Kind]) {
        for c in self.hidden.iter().map(|h| h.0).chain(self.picks.iter().map(|p| p.col)) {
            colcache::warm(st, c, Need::Dict);
        }
        for (c, t) in &self.cols {
            match t {
                ColTest::Num(..) => {
                    colcache::warm(st, *c, Need::Num);
                    if kinds.get(*c) == Some(&Kind::Date) { colcache::warm(st, *c, Need::Date); }
                }
                t if t.on_folded(self.case) => colcache::warm(st, *c, Need::Text),
                _ => colcache::warm(st, *c, Need::Dict),
            }
        }
    }
}

/// Builds in the background the caches a sort on these keys reads.
pub fn warm_sort(st: &State, keys: &[SortKey], kinds: &[Kind]) {
    for k in keys {
        colcache::warm(st, k.col, need_of(kinds.get(k.col).copied().unwrap_or(Kind::Text)));
    }
}

fn need_of(kind: Kind) -> Need {
    match kind { Kind::Text => Need::Rank, Kind::Number => Need::Num, Kind::Date => Need::Date }
}

#[derive(Default)]
struct Scratch {
    tmp: Vec<u8>,
    buf: String,
    low: Vec<u8>,
    /// Per record of a block: HIT, NONASCII, SUSPECT, UNKNOWN.
    flags: Vec<u8>,
    /// (character, whether it folds into one of the needle's letters) of three-byte characters
    /// met, by code point modulo 64, for the needle `fold3_mask` (0: none yet).
    fold3: Vec<(u32, bool)>,
    fold3_mask: u128,
    row: Vec<String>,
}

enum Test<'a> {
    Hidden(&'a HashSet<String>),
    Pick(&'a ValuePick),
    Col(&'a ColTest),
}

impl Test<'_> {
    /// Whether a row with this cell stays.
    #[inline]
    fn keep(&self, v: &str, case: bool, buf: &mut String) -> bool {
        match self {
            Test::Hidden(h) => !h.contains(v),
            Test::Pick(p) => p.test(v),
            Test::Col(t) => t.test(v, case, buf),
        }
    }
}

/// How a column test reads its rows (cells edited here are always read live).
enum Kernel {
    /// The verdict of each distinct value.
    Dict(Vec<bool>),
    /// Numbers parsed once.
    Num,
    /// fold(cell), kept.
    Folded,
    /// The cell, read now.
    Live,
}

/// One column test, ready to run.
struct Prep<'a> {
    col: usize,
    field: Option<usize>,
    edited: Option<Bits>,
    cache: Col,
    test: Test<'a>,
    kernel: Kernel,
}

impl<'a> Prep<'a> {
    /// `rows`: how many rows the filter goes over. Most of the file: the cache this test
    /// reads is built now (it costs what one pass reading the cells would, and stays).
    fn new(st: &State, col: usize, test: Test<'a>, case: bool, rows: usize, cancel: &AtomicBool) -> Prep<'a> {
        let field = colcache::field_of(st, col);
        let mut cache = if field.is_some() { colcache::get(st, col) } else { Col::default() };
        if field.is_some() && !st.base.partial && rows * 2 >= st.len() && cache.dict.is_none() {
            let need = match &test {
                Test::Hidden(_) | Test::Pick(_) => Some(Need::Dict),
                Test::Col(ColTest::Num(..)) if cache.num.is_none() => Some(Need::Num),
                Test::Col(t) if t.on_folded(case) && cache.folded.is_none() => Some(Need::Text),
                Test::Col(ColTest::Num(..)) => None,
                Test::Col(t) if t.on_folded(case) => None,
                Test::Col(_) => Some(Need::Dict),
            };
            if let Some(need) = need {
                if let Some(c) = colcache::ensure(st, col, need, cancel) { cache = c; }
            }
        }
        let kernel = match (&cache.dict, &test) {
            _ if field.is_none() => Kernel::Live,
            (Some(d), _) => Kernel::Dict(d.values.par_iter().with_min_len(256).map_init(String::new, |buf, v| test.keep(v, case, buf)).collect()),
            (None, Test::Col(ColTest::Num(..))) if cache.num.is_some() => Kernel::Num,
            (None, Test::Col(t)) if t.on_folded(case) && cache.folded.is_some() => Kernel::Folded,
            _ => Kernel::Live,
        };
        Prep { col, field, edited: colcache::edited_rows(st, col), cache, test, kernel }
    }

    /// A substring test on folded text over consecutive records of one arena
    /// chunk: one search through the arena instead of one per cell. False:
    /// not that case, `ids` untouched.
    fn block(&self, st: &State, ids: &mut Vec<RowId>, s: &mut Scratch) -> bool {
        let (Kernel::Folded, Test::Col(t @ (ColTest::Contains(Pat::Sub { finder, needle, .. }) | ColTest::NotContains(Pat::Sub { finder, needle, .. })))) = (&self.kernel, &self.test) else { return false };
        let (first, len) = (ids[0] as usize, ids.len());
        // Consecutive records in file order: the ends alone do not tell (a row moved inside the run).
        if len < 64 || needle.is_empty() || ids[len - 1] as usize != first + len - 1 || first + len > st.base.n || !ids.windows(2).all(|w| w[1] == w[0] + 1) {
            return false;
        }
        let (arena, offs, c0) = self.cache.folded.as_ref().unwrap().chunk(first);
        if first + len > c0 + colcache::CH {
            return false;
        }
        let (a, e) = (first - c0, first - c0 + len);
        s.flags.clear();
        s.flags.resize(len, 0);
        let (lo, hi) = (offs[a] as usize, offs[e] as usize);
        let mut pos = lo;
        let mut r = a;
        while let Some(k) = finder.find(&arena[pos..hi]) {
            let p = pos + k;
            while offs[r + 1] as usize <= p { r += 1; }
            // Cells are end to end: a match running past its cell's end is no match, and
            // none starts later in that cell.
            if p + needle.len() <= offs[r + 1] as usize { s.flags[r - a] = HIT; }
            r += 1;
            if r >= e { break; }
            pos = offs[r] as usize;
        }
        let want = matches!(t, ColTest::Contains(_));
        let mut j = 0;
        let flags = std::mem::take(&mut s.flags);
        ids.retain(|&id| {
            let hit = flags[j] == HIT;
            j += 1;
            if self.edited.as_ref().is_some_and(|e| e.has(id)) {
                return self.keep(st, id, false, s);
            }
            hit == want
        });
        s.flags = flags;
        true
    }

    fn cost(&self) -> u8 {
        match self.kernel { Kernel::Dict(_) => 0, Kernel::Num => 1, Kernel::Folded => 2, Kernel::Live => 3 }
    }

    #[inline]
    fn keep(&self, st: &State, id: RowId, case: bool, s: &mut Scratch) -> bool {
        let live = matches!(self.kernel, Kernel::Live) || id as usize >= st.base.n || self.edited.as_ref().is_some_and(|e| e.has(id));
        if live {
            let (test, buf) = (&self.test, &mut s.buf);
            return st.with_cell(id, self.col, &mut s.tmp, |v| test.keep(v, case, buf));
        }
        let i = id as usize;
        match &self.kernel {
            Kernel::Dict(ok) => ok[self.cache.dict.as_ref().unwrap().codes[i] as usize],
            Kernel::Num => {
                let Test::Col(ColTest::Num(c, v)) = &self.test else { unreachable!() };
                let mut x = self.cache.num.as_ref().unwrap()[i];
                if x.is_nan() {
                    // Not a number: maybe a date.
                    x = self.cache.date(i).unwrap_or_else(|| {
                        colcache::with_base_cell(&st.base, i, self.field.unwrap(), &mut s.tmp, |v| parse_date(v).unwrap_or(f64::NAN))
                    });
                }
                cmp(c, x, *v)
            }
            Kernel::Folded => {
                let Test::Col(t) = &self.test else { unreachable!() };
                t.test_folded(self.cache.folded.as_ref().unwrap().get(i))
            }
            Kernel::Live => unreachable!(),
        }
    }
}

/// A filter ready to run on a table.
struct Plan<'a> {
    tests: Vec<Prep<'a>>,
    /// Rows with any cell edited here: never searched as raw bytes.
    edited: Option<Bits>,
}

impl<'a> Plan<'a> {
    fn new(st: &State, f: &'a Filter, rows: usize, cancel: &AtomicBool) -> Plan<'a> {
        let mut tests: Vec<Prep> = f.hidden.iter().map(|(c, h)| Prep::new(st, *c, Test::Hidden(h), f.case, rows, cancel)).collect();
        tests.extend(f.picks.iter().map(|p| Prep::new(st, p.col, Test::Pick(p), f.case, rows, cancel)));
        tests.extend(f.cols.iter().map(|(c, t)| Prep::new(st, *c, Test::Col(t), f.case, rows, cancel)));
        // All must pass: the cheapest first, the others see fewer rows.
        tests.sort_by_key(|p| p.cost());
        let edited = f.global.first().and_then(|_| {
            let mut any: Option<Bits> = None;
            for c in 0..st.cols.len() {
                if let Some(e) = colcache::edited_rows(st, c) {
                    any = Some(match any { None => e, Some(mut a) => { a.or(&e); a } });
                }
            }
            any
        });
        Plan { tests, edited }
    }

    /// Keeps the rows of `ids` that pass.
    fn apply(&self, st: &State, f: &Filter, ids: &mut Vec<RowId>, s: &mut Scratch) {
        for t in &self.tests {
            if ids.is_empty() { return; }
            if !t.block(st, ids, s) {
                ids.retain(|&id| t.keep(st, id, f.case, s));
            }
        }
        match f.global.as_slice() {
            [] => {}
            [g] => if !ids.is_empty() { self.global(st, f, g, f.invert, ids, s) },
            terms => {
                // Every term somewhere in the row: one pass per term, each on the rows kept
                // so far. Inverted: the rows that do not hold them all.
                let all = f.invert.then(|| ids.clone());
                for g in terms {
                    if ids.is_empty() { break; }
                    self.global(st, f, g, false, ids, s);
                }
                if let Some(all) = all {
                    // `ids` kept `all`'s order: the rows of `all` not in it, in one walk.
                    let mut hit = ids.iter().peekable();
                    *ids = all.into_iter().filter(|id| if hit.peek() == Some(&id) { hit.next(); false } else { true }).collect();
                }
            }
        }
    }

    fn global(&self, st: &State, f: &Filter, g: &Pat, invert: bool, ids: &mut Vec<RowId>, s: &mut Scratch) {
        let b = &*st.base;
        // A record's raw bytes are its row's cells — searched as such — when no column was
        // removed (columns moved or added only: each field still a column, `fields` of them)
        // and the pattern finds nothing in an empty cell (an added column, a short record's
        // missing fields).
        let fields = match &st.cmap {
            None => Some(usize::MAX),
            Some(m) => {
                let mut mapped: Vec<i32> = m.iter().copied().filter(|&x| x >= 0).collect();
                mapped.sort_unstable();
                let whole = mapped.iter().enumerate().all(|(k, &x)| x == k as i32);
                (whole && !g.test("", f.case, &mut s.buf)).then_some(mapped.len())
            }
        };
        let width = |id: RowId| b.odd.binary_search_by_key(&id, |o| o.0).map_or(b.width, |k| b.odd[k].1 as usize);
        let pristine = |id: RowId| fields.is_some_and(|k| (id as usize) < b.n && (k == usize::MAX || width(id) <= k)) && !self.edited.as_ref().is_some_and(|e| e.has(id));
        let block = match g {
            Pat::Sub { raw_ok, .. } => *raw_ok,
            Pat::Re { pre: Some(p), .. } => p.ascii || b.enc.mem_utf8(),
            Pat::Re { .. } => false,
        };
        let trusted = matches!(g, Pat::Sub { needle, blank, .. } if trust(needle, *blank, b));
        // Spaces alone may sit after a closing quote (`"x"  ,`): in the raw bytes, not in a cell.
        let blank = matches!(g, Pat::Sub { blank: true, .. });
        let mut out = Vec::with_capacity(ids.len());
        let mut k = 0;
        while k < ids.len() {
            let id = ids[k];
            if !pristine(id) {
                st.row(id, &mut s.row);
                let (case, buf) = (f.case, &mut s.buf);
                if s.row.iter().any(|v| g.test(v, case, buf)) != invert { out.push(id); }
                k += 1;
                continue;
            }
            // A run of consecutive untouched records: one block of the file.
            let mut e = k + 1;
            while e < ids.len() && ids[e] == ids[e - 1] + 1 && pristine(ids[e]) { e += 1; }
            if block && e - k >= 16 {
                block_search(b, id as usize, e - k, g, f.case, s);
                for j in 0..e - k {
                    let (r, fl) = (id as usize + j, s.flags[j]);
                    let hit = fl & HIT != 0;
                    let keep = match g {
                        Pat::Sub { .. } => {
                            if fl & UNKNOWN != 0 {
                                record_matches(b, r, g, f.case, s, Some(hit))
                            } else if hit {
                                // In the raw bytes or the fold around an accented letter: in a
                                // cell, when trusted. (ASCII records keep the raw answer whatever,
                                // as they always have.)
                                trusted || (fl & NONASCII == 0 && !blank) || record_matches(b, r, g, f.case, s, Some(hit))
                            } else {
                                // Only a character folding into one of the needle's letters can
                                // make a match the raw bytes lack.
                                fl & SUSPECT != 0 && record_matches(b, r, g, f.case, s, Some(hit))
                            }
                        }
                        Pat::Re { .. } => hit && record_matches(b, r, g, f.case, s, None),
                    };
                    if keep != invert { out.push(r as RowId); }
                }
            } else {
                for &id in &ids[k..e] {
                    if record_matches(b, id as usize, g, f.case, s, None) != invert { out.push(id); }
                }
            }
            k = e;
        }
        *ids = out;
    }
}

/// Whether a substring found in a record (raw, or folded whole) is in one of
/// its cells: when blank it may sit after a closing quote; a delimiter forced
/// on the file (not one of the separators) may be in it.
fn trust(needle: &str, blank: bool, b: &crate::Base) -> bool {
    !blank && !needle.as_bytes().contains(&b.dialect.delim)
}

/// Block flags of a record: its raw bytes hold the needle (a substring: a cell
/// holds it) or one of the regex's literals (it may match); it has non-ASCII
/// characters; one of them folds into a letter of the needle; its bytes are
/// not plain UTF-8 (read it the slow way).
const HIT: u8 = 1;
const NONASCII: u8 = 2;
const SUSPECT: u8 = 4;
const UNKNOWN: u8 = 8;

/// Sets `s.flags` for the records a..a+len, reading their bytes in blocks of
/// about 64 KB: folded to lower case once, searched in one go, their non-ASCII
/// characters looked at once each.
fn block_search(b: &crate::Base, a: usize, len: usize, g: &Pat, case: bool, s: &mut Scratch) {
    s.flags.clear();
    let utf8 = b.enc.mem_utf8();
    let (mask, fold2) = match g {
        Pat::Sub { mask, fold2, .. } => (Some(*mask), Some(&**fold2)),
        Pat::Re { .. } => (None, None),
    };
    let (needle_len, trust) = match g {
        Pat::Sub { needle, blank, .. } => (needle.len(), trust(needle, *blank, b)),
        Pat::Re { .. } => (0, false),
    };
    if let Some(m) = mask.filter(|&m| m != s.fold3_mask || s.fold3.is_empty()) {
        s.fold3 = vec![(0, false); 64];
        s.fold3_mask = m;
    }
    // Other encodings: the non-ASCII records are read the slow way.
    s.flags.resize(len, if mask.is_some() && !utf8 { UNKNOWN } else { 0 });
    let bytes = b.bytes();
    let end = a + len;
    let mut r = a;
    while r < end {
        let bs = b.starts.get(r);
        let mut e = r + 1;
        while e < end && b.starts.get(e + 1) - bs <= 1 << 16 { e += 1; }
        let raw = &bytes[bs..b.starts.get(e)];
        let hay: &[u8] = match g {
            Pat::Sub { ci: None, .. } if !case => {
                s.low.clear();
                s.low.extend(raw.iter().map(u8::to_ascii_lowercase));
                &s.low
            }
            _ => raw,
        };
        let mut pos = 0;
        let mut rec = r;
        loop {
            let found = match g {
                Pat::Sub { ci: Some(ci), .. } if !case => ci.find(&hay[pos..]),
                Pat::Sub { finder, .. } => finder.find(&hay[pos..]),
                Pat::Re { pre: Some(p), .. } => p.ac.find(&hay[pos..]).map(|m| m.start()),
                Pat::Re { .. } => unreachable!(),
            };
            let Some(k) = found else { break };
            // The needle holds no line break: the match lies in the record it starts in.
            let p = bs + pos + k;
            while b.starts.get(rec + 1) <= p { rec += 1; }
            s.flags[rec - a] |= HIT;
            rec += 1;
            if rec >= e { break; }
            pos = b.starts.get(rec) - bs;
        }
        if let (Some(mask), true) = (mask, utf8) {
            // A needle trusted in a cell reads NONASCII nowhere: only the characters folding
            // into one of its letters matter (none when the case counts), the two-byte ones
            // told by `fold2` without decoding them or finding their record.
            let quick = fold2.filter(|_| trust);
            // Checked UTF-8: only the first bytes of characters are looked at (8 at a time).
            let leads = quick.is_some() && b.checked_utf8();
            let mut i = 0;
            let mut rec = r;
            loop {
                let next = if leads { crate::text::find_lead(raw, i) } else { crate::text::find_high(&raw[i..]).map(|k| i + k) };
                let Some(p) = next else { break };
                if let Some(t) = quick {
                    let at = |k: usize| raw.get(p + k).copied().unwrap_or(0);
                    let (c0, c1, c2) = (raw[p], at(1), at(2));
                    if (0xC2..=0xDF).contains(&c0) && c1 & 0xC0 == 0x80 {
                        let u = ((c0 as usize & 0x1F) << 6 | (c1 as usize & 0x3F)) - 0x80;
                        if case || t[u / 64] >> (u % 64) & 1 == 0 {
                            i = p + 2;
                            continue;
                        }
                    } else if (0xE0..=0xEF).contains(&c0) && c1 & 0xC0 == 0x80 && c2 & 0xC0 == 0x80 && !(c0 == 0xE0 && c1 < 0xA0) && !(c0 == 0xED && c1 >= 0xA0) {
                        // Three bytes (€, the narrow no-break space…): the verdict of the last
                        // characters seen, kept, since folding one of these costs more.
                        let u = (c0 as u32 & 0x0F) << 12 | (c1 as u32 & 0x3F) << 6 | (c2 as u32 & 0x3F);
                        let m = &mut s.fold3[u as usize % 64];
                        if m.0 != u {
                            *m = (u, char::from_u32(u).is_some_and(|c| crate::text::folds_into(c, mask)));
                        }
                        if case || !m.1 {
                            i = p + 3;
                            continue;
                        }
                    }
                }
                while b.starts.get(rec + 1) <= bs + p { rec += 1; }
                let n = match raw[p] { 0xC2..=0xDF => 2, 0xE0..=0xEF => 3, 0xF0..=0xF4 => 4, _ => 0 };
                match std::str::from_utf8(&raw[p..(p + n).min(raw.len())]).ok().filter(|_| n > 0).and_then(|c| c.chars().next()) {
                    Some(c) => {
                        s.flags[rec - a] |= NONASCII;
                        if !case && s.flags[rec - a] & HIT == 0 && crate::text::folds_into(c, mask) {
                            // The needle may be in the fold around this character: its L
                            // characters lie within 4·L bytes of it, in this record.
                            let (rs, re) = (b.starts.get(rec) - bs, b.starts.get(rec + 1) - bs);
                            let l = 4 * needle_len;
                            let mut lo = p.saturating_sub(l).max(rs);
                            while lo > rs && raw[lo] & 0xC0 == 0x80 { lo -= 1; }
                            let mut hi = (p + n + l).min(re);
                            while hi < re && raw[hi] & 0xC0 == 0x80 { hi += 1; }
                            match std::str::from_utf8(&raw[lo..hi]) {
                                Ok(w) if trust => {
                                    s.buf.clear();
                                    crate::text::fold_into(w, &mut s.buf);
                                    if let Pat::Sub { finder, .. } = g {
                                        if finder.find(s.buf.as_bytes()).is_some() { s.flags[rec - a] |= HIT; }
                                    }
                                }
                                _ => s.flags[rec - a] |= SUSPECT,
                            }
                        }
                        i = p + n;
                    }
                    None => {
                        s.flags[rec - a] |= UNKNOWN;
                        i = p + 1;
                    }
                }
            }
        }
        r = e;
    }
}

/// Whether some cell of an untouched record holds the pattern. `raw_hit`:
/// whether its raw bytes hold the needle, if already known.
fn record_matches(b: &crate::Base, id: usize, g: &Pat, case: bool, s: &mut Scratch, raw_hit: Option<bool>) -> bool {
    let rec = b.record(id);
    if let Pat::Sub { finder, raw_ok: true, needle, blank, mask, ci, .. } = g {
        if needle.is_empty() { return true; }
        let hit = raw_hit.unwrap_or_else(|| {
            if case { return finder.find(rec).is_some(); }
            if let Some(ci) = ci { return ci.find(rec).is_some(); }
            s.low.clear();
            s.low.extend(rec.iter().map(u8::to_ascii_lowercase));
            finder.find(&s.low).is_some()
        });
        let ascii = rec.is_ascii();
        // An ASCII needle that cannot span two fields, found in the raw bytes: it
        // is in one cell (unless it is blank: it could sit after a closing quote).
        if hit && ((ascii && !*blank) || trust(needle, *blank, b)) { return true; }
        if !hit {
            if ascii { return false; }
            // Not in the raw bytes, but a cell's accented letter may fold into it —
            // only if one folds into one of its letters.
            if b.enc.mem_utf8() {
                if let Ok(t) = std::str::from_utf8(rec) {
                    if case || !crate::text::folds_into_any(t, *mask) { return false; }
                }
            }
        }
    }
    let enc = b.enc;
    if b.dialect.lines {
        return g.test(&b.text(rec), case, &mut s.buf);
    }
    // Ruled out on the whole record at once, when it can be.
    match g {
        Pat::Sub { finder, rec_ok: true, needle, blank, .. } if !needle.is_empty() && !needle.as_bytes().contains(&b.dialect.delim) => {
            let d = b.text(rec);
            let found = if case {
                finder.find(d.as_bytes()).is_some()
            } else {
                s.buf.clear();
                fold_into(&d, &mut s.buf);
                finder.find(s.buf.as_bytes()).is_some()
            };
            // Without separators the needle is found in the whole record only
            // within one cell (each character folds on its own; separators fold
            // to themselves) — unless blank: it may sit after a closing quote.
            if !found || !*blank { return found; }
        }
        Pat::Re { re, pre, local } => {
            if let Some(p) = pre.as_ref().filter(|p| p.ascii || enc.mem_utf8()) {
                if !p.ac.is_match(rec) { return false; }
            }
            // Without quotes the record is its cells end to end: one search for all.
            if *local && memchr::memchr(b'"', rec).is_none() && !re.maybe(&b.text(rec)) {
                return false;
            }
        }
        _ => {}
    }
    let mut hit = false;
    let buf = &mut s.buf;
    crate::scan::visit_fields(rec, b.dialect.delim, &mut s.tmp, |_, f| {
        hit = g.test(&b.text(f), case, buf);
        !hit
    });
    hit
}

/// Rows per chunk of work.
const CHUNK: usize = 1 << 13;

fn run(st: &State, f: &Filter, len: usize, id_at: impl Fn(usize) -> RowId + Sync, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    let plan = Plan::new(st, f, len, cancel);
    let parts: Vec<Vec<RowId>> = (0..len.div_ceil(CHUNK))
        .into_par_iter()
        .map_init(Scratch::default, |s, c| {
            if cancel.load(Ordering::Relaxed) { return Vec::new(); }
            let mut ids: Vec<RowId> = (c * CHUNK..((c + 1) * CHUNK).min(len)).map(&id_at).collect();
            plan.apply(st, f, &mut ids, s);
            ids
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(concat(parts)) }
}

/// The parts end to end, copied in parallel.
fn concat(parts: Vec<Vec<RowId>>) -> Vec<RowId> {
    if parts.len() == 1 {
        return parts.into_iter().next().unwrap();
    }
    let total = parts.iter().map(Vec::len).sum();
    let mut out: Vec<RowId> = Vec::with_capacity(total);
    let mut rest = &mut out.spare_capacity_mut()[..total];
    let mut slices = Vec::with_capacity(parts.len());
    for p in &parts {
        let (a, b) = rest.split_at_mut(p.len());
        slices.push(a);
        rest = b;
    }
    slices.into_par_iter().zip(&parts).for_each(|(d, s)| {
        for (d, &s) in d.iter_mut().zip(s) { d.write(s); }
    });
    // SAFETY: the slices cover 0..total, each written above.
    unsafe { out.set_len(total) };
    out
}

/// The data rows (file order) that pass the filter; None if cancelled.
pub fn filter(st: &State, f: &Filter, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    let n = st.len();
    if f.is_empty() {
        return Some((0..n).into_par_iter().with_min_len(1 << 14).map(|p| st.data_id(p)).collect());
    }
    run(st, f, n, |p| st.data_id(p), cancel)
}

/// Of these rows (in this order), those that pass the filter; None if cancelled.
pub fn filter_ids(st: &State, f: &Filter, ids: &[RowId], cancel: &AtomicBool) -> Option<Vec<RowId>> {
    if f.is_empty() {
        return Some(ids.to_vec());
    }
    run(st, f, ids.len(), |k| ids[k], cancel)
}

/// What a column holds, from a sample of its non-empty cells: numbers or dates
/// when 95 % of them read as such.
pub fn detect_kinds(st: &State) -> Vec<Kind> {
    detect_kinds_of(st, &(0..st.ncols).collect::<Vec<_>>())
}

/// `detect_kinds` of the columns `cols` only (one inserted, the others moved).
pub fn detect_kinds_of(st: &State, cols: &[usize]) -> Vec<Kind> {
    let n = st.len();
    let sample: Vec<usize> = if n <= 4000 { (0..n).collect() } else { (0..4000).map(|k| k * n / 4000).collect() };
    cols.par_iter()
        .map(|&c| {
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

/// A number as a u64 in the same order (the bits of a float, the sign turned
/// around); NaN (empty, not a number) last whichever the direction.
#[inline]
fn num_word(x: f64, desc: bool) -> u64 {
    if x.is_nan() { return u64::MAX; }
    let x = if x == 0.0 { 0.0 } else { x }; // -0 == 0: a tie, as compared before
    let b = x.to_bits();
    let m = if b >> 63 == 1 { !b } else { b | 1 << 63 };
    if desc { !m } else { m }
}

/// A text's rank as a sort word: empty (rank `nranks`) last whichever the direction.
#[inline]
fn rank_word(r: u32, nranks: u32, desc: bool) -> u64 {
    if r >= nranks { nranks as u64 } else if desc { (nranks - 1 - r) as u64 } else { r as u64 }
}

thread_local! {
    /// A cell's bytes and its fold, while a sort key is made of it.
    static FOLD: std::cell::RefCell<(Vec<u8>, String)> = const { std::cell::RefCell::new((Vec::new(), String::new())) };
}

/// One sort key's word for each row of `ids`; None if cancelled.
fn key_words(st: &State, ids: &[RowId], key: SortKey, kind: Kind, cancel: &AtomicBool) -> Option<Vec<u64>> {
    let (col, desc) = (key.col, key.desc);
    let n = st.base.n;
    let field = colcache::field_of(st, col);
    let edited = colcache::edited_rows(st, col);
    let live = |id: RowId| field.is_none() || id as usize >= n || edited.as_ref().is_some_and(|e| e.has(id));
    let need = need_of(kind);
    let mut cache = if field.is_some() { colcache::get(st, col) } else { Col::default() };
    if field.is_some() && !cache.has(need) && !st.base.partial {
        // Sorting most of the file: the cache costs what the keys would, and stays.
        if ids.len() * 2 >= st.len() {
            cache = colcache::ensure(st, col, need, cancel).unwrap_or_default();
        } else {
            colcache::warm(st, col, need);
        }
    }
    if cancel.load(Ordering::Relaxed) { return None; }
    let words = match kind {
        // A dictionary: the values' ranks, small words (a counting sort).
        Kind::Number | Kind::Date if cache.dict.is_some() && !ids.par_iter().any(|&id| live(id)) => {
            let d = cache.dict.as_ref().unwrap();
            let (rank, nr) = if kind == Kind::Date { &d.date_rank } else { &d.num_rank };
            ids.par_iter().with_min_len(4096).map(|&id| rank_word(rank[d.codes[id as usize] as usize], *nr, desc)).collect()
        }
        Kind::Number | Kind::Date => {
            let date = kind == Kind::Date;
            let parse = if date { parse_date } else { parse_number };
            ids.par_iter()
                .with_min_len(4096)
                .map_init(Vec::new, |tmp, &id| {
                    let cached = if live(id) { None } else if date { cache.date(id as usize) } else { cache.num(id as usize) };
                    let x = cached.unwrap_or_else(|| st.with_cell(id, col, tmp, |v| parse(v.trim()).unwrap_or(f64::NAN)));
                    num_word(x, desc)
                })
                .collect()
        }
        Kind::Text => {
            if cache.has(Need::Rank) && !ids.par_iter().any(|&id| live(id)) {
                let nr = cache.nranks().unwrap();
                ids.par_iter().with_min_len(4096).map(|&id| rank_word(cache.rank(id as usize).unwrap(), nr, desc)).collect()
            } else {
                // Each row's sort key (`text::sort_key_into`) from its folded text.
                let keys = crate::radix::Keys::build(ids.len(), |p, out| {
                    let id = ids[p];
                    match if live(id) { None } else { cache.folded(id as usize) } {
                        Some(f) => crate::text::sort_key_into(f.trim().as_bytes(), out),
                        None => FOLD.with_borrow_mut(|(tmp, buf)| {
                            buf.clear();
                            st.with_cell(id, col, tmp, |v| fold_into(v.trim(), buf));
                            crate::text::sort_key_into(buf.as_bytes(), out);
                        }),
                    }
                });
                if cancel.load(Ordering::Relaxed) { return None; }
                let (_, rank, nr) = crate::radix::text_order_ties(ids.len(), |p| keys.get(p), |p| !live(ids[p]) && cache.plain(ids[p] as usize), |p, out| {
                    FOLD.with_borrow_mut(|(tmp, _)| st.with_cell(ids[p], col, tmp, |v| crate::text::tie_key_into(v.trim(), out)))
                });
                rank.par_iter().with_min_len(4096).map(|&r| rank_word(r, nr, desc)).collect()
            }
        }
    };
    Some(words)
}

/// The stable order of positions by several keys' words: words packed into
/// one u64 while their widths fit, radix sorted from the last pack to the first.
fn order(words: Vec<Vec<u64>>, cancel: &AtomicBool) -> Option<Vec<u32>> {
    let len = words[0].len();
    // Each key relative to its least word, with its width; a key all equal orders nothing.
    let keys: Vec<(Vec<u64>, u64, u32)> = words
        .into_iter()
        .filter_map(|w| {
            let (lo, hi) = w
                .par_iter()
                .with_min_len(1 << 16)
                .fold(|| (u64::MAX, 0), |(a, b), &x| (a.min(x), b.max(x)))
                .reduce(|| (u64::MAX, 0), |x, y| (x.0.min(y.0), x.1.max(y.1)));
            (lo < hi).then(|| (w, lo, 64 - (hi - lo).leading_zeros()))
        })
        .collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut width = 0;
    for (k, key) in keys.iter().enumerate() {
        match groups.last_mut() {
            Some(g) if width + key.2 <= 64 => { g.push(k); width += key.2; }
            _ => { groups.push(vec![k]); width = key.2; }
        }
    }
    let mut perm: Option<Vec<u32>> = None;
    for g in groups.iter().rev() {
        if cancel.load(Ordering::Relaxed) { return None; }
        let packed: Vec<u64> = (0..len)
            .into_par_iter()
            .with_min_len(4096)
            .map(|j| {
                let i = perm.as_ref().map_or(j, |p| p[j] as usize);
                g.iter().fold(0u64, |acc, &k| {
                    let (w, lo, bits) = &keys[k];
                    acc.checked_shl(*bits).unwrap_or(0) | (w[i] - lo)
                })
            })
            .collect();
        let p = crate::radix::sort_perm(&packed);
        perm = Some(match perm {
            None => p,
            Some(prev) => p.par_iter().with_min_len(4096).map(|&j| prev[j as usize]).collect(),
        });
    }
    Some(perm.unwrap_or_else(|| (0..len as u32).collect()))
}

/// Sorts `ids` on the keys, in parallel (stable: ties keep their order).
/// Empty and unreadable values go last, ascending or descending.
pub fn sort(st: &State, ids: &mut Vec<RowId>, keys: &[SortKey], kinds: &[Kind], cancel: &AtomicBool) -> bool {
    if keys.is_empty() || ids.len() < 2 {
        return true;
    }
    let mut words = Vec::with_capacity(keys.len());
    for k in keys {
        let kind = kinds.get(k.col).copied().unwrap_or(Kind::Text);
        let Some(w) = key_words(st, ids, *k, kind, cancel) else { return false };
        words.push(w);
    }
    let Some(perm) = order(words, cancel) else { return false };
    if cancel.load(Ordering::Relaxed) { return false; }
    let sorted: Vec<RowId> = perm.par_iter().with_min_len(4096).map(|&p| ids[p as usize]).collect();
    *ids = sorted;
    true
}

#[cfg(test)]
#[path = "view_old.rs"]
mod old;

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
