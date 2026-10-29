//! "Why does this row look wrong?": one record read as it is in the file —
//! its bytes, not only its decoded text — and what is off and why: the field
//! count against the file's usual one and the probable cause (an unquoted
//! delimiter, located by the column kinds it would realign; a line cut by an
//! unquoted line break; a quote never closed; fields missing), quote
//! anomalies, encoding evidence (UTF-8 bytes in a Windows-1252 file, garbled
//! accents, replacement characters), NUL and other control or invisible
//! characters, cells a spreadsheet would run as formulas. Each finding
//! carries the bytes around it. A repair is proposed when one is safe and
//! unambiguous enough.

use crate::enc::{self, Enc};
use crate::kinds::{self, cell_type, fmt_int, Kind};
use crate::scan;
use crate::table::{RowId, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Info,
}

/// The bytes around a finding, as hex (the finding's own marked) and as text.
#[derive(Clone, Debug, PartialEq)]
pub struct Near {
    /// File offset of the finding.
    pub at: u64,
    /// File offset of the first byte shown.
    pub from: u64,
    pub hex: Vec<(u8, bool)>,
    pub text: String,
}

impl Near {
    /// The marked bytes as `3B` or `C3 A9`.
    pub fn hit_hex(&self) -> String {
        self.hex.iter().filter(|h| h.1).map(|h| format!("{:02X}", h.0)).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// extra-fields, trailing-delimiter, split-line, missing-fields, unclosed-quote,
    /// runaway-quote, text-after-quote, space-before-quote, quote-inside, multiline,
    /// nul, replacement, invisible, mojibake, utf8-line, cp1252, formula.
    pub kind: &'static str,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub near: Option<Near>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairKind {
    /// Empty fields at the end dropped.
    Trim,
    /// Fields cut by an unquoted delimiter made one again.
    Merge,
    /// This row and the next one (cut by a line break) made one.
    Join,
    /// Empty fields added at the end.
    Pad,
    /// The line read as UTF-8.
    Reencode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Repair {
    pub kind: RepairKind,
    pub label: String,
    /// The row's fields once repaired: a re-split of the record.
    pub fields: Vec<String>,
    /// Join: the next row, to delete once joined.
    pub next: Option<RowId>,
}

impl Repair {
    /// The repair as cell changes for `Table::set_cells`: every field, and
    /// the fields past the new end emptied. `set_cells` cannot make a row
    /// shorter: a Trim or a Merge leaves it as wide, with empty fields at
    /// its end, until a call replaces a row's fields whole.
    pub fn cells(&self, id: RowId, observed: usize) -> Vec<(RowId, usize, String)> {
        (0..self.fields.len().max(observed)).map(|c| (id, c, self.fields.get(c).cloned().unwrap_or_default())).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnosis {
    pub row: RowId,
    /// The record's number in the file, 1-based (header included).
    pub line: Option<usize>,
    /// Fields expected (the header's), most rows' count, this row's.
    pub expected: usize,
    pub dominant: usize,
    pub observed: usize,
    /// The record's bytes in the file (line break excluded).
    pub bytes: (u64, u64),
    pub encoding: &'static str,
    /// The row differs from its record as read.
    pub edited: bool,
    pub findings: Vec<Finding>,
    pub cause: Option<String>,
    pub repair: Option<Repair>,
    /// Why there is nothing to read (a row created here).
    pub none: Option<&'static str>,
}

/// The field count most records have — the header line aside; equals go to the header's.
pub fn dominant_width(st: &State) -> usize {
    let b = &st.base;
    let w = kinds::expected_width(st);
    if b.dialect.lines { return w; }
    let skip0 = st.header;
    let mut counts: Vec<(usize, usize)> = vec![(b.width, b.n - b.odd.len() - usize::from(skip0 && b.n > 0 && b.odd.first().is_none_or(|o| o.0 != 0)))];
    for &(r, f) in &b.odd {
        if skip0 && r == 0 { continue; }
        match counts.iter_mut().find(|c| c.0 == f as usize) { Some(c) => c.1 += 1, None => counts.push((f as usize, 1)) }
    }
    let mut best = (b.width, 0usize, false);
    for (k, &(cw, n)) in counts.iter().enumerate() {
        if k == 0 || n > best.1 || (n == best.1 && cw == w) { best = (cw, n, true); }
    }
    best.0
}

/// A field of the record's text: where it lies (byte offsets in the text), its value, its quote issues.
#[derive(Clone, Debug)]
struct Span {
    start: usize,
    end: usize,
    value: String,
    issues: Vec<(&'static str, usize)>,
}

/// The record's fields by the scanner's rules, and what is odd about their quotes.
fn field_spans(s: &str, d: char) -> Vec<Span> {
    let b = s.as_bytes();
    let l = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    let dl = d.len_utf8();
    let at_d = |k: usize| s[k..].starts_with(d);
    loop {
        let mut f = Span { start: i, end: i, value: String::new(), issues: Vec::new() };
        let mut k0 = i;
        while k0 < l && b[k0] == b' ' && d != ' ' { k0 += 1; }
        if i < l && b[i] == b'"' {
            let mut j = i + 1;
            loop {
                let Some(k) = s[j..].find('"').map(|x| x + j) else {
                    f.value.push_str(&s[j..]);
                    f.issues.push(("unclosed", i));
                    i = l;
                    break;
                };
                if b.get(k + 1) == Some(&b'"') { f.value.push_str(&s[j..=k]); j = k + 2; continue; }
                let mut m = k + 1;
                while m < l && (b[m] == b' ' || (b[m] == b'\t' && d != '\t')) { m += 1; }
                if m >= l || at_d(m) { f.value.push_str(&s[j..k]); i = m; break; }
                if !f.issues.iter().any(|x| x.0 == "after-quote") { f.issues.push(("after-quote", k + 1)); }
                f.value.push_str(&s[j..=k]);
                j = k + 1;
            }
        } else {
            let end = s[i..].find(d).map_or(l, |k| k + i);
            f.value = s[i..end].to_string();
            if let Some(q) = f.value.find('"') {
                f.issues.push((if k0 < end && b[k0] == b'"' && k0 > i { "space-before-quote" } else { "quote-inside" }, i + q));
            }
            i = end;
        }
        f.end = i;
        out.push(f);
        if i < l && at_d(i) {
            i += dl;
            if i >= l {
                out.push(Span { start: i, end: i, value: String::new(), issues: Vec::new() });
                break;
            }
            continue;
        }
        return out;
    }
    out
}

/// Where the record lies in the file, and the bytes there.
struct Place<'a> {
    st: &'a State,
    text: &'a str,
    /// File offset of the record's first byte.
    s: u64,
    bom: usize,
    utf16: bool,
}

impl Place<'_> {
    /// A byte offset in the record's text → a file offset.
    fn at(&self, i: usize) -> u64 {
        let i = i.min(self.text.len());
        let b = &self.st.base;
        if self.utf16 { return self.s + 2 * self.text[..i].encode_utf16().count() as u64; }
        if b.enc.mem_utf8() { self.s + i as u64 } else { self.s + self.text[..i].chars().count() as u64 }
    }

    /// The file's byte at file offset k (the BOM included), as far as it can be told.
    fn file(&self) -> (Vec<u8>, u64) {
        let b = &self.st.base;
        if self.utf16 {
            // The file's UTF-16 is not kept: the record's text, encoded again (a lone surrogate, read as U+FFFD, shows as FD FF).
            let mut out = Vec::new();
            enc::utf8_to_utf16(self.text, b.enc == Enc::Utf16Be, &mut out);
            return (out, self.s);
        }
        let mut v = b.enc.bom()[..self.bom].to_vec();
        v.extend_from_slice(b.bytes());
        (v, 0)
    }

    fn near(&self, at: u64, len: usize) -> Near {
        let (f, base) = self.file();
        // Within the file: a text decoded with U+FFFD for bad bytes is longer than they were.
        let rel = ((at - base.min(at)) as usize).min(f.len());
        let (a, z) = (rel.saturating_sub(8), (rel + len + 8).min(f.len()));
        let a2 = if self.utf16 { a - (a & 1) } else { a };
        let hex = (a..z).map(|i| (f[i], i >= rel && i < rel + len)).collect();
        let enc = if self.utf16 { if self.st.base.enc == Enc::Utf16Be { encoding_rs::UTF_16BE } else { encoding_rs::UTF_16LE } } else { enc_of(self.st.base.enc) };
        let text = enc.decode_without_bom_handling(&f[a2..z]).0.into_owned();
        Near { at, from: base + a as u64, hex, text }
    }
}

fn enc_of(e: Enc) -> &'static encoding_rs::Encoding {
    match e {
        Enc::Windows1252 | Enc::Latin1 => encoding_rs::WINDOWS_1252,
        Enc::Latin9 => encoding_rs::ISO_8859_15,
        Enc::MacRoman => encoding_rs::MACINTOSH,
        _ => encoding_rs::UTF_8,
    }
}

/// Pictographs, roughly (Extended_Pictographic): a ZWJ between two is how 👨‍👩‍👧 is written.
fn pictographic(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x1FC00..=0x1FFFD | 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF | 0x2190..=0x21FF | 0xA9 | 0xAE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

fn invisible(c: char) -> bool {
    matches!(c, '\0'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}' | '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{fffd}')
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", fmt_int(n), if n == 1 { one } else { many })
}

/// What is wrong with row `id`, from its bytes.
pub fn diagnose(st: &State, id: RowId) -> Diagnosis {
    let b = &st.base;
    let mut out = Diagnosis { row: id, encoding: b.enc.label(), ..Default::default() };
    if id as usize >= b.n {
        out.none = Some("This row was created here: it has no bytes in the file yet.");
        return out;
    }
    let i = id as usize;
    out.line = Some(i + 1);
    let rec = b.record(i);
    let text = enc::decode(b.enc, rec).into_owned();
    let d = b.dialect.delim as char;
    let utf16 = b.enc.is_utf16();
    let bom = if b.bom && !utf16 { b.enc.bom().len() } else { 0 };
    let start = b.starts.get(i);
    let s = if utf16 {
        let units: u64 = b.bytes()[..start].iter().map(|&x| u64::from(x & 0xC0 != 0x80) + u64::from(x >= 0xF0)).sum();
        2 * (units + u64::from(b.bom))
    } else {
        (bom + start) as u64
    };
    let place = Place { st, text: &text, s, bom, utf16 };
    let fields = if b.dialect.lines { vec![Span { start: 0, end: text.len(), value: text.clone(), issues: vec![] }] } else { field_spans(&text, d) };
    let width = kinds::expected_width(st);
    let dominant = dominant_width(st);
    // Every column deleted: still a field a record (none expected made the shape checks panic).
    let expected = (if b.dialect.lines { 1 } else if st.header { width } else { dominant }).max(1);
    let mut h = st.headers();
    h.truncate(width);
    out.expected = expected;
    out.dominant = dominant;
    out.observed = fields.len();
    out.bytes = (place.at(0), place.at(text.len()));
    out.edited = {
        let (mut a, mut f) = (Vec::new(), Vec::new());
        st.row(id, &mut a);
        b.fields(i, &mut f);
        a != f
    };
    let col_name = |c: usize| if c < h.len() { format!("“{}”", h[c]) } else { format!("field {}", c + 1) };
    let mut add = |out: &mut Diagnosis, kind: &'static str, severity: Severity, title: String, detail: String, at: Option<u64>, len: usize| {
        out.findings.push(Finding { kind, severity, title, detail, near: at.map(|a| place.near(a, len)) });
    };

    // --- shape: how many fields, and why not the expected count
    if !b.dialect.lines && fields.len() != expected {
        if fields.len() > expected {
            shape_extra(st, &fields, &text, expected, &place, &mut out, &mut add, &col_name);
        } else {
            shape_short(st, id, &fields, expected, &place, &mut out, &mut add);
        }
    }
    // --- quotes
    for (c, f) in fields.iter().enumerate() {
        for &(kind, at) in &f.issues {
            match kind {
                "unclosed" => {
                    let lines = text.split('\n').count();
                    let detail = if lines > 1 { format!("Everything after it, {} included, was read as one field.", plural(lines - 1, "line break", "line breaks")) } else { "The field runs to the end of the line.".into() };
                    add(&mut out, "unclosed-quote", Severity::Error, format!("A quote opened in {} is never closed", col_name(c)), detail, Some(place.at(at)), 1);
                }
                "after-quote" => {
                    let swallowed = f.value.split('\n').count() - 1;
                    if swallowed > 0 {
                        let follows: String = text[at..].chars().take(12).collect::<String>().split('\n').next().unwrap_or("").to_string();
                        let detail = format!("“{follows}” follows its closing quote, so that quote does not close the field: by the CSV rules the field goes on to the next quote, {} further. {} read as part of this row, not as rows of their own — even when the field count still looks right.",
                            plural(swallowed, "line", "lines"), if swallowed > 1 { "Those lines are" } else { "That line is" });
                        add(&mut out, "runaway-quote", Severity::Error, format!("A quote in {} runs on past the end of its line", col_name(c)), detail, Some(place.at(at)), 1);
                    } else {
                        add(&mut out, "text-after-quote", Severity::Warn, format!("Text after a closing quote in {}", col_name(c)),
                            "A quoted field must end at its closing quote; the quote is kept as text here, other programs may split or drop it.".into(), Some(place.at(at)), 1);
                    }
                }
                "space-before-quote" => add(&mut out, "space-before-quote", Severity::Warn, format!("Spaces before a quote in {}", col_name(c)),
                    "The field is read as text with its quotes; programs that trim spaces read it as a quoted field.".into(), Some(place.at(at)), 1),
                _ => add(&mut out, "quote-inside", Severity::Info, format!("A quote inside the unquoted {}", col_name(c)),
                    "Kept as an ordinary character (RFC 4180 does not allow it; most readers accept it).".into(), Some(place.at(at)), 1),
            }
        }
    }
    // A record over several lines of the file, for no reason found above: say so.
    let spans = text.split('\n').count() - 1;
    if spans > 0 && !out.findings.iter().any(|f| f.kind == "runaway-quote" || f.kind == "unclosed-quote") {
        add(&mut out, "multiline", Severity::Info, format!("This row spans {} lines of the file", fmt_int(spans + 1)),
            "A quoted value holds a line break: valid CSV, but programs that read line by line cut the row there.".into(), Some(place.at(text.find('\n').unwrap())), 1);
    }
    // --- characters
    let mut seen = Vec::new();
    for (k, ch) in text.char_indices().filter(|e| invisible(e.1)) {
        let cp = ch as u32;
        if seen.contains(&cp) { continue; }
        if cp == 0x200D {
            let before = text[..k].chars().rev().scan(0, |u, c| { *u += c.len_utf16(); (*u <= 4).then_some(c) }).any(pictographic);
            let after = text[k + 3..].chars().scan(0, |u, c| { *u += c.len_utf16(); (*u <= 2).then_some(c) }).any(pictographic);
            if before && after { continue; }
        }
        seen.push(cp);
        let where_ = fields.iter().position(|f| k >= f.start && k < f.end.max(f.start + 1)).map_or(String::new(), |c| format!(" in {}", col_name(c)));
        if cp == 0 {
            add(&mut out, "nul", Severity::Error, format!("NUL byte{where_}"), "Many programs stop reading a value, or the whole file, at a NUL.".into(), Some(place.at(k)), 1);
        } else if cp == 0xFFFD {
            let detail = if utf16 { "Invalid UTF-16 here (a lone surrogate): kept byte for byte in the file, shown as �." } else { "U+FFFD is in the file itself: a character was already lost by the program that wrote it." };
            add(&mut out, "replacement", Severity::Error, format!("Replacement character �{where_}"), detail.into(), Some(place.at(k)), if utf16 { 2 } else { 3 });
        } else {
            add(&mut out, "invisible", if cp < 32 || cp == 127 { Severity::Warn } else { Severity::Info }, format!("{} (U+{cp:04X}){where_}", kinds::inv_name(cp)),
                "Invisible in most programs; it makes two values that look the same compare different.".into(), Some(place.at(k)), 1);
        }
    }
    // --- encoding
    if !utf16 {
        let high = rec.iter().filter(|&&x| x >= 0x80).count();
        let first_high = rec.iter().position(|&x| x >= 0x80);
        if b.enc == Enc::Utf8 {
            if kinds::has_mojibake(&text) {
                let m = kinds::find_mojibake(&text);
                let what = m.map_or("", |(a, z)| &text[a..z]);
                add(&mut out, "mojibake", Severity::Warn, "Garbled accents (mojibake)".into(),
                    format!("“{what}” is UTF-8 that was read as Windows-1252 and saved again, before this file was written. Clean up › Repair garbled accents decodes it back."), m.map(|(a, _)| place.at(a)), 2);
            }
        } else if let Some(fh) = first_high {
            match std::str::from_utf8(rec) {
                Ok(utf8) => {
                    let shown = if utf8.chars().count() > 60 { format!("{}…", utf8.chars().take(60).collect::<String>()) } else { utf8.to_string() };
                    add(&mut out, "utf8-line", Severity::Error, format!("This line is UTF-8 in a file read as {}", b.enc.label()),
                        format!("Its {} bytes ≥ 0x80 form valid UTF-8 sequences, which text in {} almost never does: read as UTF-8 it says “{shown}”. The file mixes two encodings (lines appended by another program?).", fmt_int(high), b.enc.label()),
                        Some(s + fh as u64), 2);
                    // Saved back in the file's encoding: only when every character exists there (else "?" would be written).
                    let f: Vec<String> = scan::split_fields(utf8.as_bytes(), b.dialect.delim).into_iter().map(|v| String::from_utf8(v).unwrap()).collect();
                    if out.repair.is_none() && !f.iter().any(|v| enc::unencodable(b.enc, v).is_some()) {
                        out.repair = Some(Repair { kind: RepairKind::Reencode, label: format!("Read this line as UTF-8 (saved back in {})", b.enc.label()), fields: f, next: None });
                    }
                }
                Err(_) if b.enc == Enc::Windows1252 => {
                    let ch = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&rec[fh..=fh]).0.into_owned();
                    add(&mut out, "cp1252", Severity::Info, "Read as Windows-1252".into(),
                        format!("{} ≥ 0x80 here (first: 0x{:X}, “{ch}”), not valid UTF-8: consistent with the file's encoding.", plural(high, "byte", "bytes"), rec[fh]), Some(s + fh as u64), 1);
                }
                Err(_) => {}
            }
        }
    }
    // --- spreadsheet formulas
    let formula = |v: &str| {
        let Some(c0) = v.chars().next() else { return false };
        let numeric = { let t = v.strip_prefix(['+', '-']).unwrap_or(v); !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || kinds::is_js_space(c) || c == '.' || c == ',') };
        matches!(c0, '=' | '+' | '-' | '@' | '\t' | '\r') && !numeric && !v.chars().all(|c| c == '-')
    };
    if let Some(c) = fields.iter().position(|f| formula(&f.value)) {
        let c0 = fields[c].value.chars().next().unwrap();
        add(&mut out, "formula", Severity::Info, format!("{} starts with “{}”", col_name(c), if c0 == '\t' { '⇥' } else { c0 }),
            "Excel and LibreOffice run a cell starting with = + - @ as a formula when the file is opened there (CSV injection).".into(), Some(place.at(fields[c].start)), 1);
    }
    out
}

