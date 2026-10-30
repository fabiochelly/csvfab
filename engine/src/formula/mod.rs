//! Formulas: computed columns and the expression filter.
//!
//! A formula is the web app's language (a safe part of JavaScript's
//! expressions, see `parse`): `{Amount}.num() * 1.2`, `num({Amount}) > 1000
//! && contains({City}, "lyon")`, `ifs({N}.num() > 100, "big", "small")`.
//! It is compiled once — columns resolved to the few the formula reads,
//! constants folded, text patterns compiled — then run on every row in
//! parallel. A row reads only those columns, borrowed from the file's bytes;
//! the common formulas allocate nothing per row. A row whose evaluation fails
//! (a method on null, a bad pattern built from a cell) gives an empty cell or
//! fails the filter, and is counted with the first message, as in the web app.

mod date;
pub mod doc;
mod eval;
mod fx;
mod hash;
mod jsre;
mod parse;
mod sound;
mod value;

use crate::table::{RowId, State};
use eval::{Cx, Node, Scratch};
use rayon::prelude::*;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

pub use jsre::TOO_BIG as REGEX_TOO_BIG;
pub use parse::regex_sources;
pub use sound::{phonetic, similarity};

/// Why a formula does not compile: the message the user reads, and where (byte range in the source).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
    pub span: Range<usize>,
}

impl Error {
    /// Nothing typed yet: no message to show, only the box's placeholder.
    pub fn is_empty(&self) -> bool {
        self.msg.is_empty()
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

pub struct Expr {
    root: Node,
    /// slot → column
    cols: Vec<usize>,
    /// For each column up to the last read, its slot (NONE: not read).
    slot_of: Vec<usize>,
    uses_row: bool,
    /// uuid7() counts up: such a formula runs row after row, in order.
    ordered: bool,
    comma: bool,
}

impl std::fmt::Debug for Expr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Expr").field("columns", &self.cols).field("uses_row", &self.uses_row).finish_non_exhaustive()
    }
}

const NONE: usize = usize::MAX;

/// A formula on the columns `headers` ({Name}: exactly, else trimmed, else
/// ignoring case, accents and symbols).
pub fn compile(src: &str, headers: &[String]) -> Result<Expr, Error> {
    build(src, parse::Mode::Cols(headers))
}

/// A formula on one value, `v` (the duplicates' key cleaning: `left(slug(v), 3)`).
pub fn compile_value(src: &str) -> Result<Expr, Error> {
    build(src, parse::Mode::Value)
}

fn build(src: &str, mode: parse::Mode) -> Result<Expr, Error> {
    if src.trim().is_empty() {
        return Err(Error { msg: String::new(), span: 0..0 });
    }
    let p = parse::parse(src, mode)?;
    let root = eval::optimize(p.root, false).map_err(|msg| Error { msg, span: 0..src.len() })?;
    let last = p.cols.iter().max().map_or(0, |&c| c + 1);
    let mut slot_of = vec![NONE; last];
    for (s, &c) in p.cols.iter().enumerate() {
        slot_of[c] = s;
    }
    Ok(Expr { root, cols: p.cols, slot_of, uses_row: p.uses_row, ordered: p.uses.contains(&fx::Fx::Uuid7), comma: false })
}

/// Whether numbers are written with a decimal comma in this file: a ;-separated
/// file follows the French spreadsheet convention.
pub fn decimal_comma(st: &State) -> bool {
    !st.base.dialect.lines && st.base.dialect.delim == b';'
}

/// A pattern written the JavaScript way (as the web app's regex boxes took it), with the guard's bounds.
pub fn js_regex(src: &str, case_insensitive: bool) -> Result<regex::Regex, String> {
    let r = jsre::build(src, if case_insensitive { "i" } else { "" })?;
    match r.re {
        jsre::Engine::Fast(re) => Ok(re),
        jsre::Engine::Fancy(_) => Err("look-ahead, look-behind and back-references (\\1) work in formulas (ƒx), not here".into()),
    }
}

/// The outcome of a run over many rows: rows whose evaluation failed are
/// counted (their cell left empty, or filtered out) with the first message.
#[derive(Debug, Default)]
pub struct Outcome<T> {
    pub out: T,
    pub errors: usize,
    pub first_error: Option<String>,
}

