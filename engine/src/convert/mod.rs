//! Conversions between CSV and other formats. The editor only edits CSV: an
//! Excel workbook, a SQLite database, a JSON or a Parquet file is first written
//! as a CSV beside it (`<name>.csv`, `<name> - <sheet>.csv`), and that CSV is
//! what opens; a table is exported as a formatted workbook, a typed database
//! or a typed Parquet file.
//!
//! Ported from the web app's bridge (`xlsx_reader.py`, `xlsx_writer.py`,
//! `xlsx_columns.py`, `sqlite_io.py`) and its JSON import (`27-import.js`),
//! with the same rules: what a cell becomes, which sheet is picked, how a
//! column is typed. Long operations report progress and can be cancelled:
//! they then return `Err(CANCELLED)`.

mod columns;
pub mod json;
pub mod parquet_read;
pub mod parquet_write;
pub mod sqlite;
pub mod xlsx_read;
pub mod xlsx_write;

use crate::table::{RowId, State};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The error a cancelled operation returns.
pub const CANCELLED: &str = "cancelled";

/// What a file is converted from or to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xlsx,
    Sqlite,
    Json,
    Parquet,
}

impl Format {
    /// What a sheet is called in this format, for messages and pickers.
    pub fn noun(self) -> &'static str {
        if self == Format::Sqlite { "table" } else { "sheet" }
    }
}

/// The format a file is opened through, from its extension (None: a CSV or
/// text file, opened as it is).
pub fn import_format(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "xlsx" | "xlsm" => Format::Xlsx,
        "json" | "jsonl" | "ndjson" => Format::Json,
        "sqlite" | "sqlite3" | "db" | "db3" => Format::Sqlite,
        "parquet" | "parq" | "pqt" => Format::Parquet,
        _ => return None,
    })
}

/// A sheet of a workbook, or a table (or view) of a database, as a picker shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sheet {
    pub name: String,
    pub hidden: bool,
    /// Holds at least one value.
    pub filled: bool,
    /// A database view.
    pub view: bool,
    /// (rows, columns), when known without reading the data.
    pub dim: Option<(u64, u64)>,
    /// `dim`'s row count is an upper bound (a table's max(rowid)).
    pub approx: bool,
}

/// The sheets worth offering: visible, with at least one value. A picker is
/// needed only when there are several; with one or none, the CSV takes the
/// first of these (else the first sheet) and, for a workbook, no sheet name.
pub fn offered(sheets: &[Sheet]) -> Vec<&Sheet> {
    sheets.iter().filter(|s| s.filled && !s.hidden).collect()
}

/// Where the CSV of `src` goes: beside it, `<stem>.csv`, or `<stem> - <sheet>.csv`
/// when a sheet was picked (always, for a database: it is named after its table).
pub fn csv_path_for(src: &Path, sheet: Option<&str>) -> PathBuf {
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // As the page did: the last extension off, whatever it is.
    let stem = match name.rfind('.') {
        Some(k) if k + 1 < name.len() => &name[..k],
        _ => &name[..],
    };
    let mut out = stem.to_string();
    if let Some(sh) = sheet {
        let mut clean = String::with_capacity(sh.len());
        let mut run = false;
        for c in sh.chars() {
            let bad = matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 0x20;
            if bad && !run { clean.push('_'); }
            if !bad { clean.push(c); }
            run = bad;
        }
        out.push_str(" - ");
        out.push_str(clean.trim());
    }
    out.push_str(".csv");
    src.with_file_name(out)
}

/// Whether files can be made in `dir` (not a read-only mount, an archive's view, a share or a
/// folder of someone else's).
pub fn dir_writable(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
        // access() answers for the mount too (EROFS); no file is made.
        std::ffi::CString::new(dir.as_os_str().as_bytes()).is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
    }
    #[cfg(not(unix))]
    {
        let probe = dir.join(format!(".csvfab-{}.probe", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(_) => { let _ = std::fs::remove_file(&probe); true }
            Err(_) => false,
        }
    }
}

