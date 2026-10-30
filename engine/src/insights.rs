//! "What I noticed": a read of the rows (an even sample past IN_SAMPLE) and
//! detectors, each turning what it finds into a card:
//! - the key: a column filled and different on every row, or one that nearly
//!   is (the values that come back are the finding);
//! - a column that decides another (postal code → city) but not on a few
//!   rows: those rows disagree with the rest of their group;
//! - values written otherwise than the rest of their column (7501 among
//!   75011, 2024-03-15 among 15/03/2024, DUPONT among Dupont, 12.50 among 10,50);
//! - numbers or dates far from all the others, a few negatives among positives;
//! - rows that are exact copies of an earlier one;
//! - hidden arithmetic: TTC = HT × 1,2, TTC = HT + TVA — on every row, or on
//!   all but a few;
//! - empty or constant columns, stray spaces and invisible characters,
//!   irregular rows, garbled accents.
//!
//! Same thresholds, order and texts as the JS version. A card's rows are not
//! listed with it: `marks` counts them again on every row (the sample only
//! found the rule), with the cell at fault and what was expected there.

use crate::kinds::{self, cell_type, code_like, fmt_int, fmt_num, id_like, js_trim, Kind, FxMap, FxSet};
use crate::table::{RowId, State};
use rayon::prelude::*;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const IN_SAMPLE: usize = 100_000; // rows read at most for the detection (evenly spaced past that)
const IN_CELLS: usize = 2_000_000; // …and cells
const IN_DISTINCT: usize = 50_000; // distinct values counted per column
const IN_FD_ROWS: usize = 30_000; // rows the dependencies are looked for in
const IN_FD_COLS: usize = 30; // columns tried on each side
const IN_REL_ROWS: usize = 20_000; // rows the arithmetic between columns is looked for in
const IN_REL_COLS: usize = 16; // numeric columns tried (sums: 12)
const IN_SHAPE_MIN: f64 = 0.85;
const IN_SHAPE_ODD: f64 = 0.05;
const IN_SHAPE_CASE: f64 = 0.12;
const IN_SHAPE_ROWS: usize = 20_000;
/// Rows per slice of a pass over the whole file (cancellation is checked between slices).
const SLICE: usize = 1 << 15;

// ---- Cards -------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    /// A problem in the data: shown first.
    Warn,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon { Key, Dep, Shape, Far, Dup, Rel, Empty, Space, Warn }

/// What a card's buttons open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Show the irregular rows alone.
    ShowIrregular,
    CleanUp,
    RemoveDuplicates,
}

/// A piece of a card's text: a column is a chip that goes to the column, a value is shown as one.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    Col(usize),
    /// Already as shown: cut to 40 characters, "(empty)" for nothing.
    Val(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rich(pub Vec<Part>);

impl Rich {
    fn t(mut self, s: impl AsRef<str>) -> Rich {
        let s = s.as_ref();
        if s.is_empty() { return self; }
        if let Some(Part::Text(t)) = self.0.last_mut() { t.push_str(s); } else { self.0.push(Part::Text(s.into())); }
        self
    }
    fn c(mut self, c: usize) -> Rich {
        self.0.push(Part::Col(c));
        self
    }
    fn v(mut self, v: &str) -> Rich {
        self.0.push(Part::Val(kinds::cut(if v.is_empty() { "(empty)" } else { v }, 40)));
        self
    }
    fn join(mut self, items: impl IntoIterator<Item = Rich>, sep: &str) -> Rich {
        for (k, r) in items.into_iter().enumerate() {
            if k > 0 { self = self.t(sep); }
            for p in r.0 {
                self = match p { Part::Text(s) => self.t(s), p => { self.0.push(p); self } };
            }
        }
        self
    }
    /// As plain text, columns by their names.
    pub fn plain(&self, headers: &[String]) -> String {
        self.0.iter().map(|p| match p {
            Part::Text(s) | Part::Val(s) => s.as_str(),
            Part::Col(c) => headers.get(*c).map_or("", |s| s.as_str()),
        }).collect()
    }
}

fn r() -> Rich {
    Rich::default()
}

#[derive(Clone, Debug)]
pub struct Insight {
    pub sev: Sev,
    pub icon: Icon,
    /// Weight among the cards of a severity.
    pub score: u32,
    pub title: Rich,
    pub sub: Rich,
    /// How to find its rows on the whole file: `marks`.
    pub show: Option<Show>,
    pub show_label: &'static str,
    pub acts: Vec<Act>,
}

impl Insight {
    /// A card on values far from all the others.
    pub fn is_far(&self) -> bool {
        matches!(self.show, Some(Show { what: What::Far { .. }, .. }))
    }
}

/// The rows of a card, as `marks` finds them again on every row.
#[derive(Clone, Debug)]
pub struct Show {
    /// "sharing a id", "written otherwise in cp"…: what the marked rows are.
    pub label: String,
    /// What the tooltip of a cell at fault says before the expected value.
    pub tip: String,
    what: What,
}

#[derive(Clone, Debug)]
enum What {
    Repeats { col: usize },
    Decimal { col: usize, mark: char, usual: String },
    Shape { col: usize, runs: bool, dom: String, digits: bool, usual: String },
    Far { col: usize, date: bool, lo: f64, hi: f64, range: String },
    Negative { col: usize },
    Copies,
    Rule { rel: Rel, target: usize },
    Dep { a: usize, b: usize },
}

#[derive(Clone, Debug)]
enum Rel {
    Mul { a: usize, b: usize, k: f64 },
    Add { a: usize, b: usize, c: usize },
}

/// Rows to mark, in file order; cells at fault with what was expected there.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    pub label: String,
    pub tip: String,
    pub rows: Vec<RowId>,
    pub cells: Vec<(RowId, usize, String)>,
}

#[derive(Clone, Debug)]
pub struct Report {
    /// Data problems first, then by weight.
    pub items: Vec<Insight>,
    /// Rows read, and in the file.
    pub n: usize,
    pub total: usize,
    pub sampled: bool,
}

impl Report {
    /// The dialog's summary line.
    pub fn summary(&self) -> String {
        let read = if self.sampled { format!("{} rows of {} read", fmt_int(self.n), fmt_int(self.total)) } else { format!("{} read", pl(self.total, "row", "rows")) };
        let n = self.items.len();
        let warn = self.items.iter().filter(|x| x.sev == Sev::Warn).count();
        if n == 0 { return format!("{read} · nothing stands out"); }
        format!("{read} · {} noticed{}", pl(n, "thing", "things"), if warn > 0 { format!(", {} to look at", fmt_int(warn)) } else { String::new() })
    }

    /// What the dialog says when there is no card.
    pub fn nothing(name: &str) -> String {
        format!("Nothing stands out in {name}: no key broken, no value written otherwise, no outlier, no copy.")
    }
}

fn pl(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", fmt_int(n), if n == 1 { one } else { many })
}

// ---- The rows read -----------------------------------------------------

/// One column's values over the rows read, trimmed, end to end.
#[derive(Default)]
struct Vals {
    arena: String,
    ends: Vec<u32>,
}

impl Vals {
    #[inline]
    fn get(&self, i: usize) -> &str {
        let s = if i == 0 { 0 } else { self.ends[i - 1] as usize };
        &self.arena[s..self.ends[i] as usize]
    }
    fn len(&self) -> usize {
        self.ends.len()
    }
    fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }
    fn push(&mut self, s: &str) {
        self.arena.push_str(s);
        self.ends.push(self.arena.len() as u32);
    }
}

/// Values counted in the order they first come.
struct Counter<K> {
    idx: FxMap<K, usize>,
    items: Vec<(K, u32)>,
}

impl<K: Hash + Eq + Clone> Counter<K> {
    fn new() -> Self {
        Counter { idx: FxMap::default(), items: Vec::new() }
    }
    fn add(&mut self, k: K) {
        match self.idx.get(&k) {
            Some(&i) => self.items[i].1 += 1,
            None => {
                self.idx.insert(k.clone(), self.items.len());
                self.items.push((k, 1));
            }
        }
    }
    fn get(&self, k: &K) -> u32 {
        self.idx.get(k).map_or(0, |&i| self.items[i].1)
    }
    /// The most common, the first of equals.
    fn top(&self) -> Option<&(K, u32)> {
        self.items.iter().fold(None, |b: Option<&(K, u32)>, x| if b.is_none_or(|b| x.1 > b.1) { Some(x) } else { b })
    }
}

/// A column over the rows read, and its counts.
struct Col<'a> {
    c: usize,
    v: &'a Vals,
    /// Distinct values (IN_DISTINCT at most) and how often, in the order met.
    counts: Counter<&'a str>,
    filled: usize,
    kind: Kind,
    /// Past IN_DISTINCT distinct values: the counts stopped growing.
    over: bool,
    /// Cells with stray spaces or invisible characters.
    raw: usize,
}

impl Col<'_> {
    fn distinct(&self) -> usize {
        self.counts.items.len()
    }
}

