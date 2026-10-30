//! A Parquet file as CSV. The file is mapped (nothing is read twice, the page cache holds it)
//! and its row groups are decoded by several threads at once; a thread writes the text of
//! each batch to a queue of its row group and the caller writes the queues in file order.
//! What waits in the queues is bounded in bytes, so a file of any size needs little RAM: past
//! the bound only the group next in the file goes on.
//!
//! What a cell becomes:
//! - null: an empty cell;
//! - integers as they are; floats in their shortest round-trip digits, without an exponent
//!   unless the value is beyond 10^±21 (`1e+300`), `nan`, `inf`, `-inf`; decimals with all
//!   their digits (`12.50`). A decimal comma when the CSV is `;`-delimited, as for the other
//!   imports (`push_float`, `push_decimal`);
//! - booleans `true`/`false`;
//! - strings as they are; binary in hexadecimal, `0x00ff`, as the SQLite import; a UUID as
//!   `8-4-4-4-12`;
//! - dates `yyyy-mm-dd`, times `hh:mm:ss[.fff]`, intervals of time as ISO durations;
//! - lists, maps and structs as JSON text (`Float` and `Decimal` as numbers, everything that
//!   has no JSON form as a string).
//!
//! Timestamps: a timestamp without a zone (`isAdjustedToUTC=false`, INT96) is shown as stored,
//! `yyyy-mm-dd hh:mm:ss[.fff]`. An instant (`isAdjustedToUTC=true`) is always shown in UTC with
//! a final `Z`, `yyyy-mm-ddThh:mm:ss[.fff]Z`: never in this machine's zone, so a file gives the
//! same CSV everywhere (an Arrow zone name, only a display hint, is not read at all). The
//! fraction has 3, 6 or 9 digits, none when zero.
//!
//! A file written by csvfab keeps how its cells were written (`csvfab.cells`, see
//! [`Shape`]): booleans in capitals, a `T` before the time, a fixed number of decimals of a
//! second, so that reopening gives the cells back as they were.

use super::{check, civil_from_days, io_err, tick, CsvOut, Imported};
use crate::mapguard::Guarded;
use arrow_array::cast::AsArray;
use arrow_array::types::*;
use arrow_array::{Array, ArrowPrimitiveType};
use arrow_schema::{DataType, IntervalUnit, TimeUnit};
use bytes::Bytes;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::basic::{Compression, ConvertedType, LogicalType};
use std::fmt::Write as _;
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// The key of the file's metadata holding each column's [`Shape`], `,`-separated.
pub(super) const CELLS_KEY: &str = "csvfab.cells";

/// The text waiting to be written, at most (past it, only the next group in the file goes on).
/// Going from 1 GB to this changed no time on the 2 GB file (docs/perf.md).
const HELD: usize = 512 << 20;
/// A batch holds about this many bytes (by the row group's own count), this many rows at most.
const BATCH_BYTES: u64 = 16 << 20;
const BATCH_ROWS: u64 = 32 * 1024;

fn unreadable(e: impl std::fmt::Display) -> String {
    format!("not a readable Parquet file ({e})")
}

/// How the cells of a column are written, when not as for any Parquet file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Shape {
    #[default]
    Plain,
    /// Booleans as `TRUE`, `FALSE`.
    Upper,
    /// Booleans as `True`, `False`.
    Title,
    /// A timestamp: `T` or a space before the time; seconds or not; digits after them, all of them.
    Stamp { t: bool, secs: bool, digits: u8 },
    /// Sixteen bytes as `8-4-4-4-12`.
    Uuid,
    /// An ENUM: text, which Arrow reads as bytes.
    Enum,
}

impl Shape {
    /// As `csvfab.cells` holds it: empty when plain.
    pub(super) fn code(self) -> String {
        match self {
            Shape::Upper => "TRUE".into(),
            Shape::Title => "True".into(),
            Shape::Stamp { t, secs, digits } => format!("{}{}", if t { 'T' } else { 's' }, if secs { (b'0' + digits) as char } else { 'm' }),
            Shape::Plain | Shape::Uuid | Shape::Enum => String::new(),
        }
    }

    fn of(code: &str) -> Shape {
        match *code.as_bytes() {
            [b'T', b'R', b'U', b'E'] => Shape::Upper,
            [b'T', b'r', b'u', b'e'] => Shape::Title,
            [s @ (b'T' | b's'), b'm'] => Shape::Stamp { t: s == b'T', secs: false, digits: 0 },
            [s @ (b'T' | b's'), d @ b'0'..=b'9'] => Shape::Stamp { t: s == b'T', secs: true, digits: d - b'0' },
            _ => Shape::Plain,
        }
    }
}

// --- numbers and dates as text --------------------------------------------------

/// A float in its shortest round-trip digits: no `.0`, no exponent unless the value is
/// beyond 10^±21. `comma`: a decimal comma.
pub(super) fn push_float<F: zmij::Float + Into<f64> + Copy>(x: F, comma: bool, s: &mut String) {
    let w: f64 = x.into();
    if !w.is_finite() {
        s.push_str(if w.is_nan() { "nan" } else if w > 0.0 { "inf" } else { "-inf" });
        return;
    }
    if w == 0.0 {
        s.push('0');
        return;
    }
    let mut buf = zmij::Buffer::new();
    let t = buf.format_finite(x);
    let (mant, exp) = match t.find('e') {
        Some(k) => (&t[..k], t[k + 1..].parse::<i32>().unwrap_or(0)),
        None => (t, 0),
    };
    let mant = mant.strip_suffix(".0").unwrap_or(mant);
    let m = mant.strip_prefix('-').unwrap_or(mant);
    if w < 0.0 { s.push('-'); }
    let mark = if comma { ',' } else { '.' };
    if exp == 0 {
        match m.find('.') {
            Some(k) => { s.push_str(&m[..k]); s.push(mark); s.push_str(&m[k + 1..]); }
            None => s.push_str(m),
        }
        return;
    }
    let (head, tail) = m.split_once('.').unwrap_or((m, ""));
    let point = head.len() as i32 + exp;
    if exp.abs() >= 21 {
        s.push_str(head);
        if !tail.is_empty() { s.push(mark); s.push_str(tail); }
        let _ = write!(s, "e{}{}", if exp < 0 { '-' } else { '+' }, exp.abs());
    } else if point <= 0 {
        s.push('0');
        s.push(mark);
        for _ in 0..-point { s.push('0'); }
        s.push_str(head);
        s.push_str(tail);
    } else {
        // The digits around the point: zeros after them when it lies past their end.
        let n = head.len() + tail.len();
        let (p, all) = (point as usize, [head, tail].concat());
        if p >= n {
            s.push_str(&all);
            for _ in 0..p - n { s.push('0'); }
        } else {
            s.push_str(&all[..p]);
            s.push(mark);
            s.push_str(&all[p..]);
        }
    }
}

/// A decimal from its unscaled digits, every digit of its scale kept.
pub(super) fn push_decimal(digits: &str, scale: i8, comma: bool, s: &mut String) {
    let d = digits.strip_prefix('-').unwrap_or(digits);
    if d.len() < digits.len() { s.push('-'); }
    if scale <= 0 {
        s.push_str(d);
        if d != "0" { for _ in 0..-(scale as i32) { s.push('0'); } }
        return;
    }
    let sc = scale as usize;
    let mark = if comma { ',' } else { '.' };
    if d.len() > sc {
        s.push_str(&d[..d.len() - sc]);
        s.push(mark);
        s.push_str(&d[d.len() - sc..]);
    } else {
        s.push('0');
        s.push(mark);
        for _ in 0..sc - d.len() { s.push('0'); }
        s.push_str(d);
    }
}

