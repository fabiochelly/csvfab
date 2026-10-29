//! Record scanner: finds where every record starts, in parallel slices.
//!
//! Quotes follow Papa Parse (as the JS version did): a field is quoted only if
//! its first byte is a quote; `""` inside is a quote; a closing quote must be
//! followed (spaces aside) by a delimiter or a line break, otherwise it is
//! kept as a character and counted as an error. An empty line is no record:
//! it stays in the previous record's span, so the spans put end to end give
//! back the file byte for byte. In `lines` mode (raw text) every line is a
//! record, empty ones included, and quotes mean nothing.
//!
//! Two scanners share the work: 64-byte blocks turned into bit masks (line
//! breaks, delimiters, quotes) take every record whose quoting is regular —
//! quotes opening fields, closing before a delimiter or a line break — and
//! the byte-by-byte one below takes any other record, exactly as written.
//! The UTF-8 check of the file runs in the same pass, a window ahead of the
//! scanner: the bytes are read from memory once.

use memchr::memchr;
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const Q: u8 = b'"';

#[derive(Clone, Copy, Debug)]
pub struct Dialect {
    pub delim: u8,
    /// The line-break byte: b'\n' (CRLF included) or b'\r' (old Mac files).
    pub nl: u8,
    pub lines: bool,
}

#[derive(Debug)]
pub struct Slice {
    pub starts: Offsets,
    /// Where the next record would start: the stitching checks it against the next slice's start.
    pub next: usize,
    /// (record index within the slice, field count) of records whose count differs from the width.
    pub odd: Vec<(u32, u32)>,
    pub qerr: u32,
    /// The bytes from the slice's start to `next` are valid UTF-8 (when checked).
    pub utf8: bool,
}

/// How far ahead of the scanner the UTF-8 check runs: the scanner then reads cached bytes.
const UTF8_AHEAD: usize = 64 << 10;

/// Scans the records starting in [from, stop) — the last one may run past
/// `stop`; `limit` caps how many. `width`: the expected field count (None:
/// the first record's).
pub fn scan_range(b: &[u8], from: usize, stop: usize, d: Dialect, width: Option<u32>, limit: usize, progress: Option<&AtomicUsize>) -> (Slice, u32) {
    scan_slice(b, from, stop, d, width, limit, progress, false)
}