/// Spaces doubled, no-break and narrow ones, the invisible format characters met in pasted text.
fn stray(s: &str) -> bool {
    let mut prev = false;
    for c in s.chars() {
        let sp = kinds::is_js_space(c);
        if (sp && prev) || matches!(c, '\u{a0}' | '\u{202f}' | '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}') {
            return true;
        }
        prev = sp;
    }
    false
}

struct Sample {
    n: usize,
    total: usize,
    sampled: bool,
    vals: Vec<Vals>,
    raw: Vec<usize>,
}

/// Every row, or an even sample: each column's values trimmed.
fn read_sample(st: &State, ncols: usize, cancel: &AtomicBool) -> Option<Sample> {
    let total = st.len();
    let max = 2000.max(IN_SAMPLE.min(IN_CELLS / ncols.max(1)));
    let sampled = total > max;
    let step = if sampled { total as f64 / max as f64 } else { 1.0 };
    let ids: Vec<RowId> = if sampled { (0..max).map(|i| st.data_id((i as f64 * step).floor() as usize)).collect() } else { (0..total).map(|p| st.data_id(p)).collect() };
    let parts: Vec<(Vec<Vals>, Vec<usize>)> = ids
        .par_chunks(2048)
        .map(|chunk| {
            let mut cols: Vec<Vals> = (0..ncols).map(|_| Vals::default()).collect();
            let mut raw = vec![0usize; ncols];
            for &id in chunk {
                if cancel.load(Ordering::Relaxed) { break; }
                let mut upto = 0;
                st.visit(id, |c, v| {
                    if c >= ncols { return; }
                    let s = js_trim(v);
                    cols[c].push(s);
                    if !s.is_empty() && (s.len() != v.len() || stray(s)) { raw[c] += 1; }
                    upto = c + 1;
                });
                for col in &mut cols[upto..] { col.push(""); }
            }
            (cols, raw)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut vals: Vec<Vals> = (0..ncols).map(|_| Vals::default()).collect();
    let mut raw = vec![0usize; ncols];
    for (cols, rw) in parts {
        for (c, v) in cols.into_iter().enumerate() {
            let off = vals[c].arena.len() as u32;
            vals[c].arena.push_str(&v.arena);
            vals[c].ends.extend(v.ends.iter().map(|e| e + off));
            raw[c] += rw[c];
        }
    }
    Some(Sample { n: ids.len(), total, sampled, vals, raw })
}

fn col_stats<'a>(c: usize, v: &'a Vals, raw: usize) -> Col<'a> {
    let mut counts = Counter::new();
    let (mut filled, mut num, mut date, mut over) = (0, 0, 0, false);
    for s in v.iter() {
        if s.is_empty() { continue; }
        filled += 1;
        match cell_type(s) {
            Kind::Num => num += 1,
            Kind::Date => date += 1,
            _ => {}
        }
        if counts.items.len() < IN_DISTINCT || counts.idx.contains_key(&s) { counts.add(s); } else { over = true; }
    }
    let f = filled as f64;
    let kind = if filled == 0 { Kind::Empty } else if num as f64 / f >= 0.9 { Kind::Num } else if date as f64 / f >= 0.9 { Kind::Date } else { Kind::Text };
    Col { c, v, counts, filled, kind, over, raw }
}

// ---- Passes over every row ---------------------------------------------

/// Runs `f` on every data row (position, id) in slices, in parallel; the
/// slices' results in file order. None if cancelled.
fn every<T: Send>(st: &State, cancel: &AtomicBool, init: impl Fn() -> T + Sync, f: impl Fn(&mut T, usize, RowId) + Sync) -> Option<Vec<T>> {
    let n = st.len();
    let slices: Vec<usize> = (0..n.div_ceil(SLICE)).collect();
    let out: Option<Vec<T>> = slices
        .par_iter()
        .map(|&k| {
            if cancel.load(Ordering::Relaxed) { return None; }
            let mut t = init();
            for p in k * SLICE..((k + 1) * SLICE).min(n) {
                f(&mut t, p, st.data_id(p));
            }
            Some(t)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { out }
}

/// Every row whose cell in `col` (trimmed) gives Some(expected).
fn every_cell(st: &State, col: usize, cancel: &AtomicBool, test: impl Fn(&str) -> Option<String> + Sync) -> Option<Vec<(RowId, String)>> {
    let parts = every(st, cancel, || (Vec::new(), Vec::new()), |(out, tmp): &mut (Vec<(RowId, String)>, Vec<u8>), _, id| {
        if let Some(w) = st.with_cell(id, col, tmp, |v| test(js_trim(v))) { out.push((id, w)); }
    })?;
    Some(parts.into_iter().flat_map(|p| p.0).collect())
}

fn hash_of(b: &[u8], tag: u8) -> u64 {
    let mut h = rustc_hash::FxHasher::default();
    h.write(b);
    h.write_u8(tag);
    h.finish()
}

/// Rows that are exact copies of an earlier one, in file order. A row still
/// its record is compared by its bytes; an edited or added row by its cells
/// (joined by U+001F). Blank rows are Clean up's business, not copies.
fn copies(st: &State, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    const BLANK: &[u8] = b" \t\r\n\";,|";
    let key = |id: RowId| -> Option<(u64, Option<String>)> {
        if st.pristine(id) {
            let rec = st.base.record(id as usize);
            if rec.iter().all(|b| BLANK.contains(b)) { return None; }
            return Some((hash_of(rec, 0), None));
        }
        let mut k = String::new();
        st.visit(id, |c, v| {
            if c > 0 { k.push('\u{1f}'); }
            k.push_str(v);
        });
        if k.chars().all(|c| c == '\u{1f}' || kinds::is_js_space(c)) { return None; }
        Some((hash_of(k.as_bytes(), 1), Some(k)))
    };
    let parts = every(st, cancel, Vec::new, |out: &mut Vec<(u64, u32)>, p, id| {
        if let Some((h, _)) = key(id) { out.push((h, p as u32)); }
    })?;
    let mut hs: Vec<(u64, u32)> = parts.into_iter().flatten().collect();
    hs.par_sort_unstable();
    if cancel.load(Ordering::Relaxed) { return None; }
    // Runs of one hash: each member against the distinct ones before it.
    let runs: Vec<&[(u64, u32)]> = hs.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1).collect();
    let mut out: Vec<u32> = runs
        .par_iter()
        .flat_map_iter(|g| {
            let mut firsts: Vec<(u32, Option<String>)> = Vec::new();
            let mut cp = Vec::new();
            for &(_, p) in g.iter() {
                let id = st.data_id(p as usize);
                let k = key(id).unwrap().1;
                let same = |&(q, ref kq): &(u32, Option<String>)| match (&k, kq) {
                    (None, None) => st.base.record(st.data_id(q as usize) as usize) == st.base.record(id as usize),
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
                if firsts.iter().any(same) { cp.push(p); } else { firsts.push((p, k)); }
            }
            cp
        })
        .collect();
    out.par_sort_unstable();
    Some(out.into_iter().map(|p| st.data_id(p as usize)).collect())
}

/// A value of a column that comes back, and the positions of its rows.
struct Rep {
    value: String,
    pos: Vec<u32>,
}

/// Every trimmed value of `col` met on more than one row (in the order they
/// first come), and the count of empty cells. Hashes are sorted rather than
/// kept in a map: only the values of equal hashes are read again.
fn repeats(st: &State, col: usize, cancel: &AtomicBool) -> Option<(Vec<Rep>, usize)> {
    let parts = every(st, cancel, || (Vec::new(), 0usize, Vec::new()), |(out, empty, tmp): &mut (Vec<(u64, u32)>, usize, Vec<u8>), p, id| {
        st.with_cell(id, col, tmp, |v| {
            let s = js_trim(v);
            if s.is_empty() { *empty += 1 } else { out.push((hash_of(s.as_bytes(), 0), p as u32)) }
        });
    })?;
    let empty = parts.iter().map(|p| p.1).sum();
    let mut hs: Vec<(u64, u32)> = parts.into_iter().flat_map(|p| p.0).collect();
    hs.par_sort_unstable();
    if cancel.load(Ordering::Relaxed) { return None; }
    let runs: Vec<&[(u64, u32)]> = hs.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1).collect();
    let mut reps: Vec<Rep> = runs
        .par_iter()
        .flat_map_iter(|g| {
            let mut tmp = Vec::new();
            let mut by: Vec<Rep> = Vec::new();
            for &(_, p) in g.iter() {
                let v = st.with_cell(st.data_id(p as usize), col, &mut tmp, |v| js_trim(v).to_string());
                match by.iter_mut().find(|r| r.value == v) {
                    Some(r) => r.pos.push(p),
                    None => by.push(Rep { value: v, pos: vec![p] }),
                }
            }
            by.into_iter().filter(|r| r.pos.len() > 1)
        })
        .collect();
    reps.sort_by_key(|r| r.pos[0]);
    Some((reps, empty))
}

// ---- The detectors -----------------------------------------------------

struct Ctx<'a> {
    st: &'a State,
    h: Vec<String>,
    n: usize,
    total: usize,
    sampled: bool,
    cols: Vec<Col<'a>>,
    items: Vec<Insight>,
    cancel: &'a AtomicBool,
}

