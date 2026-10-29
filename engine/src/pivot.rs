//! Pivot table: the rows shown, one column's values down the side, another's
//! across the top (or none), and in each cell a count, sum, average, min, max
//! or number of distinct values — with totals, a colour scale, dates grouped
//! by year, quarter, month or day, and the rows behind any cell (`drill`).
//!
//! Parallel: each rayon job interns its labels and fills its own cells, the
//! jobs are merged; the totals are then made from the cells (the sets of
//! distinct values too, as unions), so a row is read once and added once.
//! Distinct values are kept as 64-bit hashes (8 bytes each, not the text).

use crate::dupes::js_space;
use crate::group::{coll_key, Interner, csv_bytes, fr_num, js_round, num_text, file_safe, Reader};
use crate::table::{RowId, State};
use crate::text::{parse_date, parse_number};
use crate::view::Kind;
use crate::group::{HashMap, HashSet, Hasher};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AO};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fun {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Distinct,
}

impl Fun {
    pub const ALL: [Fun; 6] = [Fun::Count, Fun::Sum, Fun::Avg, Fun::Min, Fun::Max, Fun::Distinct];

    pub fn label(self) -> &'static str {
        match self {
            Fun::Count => "Number of rows",
            Fun::Sum => "Sum",
            Fun::Avg => "Average",
            Fun::Min => "Smallest",
            Fun::Max => "Largest",
            Fun::Distinct => "Distinct values",
        }
    }

    fn numeric(self) -> bool {
        matches!(self, Fun::Sum | Fun::Avg | Fun::Min | Fun::Max)
    }
}

/// How a date column's values are grouped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dates {
    #[default]
    Month,
    Quarter,
    Year,
    Day,
    AsWritten,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotSpec {
    pub rows: usize,
    pub rows_by: Dates,
    /// The column across; None: a single column of totals.
    pub cols: Option<usize>,
    pub cols_by: Dates,
    pub fun: Fun,
    /// The column computed (every function but Count needs one).
    pub value: Option<usize>,
}

impl PivotSpec {
    /// The dialog's first choice: `col` (or a column of categories) down the
    /// side, a second column of few values across, the sum of an amount column
    /// (or the number of rows).
    pub fn guess(st: &State, ids: &[RowId], kinds: &[Kind], col: Option<usize>) -> PivotSpec {
        let headers = st.headers();
        let value = (0..st.ncols).find(|&c| Some(c) != col && crate::group::sum_by_default(st, &headers, kinds, c));
        let rows = col.unwrap_or_else(|| split_guess(st, ids));
        let mut tmp = Vec::new();
        let cols = (0..st.ncols).find(|&c| {
            if c == rows || Some(c) == value || kinds.get(c) == Some(&Kind::Number) { return false }
            let mut seen = std::collections::HashSet::new();
            for &id in ids.iter().take(3000) {
                seen.insert(st.with_cell(id, c, &mut tmp, |s| s.to_string()));
                if seen.len() > 12 { break }
            }
            (2..=12).contains(&seen.len())
        });
        PivotSpec { rows, rows_by: Dates::Month, cols, cols_by: Dates::Month, fun: if value.is_some() { Fun::Sum } else { Fun::Count }, value }
    }

    fn date(&self, c: usize, by: Dates, kinds: &[Kind]) -> Option<Dates> {
        (by != Dates::AsWritten && kinds.get(c) == Some(&Kind::Date)).then_some(by)
    }
}

