//! Clean up: one pass over every cell (filtered or not) and the header line —
//! garbled accents, invisible characters, odd spaces, line breaks — then the
//! empty rows and columns.

use super::{all_rows, as_one_step, is_cf, js_space, js_trim, not_saved, par_chunks, plural, visit_cells, Change, ColSet};
use crate::table::{RowId, State, Table};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

/// The options, in the order they are applied: mojibake before the spaces,
/// since "à" garbled is "Ã" + a no-break space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CleanOp {
    Moji,
    Invis,
    Nbsp,
    Breaks,
    Collapse,
    Trim,
    Rows,
    Cols,
}

/// Each option, its label and whether it is ticked at first.
pub const CLEAN_OPS: [(CleanOp, &str, bool); 8] = [
    (CleanOp::Moji, "Repair garbled accents (Ã© → é, â€™ → ’)", true),
    (CleanOp::Invis, "Remove invisible characters (zero-width, direction marks, stray BOM)", true),
    (CleanOp::Nbsp, "No-break and narrow spaces → plain spaces", true),
    (CleanOp::Breaks, "Line breaks inside cells → spaces", false),
    (CleanOp::Collapse, "Collapse repeated spaces", false),
    (CleanOp::Trim, "Trim spaces at both ends of each cell", true),
    (CleanOp::Rows, "Delete empty rows", true),
    (CleanOp::Cols, "Delete empty columns (header aside)", false),
];

/// The cell options.
const CELL_OPS: [CleanOp; 6] = [CleanOp::Moji, CleanOp::Invis, CleanOp::Nbsp, CleanOp::Breaks, CleanOp::Collapse, CleanOp::Trim];

impl CleanOp {
    pub fn index(self) -> usize {
        CLEAN_OPS.iter().position(|o| o.0 == self).unwrap()
    }

    /// "12 cells", "1 row", "—"…: what the option would touch.
    pub fn count_label(self, n: usize) -> String {
        let unit = match self { CleanOp::Rows => "row", CleanOp::Cols => "column", _ => "cell" };
        if n == 0 { "—".into() } else { plural(n, unit) }
    }
}

/* ---- Mojibake ------------------------------------------------------- */

/// What bytes 0x80–0xFF read as in Windows-1252 (WHATWG: every byte maps).
fn cp1252_high() -> &'static [char; 128] {
    static T: OnceLock<[char; 128]> = OnceLock::new();
    T.get_or_init(|| {
        let bytes: Vec<u8> = (0x80..=0xFF).collect();
        let s = encoding_rs::WINDOWS_1252.decode_without_bom_handling(&bytes).0;
        let mut t = ['\0'; 128];
        for (i, c) in s.chars().enumerate() {
            t[i] = c;
        }
        t
    })
}

/// The 1252 byte that reads as `c`, for c not ASCII.
#[inline]
fn byte_of(c: char) -> Option<u8> {
    if ('\u{a0}'..='\u{ff}').contains(&c) {
        return Some(c as u8);
    }
    cp1252_high()[..32].iter().position(|&x| x == c).map(|i| 0x80 + i as u8)
}

/// UTF-8 bytes read as Windows-1252 upstream ("Ã©" for é, "â€™" for ’): a
/// lead byte followed by its continuation bytes, as 1252 characters, is
/// decoded back on its own — a cell mixing sound and garbled accents is
/// repaired too. A decoded character is kept only in the ranges mojibake of
/// Western text lands in: "voilà »" with its no-break space is also
/// à + A0 + BB, a valid sequence for a Samaritan letter. Calls `f` on each
/// match with its replacement (None: kept as is); stops when `f` says so.
fn moji_matches(s: &str, mut f: impl FnMut(usize, usize, Option<char>) -> bool) {
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let need = match byte_of(c) {
            Some(0xC2..=0xDF) => 1,
            Some(0xE0..=0xEF) => 2,
            Some(0xF0..=0xF4) => 3,
            _ => continue,
        };
        let mut buf = [byte_of(c).unwrap(), 0, 0, 0];
        let mut look = it.clone();
        let mut end = i + c.len_utf8();
        let mut ok = true;
        for k in 1..=need {
            match look.next().and_then(|(j, x)| byte_of(x).filter(|b| (0x80..=0xBF).contains(b)).map(|b| (j, x, b))) {
                Some((j, x, b)) => { buf[k] = b; end = j + x.len_utf8(); }
                None => { ok = false; break; }
            }
        }
        if !ok {
            continue;
        }
        it = look;
        let ch = std::str::from_utf8(&buf[..=need]).ok().and_then(|d| d.chars().next())
            .filter(|&d| matches!(d as u32, 0xA0..=0x24F | 0x2000..=0x214F | 0x1F000..));
        if !f(i, end, ch) {
            return;
        }
    }
}