type Add<'a> = dyn FnMut(&mut Diagnosis, &'static str, Severity, String, String, Option<u64>, usize) + 'a;

/// Too many fields: which delimiter is part of a value? Each place where
/// k+1 consecutive fields could be one is tried, the realigned row scored
/// against the columns' kinds; a delimiter followed by a space ("Lyon;
/// France") is the likeliest to be text.
fn shape_extra(st: &State, fields: &[Span], text: &str, expected: usize, place: &Place, out: &mut Diagnosis, add: &mut Add, col_name: &dyn Fn(usize) -> String) {
    let k = fields.len() - expected;
    let d = st.base.dialect.delim as char;
    let vals: Vec<&str> = fields.iter().map(|f| f.value.as_str()).collect();
    let s_k = if k > 1 { "s" } else { "" };
    if vals[expected..].iter().all(|v| v.is_empty()) {
        add(out, "trailing-delimiter", Severity::Warn, format!("{} empty field{s_k} at the end of the line", fmt_int(k)),
            "A delimiter at the end of the line: harmless, but the row is one field wider than the header.".into(), Some(place.at(fields[expected].start - 1)), 1);
        out.cause = Some("a delimiter at the end of the line".into());
        out.repair = Some(Repair { kind: RepairKind::Trim, label: format!("Drop the {} empty trailing field{s_k}", fmt_int(k)), fields: vals[..expected].iter().map(|s| s.to_string()).collect(), next: None });
        return;
    }
    let kinds = kinds::column_kinds(st);
    let fits = |v: &str, kind: Option<&Kind>| -> f64 {
        let ty = cell_type(v);
        match kind { _ if ty == Kind::Empty => 0.5, None | Some(Kind::Empty) => 0.5, Some(k) if *k == ty => 1.0, _ => 0.0 }
    };
    let ds = d.to_string();
    let mut best: Option<(usize, f64, Vec<String>, bool)> = None;
    for p in 0..fields.len() - k {
        let merged = vals[p..=p + k].join(&ds);
        let aligned: Vec<String> = vals[..p].iter().map(|s| s.to_string()).chain(std::iter::once(merged)).chain(vals[p + k + 1..].iter().map(|s| s.to_string())).collect();
        let mut score: f64 = aligned.iter().enumerate().map(|(c, v)| fits(v, kinds.get(c))).sum();
        for f in &fields[p..p + k] {
            if text[f.end..].chars().nth(1) == Some(' ') { score += 0.75; }   // "; " reads as text
        }
        if kinds.get(p) == Some(&Kind::Text) { score += 0.25; }
        match &mut best {
            Some(b) if score == b.1 => b.3 = true,
            Some(b) if score <= b.1 => {}
            _ => best = Some((p, score, aligned, false)),
        }
    }
    let (p, _, aligned, tie) = best.unwrap();
    let at = place.at(fields[p].end);   // the first delimiter that should not split
    let s_k1 = if k > 1 { "s" } else { "" };
    if tie {
        add(out, "extra-fields", Severity::Error, format!("{} field{s_k1} more than the header", fmt_int(k)),
            format!("An unquoted delimiter inside a value, most likely in {} — the column types do not settle it for sure.", col_name(p)), Some(at), 1);
        out.cause = Some(format!("an unquoted delimiter, perhaps in {}", col_name(p)));
    } else {
        add(out, "extra-fields", Severity::Error, format!("Unquoted delimiter in {}", col_name(p)),
            format!("“{}” is one value cut in {}: realigned this way, every column gets the kind of value it usually holds.", vals[p..=p + k].join(&ds), fmt_int(k + 1)), Some(at), 1);
        out.cause = Some(format!("unquoted delimiter in {}", col_name(p)));
    }
    let label = format!("Quote fields {}–{} as one {}", p + 1, p + k + 1, col_name(p));
    out.repair = Some(Repair { kind: RepairKind::Merge, label, fields: aligned, next: None });
}

