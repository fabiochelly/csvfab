//! Writing a table back: untouched rows are copied as the bytes they were
//! (line breaks and empty lines included), so opening and saving a file
//! without changing it gives back the same bytes; an edited row is written in
//! the file's delimiter, encoding and line break. Saves are atomic: a
//! temporary file beside the target, synced, then renamed over it — after a
//! timestamped `.bak` of the original, the first time it is overwritten.

use crate::base::{Base, Stamp};
use crate::enc::{self, Enc};
use crate::scan::Offsets;
use crate::table::{RowId, State};
use rayon::prelude::*;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// None: as read.
    pub enc: Option<Enc>,
    pub delim: Option<u8>,
    pub bom: Option<bool>,
    /// Keep a timestamped copy of the file being replaced.
    pub backup: bool,
    /// Rows to write, in this order (ids); None: every row in file order.
    pub rows: Option<Vec<crate::table::RowId>>,
}

impl Default for SaveOptions {
    fn default() -> Self {
        SaveOptions { enc: None, delim: None, bom: None, backup: true, rows: None }
    }
}

#[derive(Debug)]
pub enum SaveError {
    /// The file changed on disk since it was read: (now, then).
    Changed,
    /// Another program cut the file short while it was open: the bytes past its new end
    /// are lost, writing would put zeros in their place.
    CutShort,
    Io(io::Error),
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> Self {
        SaveError::Io(e)
    }
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Changed => write!(f, "the file was changed by another program since it was opened"),
            SaveError::CutShort => write!(f, "another program cut the file short while it was open: part of what was read is lost, so it cannot be written back"),
            SaveError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Where a save's bytes go: `put` the bytes made here, `copy` a run of the
/// file's own bytes (`at`: its offset in `Base::bytes`) — which a file sink
/// leaves to the kernel.
trait Sink {
    fn put(&mut self, b: &[u8]) -> io::Result<()>;
    fn copy(&mut self, b: &[u8], _at: usize) -> io::Result<()> {
        self.put(b)
    }
}

/// Bytes kept in memory (UTF-8 for UTF-16 files) → bytes written.
struct Out<W: Write> {
    w: W,
    utf16: Option<bool>,
    pending: Vec<u8>,
}

impl<W: Write> Sink for Out<W> {
    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        match self.utf16 {
            None => self.w.write_all(b),
            Some(be) => {
                self.pending.extend_from_slice(b);
                if self.pending.len() >= 1 << 20 { self.flush_utf16(be, false)?; }
                Ok(())
            }
        }
    }
}

impl<W: Write> Out<W> {
    fn flush_utf16(&mut self, be: bool, all: bool) -> io::Result<()> {
        let cut = if all { self.pending.len() } else {
            let mut c = self.pending.len();
            while c > 0 && self.pending[c - 1] & 0xC0 == 0x80 { c -= 1; }
            if c > 0 && self.pending[c - 1] >= 0xC0 { c - 1 } else { self.pending.len() }
        };
        let s = String::from_utf8_lossy(&self.pending[..cut]);
        let mut o = Vec::with_capacity(cut * 2);
        enc::utf8_to_utf16(&s, be, &mut o);
        self.w.write_all(&o)?;
        self.pending.drain(..cut);
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        if let Some(be) = self.utf16 { self.flush_utf16(be, true)?; }
        self.w.flush()?;
        Ok(self.w)
    }
}

/// A file being written whose long runs of untouched rows the kernel copies
/// from the file read (`copy_file_range`): no trip through this process, and
/// on btrfs or XFS, where the offsets line up (all the rows before the first
/// edit, edits of the same length), shared blocks instead of copies.
struct FileSink {
    w: io::BufWriter<File>,
    /// The file read, and where its bytes (`Base::bytes`) start in it.
    src: Option<(File, u64)>,
}

/// Below this, a run is written as bytes: a system call per short run would cost more.
const KERNEL_COPY: usize = 1 << 20;

impl Sink for FileSink {
    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.w.write_all(b)
    }

    #[cfg(target_os = "linux")]
    fn copy(&mut self, b: &[u8], at: usize) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let Some((src, skip)) = &self.src else { return self.w.write_all(b) };
        if b.len() < KERNEL_COPY { return self.w.write_all(b); }
        self.w.flush()?;
        let (fd_in, fd_out) = (src.as_raw_fd(), self.w.get_ref().as_raw_fd());
        let mut off: libc::loff_t = (*skip + at as u64) as libc::loff_t;
        let mut done = 0usize;
        while done < b.len() {
            // SAFETY: two open descriptors; the output offset is the file's own position.
            let r = unsafe { libc::copy_file_range(fd_in, &mut off, fd_out, std::ptr::null_mut(), b.len() - done, 0) };
            if r > 0 { done += r as usize; continue; }
            let e = io::Error::last_os_error();
            if r < 0 && e.kind() == io::ErrorKind::Interrupted { continue; }
            // Not supported here (another file system, an old kernel): the bytes, written.
            self.src = None;
            return self.w.write_all(&b[done..]);
        }
        Ok(())
    }
}

