//! Reading the grid at a glance — the data behind it, not the drawing:
//! category pills for columns of few values, rich values (links, e-mails,
//! yes / no, colours, JSON), a numeric column's colour scale and data bars,
//! groups of a sorted view; and the scroll strip along the scrollbar: the
//! sort column's labels (initials, years, values) or round row numbers, and
//! marks for irregular, duplicate or marked rows.

use crate::sparkline::{Col, Spark, SEGS};
use crate::stats::{cell_kind, fmt_num, num_cell, quantiles};
use crate::table::{RowId, State};
use crate::text::parse_number;
use crate::view::Kind;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Distinct values at most for a column of pills.
pub const PILL_MAX: usize = 12;
/// …each this long at most on average.
pub const PILL_LEN: usize = 24;
/// Rows read for the pills without the charts, and for the rich values.
pub const LOOK_ROWS: usize = 400;
/// Rows of a sorted view read to tell whether it has groups.
pub const GROUP_ROWS: usize = 400;

/// The first `n` data rows' cells of column `col`, trimmed, the empty ones skipped.
fn head_cells(st: &State, col: usize, n: usize, mut f: impl FnMut(&str) -> bool) {
    let mut tmp = Vec::new();
    for p in 0..st.len().min(n) {
        let go = st.with_cell(st.data_id(p), col, &mut tmp, |v| {
            let v = v.trim();
            v.is_empty() || f(v)
        });
        if !go {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Pills

/// A column of pills: its values in the ribbon's order, each its colour slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pills {
    pub values: Vec<String>,
}

impl Pills {
    /// A cell's pill: None for an empty cell, Some(None) for a value without a
    /// colour of its own (grey), Some(Some(slot)) for the ribbon's slot-th colour.
    pub fn slot(&self, cell: &str) -> Option<Option<usize>> {
        let v = cell.trim();
        if v.is_empty() {
            return None;
        }
        Some(self.values.iter().position(|x| x.trim() == v))
    }

    fn qualify(values: Vec<String>, distinct: usize) -> Option<Pills> {
        let len: usize = values.iter().map(|v| v.chars().count()).sum();
        (values.len() >= 2 && distinct <= PILL_MAX && len <= PILL_LEN * values.len()).then_some(Pills { values })
    }
}

/// Per column, its pills when it has few values: from the charts' ribbons, in
/// their order and colours. A column of rich values (yes / no…) gets none.
pub fn pills_from_spark(spark: &Spark, kinds: &[Kind], rich: &[Option<Rich>]) -> Vec<Option<Pills>> {
    spark
        .cols
        .iter()
        .enumerate()
        .map(|(c, col)| {
            if kinds.get(c) != Some(&Kind::Text) || rich.get(c).is_some_and(|r| r.is_some()) {
                return None;
            }
            let Col::Ribbon(r) = col else { return None };
            Pills::qualify(r.segs.iter().map(|s| s.0.clone()).collect(), if r.many { usize::MAX } else { r.distinct })
        })
        .collect()
}

/// The same when the charts are off: from the first LOOK_ROWS rows.
pub fn pills_from_rows(st: &State, kinds: &[Kind], rich: &[Option<Rich>]) -> Vec<Option<Pills>> {
    (0..st.ncols)
        .into_par_iter()
        .map(|c| {
            if kinds.get(c) != Some(&Kind::Text) || rich.get(c).is_some_and(|r| r.is_some()) {
                return None;
            }
            let mut m: Vec<(String, u32)> = Vec::new();
            head_cells(st, c, LOOK_ROWS, |v| {
                match m.iter_mut().find(|x| x.0 == v) {
                    Some(x) => x.1 += 1,
                    None => m.push((v.to_string(), 1)),
                }
                m.len() <= PILL_MAX
            });
            let distinct = m.len();
            m.sort_by_key(|x| std::cmp::Reverse(x.1));   // stable: ties in order of appearance
            Pills::qualify(m.into_iter().filter(|x| x.1 >= 2).take(SEGS).map(|x| x.0).collect(), distinct)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rich values

/// A text column whose filled values are, 90 % of them, of one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rich {
    Url,
    Email,
    /// Yes / no: the true and the false word, lower case.
    Bool { yes: &'static str, no: &'static str },
    Color,
    Json,
}

const RICH_MIN: f64 = 0.9;
const RICH_BOOL: [(&str, &str); 4] = [("true", "false"), ("yes", "no"), ("oui", "non"), ("vrai", "faux")];

struct Res {
    url: regex::Regex,
    email: regex::Regex,
    color: regex::Regex,
}

fn res() -> &'static Res {
    static R: OnceLock<Res> = OnceLock::new();
    R.get_or_init(|| Res {
        url: regex::Regex::new(r#"(?i)^https?://[^\s<>"'`]+$"#).unwrap(),
        email: regex::Regex::new(r#"(?i)^[^\s@<>"'`()]+@[^\s@<>"'`()]+\.[a-z]{2,}$"#).unwrap(),
        color: regex::Regex::new(r"(?i)^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$").unwrap(),
    })
}

impl Rich {
    /// Whether a cell is of this kind (a link to open, an address to write to, a colour to show).
    pub fn holds(&self, v: &str) -> bool {
        let v = v.trim();
        match self {
            Rich::Url => res().url.is_match(v),
            Rich::Email => res().email.is_match(v),
            Rich::Color => res().color.is_match(v),
            Rich::Json => json_object(v),
            Rich::Bool { yes, no } => v.eq_ignore_ascii_case(yes) || v.eq_ignore_ascii_case(no),
        }
    }

    /// A yes / no cell's state: Some(true) for the true word.
    pub fn checked(&self, v: &str) -> Option<bool> {
        let Rich::Bool { yes, no } = self else { return None };
        let v = v.trim();
        if v.eq_ignore_ascii_case(yes) { Some(true) } else if v.eq_ignore_ascii_case(no) { Some(false) } else { None }
    }
}

/// Per column, its kind of rich values (text columns only), from the first LOOK_ROWS rows.
pub fn rich_cols(st: &State, kinds: &[Kind]) -> Vec<Option<Rich>> {
    let r = res();
    (0..st.ncols)
        .into_par_iter()
        .map(|c| {
            if kinds.get(c) != Some(&Kind::Text) {
                return None;
            }
            let (mut n, mut url, mut mail, mut col, mut json, mut parsed) = (0, 0, 0, 0, 0, 0);
            let mut words: Vec<String> = Vec::new();
            head_cells(st, c, LOOK_ROWS, |s| {
                n += 1;
                if words.len() <= 2 {
                    let w = s.to_lowercase();
                    if !words.contains(&w) { words.push(w); }
                }
                match s.as_bytes()[0] {
                    b'h' | b'H' => url += r.url.is_match(s) as usize,
                    b'#' => col += r.color.is_match(s) as usize,
                    b'{' | b'[' => {
                        // A few parsed: a column of JSON shows at once.
                        parsed += 1;
                        if parsed <= 40 { json += json_object(s) as usize; }
                    }
                    _ => {}
                }
                if s.find('@').is_some_and(|k| k > 0) && r.email.is_match(s) { mail += 1; }
                true
            });
            if n < 3 {
                return None;
            }
            let need = |k: usize| k as f64 >= n as f64 * RICH_MIN;
            let pair = (words.len() <= 2).then(|| RICH_BOOL.iter().find(|p| words.iter().all(|w| w == p.0 || w == p.1))).flatten();
            if let Some(&(yes, no)) = pair { Some(Rich::Bool { yes, no }) }
            else if need(url) { Some(Rich::Url) }
            else if need(mail) { Some(Rich::Email) }
            else if need(col) { Some(Rich::Color) }
            else if json as f64 >= n.min(40) as f64 * RICH_MIN { Some(Rich::Json) }
            else { None }
        })
        .collect()
}

/// Whether `s` is a JSON object or array, whole.
pub fn json_object(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    matches!(b.iter().find(|c| !c.is_ascii_whitespace()), Some(b'{' | b'[')) && json_value(b, &mut i, 0) && {
        ws(b, &mut i);
        i == b.len()
    }
}

fn ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r') { *i += 1; }
}

fn json_value(b: &[u8], i: &mut usize, depth: usize) -> bool {
    ws(b, i);
    if *i >= b.len() || depth > 256 {
        return false;
    }
    match b[*i] {
        open @ (b'{' | b'[') => {
            let close = if open == b'{' { b'}' } else { b']' };
            *i += 1;
            ws(b, i);
            if b.get(*i) == Some(&close) { *i += 1; return true; }
            loop {
                if open == b'{' {
                    ws(b, i);
                    if b.get(*i) != Some(&b'"') || !json_string(b, i) { return false; }
                    ws(b, i);
                    if b.get(*i) != Some(&b':') { return false; }
                    *i += 1;
                }
                if !json_value(b, i, depth + 1) { return false; }
                ws(b, i);
                match b.get(*i) {
                    Some(&c) if c == close => { *i += 1; return true; }
                    Some(b',') => *i += 1,
                    _ => return false,
                }
            }
        }
        b'"' => json_string(b, i),
        b't' | b'f' | b'n' => ["true", "false", "null"].iter().any(|w| {
            let ok = b[*i..].starts_with(w.as_bytes());
            if ok { *i += w.len(); }
            ok
        }),
        _ => {
            // A number: -?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?
            let start = *i;
            let digits = |i: &mut usize| { let s = *i; while *i < b.len() && b[*i].is_ascii_digit() { *i += 1; } *i > s };
            if b[*i] == b'-' { *i += 1; }
            if b.get(*i) == Some(&b'0') { *i += 1; } else if !digits(i) { return false; }
            if b.get(*i) == Some(&b'.') { *i += 1; if !digits(i) { return false; } }
            if matches!(b.get(*i), Some(b'e' | b'E')) {
                *i += 1;
                if matches!(b.get(*i), Some(b'+' | b'-')) { *i += 1; }
                if !digits(i) { return false; }
            }
            *i > start
        }
    }
}

fn json_string(b: &[u8], i: &mut usize) -> bool {
    *i += 1;
    while *i < b.len() {
        match b[*i] {
            b'"' => { *i += 1; return true; }
            b'\\' => {
                *i += 1;
                match b.get(*i) {
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => *i += 1,
                    Some(b'u') if b.len() >= *i + 5 && b[*i + 1..*i + 5].iter().all(u8::is_ascii_hexdigit) => *i += 5,
                    _ => return false,
                }
            }
            c if c < 0x20 => return false,
            _ => *i += 1,
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Colour scale and data bars

/// A numeric column's colour scale: light to strong from its 2nd to its 98th
/// percentile (an outlier does not squash the others into one shade).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heat {
    pub lo: f64,
    pub hi: f64,
}

impl Heat {
    /// A value's tint: (in the danger colour — a negative where the column
    /// holds both signs —, % of that colour mixed into the row's background: 6 to 46).
    pub fn tint(&self, x: f64) -> (bool, f64) {
        let (h, mut neg) = (self, false);
        let f = if h.lo < 0.0 && h.hi > 0.0 {
            if x < 0.0 { neg = true; x / h.lo } else { x / h.hi }
        } else if h.hi > h.lo {
            (x - h.lo) / (h.hi - h.lo)
        } else {
            1.0
        };
        (neg, (6.0 + f.clamp(0.0, 1.0) * 40.0).round())
    }

    /// A cell's tint; None when it holds no number.
    pub fn cell(&self, v: &str) -> Option<(bool, f64)> {
        num_cell(v).map(|x| self.tint(x))
    }
}

/// Column `col`'s numbers over rows `ids`, in parallel. None if cancelled.
fn numbers(st: &State, ids: &[RowId], col: usize, cancel: &AtomicBool) -> Option<Vec<f64>> {
    let v: Vec<f64> = ids
        .par_iter()
        .with_min_len(4096)
        .map_init(Vec::new, |tmp, &id| {
            if cancel.load(Ordering::Relaxed) { return None; }
            st.with_cell(id, col, tmp, num_cell)
        })
        .flatten()
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(v) }
}

/// The colour scale of column `col` over rows `ids` (all of them, as the JS);
/// Some(None) when it holds no number, None if cancelled.
pub fn heat(st: &State, ids: &[RowId], col: usize, cancel: &AtomicBool) -> Option<Option<Heat>> {
    let mut v = numbers(st, ids, col, cancel)?;
    if v.is_empty() {
        return Some(None);
    }
    let (lo, hi) = quantiles(&mut v, 0.02, 0.98);
    Some(Some(Heat { lo, hi }))
}

/// A numeric column's data bars: its range, zero included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataBars {
    pub min: f64,
    pub max: f64,
}

impl DataBars {
    /// A value's bar, in % of the cell's width: (start, width, negative), from zero.
    pub fn bar(&self, x: f64) -> (f64, f64, bool) {
        let span = if self.max > self.min { self.max - self.min } else { 1.0 };
        let z = -self.min / span * 100.0;
        let p = (x - self.min) / span * 100.0;
        (z.min(p), (p - z).abs().max(0.6), x < 0.0)
    }
}

/// The data bars of column `col` over rows `ids`; Some(None) when it holds no number.
pub fn data_bars(st: &State, ids: &[RowId], col: usize, cancel: &AtomicBool) -> Option<Option<DataBars>> {
    let (lo, hi) = ids
        .par_iter()
        .with_min_len(4096)
        .fold(
            || (f64::INFINITY, f64::NEG_INFINITY, Vec::new()),
            |(lo, hi, mut tmp), &id| {
                if cancel.load(Ordering::Relaxed) { return (lo, hi, tmp); }
                match st.with_cell(id, col, &mut tmp, num_cell) {
                    Some(x) => (lo.min(x), hi.max(x), tmp),
                    None => (lo, hi, tmp),
                }
            },
        )
        .map(|(a, b, _)| (a, b))
        .reduce(|| (f64::INFINITY, f64::NEG_INFINITY), |a, b| (a.0.min(b.0), a.1.max(b.1)));
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some((lo <= hi).then(|| DataBars { min: lo.min(0.0), max: hi.max(0.0) }))
}

/// Whether a sorted view's first sort column holds groups worth a line
/// between them: among its first GROUP_ROWS rows shown, fewer changes of
/// value than half the rows.
pub fn has_groups(st: &State, ids: &[RowId], col: usize) -> bool {
    let n = ids.len().min(GROUP_ROWS);
    let mut tmp = Vec::new();
    let (mut prev, mut changes) = (String::new(), 0);
    for (i, &id) in ids[..n].iter().enumerate() {
        st.with_cell(id, col, &mut tmp, |v| {
            let v = v.trim();
            if i > 0 && v != prev { changes += 1; }
            prev.clear();
            prev.push_str(v);
        });
    }
    n > 3 && changes * 2 < n
}

// ---------------------------------------------------------------------------
// The scroll strip

/// Pixels between two labels of the strip.
pub const LABEL_STEP: f64 = 44.0;

/// The strip's geometry: `h` pixels stand for the grid's whole scroll extent
/// (header `th`, then the rows `row_h` each, then the totals row).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripGeom {
    pub h: f64,
    pub th: f64,
    pub row_h: f64,
    /// The scroll extent.
    pub sh: f64,
}

impl StripGeom {
    pub fn new(h: f64, th: f64, row_h: f64, rows: usize, foot: f64) -> StripGeom {
        StripGeom { h, th, row_h, sh: h.max(th + rows as f64 * row_h + foot) }
    }

    /// Where row i starts on the strip.
    pub fn row_y(&self, i: usize) -> f64 {
        (self.th + i as f64 * self.row_h) / self.sh * self.h
    }

    /// The row at y on the strip (of `n` rows).
    pub fn row_at(&self, y: f64, n: usize) -> usize {
        (((y / self.h * self.sh - self.th) / self.row_h).floor().max(0.0) as usize).min(n.saturating_sub(1))
    }

    /// The pixel row of row i: where its mark goes.
    pub fn row_px(&self, i: usize) -> usize {
        ((self.th + i as f64 * self.row_h) * self.h / self.sh).floor() as usize
    }

    /// Where the sort column's labels go: every LABEL_STEP px, (y, row there).
    pub fn label_rows(&self, n: usize) -> Vec<(f64, usize)> {
        let mut out = Vec::new();
        let mut y = 8.0;
        while y < self.h - 10.0 && n > 0 {
            out.push((y, self.row_at(y, n)));
            y += LABEL_STEP;
        }
        out
    }

    /// The step of the row numbers along an unsorted view: the smallest power
    /// of ten whose ticks sit LABEL_STEP apart — halved under five ticks
    /// (1 600 rows: 500, 1k, 1,5k rather than a lone 1k).
    pub fn tick_step(&self, n: usize) -> usize {
        let px = |k: f64| k * self.row_h * self.h / self.sh;
        let mut step = 10f64.powf((LABEL_STEP / px(1.0)).log10().ceil().max(0.0)).round();
        if step >= 10.0 && (n as f64) < 5.0 * step && px(step / 2.0) >= LABEL_STEP {
            step /= 2.0;
        }
        step as usize
    }

    /// An unsorted view's labels: round row numbers, (y, label).
    pub fn ticks(&self, n: usize) -> Vec<(f64, String)> {
        let step = self.tick_step(n).max(1);
        (1..=n / step)
            .map(|j| j * step)
            .filter_map(|k| {
                let y = (self.h - 6.0).min(self.row_y(k - 1));
                (y >= 8.0).then(|| (y, compact_tick(k as f64)))
            })
            .collect()
    }
}

/// How the strip labels a column: by its values for numbers, its years for
/// dates, its initials for text — read from its first 2 000 filled cells (90 %).
pub fn sort_kind(st: &State, col: usize) -> Kind {
    let (mut seen, mut nums, mut dates) = (0, 0, 0);
    let mut tmp = Vec::new();
    for p in 0..st.len() {
        let k = st.with_cell(st.data_id(p), col, &mut tmp, cell_kind);
        match k {
            None => continue,
            Some(Kind::Number) => nums += 1,
            Some(Kind::Date) => dates += 1,
            Some(Kind::Text) => {}
        }
        seen += 1;
        if seen >= 2000 { break; }
    }
    if seen > 0 && nums * 10 >= seen * 9 { Kind::Number } else if seen > 0 && dates * 10 >= seen * 9 { Kind::Date } else { Kind::Text }
}

/// A cell's label on the strip: a number compact (`1,5k`), a date's year, a
/// text's initial without its accent; "" for none.
pub fn strip_label(v: &str, kind: Kind) -> String {
    let v = v.trim();
    match kind {
        Kind::Number => parse_number(v).map_or(String::new(), compact_tick),
        Kind::Date => {
            let b = v.as_bytes();
            (0..b.len().saturating_sub(3)).find(|&i| b[i..i + 4].iter().all(u8::is_ascii_digit)).map_or(String::new(), |i| v[i..i + 4].to_string())
        }
        Kind::Text => {
            let Some(c) = v.chars().next() else { return String::new() };
            let mut out = String::new();
            for u in c.to_uppercase() {
                unicode_normalization::char::decompose_canonical(u, |d| {
                    if !unicode_normalization::char::is_combining_mark(d) { out.push(d); }
                });
            }
            out
        }
    }
}

/// The sort column's labels at `rows` ((y, row) — `StripGeom::label_rows`):
/// (y, label), a label equal to the one before left out.
pub fn strip_labels(st: &State, ids: &[RowId], col: usize, kind: Kind, rows: &[(f64, usize)]) -> Vec<(f64, String)> {
    let mut tmp = Vec::new();
    let mut out: Vec<(f64, String)> = Vec::new();
    let mut last = String::new();
    for &(y, i) in rows {
        let Some(&id) = ids.get(i) else { continue };
        let label = st.with_cell(id, col, &mut tmp, |v| strip_label(v, kind));
        if label.is_empty() || label == last { continue; }
        last.clone_from(&label);
        out.push((y, label));
    }
    out
}

/// A number as fr-FR writes it short: `150`, `12,5`, `1,5k`, `2,3M`, `1Md`.
/// A scroll-strip label: as short as it gets, never a decimal — 7, 950, 1k, 12k,
/// 1M; a value between round thousands below 10 000 is written whole (1500).
pub fn compact_tick(x: f64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "Bn"), (1e9, "Md"), (1e6, "M"), (1e3, "k")];
    let sign = if x < 0.0 { "-" } else { "" };
    let a = x.abs().round();
    // The largest unit the value is a whole number of: 2M, 1100k (not 1,1M), 1500.
    for (u, name) in UNITS {
        if a >= u && a % u == 0.0 && a / u < 10_000.0 {
            return format!("{sign}{}{name}", a / u);
        }
    }
    if a < 10_000.0 { return format!("{sign}{a}"); }
    // Not a round number (a sorted column's value): rounded to its unit, approximately.
    let k = UNITS.iter().position(|(u, _)| a >= *u).unwrap_or(UNITS.len() - 1);
    let v = (a / UNITS[k].0).round();
    // 999 600 rounds to 1000k: that is 1M.
    if v >= 1000.0 && k > 0 { return format!("{sign}{}{}", (v / 1000.0).round(), UNITS[k - 1].1); }
    format!("{sign}{v}{}", UNITS[k].1)
}

pub fn compact_fr(x: f64) -> String {
    const UNITS: [(f64, &str); 5] = [(1.0, ""), (1e3, "k"), (1e6, "M"), (1e9, "Md"), (1e12, "Bn")];
    let a = x.abs();
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    let mut k = UNITS.iter().rposition(|u| a >= u.0).unwrap_or(0);
    let mut v = r1(a / UNITS[k].0);
    if v >= 1000.0 && k + 1 < UNITS.len() {
        k += 1;   // 999 950 is 1M, not 1 000k
        v = r1(a / UNITS[k].0);
    }
    format!("{}{}{}", if x < 0.0 && v > 0.0 { "-" } else { "" }, fmt_num(v, 0, 1, false), UNITS[k].1)
}

/// Mark levels per pixel row of the strip (`h` px): the strongest level among
/// the rows drawn there (`level`: 0 none; the app's order — irregular 1,
/// duplicate 2, marked 3). None if cancelled.
pub fn strip_marks(ids: &[RowId], g: &StripGeom, level: impl Fn(RowId) -> u8 + Sync, cancel: &AtomicBool) -> Option<Vec<u8>> {
    let h = g.h.ceil() as usize + 1;
    let bands = ids
        .par_iter()
        .enumerate()
        .with_min_len(1 << 14)
        .fold(
            || vec![0u8; h],
            |mut b, (i, &id)| {
                if i & 1023 == 0 && cancel.load(Ordering::Relaxed) { return b; }
                let k = level(id);
                if k > 0 {
                    let y = g.row_px(i).min(h - 1);
                    b[y] = b[y].max(k);
                }
                b
            },
        )
        .reduce(|| vec![0u8; h], |mut a, b| {
            a.iter_mut().zip(b).for_each(|(x, y)| *x = (*x).max(y));
            a
        });
    if cancel.load(Ordering::Relaxed) { None } else { Some(bands) }
}

/// Mark bands as runs to draw: (first pixel row, end, level).
pub fn mark_runs(bands: &[u8]) -> Vec<(usize, usize, u8)> {
    let mut out = Vec::new();
    let mut from = 0;
    for y in 1..=bands.len() {
        if y == bands.len() || bands[y] != bands[from] {
            if bands[from] > 0 { out.push((from, y, bands[from])); }
            from = y;
        }
    }
    out
}

/// Rows passing `pred` per pixel row of the strip: a hovered chart part's
/// rows, a find's matches — drawn by their density. None if cancelled.
pub fn strip_counts(ids: &[RowId], g: &StripGeom, pred: impl Fn(RowId) -> bool + Sync, cancel: &AtomicBool) -> Option<Vec<u32>> {
    let h = g.h.ceil() as usize + 2;
    let c = ids
        .par_iter()
        .enumerate()
        .with_min_len(1 << 14)
        .fold(
            || vec![0u32; h],
            |mut c, (i, &id)| {
                if (i & 1023 != 0 || !cancel.load(Ordering::Relaxed)) && pred(id) {
                    c[g.row_px(i).min(h - 1)] += 1;
                }
                c
            },
        )
        .reduce(|| vec![0u32; h], |mut a, b| {
            a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
            a
        });
    if cancel.load(Ordering::Relaxed) { None } else { Some(c) }
}

/// The records whose field count differs from the file's: irregular rows
/// (an edited row has the header's width again — told only while no column
/// was added, moved or removed).
pub struct Irregular<'a> {
    st: &'a State,
}

