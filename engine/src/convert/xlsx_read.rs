//! A workbook's sheet as CSV (xlsx_reader.py). The sheet is the one picked,
//! else the first visible one holding a value; it is streamed cell by cell:
//! values as text, numbers in their shortest form, cells whose style is a
//! date format as `yyyy-mm-dd[ hh:mm[:ss]]`, booleans as TRUE/FALSE, a
//! decimal comma when the CSV is `;`-delimited. Empty rows (formatting only)
//! are left out; rows are padded to the sheet's declared width.

use super::{check, date_text, io_err, number_text, tick, CsvOut, Imported, Sheet};
use calamine::{DataRef, Reader, SheetVisible, Xlsx, XlsxError};
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

// An archive announcing more than this once decompressed, or a big member that
// compresses better than XML ever does (5 to 30 times; deflate tops out near
// 1,000), is a zip bomb, not a workbook.
const MAX_TOTAL: u64 = 16 << 30;
const MAX_RATIO: u64 = 500;
const RATIO_FROM: u64 = 256 << 20;

type Book = Xlsx<BufReader<File>>;

fn unreadable(e: impl std::fmt::Display) -> String {
    format!("not a readable xlsx workbook ({e})")
}

fn open_file(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "file not found".into() } else { e.to_string() })
}

/// Refuses a zip bomb from the sizes its directory announces (they bound what is read).
fn guard(f: &File, max_total: u64, ratio_from: u64) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let m = zip.by_index_raw(i).map_err(|e| e.to_string())?;
        let (size, packed) = (m.size(), m.compressed_size().max(1));
        total += size;
        if size > ratio_from && size > MAX_RATIO * packed {
            return Err(format!("{} decompresses {} times over: a booby-trapped archive", m.name().map(|n| n.into_owned()).unwrap_or_default(), size / packed));
        }
    }
    if total > max_total {
        return Err(format!("{} GiB once decompressed: archive refused", total >> 30));
    }
    Ok(())
}

fn open(path: &Path) -> Result<Book, String> {
    let f = open_file(path)?;
    guard(&f, MAX_TOTAL, RATIO_FROM).map_err(unreadable)?;
    Xlsx::new(BufReader::with_capacity(1 << 16, f)).map_err(unreadable)
}

/// Whether a cell holds a value: a blank inline string does not.
fn has_value(v: &DataRef) -> bool {
    match v {
        DataRef::Empty => false,
        DataRef::String(s) => !s.trim().is_empty(),
        _ => true,
    }
}

/// (holds a value?, size declared by <dimension>). Stops at the first value: an
/// empty sheet (the usual Feuil2, Feuil3, cells only formatted) has none, a full
/// one has one at once.
fn probe(wb: &mut Book, name: &str) -> Result<(bool, Option<(u64, u64)>), String> {
    let mut r = match wb.worksheet_cells_reader(name) {
        Ok(r) => r,
        Err(XlsxError::NotAWorksheet(_)) => return Ok((false, None)),    // a chart sheet
        Err(e) => return Err(unreadable(e)),
    };
    let d = r.dimensions();
    let dim = (d.start != d.end).then(|| ((d.end.0 - d.start.0) as u64 + 1, (d.end.1 - d.start.1) as u64 + 1));
    loop {
        match r.next_cell() {
            Ok(Some(c)) if has_value(c.get_value()) => return Ok((true, dim)),
            Ok(Some(_)) | Err(XlsxError::CellError(_) | XlsxError::ParseFloat(_)) => {}
            Ok(None) => return Ok((false, dim)),
            Err(e) => return Err(unreadable(e)),
        }
    }
}

fn list(wb: &mut Book) -> Result<Vec<Sheet>, String> {
    let meta = wb.sheets_metadata().to_vec();
    if meta.is_empty() {
        return Err(unreadable("workbook without a sheet"));
    }
    meta.into_iter()
        .map(|m| {
            let (filled, dim) = probe(wb, &m.name)?;
            Ok(Sheet { hidden: m.visible != SheetVisible::Visible, name: m.name, filled, view: false, dim, approx: false })
        })
        .collect()
}

/// Every sheet with its state, hidden and empty ones included, in tab order.
pub fn sheets(path: &Path) -> Result<Vec<Sheet>, String> {
    list(&mut open(path)?)
}

