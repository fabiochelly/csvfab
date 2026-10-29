//! The column charts under the titles: numbers and dates as a curve of how
//! they spread (bins, range, how many cells are filled), text as a ribbon of
//! its most frequent values ending with the empty cells. The whole file's
//! picture is made once per state; the rows a filter lets through are counted
//! into the same bins and values, so the two draw one over the other.
//!
//! As the JS did, a big file is read from an even sample (every k-th record,
//! by record, not by place: a sort gives the same rows, so the same picture);
//! `Sample::All` reads every row instead. Then a part of a chart — bars, a
//! value, the filled or the empty cells — maps back to the rows it stands for
//! (`Keep`) and to column filter texts (`view::FilterSpec::cols`).

use crate::stats::{cell_kind, chunked, Best, fmt_int, hash, quantiles, visit_cols, Counter};
use crate::table::{RowId, State};
use crate::text::{parse_date, parse_number};
use crate::view::Kind;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// Rows read at most: every row of a smaller file (a 32-bar picture needs no more).
pub const ROWS: usize = 10_000;
/// Rows × columns read at most (a file of 1 000 columns: 1 500 rows).
pub const CELLS: usize = 1_500_000;
/// Bars of a curve (an integer column of a few values: one each).
pub const BINS: usize = 32;
/// Text values given a colour of their own.
pub const SEGS: usize = 5;
/// Distinct text values counted per column; later new ones count as "others".
pub const DISTINCT: usize = 4000;
/// Pixels a ribbon part (its 2 px gap included) or a gauge part takes at least: enough to hover it.
pub const SEG_MIN: f64 = 6.0;

/// How many rows a pass reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sample {
    /// As the JS: at most ROWS, and CELLS over every column.
    #[default]
    Js,
    /// Every row.
    All,
    /// At most this many.
    Rows(usize),
}

impl Sample {
    fn size(self, ncols: usize) -> usize {
        match self {
            Sample::Js => 500.max(ROWS.min(CELLS / ncols.max(1))),
            Sample::All => usize::MAX,
            Sample::Rows(n) => n.max(1),
        }
    }
}

// ---------------------------------------------------------------------------
// Dates on a time line

/// A date on a time line, in seconds since 1970: YYYYMMDDhhmmss would leave
/// gaps between the months.
pub fn time_key(s: &str) -> Option<f64> {
    let k = parse_date(s)? as i64;
    let p = |n: i64| (k / n) % 100;
    // Days past the month's end roll over, as Date.UTC did.
    let days = days_from_civil(k / 10_000_000_000, p(100_000_000), 1) + p(1_000_000) - 1;
    Some((days * 86_400 + p(10_000) * 3600 + p(100) * 60 + k % 100) as f64)
}

/// Seconds since 1970 as the filters read a date: `2024-03-15 10:20:30`.
pub fn time_text(t: f64) -> String {
    let t = t.floor() as i64;
    let (days, s) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

// Howard Hinnant's algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// A number as the column filters read it back (no exponent unless needed:
/// the filters read 40 characters at most).
fn num_text(x: f64) -> String {
    let s = format!("{x}");
    if s.len() <= 30 { s } else { format!("{x:e}") }
}

// ---------------------------------------------------------------------------
// The pictures

/// How a curve's keys fall into its bars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bins {
    pub kind: Kind,
    pub n: usize,
    pub lo: f64,
    pub w: f64,
}

impl Bins {
    /// The key a value bins on: a number, or a date in seconds.
    #[inline]
    pub fn key(&self, s: &str) -> Option<f64> {
        match self.kind {
            Kind::Number => parse_number(s),
            _ => time_key(s),
        }
    }

    /// The bar of key x (values past the ends go to the end bars).
    #[inline]
    pub fn of(&self, x: f64) -> usize {
        ((x - self.lo) / self.w).floor().clamp(0.0, (self.n - 1) as f64) as usize
    }

    /// Where bar b starts on the key line.
    pub fn edge(&self, b: usize) -> f64 {
        self.lo + b as f64 * self.w
    }

    /// Bar b's lower bound as a filter writes it (a date's whole second: its key is in seconds).
    fn edge_text(&self, b: usize) -> String {
        match self.kind {
            Kind::Number => num_text(self.edge(b)),
            _ => time_text(self.edge(b).ceil()),
        }
    }
}

/// A number or date column's picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub bins: Bins,
    /// Filled cells read, of any kind.
    pub fill: usize,
    /// Filled cells not of the column's kind.
    pub bad: usize,
    /// Cells binned.
    pub typed: usize,
    pub cnt: Vec<u32>,
    /// Each bar's smallest and largest values as written.
    pub first: Vec<Option<String>>,
    pub last: Vec<Option<String>>,
}