/// Mojibake repaired; up to three passes, for text garbled twice ("ÃƒÂ©").
pub fn fix_mojibake(s: &str) -> std::borrow::Cow<'_, str> {
    let mut cur = std::borrow::Cow::Borrowed(s);
    for _ in 0..3 {
        if !cur.chars().any(|c| ('\u{c2}'..='\u{f4}').contains(&c)) {
            break;
        }
        let (mut out, mut last, mut changed) = (String::new(), 0, false);
        moji_matches(&cur, |a, b, ch| {
            if let Some(ch) = ch {
                out.push_str(&cur[last..a]);
                out.push(ch);
                last = b;
                changed = true;
            }
            true
        });
        if !changed {
            break;
        }
        out.push_str(&cur[last..]);
        cur = std::borrow::Cow::Owned(out);
    }
    cur
}

/// Whether some text (a whole file's, decoded) holds mojibake: stops at the
/// first match that really decodes, or after 1 000 that don't.
pub fn has_mojibake(text: &str) -> bool {
    let (mut found, mut tries) = (false, 0);
    moji_matches(text, |_, _, ch| {
        found = ch.is_some();
        tries += 1;
        !found && tries <= 1000
    });
    found
}

/* ---- The cell options ----------------------------------------------- */

fn is_nbsp(c: char) -> bool {
    matches!(c, '\u{a0}' | '\u{2007}' | '\u{202f}')
}

/// Whether option `op` would change `v` (the counts of the dialog).
fn touches(op: CleanOp, v: &str) -> bool {
    let ascii = v.is_ascii();
    match op {
        CleanOp::Moji => !ascii && matches!(fix_mojibake(v), std::borrow::Cow::Owned(_)),
        CleanOp::Invis => !ascii && v.chars().any(is_cf),
        CleanOp::Nbsp => !ascii && v.chars().any(is_nbsp),
        CleanOp::Breaks => v.contains(['\r', '\n']),
        CleanOp::Collapse => v.contains("  "),
        CleanOp::Trim => v.starts_with(js_space) || v.ends_with(js_space),
        CleanOp::Rows | CleanOp::Cols => false,
    }
}

/// Option `op` applied to `v` into `out`; false (out untouched) when it changes nothing.
fn step(op: CleanOp, v: &str, out: &mut String) -> bool {
    if !touches(op, v) {
        return false;
    }
    out.clear();
    match op {
        CleanOp::Moji => out.push_str(&fix_mojibake(v)),
        CleanOp::Invis => out.extend(v.chars().filter(|&c| !is_cf(c))),
        CleanOp::Nbsp => out.extend(v.chars().map(|c| if is_nbsp(c) { ' ' } else { c })),
        CleanOp::Breaks => {
            // [ \t]*[\r\n]+[ \t]* → one space
            let mut rest = v;
            while let Some(k) = rest.find(['\r', '\n']) {
                out.push_str(rest[..k].trim_end_matches([' ', '\t']));
                out.push(' ');
                rest = rest[k..].trim_start_matches(['\r', '\n']).trim_start_matches([' ', '\t']);
            }
            out.push_str(rest);
        }
        CleanOp::Collapse => {
            let mut prev = '\0';
            for c in v.chars() {
                if !(c == ' ' && prev == ' ') {
                    out.push(c);
                }
                prev = c;
            }
        }
        CleanOp::Trim => out.push_str(js_trim(v)),
        CleanOp::Rows | CleanOp::Cols => return false,
    }
    out != v
}

/// Spaces and invisible characters only.
pub fn is_blank(v: &str) -> bool {
    v.chars().all(|c| js_space(c) || is_cf(c))
}

