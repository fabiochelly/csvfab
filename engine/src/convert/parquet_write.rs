//! A table as a Parquet file, typed. Each column's type comes from what the app detects
//! (`view::detect_kinds`, on a sample) and must then hold for **every** value written, the
//! whole column being read first: one value it cannot hold and the column is a string. A
//! type holds a value only if reopening the file gives the very cell back, but for the
//! decimal mark (the import writes the one of the CSV's delimiter) and a negative zero:
//!
//! - integer: `-?[1-9]\d*` or `0`, within 64 bits: not `007`, not `+5`, not `1 234`;
//! - float: the cell is the double's own shortest text (`1.5`, `3`, not `1.50`, not 17+ digits
//!   rounded);
//! - decimal: otherwise, every cell with the same number of decimals, at most 38 digits
//!   (`12.50` and `3.10`, or a 20-digit number);
//! - boolean: `true`/`false`, or in capitals, or `True`/`False`, one way for the column;
//! - date: `yyyy-mm-dd`, a real day. `31/12/2024` stays a string: day and month may be either
//!   way round, and the cell would come back written otherwise;
//! - timestamp: `yyyy-mm-dd hh:mm[:ss[.f]]` or with a `T`, the same way all along the column;
//!   a final `Z` on every cell makes it an instant (UTC), none a timestamp without a zone; any
//!   other offset stays a string (the instant would hold, the offset not).
//!
//! A negative zero (`-0`, `-0.00`) fits every number type, whatever its decimals: the user's
//! choice, so that a rounded `-0.00` does not make a column of amounts a string. It is
//! written as a plain zero and reopens as one (`0`, `0.00`), the one cell that comes back
//! changed. A number with a comma is a decimal comma, but a column whose marked cells all read as
//! thousands (`1,234`, or `1.234` in a `;` file, where dots group thousands) stays a string:
//! neither reading can be told. Empty cells are nulls; a string column has them too.
//!
//! Row groups are encoded in waves, a wave's groups and each group's columns in parallel,
//! while the thread of its own writes the wave before. zstd, level 1: see docs/perf.md.

use super::parquet_read::{push_float, Shape, CELLS_KEY};
use super::{check, days_from_civil, days_in_month, tick, write_beside, Exported, Rows};
use crate::table::{RowId, State};
use crate::view::{self, Kind};
use arrow_array::builder::{BooleanBuilder, Date32Builder, Decimal128Builder, Float64Builder, Int64Builder, LargeStringBuilder};
use arrow_array::{ArrayRef, Int64Array, TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray};
use arrow_schema::{DataType, Field, FieldRef, Schema, TimeUnit};
use parquet::arrow::arrow_writer::{compute_leaves, ArrowColumnChunk, ArrowColumnWriter, ArrowRowGroupWriterFactory};
use parquet::arrow::ArrowSchemaConverter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::file::writer::SerializedFileWriter;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;

/// Rows of a row group, at most; fewer when the rows are long, so that a group stays near
/// `GROUP_BYTES` of text (a wave holds as many groups as threads).
const GROUP_ROWS: usize = 122_880;
const GROUP_BYTES: usize = 64 << 20;
const ZSTD_LEVEL: i32 = 1;

fn failed(e: impl std::fmt::Display) -> String {
    format!("Parquet: {e}")
}

/// The most exact type of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    Int,
    Float,
    /// Precision, scale.
    Decimal(u8, u8),
    Bool(Shape),
    Date,
    /// `z`: an instant, UTC.
    Stamp { shape: Shape, z: bool },
    Text,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Int => "INT64",
            Ty::Float => "DOUBLE",
            Ty::Decimal(..) => "DECIMAL",
            Ty::Bool(_) => "BOOLEAN",
            Ty::Date => "DATE",
            Ty::Stamp { .. } => "TIMESTAMP",
            Ty::Text => "STRING",
        }
    }

    fn arrow(self) -> DataType {
        match self {
            Ty::Int => DataType::Int64,
            Ty::Float => DataType::Float64,
            Ty::Decimal(p, s) => DataType::Decimal128(p, s as i8),
            Ty::Bool(_) => DataType::Boolean,
            Ty::Date => DataType::Date32,
            Ty::Stamp { shape, z } => DataType::Timestamp(unit(shape).0, z.then(|| "UTC".into())),
            Ty::Text => DataType::LargeUtf8,
        }
    }

    fn shape(self) -> Shape {
        match self {
            Ty::Bool(s) | Ty::Stamp { shape: s, .. } => s,
            _ => Shape::Plain,
        }
    }
}

/// The time unit a timestamp's digits need, and how many of it make a second.
fn unit(shape: Shape) -> (TimeUnit, i64) {
    match shape {
        Shape::Stamp { digits: 7.., .. } => (TimeUnit::Nanosecond, 1_000_000_000),
        Shape::Stamp { digits: 4..=6, .. } => (TimeUnit::Microsecond, 1_000_000),
        _ => (TimeUnit::Millisecond, 1000),
    }
}

// --- reading the cells -----------------------------------------------------------

/// `yyyy-mm-dd`, a real day: days since 1970-01-01.
fn date_days(b: &[u8]) -> Option<i64> {
    let d = |i: usize| b.get(i).filter(|c| c.is_ascii_digit()).map(|c| (c - b'0') as u32);
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' { return None; }
    let y = d(0)? * 1000 + d(1)? * 100 + d(2)? * 10 + d(3)?;
    let (m, day) = (d(5)? * 10 + d(6)?, d(8)? * 10 + d(9)?);
    ((1..=12).contains(&m) && (1..=days_in_month(y as i64, m)).contains(&day)).then(|| days_from_civil(y as i64, m, day))
}

