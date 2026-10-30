//! Split into files: the rows shown, one file per value of a column, written
//! in a new folder beside the source — a folder of its own, so no file
//! already there is ever overwritten. Each file is written like a save (same
//! delimiter, encoding, BOM and line breaks, untouched rows copied as their
//! bytes), the title line repeated in each. The source is never touched.

use crate::save::{self, SaveOptions};
use crate::table::{RowId, State};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use unicode_normalization::UnicodeNormalization;

/// Files beyond which the app asks first…
pub const SPLIT_ASK: usize = 100;
/// …and beyond which it refuses: a column of ids, not of categories.
pub const SPLIT_MAX: usize = 5000;

/// How two cells count as the same value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Match {
    #[default]
    Exact,
    /// Case and surrounding spaces ignored.
    Loose,
    /// Case, accents and punctuation ignored (`slugify`).
    Slug,
}

impl Match {
    pub fn key<'a>(self, v: &'a str) -> std::borrow::Cow<'a, str> {
        match self {
            Match::Exact => v.into(),
            Match::Loose => v.trim().to_lowercase().into(),
            Match::Slug => slugify(v).into(),
        }
    }
}

/// Letters without their accents: canonical decomposition, combining
/// diacritics (U+0300–U+036F) dropped — as `normalize('NFD')` + strip did.
pub fn remove_accents(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    s.nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect()
}

#[inline]
pub(crate) fn is_letter_or_number(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

/// A title or value reduced to what tells it apart: lower case, no accents,
/// ligatures spelled out, every run of other characters a single space.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    let mut put = |c: char, out: &mut String| {
        if is_letter_or_number(c) {
            if gap && !out.is_empty() { out.push(' '); }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    };
    for c in s.chars() {
        if c.is_ascii() {
            put(c.to_ascii_lowercase(), &mut out);
            continue;
        }
        for l in c.to_lowercase() {
            // Letters NFD cannot split into base + accent.
            let lig = match l { 'œ' => "oe", 'æ' => "ae", 'ß' => "ss", 'ø' => "o", 'đ' => "d", 'ł' => "l", 'ð' => "d", 'þ' => "th", _ => "" };
            if !lig.is_empty() {
                lig.chars().for_each(|x| put(x, &mut out));
                continue;
            }
            for d in std::iter::once(l).nfd() {
                if !('\u{300}'..='\u{36f}').contains(&d) { put(d, &mut out); }
            }
        }
    }
    out
}

/// A value as a file name on every system: no `/ \ : * ? " < > |` nor control
/// characters, no trailing dot or space (Windows drops them), 80 characters at most.
pub fn safe_file_name(s: &str) -> String {
    let mut v = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        let c = if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 0x20 || c == '\u{7f}' { '_' } else { c };
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !v.is_empty() { v.push(' '); }
        space = false;
        v.push(c);
    }
    let v = v.trim_end_matches(['.', ' ']);
    if v.chars().count() > 80 { v.chars().take(80).collect::<String>().trim().to_string() } else { v.to_string() }
}

/// The name without its table extension.
pub fn stem_of(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".csv", ".tsv", ".txt", ".xlsx", ".sqlite", ".parquet"] {
        if lower.ends_with(ext) && name.is_char_boundary(name.len() - ext.len()) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// The extension new files get: the source's own (.csv, .tsv, .txt), else .csv.
pub fn csv_ext(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".csv", ".tsv", ".txt"] {
        if lower.ends_with(ext) && name.is_char_boundary(name.len() - ext.len()) {
            return &name[name.len() - ext.len()..];
        }
    }
    ".csv"
}

pub struct Group {
    /// The value as first met.
    pub value: String,
    /// Its file's name (unique, case ignored).
    pub name: String,
    /// Its rows, in the order given.
    pub rows: Vec<RowId>,
}

/// A column of categories: the first with 2 to 50 values over the first 5000 rows.
pub fn guess_column(st: &State, rows: &[RowId]) -> usize {
    let rows = &rows[..rows.len().min(5000)];
    let mut tmp = Vec::new();
    for c in 0..st.ncols {
        let mut seen = std::collections::HashSet::new();
        for &id in rows {
            st.with_cell(id, c, &mut tmp, |v| { if !seen.contains(v) { seen.insert(v.to_string()); } });
            if seen.len() > 50 { break; }
        }
        if (2..=50).contains(&seen.len()) { return c; }
    }
    0
}

