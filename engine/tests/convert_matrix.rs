//! Every way in, every way out: one small table (Unicode, empty cells, numbers, dates) read
//! from each input format and written to each output format, then read back. The workbooks no
//! writer here makes (`.xls`, `.xlsb`, `.ods`) are built by hand, byte by byte.

use csvfab_engine::base::{Base, OpenOptions};
use csvfab_engine::convert::text_write::{self, TextFormat};
use csvfab_engine::convert::{self, packed, Format};
use csvfab_engine::enc::Enc;
use csvfab_engine::save::{self, SaveOptions};
use csvfab_engine::table::Table;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const TITLES: [&str; 5] = ["name", "city", "n", "date", "note"];
const ROWS: [[&str; 5]; 3] = [
    ["Zoé", "東京", "12", "2024-03-15", "a,b"],
    ["Иван", "", "7.5", "2023-12-31", "日本語 😀"],
    ["", "القاهرة", "", "", "x\"y"],
];

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("csvfab-matrix-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn csv_text(d: char) -> String {
    let q = |v: &str| if v.contains([d, '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() };
    let mut s = TITLES.join(&d.to_string());
    s.push('\n');
    for r in ROWS {
        s += &r.map(q).join(&d.to_string());
        s.push('\n');
    }
    s
}

fn table_of(bytes: Vec<u8>, delim: u8) -> Table {
    Table::new(Arc::new(Base::from_bytes(bytes, OpenOptions { enc: Some(Enc::Utf8), delim: Some(delim) })), None)
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// The rows of a table read back, by column title, a number written either way the same.
fn check(t: &Table, ctx: &str) {
    let heads = t.st.headers();
    assert!(t.st.header, "{ctx}: titles");
    let at: HashMap<&str, usize> = heads.iter().enumerate().map(|(i, h)| (h.as_str(), i)).collect();
    assert_eq!(t.st.len(), ROWS.len(), "{ctx}: rows in {heads:?}");
    for (p, r) in ROWS.iter().enumerate() {
        let id = t.st.data_id(p);
        for (c, want) in r.iter().enumerate() {
            let col = *at.get(TITLES[c]).unwrap_or_else(|| panic!("{ctx}: no column {} in {heads:?}", TITLES[c]));
            let got = t.st.cell(id, col);
            let same = got == *want || (TITLES[c] == "n" && got.parse::<f64>().ok() == want.parse::<f64>().ok());
            assert!(same, "{ctx}: row {p} {}: {got:?}, not {want:?}", TITLES[c]);
        }
    }
}

// --- the inputs ---------------------------------------------------------------

/// A BIFF8 record.
fn rec(o: &mut Vec<u8>, id: u16, body: &[u8]) {
    o.extend_from_slice(&id.to_le_bytes());
    o.extend_from_slice(&(body.len() as u16).to_le_bytes());
    o.extend_from_slice(body);
}

fn xls_label(o: &mut Vec<u8>, row: u16, col: u16, s: &str) {
    let u: Vec<u16> = s.encode_utf16().collect();
    let mut b = Vec::new();
    for x in [row, col, 0, u.len() as u16] { b.extend_from_slice(&x.to_le_bytes()); }
    b.push(1);
    for x in u { b.extend_from_slice(&x.to_le_bytes()); }
    rec(o, 0x0204, &b);
}

fn xls_number(o: &mut Vec<u8>, row: u16, col: u16, xf: u16, v: f64) {
    let mut b = Vec::new();
    for x in [row, col, xf] { b.extend_from_slice(&x.to_le_bytes()); }
    b.extend_from_slice(&v.to_le_bytes());
    rec(o, 0x0203, &b);
}

/// A `.xls`: a compound file of one stream, its sectors in a row.
fn make_xls(path: &Path) {
    let bof = |kind: u16| { let mut b = 0x0600u16.to_le_bytes().to_vec(); b.extend_from_slice(&kind.to_le_bytes()); b.resize(16, 0); b };
    let name: Vec<u16> = "Données".encode_utf16().collect();
    let sheet_record_len = 4 + 4 + 2 + 2 + name.len() * 2;
    let xf_len = 2 * (4 + 20);
    let globals_len = (4 + 16) + xf_len + sheet_record_len + 4;
    let mut wb = Vec::new();
    rec(&mut wb, 0x0809, &bof(5));
    // Two formats: General, and built-in 14 (a date).
    for fmt in [0u16, 14] {
        let mut x = vec![0u8; 20];
        x[2..4].copy_from_slice(&fmt.to_le_bytes());
        rec(&mut wb, 0x00E0, &x);
    }
    let mut sh = (globals_len as u32).to_le_bytes().to_vec();
    sh.extend_from_slice(&[0, 0, name.len() as u8, 1]);
    for x in &name { sh.extend_from_slice(&x.to_le_bytes()); }
    rec(&mut wb, 0x0085, &sh);
    rec(&mut wb, 0x000A, &[]);
    assert_eq!(wb.len(), globals_len);
    rec(&mut wb, 0x0809, &bof(0x10));
    for (c, t) in TITLES.iter().enumerate() { xls_label(&mut wb, 0, c as u16, t); }
    for (p, r) in ROWS.iter().enumerate() {
        let row = p as u16 + 1;
        for (c, v) in r.iter().enumerate() {
            if v.is_empty() { continue; }
            if c == 2 { xls_number(&mut wb, row, c as u16, 0, v.parse().unwrap()); }
            else if c == 3 { xls_number(&mut wb, row, c as u16, 1, if *v == "2024-03-15" { 45366.0 } else { 45291.0 }); }
            else { xls_label(&mut wb, row, c as u16, v); }
        }
    }
    rec(&mut wb, 0x000A, &[]);
    // Past the mini-stream's 4096 bytes, whole sectors.
    wb.resize(wb.len().max(4096).next_multiple_of(512), 0);
    let sectors = wb.len() / 512;
    let mut f = vec![0u8; 512];
    f[..8].copy_from_slice(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1");
    f[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    f[26..28].copy_from_slice(&3u16.to_le_bytes());
    f[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    f[30..32].copy_from_slice(&9u16.to_le_bytes());
    f[32..34].copy_from_slice(&6u16.to_le_bytes());
    f[44..48].copy_from_slice(&1u32.to_le_bytes());     // one FAT sector
    f[48..52].copy_from_slice(&1u32.to_le_bytes());     // the directory at sector 1
    f[56..60].copy_from_slice(&4096u32.to_le_bytes());
    f[60..64].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    f[68..72].copy_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    f[76..80].copy_from_slice(&0u32.to_le_bytes());     // the FAT at sector 0
    for k in 1..109 { f[76 + k * 4..80 + k * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); }
    // The FAT: itself, the directory, then the stream.
    let mut fat: Vec<u32> = vec![0xFFFF_FFFD, 0xFFFF_FFFE];
    for k in 0..sectors { fat.push(if k + 1 == sectors { 0xFFFF_FFFE } else { k as u32 + 3 }); }
    assert!(fat.len() <= 128);
    fat.resize(128, 0xFFFF_FFFF);
    for x in fat { f.extend_from_slice(&x.to_le_bytes()); }
    let entry = |name: &str, kind: u8, start: u32, size: u32| {
        let mut e = vec![0u8; 128];
        for (k, u) in name.encode_utf16().enumerate() { e[k * 2..k * 2 + 2].copy_from_slice(&u.to_le_bytes()); }
        e[64..66].copy_from_slice(&((name.len() as u16 + 1) * 2).to_le_bytes());
        e[66] = kind;
        e[116..120].copy_from_slice(&start.to_le_bytes());
        e[120..124].copy_from_slice(&size.to_le_bytes());
        e
    };
    f.extend_from_slice(&entry("Root Entry", 5, 0xFFFF_FFFE, 0));
    f.extend_from_slice(&entry("Workbook", 2, 2, wb.len() as u32));
    f.extend_from_slice(&[0u8; 256]);
    f.extend_from_slice(&wb);
    std::fs::write(path, f).unwrap();
}

/// A BIFF12 record of `.xlsb`.
fn brt(o: &mut Vec<u8>, id: u16, body: &[u8]) {
    let n = body.len();
    if id < 0x80 { o.push(id as u8); } else { o.push((id & 0x7F) as u8 | 0x80); o.push((id >> 7) as u8); }
    assert!(n < 1 << 14);
    if n < 0x80 { o.push(n as u8); } else { o.push((n & 0x7F) as u8 | 0x80); o.push((n >> 7) as u8); }
    o.extend_from_slice(body);
}

fn wide(s: &str, o: &mut Vec<u8>) {
    let u: Vec<u16> = s.encode_utf16().collect();
    o.extend_from_slice(&(u.len() as u32).to_le_bytes());
    for x in u { o.extend_from_slice(&x.to_le_bytes()); }
}

fn zip_of(path: &Path, parts: &[(&str, Vec<u8>)]) {
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    for (n, b) in parts {
        z.start_file(*n, o).unwrap();
        z.write_all(b).unwrap();
    }
    z.finish().unwrap();
}

fn make_xlsb(path: &Path) {
    let mut wb = Vec::new();
    let mut sh = 0u32.to_le_bytes().to_vec();
    sh.extend_from_slice(&1u32.to_le_bytes());
    wide("rId1", &mut sh);
    wide("Feuil1", &mut sh);
    brt(&mut wb, 0x009C, &sh);
    brt(&mut wb, 0x0090, &[]);
    brt(&mut wb, 0x0084, &[]);
    let mut s = Vec::new();
    let mut dim = Vec::new();
    for x in [0u32, ROWS.len() as u32, 0, 4] { dim.extend_from_slice(&x.to_le_bytes()); }
    brt(&mut s, 0x0094, &dim);
    brt(&mut s, 0x0091, &[]);
    let put_row = |s: &mut Vec<u8>, row: u32, cells: &[(usize, &str)], num: Option<usize>| {
        brt(s, 0x0000, &row.to_le_bytes());
        for &(c, v) in cells {
            let mut b = (c as u32).to_le_bytes().to_vec();
            b.extend_from_slice(&0u32.to_le_bytes());
            if Some(c) == num {
                b.extend_from_slice(&v.parse::<f64>().unwrap().to_le_bytes());
                brt(s, 0x0005, &b);
            } else {
                wide(v, &mut b);
                brt(s, 0x0006, &b);
            }
        }
    };
    put_row(&mut s, 0, &TITLES.iter().copied().enumerate().collect::<Vec<_>>(), None);
    for (p, r) in ROWS.iter().enumerate() {
        let cells: Vec<(usize, &str)> = r.iter().copied().enumerate().filter(|x| !x.1.is_empty()).collect();
        put_row(&mut s, p as u32 + 1, &cells, Some(2));
    }
    brt(&mut s, 0x0092, &[]);
    let rels = br#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/></Relationships>"#.to_vec();
    zip_of(path, &[("xl/_rels/workbook.bin.rels", rels), ("xl/workbook.bin", wb), ("xl/worksheets/sheet1.bin", s)]);
}

fn make_ods(path: &Path) {
    let esc = |v: &str| v.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
    let text = |v: &str| format!(r#"<table:table-cell office:value-type="string"><text:p>{}</text:p></table:table-cell>"#, esc(v));
    let empty = "<table:table-cell/>".to_string();
    let mut rows = format!("<table:table-row>{}</table:table-row>", TITLES.map(text).concat());
    for r in ROWS {
        let cells: String = r.iter().enumerate().map(|(c, v)| match (c, v.is_empty()) {
            (_, true) => empty.clone(),
            (2, _) => format!(r#"<table:table-cell office:value-type="float" office:value="{v}"><text:p>{v}</text:p></table:table-cell>"#),
            (3, _) => format!(r#"<table:table-cell office:value-type="date" office:date-value="{v}"><text:p>{v}</text:p></table:table-cell>"#),
            _ => text(v),
        }).collect();
        rows += &format!("<table:table-row>{cells}</table:table-row>");
    }
    let content = format!(r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"><office:body><office:spreadsheet><table:table table:name="Feuille1">{rows}</table:table></office:spreadsheet></office:body></office:document-content>"#);
    let manifest = br#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/></manifest:manifest>"#.to_vec();
    zip_of(path, &[("mimetype", b"application/vnd.oasis.opendocument.spreadsheet".to_vec()), ("META-INF/manifest.xml", manifest), ("content.xml", content.into_bytes())]);
}

/// The file of each input format, the table written there by the table's own exports where
/// there is one.
fn inputs(d: &Path) -> Vec<PathBuf> {
    let base = table_of(csv_text(',').into_bytes(), b',');
    let no = no_cancel();
    let mut v = Vec::new();
    let p = d.join("in-comma.csv");
    std::fs::write(&p, csv_text(',')).unwrap();
    v.push(p);
    let p = d.join("in-tab.tsv");
    std::fs::write(&p, csv_text('\t')).unwrap();
    v.push(p);
    let p = d.join("in-semicolon.csv.gz");
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(csv_text(';').as_bytes()).unwrap();
    std::fs::write(&p, z.finish().unwrap()).unwrap();
    v.push(p);
    let p = d.join("in-zstd.tsv.zst");
    std::fs::write(&p, zstd::encode_all(csv_text('\t').as_bytes(), 3).unwrap()).unwrap();
    v.push(p);
    let p = d.join("in-one.zip");
    zip_of(&p, &[("folder/in-member.csv", csv_text(',').into_bytes()), ("readme.pdf", b"x".to_vec())]);
    v.push(p);
    for (f, n) in [(Format::Xlsx, "in.xlsx"), (Format::Sqlite, "in.sqlite"), (Format::Parquet, "in.parquet")] {
        let p = d.join(n);
        convert::export(f, &base.st, None, &p, "data", None, &no).unwrap();
        v.push(p);
    }
    for (f, n) in [(TextFormat::Json, "in.json"), (TextFormat::JsonLines, "in.jsonl")] {
        let p = d.join(n);
        text_write::write(f, &base.st, None, &p, "data", None, &no).unwrap();
        v.push(p);
    }
    let p = d.join("in.xls");
    make_xls(&p);
    v.push(p);
    let p = d.join("in.xlsb");
    make_xlsb(&p);
    v.push(p);
    let p = d.join("in.ods");
    make_ods(&p);
    v.push(p);
    v
}

/// `src` opened as the app opens it: unpacked, converted, down to a table.
fn open(src: &Path, ctx: &str) -> (Table, PathBuf) {
    let no = no_cancel();
    let Some(f) = convert::import_format(src) else {
        let b = std::fs::read(src).unwrap();
        return (Table::new(Arc::new(Base::from_bytes(b, OpenOptions::default())), None), src.to_path_buf());
    };
    let sheet = match f {
        Format::Sqlite => Some(convert::sqlite::tables(src).unwrap()[0].name.clone()),
        Format::Zip => { let m = packed::members(src).unwrap_or_else(|e| panic!("{ctx}: {e}")); assert_eq!(m.len(), 1, "{ctx}"); Some(m[0].name.clone()) }
        _ => None,
    };
    if f == Format::Book {
        let s = convert::book_read::sheets(src).unwrap_or_else(|e| panic!("{ctx}: {e}"));
        assert!(s.iter().any(|x| x.filled && x.dim.is_some()), "{ctx}: {s:?}");
    }
    let dest = convert::dest_for(f, src, sheet.as_deref());
    let dest = if matches!(f, Format::Packed | Format::Zip) { dest } else { dest.with_file_name(format!("{}.conv.csv", dest.file_stem().unwrap().to_string_lossy())) };
    convert::import(f, src, sheet.as_deref(), b',', &dest, false, None, &no).unwrap_or_else(|e| panic!("{ctx}: {e}"));
    if matches!(f, Format::Packed | Format::Zip) { return open(&dest, ctx); }
    let b = std::fs::read(&dest).unwrap();
    (table_of(b, b','), dest)
}

fn outputs(t: &Table, d: &Path, ctx: &str) {
    let no = no_cancel();
    // Delimited: a save, UTF-8 and otherwise.
    for (delim, enc) in [(b',', Enc::Utf8), (b'\t', Enc::Utf8), (b'~', Enc::Utf8), (b';', Enc::Utf16Le), (b'|', Enc::Gb18030)] {
        let o = SaveOptions { enc: Some(enc), delim: Some(delim), ..Default::default() };
        let b = save::write_to(&t.st, &o, Vec::new()).unwrap();
        let back = Table::new(Arc::new(Base::from_bytes(b, OpenOptions { enc: Some(enc), delim: Some(delim) })), None);
        check(&back, &format!("{ctx} → {} {enc:?}", delim as char));
    }
    let name = "out";
    for (f, file) in [(Format::Xlsx, "out.xlsx"), (Format::Sqlite, "out.sqlite"), (Format::Parquet, "out.parquet")] {
        let p = d.join(file);
        let _ = std::fs::remove_file(&p);
        convert::export(f, &t.st, None, &p, name, None, &no).unwrap_or_else(|e| panic!("{ctx} → {file}: {e}"));
        check(&open(&p, ctx).0, &format!("{ctx} → {file}"));
    }
    for (f, file) in [(TextFormat::Json, "out.json"), (TextFormat::JsonLines, "out.jsonl")] {
        let p = d.join(file);
        text_write::write(f, &t.st, None, &p, name, None, &no).unwrap_or_else(|e| panic!("{ctx} → {file}: {e}"));
        check(&open(&p, ctx).0, &format!("{ctx} → {file}"));
    }
    for (f, file) in [(TextFormat::Html, "out.html"), (TextFormat::Markdown, "out.md")] {
        let p = d.join(file);
        text_write::write(f, &t.st, None, &p, name, None, &no).unwrap_or_else(|e| panic!("{ctx} → {file}: {e}"));
        let s = std::fs::read_to_string(&p).unwrap();
        for v in ROWS.iter().flatten().chain(TITLES.iter()).filter(|v| !v.is_empty()) {
            assert!(s.contains(*v), "{ctx} → {file}: {v}\n{s}");
        }
        assert_eq!(s.matches(if file.ends_with("html") { "<tr>" } else { "\n" }).count(), if file.ends_with("html") { 4 } else { 5 }, "{ctx} → {file}");
    }
}

#[test]
fn each_input_to_each_output() {
    let d = dir("all");
    for src in inputs(&d) {
        let ctx = src.file_name().unwrap().to_string_lossy().into_owned();
        let (t, _) = open(&src, &ctx);
        check(&t, &ctx);
        outputs(&t, &d, &ctx);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn hidden_columns_and_filtered_rows_stay_out() {
    let d = dir("hidden");
    let t = table_of(csv_text(',').into_bytes(), b',');
    let st = t.st.without_columns(|c| c == 1);
    let rows = [st.data_id(2), st.data_id(0)];
    let p = d.join("x.jsonl");
    text_write::write(TextFormat::JsonLines, &st, Some(&rows), &p, "x", None, &no_cancel()).unwrap();
    let s = std::fs::read_to_string(&p).unwrap();
    assert_eq!(s.lines().count(), 2);
    assert!(!s.contains("city") && !s.contains("東京") && !s.contains("Иван"), "{s}");
    assert!(s.starts_with("{\"name\":\"\","), "{s}");
    let p = d.join("x.tsv");
    let mut b = false;
    save::save(&st, &p, &SaveOptions { delim: Some(b'\t'), rows: Some(rows.to_vec()), backup: false, ..Default::default() }, None, &mut b).unwrap();
    let s = std::fs::read_to_string(&p).unwrap();
    assert_eq!(s.lines().next(), Some("name\tn\tdate\tnote"));
    assert_eq!(s.lines().count(), 3);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn compressed_names_and_members() {
    let p = Path::new("/x/data.csv.gz");
    assert_eq!(packed::out_path(p), Path::new("/x/data.csv"));
    assert_eq!(packed::out_path(Path::new("/x/dump.gz")), Path::new("/x/dump.csv"));
    assert_eq!(packed::out_path(Path::new("/x/a.tsv.zst")), Path::new("/x/a.tsv"));
    assert_eq!(packed::out_path(Path::new("/x/a.json.gz")), Path::new("/x/a.json"));
    assert_eq!(packed::member_path(Path::new("/x/a.zip"), "../../etc/passwd.csv"), Path::new("/x/passwd.csv"));
    assert_eq!(convert::import_format(Path::new("a.CSV.GZ")), Some(Format::Packed));
    assert_eq!(convert::import_format(Path::new("a.ODS")), Some(Format::Book));
    assert_eq!(convert::import_format(Path::new("a.zip")), Some(Format::Zip));
    let d = dir("zip");
    let z = d.join("two.zip");
    zip_of(&z, &[("a.csv", b"x\n1\n".to_vec()), ("sub/b.tsv", b"y\n2\n".to_vec()), ("__MACOSX/._a.csv", b"?".to_vec()), ("sub/", Vec::new()), ("notes.pdf", b"?".to_vec())]);
    let m = packed::members(&z).unwrap();
    assert_eq!(m.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["a.csv", "sub/b.tsv"]);
    assert_eq!(convert::offered(&m).len(), 2);
    let dest = convert::dest_for(Format::Zip, &z, Some("sub/b.tsv"));
    assert_eq!(dest, d.join("b.tsv"));
    let j = convert::import(Format::Zip, &z, Some("sub/b.tsv"), b',', &dest, false, None, &no_cancel()).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"y\n2\n");
    assert_eq!(convert::import_note("two.zip", "b.tsv", Format::Zip, &j), "two.zip → b.tsv: 1 MB unpacked.");
    // A bad file says so.
    let bad = d.join("bad.csv.gz");
    std::fs::write(&bad, b"not gzip at all").unwrap();
    assert!(convert::import(Format::Packed, &bad, None, b',', &d.join("bad.csv"), false, None, &no_cancel()).is_err());
    assert!(!d.join("bad.csv").exists());
    let _ = std::fs::remove_dir_all(&d);
}