/// A column of categories: the first with 2 to 50 values over the first rows.
pub fn split_guess(st: &State, ids: &[RowId]) -> usize {
    let mut tmp = Vec::new();
    (0..st.ncols).find(|&c| {
        let mut seen = std::collections::HashSet::new();
        for &id in ids.iter().take(5000) {
            seen.insert(st.with_cell(id, c, &mut tmp, |s| s.to_string()));
            if seen.len() > 50 { break }
        }
        (2..=50).contains(&seen.len())
    }).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PivotError {
    /// A function other than Count, and no value column.
    NoValue,
    Cancelled,
}

/// A row's label on a column: its value trimmed, or its date's bucket.
fn label_into(v: &str, date: Option<Dates>, out: &mut String) {
    out.clear();
    let v = v.trim_matches(js_space);
    let Some(by) = date.filter(|_| !v.is_empty()) else { out.push_str(v); return };
    let Some(k) = parse_date(v) else { out.push_str(v); return };
    let k = k as u64;
    let (y, m, d) = (k / 10_000_000_000, k / 100_000_000 % 100, k / 1_000_000 % 100);
    use std::fmt::Write;
    let _ = match by {
        Dates::Year => write!(out, "{y}"),
        Dates::Quarter => write!(out, "{y}-Q{}", m.div_ceil(3)),
        Dates::Month => write!(out, "{y}-{m:02}"),
        _ => write!(out, "{y}-{m:02}-{d:02}"),
    };
}

#[derive(Clone)]
struct Acc {
    n: u64,
    s: f64,
    mn: f64,
    mx: f64,
    /// Distinct values (counted from Local::seen at the end).
    dist: u64,
}

impl Default for Acc {
    fn default() -> Acc {
        Acc { n: 0, s: 0.0, mn: f64::INFINITY, mx: f64::NEG_INFINITY, dist: 0 }
    }
}

impl Acc {
    /// All but the distinct values, which merge through the sets.
    fn merge(&mut self, o: &Acc) {
        self.n += o.n;
        self.s += o.s;
        self.mn = self.mn.min(o.mn);
        self.mx = self.mx.max(o.mx);
    }

    fn value(&self, f: Fun) -> f64 {
        match f {
            Fun::Count => self.n as f64,
            Fun::Sum => self.s,
            Fun::Avg => self.s / self.n as f64,
            Fun::Min => self.mn,
            Fun::Max => self.mx,
            Fun::Distinct => self.dist as f64,
        }
    }
}

/// One job's cells. Labels are interned (one arena each for rows and columns).
#[derive(Default)]
struct Local {
    rows: Interner,
    cols: Interner,
    cells: HashMap<u64, Acc>,
    /// (row, column, value's hash): the distinct values of every cell.
    seen: HashSet<(u32, u32, u64)>,
    skipped: usize,
    dec: bool,
}

#[inline]
fn cell_key(r: u32, c: u32) -> u64 {
    (r as u64) << 32 | c as u64
}

impl Local {
    fn merge(mut self, mut o: Local, h: &Hasher) -> Local {
        if self.cells.len() < o.cells.len() { std::mem::swap(&mut self, &mut o) }
        let rmap: Vec<u32> = (0..o.rows.len()).map(|i| self.rows.id(h, o.rows.get(i), o.rows.first[i]).0).collect();
        let cmap: Vec<u32> = (0..o.cols.len()).map(|i| self.cols.id(h, o.cols.get(i), o.cols.first[i]).0).collect();
        for (k, a) in o.cells {
            self.cells.entry(cell_key(rmap[(k >> 32) as usize], cmap[k as u32 as usize])).or_default().merge(&a);
        }
        // Counted once at the end: a merge is a plain union.
        self.seen.extend(o.seen.into_iter().map(|(r, c, x)| (rmap[r as usize], cmap[c as usize], x)));
        self.skipped += o.skipped;
        self.dec |= o.dec;
        self
    }
}

/// How the rows are ordered: by label (A→Z), by their total or by column j
/// (largest first); `flip` the other way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PivotSort {
    pub by: SortBy,
    pub flip: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Label,
    Total,
    Col(usize),
}

/// Rows drawn by the dialog, and columns across (the largest totals beyond).
pub const SHOW_ROWS: usize = 500;
pub const SHOW_COLS: usize = 60;

pub struct Pivot {
    pub fun: Fun,
    pub across: bool,
    /// Row labels, in the order first met.
    pub row_keys: Vec<String>,
    /// Column labels, in label order ("" alone without a column across).
    pub col_keys: Vec<String>,
    /// (column, value), row after row (row r's: cells[starts[r]..starts[r + 1]]), by column.
    cells: Vec<(u32, f64)>,
    starts: Vec<u32>,
    pub row_totals: Vec<f64>,
    pub col_totals: Vec<f64>,
    /// The grand total; None when no value counted (an empty sum has none).
    pub total: Option<f64>,
    /// Values left out: not a number.
    pub skipped: usize,
    /// Some value had decimals (or an average): amounts shown with two.
    pub decimals: bool,
    /// Rows read.
    pub rows: usize,
    /// Per row: its label's rank (equal labels share one; empty last).
    rank: Vec<u32>,
    titles: [String; 3],
}