/// One row's fields as a record (no line break): quoted when a field holds the
/// delimiter, a quote, a line break, or starts or ends with a space (or is alone and empty).
pub fn write_record(fields: &[String], d: u8, enc: Enc, lines: bool, out: &mut Vec<u8>) {
    if lines {
        enc::encode_into(enc, fields.first().map_or("", |s| s.as_str()), out);
        return;
    }
    // One empty field: quoted, or the record would be an empty line, read back as none.
    if fields.len() <= 1 && fields.first().is_none_or(|f| f.is_empty()) {
        out.extend_from_slice(b"\"\"");
        return;
    }
    for (i, f) in fields.iter().enumerate() {
        if i > 0 { out.push(d); }
        let needs = f.bytes().any(|b| b == d || b == b'"' || b == b'\n' || b == b'\r') || f.starts_with(' ') || f.ends_with(' ');
        if needs {
            out.push(b'"');
            enc::encode_into(enc, &f.replace('"', "\"\""), out);
            out.push(b'"');
        } else {
            enc::encode_into(enc, f, out);
        }
    }
}

/// Every row of the table, in file order, to `w`.
pub fn write_to<W: Write>(st: &State, o: &SaveOptions, w: W) -> io::Result<W> {
    let p = Plan::new(st, o);
    let mut out = Out { w, utf16: p.utf16, pending: Vec::new() };
    if p.bom {
        // The BOM is written raw: through `put` a UTF-16 one would be transcoded.
        out.w.write_all(p.enc.bom())?;
    }
    write_rows(st, o, &p, &mut out, None)?;
    out.finish()
}

/// How a save writes: decided once from the options and the file read.
struct Plan {
    enc: Enc,
    d: u8,
    lines: bool,
    bom: bool,
    utf16: Option<bool>,
    /// Records are copied as bytes (nothing about how they are written changed).
    same: bool,
    /// The encoding records are made in (the bytes kept in memory are UTF-8 for UTF-16).
    mem_enc: Enc,
}

impl Plan {
    fn new(st: &State, o: &SaveOptions) -> Plan {
        let b = &st.base;
        let enc = o.enc.unwrap_or(b.enc);
        let d = o.delim.unwrap_or(b.dialect.delim);
        let lines = b.dialect.lines && o.delim.is_none_or(|x| x == 0);
        let bom = o.bom.unwrap_or(if enc == b.enc { b.bom } else { enc.is_utf16() });
        let utf16 = match enc { Enc::Utf16Le => Some(false), Enc::Utf16Be => Some(true), _ => None };
        let same = enc.mem_utf8() == b.enc.mem_utf8() && (enc == b.enc || enc.mem_utf8()) && d == b.dialect.delim && lines == b.dialect.lines;
        let mem_enc = if enc.is_utf16() { Enc::Utf8 } else { enc };
        Plan { enc, d, lines, bom, utf16, same, mem_enc }
    }
}

/// The index of the file being written, built as it goes: reading the saved
/// file again then costs nothing (`save_indexed`). `ok` turns false at the
/// first row a scan of the file might see otherwise.
struct Index {
    pos: usize,
    starts: Offsets,
    width: Option<u32>,
    odd: Vec<(u32, u32)>,
    ok: bool,
}

impl Index {
    fn record(&mut self, start: usize, fields: u32) {
        self.starts.widen_for(start);
        self.starts.push(start as u64);
        let w = *self.width.get_or_insert(fields);
        if fields != w { self.odd.push(((self.starts.len() - 1) as u32, fields)); }
    }
}

/// Rows made here, encoded in parallel past this many in a row.
const PAR_ROWS: usize = 256;