/// A timestamp's value in its unit, and the way it is written. None: not one, or too far for
/// nanoseconds.
fn stamp(b: &[u8]) -> Option<(i64, Shape, bool)> {
    let days = date_days(b.get(..10)?)?;
    let t = match b.get(10)? { b'T' => true, b' ' => false, _ => return None };
    let (b, z) = match b.strip_suffix(b"Z") { Some(r) => (&r[11..], true), None => (&b[11..], false) };
    let two = |i: usize, max: u32| -> Option<u32> {
        let (a, c) = (*b.get(i)?, *b.get(i + 1)?);
        let n = (a.wrapping_sub(b'0') as u32) * 10 + c.wrapping_sub(b'0') as u32;
        (a.is_ascii_digit() && c.is_ascii_digit() && n <= max).then_some(n)
    };
    let mut secs = days * 86400 + two(0, 23)? as i64 * 3600 + two(3, 59)? as i64 * 60;
    if b.get(2) != Some(&b':') { return None; }
    let shape;
    let mut frac: &[u8] = b"";
    match b.get(5) {
        None => shape = Shape::Stamp { t, secs: false, digits: 0 },
        Some(b':') => {
            secs += two(6, 59)? as i64;
            let digits = match b.get(8) {
                None => 0,
                Some(b'.') => {
                    frac = &b[9..];
                    if frac.is_empty() || frac.len() > 9 || !frac.iter().all(u8::is_ascii_digit) { return None; }
                    frac.len() as u8
                }
                _ => return None,
            };
            shape = Shape::Stamp { t, secs: true, digits };
        }
        _ => return None,
    }
    let per = unit(shape).1;
    let mut part = 0i64;
    let mut scale = per;
    for &c in frac {
        scale /= 10;
        part += (c - b'0') as i64 * scale;
    }
    Some((secs.checked_mul(per)?.checked_add(part)?, shape, z))
}

/// A number written plainly: sign, integer digits, mark, decimals. None: anything else — a
/// leading zero, a plus sign, spaces, a group separator.
fn number(b: &[u8]) -> Option<(bool, &[u8], Option<u8>, &[u8])> {
    let (neg, b) = match b.split_first() { Some((b'-', r)) => (true, r), _ => (false, b) };
    let n = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 || (b[0] == b'0' && n > 1) { return None; }
    let (int, rest) = b.split_at(n);
    match rest.split_first() {
        None => Some((neg, int, None, b"")),
        Some((&m @ (b'.' | b','), frac)) if !frac.is_empty() && frac.iter().all(u8::is_ascii_digit) => Some((neg, int, Some(m), frac)),
        _ => None,
    }
}

/// `v` with a point for its decimal mark, on the stack: Rust parses no comma.
fn to_f64(v: &str) -> Option<f64> {
    if !v.contains(',') { return v.parse().ok(); }
    let mut b = [0u8; 64];
    if v.len() > b.len() { return None; }
    b[..v.len()].copy_from_slice(v.as_bytes());
    if let Some(p) = b[..v.len()].iter_mut().find(|c| **c == b',') { *p = b'.'; }
    std::str::from_utf8(&b[..v.len()]).ok()?.parse().ok()
}

/// Whether the double read from `v` is written `v` again.
fn float_holds(v: &str) -> bool {
    let Some(x) = to_f64(v).filter(|x| x.is_finite()) else { return false };
    let mut s = String::with_capacity(v.len() + 8);
    push_float(x, v.contains(','), &mut s);
    s == v
}

/// The digits of a decimal's cell as one integer.
fn unscaled(v: &str) -> Option<i128> {
    let (neg, int, _, frac) = number(v.as_bytes())?;
    let mut n = 0i128;
    for &c in int.iter().chain(frac) {
        n = n.checked_mul(10)?.checked_add((c - b'0') as i128)?;
    }
    Some(if neg { -n } else { n })
}

/// What the cells of a column seen so far allow.
#[derive(Clone)]
struct Seen {
    filled: bool,
    int: bool,
    float: bool,
    dec: bool,
    date: bool,
    stamp: bool,
    /// Booleans, each way of writing them seen (bits).
    bools: bool,
    case: u8,
    scale: Option<u32>,
    /// Digits before the mark, at most.
    head: u32,
    dot: bool,
    comma: bool,
    /// Every cell with a mark reads as thousands too.
    grouped: bool,
    shape: Option<(Shape, bool)>,
}

impl Seen {
    fn new(k: Kind) -> Seen {
        let (num, date) = (k == Kind::Number, k == Kind::Date);
        Seen { filled: false, int: num, float: num, dec: num, date, stamp: date, bools: k == Kind::Text, case: 0, scale: None, head: 0, dot: false, comma: false, grouped: true, shape: None }
    }

    fn dead(&self) -> bool {
        !(self.int || self.float || self.dec || self.date || self.stamp || self.bools)
    }

    fn see(&mut self, v: &str) {
        if v.is_empty() { return; }
        self.filled = true;
        if self.int || self.float || self.dec { self.see_number(v); }
        if self.date || self.stamp { self.see_date(v.as_bytes()); }
        if self.bools {
            match v {
                "true" | "false" => self.case |= 1,
                "TRUE" | "FALSE" => self.case |= 2,
                "True" | "False" => self.case |= 4,
                _ => self.bools = false,
            }
            if !self.case.is_power_of_two() { self.bools = false; }
        }
    }

    fn see_number(&mut self, v: &str) {
        let Some((neg, int, mark, frac)) = number(v.as_bytes()) else {
            // An exponent, as the import writes the values beyond 10^±21.
            self.int = false;
            self.dec = false;
            self.float = self.float && float_holds(v);
            return;
        };
        let zero = int.iter().chain(frac).all(|&c| c == b'0');
        // A negative zero fits, as a plain zero: its decimals and mark settle nothing.
        if neg && zero {
            self.int = self.int && mark.is_none();
            return;
        }
        match mark {
            None => self.int = self.int && v.parse::<i64>().is_ok(),
            Some(m) => {
                self.int = false;
                if m == b'.' { self.dot = true; } else { self.comma = true; }
                self.grouped = self.grouped && int.len() <= 3 && frac.len() == 3;
            }
        }
        // Up to 15 digits, a double gives its text back whole, trailing zeros aside.
        if self.float && (int.len() + frac.len() > 15 || frac.last() == Some(&b'0')) {
            self.float = float_holds(v);
        }
        if self.dec {
            let sc = frac.len() as u32;
            self.head = self.head.max(int.len() as u32);
            self.dec = self.scale.is_none_or(|s| s == sc) && self.head + sc <= 38;
            self.scale = Some(sc);
        }
    }

