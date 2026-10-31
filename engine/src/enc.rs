//! Encodings: detection, per-field decoding, encoding of edited fields.
//!
//! A single-byte file is kept as its own bytes and decoded one field at a time: never
//! slices of the file decoded on their own, which would split a character in two. Wider
//! encodings are transcoded to UTF-8 once, on open (`transcode`): UTF-16 and UTF-32 have
//! zeros between the delimiters, and in Shift_JIS, GBK or Big5 a delimiter's byte can be
//! the second half of a character, so the scanner never sees those bytes.

use encoding_rs::{Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, IBM866, ISO_2022_JP, ISO_8859_15, ISO_8859_2, ISO_8859_5, ISO_8859_6, ISO_8859_7, ISO_8859_8, ISO_8859_8_I, KOI8_R, KOI8_U, MACINTOSH, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255, WINDOWS_1256, WINDOWS_1257, WINDOWS_1258, WINDOWS_874};
use rayon::prelude::*;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Enc {
    Utf8,
    Windows1252,
    /// WHATWG decodes ISO-8859-1 as Windows-1252, a superset: written back
    /// through the same table, the original bytes come back.
    Latin1,
    Latin9,
    MacRoman,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
    Windows1251,
    Koi8R,
    Koi8U,
    Iso8859_5,
    Ibm866,
    Windows1250,
    Iso8859_2,
    Windows1253,
    Iso8859_7,
    Windows1254,
    Windows1257,
    Windows1258,
    Windows874,
    Windows1255,
    Iso8859_8,
    Windows1256,
    Iso8859_6,
    ShiftJis,
    EucJp,
    Iso2022Jp,
    EucKr,
    Gbk,
    Gb18030,
    Big5,
}

impl Enc {
    /// In the order of journal.rs's index: new encodings go at the end.
    pub const ALL: [Enc; 33] = [
        Enc::Utf8, Enc::Windows1252, Enc::Latin1, Enc::Latin9, Enc::MacRoman, Enc::Utf16Le, Enc::Utf16Be, Enc::Utf32Le, Enc::Utf32Be,
        Enc::Windows1251, Enc::Koi8R, Enc::Koi8U, Enc::Iso8859_5, Enc::Ibm866, Enc::Windows1250, Enc::Iso8859_2, Enc::Windows1253, Enc::Iso8859_7,
        Enc::Windows1254, Enc::Windows1257, Enc::Windows1258, Enc::Windows874, Enc::Windows1255, Enc::Iso8859_8, Enc::Windows1256, Enc::Iso8859_6,
        Enc::ShiftJis, Enc::EucJp, Enc::Iso2022Jp, Enc::EucKr, Enc::Gbk, Enc::Gb18030, Enc::Big5,
    ];

