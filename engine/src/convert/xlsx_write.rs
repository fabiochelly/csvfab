//! A table as a formatted workbook (xlsx_writer.py): one sheet, Aptos
//! everywhere, a bold white-on-navy header row, frozen and filtered; columns
//! typed from their values (columns.rs) — numbers, percents and dates written
//! as such with their format, the rest as text; widths from the content.
//!
//! Two passes, both on every core: the columns are typed over every row first;
//! then the sheet's rows are written as XML in chunks side by side, each chunk
//! deflated on its own and ended on a byte boundary (a sync flush), so that the
//! chunks end to end are one deflate stream (pigz's way) and their CRCs combine.
//! The rest of the workbook (styles, the header row, the frozen pane, the filter,
//! the widths) is rust_xlsxwriter's, taken from a workbook it makes without the
//! rows; the zip around them is written here. rust_xlsxwriter streaming every cell
//! (`write_slow`) stays for a workbook whose layout this cannot read.

use super::columns::{ColKind, ColumnStat, Converter};
use super::{check, check_dir, replace, temp_beside, tick, Exported, Rows};
use crate::table::{RowId, State};
use rayon::prelude::*;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, FormatPattern, Workbook, Worksheet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

// Excel's hard limits.
pub const MAX_ROWS: usize = 1_048_576;
pub const MAX_COLS: usize = 16_384;
const MAX_CHARS: usize = 32_767;

const HEADER_FILL: u32 = 0x1E3A8A;
const HEADER_LINE: u32 = 0x3B82F6;

const SHEET: &str = "xl/worksheets/sheet1.xml";

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

/// What the typing pass saw: each column's stats, a bound on the sheet's XML, where its cells are.
struct Seen {
    cols: Vec<ColumnStat>,
    /// Bytes the rows' XML cannot exceed: past 4 GB the zip needs its 64-bit fields.
    bound: u64,
    /// Data rows (first, last) and columns (first, last) holding a cell.
    rows: Option<(usize, usize)>,
    used: Vec<bool>,
}

/// Whether a value makes a cell: anything but nothing, or control characters alone (`clean`).
#[inline]
fn makes_cell(v: &str) -> bool {
    v.bytes().any(|b| b >= 0x20 || matches!(b, b'\t' | b'\n' | b'\r'))
}

/// XML a cell adds besides its text's bytes, at most (`<c r="XFD1048576" t="inlineStr"><is><t
/// xml:space="preserve">…</t></is></c>`; a number's: its 24 characters included).
const CELL_XML: u64 = 88;

/// Bytes an escape adds to its byte (`&amp;`, `_x000D_`, `_x005F` before `_x`), at most.
static ESCAPE_GROWTH: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0;
    while b < 0x20 { t[b] = 6; b += 1; }
    t[b'&' as usize] = 4;
    t[b'<' as usize] = 3;
    t[b'>' as usize] = 3;
    t[b'_' as usize] = 6;
    t[0xEF] = 4;
    t
};