/// A text column's picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Ribbon {
    pub fill: usize,
    /// The SEGS most frequent values held twice or more, whatever their share
    /// (the colours always come in the same order), with their counts.
    pub segs: Vec<(String, u32)>,
    /// Filled cells holding another value.
    pub other: usize,
    pub distinct: usize,
    /// More than DISTINCT values: `distinct` is a floor.
    pub many: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Col {
    /// Nothing to draw (no value of the column's kind): only how full it is.
    Empty { fill: usize },
    Curve(Curve),
    Ribbon(Ribbon),
}

impl Col {
    pub fn fill(&self) -> usize {
        match self {
            Col::Empty { fill } => *fill,
            Col::Curve(c) => c.fill,
            Col::Ribbon(r) => r.fill,
        }
    }
}

/// The whole file's pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct Spark {
    /// Rows in the file.
    pub n: usize,
    /// Rows read.
    pub s: usize,
    pub sampled: bool,
    pub cols: Vec<Col>,
}

/// A column's counts over the rows shown, in its whole-file picture's bins or values.
#[derive(Clone, Debug, PartialEq)]
pub enum ColF {
    Empty { fill: usize },
    Curve { cnt: Vec<u32>, fill: usize, bad: usize },
    Ribbon { seg: Vec<u32>, other: usize, fill: usize },
}

impl ColF {
    pub fn fill(&self) -> usize {
        match self {
            ColF::Empty { fill } | ColF::Curve { fill, .. } | ColF::Ribbon { fill, .. } => *fill,
        }
    }
}

/// The rows shown's pictures.
#[derive(Clone, Debug, PartialEq)]
pub struct SparkF {
    /// Rows shown.
    pub n: usize,
    /// Rows read.
    pub s: usize,
    pub cols: Vec<ColF>,
}

impl SparkF {
    /// Rows shown that `k` rows read stand for.
    pub fn estimate(&self, k: usize) -> f64 {
        (k as f64 * self.n as f64 / self.s.max(1) as f64).round()
    }
}

/// Every row of a smaller file; else every k-th record of the file, whatever
/// the order shown (rows added here, with no record, by their place) — in
/// record order.
fn base_rows(st: &State, size: usize) -> Vec<RowId> {
    let n = st.len();
    let k = if n <= size { 1 } else { n.div_ceil(size) };
    let (h, nb) = (st.header as u32, st.base.n as u32);
    let mut out: Vec<RowId> = (0..n)
        .into_par_iter()
        .with_min_len(1 << 16)
        .filter_map(|p| {
            let id = st.data_id(p);
            let r = if id < nb { id.saturating_sub(h) as usize } else { p };
            (r % k == 0).then_some(id)
        })
        .collect();
    if !out.windows(2).all(|w| w[0] <= w[1]) {
        out.par_sort_unstable();
    }
    out
}

/// One accumulator over all of `0..n` (sequential), or one per chunk in parallel.
fn run<A: Send>(n: usize, par: bool, cancel: &AtomicBool, init: impl Fn() -> A + Sync + Send, step: impl Fn(&mut A, usize) + Sync + Send) -> Option<Vec<A>> {
    if par {
        return chunked(n, cancel, init, step);
    }
    let mut a = init();
    for p in 0..n {
        if p & 1023 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        step(&mut a, p);
    }
    Some(vec![a])
}

enum Acc {
    Num { fill: usize, bad: usize, keys: Vec<(f64, u32)>, frac: bool, tmp: Vec<u8> },
    Text { fill: usize, c: Counter, over: usize, tmp: Vec<u8> },
}