    /// By region, as the pickers list them.
    pub const GROUPS: [(&'static str, &'static [Enc]); 15] = [
        ("Unicode", &[Enc::Utf8, Enc::Utf16Le, Enc::Utf16Be, Enc::Utf32Le, Enc::Utf32Be]),
        ("Western", &[Enc::Windows1252, Enc::Latin1, Enc::Latin9, Enc::MacRoman]),
        ("Central Europe", &[Enc::Windows1250, Enc::Iso8859_2]),
        ("Cyrillic", &[Enc::Windows1251, Enc::Koi8R, Enc::Koi8U, Enc::Iso8859_5, Enc::Ibm866]),
        ("Greek", &[Enc::Windows1253, Enc::Iso8859_7]),
        ("Turkish", &[Enc::Windows1254]),
        ("Baltic", &[Enc::Windows1257]),
        ("Vietnamese", &[Enc::Windows1258]),
        ("Thai", &[Enc::Windows874]),
        ("Hebrew", &[Enc::Windows1255, Enc::Iso8859_8]),
        ("Arabic", &[Enc::Windows1256, Enc::Iso8859_6]),
        ("Japanese", &[Enc::ShiftJis, Enc::EucJp, Enc::Iso2022Jp]),
        ("Korean", &[Enc::EucKr]),
        ("Chinese, simplified", &[Enc::Gbk, Enc::Gb18030]),
        ("Chinese, traditional", &[Enc::Big5]),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Windows1252 => "Windows-1252",
            Enc::Latin1 => "ISO-8859-1",
            Enc::Latin9 => "ISO-8859-15",
            Enc::MacRoman => "Mac Roman",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
            Enc::Utf32Le => "UTF-32 LE",
            Enc::Utf32Be => "UTF-32 BE",
            Enc::Windows1251 => "Windows-1251",
            Enc::Koi8R => "KOI8-R",
            Enc::Koi8U => "KOI8-U",
            Enc::Iso8859_5 => "ISO-8859-5",
            Enc::Ibm866 => "IBM866",
            Enc::Windows1250 => "Windows-1250",
            Enc::Iso8859_2 => "ISO-8859-2",
            Enc::Windows1253 => "Windows-1253",
            Enc::Iso8859_7 => "ISO-8859-7",
            Enc::Windows1254 => "Windows-1254",
            Enc::Windows1257 => "Windows-1257",
            Enc::Windows1258 => "Windows-1258",
            Enc::Windows874 => "Windows-874",
            Enc::Windows1255 => "Windows-1255",
            Enc::Iso8859_8 => "ISO-8859-8",
            Enc::Windows1256 => "Windows-1256",
            Enc::Iso8859_6 => "ISO-8859-6",
            Enc::ShiftJis => "Shift_JIS",
            Enc::EucJp => "EUC-JP",
            Enc::Iso2022Jp => "ISO-2022-JP",
            Enc::EucKr => "EUC-KR",
            Enc::Gbk => "GBK",
            Enc::Gb18030 => "GB18030",
            Enc::Big5 => "Big5",
        }
    }

    /// The region it is listed under, then its label: what a search in a picker looks through.
    pub fn group(self) -> &'static str {
        Enc::GROUPS.iter().find(|g| g.1.contains(&self)).map_or("", |g| g.0)
    }

    /// Whether a picker's search `q` (lower case) finds it: by label, region or language.
    pub fn matches(self, q: &str) -> bool {
        let g = self.group();
        let words = match g {
            "Western" => "latin french german spanish italian portuguese",
            "Central Europe" => "polish czech hungarian slovak croatian romanian latin2",
            "Cyrillic" => "russian ukrainian bulgarian serbian dos",
            "Chinese, simplified" | "Chinese, traditional" => "cjk taiwan hong kong prc",
            "Japanese" | "Korean" => "cjk",
            "Arabic" => "persian farsi urdu",
            _ => "",
        };
        q.split_whitespace().all(|w| [self.label(), g, words].iter().any(|t| t.to_lowercase().contains(w)))
    }

    pub fn is_utf16(self) -> bool {
        matches!(self, Enc::Utf16Le | Enc::Utf16Be)
    }

    /// UTF-16 or UTF-32: units wider than a byte, each with its byte order.
    pub fn wide(self) -> bool {
        matches!(self, Enc::Utf16Le | Enc::Utf16Be | Enc::Utf32Le | Enc::Utf32Be)
    }

    /// Several bytes to a character, an ASCII byte possibly among them, with no table of
    /// single bytes: Shift_JIS, EUC, GBK, Big5, ISO-2022-JP.
    pub fn multi_byte(self) -> bool {
        matches!(self, Enc::ShiftJis | Enc::EucJp | Enc::Iso2022Jp | Enc::EucKr | Enc::Gbk | Enc::Gb18030 | Enc::Big5)
    }

    /// Whether the bytes kept in memory are UTF-8 (the others are transcoded on open).
    pub fn mem_utf8(self) -> bool {
        self == Enc::Utf8 || self.transcoded()
    }

    /// Read through UTF-8: the bytes kept in memory are not the file's.
    pub fn transcoded(self) -> bool {
        self.wide() || self.multi_byte()
    }

    /// Holds only part of Unicode: a save may have to say what it cannot write.
    pub fn limited(self) -> bool {
        self != Enc::Utf8 && !self.wide()
    }

    fn single_byte(self) -> Option<&'static Encoding> {
        Some(match self {
            Enc::Windows1252 | Enc::Latin1 => WINDOWS_1252,
            Enc::Latin9 => ISO_8859_15,
            Enc::MacRoman => MACINTOSH,
            Enc::Windows1251 => WINDOWS_1251,
            Enc::Koi8R => KOI8_R,
            Enc::Koi8U => KOI8_U,
            Enc::Iso8859_5 => ISO_8859_5,
            Enc::Ibm866 => IBM866,
            Enc::Windows1250 => WINDOWS_1250,
            Enc::Iso8859_2 => ISO_8859_2,
            Enc::Windows1253 => WINDOWS_1253,
            Enc::Iso8859_7 => ISO_8859_7,
            Enc::Windows1254 => WINDOWS_1254,
            Enc::Windows1257 => WINDOWS_1257,
            Enc::Windows1258 => WINDOWS_1258,
            Enc::Windows874 => WINDOWS_874,
            Enc::Windows1255 => WINDOWS_1255,
            Enc::Iso8859_8 => ISO_8859_8,
            Enc::Windows1256 => WINDOWS_1256,
            Enc::Iso8859_6 => ISO_8859_6,
            _ => return None,
        })
    }

    /// encoding_rs's table for this encoding (none for UTF-8, UTF-16 and UTF-32, written by hand).
    fn table(self) -> Option<&'static Encoding> {
        self.single_byte().or(match self {
            Enc::ShiftJis => Some(SHIFT_JIS),
            Enc::EucJp => Some(EUC_JP),
            Enc::Iso2022Jp => Some(ISO_2022_JP),
            Enc::EucKr => Some(EUC_KR),
            Enc::Gbk => Some(GBK),
            Enc::Gb18030 => Some(GB18030),
            Enc::Big5 => Some(BIG5),
            _ => None,
        })
    }

    pub fn bom(self) -> &'static [u8] {
        match self {
            Enc::Utf8 => b"\xEF\xBB\xBF",
            Enc::Utf16Le => b"\xFF\xFE",
            Enc::Utf16Be => b"\xFE\xFF",
            Enc::Utf32Le => b"\xFF\xFE\0\0",
            Enc::Utf32Be => b"\0\0\xFE\xFF",
            _ => b"",
        }
    }
}

