//! Combine files: several tables into one new file, columns matched by their
//! title (in any order; case, accents and punctuation ignored by default),
//! whatever each file's delimiter or encoding — each was decoded by its own.
//! The first source leads: its columns come first, and its delimiter, line
//! break and encoding are the result's (UTF-8 when the files do not share
//! one, since UTF-8 holds every character of all of them). Nothing read is
//! touched.

use crate::base::{Base, OpenOptions};
use crate::enc::{self, Enc};
use crate::split::{csv_ext, slugify, stem_of};
use crate::table::{RowId, State, Table};
use rayon::prelude::*;
use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// One table to combine: an open tab's snapshot, or a file read for this.
#[derive(Clone)]
pub struct Source {
    /// The file name, written in the source column.
    pub name: String,
    pub st: State,
    /// The rows it gives (a filtered tab's rows shown); None: every data row.
    pub rows: Option<Arc<Vec<RowId>>>,
}

impl Source {
    /// A file read with its own encoding and delimiter, detected.
    pub fn open(path: &Path) -> io::Result<Source> {
        let base = Base::open(path, OpenOptions::default(), None)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Source { name, st: Table::new(Arc::new(base), None).st, rows: None })
    }

    pub fn len(&self) -> usize {
        self.rows.as_ref().map_or(self.st.len(), |r| r.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    fn id(&self, k: usize) -> RowId {
        self.rows.as_ref().map_or_else(|| self.st.data_id(k), |r| r[k])
    }
}

/// Raw text (one line per row) has no columns to match.
pub fn combinable(st: &State) -> bool {
    !st.base.dialect.lines
}

/// A column of the result: its title, and its column in each source (None: left empty there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutCol {
    pub name: String,
    pub from: Vec<Option<usize>>,
}

/// The result's columns. Titles matched as written, or (`loose`) by their
/// slug; a title twice in one file matches its own rank in the others. A
/// file without a title line lines up by position, and so does everything
/// when the first one has none. `only_common`: the columns every file has.
pub fn columns(sources: &[Source], loose: bool, only_common: bool) -> Vec<OutCol> {
    let Some(first) = sources.first() else { return Vec::new() };
    let n = sources.len();
    let pos = !first.st.header;
    // Holes are possible: a file without titles places column c at index c,
    // past the columns found so far.
    let mut out: Vec<Option<OutCol>> = Vec::new();
    let mut by_key: HashMap<(String, usize), usize> = HashMap::new();
    for (k, x) in sources.iter().enumerate() {
        let mut rank: HashMap<String, usize> = HashMap::new();
        for (c, h) in x.st.headers().into_iter().enumerate() {
            if pos || !x.st.header {
                if out.len() <= c { out.resize(c + 1, None); }
                let o = out[c].get_or_insert_with(|| OutCol { name: if pos { (c + 1).to_string() } else { h.clone() }, from: vec![None; n] });
                o.from[k] = Some(c);
                continue;
            }
            let k0 = if loose { let s = slugify(&h); if s.is_empty() { h.clone() } else { s } } else { h.clone() };
            let r = rank.entry(k0.clone()).or_insert(0);
            *r += 1;
            let at = *by_key.entry((k0, *r)).or_insert_with(|| {
                out.push(Some(OutCol { name: h.clone(), from: vec![None; n] }));
                out.len() - 1
            });
            out[at].as_mut().unwrap().from[k] = Some(c);
        }
    }
    out.into_iter().flatten().filter(|o| !only_common || o.from.iter().all(Option::is_some)).collect()
}

/// How the result is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub delim: u8,
    pub eol: &'static [u8],
    pub enc: Enc,
    pub bom: bool,
    /// Whether a title line is written (the first file has one).
    pub header: bool,
}

/// The first file's delimiter, line break and encoding (BOM as it had) — or
/// UTF-8 when the files do not share an encoding.
pub fn format(sources: &[Source]) -> Format {
    let f = &sources[0].st;
    let b = &f.base;
    let same = sources.iter().all(|x| x.st.base.enc == b.enc && crate::save::new_file_enc(&x.st).is_none());
    let (enc, bom) = if same { (b.enc, b.bom) } else { (Enc::Utf8, b.bom && b.enc == Enc::Utf8) };
    Format { delim: b.dialect.delim, eol: b.eol, enc, bom, header: f.header }
}

/// `<stem> - combined<ext>` beside the first file, or (2), (3)… — a name not taken.
pub fn dest_path(first: &Path) -> Option<PathBuf> {
    let name = first.file_name()?.to_string_lossy().into_owned();
    let (stem, ext) = (stem_of(&name), csv_ext(&name));
    (1..1000)
        .map(|n| first.with_file_name(format!("{stem} - combined{}{ext}", if n > 1 { format!(" ({n})") } else { String::new() })))
        .find(|p| std::fs::symlink_metadata(p).is_err())
}