/// The ticked cell options applied in order to `v`: Some(new text) if it changes.
fn clean_cell(ops: &[CleanOp], v: &str, a: &mut String, b: &mut String) -> Option<String> {
    let mut changed = false;
    for &op in ops {
        let src = if changed { &*a } else { v };
        if step(op, src, b) {
            std::mem::swap(a, b);
            changed = true;
        }
    }
    (changed && a != v).then(|| a.clone())
}

/// Per option (in CLEAN_OPS's order) how many cells, rows or columns it would
/// touch; and which columns hold nothing but blanks (header aside).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CleanScan {
    pub counts: [usize; 8],
    pub empty_cols: Vec<usize>,
}

impl CleanScan {
    pub fn count(&self, op: CleanOp) -> usize {
        self.counts[op.index()]
    }
}

/// Counted once when the dialog opens: every data row, and the header.
pub fn scan(st: &State, cancel: &AtomicBool) -> Option<CleanScan> {
    let ids = all_rows(st);
    let cols = ColSet::new(&(0..st.ncols).collect::<Vec<_>>());
    let n = st.ncols;
    let count = |v: &str, counts: &mut [usize; 8]| {
        if !v.is_empty() {
            for op in CELL_OPS {
                counts[op.index()] += touches(op, v) as usize;
            }
        }
    };
    let parts = par_chunks(ids.len(), cancel, |r, _: &mut ()| {
        let (mut counts, mut filled) = ([0usize; 8], vec![false; n]);
        for &id in &ids[r] {
            let mut blank = true;
            visit_cells(st, id, &cols, |c, v| {
                count(v, &mut counts);
                if !is_blank(v) {
                    blank = false;
                    filled[c] = true;
                }
            });
            counts[CleanOp::Rows.index()] += blank as usize;
        }
        (counts, filled)
    })?;
    let mut out = CleanScan::default();
    let mut filled = vec![false; n];
    for (c, f) in parts {
        for k in 0..8 {
            out.counts[k] += c[k];
        }
        filled.iter_mut().zip(f).for_each(|(a, b)| *a |= b);
    }
    if st.header && st.all_len() > 0 {
        visit_cells(st, st.all_id(0), &cols, |_, v| count(v, &mut out.counts));
    }
    out.empty_cols = (0..n).filter(|&c| !filled[c]).collect();
    out.counts[CleanOp::Cols.index()] = out.empty_cols.len();
    Some(out)
}

pub fn scan_note(s: &CleanScan, on: &[CleanOp]) -> &'static str {
    if on.iter().any(|&o| s.count(o) > 0) {
        "Counts are per option: a cell can need several. Nothing is written until Save."
    } else {
        "Nothing to clean with these options."
    }
}

/// What Apply does.
#[derive(Debug, Default)]
pub struct CleanPlan {
    /// Cells cleaned, the header's included.
    pub changes: Vec<Change>,
    pub rows: HashSet<RowId>,
    pub cols: Vec<usize>,
}

/// `on`: the ticked options; `s`: the scan made at opening — the empty columns
/// it found stay empty, no cell option can fill one.
pub fn plan(st: &State, on: &[CleanOp], s: &CleanScan, cancel: &AtomicBool) -> Option<CleanPlan> {
    let ops: Vec<CleanOp> = CELL_OPS.iter().copied().filter(|o| on.contains(o)).collect();
    let rows = on.contains(&CleanOp::Rows);
    let ids = all_rows(st);
    let cols = ColSet::new(&(0..st.ncols).collect::<Vec<_>>());
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (String, String)| {
        let (mut changes, mut empty) = (Vec::new(), Vec::new());
        for &id in &ids[r] {
            let mut blank = true;
            visit_cells(st, id, &cols, |c, v| {
                let new = if v.is_empty() || ops.is_empty() { None } else { clean_cell(&ops, v, &mut s.0, &mut s.1) };
                blank &= is_blank(new.as_deref().unwrap_or(v));
                if let Some(new) = new {
                    changes.push((id, c, new));
                }
            });
            if rows && blank {
                empty.push(id);
            }
        }
        (changes, empty)
    })?;
    let mut p = CleanPlan { cols: if on.contains(&CleanOp::Cols) { s.empty_cols.clone() } else { Vec::new() }, ..Default::default() };
    if st.header && st.all_len() > 0 && !ops.is_empty() {
        let (h, mut a, mut b) = (st.all_id(0), String::new(), String::new());
        visit_cells(st, h, &cols, |c, v| {
            if let Some(new) = clean_cell(&ops, v, &mut a, &mut b) {
                p.changes.push((h, c, new));
            }
        });
    }
    for (c, e) in parts {
        p.changes.extend(c);
        p.rows.extend(e);
    }
    Some(p)
}