/// A BOM decides; the escape switching ISO-2022-JP to Japanese (a file of ASCII bytes, valid
/// UTF-8 otherwise); then zero bytes on every other position mean UTF-16 without BOM; otherwise
/// UTF-8, to be confirmed by `valid_utf8` on the whole file (a single 1252 "é" can sit on the
/// last line). Returns the BOM length.
pub fn sniff(b: &[u8]) -> (Enc, usize) {
    if b.starts_with(b"\xEF\xBB\xBF") {
        return (Enc::Utf8, 3);
    }
    // UTF-32's before UTF-16's: FF FE starts both.
    if b.starts_with(b"\xFF\xFE\0\0") {
        return (Enc::Utf32Le, 4);
    }
    if b.starts_with(b"\0\0\xFE\xFF") {
        return (Enc::Utf32Be, 4);
    }
    if b.starts_with(b"\xFF\xFE") {
        return (Enc::Utf16Le, 2);
    }
    if b.starts_with(b"\xFE\xFF") {
        return (Enc::Utf16Be, 2);
    }
    let head = &b[..b.len().min(1 << 16)];
    if memchr::memchr(0x1B, head).is_some() && memchr::memmem::find(head, b"\x1B$B").is_some() && head.is_ascii() {
        return (Enc::Iso2022Jp, 0);
    }
    let n = b.len().min(4096) & !1;
    let (mut z_even, mut z_odd) = (0usize, 0usize);
    for (i, &x) in b[..n].iter().enumerate() {
        if x == 0 {
            if i & 1 == 1 { z_odd += 1 } else { z_even += 1 }
        }
    }
    if n > 0 && z_odd > n / 8 && z_even < n / 64 {
        return (Enc::Utf16Le, 0);
    }
    if n > 0 && z_even > n / 8 && z_odd < n / 64 {
        return (Enc::Utf16Be, 0);
    }
    (Enc::Utf8, 0)
}

/// Strict UTF-8 check of the whole buffer, in parallel slices cut on character starts.
pub fn valid_utf8(b: &[u8]) -> bool {
    const CHUNK: usize = 16 << 20;
    if b.len() <= CHUNK {
        return std::str::from_utf8(b).is_ok();
    }
    let mut cuts = vec![0usize];
    let mut p = CHUNK;
    while p < b.len() {
        let mut q = p;
        while q > p - 3 && b[q] & 0xC0 == 0x80 {
            q -= 1;
        }
        cuts.push(q);
        p += CHUNK;
    }
    cuts.push(b.len());
    cuts.par_windows(2).all(|w| std::str::from_utf8(&b[w[0]..w[1]]).is_ok())
}

/// What `detect` reads: the line holding the file's first non-ASCII byte and what follows,
/// then as much from the middle.
const SAMPLE: usize = 1 << 16;

