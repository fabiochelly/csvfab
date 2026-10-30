//! SQLite (sqlite_io.py): a table opened as a CSV, a table written as a database.
//!
//! Reading never writes: the database is opened read-only and immutable, so no
//! journal appears beside it. Writing types each column from its values —
//! INTEGER when all are integers (without a leading zero: a postal code stays
//! text), REAL when all are numbers, TEXT otherwise — and inserts every row in
//! one transaction through one prepared statement.

use super::{check, check_dir, io_err, number_text, replace, temp_beside, tick, CsvOut, Exported, Imported, Rows, Sheet};
use crate::table::{RowId, State};
use rayon::prelude::*;
use rusqlite::types::{Null, ValueRef};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

const MAGIC: &[u8; 16] = b"SQLite format 3\0";

fn unreadable(e: impl std::fmt::Display) -> String {
    format!("not a readable SQLite database ({e})")
}

/// A name quoted for SQL. SQLite reads none past a NUL (the statement was refused): dropped.
fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('\0', "").replace('"', "\"\""))
}

/// Read-only and immutable: opening a database to read it must never write it.
fn open_ro(path: &Path) -> Result<Connection, String> {
    let mut head = [0u8; 16];
    let mut f = std::fs::File::open(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "file not found".into() } else { e.to_string() })?;
    if f.read_exact(&mut head).is_err() || &head != MAGIC {
        return Err(unreadable("not a SQLite database"));
    }
    let abs = std::path::absolute(path).map_err(io_err)?;
    let mut uri = String::from("file:");
    for &b in abs.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/_.-~".contains(&b) { uri.push(b as char); } else { uri.push_str(&format!("%{b:02X}")); }
    }
    uri.push_str("?mode=ro&immutable=1");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(uri, flags).map_err(unreadable)
}

/// Tables and views, as a workbook's sheets. Listing never reads a table: one row
/// (LIMIT 1) tells whether it is filled, max(rowid) — an index page — its row
/// count: exact without deletions, an upper bound otherwise (approx). A view or
/// a WITHOUT ROWID table has no cheap count: dim None.
fn list(con: &Connection) -> Result<Vec<Sheet>, String> {
    let mut q = con.prepare("SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY type = 'view', name").map_err(unreadable)?;
    let names: Vec<(String, String)> = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(unreadable)?.collect::<Result<_, _>>().map_err(unreadable)?;
    Ok(names.into_iter().map(|(name, typ)| {
        let q = ident(&name);
        let view = typ == "view";
        let probe = || -> rusqlite::Result<(usize, bool)> {
            let cols = con.prepare(&format!("SELECT * FROM {q} LIMIT 0"))?.column_count();
            let filled = con.prepare(&format!("SELECT 1 FROM {q} LIMIT 1"))?.exists([])?;
            Ok((cols, filled))
        };
        // A broken view (its table gone): listed, empty.
        let (cols, filled, n) = match probe() {
            Ok((cols, filled)) => {
                let n = if view { None } else {
                    con.query_row(&format!("SELECT max(rowid) FROM {q}"), [], |r| r.get::<_, Option<i64>>(0)).ok().map(|n| n.unwrap_or(0) as u64)
                };
                (cols, filled, n)
            }
            Err(_) => (0, false, Some(0)),
        };
        let dim = if n.is_some() || !filled { Some((if filled { n.unwrap_or(0) } else { 0 }, cols as u64)) } else { None };
        Sheet { name, hidden: false, filled, view, dim, approx: filled && n.is_some() }
    }).collect())
}

/// The tables and views of the database at `path`.
pub fn tables(path: &Path) -> Result<Vec<Sheet>, String> {
    list(&open_ro(path)?)
}