fn base_col(st: &State, rows: &[RowId], col: usize, kind: Kind, par: bool, cancel: &AtomicBool) -> Option<Col> {
    let init = || match kind {
        Kind::Text => Acc::Text { fill: 0, c: Counter::default(), over: 0, tmp: Vec::new() },
        _ => Acc::Num { fill: 0, bad: 0, keys: Vec::new(), frac: false, tmp: Vec::new() },
    };
    let bins = Bins { kind, n: 1, lo: 0.0, w: 1.0 };
    let parts = run(rows.len(), par, cancel, init, |a, p| match a {
        Acc::Num { fill, bad, keys, frac, tmp } => st.with_cell(rows[p], col, tmp, |v| {
            let Some(ty) = cell_kind(v) else { return };
            *fill += 1;
            if ty != kind {
                *bad += 1;
                return;
            }
            match bins.key(v.trim()) {
                None => *bad += 1,
                Some(x) => {
                    keys.push((x, p as u32));
                    *frac |= x.fract() != 0.0;
                }
            }
        }),
        Acc::Text { fill, c, over, tmp } => st.with_cell(rows[p], col, tmp, |v| {
            if cell_kind(v).is_none() {
                return;
            }
            *fill += 1;
            if !c.add(v.as_bytes(), hash(v.as_bytes()), 1, p as u32, DISTINCT) {
                *over += 1;
            }
        }),
    })?;
    // In order, so the values counted are the first DISTINCT met, as one pass counts them.
    let mut it = parts.into_iter();
    let mut acc = it.next()?;
    for b in it {
        match (&mut acc, b) {
            (Acc::Num { fill, bad, keys, frac, .. }, Acc::Num { fill: f2, bad: b2, keys: k2, frac: r2, .. }) => {
                *fill += f2;
                *bad += b2;
                keys.extend(k2);
                *frac |= r2;
            }
            (Acc::Text { fill, c, over, .. }, Acc::Text { fill: f2, c: c2, over: o2, .. }) => {
                *fill += f2;
                *over += o2;
                for e in &c2.ents {
                    if !c.add(c2.bytes(e), e.hash, e.count, e.first, DISTINCT) {
                        *over += e.count as usize;
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    Some(match acc {
        Acc::Text { fill, c, over, .. } => {
            if fill == 0 {
                return Some(Col::Empty { fill });
            }
            let mut all: Vec<usize> = (0..c.ents.len()).collect();
            all.sort_by(|&a, &b| c.ents[b].count.cmp(&c.ents[a].count));   // stable: ties in order of appearance
            let segs: Vec<(String, u32)> = all.iter().take(SEGS).map(|&i| &c.ents[i]).filter(|e| e.count >= 2).map(|e| (c.str(e).to_string(), e.count)).collect();
            let other = fill - segs.iter().map(|s| s.1 as usize).sum::<usize>();
            Col::Ribbon(Ribbon { fill, segs, other, distinct: all.len() + (over > 0) as usize, many: over > 0 })
        }
        Acc::Num { fill, bad, keys, frac, .. } => {
            if keys.is_empty() {
                return Some(Col::Empty { fill });
            }
            let mut ks: Vec<f64> = keys.iter().map(|k| k.0).collect();
            let (mut lo, mut hi) = ks.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| (a.min(x), b.max(x)));
            // Min to max, unless a few outliers would squeeze every other value
            // into one bar — then the 1st to 99th percentile, the end bars taking the outliers.
            let (p1, p99) = quantiles(&mut ks, 0.01, 0.99);
            if p99 > p1 && hi - lo > 4.0 * (p99 - p1) {
                (lo, hi) = (p1, p99);
            }
            let bins = if kind == Kind::Number && !frac && hi - lo < 40.0 {
                Bins { kind, n: (hi - lo) as usize + 1, lo: lo - 0.5, w: 1.0 }   // a few integers: one bar each
            } else if hi == lo {
                Bins { kind, n: 1, lo: lo - 0.5, w: 1.0 }
            } else {
                Bins { kind, n: BINS, lo, w: (hi - lo) / BINS as f64 }
            };
            let mut cnt = vec![0u32; bins.n];
            let mut ends = vec![(Best::NONE, Best::NONE); bins.n];
            for &(x, p) in &keys {
                let b = bins.of(x);
                cnt[b] += 1;
                ends[b].0.lower(x, p);
                ends[b].1.higher(x, p);
            }
            let mut tmp = Vec::new();
            let mut text = |b: Best| (b.pos != u32::MAX).then(|| st.with_cell(rows[b.pos as usize], col, &mut tmp, |v| v.trim().to_string()));
            let (first, last) = ends.into_iter().map(|(a, b)| (text(a), text(b))).unzip();
            Col::Curve(Curve { bins, fill, bad, typed: keys.len(), cnt, first, last })
        }
    })
}

/// The whole file's pictures (`kinds`: each column's, as `view::detect_kinds`
/// says). None if cancelled.
pub fn spark_base(st: &State, kinds: &[Kind], sample: Sample, cancel: &AtomicBool) -> Option<Spark> {
    let nc = st.ncols;
    let rows = base_rows(st, sample.size(nc));
    let kind = |c: usize| kinds.get(c).copied().unwrap_or(Kind::Text);
    // A sample of a few thousand rows: a column per thread. Every row of a big
    // file: a column at a time, its rows in parallel — one column's keys held at once.
    let cols: Option<Vec<Col>> = if rows.len() > 1 << 16 {
        (0..nc).map(|c| base_col(st, &rows, c, kind(c), true, cancel)).collect()
    } else {
        (0..nc).into_par_iter().map(|c| base_col(st, &rows, c, kind(c), false, cancel)).collect()
    };
    let n = st.len();
    Some(Spark { n, s: rows.len(), sampled: n > rows.len(), cols: cols? })
}

/// The rows shown (`ids`, a filter's result) counted into the whole file's
/// bins and values: one pass, every column at once. None if cancelled.
pub fn spark_filtered(st: &State, base: &Spark, ids: &[RowId], sample: Sample, cancel: &AtomicBool) -> Option<SparkF> {
    let nc = base.cols.len().min(st.ncols);
    let n = ids.len();
    let k = if n <= sample.size(st.ncols) { 1 } else { n.div_ceil(sample.size(st.ncols)) };
    let s = n.div_ceil(k);
    let blank: Vec<ColF> = base.cols[..nc]
        .iter()
        .map(|c| match c {
            Col::Empty { .. } => ColF::Empty { fill: 0 },
            Col::Curve(c) => ColF::Curve { cnt: vec![0; c.bins.n], fill: 0, bad: 0 },
            Col::Ribbon(r) => ColF::Ribbon { seg: vec![0; r.segs.len()], other: 0, fill: 0 },
        })
        .collect();
    let cols: Vec<usize> = (0..nc).collect();
    let parts = run(s, s > 4096, cancel, || (blank.clone(), Vec::new()), |(acc, tmp), p| {
        visit_cols(st, ids[p * k], &cols, tmp, |c, v| match (&mut acc[c], &base.cols[c]) {
            (ColF::Empty { fill }, _) => *fill += cell_kind(v).is_some() as usize,
            (ColF::Curve { cnt, fill, bad }, Col::Curve(b)) => {
                let Some(ty) = cell_kind(v) else { return };
                *fill += 1;
                match b.bins.key(v.trim()) {
                    Some(x) if ty == b.bins.kind => cnt[b.bins.of(x)] += 1,
                    _ => *bad += 1,
                }
            }
            (ColF::Ribbon { seg, other, fill }, Col::Ribbon(r)) => {
                if cell_kind(v).is_none() {
                    return;
                }
                *fill += 1;
                match r.segs.iter().position(|x| x.0 == v) {
                    Some(i) => seg[i] += 1,
                    None => *other += 1,
                }
            }
            _ => unreachable!(),
        });
    })?;
    let mut it = parts.into_iter().map(|p| p.0);
    let mut acc = it.next().unwrap_or(blank);
    for b in it {
        for (x, y) in acc.iter_mut().zip(b) {
            match (x, y) {
                (ColF::Empty { fill }, ColF::Empty { fill: f }) => *fill += f,
                (ColF::Curve { cnt, fill, bad }, ColF::Curve { cnt: c, fill: f, bad: d }) => {
                    *fill += f;
                    *bad += d;
                    cnt.iter_mut().zip(c).for_each(|(a, b)| *a += b);
                }
                (ColF::Ribbon { seg, other, fill }, ColF::Ribbon { seg: g, other: o, fill: f }) => {
                    *fill += f;
                    *other += o;
                    seg.iter_mut().zip(g).for_each(|(a, b)| *a += b);
                }
                _ => unreachable!(),
            }
        }
    }
    Some(SparkF { n, s, cols: acc })
}

// ---------------------------------------------------------------------------
// Drawing data

/// Bar heights in % of the tallest; a bar holding anything is never under
/// 7 %, so a lone outlier still shows.
pub fn heights(cnt: &[u32]) -> Vec<f64> {
    let max = cnt.iter().copied().max().unwrap_or(0) as f64;
    cnt.iter().map(|&n| if n > 0 { (n as f64 / max * 100.0).max(7.0) } else { 0.0 }).collect()
}

/// Whether counts are drawn as a smooth area (a continuous column) rather
/// than bars (a few values: one bar each).
pub fn smooth(cnt: &[u32]) -> bool {
    cnt.len() == BINS
}

/// The counts as a smooth area: a monotone cubic (Fritsch–Carlson: it never
/// rises above a bar nor dips under zero between two, so a peak or a gap
/// stays where it is) through the middle of every bar. In bars along x (0 to
/// B) and % of the height along y (0 at the bottom): the line starts at
/// (0, y0), runs flat to (0.5, y0), then one cubic per pair of bars
/// `[c1x, c1y, c2x, c2y, x, y]`, then flat to (B, its last y).
pub fn area(cnt: &[u32]) -> (f64, Vec<[f64; 6]>) {
    let y = heights(cnt);
    let b = y.len();
    if b == 0 {
        return (0.0, Vec::new());
    }
    let d: Vec<f64> = y.windows(2).map(|w| w[1] - w[0]).collect();
    let mut t: Vec<f64> = (0..b)
        .map(|k| {
            if k == 0 { d.first().copied().unwrap_or(0.0) }
            else if k == b - 1 { d[b - 2] }
            else if d[k - 1] * d[k] <= 0.0 { 0.0 }
            else { (d[k - 1] + d[k]) / 2.0 }
        })
        .collect();
    for k in 0..b.saturating_sub(1) {
        if d[k] == 0.0 {
            t[k] = 0.0;
            t[k + 1] = 0.0;
            continue;
        }
        let (a, c) = (t[k] / d[k], t[k + 1] / d[k]);
        let h = a * a + c * c;
        if h > 9.0 {
            let r = 3.0 / h.sqrt();
            t[k] = r * a * d[k];
            t[k + 1] = r * c * d[k];
        }
    }
    let segs = (0..b - 1)
        .map(|k| {
            let x = k as f64;
            [x + 0.5 + 1.0 / 3.0, y[k] + t[k] / 3.0, x + 1.5 - 1.0 / 3.0, y[k + 1] - t[k + 1] / 3.0, x + 1.5, y[k + 1]]
        })
        .collect();
    (y[0], segs)
}

/// A part of a ribbon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Seg(usize),
    Other,
    Empty,
}

/// Where each part of a ribbon `w` px wide ends: its share of the rows of
/// what is left once every part has SEG_MIN px (a value held by 0.4 % of a
/// mostly empty column would be under a px). The same ends map the pointer
/// to a part (`ribbon_at`).
pub fn ribbon_ends(parts: &[(Part, usize)], w: f64) -> Vec<f64> {
    let n = parts.len();
    let tot: usize = parts.iter().map(|p| p.1).sum();
    let mut cum = 0.0;
    parts
        .iter()
        .enumerate()
        .map(|(k, p)| {
            cum += p.1 as f64 / tot.max(1) as f64;
            if k == n - 1 { w } else { cum * w + (k as f64 + 1.0 - cum * n as f64) * SEG_MIN }
        })
        .collect()
}

/// The part of a ribbon under x px: (its index, where it starts, its width), in px.
pub fn ribbon_at(parts: &[(Part, usize)], w: f64, x: f64) -> Option<(usize, f64, f64)> {
    let ends = ribbon_ends(parts, w);
    let mut at = 0.0;
    for (k, &e) in ends.iter().enumerate() {
        if x < e || k == ends.len() - 1 {
            return Some((k, at, e - at));
        }
        at = e;
    }
    None
}

/// Where a gauge `w` px wide parts its filled and empty sides, in px, as
/// drawn: each side SEG_MIN px at least.
pub fn gauge_split(fill: f64, w: f64) -> f64 {
    if fill > 0.0 && fill < 1.0 { (fill * w).max(SEG_MIN).min(w - SEG_MIN) } else { fill * w }
}

impl Spark {
    /// Rows of the file that `k` rows read stand for.
    pub fn estimate(&self, k: usize) -> f64 {
        (k as f64 * self.n as f64 / self.s.max(1) as f64).round()
    }

    /// How full column `col` is: of the whole file, or of the rows shown under a filter.
    pub fn fill(&self, col: usize, f: Option<&SparkF>) -> f64 {
        match f.and_then(|f| f.cols.get(col)) {
            Some(fc) => fc.fill() as f64 / f.unwrap().s.max(1) as f64,
            None => self.cols[col].fill() as f64 / self.s.max(1) as f64,
        }
    }

    /// A text column's ribbon in order — its coloured values, the others, the
    /// empty cells — the parts holding rows (of the rows shown under a filter).
    pub fn ribbon(&self, col: usize, f: Option<&SparkF>) -> Vec<(Part, usize)> {
        let Some(Col::Ribbon(r)) = self.cols.get(col) else { return Vec::new() };
        let fr = f.and_then(|f| match f.cols.get(col) {
            Some(ColF::Ribbon { seg, other, fill }) => Some((seg, *other, f.s - fill)),
            _ => None,
        });
        let mut parts: Vec<(Part, usize)> = match fr {
            Some((seg, ..)) => seg.iter().enumerate().map(|(k, &n)| (Part::Seg(k), n as usize)).collect(),
            None => r.segs.iter().enumerate().map(|(k, s)| (Part::Seg(k), s.1 as usize)).collect(),
        };
        let (other, empty) = fr.map_or((r.other, self.s - r.fill), |x| (x.1, x.2));
        parts.push((Part::Other, other));
        parts.push((Part::Empty, empty));
        parts.retain(|p| p.1 > 0);
        parts
    }

    /// The value a text column's label names: its most frequent (among the rows
    /// shown under a filter), when it holds at least half as many rows as the
    /// others together.
    pub fn top(&self, col: usize, f: Option<&SparkF>) -> Option<usize> {
        let Some(Col::Ribbon(r)) = self.cols.get(col) else { return None };
        let (seg, other): (Vec<u32>, usize) = match f.and_then(|f| f.cols.get(col)) {
            Some(ColF::Ribbon { seg, other, .. }) => (seg.clone(), *other),
            _ => (r.segs.iter().map(|s| s.1).collect(), r.other),
        };
        let mut top: Option<usize> = None;
        for (k, &n) in seg.iter().enumerate() {
            if n > 0 && top.is_none_or(|t| n > seg[t]) {
                top = Some(k);
            }
        }
        top.filter(|&t| seg[t] as f64 >= other as f64 / 2.0)
    }

    /// The test a pick of column `col` puts on a cell: the rows it stands for.
    pub fn keep(&self, col: usize, pick: Pick) -> Keep<'_> {
        Keep { col: &self.cols[col], pick }
    }

    /// The pick as column filter texts, each one a `(col, text)` of
    /// `view::FilterSpec::cols` (all of them apply): `!` the filled cells, `=`
    /// the empty ones, `=value` a value, `>=a` / `<b` a range of bars (an end
    /// bar is open: it holds the outliers). None for the other values of a
    /// ribbon, which no column filter says — `refine` with `keep` does.
    /// `=value` ignores case and accents unless the filter matches case;
    /// `refine` is the exact test.
    pub fn filter_texts(&self, col: usize, pick: Pick) -> Option<Vec<String>> {
        Some(match (&self.cols[col], pick) {
            (_, Pick::Filled) => vec!["!".into()],
            (_, Pick::Empty) => vec!["=".into()],
            (Col::Ribbon(r), Pick::Seg(i)) => vec![format!("={}", r.segs.get(i)?.0.trim())],
            (Col::Curve(c), Pick::Bars(a, b)) => {
                let mut v = Vec::new();
                if a > 0 { v.push(format!(">={}", c.bins.edge_text(a))); }
                if b + 1 < c.bins.n { v.push(format!("<{}", c.bins.edge_text(b + 1))); }
                if v.is_empty() { v.push(">=-1e308".into()); }   // every bar: any number or date
                v
            }
            _ => return None,
        })
    }

    /// Bars a..=b's smallest and largest values as written.
    pub fn bar_values(&self, col: usize, a: usize, b: usize) -> Option<(&str, &str)> {
        let Col::Curve(c) = &self.cols[col] else { return None };
        let b = b.min(c.bins.n - 1);
        let lo = c.first[a..=b].iter().flatten().next()?;
        let hi = c.last[a..=b].iter().flatten().next_back()?;
        Some((lo, hi))
    }

    /// What a part stands for, as its tooltip names it: `2 – 5`, a value, `12 other values`.
    pub fn what(&self, col: usize, pick: Pick) -> String {
        match (&self.cols[col], pick) {
            (_, Pick::Filled) => "filled".into(),
            (_, Pick::Empty) => "empty".into(),
            (Col::Curve(_), Pick::Bars(a, b)) => match self.bar_values(col, a, b) {
                None => "No value here".into(),
                Some((x, y)) if x == y => x.into(),
                Some((x, y)) => format!("{x} – {y}"),
            },
            (Col::Ribbon(r), Pick::Seg(i)) => r.segs.get(i).map_or(String::new(), |s| s.0.clone()),
            (Col::Ribbon(r), Pick::Other) => format!("{}{} other values", fmt_int(r.distinct.saturating_sub(r.segs.len()) as u64), if r.many || self.sampled { "+" } else { "" }),
            _ => String::new(),
        }
    }

    /// Rows read in a part: (of the whole file, of the rows shown when filtered).
    pub fn count(&self, col: usize, pick: Pick, f: Option<&SparkF>) -> (usize, Option<usize>) {
        let fc = f.and_then(|f| f.cols.get(col).map(|c| (f, c)));
        match (&self.cols[col], pick) {
            (c, Pick::Filled) => (c.fill(), fc.map(|(_, c)| c.fill())),
            (c, Pick::Empty) => (self.s - c.fill(), fc.map(|(f, c)| f.s - c.fill())),
            (Col::Curve(c), Pick::Bars(a, b)) => {
                let b = b.min(c.bins.n - 1);
                let fl = fc.and_then(|(_, x)| if let ColF::Curve { cnt, .. } = x { Some(cnt[a..=b].iter().map(|&n| n as usize).sum()) } else { None });
                (c.cnt[a..=b].iter().map(|&n| n as usize).sum(), fl)
            }
            (Col::Ribbon(r), Pick::Seg(i)) => (r.segs.get(i).map_or(0, |s| s.1 as usize), fc.and_then(|(_, x)| if let ColF::Ribbon { seg, .. } = x { seg.get(i).map(|&n| n as usize) } else { None })),
            (Col::Ribbon(r), Pick::Other) => (r.other, fc.and_then(|(_, x)| if let ColF::Ribbon { other, .. } = x { Some(*other) } else { None })),
            _ => (0, None),
        }
    }
}

/// A part of a chart, picked to filter on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A curve's bars a..=b (a click: one bar; a drag: a range).
    Bars(usize, usize),
    /// A ribbon's coloured value.
    Seg(usize),
    /// A ribbon's other values.
    Other,
    /// The gauge's filled part (a ribbon's has none: its empty part is its last).
    Filled,
    Empty,
}

/// The test a pick puts on a cell (the one a chart part's hover lights too).
pub struct Keep<'a> {
    col: &'a Col,
    pick: Pick,
}