/// The groups of `rows` by their value in `col`, in order of first
/// appearance, named after `file_name` (the source's). None if cancelled.
pub fn groups(st: &State, rows: &[RowId], col: usize, m: Match, file_name: &str, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Vec<Group>> {
    const CH: usize = 1 << 16;
    // Each slice groups its own rows (in order); the slices are then merged
    // in order, so "first appearance" is the same as one pass would give.
    let parts: Vec<Vec<(String, String, Vec<RowId>)>> = rows
        .par_chunks(CH)
        .map(|chunk| {
            let mut idx: HashMap<String, usize> = HashMap::new();
            let mut out: Vec<(String, String, Vec<RowId>)> = Vec::new();
            let mut tmp = Vec::new();
            for &id in chunk {
                if cancel.load(Ordering::Relaxed) { break; }
                st.with_cell(id, col, &mut tmp, |v| {
                    let k = m.key(v);
                    match idx.get(k.as_ref()) {
                        Some(&g) => out[g].2.push(id),
                        None => {
                            idx.insert(k.clone().into_owned(), out.len());
                            out.push((k.into_owned(), v.to_string(), vec![id]));
                        }
                    }
                });
            }
            if let Some(p) = progress { p.fetch_add(chunk.len(), Ordering::Relaxed); }
            out
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for part in parts {
        for (k, value, ids) in part {
            match idx.get(&k) {
                Some(&g) => groups[g].rows.extend_from_slice(&ids),
                None => {
                    idx.insert(k, groups.len());
                    groups.push(Group { value, name: String::new(), rows: ids });
                }
            }
        }
    }
    // Two values giving one name (Paris / PARIS on a disk that ignores case,
    // a / and a _) get (2), (3)… rather than one overwriting the other.
    let stem = { let s = safe_file_name(stem_of(file_name)); if s.is_empty() { "part".to_string() } else { s } };
    let ext = csv_ext(file_name);
    let mut used = std::collections::HashSet::new();
    for g in &mut groups {
        let v = safe_file_name(&g.value);
        let base = format!("{stem} - {}", if v.is_empty() { "(empty)" } else { &v });
        let mut name = format!("{base}{ext}");
        let mut n = 2;
        while used.contains(&name.to_lowercase()) {
            name = format!("{base} ({n}){ext}");
            n += 1;
        }
        used.insert(name.to_lowercase());
        g.name = name;
    }
    Some(groups)
}

/// The folder the files go in: `<stem> - by <column title>`.
pub fn folder_name(file_name: &str, title: &str, col: usize) -> String {
    let stem = safe_file_name(stem_of(file_name));
    let t = safe_file_name(title);
    format!("{} - by {}", if stem.is_empty() { "split" } else { &stem }, if t.is_empty() { format!("column {}", col + 1) } else { t })
}

/// What a split did: the folder made, and how many files were written in it.
#[derive(Debug)]
pub struct Split {
    pub folder: Option<PathBuf>,
    pub written: usize,
    /// None: every file written.
    pub error: Option<io::Error>,
}

/// A writer that stops when the job is cancelled.
struct Guard<'a, W> {
    w: W,
    stop: &'a AtomicBool,
    cancel: &'a AtomicBool,
}

impl<W: Write> Write for Guard<'_, W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed) {
            // Not ErrorKind::Interrupted: write_all retries those, forever.
            return Err(io::Error::other("cancelled"));
        }
        self.w.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// Makes `<parent>/<folder>`, or `<folder> (2)`, (3)… — never an existing one.
fn new_folder(parent: &Path, folder: &str) -> io::Result<PathBuf> {
    for n in 1..1000 {
        let p = parent.join(if n > 1 { format!("{folder} ({n})") } else { folder.to_string() });
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free folder name there"))
}

/// Writes one file per group in a new folder `folder` of `parent`. Groups
/// are written in parallel, each file opened, written and closed by one task:
/// at most one open file per thread, whatever the number of groups. Every
/// file is what a save of those rows would give (`save::write_to`): the
/// title line, then the rows, untouched ones as their bytes. `progress`
/// counts the rows of the files written.
pub fn write(st: &State, groups: &[Group], parent: &Path, folder: &str, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Split {
    let dir = match new_folder(parent, folder) {
        Ok(d) => d,
        Err(e) => return Split { folder: None, written: 0, error: Some(e) },
    };
    let stop = AtomicBool::new(false);
    let written = AtomicUsize::new(0);
    let first_err: std::sync::Mutex<Option<io::Error>> = Default::default();
    groups.par_iter().with_max_len(1).for_each(|g| {
        if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) { return; }
        let path = dir.join(&g.name);
        let res = (|| -> io::Result<()> {
            let f = std::fs::File::create(&path)?;
            let o = SaveOptions { rows: Some(g.rows.clone()), backup: false, ..Default::default() };
            let w = save::write_to(st, &o, BufWriter::with_capacity(256 << 10, Guard { w: f, stop: &stop, cancel }))?;
            w.into_inner().map_err(|e| e.into_error())?;
            Ok(())
        })();
        match res {
            Ok(()) => {
                written.fetch_add(1, Ordering::Relaxed);
                if let Some(p) = progress { p.fetch_add(g.rows.len(), Ordering::Relaxed); }
            }
            Err(e) => {
                // A half-written file would pass for a whole one.
                let _ = std::fs::remove_file(&path);
                if !cancel.load(Ordering::Relaxed) && !stop.swap(true, Ordering::Relaxed) {
                    *first_err.lock().unwrap() = Some(io::Error::new(e.kind(), format!("{}: {e}", g.name)));
                }
            }
        }
    });
    let mut error = first_err.into_inner().unwrap();
    if error.is_none() && cancel.load(Ordering::Relaxed) {
        error = Some(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    Split { folder: Some(dir), written: written.into_inner(), error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    const VENTES: &[u8] = b"id;ville;note\r\n1;Paris;\"a;b\"\r\n2;Lyon;x\r\n3;;vide\r\n4;A/B;slash\r\n5;PARIS;caps\r\n6;Paris;  espaces  \r\n";

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("csvfab-split-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn files(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(dir).unwrap().map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())
        }).collect()
    }

    #[test]
    fn names() {
        assert_eq!(slugify("  Élodie-Œuvre, ÇA!! "), "elodie oeuvre ca");
        assert_eq!(safe_file_name("a/b: c?  d. "), "a_b_ c_ d");
        assert_eq!(stem_of("ventes.CSV"), "ventes");
        assert_eq!(csv_ext("x.TSV"), ".TSV");
        assert_eq!(csv_ext("x.xlsx"), ".csv");
        assert_eq!(folder_name("ventes.csv", "ville", 1), "ventes - by ville");
    }

    #[test]
    fn split_by_value() {
        let mut t = Table::new(Arc::new(Base::from_bytes(VENTES.to_vec(), OpenOptions::default())), None);
        let no = AtomicBool::new(false);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        assert_eq!(guess_column(&t.st, &rows), 0);    // six ids: as good as categories here
        let g = groups(&t.st, &rows, 1, Match::Exact, "ventes.csv", &no, None).unwrap();
        let names: Vec<&str> = g.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["ventes - Paris.csv", "ventes - Lyon.csv", "ventes - (empty).csv", "ventes - A_B.csv", "ventes - PARIS (2).csv"]);
        let dir = tmpdir("a");
        let s = write(&t.st, &g, &dir, &folder_name("ventes.csv", "ville", 1), &no, None);
        assert!(s.error.is_none());
        assert_eq!(s.written, 5);
        let got = files(&dir.join("ventes - by ville"));
        let head = b"id;ville;note\r\n".to_vec();
        let cat = |b: &[u8]| [head.clone(), b.to_vec()].concat();
        assert_eq!(got["ventes - Paris.csv"], cat(b"1;Paris;\"a;b\"\r\n6;Paris;  espaces  \r\n"));
        assert_eq!(got["ventes - Lyon.csv"], cat(b"2;Lyon;x\r\n"));
        assert_eq!(got["ventes - (empty).csv"], cat(b"3;;vide\r\n"));
        assert_eq!(got["ventes - A_B.csv"], cat(b"4;A/B;slash\r\n"));
        assert_eq!(got["ventes - PARIS (2).csv"], cat(b"5;PARIS;caps\r\n"));

        // Again, slug, an edited row: another folder, Paris and PARIS together.
        let id = t.st.data_id(1);
        t.set_cells(&[(id, 2, "modifié".into())]);
        let g = groups(&t.st, &rows, 1, Match::Slug, "ventes.csv", &no, None).unwrap();
        let names: Vec<&str> = g.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["ventes - Paris.csv", "ventes - Lyon.csv", "ventes - (empty).csv", "ventes - A_B.csv"]);
        let s = write(&t.st, &g, &dir, &folder_name("ventes.csv", "ville", 1), &no, None);
        assert_eq!(s.folder.as_deref(), Some(dir.join("ventes - by ville (2)").as_path()));
        let got = files(&dir.join("ventes - by ville (2)"));
        assert_eq!(got.len(), 4);
        assert_eq!(got["ventes - Paris.csv"], cat(b"1;Paris;\"a;b\"\r\n5;PARIS;caps\r\n6;Paris;  espaces  \r\n"));
        assert_eq!(got["ventes - Lyon.csv"], cat("2;Lyon;modifié\r\n".as_bytes()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_split_stops() {
        let csv: String = std::iter::once("k,v\n".to_string()).chain((0..50_000).map(|i| format!("{},{i}\n", i % 7))).collect();
        let t = Table::new(Arc::new(Base::from_bytes(csv.into_bytes(), OpenOptions::default())), None);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let g = groups(&t.st, &rows, 0, Match::Exact, "f.csv", &AtomicBool::new(false), None).unwrap();
        let dir = tmpdir("c");
        let s = write(&t.st, &g, &dir, "out", &AtomicBool::new(true), None);
        assert_eq!(s.written, 0);
        assert_eq!(s.error.unwrap().kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read_dir(dir.join("out")).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn guesses_a_column_of_categories() {
        let csv: String = std::iter::once("id;ville\n".to_string()).chain((0..80).map(|i| format!("{i};{}\n", ["Paris", "Lyon", "Brest"][i % 3]))).collect();
        let t = Table::new(Arc::new(Base::from_bytes(csv.into_bytes(), OpenOptions::default())), None);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        assert_eq!(guess_column(&t.st, &rows), 1);
    }

    #[test]
    fn keeps_encoding_and_bom() {
        let mut bytes = b"\xEF\xBB\xBFk,v\n".to_vec();
        bytes.extend_from_slice("a,é\nb,2\na,3".as_bytes());
        let t = Table::new(Arc::new(Base::from_bytes(bytes, OpenOptions::default())), None);
        let no = AtomicBool::new(false);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let g = groups(&t.st, &rows, 0, Match::Exact, "f.csv", &no, None).unwrap();
        let dir = tmpdir("b");
        let s = write(&t.st, &g, &dir, "out", &no, None);
        assert!(s.error.is_none());
        let got = files(&dir.join("out"));
        assert_eq!(got["f - a.csv"], "\u{feff}k,v\na,é\na,3".as_bytes());
        assert_eq!(got["f - b.csv"], "\u{feff}k,v\nb,2\n".as_bytes());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Timings on a big file: `CSVFAB_BENCH=customers-1m.csv [CSVFAB_BENCH_OUT=dir] cargo test
    /// --release bench_customers -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_customers() {
        use crate::{anonymize, combine, fill_example, review, tail};
        use std::time::Instant;
        let path = PathBuf::from(std::env::var("CSVFAB_BENCH").expect("CSVFAB_BENCH: a CSV file"));
        let out = std::env::var("CSVFAB_BENCH_OUT").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir()).join(format!("csvfab-bench-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        let no = AtomicBool::new(false);
        let time = |what: &str, t: Instant| eprintln!("{what:<44} {:>8.1} ms", t.elapsed().as_secs_f64() * 1e3);

        let t0 = Instant::now();
        let mut t = Table::new(Arc::new(Base::open(&path, OpenOptions::default(), None).unwrap()), None);
        time(&format!("open ({} rows)", t.st.len()), t0);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let headers = t.st.headers();
        let col = headers.iter().position(|h| h == "country").unwrap_or(0);

        let t0 = Instant::now();
        let g = groups(&t.st, &rows, col, Match::Exact, &name, &no, None).unwrap();
        time(&format!("split: group by {} ({} values)", headers[col], g.len()), t0);
        let t0 = Instant::now();
        let g2 = groups(&t.st, &rows, col, Match::Slug, &name, &no, None).unwrap();
        time(&format!("split: group, slug ({} values)", g2.len()), t0);
        let t0 = Instant::now();
        let s = write(&t.st, &g, &out, &folder_name(&name, &headers[col], col), &no, None);
        assert!(s.error.is_none(), "{:?}", s.error);
        time(&format!("split: write {} files", s.written), t0);
        std::fs::remove_dir_all(s.folder.unwrap()).unwrap();
        // Many files: one per company.
        if let Some(c) = headers.iter().position(|h| h == "company") {
            let t0 = Instant::now();
            let g = groups(&t.st, &rows, c, Match::Exact, &name, &no, None).unwrap();
            time(&format!("split: group by company ({} values)", g.len()), t0);
            let t0 = Instant::now();
            let s = write(&t.st, &g, &out, "by company", &no, None);
            assert!(s.error.is_none(), "{:?}", s.error);
            time(&format!("split: write {} files", s.written), t0);
            std::fs::remove_dir_all(s.folder.unwrap()).unwrap();
        }

        let t0 = Instant::now();
        let src = combine::Source { name: name.clone(), st: t.st.clone(), rows: None };
        let srcs = [src.clone(), src];
        let cols = combine::columns(&srcs, true, false);
        let n = combine::write(&srcs, &cols, true, &combine::format(&srcs), &out.join("combined.csv"), &no, None).unwrap();
        time(&format!("combine: file twice + source column ({n} rows)"), t0);
        std::fs::remove_file(out.join("combined.csv")).unwrap();

        // Fill by example: a new column, two examples.
        let (fi, la) = (headers.iter().position(|h| h == "first_name").unwrap_or(1), headers.iter().position(|h| h == "last_name").unwrap_or(2));
        let target = t.st.ncols;
        t.insert_column(target, "login");
        let ex = |p: usize| {
            let id = t.st.data_id(p);
            let f = t.st.cell(id, fi).to_lowercase();
            (id, format!("{}.{}", &f[..1], t.st.cell(id, la).to_lowercase()))
        };
        let exs = vec![ex(0), ex(1)];
        let t0 = Instant::now();
        let rule = fill_example::infer(&exs, target, &t.st).unwrap();
        time(&format!("fill: infer {:?} → {}", exs[0].1, rule.describe(&t.st.headers())), t0);
        let t0 = Instant::now();
        let k = fill_example::count(&t.st, &rule, &rows, &no).unwrap();
        time(&format!("fill: count ({k} cells)"), t0);
        let t0 = Instant::now();
        let ch = fill_example::apply(&t.st, &rule, &rows, &no, None).unwrap();
        time(&format!("fill: apply ({} cells)", ch.len()), t0);
        let t0 = Instant::now();
        t.set_cells(&ch);
        time("fill: set_cells (one undo step)", t0);
        drop(ch);
        t.undo();
        t.undo();

        let t0 = Instant::now();
        let modes = anonymize::guess_all(&t.st);
        time(&format!("anonymize: guess {modes:?}"), t0);
        let t0 = Instant::now();
        let a = anonymize::anonymize(&t.st, &rows, &modes, &no, None).unwrap();
        time(&format!("anonymize: {} cells, {} kept", a.changes.len(), a.kept), t0);
        t.set_cells(&a.changes);
        drop(a);
        let t0 = Instant::now();
        let c = review::changes(&t.st, &no).unwrap();
        time(&format!("review after anonymize: {}", c.summary()), t0);
        drop(c);
        t.undo();

        let t0 = Instant::now();
        let b = Base::open(&path, OpenOptions { enc: None, delim: Some(0) }, None).unwrap();
        time(&format!("tail: re-take the file as raw text ({} lines)", b.n), t0);
        let mut f = tail::Follower::new();
        let t0 = Instant::now();
        assert!(matches!(f.step(&b, false).unwrap(), tail::Tail::Same));
        time("tail: a check with nothing new", t0);
        std::fs::remove_dir_all(&out).unwrap();
        let hwm = std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmHWM")).map(str::to_string));
        eprintln!("peak memory: {}", hwm.unwrap_or_default());
    }
}
