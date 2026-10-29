//! A file as read: its bytes (memory-mapped, never copied — UTF-16 aside) and
//! where each record starts. Immutable once built, shared with background jobs.

use crate::enc::{self, Enc};
use crate::scan::{self, Dialect, Offsets};
use crate::mapguard::Guarded;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

/// Shared: a base read again with another encoding or delimiter, or grown by
/// a tail, keeps the same bytes (and the pages already mapped).
#[derive(Clone)]
pub enum Data {
    Map(Arc<Guarded>),
    Owned(Arc<Vec<u8>>),
}

impl std::ops::Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Map(m) => m,
            Data::Owned(v) => v,
        }
    }
}

/// What the file looked like on disk when read: a save checks nobody changed it meanwhile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub mtime: Option<SystemTime>,
    /// A save renames a new file over the old one: the inode tells even when
    /// the size and the clock tick are the same.
    pub ino: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Stamp> {
        let m = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let ino = std::os::unix::fs::MetadataExt::ino(&m);
        #[cfg(not(unix))]
        let ino = 0;
        Some(Stamp { len: m.len(), mtime: m.modified().ok(), ino })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenOptions {
    pub enc: Option<Enc>,
    /// The delimiter; Some(0): raw text, one line per row.
    pub delim: Option<u8>,
}

pub struct Base {
    pub path: Option<PathBuf>,
    data: Data,
    /// Where the data starts in `data` (after a BOM).
    skip: usize,
    pub enc: Enc,
    /// The bytes kept are known to be valid UTF-8 (checked whole by the scan, or transcoded
    /// from UTF-16): a field — cut on ASCII delimiters — is then text as it is, unchecked.
    valid_utf8: bool,
    pub bom: bool,
    pub dialect: Dialect,
    /// The line break new rows get: the file's first one.
    pub eol: &'static [u8],
    pub starts: Offsets,
    pub n: usize,
    pub width: usize,
    pub odd: Vec<(u32, u32)>,
    pub qerr: u32,
    pub size: u64,
    pub stamp: Option<Stamp>,
    pub load_time: std::time::Duration,
    /// Only the first records (`open_head`): the whole index is being built.
    pub partial: bool,
}

/// What `open_head` scans: enough rows for the first screens, the column
/// widths and kinds, read in a few milliseconds even from a cold disk.
const HEAD_BYTES: usize = 1 << 20;

impl Base {
    pub fn open(path: &Path, o: OpenOptions, progress: Option<&AtomicUsize>) -> std::io::Result<Base> {
        let t0 = Instant::now();
        let (data, stamp, size) = map(path)?;
        let mut b = Base::from_data(data, o, progress);
        b.path = Some(path.to_path_buf());
        b.stamp = stamp;
        b.size = size;
        b.load_time = t0.elapsed();
        Ok(b)
    }

    /// The first records only (about a megabyte), in a few milliseconds
    /// whatever the size: shown while `complete` indexes the rest. A small
    /// file is read whole: then `partial` is false and that is the base.
    /// Its encoding is the first megabyte's (a 1252 byte further down
    /// shows when the whole base comes).
    pub fn open_head(path: &Path, o: OpenOptions) -> std::io::Result<Base> {
        let t0 = Instant::now();
        let (data, stamp, size) = map(path)?;
        let mut b = Base::head(data, o);
        b.path = Some(path.to_path_buf());
        b.stamp = stamp;
        b.size = size;
        b.load_time = t0.elapsed();
        Ok(b)
    }

    /// The whole base of a head from `open_head`: the same mapping (its pages
    /// already in), every record indexed.
    pub fn complete(&self, o: OpenOptions, progress: Option<&AtomicUsize>) -> Base {
        let t0 = Instant::now();
        let mut b = Base::from_data(self.data.clone(), o, progress);
        b.path = self.path.clone();
        b.stamp = self.stamp;
        b.size = self.size;
        b.load_time = self.load_time + t0.elapsed();
        b
    }

    /// The same bytes read with other options (another encoding, another
    /// delimiter), without reading the file again: None when the file changed
    /// on disk since, or UTF-16 is involved (the bytes held are transcoded) —
    /// then `open` it. Only the encoding changing keeps the index as it is.
    pub fn reopen(&self, o: OpenOptions, progress: Option<&AtomicUsize>) -> Option<Base> {
        if self.partial || self.enc.is_utf16() || o.enc.is_some_and(|e| e.is_utf16()) {
            return None;
        }
        if let Some(p) = &self.path {
            if Stamp::of(p) != self.stamp { return None; }
        }
        let t0 = Instant::now();
        let same_split = match o.delim {
            None => true,
            Some(d) => d == if self.dialect.lines { 0 } else { self.dialect.delim },
        };
        let (sniffed, bom_len) = enc::sniff(&self.data);
        let enc = o.enc.unwrap_or(sniffed);
        let skip = if enc == sniffed { bom_len } else { 0 };
        let mut b = if same_split && o.enc.is_some() && skip == self.skip {
            Base { enc, valid_utf8: self.valid_utf8 && enc == self.enc, starts: self.starts.clone(), odd: self.odd.clone(), data: self.data.clone(), path: None, stamp: None, ..*self }
        } else {
            Base::from_data(self.data.clone(), o, progress)
        };
        b.path = self.path.clone();
        b.stamp = self.stamp;
        b.size = self.size;
        b.load_time = t0.elapsed();
        Some(b)
    }

    /// A raw-text file that grew (complete lines appended after a line
    /// break): its new lines indexed after the ones known, the old ones kept
    /// as they are. None when that does not apply — then `open` it again.
    pub fn append(&self) -> std::io::Result<Option<Base>> {
        let Some(path) = &self.path else { return Ok(None) };
        if self.partial || !self.dialect.lines || self.enc.is_utf16() || !matches!(self.data, Data::Map(_)) || self.skip > 0 {
            return Ok(None);
        }
        let old = self.bytes().len();
        if old > 0 && self.bytes()[old - 1] != self.dialect.nl {
            return Ok(None);
        }
        let t0 = Instant::now();
        let (data, stamp, size) = map(path)?;
        if (size as usize) < old || data[..old.min(4096)] != self.bytes()[..old.min(4096)] {
            return Ok(None);
        }
        let (s, _) = scan::scan_range(&data, old, data.len(), self.dialect, Some(1), usize::MAX, None);
        let mut starts = self.starts.clone();
        starts.truncate(self.n);
        starts.widen_for(data.len());
        for k in 0..s.starts.len() { starts.push(s.starts.get(k) as u64); }
        starts.push(data.len() as u64);
        Ok(Some(Base {
            data,
            n: self.n + s.starts.len(),
            starts,
            odd: Vec::new(),
            path: self.path.clone(),
            stamp,
            size,
            load_time: t0.elapsed(),
            valid_utf8: false,
            ..*self
        }))
    }

    pub fn from_bytes(bytes: Vec<u8>, o: OpenOptions) -> Base {
        let size = bytes.len() as u64;
        let mut b = Base::from_data(Data::Owned(Arc::new(bytes)), o, None);
        b.size = size;
        b
    }

    /// Sniffs the encoding (BOM, UTF-16), the BOM to skip, transcodes UTF-16.
    fn prepare(data: Data, o: OpenOptions) -> (Data, Enc, bool, usize) {
        let (sniffed, bom_len) = enc::sniff(&data);
        let enc = o.enc.unwrap_or(sniffed);
        // A BOM is only skipped when it belongs to the encoding used.
        let skip = if enc == sniffed || (enc.is_utf16() && sniffed.is_utf16()) { bom_len } else { 0 };
        let data = if enc.is_utf16() {
            Data::Owned(Arc::new(enc::transcode_utf16(&data[skip..], enc)))
        } else {
            data
        };
        let bom = skip > 0;
        let skip = if enc.is_utf16() { 0 } else { skip };
        (data, enc, bom, skip)
    }

    fn from_data(data: Data, o: OpenOptions, progress: Option<&AtomicUsize>) -> Base {
        let (data, mut enc, bom, skip) = Base::prepare(data, o);
        let bytes = &data[skip..];
        let (dialect, eol) = detect_dialect(bytes, o.delim);
        // UTF-8 unless a byte says otherwise: checked by the scan, in the same pass over the bytes.
        let check = o.enc.is_none() && enc == Enc::Utf8;
        let s = scan::scan_checked(bytes, dialect, progress, check);
        // A UTF-8 BOM decides (as the web app: no check then): a few bad bytes stay U+FFFD,
        // the BOM is written back.
        if check && !s.utf8 && !bom {
            enc = Enc::Windows1252;
        }
        let valid_utf8 = ((check && s.utf8) || enc.is_utf16()) && cuts_on_ascii(dialect);
        // The slices' offsets, freed on the scanning threads, would stay in their
        // allocator arenas: given back, the heap is the index alone.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        if bytes.len() > 64 << 20 {
            // SAFETY: glibc's malloc_trim only releases free memory.
            unsafe { libc::malloc_trim(0) };
        }
        Base {
            valid_utf8,
            path: None,
            skip,
            enc,
            bom,
            dialect,
            eol,
            n: s.n,
            width: s.width as usize,
            odd: s.odd,
            qerr: s.qerr,
            starts: s.starts,
            data,
            size: 0,
            stamp: None,
            load_time: Default::default(),
            partial: false,
        }
    }

    /// The records starting in the first `HEAD_BYTES`, one thread; the whole
    /// base when that is all there is.
    fn head(data: Data, o: OpenOptions) -> Base {
        if data.len() <= HEAD_BYTES || enc::sniff(&data).0.is_utf16() || o.enc.is_some_and(|e| e.is_utf16()) {
            return Base::from_data(data, o, None);
        }
        let (data, mut enc, bom, skip) = Base::prepare(data, o);
        let bytes = &data[skip..];
        let (dialect, eol) = detect_dialect(bytes, o.delim);
        let (mut s, width) = scan::scan_range(bytes, 0, HEAD_BYTES, dialect, None, usize::MAX, None);
        if s.next >= bytes.len() {
            return Base::from_data(data, o, None);          // one record ran to the end: all of it
        }
        let head_ok = std::str::from_utf8(&bytes[..s.next]).is_ok();
        if o.enc.is_none() && enc == Enc::Utf8 && !head_ok && !bom {
            enc = Enc::Windows1252;
        }
        let valid_utf8 = o.enc.is_none() && enc == Enc::Utf8 && head_ok && cuts_on_ascii(dialect);
        let n = s.starts.len();
        s.starts.push(s.next as u64);
        Base {
            valid_utf8,
            path: None,
            skip,
            enc,
            bom,
            dialect,
            eol,
            n,
            width: width as usize,
            odd: s.odd,
            qerr: s.qerr,
            starts: s.starts,
            data,
            size: 0,
            stamp: None,
            load_time: Default::default(),
            partial: true,
        }
    }

    /// The data, BOM excluded (UTF-8 for a UTF-16 file).
    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.data[self.skip..]
    }

    /// Record i with its line break and the empty lines after it.
    #[inline]
    pub fn span(&self, i: usize) -> &[u8] {
        &self.bytes()[self.starts.get(i)..self.starts.get(i + 1)]
    }

    /// Record i without its line break.
    #[inline]
    pub fn record(&self, i: usize) -> &[u8] {
        let s = self.span(i);
        &s[..scan::content_len(s, self.dialect)]
    }

    /// Bytes before the first record (empty lines a scan skipped).
    pub fn prefix(&self) -> &[u8] {
        if self.n == 0 { self.bytes() } else { &self.bytes()[..self.starts.get(0)] }
    }

    /// One field's bytes as text: borrowed unchecked when the whole file is known valid UTF-8.
    #[inline]
    pub fn text<'a>(&self, b: &'a [u8]) -> std::borrow::Cow<'a, str> {
        if self.valid_utf8 {
            // SAFETY: valid_utf8 means the whole data was checked (or produced) as UTF-8, and a
            // field is cut at ASCII bytes (delimiters, quotes, line breaks), which never fall
            // inside a multi-byte character; an unquoted `""` pair is removed byte-wise too.
            return std::borrow::Cow::Borrowed(unsafe { std::str::from_utf8_unchecked(b) });
        }
        enc::decode(self.enc, b)
    }

    /// Record i's fields, decoded.
    pub fn fields(&self, i: usize, out: &mut Vec<String>) {
        out.clear();
        let rec = self.record(i);
        if self.dialect.lines {
            out.push(self.text(rec).into_owned());
            return;
        }
        let mut tmp = Vec::new();
        scan::visit_fields(rec, self.dialect.delim, &mut tmp, |_, f| {
            out.push(self.text(f).into_owned());
            true
        });
    }

    /// Whether the bytes are the file's, mapped (not a copy in memory).
    pub fn mapped(&self) -> bool {
        matches!(self.data, Data::Map(_))
    }

    /// Whether another program cut the file short while it was open: the bytes past
    /// its new end read as zeros, so they must not be written anywhere as the file's.
    pub fn cut_short(&self) -> bool {
        matches!(&self.data, Data::Map(m) if m.cut_short())
    }

    /// A file just written whose index is known (`save::save_indexed`):
    /// mapped, nothing scanned. Its encoding and delimiter are `like`'s.
    /// None if the file is not what the index says (its size).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_index(path: &Path, like: &Base, bom: bool, skip: usize, starts: Offsets, n: usize, width: usize, odd: Vec<(u32, u32)>, stamp: Stamp) -> std::io::Result<Option<Base>> {
        let t0 = Instant::now();
        let (data, now, size) = map(path)?;
        // Written without BOM but starting as one (a first cell's U+FEFF): a reading skips it.
        let bom_like = !bom && !like.enc.bom().is_empty() && data.starts_with(like.enc.bom());
        if now != Some(stamp) || size as usize != skip + starts.get(n) || data[..skip] != *(if bom { like.enc.bom() } else { b"" }) || bom_like {
            return Ok(None);
        }
        let forced = if like.dialect.lines { 0 } else { like.dialect.delim };
        let (dialect, eol) = detect_dialect(&data[skip..], Some(forced));
        // Read again, the file would break its lines otherwise (raw text of CR lines holding a
        // LF, taken for CRLF): the index is not what a reading gives.
        if dialect.nl != like.dialect.nl { return Ok(None); }
        Ok(Some(Base {
            path: Some(path.to_path_buf()),
            data,
            skip,
            enc: like.enc,
            valid_utf8: like.valid_utf8,
            bom,
            dialect,
            eol,
            starts,
            n,
            width,
            odd,
            qerr: 0,
            size,
            stamp: Some(stamp),
            load_time: t0.elapsed(),
            partial: false,
        }))
    }

    /// Whether both hold the very same bytes (one map, one buffer).
    pub fn same_bytes(&self, other: &Base) -> bool {
        match (&self.data, &other.data) {
            (Data::Map(a), Data::Map(b)) => Arc::ptr_eq(a, b),
            (Data::Owned(a), Data::Owned(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Memory held besides the mapped file.
    pub fn heap_bytes(&self) -> usize {
        self.starts.bytes() + self.odd.capacity() * 8 + if let Data::Owned(v) = &self.data { v.capacity() } else { 0 }
    }
}

/// The file mapped (read-only), as it is now.
fn map(path: &Path) -> std::io::Result<(Data, Option<Stamp>, u64)> {
    let f = File::open(path)?;
    let stamp = Stamp::of(path);
    let size = f.metadata()?.len();
    let data = if size == 0 {
        Data::Owned(Arc::new(Vec::new()))
    } else {
        // Read-only; saves write a new file and rename it over this one, so the mapped
        // inode stays as it was. Another program cutting it short is caught (mapguard).
        let m = Guarded::map(&f)?;
        #[cfg(unix)]
        let _ = m.advise(memmap2::Advice::Sequential);
        // Read from a cold disk into large folios: a quarter to half the page faults.
        #[cfg(target_os = "linux")]
        let _ = m.advise(memmap2::Advice::HugePage);
        Data::Map(Arc::new(m))
    };
    Ok((data, stamp, size))
}

/// Whether fields are cut on ASCII bytes only: a delimiter forced to a byte above 0x7F would
/// cut UTF-8 characters in two, and the fields could not be taken as text unchecked.
fn cuts_on_ascii(d: Dialect) -> bool {
    d.lines || d.delim.is_ascii()
}

/// The delimiter among `;` `,` tab `|` whose count is the most regular over
/// the first lines (quotes respected), and the line break. A file where no
/// delimiter repeats is raw text: one line per row.
pub fn detect_dialect(b: &[u8], forced: Option<u8>) -> (Dialect, &'static [u8]) {
    const CANDS: [u8; 4] = [b';', b',', b'\t', b'|'];
    let head = &b[..b.len().min(256 << 10)];
    // The first line break, outside quotes when the delimiter is known (`delim`): a quoted one is
    // a value's — a CR file whose first title holds a `\n` is still a CR file. None outside them
    // (a stray quote): the first of all, as for raw text (no quotes).
    let breaks = |delim: Option<u8>| -> (u8, &'static [u8]) {
        let first = |x: u8, y: u8| {
            // A quote opens a quoted section only at a field's start (as the scan reads them):
            // a stray one in a title (`5" screen`) opens nothing.
            let (mut inq, mut at_start, mut closed) = (false, true, false);
            let k = delim.and_then(|d| head.iter().position(|&c| {
                let was_closed = std::mem::take(&mut closed);
                if c == b'"' {
                    if inq { inq = false; closed = true; } else if at_start || was_closed { inq = true; }
                } else if !inq && (c == x || c == y) {
                    return true;
                }
                at_start = !inq && (c == b'\n' || c == b'\r' || c == d);
                false
            }));
            // (Past the head when none is in it: a first record of more than 256 KB.)
            k.or_else(|| memchr::memchr2(x, y, b))
        };
        let nl = match first(b'\n', b'\r') {
            Some(k) if b[k] == b'\r' && b.get(k + 1) != Some(&b'\n') => b'\r',
            _ => b'\n',
        };
        let eol: &'static [u8] = match first(b'\n', b'\n') {
            _ if nl == b'\r' => b"\r",
            Some(k) if k > 0 && b[k - 1] == b'\r' => b"\r\n",
            _ => b"\n",
        };
        (nl, eol)
    };
    // The delimiter is looked for on the lines the first break of all tells.
    let (nl, eol) = breaks(None);
    if let Some(d) = forced {
        let (nl, eol) = if d == 0 { (nl, eol) } else { breaks(Some(d)) };
        return (Dialect { delim: d, nl, lines: d == 0 }, eol);
    }
    let mut counts: Vec<[u32; 4]> = Vec::new();
    let mut cur = [0u32; 4];
    let mut inq = false;
    let mut at_start = true;
    // The previous byte closed a quoted section: a quote right after it is `""`, an escaped quote.
    let mut closed = false;
    for &c in head {
        let was_closed = std::mem::take(&mut closed);
        if c == b'"' {
            // Only a quote opening a field opens a quoted section (Papa's rule, roughly).
            if inq { inq = false; closed = true; } else if at_start || was_closed { inq = true; }
        } else if !inq && c == nl {
            counts.push(cur);
            cur = [0; 4];
            if counts.len() >= 200 { break; }
        } else if !inq {
            if let Some(k) = CANDS.iter().position(|&d| d == c) { cur[k] += 1; }
        }
        at_start = !inq && (c == nl || CANDS.contains(&c));
    }
    if cur.iter().any(|&x| x > 0) && counts.len() < 200 { counts.push(cur); }
    let mut best: Option<(f64, u8)> = None;
    for (k, &d) in CANDS.iter().enumerate() {
        let lines = counts.len();
        if lines == 0 { break; }
        if counts[0][k] == 0 { continue; }
        // The count most lines share, not the first line's: a header with one more title
        // than its rows have fields (an export's last column, empty) is still a table.
        let mut seen: Vec<u32> = counts.iter().map(|c| c[k]).filter(|&n| n > 0).collect();
        seen.sort_unstable();
        let (typical, same) = seen.chunk_by(|a, b| a == b).map(|g| (g[0], g.len())).max_by_key(|&(n, len)| (len, n)).unwrap();
        if same * 2 < lines { continue; }    // a comma in prose, not a column separator
        let score = same as f64 / lines as f64 * 1000.0 + typical as f64;
        if best.is_none_or(|(s, _)| score > s) { best = Some((score, d)); }
    }
    let csv = |d: u8| { let (nl, eol) = breaks(Some(d)); (Dialect { delim: d, nl, lines: false }, eol) };
    match best {
        Some((_, d)) => csv(d),
        None if !looks_like_text(head, nl) => csv(b','),
        None => (Dialect { delim: 0, nl, lines: true }, eol),
    }
}

/// With no delimiter: short lines without spaces are a one-column table (a list
/// of codes, of e-mails); prose, code or logs are text.
fn looks_like_text(head: &[u8], nl: u8) -> bool {
    let lines: Vec<&[u8]> = head.split(|&b| b == nl).filter(|l| !l.is_empty()).take(200).collect();
    if lines.is_empty() { return false; }
    let avg = lines.iter().map(|l| l.len()).sum::<usize>() / lines.len();
    let spaced = lines.iter().filter(|l| l.contains(&b' ')).count();
    avg > 40 || spaced * 2 > lines.len()
}

/// Whether the first record is a header: every field filled, all different,
/// and none a number or a date.
pub fn guess_header(fields: &[String]) -> bool {
    if fields.is_empty() { return false; }
    let mut seen = std::collections::HashSet::new();
    fields.iter().all(|f| {
        let t = f.trim();
        !t.is_empty() && seen.insert(t) && crate::text::parse_number(t).is_none() && crate::text::parse_date(t).is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialects() {
        assert_eq!(detect_dialect(b"a;b;c\n1;2,5;3\n", None).0.delim, b';');
        assert_eq!(detect_dialect(b"a,b\r\n\"x;y\",2\r\n", None).0.delim, b',');
        assert_eq!(detect_dialect(b"a,b\r\n1,2\r\n", None).1, b"\r\n");
        assert_eq!(detect_dialect(b"a\tb\n1\t2\n", None).0.delim, b'\t');
        // Escaped quotes inside quoted fields, commas after them: still a wide CSV.
        let head: Vec<String> = (0..200).map(|i| format!("col{i}")).collect();
        let row: Vec<String> = (0..200).map(|i| if i % 3 == 0 { "\"a \"\"quoted\"\" word, with commas\"".to_string() } else { i.to_string() }).collect();
        let wide = format!("{}\n{}\n{}\n", head.join(","), row.join(","), row.join(","));
        let (d, _) = detect_dialect(wide.as_bytes(), None);
        assert!(!d.lines && d.delim == b',');
        assert_eq!(Base::from_bytes(wide.into_bytes(), OpenOptions::default()).width, 200);
        assert_eq!(detect_dialect(b"a;b\n\"x\"\";\"\";2\n\"\"\"y;z\";3\n", None).0.delim, b';');
        let text = "Lorem ipsum dolor sit amet\n".repeat(20);
        assert!(detect_dialect(text.as_bytes(), None).0.lines);
        // One more title than the rows have fields (the last column empty, its tabs dropped).
        let short = format!("Nom\tE-mail\tEnvoi\n{}", "Jean DUPONT\tjd@example.com\n".repeat(20));
        assert_eq!(detect_dialect(short.as_bytes(), None).0.delim, b'\t');
        // Prose whose first line happens to hold a comma stays text.
        let prose = format!("Dear all, hello\n{}", "Lorem ipsum dolor sit amet\n".repeat(20));
        assert!(detect_dialect(prose.as_bytes(), None).0.lines);
    }

    #[test]
    fn latin1_fallback() {
        let b = Base::from_bytes(b"nom;ville\nJos\xE9;N\xEEmes\n".to_vec(), OpenOptions::default());
        assert_eq!(b.enc, Enc::Windows1252);
        let mut f = Vec::new();
        b.fields(1, &mut f);
        assert_eq!(f, vec!["José", "Nîmes"]);
    }

    #[test]
    fn utf16() {
        let mut bytes = vec![0xFF, 0xFE];
        enc::utf8_to_utf16("a,b\r\nÉ,2\r\n", false, &mut bytes);
        let b = Base::from_bytes(bytes, OpenOptions::default());
        assert_eq!(b.enc, Enc::Utf16Le);
        assert!(b.bom);
        let mut f = Vec::new();
        b.fields(1, &mut f);
        assert_eq!(f, vec!["É", "2"]);
    }

    fn tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("csvfab-base-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn same(a: &Base, b: &Base) {
        assert_eq!((a.n, a.width, a.enc, a.qerr, &a.odd, a.skip, a.bom), (b.n, b.width, b.enc, b.qerr, &b.odd, b.skip, b.bom));
        for i in 0..=a.n { assert_eq!(a.starts.get(i), b.starts.get(i)); }
    }

    #[test]
    fn head_then_complete() {
      // With a UTF-8 BOM the file stays UTF-8 (the BOM decides): the bad byte is U+FFFD.
      for bom in [false, true] {
        let mut bytes = if bom { b"\xEF\xBB\xBFid,name,note\r\n".to_vec() } else { b"id,name,note\r\n".to_vec() };
        for i in 0..60_000 { bytes.extend_from_slice(format!("{i},caf\u{e9},\"a\r\nb\"\r\n").as_bytes()); }
        bytes.extend_from_slice(b"x,\xE9,1,2\r\n");       // a 1252 byte and an odd row, past the head
        let p = tmp("head.csv", &bytes);
        let full = Base::open(&p, OpenOptions::default(), None).unwrap();
        assert_eq!(full.enc, if bom { Enc::Utf8 } else { Enc::Windows1252 });
        let head = Base::open_head(&p, OpenOptions::default()).unwrap();
        assert!(head.partial && head.n > 1000 && head.n < full.n);
        assert_eq!(head.enc, Enc::Utf8);
        for i in 0..=head.n { assert_eq!(head.starts.get(i), full.starts.get(i)); }
        let done = head.complete(OpenOptions::default(), None);
        assert!(!done.partial);
        same(&done, &full);
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
      }
        // Small: the head is the whole file.
        let p2 = tmp("small.csv", b"a,b\n1,2\n");
        let h = Base::open_head(&p2, OpenOptions::default()).unwrap();
        assert!(!h.partial);
        assert_eq!(h.n, 2);
        std::fs::remove_dir_all(p2.parent().unwrap()).unwrap();
    }

    #[test]
    fn reopen_reuses_the_bytes() {
        let p = tmp("re.csv", b"\xEF\xBB\xBFa;b,c\n1;2,3\n\"x;y\";4\n");
        let b = Base::open(&p, OpenOptions::default(), None).unwrap();
        for o in [
            OpenOptions { enc: Some(Enc::Windows1252), delim: None },
            OpenOptions { enc: Some(Enc::Utf8), delim: None },
            OpenOptions { enc: None, delim: Some(b',') },
            OpenOptions { enc: Some(Enc::MacRoman), delim: Some(0) },
        ] {
            let r = b.reopen(o, None).unwrap();
            let want = Base::open(&p, o, None).unwrap();
            same(&r, &want);
            assert_eq!(r.dialect.delim, want.dialect.delim);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&p, b"changed\n").unwrap();
        assert!(b.reopen(OpenOptions::default(), None).is_none());
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }

    #[test]
    fn append_indexes_the_new_lines_only() {
        let p = tmp("log.txt", b"one\ntwo\n\n");
        let o = OpenOptions { enc: None, delim: Some(0) };
        let b = Base::open(&p, o, None).unwrap();
        std::fs::OpenOptions::new().append(true).open(&p).map(|mut f| std::io::Write::write_all(&mut f, b"three\nfour\n")).unwrap().unwrap();
        let a = b.append().unwrap().unwrap();
        same(&a, &Base::open(&p, o, None).unwrap());
        assert_eq!(a.record(4), b"four");
        let csv = Base::open(&p, OpenOptions::default(), None).unwrap();
        assert!(csv.dialect.lines || csv.append().unwrap().is_none());
        std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
    }
}