/// The pivot of `ids` (the rows shown). `progress` counts rows read.
pub fn pivot(st: &State, ids: &[RowId], spec: &PivotSpec, kinds: &[Kind], cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Result<Pivot, PivotError> {
    let fun = spec.fun;
    let need_v = fun != Fun::Count;
    let value = if need_v { Some(spec.value.ok_or(PivotError::NoValue)?) } else { None };
    let across = spec.cols.filter(|&c| c != spec.rows);
    let (rd, cd) = (spec.date(spec.rows, spec.rows_by, kinds), across.and_then(|c| spec.date(c, spec.cols_by, kinds)));
    let cols: Vec<usize> = [Some(spec.rows), across, value].into_iter().flatten().collect();
    let (ci, vi) = (across.map(|_| 1), value.map(|_| cols.len() - 1));
    let h = Hasher::default();
    let parts = crate::group::aggregate(
        ids,
        || Local { dec: fun == Fun::Avg, ..Default::default() },
        || (Reader::new(&cols), String::new(), String::new()),
        |(r, rk, _), id| { r.read(st, id); label_into(&r.v[0], rd, rk); h.hash_one(rk.as_bytes()) },
        |l, (r, rk, ck), pos, id| {
            r.read(st, id);
            let (mut x, mut hx) = (1.0, 0);
            if let Some(vi) = vi {
                let s = r.v[vi].trim_matches(js_space);
                if s.is_empty() { return }
                if fun.numeric() {
                    match parse_number(s) {
                        None => { l.skipped += 1; return }
                        Some(n) => { x = n; if n.fract() != 0.0 { l.dec = true } }
                    }
                } else {
                    hx = h.hash_one(s);
                }
            }
            label_into(&r.v[0], rd, rk);
            match ci { Some(ci) => label_into(&r.v[ci], cd, ck), None => ck.clear() }
            let r = l.rows.id(&h, rk.as_bytes(), pos).0;
            let c = l.cols.id(&h, ck.as_bytes(), pos).0;
            let a = l.cells.entry(cell_key(r, c)).or_default();
            a.n += 1;
            match fun {
                Fun::Count => {}
                Fun::Distinct => { l.seen.insert((r, c, hx)); }
                _ => { a.s += x; a.mn = a.mn.min(x); a.mx = a.mx.max(x); }
            }
        },
        |a, b| a.merge(b, &h),
        cancel,
        progress,
    );
    let Some(parts) = parts else { return Err(PivotError::Cancelled) };
    // The parts hold different row labels: their rows go end to end. Columns are shared.
    let mut clabels = Interner::default();
    let cmaps: Vec<Vec<u32>> = parts.iter().map(|p| (0..p.cols.len()).map(|i| clabels.id(&h, p.cols.get(i), p.cols.first[i]).0).collect()).collect();
    let nc = clabels.len();
    let (skipped, dec) = parts.iter().fold((0, fun == Fun::Avg), |(s, d), p| (s + p.skipped, d || p.dec));
    let offs: Vec<u32> = parts.iter().scan(0, |o, p| { let x = *o; *o += p.rows.len() as u32; Some(x) }).collect();
    let distinct = fun == Fun::Distinct;
    // Each part: its cells as (row, column, value), the totals made from them, and
    // its distinct (column, value) pairs for the column totals.
    let done: Vec<_> = parts.par_iter().zip(&cmaps).zip(&offs).map(|((p, cmap), &off)| {
        let mut rtot = vec![Acc::default(); p.rows.len()];
        let mut ctot = vec![Acc::default(); nc];
        let mut entries = Vec::with_capacity(p.cells.len());
        for (k, a) in &p.cells {
            let (r, c) = ((k >> 32) as usize, cmap[*k as u32 as usize]);
            if !distinct { entries.push((off + r as u32, c, a.value(fun))) }
            rtot[r].merge(a);
            ctot[c as usize].merge(a);
        }
        let mut pairs = Vec::new();
        if distinct {
            // Counted by sorting: a cell's values are a run of (row, column, …).
            let mut t: Vec<(u32, u32, u64)> = p.seen.iter().copied().collect();
            t.par_sort_unstable();
            for run in t.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
                entries.push((off + run[0].0, cmap[run[0].1 as usize], run.len() as f64));
            }
            let mut rv: Vec<(u32, u64)> = t.par_iter().map(|&(r, _, x)| (r, x)).collect();
            rv.par_sort_unstable();
            rv.dedup();
            rv.iter().for_each(|&(r, _)| rtot[r as usize].dist += 1);
            pairs = t.par_iter().map(|&(_, c, x)| (cmap[c as usize], x)).collect();
            pairs.par_sort_unstable();
            pairs.dedup();
        }
        (entries, rtot, ctot, pairs)
    }).collect();
    let (mut entries, mut rtot, mut ctot, mut pairs) = (Vec::new(), Vec::new(), vec![Acc::default(); nc], Vec::new());
    for (e, r, c, p) in done {
        entries.extend(e);
        rtot.extend(r);
        ctot.iter_mut().zip(&c).for_each(|(x, y)| x.merge(y));
        pairs.extend(p);
    }
    let mut all = Acc::default();
    ctot.iter().for_each(|c| all.merge(c));
    if distinct {
        pairs.par_sort_unstable();
        pairs.dedup();
        pairs.iter().for_each(|&(c, _)| ctot[c as usize].dist += 1);
        let mut xs: Vec<u64> = pairs.into_iter().map(|p| p.1).collect();
        xs.par_sort_unstable();
        xs.dedup();
        all.dist = xs.len() as u64;
    }
    let labels: Vec<&str> = parts.iter().flat_map(|p| (0..p.rows.len()).map(|i| p.rows.str(i))).collect();
    let first: Vec<u32> = parts.iter().flat_map(|p| p.rows.first.iter().copied()).collect();
    // Rows in the order first met; columns in label order.
    let mut rperm: Vec<u32> = (0..labels.len() as u32).collect();
    rperm.par_sort_unstable_by_key(|&i| first[i as usize]);
    let cnames: Vec<&str> = (0..nc).map(|i| clabels.str(i)).collect();
    let (cperm, _) = label_order(&cnames, &clabels.first);
    let (rnew, cnew) = (inverse(&rperm), inverse(&cperm));
    entries.par_iter_mut().for_each(|e| { e.0 = rnew[e.0 as usize]; e.1 = cnew[e.1 as usize]; });
    entries.par_sort_unstable_by_key(|e| (e.0, e.1));
    let nr = rperm.len();
    let mut starts = vec![0u32; nr + 1];
    for e in &entries { starts[e.0 as usize + 1] += 1 }
    for k in 0..nr { starts[k + 1] += starts[k] }
    let cells: Vec<(u32, f64)> = entries.into_par_iter().map(|e| (e.1, e.2)).collect();
    let row_keys: Vec<String> = rperm.par_iter().map(|&i| labels[i as usize].to_string()).collect();
    let col_keys: Vec<String> = cperm.iter().map(|&i| cnames[i as usize].to_string()).collect();
    drop(labels);
    parts.into_par_iter().for_each(drop);
    let (_, rank) = label_order(&row_keys, &(0..nr as u32).collect::<Vec<_>>());
    let headers = st.headers();
    let title = |c: Option<usize>| c.and_then(|c| headers.get(c).cloned()).unwrap_or_default();
    Ok(Pivot {
        fun,
        across: across.is_some(),
        row_keys,
        col_keys,
        cells,
        starts,
        row_totals: rperm.iter().map(|&i| rtot[i as usize].value(fun)).collect(),
        col_totals: cperm.iter().map(|&i| ctot[i as usize].value(fun)).collect(),
        total: (all.n > 0 || matches!(fun, Fun::Count | Fun::Distinct)).then(|| all.value(fun)),
        skipped,
        decimals: dec,
        rows: ids.len(),
        rank,
        titles: [title(Some(spec.rows)), title(across), title(value)],
    })
}