impl Keep<'_> {
    pub fn test(&self, v: &str) -> bool {
        match (self.col, self.pick) {
            (_, Pick::Filled) => cell_kind(v).is_some(),
            (_, Pick::Empty) => cell_kind(v).is_none(),
            (Col::Ribbon(r), Pick::Seg(i)) => r.segs.get(i).is_some_and(|s| s.0 == v),
            (Col::Ribbon(r), Pick::Other) => cell_kind(v).is_some() && !r.segs.iter().any(|s| s.0 == v),
            (Col::Curve(c), Pick::Bars(a, b)) => {
                let s = v.trim();
                cell_kind(s) == Some(c.bins.kind) && c.bins.key(s).is_some_and(|x| (a..=b).contains(&c.bins.of(x)))
            }
            _ => false,
        }
    }
}

/// The rows among `ids` whose cell in `col` passes `keep`, in parallel. None if cancelled.
pub fn refine(st: &State, ids: &[RowId], col: usize, keep: &Keep, cancel: &AtomicBool) -> Option<Vec<RowId>> {
    let out: Vec<RowId> = ids
        .par_iter()
        .with_min_len(4096)
        .map_init(Vec::new, |tmp, &id| {
            if cancel.load(Ordering::Relaxed) { return None; }
            st.with_cell(id, col, tmp, |v| keep.test(v)).then_some(id)
        })
        .flatten()
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use crate::view::{detect_kinds, filter, Filter, FilterSpec};
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn all(t: &Table) -> Vec<RowId> {
        (0..t.st.len()).map(|p| t.st.data_id(p)).collect()
    }

    /// tests/test_sparklines.py's small file.
    fn petit() -> Table {
        let mut s = String::from("n;cat;quand;vide\n");
        for i in 0..1000 {
            let cat = if i % 10 < 6 { "A" } else if i % 10 < 9 { "B" } else { "C" };
            s += &format!("{};{cat};2024-{:02}-{:02};{}\n", i % 10, 1 + i % 12, 1 + i % 28, if i >= 800 { "" } else { "x" });
        }
        table(&s)
    }

    fn filtered(t: &Table, cols: Vec<(usize, String)>) -> Vec<RowId> {
        let no = AtomicBool::new(false);
        filter(&t.st, &Filter::compile(&FilterSpec { cols, ..Default::default() }).unwrap(), &no).unwrap()
    }

    #[test]
    fn dates_on_a_time_line() {
        assert_eq!(time_key("1970-01-02"), Some(86_400.0));
        assert_eq!(time_key("2024-03-01"), time_key("2024-02-30"));   // the 30th of February: the 1st of March
        assert_eq!(time_text(time_key("15/03/2024 10:20:30").unwrap()), "2024-03-15 10:20:30");
        assert_eq!(time_text(-86_400.0), "1969-12-31 00:00:00");
    }

    #[test]
    fn counts() {
        let t = petit();
        let no = AtomicBool::new(false);
        let kinds = detect_kinds(&t.st);
        let s = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap();
        assert!(!s.sampled);
        assert_eq!(s.s, 1000);
        let Col::Curve(n) = &s.cols[0] else { panic!() };
        assert_eq!(n.cnt, vec![100; 10]);   // an integer a bar
        assert_eq!(n.first.iter().map(|x| x.clone().unwrap()).collect::<Vec<_>>(), (0..10).map(|i| i.to_string()).collect::<Vec<_>>());
        let Col::Ribbon(cat) = &s.cols[1] else { panic!() };
        assert_eq!(cat.segs, vec![("A".into(), 600), ("B".into(), 300), ("C".into(), 100)]);
        assert_eq!(cat.other, 0);
        let Col::Curve(q) = &s.cols[2] else { panic!() };
        assert_eq!((q.bins.n, q.cnt.iter().sum::<u32>()), (32, 1000));
        assert_eq!(q.bins.kind, Kind::Date);
        let Col::Ribbon(v) = &s.cols[3] else { panic!() };
        assert_eq!((v.fill, v.segs.clone()), (800, vec![("x".into(), 800)]));
        assert_eq!(s.top(1, None), Some(0));
        assert_eq!(crate::stats::fmt_pct(600.0 / s.s as f64), "60 %");
        assert_eq!(s.ribbon(3, None), vec![(Part::Seg(0), 800), (Part::Empty, 200)]);
        assert_eq!(spark_base(&t.st, &kinds, Sample::All, &no).unwrap(), s);
    }

    #[test]
    fn picks_filter() {
        let t = petit();
        let no = AtomicBool::new(false);
        let kinds = detect_kinds(&t.st);
        let s = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap();
        let ids = all(&t);
        let cells = |ids: &[RowId], c: usize| ids.iter().map(|&id| t.st.cell(id, c)).collect::<Vec<_>>();
        // A bar: the rows holding 3, by the exact test and by the filter texts.
        let k = refine(&t.st, &ids, 0, &s.keep(0, Pick::Bars(3, 3)), &no).unwrap();
        assert_eq!(k.len(), 100);
        assert!(cells(&k, 0).iter().all(|v| v == "3"));
        let texts = s.filter_texts(0, Pick::Bars(3, 3)).unwrap();
        assert_eq!(texts, vec![">=2.5", "<3.5"]);
        assert_eq!(filtered(&t, texts.into_iter().map(|x| (0, x)).collect()), k);
        // Counted against the base: bar 3 alone, every row an "A".
        let f = spark_filtered(&t.st, &s, &k, Sample::Js, &no).unwrap();
        assert_eq!(f.cols[0], ColF::Curve { cnt: vec![0, 0, 0, 100, 0, 0, 0, 0, 0, 0], fill: 100, bad: 0 });
        assert_eq!(f.cols[1], ColF::Ribbon { seg: vec![100, 0, 0], other: 0, fill: 100 });
        // A range dragged across: 2 – 5, 400 rows; an end bar is open.
        let k = refine(&t.st, &ids, 0, &s.keep(0, Pick::Bars(2, 5)), &no).unwrap();
        assert_eq!(k.len(), 400);
        assert_eq!(s.what(0, Pick::Bars(2, 5)), "2 – 5");
        assert_eq!(s.count(0, Pick::Bars(2, 5), None), (400, None));
        assert_eq!(s.filter_texts(0, Pick::Bars(0, 2)).unwrap(), vec!["<2.5"]);
        assert_eq!(filtered(&t, vec![(0, "<2.5".into())]).len(), 300);
        // A value of the ribbon, the others, the gauge.
        let k = refine(&t.st, &ids, 1, &s.keep(1, Pick::Seg(1)), &no).unwrap();
        assert_eq!(k.len(), 300);
        assert_eq!(filtered(&t, vec![(1, s.filter_texts(1, Pick::Seg(1)).unwrap()[0].clone())]), k);
        assert_eq!(s.filter_texts(1, Pick::Other), None);
        assert_eq!(refine(&t.st, &ids, 1, &s.keep(1, Pick::Other), &no).unwrap().len(), 0);
        assert_eq!(refine(&t.st, &ids, 3, &s.keep(3, Pick::Empty), &no).unwrap().len(), 200);
        assert_eq!(filtered(&t, vec![(3, "=".into())]).len(), 200);
        assert_eq!(filtered(&t, vec![(3, "!".into())]).len(), 800);
        // A range of dates: the bars' edges as dates the filter reads.
        let texts = s.filter_texts(2, Pick::Bars(4, 9)).unwrap();
        let k = refine(&t.st, &ids, 2, &s.keep(2, Pick::Bars(4, 9)), &no).unwrap();
        assert!(!k.is_empty());
        assert_eq!(filtered(&t, texts.into_iter().map(|x| (2, x)).collect()), k);
        // Under a filter, the label follows the rows shown.
        let b = refine(&t.st, &ids, 1, &s.keep(1, Pick::Seg(1)), &no).unwrap();
        let f = spark_filtered(&t.st, &s, &b, Sample::Js, &no).unwrap();
        assert_eq!(s.top(1, Some(&f)), Some(1));
        assert_eq!(s.fill(1, Some(&f)), 1.0);
        assert_eq!(s.ribbon(1, Some(&f)), vec![(Part::Seg(1), 300)]);
    }

    #[test]
    fn big_file_sampled_by_record() {
        let mut s = String::from("montant;nom\n");
        for i in 0..60_000u64 {
            s += &format!("\"{},{:02}\";nom {}\n", (i * 7919) % 100_003, i % 100, i % 37);
        }
        let t = table(&s);
        let no = AtomicBool::new(false);
        let kinds = detect_kinds(&t.st);
        let sp = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap();
        assert!(sp.sampled);
        assert_eq!(sp.n, 60_000);
        assert!(sp.s <= 20_000);
        let Col::Curve(c) = &sp.cols[0] else { panic!("{:?}", sp.cols[0]) };
        assert_eq!(c.cnt.iter().sum::<u32>() as usize, sp.s);
        // Sorted: the same records, so the same picture.
        let mut ids = all(&t);
        crate::view::sort(&t.st, &mut ids, &[crate::view::SortKey { col: 0, desc: false }], &kinds, &no);
        let mut t2 = t;
        let n = t2.st.base.n;
        let mut order: Vec<RowId> = vec![0];
        order.extend(ids.iter().copied());
        assert_eq!(order.len(), n);
        t2.st.order = Some(Arc::new(order));
        assert_eq!(spark_base(&t2.st, &kinds, Sample::Js, &no).unwrap(), sp);
    }

    #[test]
    fn area_stays_between_bars() {
        let cnt = [0, 10, 40, 40, 5, 0, 0, 30];
        let (y0, segs) = area(&cnt);
        assert_eq!(y0, 0.0);
        assert_eq!(segs.len(), 7);
        let h = heights(&cnt);
        assert_eq!(h[1], 25.0);
        assert_eq!(h[4], 12.5);
        for (k, s) in segs.iter().enumerate() {
            let (a, b) = (h[k].min(h[k + 1]), h[k].max(h[k + 1]));
            assert!(s[1] >= a - 1e-9 && s[1] <= b + 1e-9 && s[3] >= a - 1e-9 && s[3] <= b + 1e-9, "{k}: {s:?}");
        }
        let parts = [(Part::Seg(0), 996), (Part::Other, 4)];
        let ends = ribbon_ends(&parts, 100.0);
        assert!(ends[0] < 100.0 - SEG_MIN + 1.0 && ends[1] == 100.0);
        assert_eq!(ribbon_at(&parts, 100.0, 99.0).unwrap().0, 1);
        assert_eq!(gauge_split(0.999, 100.0), 94.0);
    }
}