#[inline]
fn two(n: u32, s: &mut String) {
    s.push((b'0' + (n / 10 % 10) as u8) as char);
    s.push((b'0' + (n % 10) as u8) as char);
}

/// `yyyy-mm-dd`, from days since 1970-01-01.
fn push_date(days: i64, s: &mut String) {
    let (y, m, d) = civil_from_days(days);
    if (0..=9999).contains(&y) {
        two((y / 100) as u32, s);
        two((y % 100) as u32, s);
        s.push('-');
        two(m, s);
        s.push('-');
        two(d, s);
    } else {
        let _ = write!(s, "{y:04}-{m:02}-{d:02}");
    }
}

/// The digits after the seconds: `fixed` of them, else 3, 6 or 9, none when zero.
fn push_fraction(nanos: u32, fixed: Option<u8>, s: &mut String) {
    let digits = fixed.map_or(if nanos == 0 { 0 } else if nanos % 1_000_000 == 0 { 3 } else if nanos % 1000 == 0 { 6 } else { 9 }, u32::from);
    if digits == 0 { return; }
    s.push('.');
    let mut unit = 100_000_000;
    for _ in 0..digits {
        s.push((b'0' + (nanos / unit % 10) as u8) as char);
        unit = (unit / 10).max(1);
    }
}

/// `hh:mm:ss[.fff]`, from seconds into the day.
fn push_time(secs: i64, nanos: u32, s: &mut String) {
    let t = secs.rem_euclid(86400) as u32;
    two(t / 3600, s);
    s.push(':');
    two(t % 3600 / 60, s);
    s.push(':');
    two(t % 60, s);
    push_fraction(nanos, None, s);
}

/// A timestamp from `v` units, `per` to the second. `z`: an instant, written in UTC.
fn push_stamp(v: i64, per: i64, z: bool, shape: Shape, s: &mut String) {
    let (secs, nanos) = (v.div_euclid(per), (v.rem_euclid(per) * (1_000_000_000 / per)) as u32);
    push_date(secs.div_euclid(86400), s);
    let (sep, with_secs, fixed) = match shape {
        Shape::Stamp { t, secs, digits } => (t, secs, Some(digits)),
        _ => (z, true, None),
    };
    s.push(if sep { 'T' } else { ' ' });
    let t = secs.rem_euclid(86400) as u32;
    two(t / 3600, s);
    s.push(':');
    two(t % 3600 / 60, s);
    if with_secs {
        s.push(':');
        two(t % 60, s);
        push_fraction(nanos, fixed, s);
    }
    if z { s.push('Z'); }
}

/// ISO 8601: `P1M2DT3.5S`, `PT0S` when nothing.
fn push_interval(months: i64, days: i64, millis: i64, s: &mut String) {
    s.push('P');
    if months != 0 { let _ = write!(s, "{months}M"); }
    if days != 0 { let _ = write!(s, "{days}D"); }
    if millis != 0 {
        let _ = write!(s, "T{}", millis / 1000);
        let f = (millis % 1000).unsigned_abs();
        if f != 0 { let _ = write!(s, ".{}", format!("{f:03}").trim_end_matches('0')); }
        s.push('S');
    } else if months == 0 && days == 0 {
        s.push_str("T0S");
    }
}

fn push_hex(b: &[u8], s: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    s.push_str("0x");
    for x in b {
        s.push(HEX[(x >> 4) as usize] as char);
        s.push(HEX[(x & 15) as usize] as char);
    }
}

fn push_uuid(b: &[u8], s: &mut String) {
    for (k, x) in b.iter().enumerate() {
        if matches!(k, 4 | 6 | 8 | 10) { s.push('-'); }
        let _ = write!(s, "{x:02x}");
    }
}

// --- cells ---------------------------------------------------------------------

/// A column of a batch as text: `f(row, out)`.
type Cell<'a> = Box<dyn FnMut(usize, &mut String) + 'a>;

#[derive(Clone, Copy)]
struct Opts {
    comma: bool,
    /// Inside a list, a map or a struct: JSON, null and strings written as such.
    json: bool,
    shape: Shape,
}

fn ints<'a, T: ArrowPrimitiveType>(a: &'a dyn Array) -> Cell<'a>
where
    T::Native: itoa::Integer,
{
    let v = a.as_primitive::<T>();
    let mut buf = itoa::Buffer::new();
    Box::new(move |i, s| s.push_str(buf.format(v.value(i))))
}

/// A number in JSON has no form for these.
fn float_cell<'a, F: zmij::Float + Into<f64> + Copy + 'a>(value: impl Fn(usize) -> F + 'a, o: Opts) -> Cell<'a> {
    Box::new(move |i, s| {
        let x = value(i);
        let q = o.json && !Into::<f64>::into(x).is_finite();
        if q { s.push('"'); }
        push_float(x, o.comma, s);
        if q { s.push('"'); }
    })
}

/// A text with no character to escape, in quotes in JSON.
fn text<'a>(o: Opts, mut f: impl FnMut(usize, &mut String) + 'a) -> Cell<'a> {
    if o.json {
        Box::new(move |i, s| { s.push('"'); f(i, s); s.push('"'); })
    } else {
        Box::new(f)
    }
}

fn times<'a, T: ArrowPrimitiveType<Native = i64>>(a: &'a dyn Array, per: i64, o: Opts) -> Cell<'a> {
    let v = a.as_primitive::<T>();
    text(o, move |i, s| { let x = v.value(i); push_time(x.div_euclid(per), (x.rem_euclid(per) * (1_000_000_000 / per)) as u32, s); })
}

fn stamps<'a, T: ArrowPrimitiveType<Native = i64>>(a: &'a dyn Array, per: i64, z: bool, o: Opts) -> Cell<'a> {
    let v = a.as_primitive::<T>();
    text(o, move |i, s| push_stamp(v.value(i), per, z, o.shape, s))
}

/// A column of a batch as text, null included.
fn cell<'a>(a: &'a dyn Array, o: Opts) -> Result<Cell<'a>, String> {
    let mut f = plain(a, o)?;
    if a.null_count() == 0 { return Ok(f); }
    Ok(Box::new(move |i, s| {
        if !a.is_null(i) { f(i, s); } else if o.json { s.push_str("null"); }
    }))
}