impl<'a> Irregular<'a> {
    /// None in the usual case of a file without any: nothing to visit.
    pub fn new(st: &'a State) -> Option<Irregular<'a>> {
        (!st.base.odd.is_empty()).then_some(Irregular { st })
    }

    #[inline]
    pub fn is(&self, id: RowId) -> bool {
        let st = self.st;
        (id as usize) < st.base.n && st.base.odd.binary_search_by_key(&id, |o| o.0).is_ok() && (st.pristine(id) || st.cmap.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::sparkline::{spark_base, Sample};
    use crate::table::Table;
    use crate::view::detect_kinds;
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn all(t: &Table) -> Vec<RowId> {
        (0..t.st.len()).map(|p| t.st.data_id(p)).collect()
    }

    /// tests/test_grid_look.py's file.
    fn ventes() -> Table {
        const STATUTS: [&str; 6] = ["actif", "vip", "prospect", "inactif", "perdu", "suspendu"];
        let mut s = String::from("id;statut;montant;note\n");
        for i in 1..=120 {
            let m = ["3", "12,5", "1 250,00", "47,10"][i % 4];
            s += &format!("{i};{};{m};texte libre {i}\n", if i % 3 != 0 { STATUTS[(i * 7) % 6] } else { "actif" });
        }
        table(&s)
    }

    #[test]
    fn pills_follow_the_ribbon() {
        let t = ventes();
        let no = AtomicBool::new(false);
        let kinds = detect_kinds(&t.st);
        let rich = rich_cols(&t.st, &kinds);
        let sp = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap();
        let p = pills_from_spark(&sp, &kinds, &rich);
        let cols: Vec<usize> = p.iter().enumerate().filter(|x| x.1.is_some()).map(|x| x.0).collect();
        assert_eq!(cols, vec![1]);   // the status only (the note is free text)
        let Col::Ribbon(r) = &sp.cols[1] else { panic!() };
        let pills = p[1].as_ref().unwrap();
        assert_eq!(pills.values, r.segs.iter().map(|s| s.0.clone()).collect::<Vec<_>>());
        assert_eq!(pills.slot(&r.segs[0].0), Some(Some(0)));
        assert_eq!(pills.slot(""), None);
        assert_eq!(pills_from_rows(&t.st, &kinds, &rich)[1].as_ref().map(|p| p.values.len()), Some(5));
    }

    #[test]
    fn groups_of_a_sorted_view() {
        let t = ventes();
        let no = AtomicBool::new(false);
        let kinds = detect_kinds(&t.st);
        let mut ids = all(&t);
        assert!(!has_groups(&t.st, &ids, 1));
        crate::view::sort(&t.st, &mut ids, &[crate::view::SortKey { col: 1, desc: false }], &kinds, &no);
        assert!(has_groups(&t.st, &ids, 1));
        assert!(!has_groups(&t.st, &ids, 0));
    }

    /// tests/test_reading_aids.py's file.
    #[test]
    fn rich_columns_and_colour_scale() {
        const COULEURS: [&str; 5] = ["#e74c3c", "#3498db", "#2ecc71", "#f1c40f", "#9b59b6"];
        let mut s = String::from("nom;site;email;actif;couleur;meta;montant;remarque\n");
        for i in 1..=60i64 {
            let coul = if i != 7 { COULEURS[i as usize % 5] } else { "rouge" };
            s += &format!("Client {i};https://exemple{i}.fr/p;contact{i}@exemple.fr;{};{coul};\"{{\"\"id\"\": {i}, \"\"tags\"\": [\"\"a\"\"]}}\";{};Une remarque assez longue, numéro {i}\n", if i % 3 != 0 { "oui" } else { "non" }, i * 10 - 100);
        }
        let t = table(&s);
        let no = AtomicBool::new(false);
        let kinds = detect_kinds(&t.st);
        let rich = rich_cols(&t.st, &kinds);
        assert_eq!(rich, vec![None, Some(Rich::Url), Some(Rich::Email), Some(Rich::Bool { yes: "oui", no: "non" }), Some(Rich::Color), Some(Rich::Json), None, None]);
        assert!(!Rich::Color.holds("rouge"));
        assert_eq!(rich[3].as_ref().unwrap().checked("Oui"), Some(true));
        let sp = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap();
        assert!(pills_from_spark(&sp, &kinds, &rich)[3].is_none());   // yes / no: checks, not pills
        let h = heat(&t.st, &all(&t), 6, &no).unwrap().unwrap();
        assert!(h.lo < 0.0 && h.hi > 0.0);
        assert!(h.tint(-50.0).0);
        assert_eq!(h.tint(0.0), (false, 6.0));
        assert_eq!(h.tint(1e6), (false, 46.0));
        assert_eq!(h.cell("texte"), None);
        let b = data_bars(&t.st, &all(&t), 6, &no).unwrap().unwrap();
        assert_eq!((b.min, b.max), (-90.0, 500.0));
        assert!(json_object(r#" {"a": [1, -2.5e3, true, null, "x\"y"]} "#));
        assert!(!json_object(r#"{"a": }"#));
        assert!(!json_object("[1, 2"));
    }

    #[test]
    fn strip() {
        assert_eq!(compact_fr(150.0), "150");
        assert_eq!(compact_fr(12.5), "12,5");
        assert_eq!(compact_fr(1500.0), "1,5k");
        assert_eq!(compact_fr(1_400_000.0), "1,4M");
        assert_eq!(compact_fr(999_950.0), "1M");
        assert_eq!(compact_fr(-2527.14), "-2,5k");
        assert_eq!(compact_fr(3e9), "3Md");
        assert_eq!(strip_label("émile", Kind::Text), "E");
        assert_eq!(strip_label("15/03/2024", Kind::Date), "2024");
        assert_eq!(strip_label("1 234,5", Kind::Number), "1235");
        assert_eq!(strip_label("12 345,5", Kind::Number), "12k");
        assert_eq!(compact_tick(1000.0), "1k");
        assert_eq!(compact_tick(2000.0), "2k");
        assert_eq!(compact_tick(1500.0), "1500");
        assert_eq!(compact_tick(150_000.0), "150k");
        assert_eq!(compact_tick(1_400_000.0), "1400k");
        assert_eq!(compact_tick(1_234_567.0), "1M");
        assert_eq!(compact_tick(999_600.0), "1M");
        assert_eq!(compact_tick(-2527.14), "-2527");
        // 150 rows of 28 px in 800 px: ticks every 10 rows.
        let g = StripGeom::new(800.0, 60.0, 28.0, 150, 0.0);
        assert_eq!(g.tick_step(150), 10);
        let ticks = g.ticks(150);
        assert_eq!(ticks.first().unwrap().1, "10");
        assert_eq!(ticks.last().unwrap().1, "150");
        let g = StripGeom::new(800.0, 60.0, 28.0, 1_400_000, 0.0);
        assert_eq!(g.tick_step(1_400_000), 100_000);
        assert_eq!(g.ticks(1_400_000)[10].1, "1100k");
        assert_eq!(g.ticks(1_400_000)[9].1, "1M");
        let g = StripGeom::new(300.0, 60.0, 28.0, 1600, 0.0);
        assert_eq!(g.tick_step(1600), 500);

        let t = table("nom;n\nAnne;1\nAndré;2\nBob;3\nÉmile;4\nZoé;5\nx\n");
        let ids = all(&t);
        let g = StripGeom::new(300.0, 0.0, 60.0, ids.len(), 0.0);
        let rows = g.label_rows(ids.len());
        let labels: Vec<String> = strip_labels(&t.st, &ids, 0, sort_kind(&t.st, 0), &rows).into_iter().map(|x| x.1).collect();
        assert_eq!(labels, vec!["A", "B", "E", "Z", "X"]);
        assert_eq!(sort_kind(&t.st, 1), Kind::Number);
        // The irregular last row marked at its pixel row.
        let irr = Irregular::new(&t.st).unwrap();
        let no = AtomicBool::new(false);
        let bands = strip_marks(&ids, &g, |id| irr.is(id) as u8, &no).unwrap();
        assert_eq!(mark_runs(&bands), vec![(250, 251, 1)]);
        let c = strip_counts(&ids, &g, |id| t.st.cell(id, 0).starts_with('A'), &no).unwrap();
        assert_eq!((c[0], c[50], c.iter().sum::<u32>()), (1, 1, 2));
    }
}