    fn see_date(&mut self, b: &[u8]) {
        if self.date { self.date = date_days(b).is_some(); }
        if self.stamp {
            match stamp(b) {
                Some((_, sh, z)) if self.shape.is_none_or(|x| x == (sh, z)) => self.shape = Some((sh, z)),
                _ => self.stamp = false,
            }
        }
    }

    fn merge(&mut self, o: &Seen) {
        self.filled |= o.filled;
        self.int &= o.int;
        self.float &= o.float;
        self.dec &= o.dec;
        self.date &= o.date;
        self.stamp &= o.stamp;
        self.bools &= o.bools;
        self.case |= o.case;
        self.head = self.head.max(o.head);
        self.dot |= o.dot;
        self.comma |= o.comma;
        self.grouped &= o.grouped;
        if let (Some(a), Some(b)) = (self.scale, o.scale) { self.dec &= a == b; }
        self.scale = self.scale.or(o.scale);
        if let (Some(a), Some(b)) = (self.shape, o.shape) { self.stamp &= a == b; }
        self.shape = self.shape.or(o.shape);
    }

    /// What the whole column allows. `semi`: a `;`-delimited table.
    fn ty(&self, semi: bool) -> Ty {
        if !self.filled { return Ty::Text; }
        let marked = self.dot || self.comma;
        let unclear = (self.dot && self.comma) || (marked && self.grouped && (self.comma || semi));
        if self.int { return Ty::Int; }
        if !unclear && self.float { return Ty::Float; }
        if !unclear && self.dec { return Ty::Decimal((self.head + self.scale.unwrap_or(0)).clamp(1, 38) as u8, self.scale.unwrap_or(0) as u8); }
        if self.bools && self.case.is_power_of_two() {
            return Ty::Bool(match self.case { 2 => Shape::Upper, 4 => Shape::Title, _ => Shape::Plain });
        }
        if self.date { return Ty::Date; }
        if let (true, Some((shape, z))) = (self.stamp, self.shape) { return Ty::Stamp { shape, z }; }
        Ty::Text
    }
}

/// Column types over `n` rows: a sample settles most columns, the others are read whole, in
/// parallel. Progress: `n` rows.
fn types(st: &State, rows: &Rows, n: usize, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Vec<Ty>, String> {
    const CHUNK: usize = 8192;
    let width = st.ncols;
    let semi = st.base.dialect.delim == b';';
    let kinds = view::detect_kinds(st);
    let mut first: Vec<Seen> = (0..width).map(|c| Seen::new(kinds.get(c).copied().unwrap_or(Kind::Text))).collect();
    for k in 0..4096.min(n) {
        st.visit(rows.id(k * n / 4096.min(n)), |c, v| if let Some(x) = first.get_mut(c) { x.see(v) });
    }
    let live: Vec<usize> = (0..width).filter(|&c| !first[c].dead()).collect();
    if live.is_empty() {
        tick(progress, n);
    } else {
        let seed: Vec<Seen> = live.iter().map(|&c| first[c].clone()).collect();
        let parts: Vec<Vec<Seen>> = (0..n.div_ceil(CHUNK)).into_par_iter().map(|k| {
            let mut s = seed.clone();
            if check(cancel).is_err() { return s; }
            let end = ((k + 1) * CHUNK).min(n);
            for pos in k * CHUNK..end {
                st.visit_cols(rows.id(pos), &live, |j, v| s[j].see(v));
            }
            tick(progress, end - k * CHUNK);
            s
        }).collect();
        check(cancel)?;
        for p in &parts {
            for (&c, s) in live.iter().zip(p) { first[c].merge(s); }
        }
    }
    Ok(first.iter().map(|s| s.ty(semi)).collect())
}

/// Column names: the header's as written, `colN` when empty, `_2`, `_3`… when repeated.
fn column_names(header: Option<&[String]>, width: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    (0..width).map(|c| {
        let t = header.and_then(|h| h.get(c)).map_or("", |s| s.as_str());
        let base = if t.trim().is_empty() { format!("col{}", c + 1) } else { t.to_string() };
        let (mut n, mut i) = (base.clone(), 2);
        while !seen.insert(n.clone()) {
            n = format!("{base}_{i}");
            i += 1;
        }
        n
    }).collect()
}

// --- writing --------------------------------------------------------------------

/// A column of a row group, being filled.
enum Col {
    Int(Int64Builder),
    Float(Float64Builder),
    Dec(Decimal128Builder),
    Bool(BooleanBuilder),
    Date(Date32Builder),
    Stamp(Int64Builder),
    Text(LargeStringBuilder),
}

impl Col {
    fn new(t: Ty, rows: usize) -> Col {
        match t {
            Ty::Int => Col::Int(Int64Builder::with_capacity(rows)),
            Ty::Float => Col::Float(Float64Builder::with_capacity(rows)),
            Ty::Decimal(..) => Col::Dec(Decimal128Builder::with_capacity(rows).with_data_type(t.arrow())),
            Ty::Bool(_) => Col::Bool(BooleanBuilder::with_capacity(rows)),
            Ty::Date => Col::Date(Date32Builder::with_capacity(rows)),
            Ty::Stamp { .. } => Col::Stamp(Int64Builder::with_capacity(rows)),
            Ty::Text => Col::Text(LargeStringBuilder::new()),
        }
    }

    /// False: the cell is no longer what the first pass read (it cannot happen on a snapshot).
    #[inline]
    fn push(&mut self, v: &str) -> bool {
        if v.is_empty() { self.null(); return true; }
        match self {
            Col::Int(b) => v.parse().map(|x| b.append_value(x)).is_ok(),
            // + 0.0: a plain zero for a negative one.
            Col::Float(b) => to_f64(v).map(|x| b.append_value(x + 0.0)).is_some(),
            Col::Dec(b) => unscaled(v).map(|x| b.append_value(x)).is_some(),
            Col::Bool(b) => { b.append_value(v.as_bytes()[0] | 32 == b't'); true }
            Col::Date(b) => date_days(v.as_bytes()).map(|x| b.append_value(x as i32)).is_some(),
            Col::Stamp(b) => stamp(v.as_bytes()).map(|x| b.append_value(x.0)).is_some(),
            Col::Text(b) => { b.append_value(v); true }
        }
    }

    #[inline]
    fn null(&mut self) {
        match self {
            Col::Int(b) | Col::Stamp(b) => b.append_null(),
            Col::Float(b) => b.append_null(),
            Col::Dec(b) => b.append_null(),
            Col::Bool(b) => b.append_null(),
            Col::Date(b) => b.append_null(),
            Col::Text(b) => b.append_null(),
        }
    }

    fn finish(self, t: Ty) -> ArrayRef {
        match self {
            Col::Int(mut b) => Arc::new(b.finish()),
            Col::Float(mut b) => Arc::new(b.finish()),
            Col::Dec(mut b) => Arc::new(b.finish()),
            Col::Bool(mut b) => Arc::new(b.finish()),
            Col::Date(mut b) => Arc::new(b.finish()),
            Col::Text(mut b) => Arc::new(b.finish()),
            Col::Stamp(mut b) => {
                let a: Int64Array = b.finish();
                let (_, vals, nulls) = a.into_parts();
                let tz = match t { Ty::Stamp { z: true, .. } => Some("UTC"), _ => None };
                match unit(t.shape()).0 {
                    TimeUnit::Nanosecond => Arc::new(TimestampNanosecondArray::new(vals, nulls).with_timezone_opt(tz)),
                    TimeUnit::Microsecond => Arc::new(TimestampMicrosecondArray::new(vals, nulls).with_timezone_opt(tz)),
                    _ => Arc::new(TimestampMillisecondArray::new(vals, nulls).with_timezone_opt(tz)),
                }
            }
        }
    }
}

struct Encoder<'a> {
    st: &'a State,
    rows: &'a Rows<'a>,
    types: &'a [Ty],
    fields: &'a [FieldRef],
    factory: &'a ArrowRowGroupWriterFactory,
    group_rows: usize,
}