/// `scan_range`, checking that the bytes scanned are UTF-8 if `utf8` (`from` must start a character).
#[allow(clippy::too_many_arguments)]
fn scan_slice(b: &[u8], from: usize, stop: usize, d: Dialect, width: Option<u32>, limit: usize, progress: Option<&AtomicUsize>, utf8: bool) -> (Slice, u32) {
    let len = b.len();
    let (dl, nl, lines) = (d.delim, d.nl, d.lines);
    let cap = ((stop - from.min(stop)) / 64).min(1 << 22).min(limit);
    let starts = if len > u32::MAX as usize { Offsets::U64(Vec::with_capacity(cap)) } else { Offsets::U32(Vec::with_capacity(cap)) };
    let mut s = Slice { starts, next: 0, odd: Vec::new(), qerr: 0, utf8: true };
    let mut width = width.map(|w| w as i64).unwrap_or(-1);
    let mut i = from;
    // Validated up to here (len: not checking, or found invalid).
    let mut checked = if utf8 { from } else { len };
    // Next line break / quote / delimiter at or after i (len: none left), usize::MAX: to look up.
    let (mut nn, mut nq, mut nd) = (usize::MAX, if lines { len } else { usize::MAX }, usize::MAX);
    let find = |x: u8, at: usize| memchr(x, &b[at..]).map_or(len, |k| at + k);
    let mut reported = from;
    while i < len && i < stop && s.starts.len() < limit {
        if checked < len && checked < i + UTF8_AHEAD / 2 {
            s.utf8 &= check_utf8(b, &mut checked, i + UTF8_AHEAD);
        }
        // As far as the bit masks can go (records starting before the checked bytes end).
        let soft = if checked < len { checked.min(stop) } else { stop };
        let j = blocks(b, i, soft, d, &mut width, limit, &mut s, progress, &mut reported);
        if j != i {
            i = j;
            continue;
        }
        let rs = i;
        let mut fields: u32 = 1;
        if nn == usize::MAX || nn < i { nn = find(nl, i); }
        if nq == usize::MAX || nq < i { nq = find(Q, i); }
        let eol = nn;
        if nq >= eol {
            // No quote on this line, the usual case: count the delimiters, done.
            if !lines {
                fields += b[i..eol].iter().filter(|&&x| x == dl).count() as u32;
            }
            i = eol;
        } else {
            loop {
                if b[i] == Q {
                    let mut j = i + 1;
                    loop {
                        let Some(off) = memchr(Q, &b[j..]) else {
                            i = len; s.qerr += 1; break;        // never closed: the rest of the file
                        };
                        let k = j + off;
                        if k + 1 < len && b[k + 1] == Q { j = k + 2; continue; }
                        let mut m = k + 1;
                        while m < len && ((b[m] == b' ' && dl != b' ') || (b[m] == b'\t' && dl != b'\t')) { m += 1; }
                        if m >= len || b[m] == dl || b[m] == nl || (nl == b'\n' && b[m] == b'\r' && m + 1 < len && b[m + 1] == b'\n') {
                            i = m; break;
                        }
                        s.qerr += 1; j = k + 1;                     // a stray quote: kept, search on
                    }
                } else {
                    if nn < i { nn = find(nl, i); }
                    if nd == usize::MAX || nd < i { nd = find(dl, i); }
                    i = nd.min(nn);
                }
                if i < len && b[i] == dl {
                    fields += 1; i += 1;
                    if i >= len { break; }
                    continue;
                }
                break;
            }
        }
        let e = i;
        if i < len {
            i += if b[i] == b'\r' && nl == b'\n' && i + 1 < len && b[i + 1] == b'\n' { 2 } else { 1 };
        }
        let mut z = e;
        if nl == b'\n' && z > rs && b[z - 1] == b'\r' { z -= 1; }
        if z == rs && !lines { continue; }
        push(&mut s, rs, fields, &mut width, progress, &mut reported, i);
    }
    if let Some(p) = progress { p.fetch_add(i.max(reported) - reported, Ordering::Relaxed); }
    if checked < len && checked < i {
        s.utf8 &= check_utf8(b, &mut checked, i);
    }
    s.next = i;
    (s, width.max(0) as u32)
}

#[inline(always)]
fn push(s: &mut Slice, rs: usize, fields: u32, width: &mut i64, progress: Option<&AtomicUsize>, reported: &mut usize, i: usize) {
    s.starts.push(rs as u64);
    if *width < 0 {
        *width = fields as i64;
    } else if fields as i64 != *width {
        s.odd.push(((s.starts.len() - 1) as u32, fields));
    }
    if let Some(p) = progress {
        if s.starts.len() & 0xFFFF == 0 {
            p.fetch_add(i - *reported, Ordering::Relaxed);
            *reported = i;
        }
    }
}

/// Validates `b[*checked..to]` (rounded to whole windows); false if invalid.
/// `*checked` stays on a character start, or becomes `b.len()` once invalid.
fn check_utf8(b: &[u8], checked: &mut usize, to: usize) -> bool {
    let len = b.len();
    while *checked < to.min(len) {
        let end = (*checked + UTF8_AHEAD).min(len);
        match std::str::from_utf8(&b[*checked..end]) {
            Ok(_) => *checked = end,
            // A character cut by the window's end: from its start next time.
            Err(e) if e.error_len().is_none() && end < len => *checked += e.valid_up_to(),
            Err(_) => {
                *checked = len;
                return false;
            }
        }
    }
    true
}

/// x with each bit replaced by the parity of the bits at and below it.
#[inline(always)]
fn prefix_xor(mut x: u64) -> u64 {
    x ^= x << 1;
    x ^= x << 2;
    x ^= x << 4;
    x ^= x << 8;
    x ^= x << 16;
    x ^= x << 32;
    x
}