impl Ctx<'_> {
    /// A count found on the rows read, scaled to the file: ≈ in front.
    fn n_txt(&self, x: f64) -> String {
        if self.sampled { format!("≈ {}", fmt_int(((x * self.total as f64 / self.n as f64) + 0.5).floor().max(1.0) as usize)) } else { fmt_num(x) }
    }
    fn ns(&self, x: f64, one: &str, many: &str) -> String {
        format!("{} {}", self.n_txt(x), if (x + 0.5).floor() == 1.0 && !self.sampled { one } else { many })
    }
    fn add(&mut self, sev: Sev, icon: Icon, score: u32, title: Rich, sub: Rich) -> &mut Insight {
        self.items.push(card(sev, icon, score, title, sub));
        self.items.last_mut().unwrap()
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn card(sev: Sev, icon: Icon, score: u32, title: Rich, sub: Rich) -> Insight {
    Insight { sev, icon, score, title, sub, show: None, show_label: "Show the rows", acts: Vec::new() }
}

fn show(label: String, tip: &str, what: What) -> Option<Show> {
    Some(Show { label, tip: tip.into(), what })
}

/// Irregular rows, quote errors, garbled accents: the file's own state, counted on every row.
fn in_structure(x: &mut Ctx) -> Option<()> {
    let st = x.st;
    let w = kinds::expected_width(st);
    let irr: usize = every(st, x.cancel, || (0usize, Vec::new()), |(n, row): &mut (usize, Vec<String>), _, id| {
        if kinds::field_count(st, id, row) != w { *n += 1 }
    })?
    .iter()
    .map(|p| p.0)
    .sum();
    let q = st.base.qerr as usize;
    if irr > 0 || q > 0 {
        let title = if irr > 0 { format!("{} {} more or fewer fields than the header", pl(irr, "row", "rows"), if irr == 1 { "has" } else { "have" }) } else { format!("{} in the file {} not where a field starts or ends", pl(q, "quote", "quotes"), if q == 1 { "is" } else { "are" }) };
        let sub = format!("{}A field cut by an unquoted delimiter or line break shifts every value after it. The row card's Why? tells each one's story.", if irr > 0 && q > 0 { format!("and {}. ", pl(q, "quote error", "quote errors")) } else { String::new() });
        x.add(Sev::Warn, Icon::Warn, 90, r().t(title), r().t(sub)).acts = if irr > 0 { vec![Act::ShowIrregular] } else { vec![] };
    }
    if kinds::file_mojibake(st) {
        x.add(Sev::Warn, Icon::Warn, 85, r().t("Accents are garbled: ").v("Ã©").t(" where ").v("é").t(" was meant"),
            r().t("The text was read as Windows-1252 somewhere upstream though it was UTF-8. Clean up repairs it.")).acts = vec![Act::CleanUp];
    }
    let spaced: Vec<usize> = x.cols.iter().filter(|s| s.raw > 0).map(|s| s.c).collect();
    if !spaced.is_empty() {
        let n: usize = x.cols.iter().map(|s| s.raw).sum();
        let mut sub = r().t("In ").join(spaced.iter().take(4).map(|&c| r().c(c)), ", ");
        if spaced.len() > 4 { sub = sub.t(format!(" and {}", pl(spaced.len() - 4, "other column", "other columns"))); }
        let sub = sub.t(": spaces at the ends or doubled, no-break spaces, zero-width characters. Two values that look the same may then not be.");
        x.add(Sev::Info, Icon::Space, 20, r().t(format!("{} cells hold stray spaces or invisible characters", x.n_txt(n as f64))), sub).acts = vec![Act::CleanUp];
    }
    Some(())
}

/// Empty and constant columns (checked on every row when the rows read are a sample).
fn in_columns(x: &mut Ctx) -> Option<()> {
    let mut empty: Vec<usize> = x.cols.iter().filter(|s| s.filled == 0).map(|s| s.c).collect();
    let mut consts: Vec<(usize, String)> = x.cols.iter()
        .filter(|s| s.filled == x.n && s.distinct() == 1 && !s.over)
        .map(|s| (s.c, s.counts.items[0].0.to_string()))
        .collect();
    if x.sampled && (!empty.is_empty() || !consts.is_empty()) {
        // One pass for all: a column fails at its first cell that is not as on the rows read.
        let checks: Vec<(usize, String)> = empty.iter().map(|&c| (c, String::new())).chain(consts.iter().cloned()).collect();
        let st = x.st;
        let fails = every(st, x.cancel, || (vec![false; checks.len()], Vec::new()), |(bad, tmp): &mut (Vec<bool>, Vec<u8>), _, id| {
            for (k, (c, want)) in checks.iter().enumerate() {
                if !bad[k] && st.with_cell(id, *c, tmp, |v| js_trim(v) != want) { bad[k] = true; }
            }
        })?;
        let bad = |k: usize| fails.iter().any(|f| f.0[k]);
        let ne = empty.len();
        let mut k = 0;
        empty.retain(|_| { k += 1; !bad(k - 1) });
        let mut k = ne;
        consts.retain(|_| { k += 1; !bad(k - 1) });
    }
    if !empty.is_empty() {
        let (title, sub) = if empty.len() == 1 {
            (r().c(empty[0]).t(" is empty on every row"), r().t("Clean up can delete the empty columns."))
        } else {
            let s = r().join(empty.iter().take(8).map(|&c| r().c(c)), " ");
            (r().t(format!("{} columns are empty on every row", fmt_int(empty.len()))), if empty.len() > 8 { s.t(" …") } else { s })
        };
        x.add(Sev::Info, Icon::Empty, 30, title, sub).acts = vec![Act::CleanUp];
    }
    if !consts.is_empty() {
        let (title, sub) = if consts.len() == 1 {
            (r().c(consts[0].0).t(" holds ").v(&consts[0].1).t(" on every row"), r().t("It says nothing about any row: a filter or a sort on it does nothing."))
        } else {
            (r().t(format!("{} columns hold one same value on every row", fmt_int(consts.len()))), r().join(consts.iter().take(6).map(|(c, v)| r().c(*c).t(" ").v(v)), " · "))
        };
        x.add(Sev::Info, Icon::Rel, 10, title, sub);
    }
    Some(())
}

/// The key: filled and different on every row (checked on every row when
/// sampled); an id-like name first. Near keys: as good but for a few values
/// that come back. The whole file is read again — once at most — to confirm
/// a key (a sample may miss the one value that comes back) or to count an
/// identifier's repeats, which a sample all but hides.
fn in_keys(x: &mut Ctx) -> Option<()> {
    let mut cands: Vec<usize> = (0..x.cols.len())
        .filter(|&k| {
            let s = &x.cols[k];
            s.filled as f64 >= x.n as f64 * 0.98 && (s.over || s.distinct() as f64 >= s.filled as f64 * 0.9) && s.kind != Kind::Date && x.n >= 20
        })
        .collect();
    cands.sort_by_key(|&k| (!id_like(&x.h[x.cols[k].c]), x.cols[k].c));
    let (mut keyed, mut passes) = (false, 0);
    for &k in cands.iter().take(6) {
        let (c, idl) = (x.cols[k].c, id_like(&x.h[x.cols[k].c]));
        // Values and counts that come back, in the order they come.
        let mut rep: Vec<(String, usize)>;
        let (mut filled, mut rows) = (x.cols[k].filled, x.n);
        {
            let s = &x.cols[k];
            let all;
            let counts = if s.over {
                let mut m = Counter::new();
                for v in s.v.iter().filter(|v| !v.is_empty()) { m.add(v); }
                all = m;
                &all
            } else {
                &s.counts
            };
            rep = counts.items.iter().filter(|e| e.1 > 1).map(|e| (e.0.to_string(), e.1 as usize)).collect();
            let unique = rep.is_empty() && filled == rows;
            if if unique { keyed } else { rep.is_empty() } { continue; }
            // Amounts, dates and free text are all different by chance; an identifier, a code or an e-mail is meant to be.
            if !unique && !idl && (s.kind != Kind::Text || counts.items.iter().take(200).any(|e| e.0.chars().any(kinds::is_js_space))) { continue; }
            if x.sampled {
                if passes >= 1 || !(unique || idl) { continue; }
                passes += 1;
                let (reps, empty) = repeats(x.st, c, x.cancel)?;
                filled = x.total - empty;
                rows = x.total;
                rep = reps.into_iter().map(|r| (r.value, r.pos.len())).collect();
            }
        }
        let rep_rows: usize = rep.iter().map(|e| e.1).sum();
        if rep.is_empty() && filled == rows {
            x.add(Sev::Info, Icon::Key, 40, r().c(c).t(" is filled and different on every row: it identifies them"),
                r().t("A key: what to match rows on in Look up, Compare or Remove duplicates."));
            keyed = true;
            continue;
        }
        if rep.is_empty() || rep_rows as f64 > 2f64.max(filled as f64 * if idl { 0.2 } else { 0.02 }) { continue; }   // a contact list may hold one contact in ten twice
        rep.sort_by(|a, b| b.1.cmp(&a.1));
        let mut sub = r().join(rep.iter().take(3).map(|(v, k)| r().v(v).t(format!(" on {} rows", fmt_int(*k)))), ", ");
        if rep.len() > 3 { sub = sub.t(", …"); }
        let sub = sub.t(format!(" — {} in all.", pl(rep_rows, "row", "rows")));
        let title = r().c(c).t(format!(" identifies every row but {} that come{} back", pl(rep.len(), "value", "values"), if rep.len() == 1 { "s" } else { "" }));
        let label = format!("sharing a {}", x.h[c]);
        let it = x.add(Sev::Warn, Icon::Key, 70, title, sub);
        it.show = show(label, "Also on another row", What::Repeats { col: c });
        it.acts = vec![Act::RemoveDuplicates];
    }
    Some(())
}

/// A value's shape: letters by runs — Aa (a capital then lower case), A, a,
/// x (other scripts) — one control character each; digits as 9 (a run as
/// one, unless `runs` is false: in codes each digit counts); spaces as one.
fn in_shape(v: &str, runs: bool) -> String {
    let v = kinds::utf16_prefix(v, 60);
    let cs: Vec<char> = v.chars().collect();
    let mut out = String::with_capacity(v.len());
    // What the last character written stands for, when a run of the same merges into it.
    let mut prev = 0u8;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_uppercase() && cs.get(i + 1).is_some_and(|n| n.is_lowercase()) {
            i += 1;
            while i < cs.len() && cs[i].is_lowercase() { i += 1; }
            out.push('\x01');
            prev = 0;
            continue;
        }
        let cat = if c.is_uppercase() { 2 } else if c.is_lowercase() { 3 } else if c.is_alphabetic() { 4 } else if c.is_ascii_digit() { 9 } else if kinds::is_js_space(c) { 32 } else { 0 };
        match cat {
            0 => out.push(c),
            _ if cat == prev && (cat != 9 || runs) => {}
            2..=4 => out.push(cat as char),
            9 => out.push('9'),
            _ => out.push(' '),
        }
        prev = cat;
        i += 1;
    }
    out
}