pub const NOTHING_TO_CLEAN: &str = "Nothing to clean.";

/// Carries the plan out; returns the status line.
pub fn apply(t: &mut Table, p: CleanPlan) -> String {
    if p.changes.is_empty() && p.rows.is_empty() && p.cols.is_empty() {
        return NOTHING_TO_CLEAN.into();
    }
    let mut parts = Vec::new();
    if !p.changes.is_empty() {
        parts.push(plural(p.changes.len(), "cell") + " cleaned");
    }
    if !p.rows.is_empty() {
        parts.push(plural(p.rows.len(), "empty row") + " deleted");
    }
    if !p.cols.is_empty() {
        parts.push(plural(p.cols.len(), "empty column") + " deleted");
    }
    as_one_step(t, |t| {
        t.set_cells(&p.changes);
        t.delete_rows(&p.rows);
        if !p.cols.is_empty() {
            t.delete_columns(&p.cols);
        }
    });
    not_saved(&parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::tests::{col, table, NO};

    #[test]
    fn mojibake() {
        assert_eq!(fix_mojibake("Ã©tÃ© â€™ Ã‰mile"), "été ’ Émile");
        assert_eq!(fix_mojibake("ÃƒÂ©"), "é");
        assert_eq!(fix_mojibake("déjà Ã©"), "déjà é");
        // à + no-break space + » decodes to a Samaritan letter: kept as is.
        assert_eq!(fix_mojibake("voilà\u{a0}»"), "voilà\u{a0}»");
        assert_eq!(fix_mojibake("Ã"), "Ã");
        assert!(matches!(fix_mojibake("plain"), std::borrow::Cow::Borrowed(_)));
        assert!(has_mojibake("x Ã© y") && !has_mojibake("voilà\u{a0}» é"));
    }

    #[test]
    fn cell_steps() {
        let all = &CELL_OPS[..];
        let (mut a, mut b) = (String::new(), String::new());
        let mut c = |v: &str, ops: &[CleanOp]| clean_cell(ops, v, &mut a, &mut b);
        assert_eq!(c(" Ã\u{a0}\u{200b}x\u{a0}y ", all).as_deref(), Some("àx y"));
        assert_eq!(c("a \t\r\n\r\n  b\nc", &[CleanOp::Breaks]).as_deref(), Some("a b c"));
        assert_eq!(c("a    b", &[CleanOp::Collapse]).as_deref(), Some("a b"));
        assert_eq!(c("ok", all), None);
        assert!(is_blank(" \u{feff}\u{200b}") && !is_blank(" x"));
    }

    #[test]
    fn whole_file() {
        let mut t = table("Nom ;Ville;Vide;Code\nCafÃ©; Lyon ;;1\n \u{200b};;  ;\nBob;Paris\u{a0};;2\n");
        let s = scan(&t.st, &NO).unwrap();
        assert_eq!(s.count(CleanOp::Moji), 1);
        assert_eq!(s.count(CleanOp::Invis), 1);
        assert_eq!(s.count(CleanOp::Nbsp), 1);
        assert_eq!(s.count(CleanOp::Trim), 5);
        assert_eq!(s.count(CleanOp::Rows), 1);
        assert_eq!(s.empty_cols, vec![2]);
        assert_eq!(CleanOp::Rows.count_label(1), "1 row");
        let on: Vec<CleanOp> = CLEAN_OPS.iter().map(|o| o.0).collect();
        let p = plan(&t.st, &on, &s, &NO).unwrap();
        assert_eq!(apply(&mut t, p), "6 cells cleaned, 1 empty row deleted, 1 empty column deleted — not written yet, use Save.");
        assert_eq!(t.st.headers(), vec!["Nom", "Ville", "Code"]);
        assert_eq!(col(&t, 0), vec!["Café", "Bob"]);
        assert_eq!(col(&t, 1), vec!["Lyon", "Paris"]);
        let s = scan(&t.st, &NO).unwrap();
        let p = plan(&t.st, &[], &s, &NO).unwrap();
        assert_eq!(apply(&mut t, p), NOTHING_TO_CLEAN);
    }
}