/// Labels in the web app's order — empty last, then the collator, then first
/// met — and each one's rank (labels the collator calls equal share one).
fn label_order<S: AsRef<str> + Sync>(names: &[S], first: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let keys: Vec<Vec<u8>> = names.par_iter().map(|s| {
        let s = s.as_ref();
        let mut k = vec![s.is_empty() as u8];
        k.extend(coll_key(s));
        k
    }).collect();
    let mut perm: Vec<u32> = (0..names.len() as u32).collect();
    perm.par_sort_unstable_by(|&a, &b| keys[a as usize].cmp(&keys[b as usize]).then(first[a as usize].cmp(&first[b as usize])));
    let mut rank = vec![0u32; names.len()];
    let mut r = 0;
    for k in 1..perm.len() {
        if keys[perm[k - 1] as usize] != keys[perm[k] as usize] { r += 1 }
        rank[perm[k] as usize] = r;
    }
    (perm, rank)
}

fn inverse(perm: &[u32]) -> Vec<u32> {
    let mut inv = vec![0u32; perm.len()];
    for (k, &i) in perm.iter().enumerate() { inv[i as usize] = k as u32 }
    inv
}

impl Pivot {
    /// Row r's value in column c.
    pub fn value(&self, r: usize, c: usize) -> Option<f64> {
        let l = &self.cells[self.starts[r] as usize..self.starts[r + 1] as usize];
        l.binary_search_by_key(&(c as u32), |e| e.0).ok().map(|k| l[k].1)
    }

