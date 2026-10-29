//! A table as a formatted workbook (xlsx_writer.py): one sheet, Aptos
//! everywhere, a bold white-on-navy header row, frozen and filtered; columns
//! typed from their values (columns.rs) — numbers, percents and dates written
//! as such with their format, the rest as text; widths from the content.
//!
//! Two passes: the columns are typed over every row first (in parallel), then
//! the rows are written in order through rust_xlsxwriter's constant-memory
//! mode, which streams the sheet to a temporary file beside the destination.

use super::columns::{ColKind, ColumnStat, Converter};
use super::{check, check_dir, replace, temp_beside, tick, visit_row, Exported, Rows};
use crate::table::{RowId, State};
use rayon::prelude::*;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, FormatPattern, Workbook};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

// Excel's hard limits.
pub const MAX_ROWS: usize = 1_048_576;
pub const MAX_COLS: usize = 16_384;
const MAX_CHARS: usize = 32_767;

const HEADER_FILL: u32 = 0x1E3A8A;
const HEADER_LINE: u32 = 0x3B82F6;

/// 31 characters at most, none of `[]:*?/\` — or Excel refuses to open the file.
fn sheet_name(base: &str) -> String {
    let s: String = base.chars().map(|c| if "[]:*?/\\".contains(c) { ' ' } else { c }).collect();
    // A leading or trailing apostrophe is refused too (rust_xlsxwriter enforces it).
    let s = s.trim().trim_matches('\'').trim();
    if s.is_empty() { "Sheet1".into() } else { s.chars().take(31).collect() }
}