/// A value's text, the value being there.
fn plain<'a>(a: &'a dyn Array, o: Opts) -> Result<Cell<'a>, String> {
    Ok(match a.data_type() {
        DataType::Null => Box::new(|_, _| {}),
        DataType::Boolean => {
            let v = a.as_boolean();
            let (t, f) = match o.shape { Shape::Upper => ("TRUE", "FALSE"), Shape::Title => ("True", "False"), _ => ("true", "false") };
            Box::new(move |i, s| s.push_str(if v.value(i) { t } else { f }))
        }
        DataType::Int8 => ints::<Int8Type>(a),
        DataType::Int16 => ints::<Int16Type>(a),
        DataType::Int32 => ints::<Int32Type>(a),
        DataType::Int64 => ints::<Int64Type>(a),
        DataType::UInt8 => ints::<UInt8Type>(a),
        DataType::UInt16 => ints::<UInt16Type>(a),
        DataType::UInt32 => ints::<UInt32Type>(a),
        DataType::UInt64 => ints::<UInt64Type>(a),
        DataType::Float16 => { let v = a.as_primitive::<Float16Type>(); float_cell(move |i| v.value(i).to_f32(), o) }
        DataType::Float32 => { let v = a.as_primitive::<Float32Type>(); float_cell(move |i| v.value(i), o) }
        DataType::Float64 => { let v = a.as_primitive::<Float64Type>(); float_cell(move |i| v.value(i), o) }
        DataType::Decimal128(_, sc) => {
            let (v, sc) = (a.as_primitive::<Decimal128Type>(), *sc);
            let mut buf = itoa::Buffer::new();
            Box::new(move |i, s| push_decimal(buf.format(v.value(i)), sc, o.comma, s))
        }
        DataType::Decimal256(_, sc) => {
            let (v, sc) = (a.as_primitive::<Decimal256Type>(), *sc);
            Box::new(move |i, s| push_decimal(&v.value(i).to_string(), sc, o.comma, s))
        }
        DataType::Utf8 => {
            let v = a.as_string::<i32>();
            if o.json { Box::new(move |i, s| super::json::quote(v.value(i), s)) } else { Box::new(move |i, s| s.push_str(v.value(i))) }
        }
        DataType::Binary if o.shape == Shape::Enum => {
            let v = a.as_binary::<i32>();
            Box::new(move |i, s| s.push_str(&String::from_utf8_lossy(v.value(i))))
        }
        DataType::Binary => { let v = a.as_binary::<i32>(); text(o, move |i, s| push_hex(v.value(i), s)) }
        DataType::FixedSizeBinary(_) => {
            let v = a.as_fixed_size_binary();
            if o.shape == Shape::Uuid { text(o, move |i, s| push_uuid(v.value(i), s)) } else { text(o, move |i, s| push_hex(v.value(i), s)) }
        }
        DataType::Date32 => { let v = a.as_primitive::<Date32Type>(); text(o, move |i, s| push_date(v.value(i) as i64, s)) }
        DataType::Date64 => {
            let v = a.as_primitive::<Date64Type>();
            text(o, move |i, s| {
                let ms = v.value(i);
                push_date(ms.div_euclid(86_400_000), s);
                let t = ms.rem_euclid(86_400_000);
                if t != 0 { s.push(' '); push_time(t / 1000, (t % 1000) as u32 * 1_000_000, s); }
            })
        }
        DataType::Time32(TimeUnit::Second) => { let v = a.as_primitive::<Time32SecondType>(); text(o, move |i, s| push_time(v.value(i) as i64, 0, s)) }
        DataType::Time32(_) => {
            let v = a.as_primitive::<Time32MillisecondType>();
            text(o, move |i, s| { let x = v.value(i) as i64; push_time(x.div_euclid(1000), x.rem_euclid(1000) as u32 * 1_000_000, s); })
        }
        DataType::Time64(TimeUnit::Microsecond) => times::<Time64MicrosecondType>(a, 1_000_000, o),
        DataType::Time64(_) => times::<Time64NanosecondType>(a, 1_000_000_000, o),
        DataType::Timestamp(TimeUnit::Second, tz) => stamps::<TimestampSecondType>(a, 1, tz.is_some(), o),
        DataType::Timestamp(TimeUnit::Millisecond, tz) => stamps::<TimestampMillisecondType>(a, 1000, tz.is_some(), o),
        DataType::Timestamp(TimeUnit::Microsecond, tz) => stamps::<TimestampMicrosecondType>(a, 1_000_000, tz.is_some(), o),
        DataType::Timestamp(TimeUnit::Nanosecond, tz) => stamps::<TimestampNanosecondType>(a, 1_000_000_000, tz.is_some(), o),
        DataType::Interval(IntervalUnit::YearMonth) => {
            let v = a.as_primitive::<IntervalYearMonthType>();
            text(o, move |i, s| push_interval(v.value(i) as i64, 0, 0, s))
        }
        DataType::Interval(IntervalUnit::DayTime) => {
            let v = a.as_primitive::<IntervalDayTimeType>();
            text(o, move |i, s| { let x = v.value(i); push_interval(0, x.days as i64, x.milliseconds as i64, s); })
        }
        DataType::List(_) => {
            let l = a.as_list::<i32>();
            let (offs, mut item) = (l.value_offsets(), cell(l.values().as_ref(), inner(o))?);
            Box::new(move |i, s| {
                s.push('[');
                for k in offs[i]..offs[i + 1] {
                    if k > offs[i] { s.push(','); }
                    item(k as usize, s);
                }
                s.push(']');
            })
        }
        DataType::Map(..) => {
            let m = a.as_map();
            let offs = m.value_offsets();
            // A key's text, in quotes whatever its type: JSON has no other keys.
            let mut key = cell(m.keys().as_ref(), Opts { json: false, ..inner(o) })?;
            let mut val = cell(m.values().as_ref(), inner(o))?;
            let mut tmp = String::new();
            Box::new(move |i, s| {
                s.push('{');
                for k in offs[i]..offs[i + 1] {
                    if k > offs[i] { s.push(','); }
                    tmp.clear();
                    key(k as usize, &mut tmp);
                    super::json::quote(&tmp, s);
                    s.push(':');
                    val(k as usize, s);
                }
                s.push('}');
            })
        }
        DataType::Struct(fields) => {
            let st = a.as_struct();
            let mut parts = Vec::with_capacity(fields.len());
            for (k, f) in fields.iter().enumerate() {
                let mut name = String::new();
                super::json::quote(f.name(), &mut name);
                parts.push((name, cell(st.column(k).as_ref(), inner(o))?));
            }
            Box::new(move |i, s| {
                s.push('{');
                for (k, (name, c)) in parts.iter_mut().enumerate() {
                    if k > 0 { s.push(','); }
                    s.push_str(name);
                    s.push(':');
                    c(i, s);
                }
                s.push('}');
            })
        }
        t => return Err(unreadable(format!("no reading of the type {t}"))),
    })
}

/// What's inside a list, a map or a struct.
fn inner(_: Opts) -> Opts {
    Opts { comma: false, json: true, shape: Shape::Plain }
}

// --- the reading ---------------------------------------------------------------

/// What a thread hands over for a row group: the text of some rows, and the end.
enum Msg {
    Text(String, usize),
    End(Result<(), String>),
}

/// Bounds what the threads hold in text. The row group next in the file always goes on (the
/// writer waits for it), the others wait for room.
struct Gate {
    held: Mutex<usize>,
    room: Condvar,
    /// The row group being written.
    head: AtomicUsize,
    stop: AtomicBool,
}