/// One field, quoted when it holds the delimiter, a quote, a line break or
/// starts or ends with a space (as a save writes a field).
#[inline]
fn put_field(s: &str, d: u8, enc: Enc, out: &mut Vec<u8>) {
    let needs = s.bytes().any(|b| b == d || b == b'"' || b == b'\n' || b == b'\r') || s.starts_with(' ') || s.ends_with(' ');
    if needs {
        out.push(b'"');
        enc::encode_into(enc, &s.replace('"', "\"\""), out);
        out.push(b'"');
    } else {
        enc::encode_into(enc, s, out);
    }
}

/// Bytes in the encoding kept in memory → bytes written (transcoded where UTF-8 is kept).
fn emit<W: Write>(w: &mut W, enc: Enc, b: &[u8]) -> io::Result<()> {
    if !enc.transcoded() {
        return w.write_all(b);
    }
    let mut o = Vec::with_capacity(b.len() * 2);
    enc::encode_file(enc, &String::from_utf8_lossy(b), &mut o);
    w.write_all(&o)
}

/// Writes the combined table to `w`: the title line (when the first file has
/// one), then every source's rows one after the other, serialised in
/// parallel slices written in order. `source_col`: a last column with each
/// row's file name. `progress` counts rows. Err(Interrupted) if cancelled.
pub fn write_to<W: Write>(sources: &[Source], cols: &[OutCol], source_col: bool, f: &Format, mut w: W, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> io::Result<W> {
    const CH: usize = 8192;
    const BATCH: usize = 64;
    let cancelled = || io::Error::new(io::ErrorKind::Interrupted, "cancelled");
    // Single-byte encodings are written as such; the others are built as UTF-8 then transcoded.
    let mem = if f.enc.transcoded() { Enc::Utf8 } else { f.enc };
    let d = f.delim;
    if f.bom {
        w.write_all(f.enc.bom())?;
    }
    if f.header {
        let mut src_name = "source file".to_string();
        while cols.iter().any(|o| o.name == src_name) { src_name.push(' '); }
        let mut line = Vec::new();
        for (i, name) in cols.iter().map(|o| o.name.as_str()).chain(source_col.then_some(src_name.as_str())).enumerate() {
            if i > 0 { line.push(d); }
            put_field(name, d, mem, &mut line);
        }
        line.extend_from_slice(f.eol);
        emit(&mut w, f.enc, &line)?;
    }
    for (k, x) in sources.iter().enumerate() {
        let at: Vec<Option<usize>> = cols.iter().map(|o| o.from[k]).collect();
        let mut tail = Vec::new();
        if source_col {
            tail.push(d);
            put_field(&x.name, d, mem, &mut tail);
        }
        let n = x.len();
        let mut start = 0;
        while start < n {
            if cancel.load(Ordering::Relaxed) { return Err(cancelled()); }
            let end = (start + CH * BATCH).min(n);
            let chunks: Vec<Vec<u8>> = (start..end)
                .into_par_iter()
                .step_by(CH)
                .map(|s0| {
                    let mut out = Vec::with_capacity(CH * 64);
                    let mut tmp = Vec::new();
                    for p in s0..(s0 + CH).min(end) {
                        let id = x.id(p);
                        let row = out.len();
                        for (i, c) in at.iter().enumerate() {
                            if i > 0 { out.push(d); }
                            if let Some(c) = *c {
                                x.st.with_cell(id, c, &mut tmp, |v| put_field(v, d, mem, &mut out));
                            }
                        }
                        out.extend_from_slice(&tail);
                        // One empty cell: `""`, not an empty line, which no reading takes as a row.
                        if out.len() == row { out.extend_from_slice(b"\"\""); }
                        out.extend_from_slice(f.eol);
                    }
                    out
                })
                .collect();
            for c in &chunks {
                emit(&mut w, f.enc, c)?;
            }
            if let Some(p) = progress { p.fetch_add(end - start, Ordering::Relaxed); }
            start = end;
        }
    }
    w.flush()?;
    Ok(w)
}

/// Writes the combined file to `path`: a temporary file beside it, renamed
/// over when complete. Returns the rows written.
pub fn write(sources: &[Source], cols: &[OutCol], source_col: bool, f: &Format, path: &Path, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> io::Result<usize> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.csvfab-{}.tmp", std::process::id()));
    let res = (|| {
        let file = std::fs::File::create(&tmp)?;
        let w = write_to(sources, cols, source_col, f, BufWriter::with_capacity(1 << 20, file), cancel, progress)?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() { let _ = std::fs::remove_file(&tmp); }
    res.map(|_| sources.iter().map(Source::len).sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(name: &str, bytes: &[u8]) -> Source {
        let base = Base::from_bytes(bytes.to_vec(), OpenOptions::default());
        Source { name: name.into(), st: Table::new(Arc::new(base), None).st, rows: None }
    }

    fn run(s: &[Source], loose: bool, common: bool, source: bool) -> (Vec<String>, Format, Vec<u8>) {
        let cols = columns(s, loose, common);
        let f = format(s);
        let no = AtomicBool::new(false);
        let out = write_to(s, &cols, source, &f, Vec::new(), &no, None).unwrap();
        (cols.into_iter().map(|o| o.name).collect(), f, out)
    }

    #[test]
    fn combine_by_title() {
        let a = src("clients-a.csv", "id;Nom;Ville\r\n1;Dupont;Paris\r\n2;Martin;\"Le Havre\"\r\n".as_bytes());
        let (b_bytes, _, _) = encoding_rs::WINDOWS_1252.encode("ville,nom,code\r\nLyon,Élodie,7\r\nNantes,\"Durand, fils\",8\r\n");
        let b = src("clients-b.csv", &b_bytes);
        assert_eq!(b.st.base.enc, Enc::Windows1252);
        let s = [a, b];
        let (cols, f, out) = run(&s, true, false, true);
        assert_eq!(cols, ["id", "Nom", "Ville", "code"]);
        assert_eq!(f.enc, Enc::Utf8);
        assert_eq!(String::from_utf8(out).unwrap(),
            "id;Nom;Ville;code;source file\r\n1;Dupont;Paris;;clients-a.csv\r\n2;Martin;Le Havre;;clients-a.csv\r\n;Élodie;Lyon;7;clients-b.csv\r\n;Durand, fils;Nantes;8;clients-b.csv\r\n");
        let (cols, _, out) = run(&s, true, true, false);
        assert_eq!(cols, ["Nom", "Ville"]);
        assert_eq!(String::from_utf8(out).unwrap(), "Nom;Ville\r\nDupont;Paris\r\nMartin;Le Havre\r\nÉlodie;Lyon\r\nDurand, fils;Nantes\r\n");
        let (cols, _, _) = run(&s, false, false, false);
        assert_eq!(cols, ["id", "Nom", "Ville", "ville", "nom", "code"]);
    }

    #[test]
    fn ranks_positions_and_encoding() {
        // A title twice matches its own rank; a filtered source gives its rows only.
        let a = src("a.csv", b"x,X,y\n1,2,3\n");
        let mut b = src("b.csv", b"y,x,X\n4,5,6\n7,8,9\n");
        b.rows = Some(Arc::new(vec![b.st.data_id(1)]));
        let (cols, f, out) = run(&[a, b], true, false, false);
        assert_eq!(cols, ["x", "X", "y"]);
        assert_eq!(f.enc, Enc::Utf8);
        assert_eq!(out, b"x,X,y\n1,2,3\n8,9,7\n");
        // No title line in the first file: by position.
        let a = src("a.csv", b"1,2\n3,4\n");
        assert!(!a.st.header);
        let b = src("b.csv", b"5,6,7\n");
        let (cols, _, out) = run(&[a, b], true, false, false);
        assert_eq!(cols, ["1", "2", "3"]);
        assert_eq!(out, b"1,2,\n3,4,\n5,6,7\n");
        // Shared single-byte encoding: kept, written in it.
        let a = src("a.csv", b"n;v\nJos\xE9;1\n");
        let b = src("b.csv", b"v;n\n2;N\xEEmes\n");
        let (_, f, out) = run(&[a, b], true, false, false);
        assert_eq!(f.enc, Enc::Windows1252);
        assert_eq!(out, b"n;v\nJos\xE9;1\nN\xEEmes;2\n");
    }

    #[test]
    fn dest_names() {
        let dir = std::env::temp_dir().join(format!("csvfab-combine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("a.csv");
        assert_eq!(dest_path(&first).unwrap(), dir.join("a - combined.csv"));
        std::fs::write(dir.join("a - combined.csv"), b"").unwrap();
        let p = dest_path(&first).unwrap();
        assert_eq!(p, dir.join("a - combined (2).csv"));
        let s = [src("a.csv", b"k,v\n1,2\n"), src("b.csv", b"v,k\n3,4\n")];
        let cols = columns(&s, true, false);
        let n = write(&s, &cols, false, &format(&s), &p, &AtomicBool::new(false), None).unwrap();
        assert_eq!(n, 2);
        assert_eq!(std::fs::read(&p).unwrap(), b"k,v\n1,2\n4,3\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