fn caseless(s: &str) -> String {
    s.chars().map(|c| if ('\x01'..='\x04').contains(&c) { 'a' } else { c }).collect()
}

/// The decimal mark of a number with one: `,` for `1 234,50`, `.` for `12.5`
/// (three decimals are a thousands group, not decimals).
fn in_dec(v: &str) -> Option<char> {
    let with = |sep: char, other: char| {
        let s = v.strip_prefix(['+', '-']).unwrap_or(v);
        let Some(k) = s.rfind(sep) else { return false };
        let (head, tail) = (&s[..k], &s[k + 1..]);
        tail.bytes().all(|b| b.is_ascii_digit()) && matches!(tail.len(), 1 | 2 | 4..)
            && head.chars().last().is_some_and(|c| c.is_ascii_digit())
            && head.chars().all(|c| c.is_ascii_digit() || kinds::is_js_space(c) || c == other)
    };
    if with(',', '.') { Some(',') } else if with('.', ',') { Some('.') } else { None }
}

fn starts_0digit(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() >= 2 && b[0] == b'0' && b[1].is_ascii_digit()
}

/// Values written otherwise than the rest of their column. A column whose
/// shape is shared by most values (IN_SHAPE_MIN) and not by a few: those few.
/// Letters only: just the case (names vary in shape, Jean-Pierre is not a
/// misfit, DUPONT among Dupont is). Numbers: the decimal mark.
fn in_shapes(x: &mut Ctx) -> Option<()> {
    // Column by column, the columns in parallel (each over the sample's values); the
    // cards in column order.
    let cx = &*x;
    let per: Vec<Vec<Insight>> = (0..cx.cols.len()).into_par_iter().map(|k| {
        let (x, mut out) = (cx, Vec::new());
        if x.cancelled() { return out; }
        let s = &x.cols[k];
        if s.filled < 20 || s.distinct() < 3 { return out; }
        let (c, name) = (s.c, x.h[s.c].clone());
        let codes = s.kind == Kind::Num && (code_like(&name) || s.v.iter().any(starts_0digit))
            && !s.v.iter().any(|v| v.as_bytes().windows(2).any(|w| (w[0] == b'.' || w[0] == b',') && w[1].is_ascii_digit()) && !starts_0digit(v));
        if s.kind == Kind::Num && !codes {
            // A quantity: only its decimal mark.
            let (mut comma, mut point) = (Vec::new(), Vec::new());
            for v in s.v.iter().filter(|v| !v.is_empty()) {
                match in_dec(v) { Some(',') => comma.push(v), Some(_) => point.push(v), None => {} }
            }
            let (maj, min, km, kn) = if comma.len() >= point.len() { (',', '.', &comma, &point) } else { ('.', ',', &point, &comma) };
            if kn.is_empty() || kn.len() as f64 > (km.len() + kn.len()) as f64 * 0.1 { return out; }
            let word = |m: char| if m == ',' { "comma" } else { "point" };
            let title = r().c(c).t(format!(": {} written with a {} where the others use a {}", x.ns(kn.len() as f64, "number", "numbers"), word(min), word(maj)));
            let sub = r().v(kn[0]).t(" among values like ").v(km[0]).t(". A program reading one convention misreads the other — 12.50 may become 1250.");
            let mut it = card(Sev::Warn, Icon::Shape, 55, title, sub);
            it.show = show(format!("with a {} in {name}", if min == ',' { "decimal comma" } else { "decimal point" }), "Usual form", What::Decimal { col: c, mark: min, usual: km[0].to_string() });
            out.push(it);
            return out;
        }
        let (mut len, mut n) = (0, 0);
        for v in s.v.iter().filter(|v| !v.is_empty()) {
            len += kinds::utf16_len(v);
            n += 1;
            if n >= 2000 { break; }
        }
        if len as f64 / n as f64 > 40.0 { return out; }   // free text
        // Digits one by one only in codes (75011 vs 7501): elsewhere a run is a run.
        let runs = !codes;
        let mut shapes: Counter<String> = Counter::new();
        let mut ex: FxMap<String, String> = FxMap::default();
        let count = |shapes: &mut Counter<String>, ex: &mut FxMap<String, String>, upto: usize| {
            for i in 0..upto {
                let v = s.v.get(i);
                if v.is_empty() { continue; }
                let sh = in_shape(v, runs);
                if !ex.contains_key(&sh) { ex.insert(sh.clone(), v.to_string()); }
                shapes.add(sh);
            }
        };
        // The usual shape from IN_SHAPE_ROWS values; the rest are read only for a column that has one.
        let whole = s.v.len() <= IN_SHAPE_ROWS;
        count(&mut shapes, &mut ex, s.v.len().min(IN_SHAPE_ROWS));
        let seen: u32 = shapes.items.iter().map(|e| e.1).sum();
        let Some((dom, mut dn)) = shapes.top().cloned() else { return out };
        if (dn as f64 / seen as f64) < IN_SHAPE_MIN || (dn == seen && whole) { return out; }
        if !whole {
            shapes = Counter::new();
            count(&mut shapes, &mut ex, s.v.len());
            dn = shapes.get(&dom);
            if (dn as f64 / s.filled as f64) < IN_SHAPE_MIN || dn as usize == s.filled { return out; }
        }
        let digits = dom.contains('9') || s.kind == Kind::Date;
        let cdom = caseless(&dom);
        let odd = |sh: &str| sh != dom && (digits || caseless(sh) == cdom);
        let mut odds: Vec<&(String, u32)> = shapes.items.iter().filter(|e| odd(&e.0)).collect();
        odds.sort_by(|a, b| b.1.cmp(&a.1));
        let m: u32 = odds.iter().map(|e| e.1).sum();
        if m == 0 || m as f64 > s.filled as f64 * if digits { IN_SHAPE_ODD } else { IN_SHAPE_CASE } { return out; }
        if codes && s.distinct() == s.filled { return out; }   // numbered rows (1 … 10 000), not codes of one width
        let case_only = !digits;
        let title = r().c(c).t(format!(": {} {}", x.ns(m as f64, "value", "values"), if case_only { "in another case than the others" } else { "written otherwise than the others" }));
        let all9 = |s: &str| !s.is_empty() && s.bytes().all(|b| b == b'9');
        let mut sub = r().join(odds.iter().take(3).map(|e| r().v(&ex[&e.0])), ", ");
        if odds.len() > 3 { sub = sub.t(", …"); }
        let mut sub = sub.t(" where most look like ").v(&ex[&dom]).t(".");
        if digits && all9(&dom) && odds.iter().any(|e| all9(&e.0) && e.0.len() < dom.len()) { sub = sub.t(" Leading zeros lost on the way?"); }
        let usual = ex[&dom].to_string();
        let mut it = card(Sev::Warn, Icon::Shape, if case_only { 40 } else { 60 }, title, sub);
        it.show = show(format!("written otherwise in {name}"), "Usual form", What::Shape { col: c, runs, dom, digits, usual });
        out.push(it);
        out
    }).collect();
    if x.cancelled() { return None; }
    let out: Vec<Insight> = per.into_iter().flatten().collect();
    x.items.extend(out);
    Some(())
}

