//! Noise words: a list of words (or phrases) taken out of one column or of
//! every column, in the rows shown — "SARL", "Monsieur", "et Fils"… Whole
//! words by default, case and accents ignored, and the gap each one leaves is
//! tidied (doubled spaces, a comma left dangling). The changes are cells for
//! `Table::set_cells`: one undo step.

use crate::table::{RowId, State};
use fancy_regex::{Captures, Regex};
use rayon::prelude::*;
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

pub const PRESETS: [(&str, &str); 3] = [
    ("Legal forms", "SARL, S.A.R.L., SAS, S.A.S., SASU, SA, S.A., EURL, SCI, SNC, SCOP, SELARL, EIRL, Ltd, LLC, Inc, GmbH"),
    ("Titles", "Monsieur, Madame, Mademoiselle, M., Mme, Mlle, Mr, Mrs, Ms, Dr"),
    ("French small words", "le, la, les, l', de, du, des, d', et, un, une, au, aux"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    pub caseless: bool,
    pub accents: bool,
    pub whole: bool,
    pub tidy: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { caseless: true, accents: true, whole: true, tidy: true }
    }
}

fn remove_accents(s: &str) -> String {
    s.nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect()
}

/// Every precomposed letter whose base is a given ASCII letter, so an
/// accent-blind pattern matches "Société" from "societe" without rewriting
/// the cell (its own spelling must stay byte for byte).
fn accent_classes() -> &'static Vec<(char, String)> {
    static C: OnceLock<Vec<(char, String)>> = OnceLock::new();
    C.get_or_init(|| {
        let mut out: Vec<(char, String)> = Vec::new();
        for ch in (0xC0..=0x24F).filter_map(char::from_u32) {
            let base = remove_accents(&ch.to_string());
            let mut it = base.chars();
            let (Some(b), None) = (it.next(), it.next()) else { continue };
            if b != ch && b.is_ascii_alphabetic() {
                match out.iter_mut().find(|e| e.0 == b) {
                    Some(e) => e.1.push(ch),
                    None => out.push((b, format!("{b}{ch}"))),
                }
            }
        }
        out
    })
}

/// The list as typed: one per line, or separated by commas / semicolons; each once (case aside).
pub fn words(text: &str) -> Vec<String> {
    let mut seen = Vec::new();
    text.split(['\n', ',', ';'])
        .map(|w| w.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|w| !w.is_empty() && { let k = w.to_lowercase(); !seen.contains(&k) && { seen.push(k); true } })
        .collect()
}

fn esc(c: char) -> String {
    if ".*+?^${}()|[]\\/".contains(c) { format!("\\{c}") } else { c.to_string() }
}

const NZ: char = '\u{0}';

struct Tidy {
    runs: Regex,
    dup: Regex,
    pair: Regex,
    head: Regex,
    tail: Regex,
    mid: Regex,
}

/// The gaps a removal leaves, around what was removed (marked NUL).
fn tidy_res() -> &'static Tidy {
    static T: OnceLock<Tidy> = OnceLock::new();
    T.get_or_init(|| {
        let sep = r"[,;:/|·\-–—]";
        let re = |s: String| Regex::new(&s).unwrap();
        Tidy {
            runs: re(r"\x00(?:[ \t]*\x00)+".into()),
            dup: re(format!(r"({sep})([ \t]*)\x00[ \t]*\1")),
            pair: re(r"\([ \t]*\x00[ \t]*\)|\[[ \t]*\x00[ \t]*\]|\{[ \t]*\x00[ \t]*\}".into()),
            head: re(format!(r"^\s*\x00\s*(?:{sep}[ \t]*)?")),
            tail: re(format!(r"(?:\s*{sep})?\s*\x00\s*$")),
            mid: re(r"([ \t]*)\x00([ \t]*)".into()),
        }
    })
}

/// Words compiled: one regex for the whole list, longest first ("et Cie"
/// before "et"). Whole words: a boundary only on a side where the word ends
/// in a letter or digit, so "M." and "d'" match before what follows them;
/// hyphens are boundaries ("Saint-Martin-de-Ré" loses its "de").
pub struct Noise {
    re: Regex,
    /// The same without its boundaries: a cheap first test in linear time.
    quick: regex::Regex,
    tidy: bool,
}