/// Where a CSV goes when the folder of what it is made from cannot be written: the user's
/// documents folder (XDG's, else ~/Documents), else the home folder, if it can be.
pub fn fallback_dir() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)?;
    let xdg = std::env::var_os("XDG_DOCUMENTS_DIR").map(PathBuf::from).or_else(|| {
        let conf = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        let text = std::fs::read_to_string(conf.join("user-dirs.dirs")).ok()?;
        let v = text.lines().find_map(|l| l.trim().strip_prefix("XDG_DOCUMENTS_DIR="))?.trim().trim_matches('"').to_string();
        Some(match v.strip_prefix("$HOME") { Some(r) => home.join(r.trim_start_matches('/')), None => PathBuf::from(v) })
    });
    [xdg, Some(home.join("Documents")), Some(home)].into_iter().flatten().find(|d| d.is_dir() && dir_writable(d))
}

/// The stem of a table's file name, without the extension of a CSV, a text
/// file, a workbook, a database or a Parquet file.
fn stem_of(name: &str) -> &str {
    for ext in [".csv", ".tsv", ".txt", ".xlsx", ".sqlite", ".parquet"] {
        if name.len() > ext.len() && name.is_char_boundary(name.len() - ext.len()) && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// The file name an export suggests for the table file `name`: `clients.csv` → `clients.xlsx`.
pub fn export_name(name: &str, to: Format) -> String {
    let ext = match to { Format::Xlsx => ".xlsx", Format::Sqlite => ".sqlite", Format::Json => ".json", Format::Parquet => ".parquet" };
    format!("{}{ext}", stem_of(name))
}

/// The sheet (or table) name an export gets: its file's stem.
pub fn export_sheet_name(file_name: &str) -> String {
    let s = stem_of(file_name);
    for ext in [".sqlite3", ".db3", ".db"] {
        if s.len() > ext.len() && s.is_char_boundary(s.len() - ext.len()) && s[s.len() - ext.len()..].eq_ignore_ascii_case(ext) {
            return s[..s.len() - ext.len()].to_string();
        }
    }
    s.to_string()
}

/// What an import wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// The sheet or table converted (None for JSON and Parquet).
    pub sheet: Option<String>,
    /// Other sheets or tables holding data, not converted.
    pub others: usize,
    pub rows: u64,
    pub cols: usize,
    /// Bytes written, when written to a file.
    pub size: u64,
    /// The copy kept of the CSV this one replaced.
    pub backup: Option<PathBuf>,
}

/// The note shown once a file is converted: `book.xlsx → book.csv: 4 rows, sheet "Data", 6 columns.`
pub fn import_note(from: &str, to: &str, format: Format, j: &Imported) -> String {
    let noun = format.noun();
    let mut s = format!("{from} → {to}: {} rows", group(j.rows));
    if let Some(sh) = &j.sheet {
        let _ = write!(s, ", {noun} \"{sh}\"");
    }
    if j.others > 0 {
        let _ = write!(s, " ({} other {noun}{} with data not converted)", j.others, if j.others > 1 { "s" } else { "" });
    }
    if j.cols > 0 {
        let _ = write!(s, ", {} columns", group(j.cols as u64));
    }
    s.push('.');
    s
}

/// 1234567 → "1,234,567" (the page's fmt()).
fn group(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::with_capacity(d.len() + d.len() / 3);
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) { out.push(','); }
        out.push(c);
    }
    out
}

/// Converts `src` into a CSV at `dest`, atomically; `backup`: keep a dated
/// copy of a CSV already there. `sheet`: the sheet or table (None: the first
/// visible one with data). `delim`: the CSV's delimiter — a `;` file gets
/// decimal commas, the French spreadsheet convention.
/// Progress: rows (records, for JSON) done.
#[allow(clippy::too_many_arguments)]
pub fn import(format: Format, src: &Path, sheet: Option<&str>, delim: u8, dest: &Path, backup: bool, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let (mut info, size, bak) = write_beside(dest, backup, |w| match format {
        Format::Xlsx => xlsx_read::to_csv(src, sheet, delim, w, progress, cancel),
        Format::Sqlite => sqlite::to_csv(src, sheet, delim, w, progress, cancel),
        Format::Json => json::to_csv(src, delim, w, progress, cancel),
        Format::Parquet => parquet_read::to_csv(src, delim, w, progress, cancel),
    })?;
    info.size = size;
    info.backup = bak;
    Ok(info)
}