/// A number or a date as compared: NaN when it is not one.
fn far_key(v: &str, date: bool) -> f64 {
    if v.is_empty() { return f64::NAN; }
    if date {
        let k = kinds::date_key(v);
        return if k.is_nan() { k } else { kinds::key_days(k) };
    }
    if kinds::is_numeric_like(v) { kinds::num_key(v) } else { f64::NAN }
}

/// Numbers and dates far from all the others: past `stats::fences`, the charts'
/// far out (an extreme outlier for Tukey), a handful at most — a long tail of
/// amounts is no finding, though the charts set it apart. A few negatives among
/// positives.
fn in_far(x: &mut Ctx) -> Option<()> {
    // Column by column, the columns in parallel (each over the sample's values); the
    // cards in column order.
    let cx = &*x;
    let per: Vec<Vec<Insight>> = (0..cx.cols.len()).into_par_iter().map(|k| {
        let (x, mut out) = (cx, Vec::new());
        if x.cancelled() { return out; }
        let s = &x.cols[k];
        if !matches!(s.kind, Kind::Num | Kind::Date) || s.filled < 30 { return out; }
        let (c, name, date) = (s.c, x.h[s.c].clone(), s.kind == Kind::Date);
        let keyed: Vec<(f64, &str)> = s.v.iter().map(|v| (far_key(v, date), v)).collect();
        let xs: Vec<f64> = keyed.iter().map(|e| e.0).filter(|x| !x.is_nan()).collect();
        if xs.len() < 30 { return out; }
        if !date && xs.iter().all(|x| x.fract() == 0.0) && s.distinct() == s.filled { return out; }   // identifiers
        let mut q = xs.clone();
        q.sort_by(|a, b| a.total_cmp(b));
        let at = |p: f64| q[(q.len() - 1).min((p * (q.len() - 1) as f64).floor() as usize)];
        let (q1, q3) = (at(0.25), at(0.75));
        let iqr = q3 - q1;
        let mut add_far = None;
        if iqr > 0.0 {
            let (lo, hi) = crate::stats::fences(q1, q3);
            let mut far: Vec<(f64, &str)> = keyed.iter().filter(|e| !e.0.is_nan() && (e.0 < lo || e.0 > hi)).copied().collect();
            if !far.is_empty() && far.len() as f64 <= 1f64.max(xs.len() as f64 * 0.01) {
                let mid = (q1 + q3) / 2.0;
                far.sort_by(|a, b| (b.0 - mid).abs().total_cmp(&(a.0 - mid).abs()));
                let p05 = q[(0.05 * (q.len() - 1) as f64).floor() as usize];
                let p95 = q[(0.95 * (q.len() - 1) as f64).floor() as usize];
                let wrote = |x: f64| keyed.iter().find(|e| e.0 == x).map_or_else(|| kinds::js_num_str(x), |e| e.1.to_string());
                let (w05, w95) = (wrote(p05), wrote(p95));
                let title = r().c(c).t(format!(": {} far from all the others", x.ns(far.len() as f64, if date { "date" } else { "value" }, if date { "dates" } else { "values" })));
                let mut sub = r().join(far.iter().take(3).map(|e| r().v(e.1)), ", ");
                if far.len() > 3 { sub = sub.t(", …"); }
                let sub = sub.t(", where most lie between ").v(&w05).t(" and ").v(&w95).t(".");
                add_far = Some((title, sub, What::Far { col: c, date, lo, hi, range: format!("{w05} and {w95}") }));
            }
        }
        let neg_card = if !date {
            let neg = xs.iter().filter(|&&x| x < 0.0).count();
            if neg > 0 && neg as f64 <= 1f64.max(xs.len() as f64 * 0.01) && xs.len() - neg >= 50 {
                let title = r().c(c).t(format!(": {} among positive ones", x.ns(neg as f64, "negative number", "negative numbers")));
                let sub = r().join(keyed.iter().filter(|e| e.0 < 0.0).take(3).map(|e| r().v(e.1)), ", ").t(" — a credit note, or a sign typed by mistake?");
                Some((title, sub))
            } else { None }
        } else { None };
        if let Some((title, sub, what)) = add_far {
            let mut it = card(Sev::Warn, Icon::Far, 50, title, sub);
            it.show = show(format!("far from the others in {name}"), "Most lie between", what);
            out.push(it);
        }
        if let Some((title, sub)) = neg_card {
            let mut it = card(Sev::Warn, Icon::Far, 45, title, sub);
            it.show = show(format!("below zero in {name}"), "The others are", What::Negative { col: c });
            out.push(it);
        }
        out
    }).collect();
    if x.cancelled() { return None; }
    let out: Vec<Insight> = per.into_iter().flatten().collect();
    x.items.extend(out);
    Some(())
}

/// The decimals a value is written with (three after the mark are a thousands group).
fn decimals(v: &str) -> i32 {
    let b = v.as_bytes();
    let run = b.iter().rev().take_while(|c| c.is_ascii_digit()).count();
    if run == 0 || run == 3 || run == b.len() || !matches!(b[b.len() - run - 1], b'.' | b',') { 0 } else { run as i32 }
}

/// A cell as a number for the arithmetic: NaN when it is not one.
fn arith_num(v: &str) -> f64 {
    if !v.is_empty() && kinds::is_numeric_like(v) { kinds::num_key(v) } else { f64::NAN }
}

struct Found {
    rel: Rel,
    kt: String,
    bad: usize,
    both: usize,
    cs: Vec<usize>,
}