fn write_rows(st: &State, o: &SaveOptions, p: &Plan, out: &mut impl Sink, mut ix: Option<&mut Index>) -> io::Result<()> {
    let b = &st.base;
    let bytes = b.bytes();
    if o.rows.is_none() {
        // The empty lines before the first record: with a line break chosen, theirs too (a LF
        // there would make a CR file read as LF).
        let pre = match st.eol {
            Some(e) => std::borrow::Cow::Owned(e.repeat(memchr::memchr_iter(b.dialect.nl, b.prefix()).count())),
            // A file of no record is all prefix, a last lone CR maybe: rows made here start after
            // its last line break, not glued to that CR.
            None if b.n == 0 && st.all_len() > 0 => std::borrow::Cow::Borrowed(&b.prefix()[..memchr::memrchr(b.dialect.nl, b.prefix()).map_or(0, |k| k + 1)]),
            None => std::borrow::Cow::Borrowed(b.prefix()),
        };
        out.put(&pre)?;
        if let Some(x) = ix.as_deref_mut() { x.pos += pre.len(); }
    }
    // A line break chosen for every record (`State::eol`): put between all of them, and at
    // the end where the file had one.
    let forced = st.eol;
    let default_eol = forced.unwrap_or(b.eol);
    // Rows any edit touched, sorted: everything else is the file's bytes (columns as read).
    let edited: Vec<RowId> = if st.cmap.is_none() && p.same {
        let mut v: Vec<RowId> = st.cols.iter().flat_map(|ls| ls.iter().flat_map(|l| l.ids().iter().copied())).collect();
        v.par_sort_unstable();
        v.dedup();
        v
    } else {
        Vec::new()
    };
    let head = (o.rows.is_some() && st.header && st.all_len() > 0).then(|| st.all_id(0));
    let count = match &o.rows { None => st.all_len(), Some(r) => head.is_some() as usize + r.len() };
    let id_at = |k: usize| -> RowId {
        match &o.rows {
            None => st.all_id(k),
            Some(r) => match head { Some(h) if k == 0 => h, Some(_) => r[k - 1], None => r[k] },
        }
    };
    // A line break chosen that ends on another byte than the file's lines (CR for a LF file, LF
    // for a CR one): a record holding that byte, a value's as read, is written anew (quoted) —
    // copied, it would be cut there once read again. Raw text has no quotes: copied whatever.
    let foreign = forced.and_then(|e| e.last().copied()).filter(|&x| x != b.dialect.nl && !p.lines);
    let copyable = |id: RowId| p.same && st.cmap.is_none() && (id as usize) < b.n && edited.binary_search(&id).is_err()
        && foreign.is_none_or(|x| memchr::memchr(x, &b.span(id as usize)[..own_len(b.span(id as usize), b.dialect)]).is_none())
        // A record of CRs alone (`\r\r\n`): its CR then the LF chosen would be an empty line.
        && !(forced.is_some() && !p.lines && b.record(id as usize).is_empty());
    // The file's last record runs to the end when a quote in it never closes: copied before other
    // rows it would take them in, once read again. Written anew then (its quotes closed).
    let open_last = b.qerr > 0 && b.n > 0 && !p.lines && {
        let mut t = b.span(b.n - 1).to_vec();
        t.extend_from_slice(&[b.dialect.nl, b'x', b.dialect.nl]);
        crate::scan::scan_range(&t, 0, t.len(), b.dialect, None, 3, None).0.starts.len() < 2
    };
    // A record put first (second, in a CR file) that the file did not start with can make a
    // reading take other line breaks: a lone CR in it in a LF file, a LF in it (or starting the
    // second) in a CR file. Written anew: quoted, they are values again.
    let out_nl = forced.and_then(|e| e.last().copied()).unwrap_or(b.dialect.nl);
    let own = |id: RowId| { let s = b.span(id as usize); &s[..own_len(s, b.dialect)] };
    // (The file's own start: its first records after the empty lines it began with, if any.)
    let as_read = o.rows.is_none() || b.prefix().is_empty();
    // The first row starts the file (no BOM, no empty line before it): a U+FEFF starting it
    // would be read as a BOM, and skipped.
    let bom_start = !p.bom && p.enc.mem_utf8() && (o.rows.is_some() || b.prefix().is_empty());
    let shifted = |k: usize, id: RowId| !p.lines && (k == 0 && bom_start && !(id == 0 && as_read) && own(id).starts_with(b"\xEF\xBB\xBF") || match (k, out_nl) {
        (0, b'\n') => !(id == 0 && as_read) && (own(id).windows(2).any(|w| w[0] == b'\r' && w[1] != b'\n') || own(id).ends_with(b"\r")),
        (0, _) => !(id == 0 && as_read) && memchr::memchr(b'\n', own(id)).is_some(),
        (1, b'\r') => !(id == 1 && id_at(0) == 0 && as_read) && own(id).starts_with(b"\n"),
        _ => false,
    });
    let copy_at = |k: usize| {
        let id = id_at(k);
        copyable(id) && !(open_last && id as usize == b.n - 1 && k + 1 < count) && !(k < 2 && shifted(k, id))
    };
    let field_count = |id: RowId| b.odd.binary_search_by_key(&id, |o| o.0).map_or(b.width as u32, |k| b.odd[k].1);
    // The line break after the previous row: its own (empty for a last line that had none).
    let mut owed: &[u8] = &[];
    let mut first = true;
    // The last row written was an empty line of raw text: it needs its line break to be one.
    let mut blank = false;
    let mut k = 0;
    while k < count {
        let id = id_at(k);
        if copy_at(k) {
            // A run of records following each other in the file, none edited: one copy.
            // Never past the file's records: rows made here follow on with consecutive ids too.
            let mut limit = edited[edited.partition_point(|&e| e <= id)..].first().map_or(b.n, |&e| (e as usize).min(b.n)) as RowId;
            if open_last && (id as usize) < b.n - 1 { limit = limit.min(b.n as RowId - 1); }
            let mut q = k + 1;
            if forced.is_some() {
                // One record at a time: the line breaks between them are the chosen one.
            } else if o.rows.is_none() && st.order.is_none() {
                q = (k + (limit - id) as usize).min(count);
            } else {
                while q < count && id_at(q) == id_at(q - 1) + 1 && id_at(q) < limit { q += 1; }
            }
            let last = id + (q - k - 1) as RowId;
            if !first {
                let sep = if forced.is_some() || !is_break(owed, b.dialect) { default_eol } else { owed };
                out.put(sep)?;
                if let Some(x) = ix.as_deref_mut() { x.pos += sep.len(); }
            }
            first = false;
            let a = b.starts.get(id as usize);
            let span = b.span(last as usize);
            let z = own_len(span, b.dialect);
            let e = b.starts.get(last as usize) + z;
            if let Some(x) = ix.as_deref_mut() {
                for r in id..=last {
                    x.record(x.pos + b.starts.get(r as usize) - a, field_count(r));
                }
                x.pos += e - a;
            }
            out.copy(&bytes[a..e], a)?;
            blank = false;
            owed = &span[z..];
            k = q;
            continue;
        }
        // Rows made here: as many in a row as there are, encoded in parallel.
        let mut q = k + 1;
        while q < count && q - k < 1 << 16 && !copy_at(q) { q += 1; }
        let ids: Vec<RowId> = (k..q).map(id_at).collect();
        // Each chunk's records end to end, and where each ends with its field count.
        type Encoded = (Vec<u8>, Vec<(usize, u32)>);
        let encode = |ids: &[RowId]| -> Encoded {
            let (mut buf, mut ends) = (Vec::with_capacity(ids.len() * 128), Vec::with_capacity(ids.len()));
            let mut row = Vec::new();
            for &id in ids {
                st.row(id, &mut row);
                // A row of empty cells: its delimiters (`""` for one), never an empty line, which
                // no scan reads as a record — the row would be gone once the file is read again.
                if !p.lines && row.iter().all(|f| f.is_empty()) { row.resize(st.ncols.max(1), String::new()); }
                write_record(&row, p.d, p.mem_enc, p.lines, &mut buf);
                let fields = if p.lines { 1 } else { row.len().max(1) as u32 };
                ends.push((buf.len(), fields));
            }
            (buf, ends)
        };
        let parts: Vec<Encoded> = if ids.len() >= PAR_ROWS {
            ids.par_chunks(1024).map(encode).collect()
        } else {
            vec![encode(&ids)]
        };
        let mut idx = ids.iter();
        for (buf, ends) in &parts {
            let mut from = 0;
            for &(end, fields) in ends {
                let id = *idx.next().unwrap();
                if !first {
                    let sep = if forced.is_some() || !is_break(owed, b.dialect) { default_eol } else { owed };
                    out.put(sep)?;
                    if let Some(x) = ix.as_deref_mut() { x.pos += sep.len(); }
                }
                let quoted;
                let rec = if first && bom_start && !p.lines && buf[from..end].starts_with(b"\xEF\xBB\xBF") {
                    // Unquoted (it starts with no quote): its first field quoted, not read as a BOM.
                    let r = &buf[from..end];
                    let d = memchr::memchr(p.d, r).unwrap_or(r.len());
                    quoted = [b"\"", &r[..d], b"\"", &r[d..]].concat();
                    &quoted[..]
                } else {
                    &buf[from..end]
                };
                first = false;
                let tail = if (id as usize) < b.n {
                    let span = b.span(id as usize);
                    &span[own_len(span, b.dialect)..]
                } else {
                    default_eol
                };
                if let Some(x) = ix.as_deref_mut() {
                    if x.ok && !reads_back(rec, tail, b.dialect, fields) { x.ok = false; }
                    x.record(x.pos, fields);
                    x.pos += rec.len();
                }
                out.put(rec)?;
                blank = rec.is_empty();
                owed = tail;
                from = end;
            }
        }
        k = q;
    }
    let owed = if (forced.is_some() && !owed.is_empty()) || (owed.is_empty() && blank) { default_eol } else { owed };
    out.put(owed)?;
    if let Some(x) = ix { x.pos += owed.len(); }
    Ok(())
}