/// What an export wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exported {
    /// Rows written, a header row included.
    pub rows: u64,
    pub cols: usize,
    /// Rows or columns left out past Excel's limits.
    pub truncated_rows: bool,
    pub truncated_cols: bool,
    /// Cells cut to Excel's 32,767 characters.
    pub truncated_cells: u64,
    /// SQLite, Parquet: each column's name and type.
    pub types: Vec<(String, &'static str)>,
}

/// Exports the table to `dest` (atomically), as a workbook (one sheet named
/// `name`), a database (one table named `name`) or a Parquet file (`name` has
/// no use there). `rows`: the data rows to
/// write, in order (None: all of them); the header row is the table's own.
/// Progress: rows done, twice over (columns are typed in a first pass).
pub fn export(format: Format, st: &State, rows: Option<&[RowId]>, dest: &Path, name: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    match format {
        Format::Xlsx => xlsx_write::write(st, rows, dest, name, progress, cancel),
        Format::Sqlite => sqlite::write(st, rows, dest, name, progress, cancel),
        Format::Parquet => parquet_write::write(st, rows, dest, progress, cancel),
        Format::Json => Err("no JSON export".into()),
    }
}

// --- shared helpers -----------------------------------------------------------

#[inline]
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) { Err(CANCELLED.into()) } else { Ok(()) }
}

#[inline]
fn tick(progress: Option<&AtomicUsize>, n: usize) {
    if let Some(p) = progress { p.fetch_add(n, Ordering::Relaxed); }
}

/// The data rows to export: the given ids, or every data row in file order.
struct Rows<'a> {
    st: &'a State,
    ids: Option<&'a [RowId]>,
}

impl Rows<'_> {
    fn len(&self) -> usize {
        self.ids.map_or(self.st.len(), |r| r.len())
    }
    #[inline]
    fn id(&self, k: usize) -> RowId {
        self.ids.map_or_else(|| self.st.data_id(k), |r| r[k])
    }
}

/// CSV as Python's csv module writes it (the bridge's output): a field is
/// quoted when it holds the delimiter, a quote or a line break; `\n` ends a
/// line. `py`: a row of a single empty field is written `""` (the JS writer
/// leaves the line empty).
struct CsvOut<W: Write> {
    w: W,
    d: u8,
    py: bool,
    nf: usize,
    first_empty: bool,
}

impl<W: Write> CsvOut<W> {
    fn new(w: W, d: u8, py: bool) -> Self {
        CsvOut { w, d, py, nf: 0, first_empty: false }
    }

    fn field(&mut self, s: &str) -> io::Result<()> {
        if self.nf > 0 { self.w.write_all(&[self.d])?; }
        self.nf += 1;
        if self.nf == 1 { self.first_empty = s.is_empty(); }
        let d = self.d;
        if s.bytes().any(|b| b == d || b == b'"' || b == b'\n' || b == b'\r') {
            self.w.write_all(b"\"")?;
            let mut rest = s.as_bytes();
            while let Some(k) = memchr::memchr(b'"', rest) {
                self.w.write_all(&rest[..=k])?;
                self.w.write_all(b"\"")?;
                rest = &rest[k + 1..];
            }
            self.w.write_all(rest)?;
            self.w.write_all(b"\"")
        } else {
            self.w.write_all(s.as_bytes())
        }
    }

    fn end(&mut self) -> io::Result<()> {
        if self.py && self.nf == 1 && self.first_empty { self.w.write_all(b"\"\"")?; }
        self.nf = 0;
        self.w.write_all(b"\n")
    }
}