/// Hidden arithmetic between numeric columns: B = A × k (k the most common
/// ratio, to 4 significant digits), C = A + B. Checked to half a unit of the
/// result's last decimal, so amounts rounded to the cent still agree. On
/// every row, or on ≥ 90 % of them: the others.
fn in_arithmetic(x: &mut Ctx) -> Option<()> {
    let n = x.n.min(IN_REL_ROWS);
    let cols: Vec<&Col> = x.cols.iter()
        .filter(|s| s.kind == Kind::Num && s.filled >= 20 && s.distinct() >= 5 && !(s.distinct() == s.filled && id_like(&x.h[s.c])))
        .take(IN_REL_COLS)
        .collect();
    if cols.len() < 2 { return Some(()); }
    let xv: Vec<Vec<f64>> = cols.iter().map(|s| (0..n).map(|i| arith_num(s.v.get(i))).collect()).collect();
    let tol: Vec<Vec<f64>> = cols.iter().map(|s| (0..n).map(|i| 0.5 * 10f64.powi(-decimals(s.v.get(i))) + 1e-9).collect()).collect();
    let ok_rel = |both: usize, ok: usize| both >= 20 && ok as f64 / both as f64 >= 0.9;
    // B after A in the file: a derived column usually comes after what it is computed from (HT, TVA, TTC).
    let pairs: Vec<(usize, usize)> = (0..cols.len()).flat_map(|a| (a + 1..cols.len()).map(move |b| (a, b))).collect();
    let mut found: Vec<Found> = pairs.par_iter().filter_map(|&(a, b)| {
        let (av, bv) = (&xv[a], &xv[b]);
        let live = |i: usize| av[i] != 0.0 && bv[i] != 0.0 && av[i].is_finite() && bv[i].is_finite();
        let mut ratios: Counter<u64> = Counter::new();
        let mut both = 0;
        for i in (0..n).filter(|&i| live(i)) {
            both += 1;
            let r = kinds::round_sig(bv[i] / av[i], 4);
            ratios.add(if r == 0.0 { 0 } else { r.to_bits() });
        }
        let &(kb, kn) = ratios.top()?;
        let k = f64::from_bits(kb);
        if both == 0 || (kn as f64 / both as f64) < 0.5 || k <= 0.0 { return None; }
        let ok = (0..n).filter(|&i| live(i) && (bv[i] - av[i] * k).abs() <= tol[b][i] + 1e-9 * bv[i].abs()).count();
        ok_rel(both, ok).then(|| Found { rel: Rel::Mul { a: cols[a].c, b: cols[b].c, k }, kt: kinds::fmt_sig(k, 4), bad: both - ok, both, cs: vec![cols[a].c, cols[b].c] })
    }).collect();
    if x.cancelled() { return None; }
    if cols.len() <= 12 {
        let triples: Vec<(usize, usize, usize)> = pairs.iter().flat_map(|&(a, b)| (0..cols.len()).filter(move |&c| c != a && c != b).map(move |c| (a, b, c))).collect();
        found.extend(triples.par_iter().filter_map(|&(a, b, c)| {
            let (av, bv, cv) = (&xv[a], &xv[b], &xv[c]);
            let (mut both, mut ok, mut nza, mut nzb) = (0, 0, 0, 0);
            for i in 0..n {
                if !av[i].is_finite() || !bv[i].is_finite() || !cv[i].is_finite() { continue; }
                both += 1;
                if av[i] != 0.0 { nza += 1; }
                if bv[i] != 0.0 { nzb += 1; }
                if (cv[i] - av[i] - bv[i]).abs() <= tol[c][i] + 1e-9 * cv[i].abs() { ok += 1; }
            }
            if (nza as f64) < both as f64 / 2.0 || (nzb as f64) < both as f64 / 2.0 { return None; }   // C = A + 0 is no sum
            ok_rel(both, ok).then(|| Found { rel: Rel::Add { a: cols[a].c, b: cols[b].c, c: cols[c].c }, kt: String::new(), bad: both - ok, both, cs: vec![cols[a].c, cols[b].c, cols[c].c] })
        }).collect::<Vec<_>>());
    }
    // The surest first, a sum before a product (TTC = HT + TVA says more than
    // TTC = TVA × 6); a rule whose columns are all in rules kept says nothing new.
    let is_mul = |f: &Found| matches!(f.rel, Rel::Mul { .. });
    found.sort_by(|p, q| {
        (p.bad as f64 / p.both as f64).total_cmp(&(q.bad as f64 / q.both as f64))
            .then(is_mul(p).cmp(&is_mul(q)))
            .then(kinds::utf16_len(&p.kt).cmp(&kinds::utf16_len(&q.kt)))
    });
    let mut covered = FxSet::default();
    let mut kept = Vec::new();
    for f in found {
        if f.cs.iter().all(|c| covered.contains(c)) { continue; }
        covered.extend(f.cs.iter().copied());
        kept.push(f);
        if kept.len() >= 4 { break; }
    }
    for f in kept {
        let h = &x.h;
        let (what, plain, target) = match f.rel {
            Rel::Mul { a, b, k } if k == 1.0 => (r().c(b).t(" holds the same number as ").c(a), format!("{} = {}", h[b], h[a]), b),
            Rel::Mul { a, b, .. } => (r().c(b).t(" = ").c(a).t(format!(" × {}", f.kt)), format!("{} = {} × {}", h[b], h[a], f.kt), b),
            Rel::Add { a, b, c } => (r().c(c).t(" = ").c(a).t(" + ").c(b), format!("{} = {} + {}", h[c], h[a], h[b]), c),
        };
        if f.bad > 0 {
            let title = what.t(format!(" — except on {}", x.ns(f.bad as f64 * x.n as f64 / n as f64, "row", "rows")));
            let sub = r().t(format!("True on {} rows of {} read; the others break the rule: a typo, a rate that changed, a line edited by hand?", fmt_int(f.both - f.bad), fmt_int(f.both)));
            let it = x.add(Sev::Warn, Icon::Rel, 65, title, sub);
            it.show = show(format!("breaking {plain}"), "The rule gives", What::Rule { rel: f.rel, target });
        } else {
            let sub = r().t(format!("A rule the file keeps without saying it: {plain}{}.", if x.sampled || n < x.n { " on the rows read" } else { "" }));
            x.add(Sev::Info, Icon::Rel, 35, what.t(", on every row"), sub);
        }
    }
    Some(())
}

/// A column's values over the first rows read, as small integers (-1: empty).
struct Ids<'a> {
    a: Vec<i32>,
    d: usize,
    vals: Vec<&'a str>,
}

fn ids_of<'a>(s: &Col<'a>, n: usize) -> Ids<'a> {
    let mut m: FxMap<&str, i32> = FxMap::default();
    let mut vals = Vec::new();
    let a = (0..n).map(|i| {
        let v = s.v.get(i);
        if v.is_empty() { return -1; }
        *m.entry(v).or_insert_with(|| { vals.push(v); vals.len() as i32 - 1 })
    }).collect();
    Ids { a, d: vals.len(), vals }
}

struct DepFound {
    sa: usize,
    sb: usize,
    size: Vec<u32>,
    best_b: Vec<i32>,
    bad: usize,
    cover: usize,
}

/// Whether A decides B on the first n rows read, but for a few rows.
fn dep_pair(a: &Ids, b: &Ids, n: usize) -> Option<(Vec<u32>, Vec<i32>, usize, usize)> {
    if b.d as f64 > a.d as f64 * 1.1 + 2.0 { return None; }   // more values than the column deciding it: no
    let w = b.d as u64 + 1;
    let mut cnt: Counter<u64> = Counter::new();
    for i in 0..n {
        let g = a.a[i];
        if g < 0 { continue; }
        let bv = if b.a[i] < 0 { b.d as u64 } else { b.a[i] as u64 };
        cnt.add(g as u64 * w + bv);
    }
    let (mut size, mut best, mut best_b) = (vec![0u32; a.d], vec![0u32; a.d], vec![-1i32; a.d]);
    for &(k, m) in &cnt.items {
        let (g, bv) = ((k / w) as usize, (k % w) as i32);
        size[g] += m;
        if m > best[g] { best[g] = m; best_b[g] = bv; }
    }
    let (mut cover, mut bad, mut groups, mut empty_best) = (0usize, 0usize, 0usize, 0usize);
    let mut modes = FxSet::default();
    for g in 0..a.d {
        if size[g] >= 2 {
            groups += 1;
            cover += size[g] as usize;
            bad += (size[g] - best[g]) as usize;
            modes.insert(best_b[g]);
            if best_b[g] == b.d as i32 { empty_best += size[g] as usize; }
        }
    }
    // Groups of 3 rows on average at least: with pairs, any column "decides" another by chance.
    let cover_f = cover as f64;
    (groups >= 5 && cover >= groups * 3 && cover_f >= n as f64 * 0.3 && bad > 0 && bad as f64 <= cover_f * 0.02 && bad <= groups && modes.len() >= 2 && empty_best as f64 <= cover_f * 0.2)
        .then_some((size, best_b, bad, cover))
}

/// A column that decides another: every value of A comes with one value of B
/// (postal code → city, product → price), but for a few rows that disagree
/// with the rest of their group. Pairs of columns over the rows read
/// (IN_FD_ROWS): A repeats (2 to n/2 values), B has no more values than A.
fn in_deps(x: &mut Ctx) -> Option<()> {
    let n = x.n.min(IN_FD_ROWS);
    if n < 30 { return Some(()); }
    let as_: Vec<usize> = (0..x.cols.len()).filter(|&k| { let s = &x.cols[k]; !s.over && s.distinct() >= 2 && s.distinct() as f64 <= n as f64 / 2.0 && s.filled as f64 >= x.n as f64 * 0.5 }).take(IN_FD_COLS).collect();
    let bs: Vec<usize> = (0..x.cols.len()).filter(|&k| { let s = &x.cols[k]; !s.over && s.distinct() >= 2 && (s.distinct() as f64) < x.n as f64 * 0.9 }).take(IN_FD_COLS + 10).collect();
    let mut need: Vec<usize> = as_.iter().chain(&bs).copied().collect();
    need.sort_unstable();
    need.dedup();
    let ids: FxMap<usize, Ids> = need.par_iter().map(|&k| (k, ids_of(&x.cols[k], n))).collect();
    let pairs: Vec<(usize, usize)> = as_.iter()
        .filter(|&&a| ids[&a].d as f64 <= n as f64 / 2.0 && ids[&a].d >= 2)
        .flat_map(|&a| bs.iter().filter(move |&&b| b != a).map(move |&b| (a, b)))
        .collect();
    let results: FxMap<(usize, usize), _> = pairs.par_iter().filter_map(|&(a, b)| dep_pair(&ids[&a], &ids[&b], n).map(|r| ((a, b), r))).collect();
    if x.cancelled() { return None; }
    // In the JS order: a pair found keeps its reverse from being tried.
    let mut done = FxSet::default();
    let mut found = Vec::new();
    for &(a, b) in &pairs {
        let (ca, cb) = (x.cols[a].c, x.cols[b].c);
        if done.contains(&(cb, ca)) { continue; }
        if let Some((size, best_b, bad, cover)) = results.get(&(a, b)) {
            done.insert((ca, cb));
            found.push(DepFound { sa: a, sb: b, size: size.clone(), best_b: best_b.clone(), bad: *bad, cover: *cover });
        }
    }
    found.sort_by(|p, q| (p.bad as f64 / p.cover as f64).total_cmp(&(q.bad as f64 / q.cover as f64)));
    for f in found.into_iter().take(3) {
        let (av, bv) = (&ids[&f.sa], &ids[&f.sb]);
        let (ca, cb) = (x.cols[f.sa].c, x.cols[f.sb].c);
        let (an, bn) = (x.h[ca].clone(), x.h[cb].clone());
        let bval = |b: i32| if b as usize == bv.d { "" } else { bv.vals[b as usize] };
        // An example: the biggest group with a dissenting row.
        let (mut eg, mut ei) = (-1i64, 0usize);
        for i in 0..n {
            let g = av.a[i];
            if g < 0 || f.size[g as usize] < 2 { continue; }
            let b = if bv.a[i] < 0 { bv.d as i32 } else { bv.a[i] };
            if b != f.best_b[g as usize] && (eg < 0 || f.size[g as usize] > f.size[eg as usize]) { eg = g as i64; ei = i; }
        }
        let sub = if eg >= 0 {
            let g = eg as usize;
            r().v(av.vals[g]).t(" comes with ").v(bval(f.best_b[g])).t(format!(" on {} rows, but ", fmt_int(f.size[g] as usize - 1))).v(x.cols[f.sb].v.get(ei)).t(" on another.")
        } else { r() };
        let title = r().c(ca).t(" decides ").c(cb).t(format!(" — except on {}", x.ns(f.bad as f64 * x.n as f64 / n as f64, "row", "rows")));
        let it = x.add(Sev::Warn, Icon::Dep, 68, title, sub.t(" A typo, or a value updated in one place only?"));
        it.show = show(format!("where {bn} disagrees with {an}"), &format!("Usually, for this {an}"), What::Dep { a: ca, b: cb });
    }
    Some(())
}