impl Gate {
    fn new() -> Gate {
        Gate { held: Mutex::new(0), room: Condvar::new(), head: AtomicUsize::new(0), stop: AtomicBool::new(false) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, usize> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits for room for `n` bytes of `group`. False: stopped.
    fn enter(&self, group: usize, n: usize) -> bool {
        let mut h = self.lock();
        while *h + n > HELD && self.head.load(Ordering::Acquire) != group && !self.stop.load(Ordering::Acquire) {
            h = self.room.wait(h).unwrap_or_else(|e| e.into_inner());
        }
        *h += n;
        !self.stop.load(Ordering::Acquire)
    }

    fn leave(&self, n: usize) {
        *self.lock() -= n;
        self.room.notify_all();
    }

    /// `group` is the one written now.
    fn advance(&self, group: usize) {
        let _h = self.lock();
        self.head.store(group, Ordering::Release);
        self.room.notify_all();
    }

    fn halt(&self) {
        let _h = self.lock();
        self.stop.store(true, Ordering::Release);
        self.room.notify_all();
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

/// A batch's rows as CSV, quoted as `CsvOut` does.
fn rows_text(cells: &mut [Cell], rows: usize, d: u8, out: &mut String, tmp: &mut String) {
    let one = cells.len() == 1;
    for r in 0..rows {
        let start = out.len();
        for (k, c) in cells.iter_mut().enumerate() {
            if k > 0 { out.push(d as char); }
            let at = out.len();
            c(r, out);
            if out.as_bytes()[at..].iter().any(|&b| b == d || b == b'"' || b == b'\n' || b == b'\r') {
                tmp.clear();
                tmp.push_str(&out[at..]);
                out.truncate(at);
                out.push('"');
                for part in tmp.split_inclusive('"') {
                    out.push_str(part);
                    if part.ends_with('"') { out.push('"'); }
                }
                out.push('"');
            }
        }
        // A row of a single empty field is not an empty line.
        if one && out.len() == start { out.push_str("\"\""); }
        out.push('\n');
    }
}

struct Group<'a> {
    bytes: &'a Bytes,
    meta: &'a ArrowReaderMetadata,
    shapes: &'a [Shape],
    delim: u8,
    gate: &'a Gate,
    cancel: &'a AtomicBool,
}

impl Group<'_> {
    /// Decodes row group `i`, its text sent as it comes.
    fn read(&self, i: usize, tx: &Sender<Msg>) -> Result<(), String> {
        let rg = self.meta.metadata().row_group(i);
        let want = rg.num_rows().max(0) as u64;
        let per_row = (rg.total_byte_size().max(0) as u64 / want.max(1)).max(1);
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(self.bytes.clone(), self.meta.clone())
            .with_row_groups(vec![i])
            .with_batch_size((BATCH_BYTES / per_row).clamp(1, BATCH_ROWS) as usize)
            .build()
            .map_err(unreadable)?;
        let comma = self.delim == b';';
        let (mut got, mut tmp) = (0u64, String::new());
        let mut per_row_text = self.shapes.len() * 8 + 1;
        for batch in reader {
            check(self.cancel)?;
            if self.gate.stopped() { return Ok(()); }
            let batch = batch.map_err(unreadable)?;
            let mut cells = batch.columns().iter().zip(self.shapes)
                .map(|(a, &shape)| cell(a.as_ref(), Opts { comma, json: false, shape }))
                .collect::<Result<Vec<_>, _>>()?;
            // Sized by the batch before: a String doubling would hold twice what it says.
            let mut out = String::with_capacity((batch.num_rows() * per_row_text).max(1024) + (1 << 12));
            rows_text(&mut cells, batch.num_rows(), self.delim, &mut out, &mut tmp);
            per_row_text = (out.len() / batch.num_rows().max(1) + 1) * 9 / 8;
            got += batch.num_rows() as u64;
            out.shrink_to_fit();
            if !self.gate.enter(i, out.capacity()) { return Ok(()); }
            if tx.send(Msg::Text(out, batch.num_rows())).is_err() { return Ok(()); }
        }
        if got != want {
            return Err(unreadable(format!("row group {} holds {got} rows, {want} announced", i + 1)));
        }
        Ok(())
    }
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "damaged data".into())
}

/// The file's columns that cannot be read, said before any work; each column's shape.
fn shapes_of(m: &ArrowReaderMetadata, ncols: usize) -> Result<Vec<Shape>, String> {
    let meta = m.metadata();
    for rg in meta.row_groups() {
        for c in rg.columns() {
            let codec = match c.compression() { Compression::LZO => "LZO", Compression::BROTLI(_) => "brotli", _ => continue };
            return Err(format!("column \"{}\" is compressed with {codec}, which csvfab does not read: snappy, gzip, zstd and lz4 are read", c.column_path().string()));
        }
    }
    let file = meta.file_metadata();
    let schema = file.schema_descr();
    for c in schema.columns() {
        // Read through Arrow, an INTERVAL would silently lose its months.
        if c.converted_type() == ConvertedType::INTERVAL {
            return Err(format!("column \"{}\" has the INTERVAL type, which csvfab does not read", c.path().string()));
        }
    }
    let mut shapes = vec![Shape::Plain; ncols];
    let codes = file.key_value_metadata().and_then(|v| v.iter().find(|k| k.key == CELLS_KEY)).and_then(|k| k.value.as_deref());
    if let Some(codes) = codes.filter(|t| t.split(',').count() == ncols) {
        for (s, c) in shapes.iter_mut().zip(codes.split(',')) { *s = Shape::of(c); }
    }
    // UUIDs and enums of the top level; those in a list stay hexadecimal.
    for (k, f) in schema.root_schema().get_fields().iter().enumerate().take(ncols) {
        if !f.is_primitive() { continue; }
        let info = f.get_basic_info();
        match info.logical_type_ref() {
            Some(LogicalType::Uuid) => shapes[k] = Shape::Uuid,
            Some(LogicalType::Enum) => shapes[k] = Shape::Enum,
            _ if info.converted_type() == ConvertedType::ENUM => shapes[k] = Shape::Enum,
            _ => {}
        }
    }
    Ok(shapes)
}

/// The Parquet file at `path` as CSV into `out`. Progress: rows done.
pub fn to_csv<W: Write>(path: &Path, delim: u8, mut out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let f = std::fs::File::open(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "file not found".to_string() } else { e.to_string() })?;
    let len = f.metadata().map_err(io_err)?.len();
    if len == 0 { return Err(unreadable("the file is empty")); }
    struct Mapped(Arc<Guarded>);
    impl AsRef<[u8]> for Mapped {
        fn as_ref(&self) -> &[u8] { &self.0 }
    }
    let map = Arc::new(Guarded::map(&f).map_err(io_err)?);
    if len < 12 || (&map[..4] != b"PAR1" && &map[..4] != b"PARE") {
        return Err(unreadable("it does not start as one"));
    }
    if &map[..4] == b"PARE" || &map[len as usize - 4..] == b"PARE" {
        return Err("this Parquet file is encrypted, which csvfab does not read".into());
    }
    if &map[len as usize - 4..] != b"PAR1" {
        return Err(unreadable("its end is missing: the file is cut short"));
    }
    let bytes = Bytes::from_owner(Mapped(map.clone()));
    // The zone an Arrow writer left in the file's metadata is not read (see the top of this file).
    let opts = ArrowReaderOptions::new().with_skip_arrow_metadata(true);
    // The parsers refuse what they see as wrong; a panic on what they did not foresee is a
    // damaged file too.
    let meta = catch_unwind(AssertUnwindSafe(|| ArrowReaderMetadata::load(&bytes, opts))).map_err(|p| unreadable(panic_text(p)))?.map_err(unreadable)?;
    let names: Vec<String> = meta.schema().fields().iter().map(|f| f.name().clone()).collect();
    if names.is_empty() { return Err(unreadable("it has no column")); }
    let shapes = shapes_of(&meta, names.len())?;

    let mut csv = CsvOut::new(&mut out, delim, true);
    for n in &names { csv.field(n).map_err(io_err)?; }
    csv.end().map_err(io_err)?;