impl Expr {
    /// The columns the formula reads, in the order it names them first (the preview shows them).
    pub fn columns(&self) -> &[usize] {
        &self.cols
    }

    pub fn uses_row(&self) -> bool {
        self.uses_row
    }

    /// Numbers come out with a decimal comma (see `decimal_comma`).
    pub fn set_decimal_comma(&mut self, on: bool) {
        self.comma = on;
    }

    /// One row's value as its cell would get it (the editor's preview).
    pub fn eval_row(&self, st: &State, id: RowId) -> Result<String, String> {
        let mut w = Worker::new(self);
        w.load(st, id);
        let row = if self.uses_row { row_number_of(st, id) } else { f64::NAN };
        w.out(row, date::today())
    }

    /// Whether one row passes the formula as a filter.
    pub fn test_row(&self, st: &State, id: RowId) -> Result<bool, String> {
        let mut w = Worker::new(self);
        w.load(st, id);
        let row = if self.uses_row { row_number_of(st, id) } else { f64::NAN };
        w.test(row, date::today())
    }

    /// The value on a row given as its cells (all columns), `row` its number from 1.
    pub fn eval_cells(&self, cells: &[&str], row: usize) -> Result<String, String> {
        let mut w = Worker::new(self);
        for (s, &c) in self.cols.iter().enumerate() {
            w.cells[s].push_str(cells.get(c).copied().unwrap_or(""));
        }
        w.out(row as f64, date::today())
    }

    /// The value of a `compile_value` formula for `v`.
    pub fn eval_value(&self, v: &str) -> Result<String, String> {
        let mut sc = Scratch::default();
        let cx = Cx { cells: &[], row: f64::NAN, v, comma: self.comma, today: date::today() };
        eval::eval(&self.root, &cx, &mut sc).map(|v| value::into_out(v, self.comma))
    }
}

/// A row's number (1 for the first data row) in file order.
fn row_number_of(st: &State, id: RowId) -> f64 {
    let n = st.len();
    let p = match &st.order {
        None => (id as usize).checked_sub(st.header as usize),
        Some(_) => (0..n).find(|&p| st.data_id(p) == id),
    };
    p.map_or(f64::NAN, |p| (p + 1) as f64)
}

/// Every row's number, by id — built only for a formula reading `row` over a subset of rows.
fn row_numbers(st: &State) -> Vec<u32> {
    let n = st.len();
    let max = (0..n).map(|p| st.data_id(p)).max().map_or(0, |m| m as usize + 1);
    let mut v = vec![0u32; max];
    for p in 0..n {
        v[st.data_id(p) as usize] = p as u32 + 1;
    }
    v
}

/// One thread's state: the cells of the columns read, and its buffers.
struct Worker<'e> {
    e: &'e Expr,
    cells: Vec<String>,
    tmp: Vec<u8>,
    sc: Scratch,
}

impl<'e> Worker<'e> {
    fn new(e: &'e Expr) -> Worker<'e> {
        Worker { e, cells: vec![String::new(); e.cols.len()], tmp: Vec::new(), sc: Scratch::default() }
    }

    /// Reads the row's cells the formula uses: its record scanned once for all of them.
    fn load(&mut self, st: &State, id: RowId) {
        let cells = &mut self.cells;
        st.visit_cols(id, &self.e.cols, |s, v| {
            cells[s].clear();
            cells[s].push_str(v);
        });
    }

    fn out(&mut self, row: f64, today: f64) -> Result<String, String> {
        let cx = Cx { cells: &self.cells, row, v: "", comma: self.e.comma, today };
        eval::eval(&self.e.root, &cx, &mut self.sc).map(|v| value::into_out(v, self.e.comma))
    }

    /// `out`, appended to `buf`: no String of its own per row.
    fn out_into(&mut self, row: f64, today: f64, buf: &mut String) -> Result<(), String> {
        let cx = Cx { cells: &self.cells, row, v: "", comma: self.e.comma, today };
        match eval::eval(&self.e.root, &cx, &mut self.sc)? {
            value::Val::Str(s) => buf.push_str(&s),
            v => value::write_out(&v, self.e.comma, buf),
        }
        Ok(())
    }

    fn test(&mut self, row: f64, today: f64) -> Result<bool, String> {
        let cx = Cx { cells: &self.cells, row, v: "", comma: self.e.comma, today };
        eval::eval(&self.e.root, &cx, &mut self.sc).map(|v| value::truthy(&v))
    }
}

/// Rows per job: small enough to spread over the threads and stop soon when cancelled.
const CHUNK: usize = 8192;

/// The rows to run on, by position: their ids and numbers.
struct Rows<'a> {
    st: &'a State,
    ids: Option<&'a [RowId]>,
    numbers: Option<Vec<u32>>,
}