impl Encoder<'_> {
    /// Row group `g`: its columns filled, then encoded in parallel.
    fn group(&self, g: usize, n: usize, cancel: &AtomicBool) -> Result<Vec<ArrowColumnChunk>, String> {
        check(cancel)?;
        let (from, to) = (g * self.group_rows, ((g + 1) * self.group_rows).min(n));
        let mut cols: Vec<Col> = self.types.iter().map(|&t| Col::new(t, to - from)).collect();
        let width = cols.len();
        let mut bad = false;
        for pos in from..to {
            if pos & 4095 == 0 { check(cancel)?; }
            // A row short of cells (a ragged file, a column added): nulls.
            let mut next = 0;
            self.st.visit(self.rows.id(pos), |c, v| {
                if c >= width { return; }
                for k in next..c { cols[k].null(); }
                bad |= !cols[c].push(v);
                next = c + 1;
            });
            for c in cols.iter_mut().skip(next) { c.null(); }
        }
        if bad { return Err(failed("a cell changed while it was written")); }
        let writers = self.factory.create_column_writers(g).map_err(failed)?;
        let arrays: Vec<(ArrayRef, usize)> = cols.into_iter().zip(self.types).enumerate().map(|(c, (b, &t))| (b.finish(t), c)).collect();
        writers.into_par_iter().zip(arrays).map(|(w, (a, c)): (ArrowColumnWriter, _)| {
            let mut w = w;
            for leaf in compute_leaves(&self.fields[c], &a).map_err(failed)? {
                w.write(&leaf).map_err(failed)?;
            }
            w.close().map_err(failed)
        }).collect()
    }
}