fn io_err(e: io::Error) -> String {
    e.to_string()
}

/// A temporary name beside `dest`, hidden, unique to this process.
fn temp_beside(dest: &Path) -> PathBuf {
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    dest.with_file_name(format!(".{name}.csvfab-{}-{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
}

fn check_dir(dest: &Path) -> Result<(), String> {
    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !dir.is_dir() {
        return Err(format!("no such folder: {}", dir.display()));
    }
    Ok(())
}

/// Writes `dest` through `f`, atomically: a synced temporary file beside it,
/// renamed over it (keeping its permissions), after a dated copy of the old
/// one when `backup`. Returns f's result, the size written and the copy.
fn write_beside<T>(dest: &Path, backup: bool, f: impl FnOnce(&mut BufWriter<File>) -> Result<T, String>) -> Result<(T, u64, Option<PathBuf>), String> {
    check_dir(dest)?;
    let tmp = temp_beside(dest);
    let res = (|| {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp).map_err(io_err)?);
        let out = f(&mut w)?;
        let file = w.into_inner().map_err(|e| e.into_error().to_string())?;
        file.sync_all().map_err(io_err)?;
        let size = file.metadata().map_err(io_err)?.len();
        drop(file);
        let bak = replace(&tmp, dest, backup).map_err(io_err)?;
        Ok((out, size, bak))
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// `tmp` renamed over `dest`, with dest's permissions, after a backup of it.
fn replace(tmp: &Path, dest: &Path, backup: bool) -> io::Result<Option<PathBuf>> {
    #[cfg(unix)]
    if let Ok(m) = std::fs::metadata(dest) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(m.permissions().mode() & 0o7777));
    }
    let bak = if backup { backup_of(dest)? } else { None };
    std::fs::rename(tmp, dest)?;
    // The rename is durable only once the folder is synced.
    #[cfg(unix)]
    if let Some(dir) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(d) = File::open(dir) { let _ = d.sync_all(); }
    }
    Ok(bak)
}

