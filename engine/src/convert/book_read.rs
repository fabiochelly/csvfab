//! An old-format or OpenDocument workbook as CSV (`.xls`, `.xlsb`, `.ods`), through calamine:
//! the same rules as an `.xlsx` (xlsx_read.rs), the sheet read whole first (these formats have
//! no cell-by-cell reader; `.xls` is parsed whole on opening anyway).

use super::{check, date_text, io_err, number_text, tick, CsvOut, Imported, Sheet};
use calamine::{Data, Range, Reader, SheetVisible, Sheets};
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

type Book = Sheets<BufReader<File>>;

fn unreadable(e: impl std::fmt::Display) -> String {
    format!("not a readable workbook ({e})")
}

fn open(path: &Path) -> Result<Book, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    // By the extension, in any case (calamine's own opening wants it in lower case).
    let f = BufReader::with_capacity(1 << 16, File::open(path).map_err(|e| e.to_string())?);
    match ext.as_str() {
        "xls" => calamine::open_workbook_from_rs(f).map(Sheets::Xls).map_err(unreadable),
        "xlsb" => calamine::open_workbook_from_rs(f).map(Sheets::Xlsb).map_err(unreadable),
        _ => calamine::open_workbook_from_rs(f).map(Sheets::Ods).map_err(unreadable),
    }
}

fn has_value(v: &Data) -> bool {
    match v {
        Data::Empty => false,
        Data::String(s) => !s.trim().is_empty(),
        _ => true,
    }
}

fn list(wb: &mut Book) -> Result<Vec<Sheet>, String> {
    let meta = wb.sheets_metadata().to_vec();
    if meta.is_empty() {
        return Err(unreadable("workbook without a sheet"));
    }
    meta.into_iter()
        .map(|m| {
            // A chart sheet has no cells: as an empty one.
            let r = wb.worksheet_range(&m.name).unwrap_or_default();
            let filled = r.cells().any(|(_, _, v)| has_value(v));
            let (h, w) = r.get_size();
            let dim = (h > 0).then_some((h as u64, w as u64));
            Ok(Sheet { hidden: m.visible != SheetVisible::Visible, name: m.name, filled, view: false, dim, approx: false })
        })
        .collect()
}

/// Every sheet with its state, hidden and empty ones included, in tab order.
pub fn sheets(path: &Path) -> Result<Vec<Sheet>, String> {
    list(&mut open(path)?)
}

/// `2024-03-15T10:30:00.000` as the date cells of a workbook read: `2024-03-15 10:30`.
fn iso_date(v: &str, out: &mut String) {
    let v = v.split('.').next().unwrap_or(v);
    let (d, t) = v.split_once('T').unwrap_or((v, ""));
    out.push_str(d);
    let t = t.strip_suffix(":00").unwrap_or(t);
    if !t.is_empty() && t != "00:00" {
        out.push(' ');
        out.push_str(t);
    }
}

/// Whether the workbook counts its dates from 1904 (calamine says so only in a date's debug text).
fn from_1904(dt: &calamine::ExcelDateTime) -> bool {
    format!("{dt:?}").contains("is_1904: true")
}

fn cell_text(v: &Data, comma: bool, d1904: &mut Option<bool>, s: &mut String) {
    use std::fmt::Write;
    match v {
        Data::Empty => {}
        Data::String(v) => s.push_str(v),
        Data::DateTimeIso(v) => iso_date(v, s),
        Data::DurationIso(v) => s.push_str(v),
        Data::Float(x) => number_text(*x, comma, s),
        Data::Int(i) => { let _ = write!(s, "{i}"); }
        Data::Bool(b) => s.push_str(if *b { "TRUE" } else { "FALSE" }),
        Data::DateTime(dt) => {
            if !date_text(dt.as_f64(), *d1904.get_or_insert_with(|| from_1904(dt)), s) {
                number_text(dt.as_f64(), false, s);
            }
        }
        Data::Error(e) => { let _ = write!(s, "{e}"); }
    }
}

/// A sheet's rows as CSV: empty rows left out, the columns before the first kept (as in the
/// sheet), every row as wide as the widest. Progress: rows read.
fn rows_to_csv<W: Write>(r: &Range<Data>, delim: u8, out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<(u64, usize), String> {
    let comma = delim == b';';
    let lead = r.start().map_or(0, |s| s.1 as usize);
    let width = lead + r.width();
    let mut csv = CsvOut::new(out, delim, true);
    let mut cell = String::new();
    let mut rows = 0u64;
    let mut d1904 = None;
    for (k, row) in r.rows().enumerate() {
        if k & 1023 == 1023 {
            check(cancel)?;
            tick(progress, 1024);
        }
        if !row.iter().any(has_value) { continue; }
        for _ in 0..lead { csv.field("").map_err(io_err)?; }
        for v in row {
            cell.clear();
            cell_text(v, comma, &mut d1904, &mut cell);
            csv.field(&cell).map_err(io_err)?;
        }
        csv.end().map_err(io_err)?;
        rows += 1;
    }
    csv.w.flush().map_err(io_err)?;
    Ok((rows, width))
}

/// One sheet of the workbook at `path` as CSV into `out`; `sheet`: its name (None: the first
/// visible sheet holding a value).
pub fn to_csv<W: Write>(path: &Path, sheet: Option<&str>, delim: u8, out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let mut wb = open(path)?;
    let sheets = list(&mut wb)?;
    let pick = match sheet {
        Some(want) => sheets.iter().position(|s| s.name == want).ok_or_else(|| unreadable(format!("no sheet \"{want}\"")))?,
        None => sheets.iter().position(|s| !s.hidden && s.filled).unwrap_or(0),
    };
    let others = sheets.iter().enumerate().filter(|&(k, s)| k != pick && !s.hidden && s.filled).count();
    let name = sheets[pick].name.clone();
    let r = wb.worksheet_range(&name).map_err(unreadable)?;
    let (rows, cols) = rows_to_csv(&r, delim, out, progress, cancel)?;
    Ok(Imported { sheet: Some(name), others, rows, cols, ..Default::default() })
}