/// The sample's two parts, whole lines (a character cut in two would rule its encoding out).
fn sample(b: &[u8]) -> [&[u8]; 2] {
    const STEP: usize = 1 << 16;
    let lines = |a: usize, z: usize| -> &[u8] {
        let a = if a == 0 { 0 } else { memchr::memchr2(b'\n', b'\r', &b[a..z]).map_or(a, |k| a + k + 1) };
        let z = if z == b.len() { z } else { memchr::memrchr2(b'\n', b'\r', &b[a..z]).map_or(z, |k| a + k + 1) };
        &b[a..z]
    };
    let at = b.chunks(STEP).position(|c| !c.is_ascii()).unwrap_or(0) * STEP;
    let end = (at + SAMPLE).min(b.len());
    let mid = (b.len() / 2).max(end);
    [lines(at, end), lines(mid, (mid + SAMPLE).min(b.len()))]
}

/// Bytes ≥ 0x80 that Windows-1252 reads as what French, German, Spanish and their neighbours
/// write: letters with accents, ligatures, the usual signs and punctuation.
fn western_byte(x: u8) -> bool {
    matches!(x, 0x80 | 0x85 | 0x8C | 0x91..=0x97 | 0x9C | 0x9F | 0xA0 | 0xA1 | 0xA3 | 0xA7 | 0xA9 | 0xAA | 0xAB | 0xAE | 0xB0 | 0xB2 | 0xB4 | 0xB7 | 0xBA | 0xBB | 0xBF
        | 0xC0..=0xCF | 0xD1..=0xD6 | 0xD8..=0xDC | 0xDF..=0xEF | 0xF1..=0xF6 | 0xF8..=0xFC | 0xFF)
}

/// The text looks Western: its non-ASCII bytes are mostly alone between ASCII ones (`café`:
/// other scripts come in runs of them) and nearly all are Western there.
fn looks_western(parts: &[&[u8]]) -> bool {
    let (mut high, mut alone, mut west) = (0usize, 0usize, 0usize);
    for b in parts {
        for (i, &x) in b.iter().enumerate() {
            if x < 0x80 { continue; }
            high += 1;
            alone += (b.get(i + 1).is_none_or(|n| n.is_ascii()) && (i == 0 || b[i - 1].is_ascii())) as usize;
            west += western_byte(x) as usize;
        }
    }
    alone * 2 >= high && west * 10 >= high * 9
}

/// What a file neither UTF-8 nor marked by a BOM most likely is, from a sample: chardetng's
/// guess, unless the bytes read as Western text (the encoding such files always opened in, and
/// where a guess on a few accents is at its weakest).
pub fn detect(b: &[u8]) -> Enc {
    let parts = sample(b);
    if looks_western(&parts) {
        return Enc::Windows1252;
    }
    let mut d = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    d.feed(parts[0], false);
    d.feed(parts[1], true);
    let g = d.guess(None, chardetng::Utf8Detection::Deny);
    let g = if g == ISO_8859_8_I { ISO_8859_8 } else { g };
    Enc::ALL.into_iter().find(|e| e.table() == Some(g)).unwrap_or(Enc::Windows1252)
}

/// A file's bytes as UTF-8, and where its pieces lie in both.
pub struct Transcoded {
    pub text: Vec<u8>,
    /// (offset in `text`, offset in the file) of the start of each piece, at a line start. Empty
    /// when lines cannot be told apart in the file's bytes (`Base::raw_run`).
    pub marks: Vec<(u64, u64)>,
}

/// What one piece of a multi-byte file is, read in parallel with the others.
const PIECE: usize = 1 << 18;