impl Noise {
    pub fn new(words: &[String], o: Opts) -> Option<Noise> {
        if words.is_empty() { return None; }
        let cls = o.accents.then(accent_classes);
        let part = |w: &str| -> String {
            w.chars().map(|ch| {
                if ch.is_whitespace() { return r"\s+".to_string(); }
                let key = remove_accents(&if o.caseless { ch.to_lowercase().to_string() } else { ch.to_string() });
                let set = cls.and_then(|c| { let mut k = key.chars(); match (k.next(), k.next()) { (Some(b), None) => c.iter().find(|e| e.0 == b), _ => None } });
                match set {
                    Some((_, s)) => format!(r"[{s}]\p{{M}}*"),
                    None => esc(ch) + if cls.is_some() && ch.is_alphabetic() { r"\p{M}*" } else { "" },
                }
            }).collect()
        };
        let word_ch = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let mut sorted = words.to_vec();
        sorted.sort_by_key(|w| std::cmp::Reverse(w.encode_utf16().count()));
        let w = r"[\p{L}\p{N}_]";
        let alt: Vec<String> = sorted.iter().map(|x| {
            let p = part(x);
            if !o.whole { return p; }
            format!("{}{p}{}", if word_ch(x.chars().next()) { format!("(?<!{w})") } else { String::new() }, if word_ch(x.chars().last()) { format!("(?!{w})") } else { String::new() })
        }).collect();
        let flags = if o.caseless { "(?i)" } else { "" };
        let quick = sorted.iter().map(|x| part(x)).collect::<Vec<_>>().join("|");
        Some(Noise {
            re: Regex::new(&format!("{flags}(?:{})", alt.join("|"))).ok()?,
            quick: regex::Regex::new(&format!("{flags}(?:{quick})")).ok()?,
            tidy: o.tidy,
        })
    }

    /// Where the words are in `v`, as byte ranges (a preview marks them).
    pub fn find(&self, v: &str) -> Vec<(usize, usize)> {
        if !self.quick.is_match(v) { return Vec::new(); }
        self.re.find_iter(v).filter_map(Result::ok).map(|m| (m.start(), m.end())).collect()
    }

    /// `v` without the words. "Dupont SARL, Paris" → "Dupont, Paris",
    /// "SARL - Dupont" → "Dupont", "(SARL) Dupont" → "Dupont"; only around
    /// what was removed: the rest of the cell is not touched.
    pub fn strip<'a>(&self, v: &'a str) -> Cow<'a, str> {
        if !self.quick.is_match(v) || !self.re.is_match(v).unwrap_or(false) { return Cow::Borrowed(v); }
        if !self.tidy || v.contains(NZ) { return Cow::Owned(self.re.replace_all(v, "").into_owned()); }
        let t = tidy_res();
        let nz = NZ.to_string();
        let s = self.re.replace_all(v, nz.as_str()).into_owned();
        let s = t.runs.replace_all(&s, nz.as_str()).into_owned();
        let s = t.pair.replace_all(&s, nz.as_str()).into_owned();
        let s = t.runs.replace_all(&s, nz.as_str()).into_owned();
        let s = t.dup.replace_all(&s, "$1$2\u{0}").into_owned();
        let s = t.head.replace(&s, "").into_owned();
        let s = t.tail.replace(&s, "").into_owned();
        let out = t.mid.replace_all(&s, |c: &Captures<str>| {
            let m = c.get(0).unwrap();
            let (prev, next) = (s[..m.start()].chars().next_back(), s[m.end()..].chars().next());
            let (a, b) = (c.get(1).map_or("", |x| x.as_str()), c.get(2).map_or("", |x| x.as_str()));
            match (prev, next) {
                (None, _) | (_, None) => String::new(),
                (_, Some(n)) if ",;:.!?)]}»".contains(n) => b.to_string(),   // "Dupont SARL, Paris"; a French " ;" keeps its space
                (Some(p), _) if "([{«".contains(p) => a.to_string(),
                _ => if a.is_empty() && b.is_empty() { String::new() } else { " ".into() },
            }
        });
        Cow::Owned(out.into_owned())
    }
}