impl<'a> Rows<'a> {
    fn new(st: &'a State, e: &Expr, ids: Option<&'a [RowId]>) -> Rows<'a> {
        let numbers = (ids.is_some() && e.uses_row).then(|| row_numbers(st));
        Rows { st, ids, numbers }
    }

    fn len(&self) -> usize {
        self.ids.map_or(self.st.len(), |v| v.len())
    }

    #[inline]
    fn at(&self, k: usize) -> (RowId, f64) {
        match self.ids {
            None => (self.st.data_id(k), (k + 1) as f64),
            Some(ids) => {
                let id = ids[k];
                (id, self.numbers.as_ref().map_or(f64::NAN, |n| n.get(id as usize).map_or(f64::NAN, |&x| x as f64)))
            }
        }
    }
}

struct Tally {
    errors: usize,
    first: Option<String>,
}

impl Tally {
    fn new() -> Tally {
        Tally { errors: 0, first: None }
    }

    fn fail(&mut self, e: String) {
        self.errors += 1;
        if self.first.is_none() {
            self.first = Some(e);
        }
    }
}

/// Runs `f` on every chunk of `0..n` — in parallel, or in order for a formula that must —
/// and returns the chunks' results in order; None when cancelled.
fn chunks<T: Send>(e: &Expr, n: usize, cancel: &AtomicBool, f: impl Fn(&mut Worker, Range<usize>) -> T + Sync + Send) -> Option<Vec<T>> {
    let k = n.div_ceil(CHUNK);
    let range = |c: usize| c * CHUNK..((c + 1) * CHUNK).min(n);
    let out: Vec<T> = if e.ordered {
        let mut w = Worker::new(e);
        (0..k).map(|c| f(&mut w, range(c))).collect()
    } else {
        (0..k).into_par_iter().map_init(|| Worker::new(e), |w, c| f(w, range(c))).collect()
    };
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

fn merge<T>(parts: Vec<(T, Tally)>, mut join: impl FnMut(T)) -> (usize, Option<String>) {
    let (mut errors, mut first) = (0, None);
    for (t, tally) in parts {
        join(t);
        errors += tally.errors;
        if first.is_none() {
            first = tally.first;
        }
    }
    (errors, first)
}

/// The formula's value on every row (all data rows, or `rows`), in that order.
pub fn column(st: &State, e: &Expr, rows: Option<&[RowId]>, cancel: &AtomicBool) -> Option<Outcome<Vec<String>>> {
    let rows = Rows::new(st, e, rows);
    let today = date::today();
    let parts = chunks(e, rows.len(), cancel, |w, r| {
        let mut t = Tally::new();
        let mut out = Vec::with_capacity(r.len());
        for k in r {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (id, num) = rows.at(k);
            w.load(st, id);
            out.push(w.out(num, today).unwrap_or_else(|m| {
                t.fail(m);
                String::new()
            }));
        }
        (out, t)
    })?;
    let mut out = Vec::with_capacity(rows.len());
    let (errors, first_error) = merge(parts, |v| out.extend(v));
    Some(Outcome { out, errors, first_error })
}

/// The formula's value on every data row as column `col`'s new cells (`Table::apply`; the
/// empty ones left out): written chunk by chunk into arenas side by side, made one layer
/// in parallel — nothing per row left for the caller to sort, copy or free.
pub fn column_patch(st: &State, e: &Expr, col: usize, cancel: &AtomicBool) -> Option<Outcome<crate::table::Patch>> {
    let rows = Rows::new(st, e, None);
    let today = date::today();
    let parts = chunks(e, rows.len(), cancel, |w, r| {
        let mut t = Tally::new();
        let mut out = crate::table::Cells::with_capacity(r.len());
        let mut buf = String::new();
        for k in r {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (id, num) = rows.at(k);
            w.load(st, id);
            buf.clear();
            if let Err(m) = w.out_into(num, today, &mut buf) {
                t.fail(m);
                buf.clear();
            }
            if !buf.is_empty() {
                out.push(id, &buf);
            }
        }
        (out, t)
    })?;
    let mut pieces = Vec::with_capacity(parts.len());
    let (errors, first_error) = merge(parts, |p| pieces.push(p));
    Some(Outcome { out: crate::table::Patch::column(col, pieces), errors, first_error })
}

/// The cells of column `col` the formula would change (its value differs from
/// the cell's), ready for `Table::set_cells` — one undo step.
pub fn changes(st: &State, e: &Expr, rows: Option<&[RowId]>, col: usize, cancel: &AtomicBool) -> Option<Outcome<Vec<(RowId, usize, String)>>> {
    let rows = Rows::new(st, e, rows);
    let today = date::today();
    let parts = chunks(e, rows.len(), cancel, |w, r| {
        let mut t = Tally::new();
        let mut out = Vec::new();
        for k in r {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (id, num) = rows.at(k);
            w.load(st, id);
            let v = w.out(num, today).unwrap_or_else(|m| {
                t.fail(m);
                String::new()
            });
            let same = match e.slot_of.get(col) {
                Some(&s) if s != NONE => w.cells[s] == v,
                _ => st.with_cell(id, col, &mut w.tmp, |c| c == v),
            };
            if !same {
                out.push((id, col, v));
            }
        }
        (out, t)
    })?;
    let mut out = Vec::new();
    let (errors, first_error) = merge(parts, |v| out.extend(v));
    Some(Outcome { out, errors, first_error })
}

/// How many rows (all data rows, or `rows`) get a value — a non-empty one, or
/// with `col` one differing from that column's cell — without building the
/// column: the computed column's preview, counted on every keystroke.
pub fn tally(st: &State, e: &Expr, rows: Option<&[RowId]>, col: Option<usize>, cancel: &AtomicBool) -> Option<Outcome<usize>> {
    let rows = Rows::new(st, e, rows);
    let today = date::today();
    let parts = chunks(e, rows.len(), cancel, |w, r| {
        let mut t = Tally::new();
        let mut n = 0;
        for k in r {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (id, num) = rows.at(k);
            w.load(st, id);
            let v = w.out(num, today).unwrap_or_else(|m| {
                t.fail(m);
                String::new()
            });
            n += match col {
                None => !v.is_empty(),
                Some(c) => match e.slot_of.get(c) {
                    Some(&s) if s != NONE => w.cells[s] != v,
                    _ => !st.with_cell(id, c, &mut w.tmp, |x| x == v),
                },
            } as usize;
        }
        (n, t)
    })?;
    let mut out = 0;
    let (errors, first_error) = merge(parts, |n| out += n);
    Some(Outcome { out, errors, first_error })
}

/// The rows (all data rows, or among `rows`) for which the formula holds, in
/// order; a row whose evaluation fails is left out and counted.
pub fn filter(st: &State, e: &Expr, rows: Option<&[RowId]>, cancel: &AtomicBool) -> Option<Outcome<Vec<RowId>>> {
    let rows = Rows::new(st, e, rows);
    let today = date::today();
    let parts = chunks(e, rows.len(), cancel, |w, r| {
        let mut t = Tally::new();
        let mut out = Vec::new();
        for k in r {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (id, num) = rows.at(k);
            w.load(st, id);
            match w.test(num, today) {
                Ok(true) => out.push(id),
                Ok(false) => {}
                Err(m) => t.fail(m),
            }
        }
        (out, t)
    })?;
    let mut out = Vec::new();
    let (errors, first_error) = merge(parts, |v| out.extend(v));
    Some(Outcome { out, errors, first_error })
}

#[cfg(test)]
mod tests;