/// The card of `n` rows copying an earlier one (counted by `copies`).
fn in_copies(x: &mut Ctx, n: usize) {
    if n == 0 { return; }
    let title = r().t(format!("{} {} of an earlier row", pl(n, "row", "rows"), if n == 1 { "is an exact copy" } else { "are exact copies" }));
    let sub = r().t("Every field the same. Remove duplicates keeps one of each, or finds the rows that differ only by case, spaces or spelling.");
    let it = x.add(Sev::Warn, Icon::Dup, 75, title, sub);
    it.show = show("copying an earlier row".into(), "Expected", What::Copies);
    it.show_label = "Show the copies";
    it.acts = vec![Act::RemoveDuplicates];
}

/// What the file holds that stands out. `progress`, if given, goes from 0 to
/// 1000. None if cancelled.
pub fn insights(st: &State, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Report> {
    let set = |p: usize| if let Some(a) = progress { a.store(p, Ordering::Relaxed) };
    set(0);
    let ncols = kinds::expected_width(st).min(st.ncols).max(1);
    let sample = read_sample(st, ncols, cancel)?;
    set(250);
    let cols: Vec<Col> = sample.vals.par_iter().enumerate().map(|(c, v)| col_stats(c, v, sample.raw[c])).collect();
    let mut x = Ctx { st, h: st.headers(), n: sample.n, total: sample.total, sampled: sample.sampled, cols, items: Vec::new(), cancel };
    type Step = fn(&mut Ctx) -> Option<()>;
    let steps: [Step; 7] = [in_structure, in_columns, in_shapes, in_far, in_arithmetic, in_deps, in_keys];
    // The copies (a pass over every row) are counted while the other steps run; their
    // cards go where that step stood, before the keys'.
    let (copied, keys_at) = rayon::join(
        || copies(st, cancel),
        || -> Option<usize> {
            let mut at = 0;
            for (k, step) in steps.iter().enumerate() {
                set(250 + 750 * k / steps.len());
                if x.cancelled() { return None; }
                if k + 1 == steps.len() { at = x.items.len(); }
                step(&mut x)?;
            }
            Some(at)
        },
    );
    let keys = x.items.split_off(keys_at?);
    in_copies(&mut x, copied?.len());
    x.items.extend(keys);
    set(1000);
    let mut items = x.items;
    items.sort_by(|a, b| a.sev.cmp(&b.sev).then(b.score.cmp(&a.score)));
    Some(Report { items, n: sample.n, total: sample.total, sampled: sample.sampled })
}

/// The rows of a card, counted again on every row; None if cancelled.
pub fn marks(st: &State, show: &Show, cancel: &AtomicBool) -> Option<Marks> {
    let mut m = Marks { label: show.label.clone(), tip: show.tip.clone(), ..Default::default() };
    let cells = |col: usize, hits: Vec<(RowId, String)>, m: &mut Marks| {
        m.rows = hits.iter().map(|h| h.0).collect();
        m.cells = hits.into_iter().map(|(id, w)| (id, col, w)).collect();
    };
    match &show.what {
        What::Repeats { col } => {
            let (reps, _) = repeats(st, *col, cancel)?;
            let mut hits: Vec<(u32, &str)> = reps.iter().flat_map(|r| r.pos.iter().map(move |&p| (p, r.value.as_str()))).collect();
            hits.par_sort_unstable();
            cells(*col, hits.into_iter().map(|(p, v)| (st.data_id(p as usize), v.to_string())).collect(), &mut m);
        }
        What::Decimal { col, mark, usual } => cells(*col, every_cell(st, *col, cancel, |v| (!v.is_empty() && in_dec(v) == Some(*mark)).then(|| usual.clone()))?, &mut m),
        What::Shape { col, runs, dom, digits, usual } => {
            let cdom = caseless(dom);
            let odd = |sh: &str| sh != dom && (*digits || caseless(sh) == cdom);
            cells(*col, every_cell(st, *col, cancel, |v| (!v.is_empty() && odd(&in_shape(v, *runs))).then(|| usual.clone()))?, &mut m);
        }
        What::Far { col, date, lo, hi, range } => {
            cells(*col, every_cell(st, *col, cancel, |v| { let k = far_key(v, *date); (!k.is_nan() && (k < *lo || k > *hi)).then(|| range.clone()) })?, &mut m)
        }
        What::Negative { col } => cells(*col, every_cell(st, *col, cancel, |v| (far_key(v, false) < 0.0).then(|| "0 or more".to_string()))?, &mut m),
        What::Copies => m.rows = copies(st, cancel)?,
        What::Rule { rel, target } => {
            let cols: Vec<usize> = match *rel { Rel::Mul { a, b, .. } => vec![a, b], Rel::Add { a, b, c } => vec![a, b, c] };
            let tol = |v: &str, x: f64| 0.5 * 10f64.powi(-decimals(v)) + 1e-9 * x.abs() + 1e-9;
            // What the rule gives where the row breaks it.
            let want = |v: &[String]| -> Option<f64> {
                let g: Vec<f64> = v.iter().map(|s| arith_num(s)).collect();
                match *rel {
                    Rel::Mul { k, .. } => {
                        if g[0] == 0.0 || g[1] == 0.0 || g[0].is_nan() || g[1].is_nan() { return None; }
                        let w = g[0] * k;
                        ((g[1] - w).abs() > tol(&v[1], g[1])).then_some(w)
                    }
                    Rel::Add { .. } => {
                        if g.iter().any(|x| x.is_nan()) { return None; }
                        ((g[2] - g[0] - g[1]).abs() > tol(&v[2], g[2])).then_some(g[0] + g[1])
                    }
                }
            };
            let parts = every(st, cancel, || (Vec::new(), Vec::new()), |(out, v): &mut (Vec<(RowId, String)>, Vec<String>), _, id| {
                v.clear();
                v.resize(cols.len(), String::new());
                st.visit_cols(id, &cols, |k, s| v[k] = js_trim(s).to_string());
                if let Some(w) = want(v) { out.push((id, kinds::fmt_frac(w, 4))); }
            })?;
            cells(*target, parts.into_iter().flat_map(|p| p.0).collect(), &mut m);
        }
        What::Dep { a, b } => {
            // Each value of A, its values of B: (count, first row) — the mode is the most common, the first of equals.
            type Groups = FxMap<String, FxMap<String, (u32, u32)>>;
            let parts = every(st, cancel, || (Groups::default(), Vec::new()), |(g, tmp): &mut (Groups, Vec<u8>), p, id| {
                let av = st.with_cell(id, *a, tmp, |v| js_trim(v).to_string());
                if av.is_empty() { return; }
                let bv = st.with_cell(id, *b, tmp, |v| js_trim(v).to_string());
                g.entry(av).or_default().entry(bv).or_insert((0, p as u32)).0 += 1;
            })?;
            let mut groups = Groups::default();
            for (g, _) in parts {
                for (av, bs) in g {
                    let e = groups.entry(av).or_default();
                    for (bv, (k, p)) in bs {
                        let x = e.entry(bv).or_insert((0, p));
                        x.0 += k;
                        x.1 = x.1.min(p);
                    }
                }
            }
            let mode: FxMap<String, String> = groups.into_iter().filter_map(|(av, bs)| {
                let tot: u32 = bs.values().map(|e| e.0).sum();
                let best = bs.into_iter().max_by(|p, q| p.1 .0.cmp(&q.1 .0).then(q.1 .1.cmp(&p.1 .1)))?;
                (tot >= 2).then_some((av, best.0))
            }).collect();
            let parts = every(st, cancel, || (Vec::new(), Vec::new()), |(out, tmp): &mut (Vec<(RowId, String)>, Vec<u8>), _, id| {
                let Some(want) = st.with_cell(id, *a, tmp, |v| mode.get(js_trim(v))) else { return };
                if st.with_cell(id, *b, tmp, |v| js_trim(v) != want) { out.push((id, want.clone())); }
            })?;
            cells(*b, parts.into_iter().flat_map(|p| p.0).collect(), &mut m);
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    const VILLES: [(&str, &str); 6] = [("75011", "Paris"), ("69003", "Lyon"), ("13001", "Marseille"), ("33000", "Bordeaux"), ("31000", "Toulouse"), ("44000", "Nantes")];
    const NOMS: [&str; 10] = ["Martin", "Bernard", "Thomas", "Petit", "Robert", "Richard", "Durand", "Dubois", "Moreau", "Laurent"];

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn fr(x: f64) -> String {
        format!("{x:.2}").replace('.', ",")
    }

    /// 200 rows with defects set by hand (tests/test_insights.py's piege()).
    pub(crate) fn piege() -> String {
        let mut lines = vec!["id;cp;ville;nom;date;ht;tva;ttc;pays;fax;qte".to_string()];
        for i in 0..198usize {
            let (mut cp, mut ville) = VILLES[i % 6];
            if i == 12 { ville = "Pariss"; }
            if i == 54 || i == 66 { cp = "7501"; }
            let nom = if i == 20 || i == 21 { NOMS[i % 10].to_uppercase() } else { NOMS[i % 10].to_string() };
            let (d, m) = (1 + i % 28, 1 + i % 12);
            let date = if i == 30 || i == 31 { format!("2024-{m:02}-{d:02}") } else { format!("{d:02}/{m:02}/2024") };
            let ht = ((i * 7) % 300) as f64 + 10.5;
            let (tva, mut ttc) = ((ht * 0.2 * 100.0).round() / 100.0, (ht * 1.2 * 100.0).round() / 100.0);
            if i == 40 { ttc += 5.0; }
            let hts = if i == 70 { format!("{ht:.2}") } else { fr(ht) };
            let qte = if i == 60 { 5000 } else if i == 61 { -3 } else { 1 + (i as i64 * 3) % 7 };
            lines.push(format!("{};{cp};{ville};{nom};{date};{hts};{};{};FR;;{qte}", i + 1, fr(tva), fr(ttc)));
        }
        let (a, b) = (lines[1].clone(), lines[2].clone());
        lines.push(a);
        lines.push(b);
        lines.join("\r\n") + "\r\n"
    }

    fn texts(t: &Table, rep: &Report) -> Vec<(Sev, String)> {
        let h = t.st.headers();
        rep.items.iter().map(|it| (it.sev, format!("{} {}", it.title.plain(&h), it.sub.plain(&h)))).collect()
    }

    fn find<'a>(rep: &'a Report, cards: &[(Sev, String)], words: &[&str]) -> &'a Insight {
        let k = cards.iter().position(|c| words.iter().all(|w| c.1.contains(w)));
        &rep.items[k.unwrap_or_else(|| panic!("no card with {words:?} among {cards:#?}"))]
    }

    fn shown(t: &Table, it: &Insight) -> (Vec<String>, Vec<(String, String, String)>) {
        let m = marks(&t.st, it.show.as_ref().unwrap(), &AtomicBool::new(false)).unwrap();
        let h = t.st.headers();
        (m.rows.iter().map(|&id| t.st.cell(id, 0)).collect(), m.cells.iter().map(|(id, c, w)| (t.st.cell(*id, 0), h[*c].clone(), w.clone())).collect())
    }

    #[test]
    fn cards() {
        let t = table(&piege());
        let rep = insights(&t.st, &AtomicBool::new(false), None).unwrap();
        let c = texts(&t, &rep);
        find(&rep, &c, &["cp", "decides", "ville", "except on 1 row"]);
        find(&rep, &c, &["cp", "written otherwise", "7501", "Leading zeros"]);
        find(&rep, &c, &["nom", "another case", "MARTIN"]);
        find(&rep, &c, &["date", "written otherwise", "2024-"]);
        find(&rep, &c, &["ht", "point", "comma"]);
        find(&rep, &c, &["ttc = ht + tva", "except on 1 row"]);
        find(&rep, &c, &["tva = ht × 0,2", "every row"]);
        find(&rep, &c, &["qte", "far from all the others", "5000"]);
        find(&rep, &c, &["qte", "negative"]);
        find(&rep, &c, &["2 rows are exact copies"]);
        find(&rep, &c, &["id", "identifies every row but 2 values"]);
        find(&rep, &c, &["fax", "empty on every row"]);
        find(&rep, &c, &["pays", "FR", "every row"]);
        assert_eq!(c[0].0, Sev::Warn);
        assert!(rep.summary().contains("200 rows read"), "{}", rep.summary());
    }

    #[test]
    fn show_marks_the_right_rows() {
        let t = table(&piege());
        let rep = insights(&t.st, &AtomicBool::new(false), None).unwrap();
        let c = texts(&t, &rep);
        let dep = shown(&t, find(&rep, &c, &["decides", "ville"]));
        assert_eq!(dep.0, vec!["13"]);
        assert_eq!(dep.1, vec![("13".into(), "ville".into(), "Paris".into())]);
        assert_eq!(shown(&t, find(&rep, &c, &["ttc = ht + tva"])).0, vec!["41"]);
        assert_eq!(shown(&t, find(&rep, &c, &["written otherwise", "7501"])).0, vec!["55", "67"]);
        assert_eq!(shown(&t, find(&rep, &c, &["exact copies"])).0, vec!["1", "2"]);
        assert_eq!(shown(&t, find(&rep, &c, &["far from"])).0, vec!["61"]);
        let ids = shown(&t, find(&rep, &c, &["identifies every row but"]));
        assert_eq!(ids.0, vec!["1", "2", "1", "2"]);
        assert_eq!(shown(&t, find(&rep, &c, &["negative"])).1, vec![("62".into(), "qte".into(), "0 or more".into())]);
        let rule = shown(&t, find(&rep, &c, &["ttc = ht + tva"]));
        assert_eq!(rule.1[0].1, "ttc");
    }

    #[test]
    fn clean_file_says_nothing_stands_out() {
        let mut s = String::from("id;nom;ville\r\n");
        for i in 1..60 { s += &format!("{i};{};{}\r\n", NOMS[i % 10], VILLES[i % 6].1); }
        let t = table(&s);
        let rep = insights(&t.st, &AtomicBool::new(false), None).unwrap();
        let warns: Vec<_> = texts(&t, &rep).into_iter().filter(|c| c.0 == Sev::Warn).collect();
        assert!(warns.is_empty(), "{warns:?}");
        assert!(rep.items.iter().any(|it| it.icon == Icon::Key), "id is a key");
    }

    #[test]
    fn edited_rows_are_read_as_edited() {
        let mut t = table(&piege());
        // The copy of row 1 edited: one copy left; its id now unique.
        let id = t.st.data_id(198);
        t.set_cells(&[(id, 0, "999".into()), (id, 3, "Autre".into())]);
        let rep = insights(&t.st, &AtomicBool::new(false), None).unwrap();
        let c = texts(&t, &rep);
        find(&rep, &c, &["1 row is an exact copy"]);
        find(&rep, &c, &["identifies every row but 1 value that comes back"]);
    }

    #[test]
    fn sampled_file_scales_counts() {
        // 150 000 rows: a sample of 100 000 (2 000 000 cells / 3 columns past the cap); the key confirmed on every row.
        let mut s = String::from("id;cat;amount\n");
        for i in 0..150_000 { s += &format!("{i};{};{}\n", ["a", "b", "c"][i % 3], (i * 37) % 1000 + if i == 77_777 { 1_000_000 } else { 0 }); }
        let t = table(&s);
        let rep = insights(&t.st, &AtomicBool::new(false), None).unwrap();
        assert!(rep.sampled && rep.n == 100_000);
        let c = texts(&t, &rep);
        find(&rep, &c, &["id is filled and different on every row"]);
        assert!(rep.summary().starts_with("100\u{202f}000 rows of 150\u{202f}000 read"), "{}", rep.summary());
    }

    #[test]
    fn cancelled() {
        let t = table(&piege());
        assert!(insights(&t.st, &AtomicBool::new(true), None).is_none());
    }

    #[test]
    fn shapes() {
        assert_eq!(in_shape("Dupont", true), "\x01");
        assert_eq!(in_shape("DUPONT", true), "\x02");
        assert_eq!(in_shape("ABc", true), "\x02\x01");
        assert_eq!(in_shape("15/03/2024", true), "9/9/9");
        assert_eq!(in_shape("75011", false), "99999");
        assert_eq!(in_shape("a  b", true), "\x03 \x03");
        assert_eq!(in_dec("1 234,50"), Some(','));
        assert_eq!(in_dec("12.50"), Some('.'));
        assert_eq!(in_dec("1.234"), None);
        assert_eq!(in_dec("1.2345"), Some('.'));
    }
}