/// A dated copy `<path>.<stamp>.bak` beside `path` (`-2`, `-3`… within the same
/// second: never over an existing copy); None when there is no file to copy.
fn backup_of(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = (now / 86400) as i64;
    let (y, mo, d) = civil_from_days(days);
    let s = now % 86400;
    let base = format!("{}.{y:04}{mo:02}{d:02}-{:02}{:02}{:02}", path.display(), s / 3600, s % 3600 / 60, s % 60);
    for k in 1.. {
        let made = PathBuf::from(if k == 1 { format!("{base}.bak") } else { format!("{base}-{k}.bak") });
        // A hard link is the old file itself, kept for free: the rename gives `path` a new inode.
        match std::fs::hard_link(path, &made) {
            Ok(()) => return Ok(Some(made)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {}
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&made) {
            Ok(mut f) => {
                io::copy(&mut File::open(path)?, &mut f)?;
                return Ok(Some(made));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

// --- dates and numbers ---------------------------------------------------------

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The current year, for two-digit years.
fn this_year() -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_from_days((now / 86400) as i64).0
}

/// An Excel serial date as text: `yyyy-mm-dd`, plus ` hh:mm` or ` hh:mm:ss`
/// when it has a time; None outside years 1…9999.
fn date_text(x: f64, d1904: bool, out: &mut String) -> bool {
    // Python's round(): to the nearest second, ties to even.
    let secs = (x * 86400.0).round_ties_even();
    if !secs.is_finite() || secs.abs() > 4e11 {
        return false;
    }
    let secs = secs as i64;
    let base = if d1904 { days_from_civil(1904, 1, 1) } else { days_from_civil(1899, 12, 30) };
    let (y, m, d) = civil_from_days(base + secs.div_euclid(86400));
    if !(1..=9999).contains(&y) {
        return false;
    }
    let t = secs.rem_euclid(86400);
    let (h, mi, s) = (t / 3600, t % 3600 / 60, t % 60);
    let _ = write!(out, "{y:04}-{m:02}-{d:02}");
    if t != 0 {
        let _ = write!(out, " {h:02}:{mi:02}");
        if s != 0 { let _ = write!(out, ":{s:02}"); }
    }
    true
}

/// A number's shortest round-trip digits and decimal exponent: x = 0.d1d2… × 10^exp.
fn shortest(x: f64) -> ([u8; 24], usize, i32) {
    struct Buf([u8; 40], usize);
    impl std::fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let b = s.as_bytes();
            let end = (self.1 + b.len()).min(self.0.len());
            self.0[self.1..end].copy_from_slice(&b[..end - self.1]);
            self.1 = end;
            Ok(())
        }
    }
    let mut b = Buf([0; 40], 0);
    let _ = write!(b, "{:e}", x.abs());
    let s = &b.0[..b.1];
    let e = s.iter().position(|&c| c == b'e').unwrap_or(s.len());
    let exp: i32 = std::str::from_utf8(&s[e + 1..]).ok().and_then(|t| t.parse().ok()).unwrap_or(0);
    let mut digits = [0u8; 24];
    let mut n = 0;
    for &c in &s[..e] {
        if c.is_ascii_digit() && n < 24 { digits[n] = c; n += 1; }
    }
    (digits, n, exp + 1)
}

/// Python's repr() of a float: `1234.5`, `1234.0`, `1e+16`, `1e-05`.
fn py_repr(x: f64, out: &mut String) {
    if !x.is_finite() {
        out.push_str(if x.is_nan() { "nan" } else if x > 0.0 { "inf" } else { "-inf" });
        return;
    }
    if x.is_sign_negative() { out.push('-'); }
    if x == 0.0 {
        out.push_str("0.0");
        return;
    }
    let (dg, n, p) = shortest(x);
    let dg = &dg[..n];
    let e = p - 1;
    let push = |out: &mut String, b: &[u8]| out.push_str(std::str::from_utf8(b).unwrap_or(""));
    if (-4..16).contains(&e) {
        if p <= 0 {
            out.push_str("0.");
            for _ in 0..-p { out.push('0'); }
            push(out, dg);
        } else if p as usize >= n {
            push(out, dg);
            for _ in 0..p as usize - n { out.push('0'); }
            out.push_str(".0");
        } else {
            push(out, &dg[..p as usize]);
            out.push('.');
            push(out, &dg[p as usize..]);
        }
    } else {
        push(out, &dg[..1]);
        if n > 1 {
            out.push('.');
            push(out, &dg[1..]);
        }
        let _ = write!(out, "e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
}

/// A number as the bridge rewrote it: integers without `.0` (below 10^15),
/// else Python's repr; a decimal comma when `comma` (a `;` file).
fn number_text(x: f64, comma: bool, out: &mut String) {
    if x.is_finite() && x == x.trunc() && x.abs() < 1e15 {
        let _ = write!(out, "{}", x as i64);
        return;
    }
    let at = out.len();
    py_repr(x, out);
    if comma {
        if let Some(k) = out[at..].find('.') {
            out.replace_range(at + k..at + k + 1, ",");
        }
    }
}

/// JavaScript's String(number): `1.5`, `100`, `1e+21`, `1.5e-7`.
fn js_number(x: f64, out: &mut String) {
    if !x.is_finite() {
        out.push_str(if x.is_nan() { "NaN" } else if x > 0.0 { "Infinity" } else { "-Infinity" });
        return;
    }
    if x == 0.0 {
        out.push('0');
        return;
    }
    if x < 0.0 { out.push('-'); }
    let (dg, k, n) = shortest(x);
    let dg = &dg[..k];
    let push = |out: &mut String, b: &[u8]| out.push_str(std::str::from_utf8(b).unwrap_or(""));
    let k = k as i32;
    if k <= n && n <= 21 {
        push(out, dg);
        for _ in 0..n - k { out.push('0'); }
    } else if 0 < n && n <= 21 {
        push(out, &dg[..n as usize]);
        out.push('.');
        push(out, &dg[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..-n { out.push('0'); }
        push(out, dg);
    } else {
        push(out, &dg[..1]);
        if k > 1 {
            out.push('.');
            push(out, &dg[1..]);
        }
        let _ = write!(out, "e{}{}", if n - 1 < 0 { '-' } else { '+' }, (n - 1).abs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(f: impl Fn(&mut String)) -> String {
        let mut o = String::new();
        f(&mut o);
        o
    }

    #[test]
    fn number_texts() {
        assert_eq!(s(|o| number_text(1234.0, false, o)), "1234");
        assert_eq!(s(|o| number_text(1234.5, true, o)), "1234,5");
        assert_eq!(s(|o| number_text(0.1, false, o)), "0.1");
        assert_eq!(s(|o| number_text(1e20, false, o)), "1e+20");
        assert_eq!(s(|o| number_text(-0.0, false, o)), "0");
        assert_eq!(s(|o| number_text(1e15, true, o)), "1000000000000000,0");
        assert_eq!(s(|o| py_repr(1e16, o)), "1e+16");
        assert_eq!(s(|o| py_repr(1e-5, o)), "1e-05");
        assert_eq!(s(|o| py_repr(0.0001, o)), "0.0001");
        assert_eq!(s(|o| py_repr(0.1 + 0.2, o)), "0.30000000000000004");
        assert_eq!(s(|o| py_repr(1.5e300, o)), "1.5e+300");
        assert_eq!(s(|o| py_repr(123456789012345678.0, o)), "1.2345678901234568e+17");
        assert_eq!(s(|o| py_repr(-2.5, o)), "-2.5");
    }

    #[test]
    fn read_only_folders_told() {
        let dir = std::env::temp_dir().join(format!("csvfab-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir_writable(&dir));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            // root writes anywhere but on a read-only mount.
            if unsafe { libc::geteuid() } != 0 { assert!(!dir_writable(&dir)); }
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(!dir_writable(&dir.join("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn js_numbers() {
        for (x, want) in [(1.5, "1.5"), (100.0, "100"), (1e21, "1e+21"), (1e20, "100000000000000000000"), (1.5e-7, "1.5e-7"),
                          (0.000001, "0.000001"), (-0.0, "0"), (-12.25, "-12.25"), (123e-20, "1.23e-18"), (0.1 + 0.2, "0.30000000000000004")] {
            assert_eq!(s(|o| js_number(x, o)), want, "{x}");
        }
    }

    #[test]
    fn dates() {
        let d = |x: f64, d1904| { let mut o = String::new(); date_text(x, d1904, &mut o).then_some(o) };
        assert_eq!(d(45092.0, false).unwrap(), "2023-06-15");
        assert_eq!(d(45092.4375, false).unwrap(), "2023-06-15 10:30");
        assert_eq!(d(45092.0 + 37815.0 / 86400.0, false).unwrap(), "2023-06-15 10:30:15");
        assert_eq!(d(0.0, true).unwrap(), "1904-01-01");
        assert_eq!(d(1e12, false), None);
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn names() {
        assert_eq!(csv_path_for(Path::new("/d/book.xlsx"), None), Path::new("/d/book.csv"));
        assert_eq!(csv_path_for(Path::new("/d/book.xlsx"), Some(" Ventes: 2024/T1 ")), Path::new("/d/book - Ventes_ 2024_T1.csv"));
        assert_eq!(csv_path_for(Path::new("/d/base.db"), Some("clients")), Path::new("/d/base - clients.csv"));
        assert_eq!(import_format(Path::new("a.XLSX")), Some(Format::Xlsx));
        assert_eq!(import_format(Path::new("a.ndjson")), Some(Format::Json));
        assert_eq!(import_format(Path::new("a.db3")), Some(Format::Sqlite));
        assert_eq!(import_format(Path::new("a.PARQUET")), Some(Format::Parquet));
        assert_eq!(import_format(Path::new("a.pqt")), Some(Format::Parquet));
        assert_eq!(import_format(Path::new("a.csv")), None);
        assert_eq!(export_name("clients.csv", Format::Xlsx), "clients.xlsx");
        assert_eq!(export_name("notes", Format::Sqlite), "notes.sqlite");
        assert_eq!(export_name("clients.csv", Format::Parquet), "clients.parquet");
        assert_eq!(export_name("clients.parquet", Format::Parquet), "clients.parquet");
        assert_eq!(export_sheet_name("clients.xlsx"), "clients");
        assert_eq!(export_sheet_name("base.db"), "base");
        let j = Imported { sheet: Some("Data".into()), others: 2, rows: 1234, cols: 6, ..Default::default() };
        assert_eq!(import_note("b.xlsx", "b - Data.csv", Format::Xlsx, &j), "b.xlsx → b - Data.csv: 1,234 rows, sheet \"Data\" (2 other sheets with data not converted), 6 columns.");
    }

    #[test]
    fn csv_quoting() {
        let mut o = CsvOut::new(Vec::new(), b';', true);
        for f in ["a", "b;c", "say \"hi\"", "x\ny"] { o.field(f).unwrap(); }
        o.end().unwrap();
        o.field("").unwrap();
        o.end().unwrap();
        assert_eq!(String::from_utf8(o.w).unwrap(), "a;\"b;c\";\"say \"\"hi\"\"\";\"x\ny\"\n\"\"\n");
    }

    // --- benchmarks on the 1M-row file: one step per process, for its own peak memory.
    // CSVFAB_BENCH (the CSV) and CSVFAB_BENCH_OUT (a folder) set where; run e.g.
    // `cargo test -p csvfab-engine --release bench_xlsx_export -- --ignored --nocapture`,
    // the exports before the imports.

    fn bench_paths() -> (PathBuf, PathBuf) {
        let src = std::env::var("CSVFAB_BENCH").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(
            "../benchmark/data/customers-1m.csv"));
        let out = std::env::var("CSVFAB_BENCH_OUT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("csvfab-convert-bench"));
        std::fs::create_dir_all(&out).unwrap();
        (src, out)
    }

    fn peak_mb() -> u64 {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<u64>().ok()).unwrap_or(0) / 1024
    }

    fn bench_table() -> crate::Table {
        let (src, _) = bench_paths();
        let base = crate::Base::open(&src, crate::OpenOptions::default(), None).unwrap();
        crate::Table::new(std::sync::Arc::new(base), None)
    }

    fn report(what: &str, t: std::time::Instant, path: &Path, rows: u64) {
        let size = std::fs::metadata(path).map_or(0, |m| m.len());
        eprintln!("{what}: {rows} rows in {:.2} s, {} MB written, peak RSS {} MB", t.elapsed().as_secs_f64(), size >> 20, peak_mb());
    }

    #[test]
    #[ignore]
    fn bench_xlsx_export() {
        let t = bench_table();
        let dest = bench_paths().1.join("customers.xlsx");
        let t0 = std::time::Instant::now();
        let p = AtomicUsize::new(0);
        let info = export(Format::Xlsx, &t.st, None, &dest, "customers", Some(&p), &AtomicBool::new(false)).unwrap();
        assert_eq!(p.into_inner(), 2 * t.st.len());
        report("CSV → xlsx", t0, &dest, info.rows);
    }

    #[test]
    #[ignore]
    fn bench_xlsx_import() {
        let out = bench_paths().1;
        let dest = out.join("customers-from-xlsx.csv");
        let t0 = std::time::Instant::now();
        let info = import(Format::Xlsx, &out.join("customers.xlsx"), None, b',', &dest, false, None, &AtomicBool::new(false)).unwrap();
        report("xlsx → CSV", t0, &dest, info.rows);
    }

    #[test]
    #[ignore]
    fn bench_sqlite_export() {
        let t = bench_table();
        let dest = bench_paths().1.join("customers.sqlite");
        let t0 = std::time::Instant::now();
        let info = export(Format::Sqlite, &t.st, None, &dest, "customers", None, &AtomicBool::new(false)).unwrap();
        report("CSV → SQLite", t0, &dest, info.rows);
        eprintln!("  types: {:?}", info.types);
    }

    #[test]
    #[ignore]
    fn bench_sqlite_import() {
        let out = bench_paths().1;
        let dest = out.join("customers-from-sqlite.csv");
        let t0 = std::time::Instant::now();
        let info = import(Format::Sqlite, &out.join("customers.sqlite"), None, b',', &dest, false, None, &AtomicBool::new(false)).unwrap();
        report("SQLite → CSV", t0, &dest, info.rows);
    }

    /// The most private memory seen while `f` runs: RSS counts the mapped file too.
    fn peak_anon<R>(f: impl FnOnce() -> R) -> (R, u64) {
        let (stop, top) = (AtomicBool::new(false), AtomicUsize::new(0));
        let r = std::thread::scope(|s| {
            s.spawn(|| while !stop.load(Ordering::Relaxed) {
                let t = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
                let kb = t.lines().find(|l| l.starts_with("RssAnon:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<usize>().ok()).unwrap_or(0);
                top.fetch_max(kb, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(5));
            });
            let r = f();
            stop.store(true, Ordering::Relaxed);
            r
        });
        (r, top.into_inner() as u64 / 1024)
    }

    #[test]
    #[ignore]
    fn bench_parquet_export() {
        let t = bench_table();
        let dest = bench_paths().1.join("bench.parquet");
        let t0 = std::time::Instant::now();
        let (info, anon) = peak_anon(|| export(Format::Parquet, &t.st, None, &dest, "", None, &AtomicBool::new(false)).unwrap());
        report("CSV → Parquet", t0, &dest, info.rows);
        eprintln!("  private memory at most {anon} MB; types: {:?}", info.types);
    }

    #[test]
    #[ignore]
    fn bench_parquet_import() {
        let out = bench_paths().1;
        let t0 = std::time::Instant::now();
        parquet_read::to_csv(&out.join("bench.parquet"), b',', io::sink(), None, &AtomicBool::new(false)).unwrap();
        eprintln!("Parquet → nowhere (what the writer costs): {:.2} s", t0.elapsed().as_secs_f64());
        let dest = out.join("bench-from-parquet.csv");
        let t0 = std::time::Instant::now();
        let (info, anon) = peak_anon(|| import(Format::Parquet, &out.join("bench.parquet"), None, b',', &dest, false, None, &AtomicBool::new(false)).unwrap());
        report("Parquet → CSV", t0, &dest, info.rows);
        eprintln!("  private memory at most {anon} MB");
    }

    #[test]
    #[ignore]
    fn bench_json_import() {
        let out = bench_paths().1;
        let json = out.join("customers.json");
        if !json.exists() {
            // The table as an array of records, numbers as numbers.
            let t = bench_table();
            let h = t.st.headers();
            let mut w = std::io::BufWriter::new(File::create(&json).unwrap());
            w.write_all(b"[").unwrap();
            for k in 0..t.st.len() {
                let mut rec = String::from(if k > 0 { ",\n{" } else { "{" });
                t.st.visit(t.st.data_id(k), |c, v| {
                    if c > 0 { rec.push(','); }
                    let key = if (6..=9).contains(&c) { format!("address.{}", h[c]) } else { h[c].clone() };
                    let num = !v.is_empty() && !v.starts_with('0') && v.parse::<f64>().is_ok();
                    let val = if num { v.to_string() } else { format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")) };
                    let _ = write!(rec, "\"{key}\":{val}");
                });
                rec.push('}');
                w.write_all(rec.as_bytes()).unwrap();
            }
            w.write_all(b"]\n").unwrap();
        }
        let dest = out.join("customers-from-json.csv");
        let t0 = std::time::Instant::now();
        let info = import(Format::Json, &json, None, b',', &dest, false, None, &AtomicBool::new(false)).unwrap();
        report("JSON → CSV", t0, &dest, info.rows);
    }
}