/// Writes the rows to `dest`, atomically. See [`super::export`]. Progress: rows done, twice
/// over (typing, then writing).
pub fn write(st: &State, rows: Option<&[RowId]>, dest: &Path, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    let rows = Rows { st, ids: rows };
    let n = rows.len();
    let width = st.ncols;
    if width == 0 { return Err(failed("the table has no column")); }
    let types = types(st, &rows, n, progress, cancel)?;
    let header = st.header.then(|| st.headers());
    let names = column_names(header.as_deref(), width);
    let fields: Vec<FieldRef> = names.iter().zip(&types).map(|(c, t)| Arc::new(Field::new(c, t.arrow(), true))).collect();
    let schema = Arc::new(Schema::new(fields.clone()));
    let shapes: Vec<String> = types.iter().map(|t| t.shape().code()).collect();
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(ZSTD_LEVEL).map_err(failed)?))
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_created_by(format!("csvfab {}", env!("CARGO_PKG_VERSION")))
        .set_key_value_metadata(shapes.iter().any(|s| !s.is_empty()).then(|| vec![KeyValue::new(CELLS_KEY.to_string(), shapes.join(","))]))
        .build();
    let props = Arc::new(props);
    let pq = ArrowSchemaConverter::new().with_coerce_types(props.coerce_types()).convert(&schema).map_err(failed)?;
    // By the file's bytes per record: a table's edits rarely change its rows' length.
    let per_row = (st.base.bytes().len() / st.base.n.max(1)).max(1);
    let group_rows = (GROUP_BYTES / per_row).clamp(1024, GROUP_ROWS);
    let threads = rayon::current_num_threads().max(1);
    write_beside(dest, false, |w| {
        let mut file = SerializedFileWriter::new(w, pq.root_schema_ptr(), props.clone()).map_err(failed)?;
        let factory = ArrowRowGroupWriterFactory::new(&file, schema.clone());
        let enc = Encoder { st, rows: &rows, types: &types, fields: &fields, factory: &factory, group_rows };
        let groups = n.div_ceil(group_rows);
        std::thread::scope(|s| {
            let (tx, rx) = sync_channel::<Vec<ArrowColumnChunk>>(threads);
            let written = s.spawn(move || -> Result<(), String> {
                for chunks in rx {
                    let mut rg = file.next_row_group().map_err(failed)?;
                    for c in chunks { c.append_to_row_group(&mut rg).map_err(failed)?; }
                    rg.close().map_err(failed)?;
                }
                file.close().map_err(failed)?;
                Ok(())
            });
            let made = (|| -> Result<(), String> {
                for wave in (0..groups).collect::<Vec<_>>().chunks(threads) {
                    let done: Vec<_> = wave.par_iter().map(|&g| enc.group(g, n, cancel)).collect();
                    for (k, r) in done.into_iter().enumerate() {
                        // The writer stopped: its error says why.
                        if tx.send(r?).is_err() { return Ok(()); }
                        tick(progress, (group_rows * (wave[k] + 1)).min(n) - group_rows * wave[k]);
                    }
                }
                Ok(())
            })();
            drop(tx);
            let wrote = written.join().unwrap_or_else(|_| Err(failed("the writing stopped")));
            made.and(wrote)
        })
    })?;
    Ok(Exported { rows: n as u64, cols: width, types: names.into_iter().zip(types.iter().map(|t| t.name())).collect(), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::super::parquet_read;
    use super::super::{export, import, Format, Imported};
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::kinds;
    use crate::table::Table;
    use arrow_array::cast::AsArray;
    use arrow_array::{Array, RecordBatch};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::basic::{ConvertedType, LogicalType, Type as Physical};
    use std::fs::File;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        super::super::xlsx_read::tests::tmpdir(tag)
    }

    fn table(csv: &str, delim: u8, header: bool) -> Table {
        Table::new(Arc::new(Base::from_bytes(csv.as_bytes().to_vec(), OpenOptions { delim: Some(delim), ..Default::default() })), Some(header))
    }

    fn out(t: &Table, dest: &Path) -> Exported {
        write(&t.st, None, dest, None, &AtomicBool::new(false)).unwrap()
    }

    fn back(p: &Path, delim: u8) -> (String, Imported) {
        let mut o = Vec::new();
        let info = parquet_read::to_csv(p, delim, &mut o, None, &AtomicBool::new(false)).unwrap();
        (String::from_utf8(o).unwrap(), info)
    }

    fn types_of(info: &Exported) -> Vec<&'static str> {
        info.types.iter().map(|t| t.1).collect()
    }

    /// The batches as a plain Arrow reader sees them (no csvfab on the way).
    fn batches(p: &Path) -> Vec<RecordBatch> {
        ParquetRecordBatchReaderBuilder::try_new(File::open(p).unwrap()).unwrap().build().unwrap().map(|b| b.unwrap()).collect()
    }

    /// Exports `csv`, checks the types, and that reopening gives it back.
    fn trip(csv: &str, delim: u8, want: &[&str]) -> Exported {
        let dir = tmpdir(&format!("pqw-{}", std::thread::current().name().unwrap_or("t").replace("::", "-")));
        let dest = dir.join("t.parquet");
        let info = out(&table(csv, delim, true), &dest);
        assert_eq!(types_of(&info), want, "{csv}");
        assert_eq!(back(&dest, delim).0, csv);
        info
    }

    #[test]
    fn every_type_round_trips() {
        let csv = "\
id;f;amount;big;ok;OK;Ok;day;at;t;msz;us;ns;zone;name
1;1,5;12,50;12345678901234567890123456;true;TRUE;True;2024-03-01;2024-03-01 10:30:15;2024-03-01T10:30:15;2024-03-01T10:30:15.120Z;2024-03-01 10:30:15.000120;2024-03-01 10:30:15.123456789;2024-03-01T10:30:15Z;Zoé
-2;-0,001;0,05;-1;false;FALSE;False;1969-12-31;1900-01-01 00:00:00;2024-03-01T10:30:15;2024-03-01T10:30:15.000Z;2024-03-01 10:30:15.000000;2024-03-01 10:30:15.000000000;2024-03-01T23:59:59Z;日本語
;;;;;;;;;;;;;;
9223372036854775807;3;-3,10;0;true;TRUE;True;9999-12-31;9999-12-31 23:59:59;1000-01-01T00:00:00;2024-03-01T10:30:15.999Z;2024-03-01 10:30:15.999999;2024-03-01 10:30:15.999999999;2024-03-01T00:00:00Z;\"a;b\"\n";
        let info = trip(csv, b';', &["INT64", "DOUBLE", "DECIMAL", "DECIMAL", "BOOLEAN", "BOOLEAN", "BOOLEAN", "DATE", "TIMESTAMP", "TIMESTAMP", "TIMESTAMP", "TIMESTAMP", "TIMESTAMP", "TIMESTAMP", "STRING"]);
        assert_eq!(info.rows, 4);
    }

    #[test]
    fn dots_in_a_comma_file() {
        trip("a,b,c\n1.5,12.50,0.1\n2.25,3.10,0.30000000000000004\n1e+300,0.00,5\n", b',', &["DOUBLE", "DECIMAL", "DOUBLE"]);
    }

    #[test]
    fn physical_types_for_any_reader() {
        let dir = tmpdir("pqw-phys");
        let dest = dir.join("t.parquet");
        let csv = "i;f;d;b;day;ts;tz;s\n1;1,5;1,50;true;2024-03-01;2024-03-01 10:30:15;2024-03-01T10:30:15Z;x\n;;;;;;;\n";
        out(&table(csv, b';', true), &dest);
        let r = ParquetRecordBatchReaderBuilder::try_new(File::open(&dest).unwrap()).unwrap();
        let d = r.metadata().file_metadata().schema_descr_ptr();
        let phys: Vec<Physical> = (0..8).map(|c| d.column(c).physical_type()).collect();
        assert_eq!(phys, [Physical::INT64, Physical::DOUBLE, Physical::INT32, Physical::BOOLEAN, Physical::INT32, Physical::INT64, Physical::INT64, Physical::BYTE_ARRAY]);
        assert_eq!(d.column(2).converted_type(), ConvertedType::DECIMAL);
        assert_eq!((d.column(2).type_precision(), d.column(2).type_scale()), (3, 2));
        assert_eq!(d.column(4).converted_type(), ConvertedType::DATE);
        let ts = |c: usize| match d.column(c).logical_type_ref() { Some(LogicalType::Timestamp(t)) => t.is_adjusted_to_u_t_c, x => panic!("{x:?}") };
        assert!(!ts(5) && ts(6));
        assert_eq!(d.column(7).converted_type(), ConvertedType::UTF8);
        drop(r);
        let b = batches(&dest);
        assert_eq!(b[0].num_rows(), 2);
        // Empty cells are nulls, in every column.
        for c in b[0].columns() { assert_eq!(c.null_count(), 1); }
    }

    #[test]
    fn one_value_that_does_not_fit_makes_a_string() {
        for (col, why) in [
            ("1\n2\n007\n", "leading zero"), ("1\n+2\n", "plus sign"), ("1\n 2\n", "space"), ("1\n2 \n", "trailing space"), ("1\n1 234\n", "group"),
            ("9223372036854775807\n9223372036854775808\nabc\n", "past 64 bits"), ("1\nabc\n", "text"),
            ("1.5\n1.50\n2.125\n", "a trailing zero, decimals differing"), ("1.5\n0.30000000000000000001\n", "digits a double rounds"),
            ("1\nnan\n", "not a number"), ("1.5\n2,5\n", "two marks"), ("1,234\n5,678\n12\n", "thousands"), ("1e5\n2e5\n", "not the double's own text"), (".5\n.25\n", "no integer digit"),
            ("1.\n2.\n", "mark without decimals"), ("0.123456789012345678901234567890123456789\n0.5\n", "too long"),
            ("2024-02-30\n2024-02-28\n", "no such day"), ("2024-03-01\n01/03/2024\n", "two forms"), ("2024-03-01\n2024-3-2\n", "not padded"), ("2024-03-01\n2024-03-01 10:00:00\n", "date and timestamp"),
            ("2024-03-01 10:30:15\n2024-03-01T10:30:15\n", "two separators"), ("2024-03-01 25:00:00\n2024-03-01 10:30:00\n", "no such hour"), ("2024-03-01 10:30:60\n2024-03-01 10:30:00\n", "a leap second"),
            ("2024-03-01 10:30:15\n2024-03-01 10:30:15.5\n", "digits differ"), ("2024-03-01T10:30:15Z\n2024-03-01T10:30:15\n", "with and without a zone"), ("2024-03-01T10:30:15+02:00\n2024-03-01T11:30:15+02:00\n", "an offset"),
            ("2024-03-01 10:30:15.123456789\n1500-03-01 10:30:15.123456789\n", "too far for nanoseconds"), ("true\nfalse\nTRUE\n", "two cases"), ("true\nfalse\nyes\n", "not a boolean"), ("true\n1\n", "number"),
            ("31/12/2024\n01/02/2024\n", "day and month"), ("35,61 €\n1,00 €\n", "currency"), ("12 %\n3 %\n", "percent"), ("10:15\n23:59\n", "a time"),
        ] {
            let csv = format!("c\n{col}");
            let dir = tmpdir("pqw-fit");
            let dest = dir.join("t.parquet");
            let info = out(&table(&csv, b';', true), &dest);
            assert_eq!(types_of(&info), ["STRING"], "{why}");
            assert_eq!(back(&dest, b';').0, csv, "{why}");
        }
    }

    #[test]
    fn a_negative_zero_is_a_plain_zero() {
        let dir = tmpdir("pqw-zero");
        let dest = dir.join("t.parquet");
        // Any decimals, either mark: the column keeps its type, the cell comes back a plain zero.
        let csv = "int;dec;dbl;other\n1;12,50;1,5;-0\n-0;-0,00;-0,0;x\n3;-0;-0;\n4;3,10;2,25;\n";
        let info = out(&table(csv, b';', true), &dest);
        assert_eq!(types_of(&info), ["INT64", "DECIMAL", "DOUBLE", "STRING"]);
        // Only in a number: a string keeps its cell.
        assert_eq!(back(&dest, b';').0, "int;dec;dbl;other\n1;12,50;1,5;-0\n0;0,00;0;x\n3;0,00;0;\n4;3,10;2,25;\n");
        // Stored as a plain zero: no reader sees a sign.
        let b = batches(&dest);
        let f = b[0].column(2).as_primitive::<arrow_array::types::Float64Type>();
        assert!(f.value(1).is_sign_positive() && f.value(2).is_sign_positive() && f.value(1) == 0.0);
        // A column of nothing else stays a number; with decimals, not an integer.
        let info = out(&table("a;b\n-0;-0,00\n-0;-0,00\n", b';', true), &dest);
        assert_eq!(types_of(&info), ["INT64", "DOUBLE"]);
        assert_eq!(back(&dest, b';').0, "a;b\n0;0\n0;0\n");
    }

    #[test]
    fn a_table_without_columns_is_written_by_every_export() {
        // Edited, moved, and a column gone: each writer gets the cells of the columns left.
        let mut t = table("id;secret;name\n1;x;a\n2;y;b\n3;z;c\n", b';', true);
        t.set_cells(&[(t.st.data_id(1), 2, "B".into())]);
        t.move_column(2, 0);
        let st = t.st.without_columns(|c| c == 2);
        assert_eq!((st.ncols, st.headers()), (2, vec!["name".to_string(), "id".to_string()]));
        let rows = [t.st.data_id(2), t.st.data_id(1)];
        let dir = tmpdir("pqw-project");
        for (f, file) in [(Format::Xlsx, "t.xlsx"), (Format::Sqlite, "t.sqlite"), (Format::Parquet, "t.parquet")] {
            let dest = dir.join(file);
            export(f, &st, Some(&rows), &dest, "t", None, &AtomicBool::new(false)).unwrap();
            let csv = dir.join("back.csv");
            import(f, &dest, None, b';', &csv, false, None, &AtomicBool::new(false)).unwrap();
            assert_eq!(std::fs::read_to_string(&csv).unwrap(), "name;id\nc;3\nB;2\n", "{f:?}");
        }
        let o = crate::save::SaveOptions { rows: Some(rows.to_vec()), backup: false, ..Default::default() };
        let csv = dir.join("rows.csv");
        crate::save::save(&st, &csv, &o, None, &mut true).unwrap();
        assert_eq!(std::fs::read_to_string(&csv).unwrap(), "name;id\nc;3\nB;2\n");
        // Split: by a column of the table, the column gone or not; the files hold the others.
        let no = AtomicBool::new(false);
        let groups = crate::split::groups(&t.st, &rows, 2, crate::split::Match::Exact, "t.csv", &no, None).unwrap();
        let split = crate::split::write(&st, &groups, &dir, "parts", &no, None);
        assert_eq!((split.written, split.error.is_none()), (2, true));
        let mut files: Vec<String> = std::fs::read_dir(dir.join("parts")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
        files.sort();
        assert_eq!(files, ["name;id\nB;2\n", "name;id\nc;3\n"]);
        // Combine.
        let src = crate::combine::Source { name: "t".into(), st: st.clone(), rows: Some(Arc::new(rows.to_vec())) };
        let (sources, both) = (vec![src], dir.join("both.csv"));
        let cols = crate::combine::columns(&sources, true, false);
        crate::combine::write(&sources, &cols, false, &crate::combine::format(&sources), &both, &no, None).unwrap();
        assert_eq!(std::fs::read_to_string(&both).unwrap(), "name;id\nc;3\nB;2\n");
        // The table itself still holds them all.
        assert_eq!(t.st.ncols, 3);
    }

    #[test]
    fn a_late_value_counts() {
        // The sample sees the first rows and others: the odd one out is found by the whole pass.
        let mut csv = String::from("a,b\n");
        for i in 0..200_000 { csv += &format!("{i},2024-01-01\n"); }
        csv += "x7,2024-13-01\n";
        for i in 0..1000 { csv += &format!("{i},2024-01-01\n"); }
        let dir = tmpdir("pqw-late");
        let dest = dir.join("t.parquet");
        let info = out(&table(&csv, b',', true), &dest);
        assert_eq!(types_of(&info), ["STRING", "STRING"]);
        assert_eq!(back(&dest, b',').0, csv);
        let b = batches(&dest);
        assert!(b.iter().map(|b| b.num_rows()).sum::<usize>() == 201_001);
    }

    #[test]
    fn values_are_what_the_app_reads() {
        // What is typed here the app reads as a number or a date, with the same value.
        for v in ["0", "-12", "1.5", "0,25", "-3.10", "9223372036854775807", "1e+300", "2.5e-22"] {
            assert_eq!(kinds::cell_type(v), kinds::Kind::Num, "{v}");
        }
        for v in ["2024-03-01", "2024-03-01 10:30", "2024-03-01T10:30:15.120Z", "2024-03-01 10:30:15.123456789", "2024-03-01T10:30:15"] {
            assert_eq!(kinds::cell_type(v), kinds::Kind::Date, "{v}");
            assert!(crate::text::parse_date(v).is_some(), "{v}");
        }
    }

    #[test]
    fn a_double_gives_short_cells_back() {
        // The shortcut of `see_number` against the slow way, on cells of 1 to 15 digits.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..300_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let digits = (x >> 58) as usize % 15 + 1;
            let frac = (x >> 52 & 63) as usize % (digits + 1);
            let mut s = format!("{:0>w$}", (x & ((1 << 50) - 1)) % 10u64.pow(digits as u32), w = digits);
            s.truncate(digits);
            let (a, b) = s.split_at(digits - frac);
            let a = a.trim_start_matches('0');
            let a = if a.is_empty() { "0" } else { a };
            let v = if b.is_empty() { a.to_string() } else { format!("{a}.{}", b) };
            if v.ends_with('0') && v.contains('.') { continue; }
            assert!(float_holds(&v), "{v}");
        }
        for v in ["0.1", "0.000001", "0.00000000000001", "123456789012345", "1234567.12345678"] { assert!(float_holds(v), "{v}"); }
        for v in ["1.50", "1e5", "0.1000000000000000055"] { assert!(!float_holds(v), "{v}"); }
    }

    #[test]
    fn only_the_rows_given_in_their_order() {
        let t = table("a;b\n1;x\n2;y\n3;z\n4;w\n", b';', true);
        let (id3, id1) = (t.st.data_id(2), t.st.data_id(0));
        let dir = tmpdir("pqw-rows");
        let dest = dir.join("t.parquet");
        let p = AtomicUsize::new(0);
        let info = export(Format::Parquet, &t.st, Some(&[id3, id1]), &dest, "t", Some(&p), &AtomicBool::new(false)).unwrap();
        assert_eq!(info.rows, 2);
        assert_eq!(p.into_inner(), 4); // twice over
        assert_eq!(back(&dest, b';').0, "a;b\n3;z\n1;x\n");
        let no: &[RowId] = &[];
        write(&t.st, Some(no), &dest, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(back(&dest, b';').0, "a;b\n");
    }

    #[test]
    fn the_table_as_edited() {
        let mut t = table("a,b,c\n1,x,5\n2,y,6\n", b',', true);
        let dir = tmpdir("pqw-edit");
        let dest = dir.join("t.parquet");
        t.set_cells(&[(t.st.data_id(0), 0, "oops".into())]);
        t.move_column(2, 0);
        let info = out(&t, &dest);
        assert_eq!(types_of(&info), ["INT64", "STRING", "STRING"]);
        assert_eq!(back(&dest, b',').0, "c,a,b\n5,oops,x\n6,2,y\n");
    }

    #[test]
    fn names_ragged_rows_and_no_header() {
        let dir = tmpdir("pqw-names");
        let dest = dir.join("t.parquet");
        let info = out(&table("a;a;;b\n1;2\n3;4;5;6;7\n", b';', true), &dest);
        assert_eq!(info.types.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["a", "a_2", "col3", "b", "col5"]);
        assert_eq!(back(&dest, b';').0, "a;a_2;col3;b;col5\n1;2;;;\n3;4;5;6;7\n");
        out(&table("1;x\n2;y\n", b';', false), &dest);
        assert_eq!(back(&dest, b';').0, "col1;col2\n1;x\n2;y\n");
        // A title's own spaces are kept: nothing else needs them gone.
        out(&table(" a ;b\n1;2\n", b';', true), &dest);
        assert_eq!(back(&dest, b';').0, " a ;b\n1;2\n");
    }

    #[test]
    fn zero_rows_wide_and_unicode() {
        let dir = tmpdir("pqw-wide");
        let dest = dir.join("t.parquet");
        let info = out(&table("a;b\n", b';', true), &dest);
        assert_eq!((types_of(&info), info.rows), (vec!["STRING", "STRING"], 0));
        assert_eq!(back(&dest, b';').0, "a;b\n");
        let head: Vec<String> = (0..2000).map(|i| format!("côl{i}")).collect();
        let row: Vec<String> = (0..2000).map(|i| i.to_string()).collect();
        let csv = format!("{}\n{}\n{}\n", head.join(";"), row.join(";"), row.join(";"));
        let info = out(&table(&csv, b';', true), &dest);
        assert_eq!(info.cols, 2000);
        assert_eq!(back(&dest, b';').0, csv);
        trip("clé 🔑;v\n日本語;🦀\n\u{202f}é;\"a\nb\"\n", b';', &["STRING", "STRING"]);
    }

    #[test]
    fn many_row_groups() {
        let mut csv = String::from("id;x;d\n");
        for i in 0..300_000u32 { csv += &format!("{i};{},{:02};2024-01-{:02}\n", i % 1000, i % 100, i % 28 + 1); }
        let dir = tmpdir("pqw-groups");
        let dest = dir.join("t.parquet");
        let info = out(&table(&csv, b';', true), &dest);
        assert_eq!(types_of(&info), ["INT64", "DECIMAL", "DATE"]);
        let r = ParquetRecordBatchReaderBuilder::try_new(File::open(&dest).unwrap()).unwrap();
        assert_eq!(r.metadata().num_row_groups(), 3);
        assert_eq!(back(&dest, b';').0, csv);
    }

    #[test]
    fn long_rows_make_small_groups() {
        // 2 MB a row: a group of the usual rows would hold 245 GB.
        let row = "x".repeat(2 << 20);
        let csv = format!("a\n{}", format!("{row}\n").repeat(40));
        let dir = tmpdir("pqw-long");
        let dest = dir.join("t.parquet");
        let t = table(&csv, b',', true);
        assert_eq!(out(&t, &dest).rows, 40);
        assert_eq!(back(&dest, b',').0.len(), csv.len());
    }

    #[test]
    fn cancel_leaves_nothing() {
        let dir = tmpdir("pqw-cancel");
        let dest = dir.join("t.parquet");
        let t = table("a;b\n1;2\n3;4\n", b';', true);
        std::fs::write(&dest, "before").unwrap();
        assert_eq!(write(&t.st, None, &dest, None, &AtomicBool::new(true)).unwrap_err(), super::super::CANCELLED);
        assert_eq!(std::fs::read(&dest).unwrap(), b"before");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // A wrong folder: said.
        assert!(write(&t.st, None, &dir.join("no/t.parquet"), None, &AtomicBool::new(false)).unwrap_err().contains("no such folder"));
    }

    #[test]
    fn replaces_atomically() {
        let dir = tmpdir("pqw-atomic");
        let dest = dir.join("t.parquet");
        out(&table("a\n1\n", b',', true), &dest);
        out(&table("b\nx\n", b',', true), &dest);
        assert_eq!(back(&dest, b',').0, "b\nx\n");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn foreign_file_exported_again() {
        // A file of every kind opened (as CSV), exported, opened: the same CSV.
        let dir = tmpdir("pqw-again");
        let dest = dir.join("first.parquet");
        let csv = "n,f,d,t,u,s\n1,0.5,2024-01-02,2024-01-02 03:04:05.123,2024-01-02T03:04:05Z,x\n2,1e+300,2024-01-03,2024-01-02 03:04:05.456,2024-01-02T03:04:06Z,\n";
        out(&table(csv, b',', true), &dest);
        let first = dir.join("first.csv");
        import(Format::Parquet, &dest, None, b',', &first, false, None, &AtomicBool::new(false)).unwrap();
        let t = Table::new(Arc::new(Base::open(&first, OpenOptions::default(), None).unwrap()), None);
        let again = dir.join("again.parquet");
        assert_eq!(types_of(&out(&t, &again)), ["INT64", "DOUBLE", "DATE", "TIMESTAMP", "TIMESTAMP", "STRING"]);
        assert_eq!(back(&again, b',').0, csv);
    }

    #[test]
    fn parts_of_a_stamp() {
        let f = |v: &str| stamp(v.as_bytes());
        assert_eq!(f("1970-01-01 00:00:01"), Some((1000, Shape::Stamp { t: false, secs: true, digits: 0 }, false)));
        assert_eq!(f("1970-01-01T00:01"), Some((60_000, Shape::Stamp { t: true, secs: false, digits: 0 }, false)));
        assert_eq!(f("1970-01-01 00:00:00.25Z"), Some((250, Shape::Stamp { t: false, secs: true, digits: 2 }, true)));
        assert_eq!(f("1970-01-01 00:00:00.000001"), Some((1, Shape::Stamp { t: false, secs: true, digits: 6 }, false)));
        assert_eq!(f("1970-01-01 00:00:00.000000001"), Some((1, Shape::Stamp { t: false, secs: true, digits: 9 }, false)));
        for v in ["1970-01-01", "1970-01-01 ", "1970-01-01 1", "1970-01-01 10:00:", "1970-01-01 10:00:00.", "1970-01-01 10:00:00.1234567890", "1970-01-01 10:00:00 ", "1970-01-01 10-00", "1970-01-01Z"] {
            assert_eq!(f(v), None, "{v}");
        }
        let d = |v: &str| date_days(v.as_bytes());
        assert_eq!((d("1970-01-02"), d("2024-02-29"), d("2023-02-29")), (Some(1), Some(19782), None));
    }

    #[test]
    fn shapes_are_in_the_metadata() {
        // Any other reader sees a plain boolean; only csvfab reads how the cell was written.
        let dir = tmpdir("pqw-shape");
        let dest = dir.join("t.parquet");
        out(&table("a;b\nTRUE;2024-03-01T10:30:15\n", b';', true), &dest);
        let b = batches(&dest);
        assert!(b[0].column(0).as_boolean().value(0));
        let r = ParquetRecordBatchReaderBuilder::try_new(File::open(&dest).unwrap()).unwrap();
        let kv = r.metadata().file_metadata().key_value_metadata().unwrap();
        assert_eq!(kv.iter().find(|k| k.key == CELLS_KEY).and_then(|k| k.value.as_deref()), Some("TRUE,T0"));
    }
}