/// The bytes of a record's span before its line break: its own, a `\r` there included (a raw
/// text line's last `\r` is its CRLF's, or a last line's: not the line's).
fn own_len(span: &[u8], d: crate::scan::Dialect) -> usize {
    let z = crate::scan::content_len(span, d);
    if d.lines { z } else { span.len() - clean_break(&span[z..], d).len() }
}

/// Whether a record's tail can separate it from the next record: it starts with a line break
/// (a last line's lone `\r` does not).
fn is_break(tail: &[u8], d: crate::scan::Dialect) -> bool {
    !tail.is_empty() && clean_break(tail, d).len() == tail.len()
}

/// The line breaks ending a record's tail (what follows its content in its span): its own and
/// the empty lines after it. Not the `\r` of `abc\r\r\n` nor a last line's lone one, nor, in a
/// CR file, the LF of a last `\r\n` (a value's, or a record once rows follow): after a
/// closing quote they would leave it open, or make a row. Empty when no line break is left.
fn clean_break(tail: &[u8], d: crate::scan::Dialect) -> &[u8] {
    let mut k = tail.len();
    while k > 0 && tail[k - 1] == d.nl {
        k -= 1;
        if d.nl == b'\n' && k > 0 && tail[k - 1] == b'\r' { k -= 1; }
    }
    &tail[k..]
}