/// A table (or view) as CSV into `out`, its header line first; `table`: its name
/// (None: the first table with rows). Numbers as in a workbook (a decimal comma
/// in a `;` file), NULL empty, a BLOB in hexadecimal (0x…). Progress: rows.
pub fn to_csv<W: Write>(path: &Path, table: Option<&str>, delim: u8, out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let con = open_ro(path)?;
    let tables = list(&con)?;
    if tables.is_empty() {
        return Err(unreadable("the database holds no table"));
    }
    let pick = match table {
        Some(t) => tables.iter().position(|s| s.name == t).ok_or_else(|| unreadable(format!("no table \"{t}\"")))?,
        None => tables.iter().position(|s| s.filled && !s.view).unwrap_or(0),
    };
    let name = tables[pick].name.clone();
    let comma = delim == b';';
    let mut st = con.prepare(&format!("SELECT * FROM {}", ident(&name))).map_err(unreadable)?;
    // The names as bytes: `column_names` panics on one that is not UTF-8 (a broken file).
    let ncols = st.column_count();
    let names: Vec<String> = con.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
        .and_then(|mut q| q.query_map([&name], |r| Ok(match r.get_ref(0)? { ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(), _ => String::new() }))?.collect::<Result<Vec<_>, _>>())
        .ok().filter(|v| v.len() == ncols)
        .unwrap_or_else(|| (1..=ncols).map(|c| format!("col{c}")).collect());
    let mut csv = CsvOut::new(out, delim, true);
    for n in &names { csv.field(n).map_err(io_err)?; }
    csv.end().map_err(io_err)?;
    let mut rows = st.query([]).map_err(unreadable)?;
    let (mut count, mut cell) = (0u64, String::new());
    while let Some(r) = rows.next().map_err(unreadable)? {
        for c in 0..ncols {
            cell.clear();
            let text = match r.get_ref(c).map_err(unreadable)? {
                ValueRef::Null => "",
                ValueRef::Integer(i) => { use std::fmt::Write; let _ = write!(cell, "{i}"); &cell }
                ValueRef::Real(x) => { number_text(x, comma, &mut cell); &cell }
                ValueRef::Text(t) => match std::str::from_utf8(t) {
                    Ok(s) => s,
                    Err(_) => { cell.push_str(&String::from_utf8_lossy(t)); &cell }
                },
                ValueRef::Blob(b) => {
                    use std::fmt::Write;
                    cell.push_str("0x");
                    for x in b { let _ = write!(cell, "{x:02x}"); }
                    &cell
                }
            };
            csv.field(text).map_err(io_err)?;
        }
        csv.end().map_err(io_err)?;
        count += 1;
        if count & 4095 == 0 {
            check(cancel)?;
            tick(progress, 4096);
        }
    }
    tick(progress, (count & 4095) as usize);
    csv.w.flush().map_err(io_err)?;
    let others = tables.iter().enumerate().filter(|&(k, t)| k != pick && t.filled).count();
    Ok(Imported { sheet: Some(name), others, rows: count, cols: ncols, ..Default::default() })
}

// --- typing --------------------------------------------------------------------

/// `^-?(0|[1-9]\d{0,14})$`
fn is_int(v: &str) -> bool {
    let d = v.strip_prefix('-').unwrap_or(v).as_bytes();
    !d.is_empty() && d.len() <= 15 && d.iter().all(u8::is_ascii_digit) && (d[0] != b'0' || d.len() == 1)
}

/// `^(0\d+|\d{16,})$`: a leading zero or more than 15 digits — a code, not a number.
fn is_code(v: &str) -> bool {
    let d = v.as_bytes();
    d.len() > 1 && d.iter().all(u8::is_ascii_digit) && (d[0] == b'0' || d.len() >= 16)
}