    /// The rows in the order asked. Values: largest first, rows without one
    /// last whichever the direction, ties by label.
    pub fn order(&self, sort: PivotSort) -> Vec<u32> {
        let mut rows: Vec<u32> = (0..self.row_keys.len() as u32).collect();
        let rank = |r: u32| self.rank[r as usize];
        match sort.by {
            SortBy::Label => {
                if sort.flip { rows.par_sort_by_key(|&r| std::cmp::Reverse(rank(r))) } else { rows.par_sort_by_key(|&r| rank(r)) }
            }
            by => {
                let val = |r: u32| match by { SortBy::Col(j) => self.value(r as usize, j), _ => Some(self.row_totals[r as usize]) };
                rows.par_sort_by(|&a, &b| {
                    let (x, y) = (val(a), val(b));
                    x.is_none().cmp(&y.is_none())
                        .then_with(|| match (x, y) {
                            (Some(x), Some(y)) => if sort.flip { x.total_cmp(&y) } else { y.total_cmp(&x) },
                            _ => Ordering::Equal,
                        })
                        .then(rank(a).cmp(&rank(b)))
                });
            }
        }
        rows
    }

    /// The columns drawn: all, or the `max` with the largest totals, kept in their order.
    pub fn shown_cols(&self, max: usize) -> Vec<usize> {
        let n = self.col_keys.len();
        if n <= max { return (0..n).collect() }
        let mut by: Vec<usize> = (0..n).collect();
        by.sort_by(|&a, &b| self.col_totals[b].total_cmp(&self.col_totals[a]));
        let mut keep = by[..max].to_vec();
        keep.sort_unstable();
        keep
    }

    /// The colour scale's bounds: the smallest and largest numbers drawn.
    pub fn heat_range(&self, rows: &[u32], cols: &[usize]) -> Option<(f64, f64)> {
        let mut lo_hi: Option<(f64, f64)> = None;
        for &r in rows {
            for &c in cols {
                if let Some(x) = self.value(r as usize, c) {
                    lo_hi = Some(lo_hi.map_or((x, x), |(lo, hi)| (lo.min(x), hi.max(x))));
                }
            }
        }
        lo_hi
    }

    /// A number as drawn: two decimals for amounts with decimals, French style.
    pub fn display(&self, x: f64) -> String {
        if self.decimals && !matches!(self.fun, Fun::Count | Fun::Distinct) { fr_num(x, 2, 2) } else { fr_num(js_round(x, 1e6), 0, 3) }
    }

    /// The whole table as text: the header, every row in `order`, the total line.
    pub fn matrix(&self, order: &[u32], comma: bool) -> Vec<Vec<String>> {
        let cell = |x: Option<f64>| x.map_or(String::new(), |x| num_text(if self.fun == Fun::Avg { js_round(x, 100.0) } else { js_round(x, 1e6) }, comma));
        let label = |k: &str| if k.is_empty() { "(empty)".to_string() } else { k.to_string() };
        let mut head = vec![self.titles[0].clone()];
        if self.across { head.extend(self.col_keys.iter().map(|k| label(k))) }
        head.push(if self.across { "Total".into() } else if self.fun == Fun::Count { self.fun.label().into() } else { format!("{} of {}", self.fun.label(), self.titles[2]) });
        let mut out = vec![head];
        for &r in order {
            let r = r as usize;
            let mut line = vec![label(&self.row_keys[r])];
            if self.across { line.extend((0..self.col_keys.len()).map(|c| cell(self.value(r, c)))) }
            line.push(cell(Some(self.row_totals[r])));
            out.push(line);
        }
        let mut tot = vec!["Total".to_string()];
        if self.across { tot.extend(self.col_totals.iter().map(|&x| cell(Some(x)))) }
        tot.push(cell(self.total));
        out.push(tot);
        out
    }