/// Types every column over `n` data rows, in parallel; the header widens its column
/// for the bold font and the filter button.
fn stats(st: &State, rows: &Rows, n: usize, ncols: usize, header: Option<&[String]>, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Seen, String> {
    const CHUNK: usize = 8192;
    struct Part { s: Vec<ColumnStat>, bound: u64, rows: Option<(usize, usize)>, used: Vec<bool> }
    let parts: Vec<Part> = (0..n.div_ceil(CHUNK))
        .into_par_iter()
        .map(|k| {
            let mut p = Part { s: vec![ColumnStat::default(); ncols], bound: 0, rows: None, used: vec![false; ncols] };
            if check(cancel).is_err() { return p; }
            let end = ((k + 1) * CHUNK).min(n);
            for pos in k * CHUNK..end {
                p.bound += 32;
                let mut any = false;
                st.visit(rows.id(pos), |c, v| {
                    let Some(x) = p.s.get_mut(c) else { return };
                    x.see(v);
                    if !v.is_empty() && makes_cell(v) {
                        p.bound += CELL_XML + v.len() as u64 + v.bytes().map(|b| ESCAPE_GROWTH[b as usize] as u64).sum::<u64>();
                        p.used[c] = true;
                        any = true;
                    }
                });
                if any { p.rows = Some((p.rows.map_or(pos, |r| r.0), pos)); }
            }
            tick(progress, end - k * CHUNK);
            p
        })
        .collect();
    check(cancel)?;
    let mut out = vec![ColumnStat::default(); ncols];
    let (mut bound, mut span, mut used) = (0, None::<(usize, usize)>, vec![false; ncols]);
    for p in &parts {
        for (o, s) in out.iter_mut().zip(&p.s) { o.merge(s); }
        bound += p.bound;
        if let Some((a, b)) = p.rows { span = Some((span.map_or(a, |s| s.0), b)); }
        used.iter_mut().zip(&p.used).for_each(|(u, &x)| *u |= x);
    }
    if let Some(h) = header {
        for (o, v) in out.iter_mut().zip(h) {
            o.width = o.width.max((v.chars().count() as f64 * 1.15) as usize + 3);
        }
    }
    Ok(Seen { cols: out, bound, rows: span, used })
}

/// The workbook as rust_xlsxwriter makes it: fonts, widths, the header row styled, frozen
/// and filtered over `written` rows. `dummy`: a row `first` with a number in every typed
/// column, in its format — for its style indices, taken out again (`Skeleton`).
#[allow(clippy::too_many_arguments)]
fn workbook(sheet: &str, dest: &Path, stats: &[ColumnStat], fmts: &[Option<Format>], base: &Format, header: Option<&[String]>, written: u64, dummy: bool) -> Result<(Workbook, u64), String> {
    let ncols = stats.len();
    let mut wb = Workbook::new();
    wb.set_default_format(base, 20, 64).map_err(err)?;
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
    for (c, s) in stats.iter().enumerate() {
        // rust_xlsxwriter writes pixels / 7 as the width: these are the bridge's widths, exactly.
        ws.set_column_width_pixels(c as u16, 7 * s.width.clamp(6, 58) as u32 + 14).map_err(err)?;
    }
    let mut cut_cells = 0u64;
    if let Some(h) = header {
        let hf = base.clone().set_bold().set_font_color(Color::White).set_pattern(FormatPattern::Solid)
            .set_background_color(Color::RGB(HEADER_FILL)).set_border_bottom(FormatBorder::Medium)
            .set_border_bottom_color(Color::RGB(HEADER_LINE)).set_align(FormatAlign::VerticalCenter);
        ws.set_row_height(0, 22).map_err(err)?;
        ws.set_freeze_panes(1, 0).map_err(err)?;
        if ncols > 0 {
            ws.autofilter(0, 0, (written - 1) as u32, (ncols - 1) as u16).map_err(err)?;
        }
        let mut cbuf = String::new();
        for (c, v) in h.iter().take(ncols).enumerate() {
            let mut cut = false;
            ws.write_string_with_format(0, c as u16, clean(v, &mut cbuf, &mut cut), &hf).map_err(err)?;
            cut_cells += cut as u64;
        }
    }
    if dummy {
        let r = header.is_some() as u32;
        ws.write_number(r, 0, 0.0).map_err(err)?;
        for (c, f) in fmts.iter().enumerate() {
            if let Some(f) = f { ws.write_number_with_format(r, c as u16, 0.0, f).map_err(err)?; }
        }
    }
    Ok((wb, cut_cells))
}

/// rust_xlsxwriter's sheet XML cut where the rows go, and what the rows need of it.
struct Skeleton {
    /// Up to `<sheetData>` (excluded), the dimension's ref left out at `dim`.
    head: String,
    dim: std::ops::Range<usize>,
    /// The header row's XML (empty without one).
    rows: String,
    /// From `</sheetData>` (excluded) to the end.
    tail: String,
    /// What a data row's `<row r="N"` carries before its `>`.
    row_attrs: String,
    /// Per column, its number format's style (0: General).
    xf: Vec<u32>,
}

/// Column letters → index (`A` → 0).
fn col_index(letters: &str) -> Option<usize> {
    let mut n = 0usize;
    for b in letters.bytes() {
        if !b.is_ascii_uppercase() { return None; }
        n = n * 26 + (b - b'A' + 1) as usize;
    }
    n.checked_sub(1)
}

fn col_name(mut c: usize, out: &mut Vec<u8>) {
    let mut tmp = [0u8; 4];
    let mut k = tmp.len();
    loop {
        k -= 1;
        tmp[k] = b'A' + (c % 26) as u8;
        if c < 26 { break; }
        c = c / 26 - 1;
    }
    out.extend_from_slice(&tmp[k..]);
}

impl Skeleton {
    /// None when the XML is not laid out as expected (another rust_xlsxwriter): then `write_slow`.
    fn read(xml: &str, ncols: usize, first: u32) -> Option<Skeleton> {
        let open = xml.find("<sheetData>")?;
        let close = xml.find("</sheetData>")?;
        let d = xml.find("<dimension ref=\"")? + "<dimension ref=\"".len();
        let dim = d..d + xml[d..].find('"')?;
        let inner = &xml[open + "<sheetData>".len()..close];
        // The dummy row: its attributes, its cells' styles.
        let tag = format!("<row r=\"{}\"", first + 1);
        let at = inner.find(&tag)?;
        let end = at + inner[at..].find("</row>")? + "</row>".len();
        let row = &inner[at..end];
        let gt = row.find('>')?;
        let row_attrs = row[tag.len()..gt].to_string();
        let mut xf = vec![0u32; ncols];
        for c in row[gt..].split("<c ").skip(1) {
            let r = c.strip_prefix("r=\"")?;
            let col = col_index(r.split(|ch: char| ch.is_ascii_digit()).next()?)?;
            let s = c.split_once(" s=\"").and_then(|(a, b)| (!a.contains('>')).then_some(b));
            if let (Some(x), Some(s)) = (xf.get_mut(col), s) { *x = s[..s.find('"')?].parse().ok()?; }
        }
        Some(Skeleton {
            head: xml[..open].to_string(),
            dim,
            rows: format!("{}{}", &inner[..at], &inner[end..]),
            tail: xml[close + "</sheetData>".len()..].to_string(),
            row_attrs,
            xf,
        })
    }

    /// The head with the dimension `first:last` (rows, columns) of the cells written.
    fn head(&self, span: Option<((u32, usize), (u32, usize))>) -> Vec<u8> {
        let mut r = Vec::new();
        match span {
            Some(((r0, c0), (r1, c1))) => {
                col_name(c0, &mut r);
                r.extend_from_slice(itoa::Buffer::new().format(r0 + 1).as_bytes());
                if (r0, c0) != (r1, c1) {
                    r.push(b':');
                    col_name(c1, &mut r);
                    r.extend_from_slice(itoa::Buffer::new().format(r1 + 1).as_bytes());
                }
            }
            None => r.extend_from_slice(b"A1"),
        }
        let mut out = self.head[..self.dim.start].as_bytes().to_vec();
        out.extend_from_slice(&r);
        out.extend_from_slice(self.head[self.dim.end..].as_bytes());
        out
    }
}

/// A cell's text as rust_xlsxwriter writes it: `&`, `<`, `>` as entities; control
/// characters, U+FFFE and U+FFFF as `_xHHHH_`; a `_xHHHH_` already there escaped
/// (`_x005F_xHHHH_`), so that Excel reads it back as written.
fn push_text(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    if !b.iter().any(|&x| ESCAPE_GROWTH[x as usize] != 0) {
        out.extend_from_slice(b);
        return;
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut k = 0;
    while k < b.len() {
        let x = b[k];
        match x {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'\t' | b'\n' => out.push(x),
            0..=0x1F => out.extend_from_slice(&[b'_', b'x', b'0', b'0', HEX[(x >> 4) as usize], HEX[(x & 15) as usize], b'_']),
            0xEF if b.get(k + 1) == Some(&0xBF) && matches!(b.get(k + 2), Some(0xBE | 0xBF)) => {
                out.extend_from_slice(if b[k + 2] == 0xBE { b"_xFFFE_" } else { b"_xFFFF_" });
                k += 3;
                continue;
            }
            b'_' if b.get(k + 1) == Some(&b'x') && b.len() >= k + 7 && b[k + 2..k + 6].iter().all(u8::is_ascii_hexdigit) && b[k + 6] == b'_' => {
                out.extend_from_slice(b"_x005F_");
            }
            _ => out.push(x),
        }
        k += 1;
    }
}

/// A number as `<v>` holds it: integers as such, else the shortest digits that read back.
fn push_number(out: &mut Vec<u8>, x: f64) {
    if x == x.trunc() && x.abs() < 1e15 && !(x == 0.0 && x.is_sign_negative()) {
        out.extend_from_slice(itoa::Buffer::new().format(x as i64).as_bytes());
    } else {
        out.extend_from_slice(zmij::Buffer::new().format_finite(x).as_bytes());
    }
}

/// One chunk of the sheet's rows: deflated (ended on a byte boundary), the CRC and length
/// of its XML, the cells it cut to Excel's limit.
#[derive(Default)]
struct Piece {
    z: Vec<u8>,
    crc: crc32fast::Hasher,
    len: u64,
    cut: u64,
}

/// `data` deflated at level 1 as a part of one stream: `last` ends it, else it ends with a
/// sync flush (on a byte boundary, the window forgotten) and the next part follows on.
fn deflate(data: &[u8], last: bool) -> Vec<u8> {
    use flate2::{Compress, Compression, FlushCompress, Status};
    let mut z = Compress::new(Compression::new(1), false);
    let mut out = Vec::with_capacity(data.len() / 4 + 64);
    let flush = if last { FlushCompress::Finish } else { FlushCompress::Sync };
    loop {
        let at = z.total_in() as usize;
        let status = z.compress_vec(&data[at..], &mut out, flush).expect("deflate");
        // Done once all is in and the output had room left (the flush complete), or the stream ended.
        if z.total_in() as usize == data.len() && if last { status == Status::StreamEnd } else { out.len() < out.capacity() } { break; }
        out.reserve(out.capacity().max(1 << 16));
    }
    out
}

fn crc_of(b: &[u8]) -> crc32fast::Hasher {
    let mut h = crc32fast::Hasher::new();
    h.update(b);
    h
}

/// A zip entry written: what the central directory says of it.
struct Entry {
    name: String,
    method: u16,
    crc: u32,
    csize: u64,
    size: u64,
    at: u64,
    zip64: bool,
}

/// MS-DOS date and time of 1980-01-01 00:00, as rust_xlsxwriter stamps its parts.
const DOS_DATE: u16 = 0x21;

fn local_header(w: &mut impl Write, e: &Entry) -> std::io::Result<()> {
    let (cs, sz) = if e.zip64 { (u32::MAX, u32::MAX) } else { (e.csize as u32, e.size as u32) };
    w.write_all(&0x04034b50u32.to_le_bytes())?;
    w.write_all(&(if e.zip64 { 45u16 } else { 20 }).to_le_bytes())?;
    w.write_all(&0u16.to_le_bytes())?;
    w.write_all(&e.method.to_le_bytes())?;
    w.write_all(&0u16.to_le_bytes())?;
    w.write_all(&DOS_DATE.to_le_bytes())?;
    w.write_all(&e.crc.to_le_bytes())?;
    w.write_all(&cs.to_le_bytes())?;
    w.write_all(&sz.to_le_bytes())?;
    w.write_all(&(e.name.len() as u16).to_le_bytes())?;
    w.write_all(&(if e.zip64 { 20u16 } else { 0 }).to_le_bytes())?;
    w.write_all(e.name.as_bytes())?;
    if e.zip64 {
        w.write_all(&1u16.to_le_bytes())?;
        w.write_all(&16u16.to_le_bytes())?;
        w.write_all(&e.size.to_le_bytes())?;
        w.write_all(&e.csize.to_le_bytes())?;
    }
    Ok(())
}

fn central(w: &mut impl Write, entries: &[Entry]) -> std::io::Result<u64> {
    let mut n = 0u64;
    for e in entries {
        let (cs, sz) = if e.zip64 { (u32::MAX, u32::MAX) } else { (e.csize as u32, e.size as u32) };
        let v: u16 = if e.zip64 { 45 } else { 20 };
        for x in [0x02014b50u32] { w.write_all(&x.to_le_bytes())?; }
        for x in [v, v, 0, e.method, 0, DOS_DATE] { w.write_all(&x.to_le_bytes())?; }
        for x in [e.crc, cs, sz] { w.write_all(&x.to_le_bytes())?; }
        for x in [e.name.len() as u16, if e.zip64 { 20 } else { 0 }, 0, 0, 0] { w.write_all(&x.to_le_bytes())?; }
        for x in [0u32, e.at as u32] { w.write_all(&x.to_le_bytes())?; }
        w.write_all(e.name.as_bytes())?;
        if e.zip64 {
            for x in [1u16, 16] { w.write_all(&x.to_le_bytes())?; }
            for x in [e.size, e.csize] { w.write_all(&x.to_le_bytes())?; }
        }
        n += 46 + e.name.len() as u64 + if e.zip64 { 20 } else { 0 };
    }
    Ok(n)
}

/// Writes the table to `dest` as a workbook, atomically. See [`super::export`].
pub fn write(st: &State, rows: Option<&[RowId]>, dest: &Path, sheet: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    write_with(st, rows, dest, sheet, progress, cancel, false)
}

/// `write`; `zip64`: the 64-bit zip fields whatever the size (tests).
fn write_with(st: &State, rows: Option<&[RowId]>, dest: &Path, sheet: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool, zip64: bool) -> Result<Exported, String> {
    check_dir(dest)?;
    let rows = Rows { st, ids: rows };
    let header = st.header.then(|| st.headers());
    let ncols = st.ncols.min(MAX_COLS);
    let room = MAX_ROWS - header.is_some() as usize;
    let n = rows.len().min(room);
    let written = (n + header.is_some() as usize) as u64;
    let mut seen = stats(st, &rows, n, ncols, header.as_deref(), progress, cancel)?;
    let kinds: Vec<(ColKind, Option<String>)> = seen.cols.iter_mut().map(|s| s.decide()).collect();
    let base = Format::new().set_font_name("Aptos").set_font_size(11).set_font_family(2);
    let fmts: Vec<Option<Format>> = kinds.iter().map(|(_, f)| f.as_ref().map(|f| base.clone().set_num_format(f))).collect();
    let (mut wb, cut_head) = workbook(sheet, dest, &seen.cols, &fmts, &base, header.as_deref(), written, true)?;
    let skel = wb.save_to_buffer().map_err(err)?;
    let parts = (|| -> Option<(Vec<(String, u16, u32, u64, Vec<u8>)>, Skeleton)> {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&skel)).ok()?;
        let mut sk = None;
        let mut parts = Vec::new();
        for i in 0..z.len() {
            let mut f = z.by_index_raw(i).ok()?;
            let name = f.name().ok()?.to_string();
            if name == SHEET {
                drop(f);
                let mut xml = String::new();
                z.by_index(i).ok()?.read_to_string(&mut xml).ok()?;
                sk = Some(Skeleton::read(&xml, ncols, header.is_some() as u32)?);
                continue;
            }
            let method = match f.compression() { zip::CompressionMethod::Stored => 0, zip::CompressionMethod::Deflated => 8, _ => return None };
            let (crc, size) = (f.crc32(), f.size());
            let mut raw = Vec::new();
            f.read_to_end(&mut raw).ok()?;
            parts.push((name, method, crc, size, raw));
        }
        Some((parts, sk?))
    })();
    let Some((parts, sk)) = parts else { return write_slow(st, rows.ids, dest, sheet, progress, cancel) };
    let tmp_path = temp_beside(dest);
    let res = (|| -> Result<u64, String> {
        let io = |e: std::io::Error| e.to_string();
        let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp_path).map_err(io)?);
        let mut entries = Vec::new();
        let mut at = 0u64;
        for (name, method, crc, size, raw) in parts {
            let e = Entry { name, method, crc, csize: raw.len() as u64, size, at, zip64: false };
            local_header(&mut w, &e).map_err(io)?;
            w.write_all(&raw).map_err(io)?;
            at += 30 + e.name.len() as u64 + e.csize;
            entries.push(e);
        }
        // The sheet, last: its sizes are known once written (put back in its header then).
        let zip64 = zip64 || seen.bound + (sk.head.len() + sk.rows.len() + sk.tail.len()) as u64 + 64 >= u32::MAX as u64;
        let mut e = Entry { name: SHEET.into(), method: 8, crc: 0, csize: 0, size: 0, at, zip64 };
        local_header(&mut w, &e).map_err(io)?;
        let data_at = at + 30 + SHEET.len() as u64 + if zip64 { 20 } else { 0 };
        let cut = write_sheet(st, &rows, n, &kinds, &seen, &sk, &mut w, &mut e, progress, cancel)?;
        e.csize = w.stream_position().map_err(io)? - data_at;
        w.seek(SeekFrom::Start(at)).map_err(io)?;
        local_header(&mut w, &e).map_err(io)?;
        w.seek(SeekFrom::Start(data_at + e.csize)).map_err(io)?;
        let cd_at = data_at + e.csize;
        entries.push(e);
        let cd_len = central(&mut w, &entries).map_err(io)?;
        let big = zip64 || cd_at + cd_len >= u32::MAX as u64;
        if big {
            // The zip64 end record and its locator.
            let end64 = cd_at + cd_len;
            w.write_all(&0x06064b50u32.to_le_bytes()).map_err(io)?;
            w.write_all(&44u64.to_le_bytes()).map_err(io)?;
            for x in [45u16, 45] { w.write_all(&x.to_le_bytes()).map_err(io)?; }
            for x in [0u32, 0] { w.write_all(&x.to_le_bytes()).map_err(io)?; }
            for x in [entries.len() as u64, entries.len() as u64, cd_len, cd_at] { w.write_all(&x.to_le_bytes()).map_err(io)?; }
            w.write_all(&0x07064b50u32.to_le_bytes()).map_err(io)?;
            w.write_all(&0u32.to_le_bytes()).map_err(io)?;
            w.write_all(&end64.to_le_bytes()).map_err(io)?;
            w.write_all(&1u32.to_le_bytes()).map_err(io)?;
        }
        w.write_all(&0x06054b50u32.to_le_bytes()).map_err(io)?;
        for x in [0u16, 0, entries.len() as u16, entries.len() as u16] { w.write_all(&x.to_le_bytes()).map_err(io)?; }
        let clip = |x: u64| if big { u32::MAX.min(x as u32) } else { x as u32 };
        for x in [clip(cd_len), if cd_at >= u32::MAX as u64 { u32::MAX } else { cd_at as u32 }] { w.write_all(&x.to_le_bytes()).map_err(io)?; }
        w.write_all(&0u16.to_le_bytes()).map_err(io)?;
        let f = w.into_inner().map_err(|e| e.into_error().to_string())?;
        f.sync_all().map_err(io)?;
        drop(f);
        replace(&tmp_path, dest, false).map_err(io)?;
        Ok(cut)
    })();
    match res {
        Ok(cut) => Ok(Exported {
            rows: written,
            cols: if written > 0 { ncols } else { 0 },
            truncated_rows: rows.len() > room,
            truncated_cols: st.ncols > MAX_COLS,
            truncated_cells: cut_head + cut,
            types: Vec::new(),
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

/// The sheet's XML deflated into `w`: rust_xlsxwriter's head (with the dimension of the cells
/// written) and header row, the data rows made in chunks on every core — a wave of chunks
/// made while the one before is written — and its tail. Sets the entry's CRC and size;
/// returns the cells cut to Excel's length limit.
#[allow(clippy::too_many_arguments)]
fn write_sheet<W: Write + Send>(st: &State, rows: &Rows, n: usize, kinds: &[(ColKind, Option<String>)], seen: &Seen, sk: &Skeleton, w: &mut W, e: &mut Entry, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<u64, String> {
    let ncols = kinds.len();
    let head_row = st.header as u32;
    let mut span = (st.header && ncols > 0).then_some(((0u32, 0usize), (0u32, ncols - 1)));
    if let Some((a, b)) = seen.rows {
        let (c0, c1) = (seen.used.iter().position(|&u| u).unwrap_or(0), seen.used.iter().rposition(|&u| u).unwrap_or(0));
        let (a, b) = (a as u32 + head_row, b as u32 + head_row);
        span = Some(match span {
            Some(((r0, x0), (r1, x1))) => ((r0.min(a), x0.min(c0)), (r1.max(b), x1.max(c1))),
            None => ((a, c0), (b, c1)),
        });
    }
    let mut out = sk.head(span);
    let (mut crc, mut size, mut cut) = (crc32fast::Hasher::new(), 0u64, 0u64);
    let mut put = |w: &mut W, raw: &[u8], z: &[u8], crc: &mut crc32fast::Hasher| -> Result<(), String> {
        crc.combine(&crc_of(raw));
        size += raw.len() as u64;
        w.write_all(z).map_err(|e| e.to_string())
    };
    if seen.rows.is_none() && sk.rows.is_empty() {
        out.extend_from_slice(b"<sheetData/>");
        out.extend_from_slice(sk.tail.as_bytes());
        put(w, &out, &deflate(&out, true), &mut crc)?;
        (e.crc, e.size) = (crc.finalize(), size);
        tick(progress, n);
        return Ok(0);
    }
    out.extend_from_slice(b"<sheetData>");
    out.extend_from_slice(sk.rows.as_bytes());
    put(w, &out, &deflate(&out, false), &mut crc)?;
    let conv = Converter::new();
    let threads = rayon::current_num_threads();
    let per = (n / (threads * 8)).clamp(1024, 8192);
    let chunks = n.div_ceil(per);
    // One chunk of rows: its XML deflated, its CRC.
    let make = |k: usize| -> Piece {
        let (a, b) = (k * per, ((k + 1) * per).min(n));
        if check(cancel).is_err() { return Piece::default(); }
        let mut x = Vec::with_capacity((b - a) * 64 * ncols.min(64));
        let (mut cbuf, mut cut) = (String::new(), 0u64);
        let mut num = itoa::Buffer::new();
        for pos in a..b {
            let r = num.format(head_row + pos as u32 + 1).as_bytes().to_vec();
            let start = x.len();
            x.extend_from_slice(b"<row r=\"");
            x.extend_from_slice(&r);
            x.push(b'"');
            x.extend_from_slice(sk.row_attrs.as_bytes());
            x.push(b'>');
            let open = x.len();
            // The column a cell without `r` is read at: the one after the last.
            let mut next = 0;
            st.visit(rows.id(pos), |c, v| {
                if c >= ncols || v.is_empty() { return; }
                let value = conv.value(kinds[c].0, v);
                let text = match value {
                    Some(_) => "",
                    None => {
                        let mut was_cut = false;
                        let s = clean(v, &mut cbuf, &mut was_cut);
                        if s.is_empty() { return; }
                        cut += was_cut as u64;
                        s
                    }
                };
                x.extend_from_slice(b"<c");
                if c != next {
                    x.extend_from_slice(b" r=\"");
                    col_name(c, &mut x);
                    x.extend_from_slice(&r);
                    x.push(b'"');
                }
                next = c + 1;
                match value {
                    Some(v) => {
                        if sk.xf[c] != 0 {
                            x.extend_from_slice(b" s=\"");
                            x.extend_from_slice(itoa::Buffer::new().format(sk.xf[c]).as_bytes());
                            x.push(b'"');
                        }
                        x.extend_from_slice(b"><v>");
                        push_number(&mut x, v);
                        x.extend_from_slice(b"</v></c>");
                    }
                    None => {
                        x.extend_from_slice(b" t=\"inlineStr\"><is><t");
                        if text.starts_with(['\t', '\n', ' ']) || text.ends_with(['\t', '\n', ' ']) { x.extend_from_slice(b" xml:space=\"preserve\""); }
                        x.push(b'>');
                        push_text(&mut x, text);
                        x.extend_from_slice(b"</t></is></c>");
                    }
                }
            });
            if x.len() == open { x.truncate(start) } else { x.extend_from_slice(b"</row>") }
        }
        Piece { z: deflate(&x, false), crc: crc_of(&x), len: x.len() as u64, cut }
    };
    let wave = threads * 2;
    let mut ready: Vec<Piece> = Vec::new();
    let flush = |w: &mut W, done: Vec<Piece>, crc: &mut crc32fast::Hasher, size: &mut u64, cut: &mut u64| -> Result<(), String> {
        for p in done {
            crc.combine(&p.crc);
            *size += p.len;
            *cut += p.cut;
            w.write_all(&p.z).map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    for from in (0..chunks).step_by(wave) {
        check(cancel)?;
        let to = (from + wave).min(chunks);
        let done = std::mem::take(&mut ready);
        let (wrote, made) = rayon::join(|| flush(w, done, &mut crc, &mut size, &mut cut), || (from..to).into_par_iter().map(make).collect::<Vec<_>>());
        wrote?;
        ready = made;
        tick(progress, (to * per).min(n) - from * per);
    }
    check(cancel)?;
    flush(w, ready, &mut crc, &mut size, &mut cut)?;
    let mut end = b"</sheetData>".to_vec();
    end.extend_from_slice(sk.tail.as_bytes());
    w.write_all(&deflate(&end, true)).map_err(|e| e.to_string())?;
    crc.combine(&crc_of(&end));
    (e.crc, e.size) = (crc.finalize(), size + end.len() as u64);
    Ok(cut)
}

/// The workbook written by rust_xlsxwriter cell by cell, streamed through a temporary file
/// (one core): for a workbook layout `Skeleton` cannot read.
fn write_slow(st: &State, rows: Option<&[RowId]>, dest: &Path, sheet: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    let rows = Rows { st, ids: rows };
    let header = st.header.then(|| st.headers());
    let ncols = st.ncols.min(MAX_COLS);
    let room = MAX_ROWS - header.is_some() as usize;
    let n = rows.len().min(room);
    let written = (n + header.is_some() as usize) as u64;
    let mut seen = stats(st, &rows, n, ncols, header.as_deref(), progress, cancel)?;
    let kinds: Vec<(ColKind, Option<String>)> = seen.cols.iter_mut().map(|s| s.decide()).collect();
    let base = Format::new().set_font_name("Aptos").set_font_size(11).set_font_family(2);
    let fmts: Vec<Option<Format>> = kinds.iter().map(|(_, f)| f.as_ref().map(|f| base.clone().set_num_format(f))).collect();
    let (mut wb, mut truncated_cells) = workbook(sheet, dest, &seen.cols, &fmts, &base, header.as_deref(), written, false)?;
    let ws: &mut Worksheet = wb.worksheet_from_index(0).map_err(err)?;
    let conv = Converter::new();
    let first = header.is_some() as u32;
    let mut cbuf = String::new();
    let mut failed: Option<String> = None;
    for pos in 0..n {
        if pos & 4095 == 4095 {
            check(cancel)?;
            tick(progress, 4096);
        }
        let r = first + pos as u32;
        st.visit(rows.id(pos), |c, v| {
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
        // Synced through a handle that may write: Windows refuses to flush a read-only one.
        std::fs::OpenOptions::new().write(true).open(&tmp_path).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
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

    /// Every cell of the sheet XML by its reference (`A2`), where it is written or, without
    /// `r`, where it falls (after the cell before it): (s attribute, t attribute, value, the
    /// element without its `r`).
    fn cells(xml: &str) -> std::collections::HashMap<String, (Option<String>, Option<String>, String, String)> {
        let mut out = std::collections::HashMap::new();
        let attr = |tag: &str, a: &str| tag.find(&format!(" {a}=\"")).map(|k| { let v = &tag[k + a.len() + 3..]; v[..v.find('"').unwrap()].to_string() });
        for row in xml.split("<row ").skip(1) {
            let r: u32 = attr(&format!(" {row}"), "r").unwrap().parse().unwrap();
            let mut col = 0;
            for c in row.split("<c").skip(1).filter(|c| c.starts_with([' ', '>'])) {
                let c = &c[..c.find("</c>").or_else(|| c.find("/>")).unwrap()];
                let tag = &c[..c.find('>').unwrap_or(c.len())];
                if let Some(at) = attr(tag, "r") {
                    col = at.trim_end_matches(|ch: char| ch.is_ascii_digit()).bytes().fold(0, |n, b| n * 26 + (b - b'A' + 1) as usize) - 1;
                }
                let val = if let Some(k) = c.find("<v>") { c[k + 3..].split('<').next().unwrap().to_string() } else if let Some(k) = c.find("<t") {
                    let v = &c[k..];
                    v[v.find('>').unwrap() + 1..].split("</t>").next().unwrap().to_string()
                } else { String::new() };
                let mut name = Vec::new();
                col_name(col, &mut name);
                let bare = match attr(tag, "r") { Some(at) => c.replacen(&format!(" r=\"{at}\""), "", 1), None => c.to_string() };
                out.insert(format!("{}{r}", String::from_utf8(name).unwrap()), (attr(tag, "s"), attr(tag, "t"), val, bare));
                col += 1;
            }
        }
        out
    }

    /// The cell `r` of the sheet XML: (s attribute, t attribute, value).
    fn cell(xml: &str, r: &str) -> (Option<String>, Option<String>, String) {
        let (s, t, v, _) = cells(xml).remove(r).unwrap_or_else(|| panic!("no {r}"));
        (s, t, v)
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

    /// What makes a style index: its `<xf>` with the number format's code in place of its id.
    fn style_key(styles: &str, s: Option<&str>) -> String {
        let s = s.unwrap_or("0");
        let xfs = &styles[styles.find("<cellXfs").unwrap()..];
        let xf = xfs.split("<xf ").nth(s.parse::<usize>().unwrap() + 1).unwrap();
        let xf = &xf[..xf.find("</xf>").or_else(|| xf.find("/>")).unwrap()];
        format!("{} {}", fmt_of(styles, s), xf.split_once("numFmtId=\"").map_or(xf.to_string(), |(a, b)| format!("{a}{}", &b[b.find('"').unwrap()..])))
    }

    /// The sheet as Excel would read it: every cell (its style resolved), the rows' and the
    /// sheet's own XML around them (the cells' `r` and style indices aside).
    fn reading(p: &Path) -> (Vec<(String, String)>, String, calamine::Range<calamine::Data>) {
        use calamine::Reader;
        let (xml, styles) = (part(p, "xl/worksheets/sheet1.xml"), part(p, "xl/styles.xml"));
        let mut cs: Vec<(String, String)> = cells(&xml).into_iter().map(|(r, (s, t, v, bare))| {
            let bare = match &s { Some(s) => bare.replacen(&format!(" s=\"{s}\""), "", 1), None => bare };
            (r, format!("{} | {t:?} | {v} | {bare}", style_key(&styles, s.as_deref())))
        }).collect();
        cs.sort();
        // Everything but the cells: rows, their attributes, the head and tail.
        let mut outer = String::new();
        let mut rest = &xml[..];
        while let Some(k) = rest.find("<c") {
            let after = &rest[k + 2..];
            if !after.starts_with([' ', '>']) { outer.push_str(&rest[..k + 2]); rest = after; continue; }
            outer.push_str(&rest[..k]);
            rest = &after[after.find("</c>").map(|e| e + 4).unwrap_or_else(|| after.find("/>").unwrap() + 2)..];
        }
        outer.push_str(rest);
        let mut wb: calamine::Xlsx<_> = calamine::open_workbook(p).unwrap();
        (cs, outer, wb.worksheet_range_at(0).unwrap().unwrap())
    }

    /// Cells of every kind, gaps between them, a column empty, rows empty: the sheet written
    /// in parallel chunks reads exactly as rust_xlsxwriter's, cell for cell, through calamine
    /// too — a header or not, some rows, columns moved and edited, the 64-bit zip fields.
    #[test]
    fn same_as_rust_xlsxwriter() {
        let dir = super::super::xlsx_read::tests::tmpdir("xw-same");
        let mut csv = String::from("Id;Montant;Quand;Pct;Texte;Vide;Code;Heure\n");
        let texts = ["a & b < c > d", "_x0041_ et _x00e9", " espace ", "tab\tici", "ligne\nsuite", "cr\rici", "ctl\u{1}\u{1f}x", "é ü 漢字 🎉", "\u{FFFE}\u{FFFF}", "", "\"q\"", "=1+1"];
        for i in 0..5000 {
            let t = texts[i % texts.len()].replace('"', "\"\"");
            let amount = if i % 13 == 0 { String::new() } else { format!("{},{:02}", i as i64 * 7 - 9000, i % 100) };
            let when = if i % 17 == 0 { "pas une date".into() } else { format!("{:02}/{:02}/20{:02} {:02}:{:02}", i % 28 + 1, i % 12 + 1, i % 30, i % 24, i % 60) };
            let row = format!("{i};{amount};{when};{},{}%;\"{t}\";;{:05};{}\n", i % 100, i % 10, i * 3, if i % 7 == 0 { "" } else { "1e5" });
            csv.push_str(if i % 997 == 5 { ";;;;;;;\n" } else { &row });
        }
        csv.push_str(&format!("long;;;;\"{}\";;;\n", "x".repeat(40_000)));
        let check = |t: &Table, rows: Option<&[RowId]>, name: &str| {
            let (a, b) = (dir.join(format!("{name}-fast.xlsx")), dir.join(format!("{name}-slow.xlsx")));
            let no = AtomicBool::new(false);
            let (pa, pb) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let ia = write(&t.st, rows, &a, "S", Some(&pa), &no).unwrap();
            let ib = write_slow(&t.st, rows, &b, "S", Some(&pb), &no).unwrap();
            assert_eq!(ia, ib, "{name}");
            assert_eq!(pa.into_inner(), pb.into_inner(), "{name}: progress");
            let (ra, rb) = (reading(&a), reading(&b));
            assert_eq!(ra.0.len(), rb.0.len(), "{name}: cells");
            for (x, y) in ra.0.iter().zip(&rb.0) { assert_eq!(x, y, "{name}"); }
            // rust_xlsxwriter repeats a header row left without data rows (`<row r="1" …/>` twice).
            assert_eq!(ra.1, rb.1.replace(r#"<row r="1" ht="21.75" customHeight="1"/>"#, ""), "{name}: the XML around the cells");
            assert_eq!(ra.2, rb.2, "{name}: calamine");
            for f in ["xl/workbook.xml", "[Content_Types].xml", "xl/_rels/workbook.xml.rels", "_rels/.rels", "docProps/app.xml"] {
                assert_eq!(part(&a, f), part(&b, f), "{name}: {f}");
            }
            // The 64-bit zip fields, as a sheet past 4 GB gets them: read the same.
            let c = dir.join(format!("{name}-64.xlsx"));
            write_with(&t.st, rows, &c, "S", None, &no, true).unwrap();
            assert_eq!(reading(&c).2, ra.2, "{name}: zip64");
            let xml = part(&a, SHEET);
            assert!(ia.rows < 10 || ["<c t=", "<c s=", "<c><v>"].iter().any(|t| xml.contains(t)), "{name}: cells without `r`");
        };
        let t = table(&csv, true);
        check(&t, None, "head");
        check(&table(&csv, false), None, "nohead");
        let ids: Vec<RowId> = (0..t.st.len()).rev().step_by(3).map(|p| t.st.data_id(p)).collect();
        check(&t, Some(&ids), "rows");
        let mut e = table(&csv, true);
        e.move_column(4, 0);
        e.insert_column(3, "Nouvelle");
        e.delete_columns(&[6]);
        let ch: Vec<_> = (0..e.st.len()).step_by(5).flat_map(|p| [(e.st.data_id(p), 3, format!("{p}%")), (e.st.data_id(p), 0, format!("<{p}>"))]).collect();
        e.set_cells(&ch);
        check(&e, None, "edited");
        check(&table("", false), None, "empty");
        check(&table("a;b\n", true), None, "header only");
        check(&table(";\n;\n", false), None, "blank");
    }
}