/// Too few fields: a line cut by an unquoted line break (it and the next
/// line make one row), or fields simply missing at the end.
fn shape_short(st: &State, id: RowId, fields: &[Span], expected: usize, place: &Place, out: &mut Diagnosis, add: &mut Add) {
    let k = expected - fields.len();
    let vals: Vec<String> = fields.iter().map(|f| f.value.clone()).collect();
    let pos = (0..st.len()).find(|&p| st.data_id(p) == id);
    let next = pos.filter(|&p| p + 1 < st.len()).map(|p| st.data_id(p + 1));
    if let Some(nx) = next.filter(|&nx| st.pristine(nx) && nx == id + 1) {
        let mut nv = Vec::new();
        st.base.fields(nx as usize, &mut nv);
        if fields.len() + nv.len() - 1 == expected {
            let line = nx as usize + 1;
            add(out, "split-line", Severity::Error, "This row continues on the next line".into(),
                format!("{} fields here and {} on line {}: together, with the line break as part of a value, they make the {} expected. A line break inside a value that was not quoted.", fmt_int(fields.len()), fmt_int(nv.len()), fmt_int(line), fmt_int(expected)),
                Some(place.at(place.text.len())), 1);
            out.cause = Some("an unquoted line break: the row is cut in two".into());
            let mut joined: Vec<String> = vals[..vals.len() - 1].to_vec();
            joined.push(format!("{}\n{}", vals[vals.len() - 1], nv[0]));
            joined.extend(nv[1..].iter().cloned());
            out.repair = Some(Repair { kind: RepairKind::Join, label: format!("Join with line {}", fmt_int(line)), fields: joined, next: Some(nx) });
            return;
        }
    }
    let s = if k > 1 { "s" } else { "" };
    add(out, "missing-fields", Severity::Warn, format!("{} field{s} missing", fmt_int(k)),
        format!("{} fields for {} columns. Saved as is, the line stays short; most readers fill the missing columns with empty values.", fmt_int(fields.len()), fmt_int(expected)), None, 1);
    out.cause = Some(format!("{} field{s} missing at the end", fmt_int(k)));
    let mut padded = vals;
    padded.resize(expected, String::new());
    out.repair = Some(Repair { kind: RepairKind::Pad, label: format!("Add {} empty field{s} at the end", fmt_int(k)), fields: padded, next: None });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    fn table(b: &[u8]) -> Table {
        Table::new(Arc::new(Base::from_bytes(b.to_vec(), OpenOptions::default())), None)
    }

    /// Data row `row` (1-based, as the JS tests count) of a file from tests/corpus.py.
    fn dg(b: &[u8], row: usize) -> (Table, Diagnosis) {
        let t = table(b);
        let d = diagnose(&t.st, t.st.data_id(row - 1));
        (t, d)
    }

    fn kinds(d: &Diagnosis) -> Vec<&'static str> {
        d.findings.iter().map(|f| f.kind).collect()
    }

    const UNQUOTED_DELIMITER: &str = "id;nom;adresse;ville;cp\n1;Ann;12 rue Vert;Lyon;69001\n2;Bob;5 av. Foch;Paris;75008\n3;Cy;7; rue Haute;Lyon;69002\n4;Dee;9 bd Sud;Nice;06000\n";

    #[test]
    fn clean_row_has_no_problem() {
        let (_, d) = dg(UNQUOTED_DELIMITER.as_bytes(), 1);
        assert_eq!((d.expected, d.observed, kinds(&d)), (5, 5, vec![]));
    }

    #[test]
    fn unquoted_delimiter_is_located() {
        let (_, d) = dg(UNQUOTED_DELIMITER.as_bytes(), 3);
        assert_eq!((d.expected, d.observed), (5, 6));
        let k = kinds(&d).iter().position(|k| *k == "extra-fields").unwrap();
        assert!(d.cause.as_ref().unwrap().contains("adresse"));
        assert_eq!(d.findings[k].near.as_ref().unwrap().hit_hex(), "3B");
        let r = d.repair.unwrap();
        assert_eq!((r.kind, r.fields), (RepairKind::Merge, vec!["3", "Cy", "7; rue Haute", "Lyon", "69002"].into_iter().map(String::from).collect::<Vec<_>>()));
    }

    #[test]
    fn split_line_is_joined() {
        let mut t = table(b"id;adresse;ville\n1;12 rue Vert;Lyon\n2;5 av. Foch\nbat. B;Paris\n3;9 bd Sud;Nice\n");
        let id = t.st.data_id(1);
        let d = diagnose(&t.st, id);
        assert_eq!((d.expected, d.observed, d.findings[0].kind), (3, 2, "split-line"));
        let r = d.repair.clone().unwrap();
        assert_eq!(r.fields, vec!["2", "5 av. Foch\nbat. B", "Paris"]);
        // Applied: the row's cells, the next row deleted.
        t.set_cells(&r.cells(id, d.observed));
        t.delete_rows(&[r.next.unwrap()].into_iter().collect());
        assert_eq!(t.st.len(), 3);
        let mut row = Vec::new();
        t.st.row(id, &mut row);
        assert_eq!(row, vec!["2", "5 av. Foch\nbat. B", "Paris"]);
    }

    #[test]
    fn missing_and_extra_fields() {
        // tests/corpus.py's ragged_rows.csv has too few regular lines for base.rs to see a delimiter: a milder one.
        let (_, d) = dg(b"a;b;c\n1;2;3\n4;5\n6;7;8\n9;10;11\n", 2);
        assert_eq!((d.observed, d.findings[0].kind, d.repair.unwrap().fields), (2, "missing-fields", vec!["4".to_string(), "5".into(), "".into()]));
        let (_, d) = dg(b"a;b;\n1;2;\n", 1);
        assert_eq!(kinds(&d), Vec::<&str>::new());
        let (_, d) = dg(b"a;b\n1;2;;\n", 1);
        assert_eq!((kinds(&d), d.repair.unwrap().kind), (vec!["trailing-delimiter"], RepairKind::Trim));
    }

    #[test]
    fn quotes() {
        assert!(kinds(&dg(b"a,b\n1,\"unclosed\n2,3\n", 1).1).contains(&"unclosed-quote"));
        let stray = b"a,b\n0,\"ok\"z \"w\"\n1,x \"quoted\" y\n2,\"ok\"z\n3,4\n";
        assert_eq!(kinds(&dg(stray, 1).1), vec!["text-after-quote"]);
        assert_eq!(kinds(&dg(stray, 2).1), vec!["quote-inside"]);
        assert_eq!(kinds(&dg(stray, 3).1), vec!["runaway-quote", "unclosed-quote"]);
        assert!(kinds(&dg(b"a, b ,c\n1, \"two\" ,3\n", 1).1).contains(&"space-before-quote"));
        assert!(kinds(&dg(b"id,text\n1,\"line one\nline two\"\n2,plain\n", 1).1).contains(&"multiline"));
    }

    #[test]
    fn characters() {
        let (_, d) = dg(b"a,b\n1,x\x00y\n2,\x00\n", 1);
        assert_eq!(kinds(&d), vec!["nul"]);
        assert_eq!(d.findings[0].near.as_ref().unwrap().hit_hex(), "00");
        assert!(kinds(&dg("a;b\n1;\u{feff}x\n".as_bytes(), 1).1).contains(&"invisible"));
        assert!(kinds(&dg(b"a,b,c,d\n=1+1,+cmd|' /C calc'!A0,-2+3,@SUM(A1)\n", 1).1).contains(&"formula"));
        assert_eq!(kinds(&dg("produit;prix;qte\nA;1 234,50;3\nB;-0,5;1\n".as_bytes(), 2).1), Vec::<&str>::new());
    }

    #[test]
    fn encodings() {
        let mut mixed = "nom;ville\nZoé;Lyon\n".as_bytes().to_vec();
        mixed.extend_from_slice(b"Zo\xe9;Orl\xe9ans\n");
        let (mut t, d) = dg(&mixed, 1);
        assert_eq!(kinds(&d), vec!["utf8-line"]);
        assert_eq!(d.findings[0].near.as_ref().unwrap().hit_hex(), "C3 A9");
        let r = d.repair.clone().unwrap();
        assert_eq!((r.kind, r.fields.clone()), (RepairKind::Reencode, vec!["Zoé".to_string(), "Lyon".into()]));
        let id = t.st.data_id(0);
        t.set_cells(&r.cells(id, d.observed));
        assert_eq!(t.st.cell(id, 0), "Zoé");
        assert_eq!(kinds(&dg(&mixed, 2).1), vec!["cp1252"]);
        let mut u16 = vec![0xFF, 0xFE];
        crate::enc::utf8_to_utf16("a;b\r\n1;", false, &mut u16);
        u16.extend_from_slice(&[0x00, 0xD8]);
        crate::enc::utf8_to_utf16("x\r\n", false, &mut u16);
        assert!(kinds(&dg(&u16, 1).1).contains(&"replacement"));
        assert_eq!(kinds(&dg("id;txt\n1;👍🏽\n2;👨‍👩‍👧‍👦\n3;🇫🇷\n4;é\n".as_bytes(), 2).1), Vec::<&str>::new());
        let (_, d) = dg("a;b\n1;Ã©cole\n".as_bytes(), 1);
        assert_eq!(kinds(&d), vec!["mojibake"]);
        assert_eq!(d.findings[0].near.as_ref().unwrap().hit_hex(), "C3 83");
    }

    #[test]
    fn created_row_has_no_bytes() {
        let mut t = table(b"a;b\n1;2\n");
        let id = t.insert_rows(0, 1)[0];
        assert!(diagnose(&t.st, id).none.is_some());
    }
}