/// Without the control characters XML 1.0 cannot hold (tab and line breaks aside),
/// and within Excel's length limit; `cut`: whether it had to be shortened.
fn clean<'a>(v: &'a str, buf: &'a mut String, cut: &mut bool) -> &'a str {
    let bad = v.bytes().any(|b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r'));
    let long = v.len() > MAX_CHARS && v.chars().count() > MAX_CHARS;
    if !bad && !long {
        return v;
    }
    buf.clear();
    buf.extend(v.chars().filter(|&c| c >= ' ' || matches!(c, '\t' | '\n' | '\r')).take(MAX_CHARS));
    *cut = long;
    buf
}

fn err(e: rust_xlsxwriter::XlsxError) -> String {
    e.to_string()
}

/// Types every column over `n` data rows, in parallel; the header widens its column
/// for the bold font and the filter button.
fn stats(st: &State, rows: &Rows, n: usize, ncols: usize, header: Option<&[String]>, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Vec<ColumnStat>, String> {
    const CHUNK: usize = 8192;
    let parts: Vec<Vec<ColumnStat>> = (0..n.div_ceil(CHUNK))
        .into_par_iter()
        .map(|k| {
            let mut s = vec![ColumnStat::default(); ncols];
            if check(cancel).is_err() { return s; }
            let (mut tmp, mut buf) = (Vec::new(), Vec::new());
            let end = ((k + 1) * CHUNK).min(n);
            for pos in k * CHUNK..end {
                visit_row(st, rows.id(pos), &mut tmp, &mut buf, |c, v| {
                    if let Some(x) = s.get_mut(c) { x.see(v); }
                });
            }
            tick(progress, end - k * CHUNK);
            s
        })
        .collect();
    check(cancel)?;
    let mut out = vec![ColumnStat::default(); ncols];
    for p in &parts {
        for (o, s) in out.iter_mut().zip(p) { o.merge(s); }
    }
    if let Some(h) = header {
        for (o, v) in out.iter_mut().zip(h) {
            o.width = o.width.max((v.chars().count() as f64 * 1.15) as usize + 3);
        }
    }
    Ok(out)
}

/// Writes the table to `dest` as a workbook, atomically. See [`super::export`].
pub fn write(st: &State, rows: Option<&[RowId]>, dest: &Path, sheet: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    check_dir(dest)?;
    let rows = Rows { st, ids: rows };
    let header = st.header.then(|| st.headers());
    let ncols = st.ncols.min(MAX_COLS);
    let room = MAX_ROWS - header.is_some() as usize;
    let n = rows.len().min(room);
    let written = (n + header.is_some() as usize) as u64;
    let mut stats = stats(st, &rows, n, ncols, header.as_deref(), progress, cancel)?;
    let kinds: Vec<(ColKind, Option<String>)> = stats.iter_mut().map(|s| s.decide()).collect();

    let mut wb = Workbook::new();
    let base = Format::new().set_font_name("Aptos").set_font_size(11).set_font_family(2);
    wb.set_default_format(&base, 20, 64).map_err(err)?;
    // Level 1: the sheet of a 130 MB CSV is 780 MB of XML; waiting matters more than bytes.
    wb.set_zip_compression_level(1);
    // The sheet's XML is streamed to a file beside the destination, not to a /tmp that may be RAM.
    if let Some(dir) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        wb.set_tempdir(dir).map_err(err)?;
    }
    let ws = wb.add_worksheet_with_constant_memory().map_err(err)?;
    if ws.set_name(sheet_name(sheet)).is_err() {
        ws.set_name("Sheet1").map_err(err)?; // a name Excel reserves, as "History"
    }
    let fmts: Vec<Option<Format>> = kinds.iter().map(|(_, f)| f.as_ref().map(|f| base.clone().set_num_format(f))).collect();
    for (c, s) in stats.iter().enumerate() {
        // rust_xlsxwriter writes pixels / 7 as the width: these are the bridge's widths, exactly.
        ws.set_column_width_pixels(c as u16, 7 * s.width.clamp(6, 58) as u32 + 14).map_err(err)?;
    }
    let mut truncated_cells = 0u64;
    let mut cbuf = String::new();
    if let Some(h) = &header {
        let hf = base.clone().set_bold().set_font_color(Color::White).set_pattern(FormatPattern::Solid)
            .set_background_color(Color::RGB(HEADER_FILL)).set_border_bottom(FormatBorder::Medium)
            .set_border_bottom_color(Color::RGB(HEADER_LINE)).set_align(FormatAlign::VerticalCenter);
        ws.set_row_height(0, 22).map_err(err)?;
        ws.set_freeze_panes(1, 0).map_err(err)?;
        if ncols > 0 {
            ws.autofilter(0, 0, (written - 1) as u32, (ncols - 1) as u16).map_err(err)?;
        }
        for (c, v) in h.iter().take(ncols).enumerate() {
            let mut cut = false;
            ws.write_string_with_format(0, c as u16, clean(v, &mut cbuf, &mut cut), &hf).map_err(err)?;
            truncated_cells += cut as u64;
        }
    }
    let conv = Converter::new();
    let first = header.is_some() as u32;
    let (mut tmp, mut buf) = (Vec::new(), Vec::new());
    let mut failed: Option<String> = None;
    for pos in 0..n {
        if pos & 4095 == 4095 {
            check(cancel)?;
            tick(progress, 4096);
        }
        let r = first + pos as u32;
        visit_row(st, rows.id(pos), &mut tmp, &mut buf, |c, v| {
            if c >= ncols || v.is_empty() || failed.is_some() { return; }
            let res = match conv.value(kinds[c].0, v) {
                Some(x) => match &fmts[c] {
                    Some(f) => ws.write_number_with_format(r, c as u16, x, f).map(|_| ()),
                    None => ws.write_number(r, c as u16, x).map(|_| ()),
                },
                None => {
                    let mut cut = false;
                    let s = clean(v, &mut cbuf, &mut cut);
                    truncated_cells += cut as u64;
                    ws.write_string(r, c as u16, s).map(|_| ())
                }
            };
            if let Err(e) = res { failed = Some(err(e)); }
        });
        if let Some(e) = failed.take() { return Err(e); }
    }
    tick(progress, n % 4096);
    check(cancel)?;
    let tmp_path = temp_beside(dest);
    let res = wb.save(&tmp_path).map_err(err).and_then(|_| {
        std::fs::File::open(&tmp_path).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        replace(&tmp_path, dest, false).map_err(|e| e.to_string())
    });
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(Exported {
        rows: written,
        cols: if written > 0 { ncols } else { 0 },
        truncated_rows: rows.len() > room,
        truncated_cols: st.ncols > MAX_COLS,
        truncated_cells,
        types: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::io::Read;
    use std::sync::Arc;

    fn table(csv: &str, header: bool) -> Table {
        Table::new(Arc::new(Base::from_bytes(csv.as_bytes().to_vec(), OpenOptions::default())), Some(header))
    }

    fn part(path: &Path, name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    /// The cell `r` of the sheet XML: (s attribute, t attribute, value).
    fn cell(xml: &str, r: &str) -> (Option<String>, Option<String>, String) {
        let at = xml.find(&format!("<c r=\"{r}\"")).unwrap_or_else(|| panic!("no {r}"));
        let end = at + xml[at..].find("</c>").unwrap();
        let c = &xml[at..end];
        let attr = |a: &str| c.find(&format!(" {a}=\"")).map(|k| { let v = &c[k + a.len() + 3..]; v[..v.find('"').unwrap()].to_string() });
        let val = if let Some(k) = c.find("<v>") { c[k + 3..].split('<').next().unwrap().to_string() } else {
            let k = c.find("<t").unwrap();
            let v = &c[k..];
            v[v.find('>').unwrap() + 1..].split('<').next().unwrap().to_string()
        };
        (attr("s"), attr("t"), val)
    }

    /// The number format of style index `s`.
    fn fmt_of(styles: &str, s: &str) -> String {
        let xfs = &styles[styles.find("<cellXfs").unwrap()..];
        let xf = xfs.split("<xf ").nth(s.parse::<usize>().unwrap() + 1).unwrap();
        let id = xf.split("numFmtId=\"").nth(1).unwrap().split('"').next().unwrap();
        match styles.split(&format!("numFmtId=\"{id}\" formatCode=\"")).nth(1) {
            Some(rest) => rest.split('"').next().unwrap().to_string(),
            None => id.to_string(),
        }
    }

    const CSV: &str = "Code;Montant;Date;Pct;Nom;Quand\n\
        01000;1 234,50;15/06/2023;12%;Dupont;2023-06-15T10:30:00Z\n\
        02000;-5,25;01/01/2024;7,5%;Lefèvre;2024-01-01\n\
        03000;7;31/12/2024;100%;\"A;B\";2024-02-29 08:00\n";

    fn export(t: &Table, dir: &Path, name: &str, sheet: &str) -> (std::path::PathBuf, Exported) {
        let p = dir.join(name);
        let info = write(&t.st, None, &p, sheet, None, &AtomicBool::new(false)).unwrap();
        (p, info)
    }

    #[test]
    fn column_typing_and_header_look() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-typing");
        let (p, info) = export(&table(CSV, true), &dir, "out.xlsx", "Sheet1");
        assert_eq!(info, Exported { rows: 4, cols: 6, ..Default::default() });
        let xml = part(&p, "xl/worksheets/sheet1.xml");
        let styles = part(&p, "xl/styles.xml");
        assert_eq!(cell(&xml, "A2"), (None, Some("inlineStr".into()), "01000".into()));
        assert_eq!(["B2", "B3", "B4"].map(|r| cell(&xml, r).2), ["1234.5", "-5.25", "7"]);
        assert_eq!(fmt_of(&styles, &cell(&xml, "B2").0.unwrap()), "#,##0.00");
        assert_eq!(["C2", "C3", "C4"].map(|r| cell(&xml, r).2), ["45092", "45292", "45657"]);
        assert_eq!(fmt_of(&styles, &cell(&xml, "C2").0.unwrap()), "dd/mm/yyyy");
        assert_eq!(["D2", "D3", "D4"].map(|r| cell(&xml, r).2), ["0.12", "0.075", "1"]);
        assert_eq!(fmt_of(&styles, &cell(&xml, "D2").0.unwrap()), "0.0%");
        assert_eq!(cell(&xml, "E4"), (None, Some("inlineStr".into()), "A;B".into()));
        assert_eq!(cell(&xml, "F2").2, "45092.4375");
        assert_eq!(fmt_of(&styles, &cell(&xml, "F2").0.unwrap()), "yyyy-mm-dd hh:mm");
        // The header: styled, frozen, filtered.
        assert_eq!(cell(&xml, "A1").2, "Code");
        assert!(cell(&xml, "A1").0.is_some());
        assert!(xml.contains(r#"<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>"#), "{xml}");
        assert!(xml.contains(r#"<autoFilter ref="A1:F4"/>"#));
        // 22 pt, which rust_xlsxwriter rounds to whole pixels.
        assert!(xml.contains(r#"<row r="1" ht="21.75" customHeight="1">"#));
        let wbx = part(&p, "xl/workbook.xml");
        assert!(wbx.contains(r#"<definedName name="_xlnm._FilterDatabase" localSheetId="0" hidden="1">Sheet1!$A$1:$F$4</definedName>"#), "{wbx}");
        assert!(styles.contains(r#"rgb="FF1E3A8A""#) && styles.contains(r#"<name val="Aptos"/>"#));
        // Widths as the bridge set them: "Code" header → int(4 × 1.15) + 3 = 7, + 2, at least 8.
        assert!(xml.contains(r#"<col min="1" max="1" width="9" customWidth="1"/>"#), "{xml}");
    }

    #[test]
    fn without_header() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-nohead");
        let (p, info) = export(&table("1;a\n2;b\n", false), &dir, "t.xlsx", "T");
        assert_eq!(info.rows, 2);
        let xml = part(&p, "xl/worksheets/sheet1.xml");
        assert!(!xml.contains("<pane") && !xml.contains("<autoFilter"));
        assert_eq!(cell(&xml, "A1"), (None, None, "1".into()));
    }

    #[test]
    fn long_integers_stay_text() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-long");
        let (p, _) = export(&table("Id;N\n1234567890123456;1\n2234567890123456;2\n", true), &dir, "t.xlsx", "T");
        let xml = part(&p, "xl/worksheets/sheet1.xml");
        assert_eq!(cell(&xml, "A2"), (None, Some("inlineStr".into()), "1234567890123456".into()));
        assert_eq!(cell(&xml, "B2"), (None, None, "1".into()));
    }

    #[test]
    fn english_numbers() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-en");
        let (p, _) = export(&table("Amount,Who\n\"1,234.50\",a\n2.5,b\n", true), &dir, "t.xlsx", "T");
        let xml = part(&p, "xl/worksheets/sheet1.xml");
        assert_eq!((cell(&xml, "A2").2, cell(&xml, "A3").2), ("1234.5".into(), "2.5".into()));
    }

    fn back(p: &Path, delim: u8) -> (String, super::super::Imported) {
        let mut out = Vec::new();
        let info = super::super::xlsx_read::to_csv(p, None, delim, &mut out, None, &AtomicBool::new(false)).unwrap();
        (String::from_utf8(out).unwrap(), info)
    }

    #[test]
    fn round_trip() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-rt");
        let (p, _) = export(&table(CSV, true), &dir, "rt.xlsx", "rt");
        let (text, info) = back(&p, b';');
        assert_eq!(text, "Code;Montant;Date;Pct;Nom;Quand\n\
            01000;1234,5;2023-06-15;0,12;Dupont;2023-06-15 10:30\n\
            02000;-5,25;2024-01-01;0,075;Lefèvre;2024-01-01\n\
            03000;7;2024-12-31;1;\"A;B\";2024-02-29 08:00\n");
        assert_eq!(info, super::super::Imported { sheet: Some("rt".into()), others: 0, rows: 4, cols: 6, ..Default::default() });
    }

    #[test]
    fn text_survives() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-text");
        let mut t = table("Nom;Note\nx;y\nz;w\nq;r\n", true);
        let ids: Vec<_> = (0..3).map(|k| t.st.data_id(k)).collect();
        t.set_cells(&[(t.st.all_id(0), 1, "Note <b>&".into()), (ids[0], 0, "a\tb".into()), (ids[0], 1, "x\u{1}y".into()),
                      (ids[1], 0, "ligne\nsuivante".into()), (ids[1], 1, "é ü 漢字 🎉".into()), (ids[2], 0, String::new()), (ids[2], 1, "seule".into())]);
        let (p, info) = export(&t, &dir, "t.xlsx", "Feuille");
        assert_eq!(info.rows, 4);
        let (text, _) = back(&p, b';');
        assert_eq!(text, "Nom;Note <b>&\na\tb;xy\n\"ligne\nsuivante\";é ü 漢字 🎉\n;seule\n");
    }

    #[test]
    fn sheet_names() {
        assert_eq!(sheet_name("a/b:c*?[x]"), "a b c   x");
        assert_eq!(sheet_name(&format!("a/b:c*?[x]{}", "z".repeat(40))).chars().count(), 31);
        assert_eq!(sheet_name(" / "), "Sheet1");
        let dir = super::super::xlsx_read::tests::tmpdir("xw-names");
        let (p, _) = export(&table("a\n1\n", true), &dir, "t.xlsx", "l'été");
        assert_eq!(back(&p, b';').1.sheet.as_deref(), Some("l'été"));
        assert!(part(&p, "xl/workbook.xml").contains("'l''été'!$A$1:$A$2"));
        let (p, _) = export(&table("a\n1\n", true), &dir, "u.xlsx", "History");
        assert!(back(&p, b';').1.sheet.is_some());
    }

    #[test]
    fn wide_and_empty() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-wide");
        let row: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        let csv = format!("{0}\n{0}\n", row.iter().map(|s| format!("h{s}")).collect::<Vec<_>>().join(";"));
        let (p, _) = export(&table(&csv, true), &dir, "w.xlsx", "W");
        assert!(part(&p, "xl/worksheets/sheet1.xml").contains(r#"<autoFilter ref="A1:AD2"/>"#));
        assert_eq!(back(&p, b';').1.cols, 30);
        let (p, info) = export(&table("", false), &dir, "e.xlsx", "E");
        assert_eq!((info.rows, back(&p, b';').1.rows), (0, 0));
    }

    #[test]
    fn selected_rows_only() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-rows");
        let t = table("n\n1\n2\n3\n", true);
        let ids = [t.st.data_id(2), t.st.data_id(0)];
        let p = dir.join("s.xlsx");
        let info = write(&t.st, Some(&ids), &p, "S", None, &AtomicBool::new(false)).unwrap();
        assert_eq!(info.rows, 3);
        assert_eq!(back(&p, b';').0, "n\n3\n1\n");
    }
}