/// Digits, then groups of `sep` + 3 digits (at least one when `min1`), then an
/// optional `dec` + digits: `^-?\d{1,3}(S\d{3})+(D\d+)?$` and `^-?\d+([.,]\d+)?$`.
fn shape(x: &[u8], sep: Option<u8>, dec: &[u8]) -> bool {
    let mut i = (x.first() == Some(&b'-')) as usize;
    let n = x[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 { return false; }
    i += n;
    if let Some(s) = sep {
        if n > 3 { return false; }
        let mut groups = 0;
        while x.get(i) == Some(&s) && x.len() >= i + 4 && x[i + 1..i + 4].iter().all(u8::is_ascii_digit) && !x.get(i + 4).is_some_and(u8::is_ascii_digit) {
            i += 4;
            groups += 1;
        }
        if groups == 0 { return false; }
    }
    if x.get(i).is_some_and(|c| dec.contains(c)) {
        let m = x[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if m == 0 { return false; }
        i += 1 + m;
    }
    i == x.len()
}

/// A number written the French or English way (1 234,50 · 1,234.50 · 12,5) → f64.
pub(crate) fn sql_real(v: &str) -> Option<f64> {
    let x: String = v.chars().filter(|&c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}')).collect();
    let b = x.as_bytes();
    let (plain, en, fr) = (shape(b, None, b".,"), shape(b, Some(b','), b"."), shape(b, Some(b'.'), b","));
    if !(plain || en || fr) { return None; }
    let y = if x.contains(',') && x.contains('.') {
        if x.rfind('.') > x.rfind(',') { x.replace(',', "") } else { x.replace('.', "").replace(',', ".") }
    } else if en {
        x.replace(',', "")
    } else if fr {
        x.replace('.', "")
    } else {
        x.replace(',', ".")
    };
    y.parse().ok()
}

/// Per column: values, integers, numbers, codes.
type Counts = [u64; 4];

fn see(k: &mut Counts, v: &str) {
    let v = v.trim();
    if v.is_empty() { return; }
    k[0] += 1;
    if is_code(v) {
        k[3] += 1; // 01000, 0612345678, a 16-digit identifier: text
    } else if is_int(v) {
        k[1] += 1;
        k[2] += 1;
    } else if sql_real(v).is_some() {
        k[2] += 1;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    Int,
    Real,
    Text,
}

impl Ty {
    fn of(k: &Counts) -> Ty {
        if k[3] > 0 { Ty::Text } else if k[0] > 0 && k[1] == k[0] { Ty::Int } else if k[0] > 0 && k[2] == k[0] { Ty::Real } else { Ty::Text }
    }
    fn sql(self) -> &'static str {
        match self { Ty::Int => "INTEGER", Ty::Real => "REAL", Ty::Text => "TEXT" }
    }
}

/// Column names: the header's, trimmed, `colN` when empty, `_2`, `_3`… when repeated (case ignored).
fn column_names(header: Option<&[String]>, width: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    (0..width).map(|c| {
        // Without NUL already (see `ident`): two names differing by one stay two.
        let t = header.and_then(|h| h.get(c)).map_or(String::new(), |s| s.replace('\0', ""));
        let t = t.trim();
        let base = if t.is_empty() { format!("col{}", c + 1) } else { t.to_string() };
        let (mut n, mut i) = (base.clone(), 2);
        while seen.contains(&n.to_lowercase()) {
            n = format!("{base}_{i}");
            i += 1;
        }
        seen.insert(n.to_lowercase());
        n
    }).collect()
}

/// Writes the table to `dest` as a database of one table, `table`, atomically.
/// See [`super::export`].
pub fn write(st: &State, rows: Option<&[RowId]>, dest: &Path, table: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    check_dir(dest)?;
    let rows = Rows { st, ids: rows };
    let n = rows.len();
    let width = st.ncols;
    const CHUNK: usize = 8192;
    let parts: Vec<Vec<Counts>> = (0..n.div_ceil(CHUNK)).into_par_iter().map(|k| {
        let mut ks = vec![[0u64; 4]; width];
        if check(cancel).is_err() { return ks; }
        let end = ((k + 1) * CHUNK).min(n);
        for pos in k * CHUNK..end {
            st.visit(rows.id(pos), |c, v| if let Some(x) = ks.get_mut(c) { see(x, v) });
        }
        tick(progress, end - k * CHUNK);
        ks
    }).collect();
    check(cancel)?;
    let mut kinds = vec![[0u64; 4]; width];
    for p in &parts {
        for (a, b) in kinds.iter_mut().zip(p) {
            for i in 0..4 { a[i] += b[i]; }
        }
    }
    let types: Vec<Ty> = kinds.iter().map(Ty::of).collect();
    let header = st.header.then(|| st.headers());
    let cols = column_names(header.as_deref(), width);

    let tmp_path = temp_beside(dest);
    let res = (|| -> Result<u64, String> {
        let e = |e: rusqlite::Error| e.to_string();
        let mut con = Connection::open(&tmp_path).map_err(e)?;
        // A new file renamed into place once complete: no journal, no sync until then.
        con.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA locking_mode = EXCLUSIVE;").map_err(e)?;
        let defs: Vec<String> = cols.iter().zip(&types).map(|(c, t)| format!("{} {}", ident(c), t.sql())).collect();
        con.execute(&format!("CREATE TABLE {} ({})", ident(table), defs.join(", ")), []).map_err(e)?;
        let tx = con.transaction().map_err(e)?;
        {
            let marks = vec!["?"; width].join(", ");
            let mut ins = tx.prepare(&format!("INSERT INTO {} VALUES ({marks})", ident(table))).map_err(e)?;
            let mut failed: Option<rusqlite::Error> = None;
            for pos in 0..n {
                if pos & 4095 == 4095 {
                    check(cancel)?;
                    tick(progress, 4096);
                }
                let mut next = 0;
                st.visit(rows.id(pos), |c, v| {
                    if c >= width || failed.is_some() { return; }
                    let t = v.trim();
                    let r = if t.is_empty() {
                        ins.raw_bind_parameter(c + 1, Null)
                    } else {
                        match types[c] {
                            Ty::Int => ins.raw_bind_parameter(c + 1, t.parse::<i64>().ok()),
                            Ty::Real => ins.raw_bind_parameter(c + 1, sql_real(t)),
                            Ty::Text => ins.raw_bind_parameter(c + 1, v), // as written, spaces included
                        }
                    };
                    if let Err(x) = r { failed = Some(x); }
                    next = c + 1;
                });
                for c in next..width {
                    ins.raw_bind_parameter(c + 1, Null).map_err(e)?;
                }
                if let Some(x) = failed.take() { return Err(x.to_string()); }
                ins.raw_execute().map_err(e)?;
            }
            tick(progress, n % 4096);
        }
        tx.commit().map_err(e)?;
        con.close().map_err(|(_, x)| x.to_string())?;
        // Synced through a handle that may write: Windows refuses to flush a read-only one.
        std::fs::OpenOptions::new().write(true).open(&tmp_path).and_then(|f| f.sync_all()).map_err(io_err)?;
        replace(&tmp_path, dest, false).map_err(io_err)?;
        Ok(n as u64)
    })();
    match res {
        Ok(rows) => Ok(Exported { rows, cols: width, types: cols.into_iter().zip(types.iter().map(|t| t.sql())).collect(), ..Default::default() }),
        Err(x) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(x)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn tmpdir(tag: &str) -> PathBuf {
        super::super::xlsx_read::tests::tmpdir(tag)
    }

    fn table(csv: &str, header: bool) -> Table {
        let delim = if csv.starts_with("n,") || csv.starts_with("x\"") { b',' } else { b';' };
        Table::new(Arc::new(Base::from_bytes(csv.as_bytes().to_vec(), OpenOptions { delim: Some(delim), ..Default::default() })), Some(header))
    }

    fn base(dir: &Path) -> PathBuf {
        let db = dir.join("base.sqlite");
        let c = Connection::open(&db).unwrap();
        c.execute_batch("
            create table clients(id integer, nom text, montant real, photo blob, note text);
            insert into clients values (1, 'Dupont', 1234.5, x'00ff', null),
                                       (2, 'Lefèvre;x', -2.0, null, 'ok');
            create table vide(a);
            create view v as select id, nom from clients;").unwrap();
        db
    }

    fn convert(p: &Path, t: Option<&str>, delim: u8) -> Result<(String, Imported), String> {
        let mut out = Vec::new();
        let info = to_csv(p, t, delim, &mut out, None, &AtomicBool::new(false))?;
        Ok((String::from_utf8(out).unwrap(), info))
    }

    fn sheet(name: &str, view: bool, filled: bool, dim: Option<(u64, u64)>, approx: bool) -> Sheet {
        Sheet { name: name.into(), hidden: false, filled, view, dim, approx }
    }

    #[test]
    fn table_list() {
        let dir = tmpdir("sq-list");
        assert_eq!(tables(&base(&dir)).unwrap(), vec![
            sheet("clients", false, true, Some((2, 5)), true),
            sheet("vide", false, false, Some((0, 1)), false),
            sheet("v", true, true, None, false)]);
        let big = dir.join("big.db");
        let c = Connection::open(&big).unwrap();
        c.execute_batch("create table t(a, b); create table w(k primary key, v) without rowid; create view v as select a from t;
            with recursive s(i) as (select 0 union all select i + 1 from s where i < 4999) insert into t select i, i from s;
            delete from t where a < 10; insert into w values (1, 'x');").unwrap();
        drop(c);
        let l = tables(&big).unwrap();
        let get = |n: &str| l.iter().find(|s| s.name == n).unwrap().clone();
        assert_eq!((get("t").dim, get("t").approx), (Some((5000, 2)), true)); // 4,990 rows: a bound
        assert_eq!((get("w").filled, get("w").dim), (true, None));
        assert_eq!((get("v").filled, get("v").dim), (true, None));
    }

    #[test]
    fn table_to_csv() {
        let dir = tmpdir("sq-csv");
        let db = base(&dir);
        let (text, info) = convert(&db, Some("clients"), b';').unwrap();
        assert_eq!(text, "id;nom;montant;photo;note\n1;Dupont;1234,5;0x00ff;\n2;\"Lefèvre;x\";-2;;ok\n");
        assert_eq!(info, Imported { sheet: Some("clients".into()), others: 1, rows: 2, cols: 5, ..Default::default() });
        assert_eq!(convert(&db, Some("v"), b',').unwrap().0, "id,nom\n1,Dupont\n2,Lefèvre;x\n");
        assert_eq!(convert(&db, None, b';').unwrap().1.sheet.as_deref(), Some("clients"));
        assert!(convert(&db, Some("nope"), b';').unwrap_err().contains("no table \"nope\""));
        let one = dir.join("one.db");
        Connection::open(&one).unwrap().execute_batch("create table t(a, b); insert into t values ('x', 1.0); create table e(z);").unwrap();
        assert_eq!(super::super::offered(&tables(&one).unwrap()).len(), 1);
        assert_eq!(convert(&one, None, b';').unwrap().0, "a;b\nx;1\n");
    }

    #[test]
    fn database_is_not_modified() {
        let dir = tmpdir("sq-ro");
        let db = base(&dir);
        let before = std::fs::metadata(&db).unwrap();
        convert(&db, Some("clients"), b';').unwrap();
        let after = std::fs::metadata(&db).unwrap();
        assert_eq!((before.modified().unwrap(), before.len()), (after.modified().unwrap(), after.len()));
        let mut j = db.clone().into_os_string();
        j.push("-journal");
        assert!(!Path::new(&j).exists());
    }

    #[test]
    fn not_a_database() {
        let dir = tmpdir("sq-bad");
        let p = dir.join("x.db");
        std::fs::write(&p, "a;b\n").unwrap();
        let e = convert(&p, None, b';').unwrap_err();
        assert!(e.contains("SQLite"), "{e}");
        assert!(tables(&p).is_err());
        assert_eq!(tables(&dir.join("none.db")).unwrap_err(), "file not found");
    }

    fn export(t: &Table, dest: &Path, name: &str) -> Exported {
        write(&t.st, None, dest, name, None, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn column_types() {
        let dir = tmpdir("sq-types");
        let dest = dir.join("out.sqlite");
        let t = table("Code;Qte;Prix;Nom;Vide\n01000;3;1 234,5;Dupont;\n02000;-4;2,25;Lefèvre;\n12345678901234567;5;3;x;\n", true);
        let info = export(&t, &dest, "ventes");
        let ty = |v: &[(&str, &'static str)]| v.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        assert_eq!(info, Exported { rows: 3, cols: 5, types: ty(&[("Code", "TEXT"), ("Qte", "INTEGER"), ("Prix", "REAL"), ("Nom", "TEXT"), ("Vide", "TEXT")]), ..Default::default() });
        let c = Connection::open(&dest).unwrap();
        let mut q = c.prepare("select Code, Qte, Prix, Nom, Vide, typeof(Code), typeof(Prix), typeof(Vide) from ventes").unwrap();
        let got: Vec<(String, i64, f64, String, Option<String>, String, String, String)> = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))).unwrap().map(|r| r.unwrap()).collect();
        let row = |a: &str, b, c, d: &str| (a.to_string(), b, c, d.to_string(), None, "text".to_string(), "real".to_string(), "null".to_string());
        assert_eq!(got, vec![row("01000", 3, 1234.5, "Dupont"), row("02000", -4, 2.25, "Lefèvre"), row("12345678901234567", 5, 3.0, "x")]);
    }

    #[test]
    fn english_numbers_and_ragged_rows() {
        let dir = tmpdir("sq-en");
        let dest = dir.join("en.sqlite");
        export(&table("n,m\n\"1,234.5\",1.5\n2,2\n", true), &dest, "t");
        let c = Connection::open(&dest).unwrap();
        let got: Vec<(f64, f64)> = c.prepare("select n, m from t").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(got, vec![(1234.5, 1.5), (2.0, 2.0)]);
        let dest = dir.join("t.db");
        let info = export(&table("a;A;;b\n1;2\n3;4;5;6;7\n", true), &dest, "t");
        assert_eq!(info.types.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["a", "A_2", "col3", "b", "col5"]);
        let c = Connection::open(&dest).unwrap();
        let got: Vec<Vec<Option<i64>>> = c.prepare("select * from t").unwrap().query_map([], |r| (0..5).map(|i| r.get(i)).collect()).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(got, vec![vec![Some(1), Some(2), None, None, None], vec![Some(3), Some(4), Some(5), Some(6), Some(7)]]);
    }

    #[test]
    fn replaces_and_round_trips() {
        let dir = tmpdir("sq-rt");
        let dest = dir.join("rt.sqlite");
        export(&table("a\n1\n", true), &dest, "t");
        export(&table("Code;Montant;Nom\n007;1,5;Zoé\n008;-2;\"a;b\"\n", true), &dest, "rt");
        assert_eq!(tables(&dest).unwrap().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["rt"]);
        assert_eq!(convert(&dest, None, b';').unwrap().0, "Code;Montant;Nom\n007;1,5;Zoé\n008;-2;\"a;b\"\n");
        // Identifiers are quoted.
        export(&table("x\"; drop table t; --\n1\n", true), &dest, "ta\"ble");
        let (text, info) = convert(&dest, None, b',').unwrap();
        assert_eq!((info.sheet.as_deref(), text.as_str()), (Some("ta\"ble"), "\"x\"\"; drop table t; --\"\n1\n"));
        // Without a header: every row is data.
        export(&table("1;x\n2;y\n", false), &dest, "h");
        assert_eq!(convert(&dest, None, b';').unwrap().0, "col1;col2\n1;x\n2;y\n");
    }

    #[test]
    fn sql_reals() {
        for (t, want) in [("1 234,50", 1234.5), ("1,234.50", 1234.5), ("1.234,5", 1234.5), ("12,5", 12.5), ("1,234", 1234.0), ("-3", -3.0), ("1\u{a0}000,5", 1000.5), ("1.234", 1234.0), ("12.5", 12.5), ("1,2345", 1.2345)] {
            assert_eq!(sql_real(t), Some(want), "{t}");
        }
        for t in ["abc", "1,2,3", "1.2.3,4,5", "", "12%"] {
            assert_eq!(sql_real(t), None, "{t}");
        }
        assert!(is_int("0") && is_int("-12") && !is_int("01") && !is_int("1234567890123456"));
        assert!(is_code("01000") && is_code("1234567890123456") && !is_code("0") && !is_code("-01"));
    }
}