/// Whether a scan of the saved file would see this record as written: one
/// record of `fields` fields, its line break and nothing else.
fn reads_back(rec: &[u8], tail: &[u8], d: crate::scan::Dialect, fields: u32) -> bool {
    // The usual record: no quote, no line break — its delimiters tell.
    if memchr::memchr3(b'"', b'\n', b'\r', rec).is_none() {
        let n = if d.lines { 1 } else { 1 + memchr::memchr_iter(d.delim, rec).count() as u32 };
        return (!rec.is_empty() || d.lines) && n == fields;
    }
    let mut b = Vec::with_capacity(rec.len() + tail.len());
    b.extend_from_slice(rec);
    b.extend_from_slice(tail);
    let (s, w) = crate::scan::scan_range(&b, 0, b.len(), d, None, usize::MAX, None);
    s.starts.len() == 1 && s.starts.get(0) == 0 && s.next == b.len() && s.qerr == 0 && w == fields
}

fn temp_beside(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.csvfab-{}.tmp", std::process::id()))
}

/// A dated copy `<name>.<stamp>.bak` beside `path` (local time, as the web's), never over an
/// existing one: two in the same second are `<stamp>-2.bak`, `-3`… A hard link where the file
/// system has them (the original's bytes, kept as they are when the save renames over it).
fn make_backup(path: &Path) -> io::Result<PathBuf> {
    let (y, mo, d, h, mi, s) = local_now();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = format!("{name}.{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}");
    for k in 1.. {
        let bak = path.with_file_name(if k == 1 { format!("{stem}.bak") } else { format!("{stem}-{k}.bak") });
        match std::fs::hard_link(path, &bak) {
            Ok(()) => return Ok(bak),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // No hard links here (FAT, some network shares): a copy, its name reserved first.
            Err(_) => match std::fs::OpenOptions::new().write(true).create_new(true).open(&bak) {
                Ok(_) => { std::fs::copy(path, &bak)?; return Ok(bak); }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            },
        }
    }
    unreachable!()
}

/// Puts `tmp` in the place of `path`, atomically where the system can. Windows refuses to
/// replace a file that is mapped — the one this table was read from, still open: the old
/// file is then renamed aside (allowed while mapped), the new one takes its name, and the
/// old one is deleted once nothing maps it (`mapguard::delete_later`).
fn replace(tmp: &Path, path: &Path) -> io::Result<()> {
    let e = match std::fs::rename(tmp, path) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if !cfg!(windows) || !path.exists() { return Err(e); }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let aside = path.with_file_name(format!(".{name}.csvfab-old-{}-{stamp}", std::process::id()));
    std::fs::rename(path, &aside).map_err(|_| e)?;
    if let Err(e) = std::fs::rename(tmp, path) {
        let _ = std::fs::rename(&aside, path);
        return Err(e);
    }
    crate::mapguard::delete_later(aside);
    Ok(())
}

/// Makes a rename in `dir` durable: the file's own fsync keeps its bytes, but the entry naming
/// it is only written with the folder's — a power cut right after could bring back the old
/// version. Refused by some file systems (network, FUSE): then as before. NTFS journals renames.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(if dir.as_os_str().is_empty() { Path::new(".") } else { dir }) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// The local date and time now (UTC where the zone cannot be read).
fn local_now() -> (i64, u32, u32, u32, u32, u32) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    #[cfg(unix)]
    {
        let t = now as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers are valid for the call; localtime_r is the thread-safe form.
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return (tm.tm_year as i64 + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32, tm.tm_hour as u32, tm.tm_min as u32, tm.tm_sec as u32);
        }
    }
    civil(now)
}