/// What taking the words out would change in the rows given.
#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub cells: usize,
    pub rows: usize,
    /// (column, before, after): the first 8.
    pub examples: Vec<(usize, String, String)>,
    /// The cells, for `Table::set_cells`.
    pub changes: Vec<(RowId, usize, String)>,
}

/// The cells of `rows` (in one column, or all) that would change; None if cancelled.
pub fn scan(st: &State, rows: &[RowId], col: Option<usize>, noise: &Noise, cancel: &AtomicBool) -> Option<Scan> {
    let parts: Vec<Vec<(RowId, usize, String, String)>> = rows.par_chunks(4096).map(|chunk| {
        let mut out = Vec::new();
        if cancel.load(Ordering::Relaxed) { return out; }
        for &id in chunk {
            st.visit(id, |c, v| {
                if col.is_some_and(|k| k != c) || v.is_empty() { return; }
                if let Cow::Owned(n) = noise.strip(v) { if n != v { out.push((id, c, v.to_string(), n)); } }
            });
        }
        out
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut s = Scan::default();
    let mut last = None;
    for (id, c, old, new) in parts.into_iter().flatten() {
        s.cells += 1;
        if last != Some(id) { s.rows += 1; last = Some(id); }
        if s.examples.len() < 8 { s.examples.push((c, old, new.clone())); }
        s.changes.push((id, c, new));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    #[test]
    fn rules() {
        let d = Opts::default();
        let cases: [(&str, &str, Opts, &str); 17] = [
            ("Dupont SARL, Paris", "SARL", d, "Dupont, Paris"),
            ("SARL Dupont", "SARL", d, "Dupont"),
            ("Dupont - SARL", "SARL", d, "Dupont"),
            ("Dupont, SARL, Paris", "SARL", d, "Dupont, Paris"),
            ("Dupont SARL ; Paris", "SARL", d, "Dupont ; Paris"),
            ("(SARL) Dupont", "SARL", d, "Dupont"),
            ("Les Etablissements Dupont", "les\nétablissements", d, "Dupont"),
            ("M. Jean Dupont", "M.", d, "Jean Dupont"),
            ("Dupont ET CIE", "et, et cie", d, "Dupont"),
            ("Société d'Exploitation", "d'", d, "Société Exploitation"),
            ("Sarlat", "SARL", d, "Sarlat"),
            ("SARL", "SARL", d, ""),
            ("garde  ses   espaces SARL", "SARL", d, "garde  ses   espaces"),
            ("Le chat LE", "Le", Opts { caseless: false, ..d }, "chat LE"),
            ("Société", "societe", Opts { accents: false, ..d }, "Société"),
            ("Dupont® A-B", "®; -", Opts { whole: false, ..d }, "Dupont AB"),
            ("Dupont SARL, Paris", "SARL", Opts { tidy: false, ..d }, "Dupont , Paris"),
        ];
        for (v, w, o, want) in cases {
            let n = Noise::new(&words(w), o).unwrap();
            assert_eq!(n.strip(v), want, "{v:?} − {w:?}");
        }
    }

    #[test]
    fn one_column_then_every_column() {
        let mut t = Table::new(Arc::new(Base::from_bytes(b"nom;ville\r\nDupont SARL;Paris SARL\r\nMartin;Lyon\r\nDurand et Fils;Nantes\r\n".to_vec(), OpenOptions::default())), None);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let no = AtomicBool::new(false);
        let s = scan(&t.st, &rows, Some(0), &Noise::new(&words("SARL, et fils"), Opts::default()).unwrap(), &no).unwrap();
        assert_eq!((s.cells, s.rows), (2, 2));
        t.set_cells(&s.changes);
        assert_eq!((t.st.cell(rows[0], 0), t.st.cell(rows[0], 1), t.st.cell(rows[2], 0)), ("Dupont".into(), "Paris SARL".into(), "Durand".into()));
        t.undo();
        let s = scan(&t.st, &rows, None, &Noise::new(&words("sarl"), Opts::default()).unwrap(), &no).unwrap();
        assert_eq!((s.cells, s.rows), (2, 1));
        assert_eq!(s.examples[1], (1, "Paris SARL".into(), "Paris".into()));
    }
}