/// One sheet of the workbook at `path` as CSV into `out`; `sheet`: its name
/// (None: the first visible sheet holding a value). Progress: rows read.
pub fn to_csv<W: Write>(path: &Path, sheet: Option<&str>, delim: u8, out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let mut wb = open(path)?;
    let sheets = list(&mut wb)?;
    let pick = match sheet {
        Some(want) => sheets.iter().position(|s| s.name == want).ok_or_else(|| unreadable(format!("no sheet \"{want}\"")))?,
        None => sheets.iter().position(|s| !s.hidden && s.filled).unwrap_or(0),
    };
    // The other sheets holding data, for the note: empty and hidden ones do not count.
    let others = sheets.iter().enumerate().filter(|&(k, s)| k != pick && !s.hidden && s.filled).count();
    let name = sheets[pick].name.clone();
    let d1904 = wb.has_1904_epoch();
    let comma = delim == b';';
    let mut r = wb.worksheet_cells_reader(&name).map_err(unreadable)?;
    let d = r.dimensions();
    // No usable <dimension>: the first row with a value (the titles) sets the width.
    let mut width = if d.start != d.end { d.end.1 as usize + 1 } else { 0 };
    let mut csv = CsvOut::new(out, delim, true);
    // One row's cells, their Strings reused from row to row.
    let mut cells: Vec<String> = Vec::new();
    let mut len = 0usize;
    let mut cur: Option<u32> = None;
    let (mut rows, mut maxw, mut seen) = (0u64, 0usize, 0usize);

    let mut flush = |cells: &mut Vec<String>, len: usize, width: &mut usize| -> Result<(), String> {
        if !cells[..len].iter().any(|c| !c.is_empty()) {
            return Ok(()); // a row of formatting only has no place in the CSV
        }
        if *width == 0 { *width = len; }
        let n = len.max(*width);
        for k in 0..n {
            csv.field(cells.get(k).filter(|_| k < len).map_or("", |s| s.as_str())).map_err(io_err)?;
        }
        csv.end().map_err(io_err)?;
        rows += 1;
        maxw = maxw.max(n);
        Ok(())
    };

    loop {
        let c = match r.next_cell() {
            Ok(Some(c)) => c,
            Ok(None) => break,
            // A cell calamine cannot type (#SPILL!, #CALC!, a malformed number): left empty.
            Err(XlsxError::CellError(_) | XlsxError::ParseFloat(_)) => continue,
            Err(e) => return Err(unreadable(e)),
        };
        let (row, col) = c.get_position();
        if cur != Some(row) {
            if cur.is_some() {
                flush(&mut cells, len, &mut width)?;
                seen += 1;
                if seen & 1023 == 0 {
                    check(cancel)?;
                    tick(progress, 1024);
                }
            }
            cur = Some(row);
            len = 0;
        }
        // A cell lands at its column; one out of order (never written by Excel) after the last.
        let at = (col as usize).max(len);
        while cells.len() <= at { cells.push(String::new()); }
        for s in &mut cells[len..=at] { s.clear(); }
        len = at + 1;
        let s = &mut cells[at];
        match c.get_value() {
            DataRef::Empty => {}
            DataRef::String(v) | DataRef::DateTimeIso(v) | DataRef::DurationIso(v) => s.push_str(v),
            DataRef::SharedString(v) => s.push_str(v),
            DataRef::Float(x) => number_text(*x, comma, s),
            DataRef::Int(i) => { use std::fmt::Write; let _ = write!(s, "{i}"); }
            DataRef::Bool(b) => s.push_str(if *b { "TRUE" } else { "FALSE" }),
            DataRef::DateTime(dt) => {
                if !date_text(dt.as_f64(), d1904, s) {
                    // Out of range: the number as stored.
                    number_text(dt.as_f64(), false, s);
                }
            }
            DataRef::Error(e) => { use std::fmt::Write; let _ = write!(s, "{e}"); }
        }
    }
    if cur.is_some() {
        flush(&mut cells, len, &mut width)?;
        seen += 1;
    }
    tick(progress, seen % 1024);
    csv.w.flush().map_err(io_err)?;
    Ok(Imported { sheet: Some(name), others, rows, cols: width.max(maxw), ..Default::default() })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("csvfab-conv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn esc(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    /// A workbook made by hand, as tests/support.make_workbook does: sheets
    /// (name, state, sheetData XML), shared strings, styles 1 = date (14), 2 = date-time (22).
    pub(crate) fn make_workbook(path: &Path, sheets: &[(&str, &str, &str)], shared: &[&str], date1904: bool) {
        let f = File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, body: String| {
            z.start_file(name, o).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        };
        let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#);
        for i in 1..=sheets.len() {
            ct += &format!(r#"<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#);
        }
        ct += "</Types>";
        put("[Content_Types].xml", ct);
        put("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.into());
        let mut wb = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">"#);
        if date1904 { wb += r#"<workbookPr date1904="1"/>"#; }
        wb += "<sheets>";
        let mut rels = String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        for (i, (name, state, data)) in sheets.iter().enumerate() {
            let k = i + 1;
            let st = if state.is_empty() { String::new() } else { format!(r#" state="{state}""#) };
            wb += &format!(r#"<sheet name="{}" sheetId="{k}"{st} r:id="rId{k}"/>"#, esc(name));
            rels += &format!(r#"<Relationship Id="rId{k}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{k}.xml"/>"#);
            put(&format!("xl/worksheets/sheet{k}.xml"), format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#));
        }
        wb += "</sheets></workbook>";
        let n = sheets.len();
        rels += &format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#, n + 1);
        if !shared.is_empty() {
            rels += &format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>"#, n + 2);
            let si: String = shared.iter().map(|s| format!("<si><t xml:space=\"preserve\">{s}</t></si>")).collect();
            put("xl/sharedStrings.xml", format!(r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{0}" uniqueCount="{0}">{si}</sst>"#, shared.len()));
        }
        rels += "</Relationships>";
        put("xl/workbook.xml", wb);
        put("xl/_rels/workbook.xml.rels", rels);
        put("xl/styles.xml", r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/><xf numFmtId="22" applyNumberFormat="1"/></cellXfs></styleSheet>"#.into());
        z.finish().unwrap();
    }

    const SHARED: &[&str] = &["Nom", "Ville", "Dupont", "Lyon", "Ana &amp; Bo", "Lefèvre"];
    const DATA: &str = concat!(
        r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>Né</t></is></c>"#,
        r#"<c r="D1" t="inlineStr"><is><t>Montant</t></is></c><c r="E1" t="inlineStr"><is><t>OK</t></is></c>"#,
        r#"<c r="F1" t="inlineStr"><is><t>Vu</t></is></c></row>"#,
        r#"<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" t="s"><v>3</v></c><c r="C2" s="1"><v>45092</v></c>"#,
        r#"<c r="D2"><v>1234.5</v></c><c r="E2" t="b"><v>1</v></c><c r="F2" s="2"><v>45092.4375</v></c></row>"#,
        r#"<row r="3"><c r="A3" s="1"/></row>"#,
        r#"<row r="5"><c r="A5" t="s"><v>4</v></c><c r="D5"><v>0.1</v></c><c r="E5" t="b"><v>0</v></c></row>"#,
        r#"<row r="6"><c r="B6" t="s"><v>5</v></c><c r="D6"><v>3</v></c><c r="F6" t="str"><v>formule</v></c></row>"#
    );
    const EXPECTED: &str = "Nom;Ville;Né;Montant;OK;Vu\nDupont;Lyon;2023-06-15;1234,5;TRUE;2023-06-15 10:30\nAna & Bo;;;0,1;FALSE;\n;Lefèvre;;3;;formule\n";

    fn book(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("book.xlsx");
        make_workbook(&p, &[
            ("Vide", "", r#"<row r="1"><c r="A1" s="1"/></row>"#),
            ("Données", "", DATA),
            ("Cachée", "hidden", r#"<row r="1"><c r="A1" t="inlineStr"><is><t>x</t></is></c></row>"#),
            ("Autre", "", r#"<row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c></row><row r="2"><c r="A2"><v>2</v></c></row>"#),
        ], SHARED, false);
        p
    }

    fn convert(p: &Path, sheet: Option<&str>, delim: u8) -> Result<(String, Imported), String> {
        let mut out = Vec::new();
        let info = to_csv(p, sheet, delim, &mut out, None, &AtomicBool::new(false))?;
        Ok((String::from_utf8(out).unwrap(), info))
    }

    #[test]
    fn sheet_list() {
        let dir = tmpdir("xl-list");
        let s = sheets(&book(&dir)).unwrap();
        let got: Vec<_> = s.iter().map(|s| (s.name.as_str(), s.hidden, s.filled)).collect();
        assert_eq!(got, [("Vide", false, false), ("Données", false, true), ("Cachée", true, true), ("Autre", false, true)]);
        assert_eq!(super::super::offered(&s).len(), 2);
        let bad = dir.join("notzip.xlsx");
        std::fs::write(&bad, "a;b\n").unwrap();
        assert!(sheets(&bad).unwrap_err().contains("not a readable xlsx workbook"));
        assert_eq!(sheets(&dir.join("none.xlsx")).unwrap_err(), "file not found");
    }

    #[test]
    fn default_sheet_is_the_first_visible_with_data() {
        let dir = tmpdir("xl-default");
        let (text, info) = convert(&book(&dir), None, b';').unwrap();
        assert_eq!(text, EXPECTED);
        assert_eq!(info, Imported { sheet: Some("Données".into()), others: 1, rows: 4, cols: 6, ..Default::default() });
        let single = dir.join("single.xlsx");
        make_workbook(&single, &[("Feuil1", "", DATA), ("Feuil2", "", "")], SHARED, false);
        let s = sheets(&single).unwrap();
        assert_eq!(super::super::offered(&s).len(), 1);
        let (text, info) = convert(&single, None, b';').unwrap();
        assert_eq!((text.as_str(), info.sheet.as_deref(), info.others), (EXPECTED, Some("Feuil1"), 0));
    }

    #[test]
    fn named_sheet_and_comma_delimiter() {
        let dir = tmpdir("xl-named");
        let p = book(&dir);
        assert_eq!(convert(&p, Some("Autre"), b',').unwrap().0, "a\n2\n");
        assert!(convert(&p, Some("Données"), b',').unwrap().0.contains("Dupont,Lyon,2023-06-15,1234.5,TRUE"));
        assert!(convert(&p, Some("Nope"), b';').unwrap_err().contains("no sheet \"Nope\""));
    }

    #[test]
    fn written_beside_with_backup() {
        let dir = tmpdir("xl-beside");
        let p = book(&dir);
        let dest = super::super::csv_path_for(&p, Some("Données"));
        assert_eq!(dest.file_name().unwrap(), "book - Données.csv");
        std::fs::write(&dest, "old\n").unwrap();
        let info = super::super::import(super::super::Format::Xlsx, &p, Some("Données"), b';', &dest, true, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), EXPECTED);
        assert_eq!(info.size, std::fs::metadata(&dest).unwrap().len());
        let bak = info.backup.unwrap();
        assert!(bak.to_string_lossy().ends_with(".bak"));
        assert_eq!(std::fs::read_to_string(bak).unwrap(), "old\n");
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
        // A folder that does not exist: refused, nothing written.
        let err = super::super::import(super::super::Format::Xlsx, &p, None, b';', &dir.join("nodir/x.csv"), false, None, &AtomicBool::new(false)).unwrap_err();
        assert!(err.starts_with("no such folder"), "{err}");
    }

    #[test]
    fn date1904() {
        let dir = tmpdir("xl-1904");
        let p = dir.join("mac.xlsx");
        make_workbook(&p, &[("S", "", r#"<row r="1"><c r="A1" s="1"><v>0</v></c></row>"#)], &[], true);
        assert_eq!(convert(&p, None, b';').unwrap().0, "1904-01-01\n");
    }

    #[test]
    fn errors_and_odd_cells() {
        let dir = tmpdir("xl-odd");
        let p = dir.join("odd.xlsx");
        make_workbook(&p, &[("S", "", concat!(
            r#"<row r="1"><c r="A1" t="e"><v>#DIV/0!</v></c><c r="B1" t="e"><v>#SPILL!</v></c><c r="C1"><v>1e20</v></c><c r="D1" s="1"><v>1e12</v></c></row>"#,
            r#"<row r="2"><c r="A2" t="inlineStr"><is><t>x</t></is></c><c r="B2"><v>-0.5</v></c></row>"#))], &[], false);
        assert_eq!(convert(&p, None, b',').unwrap().0, "#DIV/0!,,1e+20,1000000000000\nx,-0.5,,\n");
    }

    #[test]
    fn zip_bomb_is_refused() {
        let dir = tmpdir("xl-bomb");
        let p = dir.join("bomb.xlsx");
        let mut z = zip::ZipWriter::new(File::create(&p).unwrap());
        z.start_file("xl/worksheets/sheet1.xml", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(&vec![0u8; 4 << 20]).unwrap();
        z.finish().unwrap();
        let f = File::open(&p).unwrap();
        assert!(guard(&f, MAX_TOTAL, 1 << 20).unwrap_err().contains("booby-trapped"));
        assert!(guard(&f, 1 << 20, RATIO_FROM).unwrap_err().contains("archive refused"));
        assert!(guard(&f, MAX_TOTAL, RATIO_FROM).is_ok());
    }

    #[test]
    fn cancelled() {
        let dir = tmpdir("xl-cancel");
        let p = dir.join("big.xlsx");
        let data: String = (1..=3000).map(|r| format!(r#"<row r="{r}"><c r="A{r}"><v>{r}</v></c></row>"#)).collect();
        make_workbook(&p, &[("S", "", &data)], &[], false);
        let mut out = Vec::new();
        let err = to_csv(&p, None, b';', &mut out, None, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(err, super::super::CANCELLED);
        let n = AtomicUsize::new(0);
        to_csv(&p, None, b';', &mut out, Some(&n), &AtomicBool::new(false)).unwrap();
        assert_eq!(n.into_inner(), 3000);
    }
}