/// Seconds since 1970 → UTC date and time (Howard Hinnant's algorithm).
fn civil(t: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (t / 86400) as i64;
    let rem = t % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// Saves to `path`. `expect`: the stamp the file had when read, to refuse
/// overwriting someone else's changes (None: do not check). `backed_up`: a
/// `.bak` was already made this session. Returns the new stamp.
pub fn save(st: &State, path: &Path, o: &SaveOptions, expect: Option<Stamp>, backed_up: &mut bool) -> Result<Stamp, SaveError> {
    save_with(st, path, o, expect, backed_up, false).map(|r| r.0)
}

/// `save`, also returning the saved file as read again — mapped, its index
/// known from what was written, nothing re-read — when that can be told
/// for sure (the file's own encoding and delimiter, every row, no quote
/// error in the file read); None: open it again.
pub fn save_indexed(st: &State, path: &Path, o: &SaveOptions, expect: Option<Stamp>, backed_up: &mut bool) -> Result<(Stamp, Option<Base>), SaveError> {
    save_with(st, path, o, expect, backed_up, true)
}

fn save_with(st: &State, path: &Path, o: &SaveOptions, expect: Option<Stamp>, backed_up: &mut bool, reindex: bool) -> Result<(Stamp, Option<Base>), SaveError> {
    if let Some(e) = expect {
        if let Some(now) = Stamp::of(path) {
            if now != e { return Err(SaveError::Changed); }
        }
    }
    let b = &st.base;
    if b.cut_short() { return Err(SaveError::CutShort); }
    let p = Plan::new(st, o);
    let mut ix = (reindex && st.eol.is_none() && o.rows.is_none() && p.same && p.utf16.is_none() && p.enc == b.enc && b.qerr == 0 && b.n > 0 && !b.partial)
        .then(|| Index { pos: 0, starts: Offsets::U32(Vec::with_capacity(st.all_len() + 1)), width: None, odd: Vec::new(), ok: true });
    let tmp = temp_beside(path);
    let res = (|| -> io::Result<()> {
        let f = File::create(&tmp)?;
        let f = if p.utf16.is_some() {
            let w = write_to(st, o, BufWriter::with_capacity(1 << 20, f))?;
            w.into_inner().map_err(|e| e.into_error())?
        } else {
            let mut out = FileSink { w: BufWriter::with_capacity(1 << 20, f), src: source(b) };
            if p.bom { out.put(p.enc.bom())?; }
            write_rows(st, o, &p, &mut out, ix.as_mut())?;
            out.w.into_inner().map_err(|e| e.into_error())?
        };
        f.sync_all()?;
        // Cut short while being written: the zeros read in its place must not land.
        if b.cut_short() { return Err(io::Error::other("cut short")); }
        #[cfg(unix)]
        if let Ok(m) = std::fs::metadata(path) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m.permissions().mode()));
        }
        if o.backup && !*backed_up && path.exists() {
            make_backup(path)?;
            *backed_up = true;
        }
        replace(&tmp, path)?;
        sync_dir(path.parent().unwrap_or(Path::new(".")));
        Ok(())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(if b.cut_short() { SaveError::CutShort } else { e.into() });
    }
    let stamp = Stamp::of(path).ok_or_else(|| SaveError::Io(io::Error::other("saved file vanished")))?;
    let base = ix.filter(|x| x.ok).and_then(|mut x| {
        let n = x.starts.len();
        x.starts.widen_for(x.pos);
        x.starts.push(x.pos as u64);
        let skip = if p.bom { p.enc.bom().len() } else { 0 };
        Base::from_index(path, b, p.bom, skip, x.starts, n, x.width.unwrap_or(0) as usize, x.odd, stamp).ok().flatten()
    });
    Ok((stamp, base))
}