/// Line breaks, delimiters, quotes and carriage returns of the 64 bytes at `p`.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn masks(b: &[u8], p: usize, nl: u8, dl: u8) -> (u64, u64, u64, u64) {
    use std::arch::x86_64::*;
    assert!(p + 64 <= b.len());
    // SAFETY: SSE2 is part of x86_64; the 64 bytes are in bounds (asserted).
    unsafe {
        let ptr = b.as_ptr().add(p);
        let (vn, vd, vq, vr) = (_mm_set1_epi8(nl as i8), _mm_set1_epi8(dl as i8), _mm_set1_epi8(Q as i8), _mm_set1_epi8(b'\r' as i8));
        let (mut n, mut d, mut q, mut r) = (0u64, 0u64, 0u64, 0u64);
        for k in 0..4 {
            let v = _mm_loadu_si128(ptr.add(16 * k) as *const __m128i);
            n |= (_mm_movemask_epi8(_mm_cmpeq_epi8(v, vn)) as u16 as u64) << (16 * k);
            d |= (_mm_movemask_epi8(_mm_cmpeq_epi8(v, vd)) as u16 as u64) << (16 * k);
            q |= (_mm_movemask_epi8(_mm_cmpeq_epi8(v, vq)) as u16 as u64) << (16 * k);
            r |= (_mm_movemask_epi8(_mm_cmpeq_epi8(v, vr)) as u16 as u64) << (16 * k);
        }
        (n, d, q, r)
    }
}

/// The bit-mask scanner: whole records from `i` (a record start) while their
/// quoting is regular, those starting before `stop`. Returns where it stopped:
/// the start of the record it could not take (or of the next record to scan).
#[cfg(target_arch = "x86_64")]
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn blocks(b: &[u8], i: usize, stop: usize, d: Dialect, width: &mut i64, limit: usize, s: &mut Slice, progress: Option<&AtomicUsize>, reported: &mut usize) -> usize {
    #[cfg(test)]
    if tests::SCALAR_ONLY.with(|c| c.get()) { return i; }
    let len = b.len();
    let (nl, dl, lines) = (d.nl, if d.lines { d.nl } else { d.delim }, d.lines);
    let mut rs = i;
    let mut fields: u32 = 1;
    let mut blk = i;
    // In a quoted section at the block's start (all ones) or not.
    let mut inq = 0u64;
    // The byte before the block may precede an opening quote; a quote closed on the block's last bit.
    let mut prev_ok = 1u64;
    let mut closer_pending = false;
    while blk + 64 <= len && rs < stop && s.starts.len() < limit {
        let (n, dm, q, r) = masks(b, blk, nl, dl);
        let (dm, q) = if lines { (0, 0) } else { (dm, q) };
        // Bits past `cut` cannot be trusted (irregular quoting there).
        let mut cut = 64u32;
        let (ns, ds) = if q == 0 && inq == 0 && !closer_pending {
            (n, dm)
        } else {
            let after = prefix_xor(q) ^ inq; // inside a quoted section after each byte
            let before = after ^ q;
            let openers = q & !before;
            let closers = q & before;
            // A quote opens a field only right after a delimiter, a line break, or a closing quote ("").
            let bad_open = openers & !(((dm | n | q) << 1) | prev_ok);
            // A closing quote is followed by a quote (""), a delimiter, or a line break (CRLF included).
            let crlf = if nl == b'\n' { r & (n >> 1) } else { 0 };
            let allowed = q | dm | n | crlf;
            let bad_close = ((closers << 1) & !allowed) >> 1 | (if closer_pending && allowed & 1 == 0 { 1 } else { 0 });
            let bad = bad_open | bad_close;
            if bad != 0 { cut = bad.trailing_zeros(); }
            closer_pending = closers >> 63 != 0;
            inq = if after >> 63 != 0 { !0 } else { 0 };
            (n & !after, dm & !after)
        };
        let below_cut = if cut == 64 { !0 } else { (1u64 << cut) - 1 };
        let mut nls = ns & below_cut;
        let mut dels = ds;
        while nls != 0 {
            let p = nls.trailing_zeros();
            nls &= nls - 1;
            let below = (1u64 << p) - 1;
            fields += (dels & below).count_ones();
            dels &= !below;
            let e = blk + p as usize;
            let next = e + 1;
            let mut z = e;
            if nl == b'\n' && z > rs && b[z - 1] == b'\r' { z -= 1; }
            if z > rs || lines {
                push(s, rs, fields, width, progress, reported, next);
                if s.starts.len() >= limit { return next; }
            }
            rs = next;
            fields = 1;
            if rs >= stop { return rs; }
        }
        if cut < 64 { return rs; }
        fields += dels.count_ones();
        prev_ok = ((dm | n | q) >> 63) & 1;
        blk += 64;
    }
    rs
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(clippy::too_many_arguments)]
fn blocks(_: &[u8], i: usize, _: usize, _: Dialect, _: &mut i64, _: usize, _: &mut Slice, _: Option<&AtomicUsize>, _: &mut usize) -> usize {
    i
}