    /// Tab-separated, to paste into a spreadsheet (decimal comma if `comma`).
    pub fn to_tsv(&self, order: &[u32], comma: bool) -> String {
        let quote = |v: &String| if v.contains(['\t', '\n', '"']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.clone() };
        self.matrix(order, comma).iter().map(|r| r.iter().map(quote).collect::<Vec<_>>().join("\t")).collect::<Vec<_>>().join("\n")
    }

    /// The table as a CSV file's bytes, for a new tab.
    pub fn to_csv(&self, order: &[u32], delim: u8) -> Vec<u8> {
        csv_bytes(self.matrix(order, delim == b';').into_iter(), delim)
    }

    /// The new tab's name: "<stem> — pivot <rows> × <cols>.csv".
    pub fn tab_name(&self, stem: &str) -> String {
        let s = format!("{stem} — pivot {}{}.csv", self.titles[0], if self.across { format!(" × {}", self.titles[1]) } else { String::new() });
        file_safe(&s)
    }
}

/// The rows behind a cell: those whose label is `row` (None: any) and `col`
/// (None: any), with a value when the function needs one — a value that is
/// not a number included, as in the web app. In the order of `ids`.
pub fn drill(st: &State, ids: &[RowId], spec: &PivotSpec, kinds: &[Kind], row: Option<&str>, col: Option<&str>, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    let across = spec.cols.filter(|&c| c != spec.rows);
    let value = spec.value.filter(|_| spec.fun != Fun::Count);
    let (rd, cd) = (spec.date(spec.rows, spec.rows_by, kinds), across.and_then(|c| spec.date(c, spec.cols_by, kinds)));
    let cols: Vec<usize> = [Some(spec.rows), across, value].into_iter().flatten().collect();
    let (ci, vi) = (across.map(|_| 1), value.map(|_| cols.len() - 1));
    let out: Vec<RowId> = ids
        .par_iter()
        .with_min_len(4096)
        .map_init(|| (Reader::new(&cols), String::new()), |(rd_, buf), &id| {
            if cancel.load(AO::Relaxed) { return None }
            rd_.read(st, id);
            if let Some(vi) = vi { if rd_.v[vi].trim_matches(js_space).is_empty() { return None } }
            if let Some(want) = row { label_into(&rd_.v[0], rd, buf); if buf != want { return None } }
            if let Some(want) = col {
                match ci { Some(ci) => label_into(&rd_.v[ci], cd, buf), None => buf.clear() }
                if buf != want { return None }
            }
            Some(id)
        })
        .flatten()
        .collect();
    (!cancel.load(AO::Relaxed)).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use crate::view::detect_kinds;
    use std::sync::Arc;

    const VENTES: &str = "id;date;ville;statut;montant\r\n1;15/01/2024;Paris;vip;10,50\r\n2;20/01/2024;Paris;actif;5\r\n3;03/02/2024;Lyon;vip;2\r\n4;04/02/2024;Lyon;vip;abc\r\n5;05/04/2024;;actif;1\r\n";

    fn setup() -> (Table, Vec<RowId>, Vec<Kind>) {
        let t = Table::new(Arc::new(Base::from_bytes(VENTES.as_bytes().to_vec(), OpenOptions::default())), None);
        let ids = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let kinds = detect_kinds(&t.st);
        (t, ids, kinds)
    }

    fn spec(r: usize, c: Option<usize>, fun: Fun, v: usize, rd: Dates) -> PivotSpec {
        PivotSpec { rows: r, rows_by: rd, cols: c, cols_by: Dates::Month, fun, value: Some(v) }
    }

    fn matrix(s: &PivotSpec, sort: PivotSort) -> (Vec<Vec<String>>, Pivot) {
        let (t, ids, kinds) = setup();
        let p = pivot(&t.st, &ids, s, &kinds, &AtomicBool::new(false), None).unwrap();
        (p.matrix(&p.order(sort), false), p)
    }

    fn strs(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter().map(|r| r.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn sum_by_city_and_status() {
        let (m, p) = matrix(&spec(2, Some(3), Fun::Sum, 4, Dates::Month), PivotSort::default());
        assert_eq!(m, strs(&[&["ville", "actif", "vip", "Total"], &["Lyon", "", "2", "2"], &["Paris", "5", "10.5", "15.5"], &["(empty)", "1", "", "1"], &["Total", "6", "12.5", "18.5"]]));
        assert_eq!(p.skipped, 1);   // "abc" is not a number
        assert_eq!(p.display(10.5), "10,50");   // two decimals on screen
        assert_eq!(p.to_tsv(&p.order(PivotSort::default()), true).lines().nth(2), Some("Paris\t5\t10,5\t15,5"));
    }

    #[test]
    fn count_average_distinct() {
        assert_eq!(matrix(&spec(2, Some(3), Fun::Count, 4, Dates::Month), PivotSort::default()).0[1], vec!["Lyon", "", "2", "2"]);   // rows, numbers or not
        assert_eq!(matrix(&spec(2, None, Fun::Avg, 4, Dates::Month), PivotSort::default()).0[2], vec!["Paris", "7.75"]);
        assert_eq!(matrix(&spec(3, None, Fun::Distinct, 2, Dates::Month), PivotSort::default()).0,
            strs(&[&["statut", "Distinct values of ville"], &["actif", "1"], &["vip", "2"], &["Total", "2"]]));
    }

    #[test]
    fn dates_and_sort() {
        let first = |m: Vec<Vec<String>>| m.into_iter().map(|r| r[0].clone()).collect::<Vec<_>>();
        assert_eq!(first(matrix(&spec(1, None, Fun::Count, 4, Dates::Month), PivotSort::default()).0), vec!["date", "2024-01", "2024-02", "2024-04", "Total"]);
        assert_eq!(first(matrix(&spec(1, None, Fun::Count, 4, Dates::Quarter), PivotSort::default()).0), vec!["date", "2024-Q1", "2024-Q2", "Total"]);
        let by_total = PivotSort { by: SortBy::Total, flip: false };
        assert_eq!(first(matrix(&spec(2, None, Fun::Sum, 4, Dates::Month), by_total).0), vec!["ville", "Paris", "Lyon", "(empty)", "Total"]);
        let flipped = PivotSort { by: SortBy::Label, flip: true };
        assert_eq!(first(matrix(&spec(2, None, Fun::Sum, 4, Dates::Month), flipped).0), vec!["ville", "(empty)", "Paris", "Lyon", "Total"]);
        let by_vip = PivotSort { by: SortBy::Col(1), flip: true };
        assert_eq!(first(matrix(&spec(2, Some(3), Fun::Sum, 4, Dates::Month), by_vip).0), vec!["ville", "Lyon", "Paris", "(empty)", "Total"]);
    }

    #[test]
    fn drill_and_new_tab() {
        let (t, ids, kinds) = setup();
        let s = spec(2, Some(3), Fun::Sum, 4, Dates::Month);
        let no = AtomicBool::new(false);
        let got = drill(&t.st, &ids, &s, &kinds, Some("Paris"), Some("vip"), &no).unwrap();
        assert_eq!(got.iter().map(|&id| t.st.cell(id, 0)).collect::<Vec<_>>(), vec!["1"]);
        assert_eq!(drill(&t.st, &ids, &s, &kinds, Some("Lyon"), None, &no).unwrap().len(), 2);   // "abc" counted among its rows
        let p = pivot(&t.st, &ids, &s, &kinds, &no, None).unwrap();
        assert_eq!(p.tab_name("ventes"), "ventes — pivot ville × statut.csv");
        let csv = String::from_utf8(p.to_csv(&p.order(PivotSort::default()), b';')).unwrap();
        assert_eq!(csv.lines().count(), 5);
        assert!(csv.starts_with("ville;actif;vip;Total\n"));
        assert_eq!(p.heat_range(&p.order(PivotSort::default()), &p.shown_cols(SHOW_COLS)), Some((1.0, 10.5)));
        assert_eq!(pivot(&t.st, &ids, &PivotSpec { value: None, ..s }, &kinds, &no, None).err(), Some(PivotError::NoValue));
    }

    #[test]
    fn guesses() {
        let (t, ids, kinds) = setup();
        let g = PivotSpec::guess(&t.st, &ids, &kinds, None);
        assert_eq!((g.rows, g.cols), (0, Some(1)));   // five ids are "2 to 50 values", as in the web app
        let g = PivotSpec::guess(&t.st, &ids, &kinds, Some(2));
        assert_eq!((g.rows, g.cols), (2, Some(1)));
        assert_eq!(g.fun, Fun::Count);   // "montant" reads as text here: "abc" among five values
    }
}