    let groups = meta.metadata().num_row_groups();
    let threads = rayon::current_num_threads().min(groups).max(1);
    let mut rows = 0u64;
    let (txs, rxs): (Vec<_>, Vec<_>) = (0..groups).map(|_| channel::<Msg>()).unzip();
    let (next, gate) = (AtomicUsize::new(0), Gate::new());
    let work = Group { bytes: &bytes, meta: &meta, shapes: &shapes, delim, gate: &gate, cancel };
    let done = std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= groups || gate.stopped() { break; }
                let r = catch_unwind(AssertUnwindSafe(|| work.read(i, &txs[i]))).unwrap_or_else(|p| Err(unreadable(panic_text(p))));
                let failed = r.is_err();
                let _ = txs[i].send(Msg::End(r));
                if failed { break; }
            });
        }
        let r = (|| {
            for (i, rx) in rxs.iter().enumerate() {
                gate.advance(i);
                loop {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(Msg::Text(t, k)) => {
                            out.write_all(t.as_bytes()).map_err(io_err)?;
                            gate.leave(t.capacity());
                            rows += k as u64;
                            tick(progress, k);
                            check(cancel)?;
                        }
                        Ok(Msg::End(r)) => { r?; break; }
                        Err(RecvTimeoutError::Timeout) => check(cancel)?,
                        Err(RecvTimeoutError::Disconnected) => return Err(unreadable("a thread stopped")),
                    }
                }
            }
            Ok(())
        })();
        gate.halt();
        r
    });
    done?;
    if map.cut_short() { return Err("the file was cut short by another program while it was read".into()); }
    out.flush().map_err(io_err)?;
    Ok(Imported { sheet: None, others: 0, rows, cols: names.len(), ..Default::default() })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arrow_array::builder::{Int32Builder, ListBuilder, MapBuilder, StringBuilder};
    use arrow_array::{ArrayRef, Int32Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::basic::{Encoding, GzipLevel, ZstdLevel};
    use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType, FloatType, Int32Type as PqI32, Int64Type as PqI64, Int96, Int96Type};
    use parquet::file::properties::{WriterProperties, WriterVersion};
    use parquet::file::writer::{SerializedFileWriter, SerializedRowGroupWriter};
    use parquet::schema::parser::parse_message_type;
    use parquet::schema::types::ColumnPath;
    use std::fs::File;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        super::super::xlsx_read::tests::tmpdir(tag)
    }

    fn csv_of(p: &Path, delim: u8) -> Result<(String, Imported), String> {
        let mut out = Vec::new();
        let info = to_csv(p, delim, &mut out, None, &AtomicBool::new(false))?;
        Ok((String::from_utf8(out).unwrap(), info))
    }

    fn text(p: &Path) -> String {
        csv_of(p, b',').unwrap().0
    }

    /// A file written by the parquet crate's low-level API, column by column.
    fn low_level(path: &Path, schema: &str, props: WriterProperties, groups: usize, mut f: impl FnMut(usize, &mut SerializedRowGroupWriter<'_, File>)) {
        let schema = Arc::new(parse_message_type(schema).unwrap());
        let mut w = SerializedFileWriter::new(File::create(path).unwrap(), schema, Arc::new(props)).unwrap();
        for g in 0..groups {
            let mut rg = w.next_row_group().unwrap();
            f(g, &mut rg);
            rg.close().unwrap();
        }
        w.close().unwrap();
    }

    macro_rules! put {
        ($rg:expr, $t:ty, $vals:expr, $defs:expr) => { put!($rg, $t, $vals, $defs, None) };
        ($rg:expr, $t:ty, $vals:expr, $defs:expr, $reps:expr) => {{
            let mut c = $rg.next_column().unwrap().unwrap();
            c.typed::<$t>().write_batch($vals, $defs, $reps).unwrap();
            c.close().unwrap();
        }};
    }

    fn flba(b: &[u8]) -> FixedLenByteArray {
        FixedLenByteArray::from(b.to_vec())
    }

    fn be(v: i128, n: usize) -> Vec<u8> {
        v.to_be_bytes()[16 - n..].to_vec()
    }

    #[test]
    fn every_type_as_text() {
        let dir = tmpdir("pq-types");
        let p = dir.join("t.parquet");
        let schema = "message t {
            REQUIRED BOOLEAN b;
            OPTIONAL INT32 i32;
            OPTIONAL INT32 i8 (INTEGER(8,true));
            OPTIONAL INT32 u16 (INTEGER(16,false));
            OPTIONAL INT64 i64;
            OPTIONAL INT64 u64 (INTEGER(64,false));
            OPTIONAL FLOAT f32;
            OPTIONAL DOUBLE f64;
            OPTIONAL BINARY s (UTF8);
            OPTIONAL BINARY e (ENUM);
            OPTIONAL BINARY j (JSON);
            OPTIONAL BINARY raw;
            OPTIONAL FIXED_LEN_BYTE_ARRAY (3) fix;
            OPTIONAL FIXED_LEN_BYTE_ARRAY (16) id (UUID);
            OPTIONAL FIXED_LEN_BYTE_ARRAY (2) h (FLOAT16);
            OPTIONAL INT32 d (DATE);
            OPTIONAL INT32 tm (TIME(MILLIS,true));
            OPTIONAL INT64 tu (TIME(MICROS,false));
            OPTIONAL INT64 tn (TIME(NANOS,false));
            OPTIONAL INT64 ts (TIMESTAMP(MILLIS,false));
            OPTIONAL INT64 tz (TIMESTAMP(MICROS,true));
            OPTIONAL INT64 tn2 (TIMESTAMP(NANOS,false));
            OPTIONAL INT64 old (TIMESTAMP_MILLIS);
            OPTIONAL INT96 i96;
            OPTIONAL INT32 d32 (DECIMAL(9,2));
            OPTIONAL INT64 d64 (DECIMAL(18,4));
            OPTIONAL FIXED_LEN_BYTE_ARRAY (16) d128 (DECIMAL(38,10));
            OPTIONAL FIXED_LEN_BYTE_ARRAY (32) d256 (DECIMAL(76,2));
            OPTIONAL BINARY dbin (DECIMAL(20,3));
        }";
        let d: &[i16] = &[1, 0];
        let id: Vec<u8> = (0..16).collect();
        low_level(&p, schema, WriterProperties::default(), 1, |_, rg| {
            put!(rg, BoolType, &[true, false], None);
            put!(rg, PqI32, &[-7], Some(d));
            put!(rg, PqI32, &[-128], Some(d));
            put!(rg, PqI32, &[65535], Some(d));
            put!(rg, PqI64, &[i64::MIN], Some(d));
            put!(rg, PqI64, &[-1], Some(d)); // u64::MAX
            put!(rg, FloatType, &[0.1], Some(d));
            put!(rg, DoubleType, &[1234.5], Some(d));
            put!(rg, ByteArrayType, &[ByteArray::from("héllo")], Some(d));
            put!(rg, ByteArrayType, &[ByteArray::from("RED")], Some(d));
            put!(rg, ByteArrayType, &[ByteArray::from("{\"a\":1}")], Some(d));
            put!(rg, ByteArrayType, &[ByteArray::from(vec![0u8, 255])], Some(d));
            put!(rg, FixedLenByteArrayType, &[flba(&[1, 2, 3])], Some(d));
            put!(rg, FixedLenByteArrayType, &[flba(&id)], Some(d));
            put!(rg, FixedLenByteArrayType, &[flba(&[0x00, 0x3e])], Some(d));
            put!(rg, PqI32, &[19783], Some(d));
            put!(rg, PqI32, &[37_815_123], Some(d));
            put!(rg, PqI64, &[37_815_000_000], Some(d));
            put!(rg, PqI64, &[37_815_000_000_500], Some(d));
            put!(rg, PqI64, &[1_709_289_015_123], Some(d));
            put!(rg, PqI64, &[1_709_289_015_000_000], Some(d));
            put!(rg, PqI64, &[1_709_289_015_000_000_001], Some(d));
            put!(rg, PqI64, &[1_709_289_015_000], Some(d));
            let mut old = Int96::new();
            let ns: u64 = 37_815_000_000_000;
            old.set_data(ns as u32, (ns >> 32) as u32, 2_460_371);
            put!(rg, Int96Type, &[old], Some(d));
            put!(rg, PqI32, &[-5], Some(d));
            put!(rg, PqI64, &[123_450_000], Some(d));
            put!(rg, FixedLenByteArrayType, &[flba(&be(10_i128.pow(30) + 1, 16))], Some(d));
            let mut x = vec![0xffu8; 16]; // sign-extended
            x.extend(be(-1250, 16));
            put!(rg, FixedLenByteArrayType, &[flba(&x)], Some(d));
            put!(rg, ByteArrayType, &[ByteArray::from(be(1500, 2))], Some(d));
        });
        let (t, info) = csv_of(&p, b',').unwrap();
        let mut lines = t.lines();
        assert_eq!(lines.next().unwrap().split(',').count(), 29);
        let cells: Vec<&str> = lines.next().unwrap().split(',').collect();
        assert_eq!(cells, [
            "true", "-7", "-128", "65535", "-9223372036854775808", "18446744073709551615", "0.1", "1234.5", "héllo", "RED", "\"{\"\"a\"\":1}\"",
            "0x00ff", "0x010203", "00010203-0405-0607-0809-0a0b0c0d0e0f", "1.5", "2024-03-01", "10:30:15.123", "10:30:15", "10:30:15.000000500",
            "2024-03-01 10:30:15.123", "2024-03-01T10:30:15Z", "2024-03-01 10:30:15.000000001", "2024-03-01T10:30:15Z", "2024-03-01 10:30:15",
            "-0.05", "12345.0000", "100000000000000000000.0000000001", "-12.50", "1.500",
        ]);
        // The null row: a value, then 28 empty cells.
        assert_eq!(lines.next().unwrap(), format!("false{}", ",".repeat(28)));
        assert_eq!(lines.next(), None);
        assert_eq!((info.rows, info.cols, info.sheet), (2, 29, None));
    }

    #[test]
    fn numbers_follow_the_delimiter() {
        let dir = tmpdir("pq-comma");
        let p = dir.join("t.parquet");
        let schema = "message t { REQUIRED DOUBLE f; REQUIRED INT64 d (DECIMAL(10,2)); REQUIRED BINARY s (UTF8); }";
        low_level(&p, schema, WriterProperties::default(), 1, |_, rg| {
            put!(rg, DoubleType, &[1.5, 1e-5, -2.25e300], None);
            put!(rg, PqI64, &[1250, 5, -1], None);
            put!(rg, ByteArrayType, &[ByteArray::from("a;b"), ByteArray::from("q\"r"), ByteArray::from("x\ny")], None);
        });
        assert_eq!(csv_of(&p, b';').unwrap().0, "f;d;s\n1,5;12,50;\"a;b\"\n0,00001;0,05;\"q\"\"r\"\n-2,25e+300;-0,01;\"x\ny\"\n");
        assert_eq!(csv_of(&p, b',').unwrap().0, "f,d,s\n1.5,12.50,a;b\n0.00001,0.05,\"q\"\"r\"\n-2.25e+300,-0.01,\"x\ny\"\n");
    }

    #[test]
    fn floats_without_an_exponent() {
        let f = |x: f64| { let mut s = String::new(); push_float(x, false, &mut s); s };
        for (x, want) in [(0.1, "0.1"), (3.0, "3"), (-0.0, "0"), (1e15, "1000000000000000"), (1e16, "10000000000000000"), (1.5e20, "150000000000000000000"),
                          (1e-5, "0.00001"), (-1.25e-7, "-0.000000125"), (1e-20, "0.00000000000000000001"), (123456789012345680.0, "123456789012345680"),
                          (1e21, "1e+21"), (2.5e-22, "2.5e-22"), (1.7976931348623157e308, "1.7976931348623157e+308"), (5e-324, "5e-324"), (0.1 + 0.2, "0.30000000000000004")] {
            assert_eq!(f(x), want);
            assert_eq!(f(x).parse::<f64>().unwrap(), x, "{want} reads back");
        }
        assert_eq!((f(f64::NAN), f(f64::INFINITY), f(f64::NEG_INFINITY)), ("nan".to_string(), "inf".to_string(), "-inf".to_string()));
        let mut s = String::new();
        push_float(0.1f32, true, &mut s);
        assert_eq!(s, "0,1");
    }

    #[test]
    fn decimals_keep_their_digits() {
        let f = |d: &str, sc: i8| { let mut s = String::new(); push_decimal(d, sc, false, &mut s); s };
        assert_eq!(f("1250", 2), "12.50");
        assert_eq!(f("-5", 3), "-0.005");
        assert_eq!(f("0", 2), "0.00");
        assert_eq!(f("12", 0), "12");
        assert_eq!(f("12", -2), "1200");
        assert_eq!(f("-125", 3), "-0.125");
    }

    #[test]
    fn timestamps_before_1970() {
        let f = |v: i64, per: i64, z: bool| { let mut s = String::new(); push_stamp(v, per, z, Shape::Plain, &mut s); s };
        assert_eq!(f(-1, 1000, false), "1969-12-31 23:59:59.999");
        assert_eq!(f(-1_500_000, 1_000_000, true), "1969-12-31T23:59:58.500Z");
        assert_eq!(f(0, 1_000_000_000, false), "1970-01-01 00:00:00");
    }

    #[test]
    fn nested_as_json() {
        let dir = tmpdir("pq-nested");
        let p = dir.join("n.parquet");
        let mut l = ListBuilder::new(Int32Builder::new());
        l.values().append_value(1);
        l.values().append_null();
        l.values().append_value(3);
        l.append(true);
        l.append(false);
        l.append(true); // empty
        let list = l.finish();
        let mut m = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
        m.keys().append_value("a\"b");
        m.values().append_value(1);
        m.append(true).unwrap();
        m.append(false).unwrap();
        m.keys().append_value("c");
        m.values().append_null();
        m.append(true).unwrap();
        let map = m.finish();
        let st = StructArray::from(vec![
            (Arc::new(Field::new("x", DataType::Int32, true)), Arc::new(Int32Array::from(vec![Some(1), None, Some(3)])) as ArrayRef),
            (Arc::new(Field::new("y", DataType::Utf8, true)), Arc::new(StringArray::from(vec![Some("é\n"), Some("b"), None])) as ArrayRef),
        ]);
        let cols: Vec<ArrayRef> = vec![Arc::new(list), Arc::new(map), Arc::new(st)];
        let schema = Arc::new(Schema::new(cols.iter().zip(["l", "m", "s"]).map(|(c, n)| Field::new(n, c.data_type().clone(), true)).collect::<Vec<_>>()));
        let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
        let mut w = ArrowWriter::try_new(File::create(&p).unwrap(), schema, None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        let t = text(&p);
        let rows: Vec<&str> = t.lines().collect();
        assert_eq!(rows[0], "l,m,s");
        assert_eq!(rows[1], r#""[1,null,3]","{""a\""b"":1}","{""x"":1,""y"":""é\n""}""#);
        assert_eq!(rows[2], r#",,"{""x"":null,""y"":""b""}""#);
        assert_eq!(rows[3], r#"[],{"c":null},"{""x"":3,""y"":null}""#.replace("{\"c\":null}", "\"{\"\"c\"\":null}\""));
    }

    #[test]
    fn nested_by_levels() {
        // list<struct<a int, b timestamp>>, written by levels: a struct inside a list.
        let dir = tmpdir("pq-levels");
        let p = dir.join("l.parquet");
        let schema = "message t {
            OPTIONAL GROUP l (LIST) {
                REPEATED GROUP list {
                    OPTIONAL GROUP element {
                        OPTIONAL INT32 a;
                        OPTIONAL DOUBLE f;
                    }
                }
            }
            OPTIONAL INT32 k;
        }";
        low_level(&p, schema, WriterProperties::default(), 1, |_, rg| {
            // rows: [{a:1,f:null},{a:2,f:null}], null, []
            put!(rg, PqI32, &[1, 2], Some(&[4, 4, 0, 1]), Some(&[0, 1, 0, 0]));
            put!(rg, DoubleType, &[f64::NAN], Some(&[4, 3, 0, 1]), Some(&[0, 1, 0, 0]));
            put!(rg, PqI32, &[7, 8, 9], Some(&[1, 1, 1]));
        });
        assert_eq!(text(&p), "l,k\n\"[{\"\"a\"\":1,\"\"f\"\":\"\"nan\"\"},{\"\"a\"\":2,\"\"f\"\":null}]\",7\n,8\n[],9\n");
    }

    /// `n` rows of an id, a repeated label (a dictionary's case), a float and nulls, over `groups`.
    fn sample(p: &Path, props: WriterProperties, n: usize, groups: usize) -> String {
        let schema = "message t { REQUIRED INT64 id; OPTIONAL BINARY s (UTF8); OPTIONAL DOUBLE f; REQUIRED BINARY u (UTF8); }";
        let mut want = String::from("id,s,f,u\n");
        low_level(p, schema, props, groups, |g, rg| {
            let ids: Vec<i64> = (0..n as i64).map(|i| i + (g * n) as i64).collect();
            let defs: Vec<i16> = ids.iter().map(|i| (i % 7 != 3) as i16).collect();
            let strs: Vec<ByteArray> = ids.iter().filter(|i| *i % 7 != 3).map(|i| ByteArray::from(["alpha", "béta", "γ", "δδ"][(*i % 4) as usize])).collect();
            let fs: Vec<f64> = ids.iter().filter(|i| *i % 7 != 3).map(|i| *i as f64 / 8.0).collect();
            let us: Vec<ByteArray> = ids.iter().map(|i| ByteArray::from(format!("row number {i}").as_str())).collect();
            put!(rg, PqI64, &ids, None);
            put!(rg, ByteArrayType, &strs, Some(&defs));
            put!(rg, DoubleType, &fs, Some(&defs));
            put!(rg, ByteArrayType, &us, None);
            for &i in &ids {
                let (s, f) = if i % 7 != 3 { (["alpha", "béta", "γ", "δδ"][(i % 4) as usize].to_string(), format!("{}", i as f64 / 8.0)) } else { Default::default() };
                want += &format!("{i},{s},{f},row number {i}\n");
            }
        });
        want
    }

    #[test]
    fn codecs_pages_and_dictionaries() {
        let dir = tmpdir("pq-codecs");
        let codecs = [Compression::UNCOMPRESSED, Compression::SNAPPY, Compression::GZIP(GzipLevel::default()), Compression::ZSTD(ZstdLevel::default()),
                      Compression::LZ4, Compression::LZ4_RAW];
        for (k, c) in codecs.into_iter().enumerate() {
            for v in [WriterVersion::PARQUET_1_0, WriterVersion::PARQUET_2_0] {
                for dict in [true, false] {
                    let p = dir.join(format!("{k}-{v:?}-{dict}.parquet"));
                    let props = WriterProperties::builder().set_compression(c).set_writer_version(v).set_dictionary_enabled(dict).set_data_page_size_limit(4096).build();
                    let want = sample(&p, props, 3000, 3);
                    assert_eq!(text(&p), want, "{c:?} {v:?} dictionary {dict}");
                }
            }
        }
    }

    #[test]
    fn other_encodings() {
        let dir = tmpdir("pq-enc");
        let path = |c: &str| ColumnPath::from(c);
        for (k, (ints, strs, fl)) in [(Encoding::DELTA_BINARY_PACKED, Encoding::DELTA_BYTE_ARRAY, Encoding::BYTE_STREAM_SPLIT), (Encoding::PLAIN, Encoding::DELTA_LENGTH_BYTE_ARRAY, Encoding::PLAIN)].into_iter().enumerate() {
            let p = dir.join(format!("{k}.parquet"));
            let props = WriterProperties::builder().set_dictionary_enabled(false).set_writer_version(WriterVersion::PARQUET_2_0)
                .set_column_encoding(path("id"), ints).set_column_encoding(path("s"), strs).set_column_encoding(path("u"), strs).set_column_encoding(path("f"), fl).build();
            let want = sample(&p, props, 2000, 2);
            assert_eq!(text(&p), want);
        }
    }

    #[test]
    fn row_groups_stay_in_order() {
        // Many small groups on every thread: the slowest never lets a later one pass.
        let dir = tmpdir("pq-order");
        let p = dir.join("o.parquet");
        let want = sample(&p, WriterProperties::builder().set_compression(Compression::SNAPPY).build(), 200, 64);
        let progress = AtomicUsize::new(0);
        let mut out = Vec::new();
        let info = to_csv(&p, b',', &mut out, Some(&progress), &AtomicBool::new(false)).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), want);
        assert_eq!((info.rows, progress.into_inner()), (12800, 12800));
    }

    #[test]
    fn gate_bounds_all_but_the_next_group() {
        let g = Gate::new();
        assert!(g.enter(0, HELD));
        // Over the bound: the next group goes on, another waits.
        assert!(g.enter(0, 10));
        std::thread::scope(|s| {
            let t = s.spawn(|| g.enter(1, 10));
            std::thread::sleep(Duration::from_millis(30));
            assert!(!t.is_finished());
            g.advance(1);
            assert!(t.join().unwrap());
        });
        std::thread::scope(|s| {
            let t = s.spawn(|| g.enter(2, 10));
            std::thread::sleep(Duration::from_millis(30));
            assert!(!t.is_finished());
            g.leave(HELD + 10);
            assert!(t.join().unwrap());
        });
        g.leave(20);
        assert!(g.enter(3, HELD));
        std::thread::scope(|s| {
            let t = s.spawn(|| g.enter(4, HELD));
            g.halt();
            assert!(!t.join().unwrap());
        });
    }

    #[test]
    fn zero_rows_and_nulls() {
        let dir = tmpdir("pq-empty");
        let p = dir.join("none.parquet");
        low_level(&p, "message t { OPTIONAL INT32 a; OPTIONAL BINARY b (UTF8); }", WriterProperties::default(), 0, |_, _| {});
        assert_eq!(csv_of(&p, b';').unwrap(), ("a;b\n".to_string(), Imported { cols: 2, ..Default::default() }));
        let p = dir.join("empty-group.parquet");
        low_level(&p, "message t { OPTIONAL INT32 a; }", WriterProperties::default(), 1, |_, rg| put!(rg, PqI32, &[], Some(&[])));
        assert_eq!(text(&p), "a\n");
        // A single column of nulls: rows, not blank lines.
        let p = dir.join("nulls.parquet");
        low_level(&p, "message t { OPTIONAL INT32 a; }", WriterProperties::default(), 1, |_, rg| put!(rg, PqI32, &[], Some(&[0, 0])));
        assert_eq!(text(&p), "a\n\"\"\n\"\"\n");
        let p = dir.join("null-type.parquet");
        low_level(&p, "message t { OPTIONAL INT32 a (UNKNOWN); REQUIRED INT32 b; }", WriterProperties::default(), 1, |_, rg| {
            put!(rg, PqI32, &[], Some(&[0, 0]));
            put!(rg, PqI32, &[1, 2], None);
        });
        assert_eq!(text(&p), "a,b\n,1\n,2\n");
    }

    #[test]
    fn wide_and_unicode_names() {
        let dir = tmpdir("pq-wide");
        let p = dir.join("w.parquet");
        let schema = format!("message t {{ {} }}", (0..3000).map(|i| format!("REQUIRED INT32 c{i};")).collect::<String>());
        low_level(&p, &schema, WriterProperties::default(), 1, |_, rg| {
            for i in 0..3000 { put!(rg, PqI32, &[i, -i], None); }
        });
        let t = text(&p);
        let mut l = t.lines();
        assert_eq!(l.next().unwrap().split(',').count(), 3000);
        assert_eq!(l.next().unwrap().split(',').nth(2999), Some("2999"));
        assert_eq!(l.next().unwrap().split(',').nth(2999), Some("-2999"));
        let p = dir.join("u.parquet");
        let f = Field::new("clé 🔑;\"x\"", DataType::Utf8, false);
        let schema = Arc::new(Schema::new(vec![f]));
        let b = RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(vec!["日本語", "🦀", "\u{202f}é"])) as ArrayRef]).unwrap();
        let mut w = ArrowWriter::try_new(File::create(&p).unwrap(), schema, None).unwrap();
        w.write(&b).unwrap();
        w.close().unwrap();
        assert_eq!(csv_of(&p, b';').unwrap().0, "\"clé 🔑;\"\"x\"\"\"\n日本語\n🦀\n\u{202f}é\n");
    }

    #[test]
    fn what_cannot_be_read_is_said() {
        let dir = tmpdir("pq-bad");
        let p = dir.join("x.parquet");
        assert_eq!(csv_of(&p, b',').unwrap_err(), "file not found");
        for (name, body, says) in [("empty", &b""[..], "empty"), ("text", b"a,b\n1,2\n3,4\n5,6\n", "not a readable Parquet"), ("magic", b"PAR1", "not a readable Parquet"), ("short", b"PAR1aaaaaaaaaaaaaaaaaaa", "cut short"), ("enc", b"PAREaaaaaaaaaaaaaPARE", "encrypted")] {
            std::fs::write(&p, body).unwrap();
            let e = csv_of(&p, b',').unwrap_err();
            assert!(e.contains(says), "{name}: {e}");
        }
        let p = dir.join("iv.parquet");
        low_level(&p, "message t { OPTIONAL FIXED_LEN_BYTE_ARRAY (12) iv (INTERVAL); }", WriterProperties::default(), 1, |_, rg| {
            put!(rg, FixedLenByteArrayType, &[flba(&[1; 12])], Some(&[1]));
        });
        let e = csv_of(&p, b',').unwrap_err();
        assert!(e.contains("\"iv\"") && e.contains("INTERVAL"), "{e}");
    }

    #[test]
    fn other_codecs_are_refused_by_name() {
        // The footer's codec of a column changed, as a Hadoop job's file would have it.
        let dir = tmpdir("pq-lzo");
        let p = dir.join("l.parquet");
        low_level(&p, "message t { REQUIRED INT32 a; }", WriterProperties::builder().set_compression(Compression::UNCOMPRESSED).build(), 1, |_, rg| put!(rg, PqI32, &[1, 2, 3], None));
        let good = std::fs::read(&p).unwrap();
        let foot = u32::from_le_bytes(good[good.len() - 8..good.len() - 4].try_into().unwrap()) as usize;
        let at = good.len() - 8 - foot;
        // ColumnMetaData: type (1), encodings (2), path (3), codec (4): the zigzag zero after the path "a".
        let k = (at..good.len() - 8).find(|&i| good[i] == b'a' && good[i + 1] == 0x15 && good[i + 2] == 0).unwrap();
        for (code, name) in [(3, "LZO"), (4, "brotli")] {
            let mut b = good.clone();
            b[k + 2] = code * 2; // zigzag
            std::fs::write(&p, &b).unwrap();
            let e = csv_of(&p, b',').unwrap_err();
            assert!(e.contains("\"a\"") && e.contains(name), "{e}");
        }
    }

    #[test]
    fn damaged_files_give_errors() {
        let dir = tmpdir("pq-damage");
        for (k, c) in [Compression::UNCOMPRESSED, Compression::SNAPPY, Compression::ZSTD(ZstdLevel::default()), Compression::GZIP(GzipLevel::default())].into_iter().enumerate() {
            let p = dir.join(format!("{k}.parquet"));
            sample(&p, WriterProperties::builder().set_compression(c).build(), 300, 3);
            let good = std::fs::read(&p).unwrap();
            let q = dir.join(format!("{k}-bad.parquet"));
            // Cut anywhere: an error.
            for cut in [good.len() - 1, good.len() - 9, good.len() / 2, 13] {
                std::fs::write(&q, &good[..cut]).unwrap();
                assert!(csv_of(&q, b',').is_err(), "cut at {cut}");
            }
            // The footer's length wrong: an error.
            let mut b = good.clone();
            let n = b.len();
            b[n - 6] ^= 0x7f;
            std::fs::write(&q, &b).unwrap();
            assert!(csv_of(&q, b',').is_err());
            // Bytes changed anywhere: an error or text, never a panic out of here, a hang or
            // an abort. A changed value is seen only where a page has a checksum or a codec.
            let mut failures = 0;
            for i in (4..good.len() - 8).step_by(good.len() / 200) {
                let mut b = good.clone();
                b[i] ^= 0xa5;
                std::fs::write(&q, &b).unwrap();
                failures += csv_of(&q, b',').is_err() as usize;
            }
            assert!(failures > 0);
        }
    }

    #[test]
    fn cancel_stops_the_threads() {
        let dir = tmpdir("pq-cancel");
        let p = dir.join("c.parquet");
        sample(&p, WriterProperties::default(), 500, 200);
        let mut out = Vec::new();
        assert_eq!(to_csv(&p, b',', &mut out, None, &AtomicBool::new(true)).unwrap_err(), super::super::CANCELLED);
        // Once under way too; the file left by a cancelled import goes (write_beside).
        let cancel = AtomicBool::new(false);
        struct Stop<'a>(&'a AtomicBool, usize);
        impl Write for Stop<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.1 += b.len();
                if self.1 > 1000 { self.0.store(true, Ordering::Relaxed); }
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        assert_eq!(to_csv(&p, b',', Stop(&cancel, 0), None, &cancel).unwrap_err(), super::super::CANCELLED);
        let dest = dir.join("c.csv");
        assert_eq!(super::super::import(super::super::Format::Parquet, &p, None, b',', &dest, false, None, &AtomicBool::new(true)).unwrap_err(), super::super::CANCELLED);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn a_failing_write_stops_the_threads() {
        let dir = tmpdir("pq-full");
        let p = dir.join("c.parquet");
        sample(&p, WriterProperties::default(), 500, 50);
        struct Full(usize);
        impl Write for Full {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                if self.0 > 3 { Err(std::io::Error::other("disk full")) } else { Ok(b.len()) }
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        assert_eq!(to_csv(&p, b',', Full(0), None, &AtomicBool::new(false)).unwrap_err(), "disk full");
    }

    #[test]
    fn opened_as_the_other_imports() {
        let dir = tmpdir("pq-import");
        let p = dir.join("data.parquet");
        let want = sample(&p, WriterProperties::default(), 100, 2);
        let dest = super::super::csv_path_for(&p, None);
        assert_eq!(dest, dir.join("data.csv"));
        let info = super::super::import(super::super::Format::Parquet, &p, None, b',', &dest, false, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), want);
        assert_eq!((info.rows, info.cols, info.size), (200, 4, want.len() as u64));
        // Over a CSV already there: a dated copy is kept.
        let again = super::super::import(super::super::Format::Parquet, &p, None, b',', &dest, true, None, &AtomicBool::new(false)).unwrap();
        assert!(again.backup.is_some_and(|b| b.exists()));
        assert_eq!(super::super::import_note("data.parquet", "data.csv", super::super::Format::Parquet, &info), "data.parquet → data.csv: 200 rows, 4 columns.");
    }
}