/// Whole file → UTF-8, once on open: the scanner and the field decoder then only know byte
/// encodings. No multi-byte encoding has a line break's byte inside a character and each comes
/// back to its start at one, so the pieces are cut there and read side by side; ISO-2022-JP
/// carries its mode across lines and is read in one go.
pub fn transcode(b: &[u8], enc: Enc) -> Transcoded {
    match enc {
        Enc::Utf16Le | Enc::Utf16Be => {
            let e = if enc == Enc::Utf16Be { UTF_16BE } else { UTF_16LE };
            Transcoded { text: e.decode_without_bom_handling(b).0.into_owned().into_bytes(), marks: Vec::new() }
        }
        Enc::Utf32Le | Enc::Utf32Be => Transcoded { text: utf32_to_utf8(b, enc == Enc::Utf32Be), marks: Vec::new() },
        _ => {
            let e = enc.table().unwrap_or(UTF_8);
            let mut cuts = vec![0usize];
            let mut p = PIECE;
            while enc != Enc::Iso2022Jp && p < b.len() {
                let Some(k) = memchr::memchr2(b'\n', b'\r', &b[p..]) else { break };
                cuts.push(p + k + 1);
                p += k + 1 + PIECE;
            }
            cuts.push(b.len());
            let parts: Vec<(Vec<u8>, bool)> = cuts.par_windows(2).map(|w| {
                let raw = &b[w[0]..w[1]];
                let t = e.decode_without_bom_handling(raw).0.into_owned().into_bytes();
                let count = |x: &[u8], c: u8| memchr::memchr_iter(c, x).count();
                let same = count(raw, b'\n') == count(&t, b'\n') && count(raw, b'\r') == count(&t, b'\r');
                (t, same)
            }).collect();
            let exact = parts.iter().all(|p| p.1);
            let mut text = Vec::with_capacity(parts.iter().map(|p| p.0.len()).sum());
            let mut marks = Vec::with_capacity(if exact { parts.len() } else { 0 });
            // One piece after the other, each freed once copied: never the text twice in memory.
            for (k, (t, _)) in parts.into_iter().enumerate() {
                if exact { marks.push((text.len() as u64, cuts[k] as u64)); }
                text.extend_from_slice(&t);
            }
            Transcoded { text, marks }
        }
    }
}

fn utf32_to_utf8(b: &[u8], be: bool) -> Vec<u8> {
    let parts: Vec<Vec<u8>> = b.par_chunks(4 << 20).map(|c| {
        let mut s = String::with_capacity(c.len() / 3);
        let mut it = c.chunks_exact(4);
        for u in &mut it {
            let u = [u[0], u[1], u[2], u[3]];
            s.push(char::from_u32(if be { u32::from_be_bytes(u) } else { u32::from_le_bytes(u) }).unwrap_or('\u{FFFD}'));
        }
        if !it.remainder().is_empty() { s.push('\u{FFFD}'); }
        s.into_bytes()
    }).collect();
    parts.concat()
}

/// One field's bytes (as kept in memory) to text.
#[inline]
pub fn decode(enc: Enc, b: &[u8]) -> Cow<'_, str> {
    if enc.mem_utf8() || b.is_ascii() {
        return String::from_utf8_lossy(b);
    }
    let e = enc.single_byte().unwrap_or(WINDOWS_1252);
    e.decode_without_bom_handling(b).0
}

/// A piece of a file, in its encoding, to text (for showing the bytes around a place).
pub fn decode_file(enc: Enc, b: &[u8]) -> String {
    match enc {
        Enc::Utf8 => String::from_utf8_lossy(b).into_owned(),
        Enc::Utf16Le => UTF_16LE.decode_without_bom_handling(b).0.into_owned(),
        Enc::Utf16Be => UTF_16BE.decode_without_bom_handling(b).0.into_owned(),
        Enc::Utf32Le | Enc::Utf32Be => String::from_utf8_lossy(&utf32_to_utf8(b, enc == Enc::Utf32Be)).into_owned(),
        _ => enc.table().unwrap_or(UTF_8).decode_without_bom_handling(b).0.into_owned(),
    }
}

/// `s` through `e`'s encoder, a character it cannot hold written as '?' (the encoder's own, so
/// that ISO-2022-JP leaves its Japanese mode for it).
fn encode_table(e: &'static Encoding, s: &str, out: &mut Vec<u8>) {
    let mut encoder = e.new_encoder();
    let mut src = s;
    let mut buf = [0u8; 1024];
    loop {
        let (res, read, written) = encoder.encode_from_utf8_without_replacement(src, &mut buf, true);
        out.extend_from_slice(&buf[..written]);
        src = &src[read..];
        match res {
            encoding_rs::EncoderResult::InputEmpty => break,
            encoding_rs::EncoderResult::OutputFull => {}
            encoding_rs::EncoderResult::Unmappable(_) => {
                let (_, _, written) = encoder.encode_from_utf8_without_replacement("?", &mut buf, false);
                out.extend_from_slice(&buf[..written]);
            }
        }
    }
}

/// Text to the bytes kept in memory for this encoding (UTF-8 for a transcoded
/// file). A character the encoding cannot hold is written as '?': the caller
/// warns beforehand with `unencodable`.
pub fn encode_into(enc: Enc, s: &str, out: &mut Vec<u8>) {
    let Some(e) = enc.single_byte() else {
        out.extend_from_slice(s.as_bytes());
        return;
    };
    if s.is_ascii() {
        out.extend_from_slice(s.as_bytes());
        return;
    }
    encode_table(e, s, out);
}