pub struct Scan {
    /// Start of every record, plus the end of the data: n + 1 entries.
    pub starts: Offsets,
    pub n: usize,
    pub width: u32,
    pub odd: Vec<(u32, u32)>,
    pub qerr: u32,
    /// The data is valid UTF-8 (checked only when asked).
    pub utf8: bool,
}

/// The whole buffer, in parallel: slices cut after a line break, each scanned
/// as if a record started there; a slice whose predecessor did not end
/// exactly at its start (a quoted line break across the cut) is rescanned
/// from where the predecessor really ended.
pub fn scan(b: &[u8], d: Dialect, progress: Option<&AtomicUsize>) -> Scan {
    scan_checked(b, d, progress, false)
}

/// `scan`, also telling whether the data is valid UTF-8 (`utf8`), in the same pass.
pub fn scan_checked(b: &[u8], d: Dialect, progress: Option<&AtomicUsize>, utf8: bool) -> Scan {
    let len = b.len();
    let (_, width) = scan_range(b, 0, len, d, None, 1, None);
    let threads = rayon::current_num_threads().max(1);
    // A few slices per thread: an even spread of the work; not too small, the
    // disk's readahead (cold) stays sequential within each.
    let chunk = (len / (threads * 4)).clamp(1 << 20, 32 << 20);
    let mut cuts = vec![0usize];
    let mut p = chunk;
    while p < len {
        let c = memchr(d.nl, &b[p..]).map_or(len, |k| p + k + 1);
        if c >= len { break; }
        cuts.push(c);
        p = c + chunk;
    }
    cuts.push(len);
    let mut slices: Vec<Slice> = cuts.par_windows(2).map(|w| scan_slice(b, w[0], w[1], d, Some(width), usize::MAX, progress, utf8).0).collect();
    let valid = slices.iter().all(|s| s.utf8);

    // Stitching: a slice that did not start where its predecessor ended is rescanned (or dropped).
    let (mut odd, mut qerr, mut expected) = (Vec::new(), 0u32, 0usize);
    let mut at = Vec::with_capacity(slices.len());
    let mut total = 0usize;
    for (k, s) in slices.iter_mut().enumerate() {
        let stop = cuts[k + 1];
        if cuts[k] != expected {
            if expected >= stop {
                s.starts = Offsets::U32(Vec::new());     // swallowed whole by a record running across it
                at.push(total);
                continue;
            }
            *s = scan_range(b, expected, stop, d, Some(width), usize::MAX, None).0;
        }
        odd.extend(s.odd.iter().map(|&(r, f)| (r + total as u32, f)));
        qerr += s.qerr;
        expected = s.next;
        at.push(total);
        total += s.starts.len();
    }
    // One array, filled in parallel: each slice's offsets at their place.
    let starts = if len > u32::MAX as usize {
        let mut v = vec![0u64; total + 1];
        fill(&mut v, &slices, &at, |o, out| if let Offsets::U64(x) = o { out.copy_from_slice(x) });
        v[total] = len as u64;
        Offsets::U64(v)
    } else {
        // Zeroed pages come from the system as they are: no pass to clear them.
        let mut v = vec![0u32; total + 1];
        fill(&mut v, &slices, &at, |o, out| if let Offsets::U32(x) = o { out.copy_from_slice(x) });
        v[total] = len as u32;
        Offsets::U32(v)
    };
    Scan { starts, n: total, width, odd, qerr, utf8: valid }
}