/// The file `b` was read from, if it is still the one mapped: the kernel copies from it.
fn source(b: &Base) -> Option<(File, u64)> {
    let path = b.path.as_ref()?;
    if !b.mapped() { return None; }
    let f = File::open(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = f.metadata().ok()?;
        if b.stamp.is_none_or(|s| s.ino != m.ino() || s.len != m.len()) { return None; }
    }
    Some((f, (b.size as usize - b.bytes().len()) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    fn roundtrip(bytes: &[u8]) -> Vec<u8> {
        let t = Table::new(Arc::new(Base::from_bytes(bytes.to_vec(), OpenOptions::default())), None);
        write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap()
    }

    /// Another program cuts the open file short: every cell still reads (zeros past the
    /// end, no crash), and no save writes those zeros, here or elsewhere.
    #[cfg(target_os = "linux")]
    #[test]
    fn cut_short_is_not_written() {
        let dir = std::env::temp_dir().join(format!("csvfab-cut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.csv");
        let body: String = std::iter::once("a,b\n".to_string()).chain((0..50_000).map(|i| format!("{i},row {i}\n"))).collect();
        std::fs::write(&path, &body).unwrap();
        let t = Table::new(Arc::new(Base::open(&path, OpenOptions::default(), None).unwrap()), None);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(100).unwrap();
        let st = &t.st;
        let n: usize = (0..st.base.n).map(|r| st.cell(r as crate::table::RowId, 1).len()).sum();
        assert!(n > 0);
        assert!(st.base.cut_short());
        let mut b = false;
        assert!(matches!(save(st, &path, &SaveOptions::default(), None, &mut b), Err(SaveError::CutShort)));
        let other = dir.join("u.csv");
        assert!(matches!(save(st, &other, &SaveOptions::default(), None, &mut b), Err(SaveError::CutShort)));
        assert!(!other.exists());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 100, "the file as the other program left it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untouched_is_byte_exact() {
        let corpus: &[&[u8]] = &[
            b"a,b\n1,2\n",
            b"a,b\r\n1,2",
            b"\n\na;b\r\n\r\n1;\"x\ny\"\n\n",
            b"\xEF\xBB\xBFa,b\n\xC3\xA9,2\n",
            b"nom;ville\nJos\xE9;N\xEEmes\r\n",
            b"a,b\n1,\"unclosed\n2,3\n",
            b"a,b\n1,2,3\n\0x,\"y\"z\n",
            b"just some text\nwith lines\n\n  and spaces  \n",
            b"a,b\r1,2\r",
        ];
        for c in corpus {
            assert_eq!(roundtrip(c), c.to_vec(), "{:?}", String::from_utf8_lossy(c));
        }
        let mut u16 = vec![0xFF, 0xFE];
        enc::utf8_to_utf16("a,b\r\nÉ,2\r\n", false, &mut u16);
        assert_eq!(roundtrip(&u16), u16);
    }

    #[test]
    fn edit_touches_only_its_line() {
        let src = b"a;b\r\n1;2\r\n3;4\r\n5;6";
        let mut t = Table::new(Arc::new(Base::from_bytes(src.to_vec(), OpenOptions::default())), None);
        let id = t.st.data_id(1);
        t.set_cells(&[(id, 1, "x;y \"z\"".into())]);
        let out = write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap();
        assert_eq!(out, b"a;b\r\n1;2\r\n3;\"x;y \"\"z\"\"\"\r\n5;6".to_vec());
        let ids = t.insert_rows(3, 1);
        t.set_cells(&[(ids[0], 0, "7".into())]);
        let out = write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap();
        assert_eq!(out, b"a;b\r\n1;2\r\n3;\"x;y \"\"z\"\"\"\r\n5;6\r\n7;\r\n".to_vec());
    }

    #[test]
    fn latin1_edit_stays_latin1() {
        let src = b"nom;ville\nJos\xE9;N\xEEmes\n";
        let mut t = Table::new(Arc::new(Base::from_bytes(src.to_vec(), OpenOptions::default())), None);
        let id = t.st.data_id(0);
        t.set_cells(&[(id, 0, "Zoé".into())]);
        let out = write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap();
        assert_eq!(out, b"nom;ville\nZo\xE9;N\xEEmes\n".to_vec());
    }

    #[test]
    fn convert_delimiter_and_encoding() {
        let src = b"a;b\n\xE9;2\n";
        let t = Table::new(Arc::new(Base::from_bytes(src.to_vec(), OpenOptions::default())), None);
        let o = SaveOptions { enc: Some(Enc::Utf8), delim: Some(b','), ..Default::default() };
        let out = write_to(&t.st, &o, Vec::new()).unwrap();
        assert_eq!(out, "a,b\né,2\n".as_bytes().to_vec());
    }

    #[test]
    fn atomic_save_with_backup() {
        let dir = std::env::temp_dir().join(format!("csvfab-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        std::fs::write(&p, b"a,b\n1,2\n").unwrap();
        let base = Arc::new(Base::open(&p, OpenOptions::default(), None).unwrap());
        let stamp = base.stamp;
        let mut t = Table::new(base, None);
        let id = t.st.data_id(0);
        t.set_cells(&[(id, 0, "9".into())]);
        let mut backed = false;
        save(&t.st, &p, &SaveOptions::default(), stamp, &mut backed).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"a,b\n9,2\n");
        assert!(backed);
        let baks: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".bak")).collect();
        assert_eq!(baks.len(), 1);
        // A second save with the old stamp: refused.
        assert!(matches!(save(&t.st, &p, &SaveOptions::default(), stamp, &mut backed), Err(SaveError::Changed)));
        // Saved over while mapped (Windows: the old file set aside): nothing left once let go.
        drop(t);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("csvfab-old")).collect();
        assert!(left.is_empty(), "{left:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A line break chosen for the file replaces every record's own, not the ones quoted inside values.
    #[test]
    fn chosen_line_break() {
        let base = Arc::new(Base::from_bytes(b"a,b\r\n1,\"x\r\ny\"\n2,3\r\n".to_vec(), OpenOptions::default()));
        let mut t = Table::new(base, None);
        t.set_eol(b"\n");
        assert_eq!(write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap(), b"a,b\n1,\"x\r\ny\"\n2,3\n");
        // Back to the file's own: each record's as read, byte for byte.
        t.set_eol(b"\r\n");
        assert_eq!(write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap(), b"a,b\r\n1,\"x\r\ny\"\n2,3\r\n");
        t.undo();
        assert_eq!(t.st.eol, Some(&b"\n"[..]));
    }

    /// Backups made in the same second never replace one another.
    #[test]
    fn backups_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("csvfab-bak-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        let mut made = Vec::new();
        for v in [&b"one"[..], b"two", b"three"] {
            std::fs::write(&p, v).unwrap();
            made.push(make_backup(&p).unwrap());
        }
        let got: Vec<Vec<u8>> = made.iter().map(|b| std::fs::read(b).unwrap()).collect();
        // Three files (the second-boundary may fall between them: then fewer suffixes, still three).
        assert_eq!(made.iter().collect::<std::collections::HashSet<_>>().len(), 3);
        // A hard link follows the file's later writes in place; the save renames over it instead.
        assert!(got.iter().all(|g| !g.is_empty()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The file a save writes, and the index it hands back, are what reading the file gives.
    #[test]
    fn saved_index_is_the_files() {
        let dir = std::env::temp_dir().join(format!("csvfab-save-ix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut big = b"\xEF\xBB\xBFid,name,note\r\n".to_vec();
        for i in 0..120_000 { big.extend_from_slice(format!("{i},n{i},\"a,\r\nb\"\r\n").as_bytes()); }
        big.extend_from_slice(b"1,2,3,4\r\n\r\n");
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("plain", b"a,b\n1,2\n3,4".to_vec()),
            ("blank lines", b"\n\na;b\r\n\r\n1;\"x\ny\"\n\n2;3\n".to_vec()),
            ("odd", b"a,b\n1,2,3\n4\n5,6\n".to_vec()),
            ("latin1", b"nom;ville\nJos\xE9;N\xEEmes\r\n".to_vec()),
            ("one column", b"a\n1\n2\n".to_vec()),
            ("text", b"just some text\nwith lines\n\n  and spaces  \n".to_vec()),
            ("big", big),
        ];
        type Edit = fn(&mut Table);
        let edits: Vec<(&str, Edit)> = vec![
            ("none", |_| {}),
            ("cell", |t| { let id = t.st.data_id(1.min(t.st.len() - 1)); t.set_cells(&[(id, 0, "x\"y".into())]); }),
            ("first row", |t| { let id = t.st.all_id(0); t.set_cells(&[(id, 3, "wider".into())]); }),
            ("empty one", |t| { let id = t.st.data_id(0); t.set_cells(&[(id, 0, String::new())]); }),
            ("insert+delete", |t| {
                let ids = t.insert_rows(1, 2);
                t.set_cells(&[(ids[0], 0, "new".into())]);
                let gone: std::collections::HashSet<RowId> = [t.st.data_id(0)].into();
                t.delete_rows(&gone);
            }),
            ("whole column", |t| {
                let ch: Vec<_> = (0..t.st.len()).map(|p| (t.st.data_id(p), 0, format!("v{p}"))).collect();
                t.set_cells(&ch);
            }),
        ];
        for (name, bytes) in &cases {
            for (what, edit) in &edits {
                let p = dir.join(format!("{name}.csv"));
                std::fs::write(&p, bytes).unwrap();
                let base = Arc::new(Base::open(&p, OpenOptions::default(), None).unwrap());
                let stamp = base.stamp;
                let mut t = Table::new(base, None);
                edit(&mut t);
                let want = write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap();
                let (_, got) = save_indexed(&t.st, &p, &SaveOptions { backup: false, ..Default::default() }, stamp, &mut false).unwrap();
                assert_eq!(std::fs::read(&p).unwrap(), want, "{name} / {what}: bytes");
                // Read again as it was read (a save keeps the file's encoding and delimiter).
                let d = &t.st.base.dialect;
                let read = Base::open(&p, OpenOptions { enc: Some(t.st.base.enc), delim: Some(if d.lines { 0 } else { d.delim }) }, None).unwrap();
                if let Some(g) = got {
                    let ctx = format!("{name} / {what}");
                    assert_eq!((g.n, g.width, &g.odd, g.bom, g.eol, g.dialect.delim, g.dialect.lines), (read.n, read.width, &read.odd, read.bom, read.eol, read.dialect.delim, read.dialect.lines), "{ctx}");
                    for i in 0..=g.n { assert_eq!(g.starts.get(i), read.starts.get(i), "{ctx}: record {i}"); }
                } else {
                    // An empty row is an empty line: read back, no record. Then the file is read again.
                    assert!(*what == "empty one" || *what == "insert+delete", "{name} / {what}: no index");
                }
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