/// Text to the bytes of a file in this encoding, whatever it keeps in memory.
pub fn encode_file(enc: Enc, s: &str, out: &mut Vec<u8>) {
    match enc {
        Enc::Utf16Le | Enc::Utf16Be => utf8_to_utf16(s, enc == Enc::Utf16Be, out),
        Enc::Utf32Le | Enc::Utf32Be => {
            out.reserve(s.len() * 4);
            for c in s.chars() {
                out.extend_from_slice(&if enc == Enc::Utf32Be { (c as u32).to_be_bytes() } else { (c as u32).to_le_bytes() });
            }
        }
        // (ASCII is ASCII here too, but for ISO-2022-JP's return to it.)
        _ => match enc.table() {
            Some(e) if !s.is_ascii() => encode_table(e, s, out),
            _ => out.extend_from_slice(s.as_bytes()),
        },
    }
}

/// Calls `f` with each character of `s` this encoding cannot hold.
pub fn each_unencodable(enc: Enc, s: &str, mut f: impl FnMut(char)) {
    let Some(e) = enc.table().filter(|_| enc.limited()) else { return };
    if s.is_ascii() {
        return;
    }
    // Written, but read back as another character: the Japanese encoders' own substitutes, and
    // private-use characters a GB18030 revision gave a real one's place.
    let moved = |c: char| match (enc, c) {
        (Enc::ShiftJis | Enc::EucJp, '\u{A5}' | '\u{203E}' | '\u{2212}') | (Enc::Iso2022Jp, '\u{2212}' | '\u{FF61}'..='\u{FF9F}') => true,
        (_, '\u{E000}'..='\u{F8FF}') => e.decode_without_bom_handling(&e.encode(c.encode_utf8(&mut [0; 4])).0).0.chars().next() != Some(c),
        _ => false,
    };
    if enc.multi_byte() {
        s.chars().filter(|&c| moved(c)).for_each(&mut f);
    }
    let mut encoder = e.new_encoder();
    let mut src = s;
    let mut buf = [0u8; 1024];
    loop {
        let (res, read, _) = encoder.encode_from_utf8_without_replacement(src, &mut buf, true);
        src = &src[read..];
        match res {
            encoding_rs::EncoderResult::InputEmpty => return,
            encoding_rs::EncoderResult::OutputFull => {}
            encoding_rs::EncoderResult::Unmappable(c) => f(c),
        }
    }
}

/// The first character of `s` this encoding cannot hold.
pub fn unencodable(enc: Enc, s: &str) -> Option<char> {
    let mut first = None;
    each_unencodable(enc, s, |c| { first.get_or_insert(c); });
    first
}

/// UTF-8 bytes to UTF-16 for writing a UTF-16 file.
pub fn utf8_to_utf16(s: &str, be: bool, out: &mut Vec<u8>) {
    out.reserve(s.len() * 2);
    for u in s.encode_utf16() {
        let b = if be { u.to_be_bytes() } else { u.to_le_bytes() };
        out.extend_from_slice(&b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_byte_roundtrip() {
        for enc in [Enc::Windows1252, Enc::Latin1, Enc::Latin9, Enc::MacRoman] {
            let bytes: Vec<u8> = (0x20u8..=0xFF).filter(|&b| b != 0x7F).collect();
            let s = decode(enc, &bytes).into_owned();
            let mut back = Vec::new();
            encode_into(enc, &s, &mut back);
            // Undefined code points decode to U+FFFD and cannot round-trip; all others must.
            for (i, (&a, &b)) in bytes.iter().zip(back.iter()).enumerate() {
                if s.chars().nth(i) != Some('\u{FFFD}') {
                    assert_eq!(a, b, "{enc:?} byte {a:#x}");
                }
            }
        }
    }

    #[test]
    fn unmappable_becomes_question_mark() {
        let mut out = Vec::new();
        encode_into(Enc::Windows1252, "a→b", &mut out);
        assert_eq!(out, b"a?b");
        assert_eq!(unencodable(Enc::Windows1252, "a→b"), Some('→'));
        assert_eq!(unencodable(Enc::Windows1252, "é€"), None);
    }

    #[test]
    fn sniffs() {
        assert_eq!(sniff(b"\xEF\xBB\xBFa,b"), (Enc::Utf8, 3));
        assert_eq!(sniff(&b"a\0,\0b\0\n\0".repeat(20)), (Enc::Utf16Le, 0));
        assert!(!valid_utf8(b"caf\xE9"));
        assert!(valid_utf8("café".as_bytes()));
    }
}