fn fill<T: Send>(v: &mut [T], slices: &[Slice], at: &[usize], copy: impl Fn(&Offsets, &mut [T]) + Sync) {
    let mut parts = Vec::with_capacity(slices.len());
    let mut rest = v;
    let mut pos = 0;
    for (s, &a) in slices.iter().zip(at) {
        let (_, r) = std::mem::take(&mut rest).split_at_mut(a - pos);
        let (mine, r) = r.split_at_mut(s.starts.len());
        parts.push((s, mine));
        rest = r;
        pos = a + s.starts.len();
    }
    parts.into_par_iter().for_each(|(s, out)| copy(&s.starts, out));
}

/// Record offsets: 4 bytes each below 4 GB, 8 above.
#[derive(Debug, Clone)]
pub enum Offsets {
    U32(Vec<u32>),
    U64(Vec<u64>),
}

impl Offsets {
    #[inline]
    pub fn get(&self, i: usize) -> usize {
        match self {
            Offsets::U32(v) => v[i] as usize,
            Offsets::U64(v) => v[i] as usize,
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Offsets::U32(v) => v.len(),
            Offsets::U64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn truncate(&mut self, n: usize) {
        match self {
            Offsets::U32(v) => v.truncate(n),
            Offsets::U64(v) => v.truncate(n),
        }
    }
    /// Switches to 8-byte offsets if data of `len` bytes needs them.
    pub fn widen_for(&mut self, len: usize) {
        if let Offsets::U32(v) = self {
            if len > u32::MAX as usize { *self = Offsets::U64(v.iter().map(|&x| x as u64).collect()); }
        }
    }
    #[inline]
    pub fn push(&mut self, x: u64) {
        match self {
            Offsets::U32(v) => v.push(x as u32),
            Offsets::U64(v) => v.push(x),
        }
    }
    pub fn bytes(&self) -> usize {
        match self {
            Offsets::U32(v) => v.capacity() * 4,
            Offsets::U64(v) => v.capacity() * 8,
        }
    }
}

/// Where a record's content ends within its span (span minus its line break
/// and the empty lines after it).
#[inline]
pub fn content_len(span: &[u8], d: Dialect) -> usize {
    let mut z = span.len();
    if d.lines {
        if z > 0 && span[z - 1] == d.nl { z -= 1; }
        if d.nl == b'\n' && z > 0 && span[z - 1] == b'\r' { z -= 1; }
        return z;
    }
    while z > 0 && (span[z - 1] == b'\n' || span[z - 1] == b'\r') {
        z -= 1;
    }
    // A quoted field may end with line breaks of its own: keep everything up to the closing quote.
    z
}

/// Visits a record's fields (content without its line break). A field holding
/// `""` is unescaped into `tmp`. `f` returns false to stop.
#[inline]
pub fn visit_fields(rec: &[u8], d: u8, tmp: &mut Vec<u8>, mut f: impl FnMut(usize, &[u8]) -> bool) {
    let len = rec.len();
    let mut pos = 0;
    let mut idx = 0;
    loop {
        if pos < len && rec[pos] == Q {
            // Quoted: up to a quote followed, spaces aside, by the delimiter or the end.
            let mut j = pos + 1;
            let mut escaped = false;
            let (end, next) = loop {
                match memchr(Q, &rec[j..]) {
                    None => break (len, None),                       // never closed: the rest
                    Some(off) => {
                        let k = j + off;
                        if k + 1 < len && rec[k + 1] == Q { escaped = true; j = k + 2; continue; }
                        let mut m = k + 1;
                        while m < len && ((rec[m] == b' ' && d != b' ') || (rec[m] == b'\t' && d != b'\t')) { m += 1; }
                        if m >= len { break (k, None); }
                        if rec[m] == d { break (k, Some(m + 1)); }
                        // A line break here is no closing (the scan would have ended the record
                        // there): a stray quote, the value goes on — as the scan read it.
                        j = k + 1;
                    }
                }
            };
            let body = &rec[pos + 1..end];
            let go = if escaped {
                tmp.clear();
                let mut k = 0;
                while k < body.len() {
                    tmp.push(body[k]);
                    k += if body[k] == Q && k + 1 < body.len() && body[k + 1] == Q { 2 } else { 1 };
                }
                f(idx, tmp)
            } else {
                f(idx, body)
            };
            if !go { return; }
            match next {
                Some(n) => { pos = n; idx += 1; }
                None => return,
            }
        } else {
            let end = memchr(d, &rec[pos..]).map_or(len, |k| pos + k);
            if !f(idx, &rec[pos..end]) { return; }
            if end >= len { return; }
            pos = end + 1;
            idx += 1;
        }
    }
}

/// A record's fields as owned byte strings.
pub fn split_fields(rec: &[u8], d: u8) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    visit_fields(rec, d, &mut tmp, |_, f| { out.push(f.to_vec()); true });
    out
}

/// Runs `f` on the one field `col`, if the record has it.
#[inline]
pub fn with_field<R>(rec: &[u8], d: u8, col: usize, tmp: &mut Vec<u8>, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    let mut f = Some(f);
    let mut out = None;
    visit_fields(rec, d, tmp, |i, v| {
        if i == col {
            out = f.take().map(|f| f(v));
            return false;
        }
        true
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// The byte-by-byte scanner alone: the reference the bit masks are checked against.
        pub static SCALAR_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn scalar(b: &[u8], d: Dialect) -> (Slice, u32) {
        SCALAR_ONLY.with(|c| c.set(true));
        let r = scan_range(b, 0, b.len(), d, None, usize::MAX, None);
        SCALAR_ONLY.with(|c| c.set(false));
        r
    }

    /// Random files made of the troublesome pieces: both scanners must agree record by record.
    #[test]
    fn masks_match_bytes() {
        let pieces: &[&[u8]] = &[
            b"a", b"bc", b"12345", b",", b",", b";", b"\t", b"\n", b"\n", b"\r\n", b"\r", b"\"", b"\"\"", b"\"x,y\"", b"\"q\"\"q\"",
            b" ", b"\" ", b"\"\n", b"\xC3\xA9", b"\xE9", b"\n\n", b",\"", b"\",", b"\"\r\n", b"\"\r", b"long field with words ",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = |n: usize| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % n as u64) as usize };
        for round in 0..3000 {
            let mut b = Vec::new();
            let target = 1 + rnd(if round % 10 == 0 { 4000 } else { 300 });
            // Mostly regular rows with now and then an odd piece.
            while b.len() < target {
                if rnd(4) == 0 {
                    b.extend_from_slice(pieces[rnd(pieces.len())]);
                } else {
                    let row = match rnd(4) { 0 => &b"x,\"y\",z\n"[..], 1 => b"\"a\",\"b, c\",\"\"\"\"\r\n", 2 => b"1,2,3\n", _ => b"\"multi\nline\",2,3\n" };
                    b.extend_from_slice(row);
                }
            }
            for d in [
                Dialect { delim: b',', nl: b'\n', lines: false },
                Dialect { delim: b'\t', nl: b'\n', lines: false },
                Dialect { delim: b',', nl: b'\r', lines: false },
                Dialect { delim: 0, nl: b'\n', lines: true },
            ] {
                let (want, ww) = scalar(&b, d);
                let (got, gw) = scan_range(&b, 0, b.len(), d, None, usize::MAX, None);
                let ctx = || format!("{:?} {:?}", d, String::from_utf8_lossy(&b));
                assert_eq!(got.starts.len(), want.starts.len(), "{}", ctx());
                for k in 0..want.starts.len() { assert_eq!(got.starts.get(k), want.starts.get(k), "record {k} {}", ctx()); }
                assert_eq!((got.next, gw, &got.odd, got.qerr), (want.next, ww, &want.odd, want.qerr), "{}", ctx());
                // From a cut in the middle, with a stop: the same again.
                let from = b.len() / 3;
                let from = memchr(d.nl, &b[from..]).map_or(b.len(), |k| from + k + 1);
                let stop = (from + b.len()) / 2;
                SCALAR_ONLY.with(|c| c.set(true));
                let (want, _) = scan_range(&b, from, stop, d, Some(3), usize::MAX, None);
                SCALAR_ONLY.with(|c| c.set(false));
                let (got, _) = scan_range(&b, from, stop, d, Some(3), usize::MAX, None);
                assert_eq!(got.starts.len(), want.starts.len(), "{}", ctx());
                assert_eq!((got.next, &got.odd, got.qerr), (want.next, &want.odd, want.qerr), "{}", ctx());
            }
        }
    }

    #[test]
    fn utf8_in_the_same_pass() {
        let mut b = Vec::new();
        for i in 0..300_000 { b.extend_from_slice(format!("{i},caf\u{e9} cr\u{e8}me,\"x\ny\"\n").as_bytes()); }
        let d = Dialect { delim: b',', nl: b'\n', lines: false };
        assert!(scan_checked(&b, d, None, true).utf8);
        let k = b.len() - 1000;
        b[k] = 0xE9;
        assert!(!scan_checked(&b, d, None, true).utf8);
        b[k] = b'e';
        // A character cut by a window's end is no error.
        assert!(scan_checked(&b, d, None, true).utf8);
    }

    const CSV: Dialect = Dialect { delim: b',', nl: b'\n', lines: false };

    fn records(b: &[u8], d: Dialect) -> Vec<Vec<String>> {
        let s = scan(b, d, None);
        (0..s.n)
            .map(|i| {
                let span = &b[s.starts.get(i)..s.starts.get(i + 1)];
                let rec = &span[..content_len(span, d)];
                split_fields(rec, d.delim).into_iter().map(|f| String::from_utf8(f).unwrap()).collect()
            })
            .collect()
    }

    #[test]
    fn quotes_and_line_breaks() {
        let b = b"a,b,c\n1,\"x,y\",\"line\nbreak\"\r\n\n2,\"say \"\"hi\"\"\",3\n";
        let r = records(b, CSV);
        assert_eq!(r, vec![
            vec!["a", "b", "c"],
            vec!["1", "x,y", "line\nbreak"],
            vec!["2", "say \"hi\"", "3"],
        ]);
    }

    #[test]
    fn stray_quote_kept() {
        let b = b"a,b\n1,ab\"c\n\"un\"closed,2\n";
        let s = scan(b, CSV, None);
        assert_eq!(s.n, 3);
        let r = records(b, CSV);
        assert_eq!(r[1], vec!["1", "ab\"c"]);
    }

    #[test]
    fn spans_cover_the_file() {
        let b = b"\na,b\r\n\r\n1,2\n3,\"4\n\"\n5,6";
        let s = scan(b, CSV, None);
        let mut joined = b[..s.starts.get(0)].to_vec();
        for i in 0..s.n {
            joined.extend_from_slice(&b[s.starts.get(i)..s.starts.get(i + 1)]);
        }
        assert_eq!(joined, b.to_vec());
        assert_eq!(s.n, 4);
    }

    #[test]
    fn parallel_matches_sequential() {
        // Big enough for several slices, with quoted line breaks everywhere.
        let mut b = Vec::new();
        for i in 0..400_000 {
            if i % 7 == 0 {
                b.extend_from_slice(format!("{i},\"multi\nline {i}\",x\n").as_bytes());
            } else {
                b.extend_from_slice(format!("{i},plain {i},y\n").as_bytes());
            }
        }
        let par = scan(&b, CSV, None);
        let (seq, _) = scan_range(&b, 0, b.len(), CSV, None, usize::MAX, None);
        assert_eq!(par.n, seq.starts.len());
        for i in 0..par.n {
            assert_eq!(par.starts.get(i), seq.starts.get(i));
        }
        assert!(par.odd.is_empty());
    }

    #[test]
    fn odd_rows() {
        let b = b"a,b\n1,2,3\n4\n5,6\n";
        let s = scan(b, CSV, None);
        assert_eq!(s.odd, vec![(1, 3), (2, 1)]);
    }

    #[test]
    fn lines_mode_keeps_empty_lines() {
        let d = Dialect { delim: 0, nl: b'\n', lines: true };
        let b = b"one \"x\n\n  three\n";
        let s = scan(b, d, None);
        assert_eq!(s.n, 3);
    }
}
