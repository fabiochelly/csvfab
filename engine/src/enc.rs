//! Encodings: detection, per-field decoding, encoding of edited fields.
//!
//! The file is kept as its own bytes (UTF-16 is transcoded to UTF-8 once, on
//! open) and decoded one field at a time: never slices of the file decoded on
//! their own, which would split a multi-byte character in two.

use encoding_rs::{Encoding, MACINTOSH, UTF_16BE, UTF_16LE, WINDOWS_1252, ISO_8859_15};
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
}

impl Enc {
    pub const ALL: [Enc; 7] = [Enc::Utf8, Enc::Windows1252, Enc::Latin1, Enc::Latin9, Enc::MacRoman, Enc::Utf16Le, Enc::Utf16Be];

    pub fn label(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Windows1252 => "Windows-1252",
            Enc::Latin1 => "ISO-8859-1",
            Enc::Latin9 => "ISO-8859-15",
            Enc::MacRoman => "Mac Roman",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
        }
    }

    pub fn is_utf16(self) -> bool {
        matches!(self, Enc::Utf16Le | Enc::Utf16Be)
    }

    /// Whether the bytes kept in memory are UTF-8 (UTF-16 is transcoded on open).
    pub fn mem_utf8(self) -> bool {
        matches!(self, Enc::Utf8 | Enc::Utf16Le | Enc::Utf16Be)
    }

    fn single_byte(self) -> Option<&'static Encoding> {
        match self {
            Enc::Windows1252 | Enc::Latin1 => Some(WINDOWS_1252),
            Enc::Latin9 => Some(ISO_8859_15),
            Enc::MacRoman => Some(MACINTOSH),
            _ => None,
        }
    }

    pub fn bom(self) -> &'static [u8] {
        match self {
            Enc::Utf8 => b"\xEF\xBB\xBF",
            Enc::Utf16Le => b"\xFF\xFE",
            Enc::Utf16Be => b"\xFE\xFF",
            _ => b"",
        }
    }
}

/// A BOM decides; otherwise zero bytes on every other position mean UTF-16
/// without BOM; otherwise UTF-8, to be confirmed by `valid_utf8` on the whole
/// file (a single 1252 "é" can sit on the last line). Returns the BOM length.
pub fn sniff(b: &[u8]) -> (Enc, usize) {
    if b.starts_with(b"\xEF\xBB\xBF") {
        return (Enc::Utf8, 3);
    }
    if b.starts_with(b"\xFF\xFE") {
        return (Enc::Utf16Le, 2);
    }
    if b.starts_with(b"\xFE\xFF") {
        return (Enc::Utf16Be, 2);
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

/// UTF-16 → UTF-8, once on open: the scanner and the field decoder then only
/// know byte encodings.
pub fn transcode_utf16(b: &[u8], enc: Enc) -> Vec<u8> {
    let e = if enc == Enc::Utf16Be { UTF_16BE } else { UTF_16LE };
    let (s, _) = e.decode_without_bom_handling(b);
    s.into_owned().into_bytes()
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

/// Text to the bytes kept in memory for this encoding (UTF-8 for UTF-16
/// files). A character the encoding cannot hold is written as '?': the caller
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
            encoding_rs::EncoderResult::Unmappable(_) => out.push(b'?'),
        }
    }
}

/// The first character of `s` this encoding cannot hold.
pub fn unencodable(enc: Enc, s: &str) -> Option<char> {
    let e = enc.single_byte()?;
    if s.is_ascii() {
        return None;
    }
    s.chars().find(|c| {
        let mut b = [0u8; 4];
        let (_, _, unmappable) = e.encode(c.encode_utf8(&mut b));
        unmappable
    })
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
