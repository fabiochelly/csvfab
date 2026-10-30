//! Edits over many cells at once: find & replace, bulk edits, series, format
//! conversions, look-ups, clean-up, split / merge columns, the clipboard.
//!
//! Each one reads a snapshot (`State`) in parallel, over the row ids it is
//! given (the rows shown, or a selection), and returns only the cells whose
//! text actually changes — `(row, column, new text)` for `Table::set_cells`,
//! one undo step. Structural edits (split, merge, look-up, clean-up) return a
//! plan that their `apply_*` function carries out through `Table`'s methods.
//! Hot loops allocate nothing but the changed cells: a cell is read in place
//! (`State::with_cell`, or one split of a pristine record for many columns)
//! and rewritten into a per-thread buffer, copied out only when it differs.

pub mod clean;
pub mod clipboard;
pub mod columns;
pub mod convert_values;
pub mod lookup;
pub mod replace;
pub mod series;

#[cfg(test)]
mod bench;

use crate::table::{RowId, State, Table};
use rayon::prelude::*;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

/// A cell to set: (row, column, new text).
pub type Change = (RowId, usize, String);

/// Rows per parallel task: enough to amortise the scheduling, few enough
/// for a cancellation to be seen within a millisecond or so.
const CHUNK: usize = 4096;

/// How many changed cells a preview shows.
pub const EXAMPLES: usize = 8;

/// Runs `f` on consecutive ranges of `0..n` in parallel, each with a scratch
/// value of its thread; the results come back in order. None if cancelled.
pub(crate) fn par_chunks<S, T, F>(n: usize, cancel: &AtomicBool, f: F) -> Option<Vec<T>>
where
    S: Default,
    T: Send,
    F: Fn(Range<usize>, &mut S) -> T + Sync,
{
    let out = (0..n.div_ceil(CHUNK))
        .into_par_iter()
        .map_init(S::default, |s, k| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            Some(f(k * CHUNK..((k + 1) * CHUNK).min(n), s))
        })
        .collect::<Option<Vec<T>>>();
    if cancel.load(Ordering::Relaxed) { None } else { out }
}

/// Columns to visit, sorted, each once.
pub(crate) struct ColSet {
    list: Vec<usize>,
}

impl ColSet {
    pub(crate) fn new(cols: &[usize]) -> ColSet {
        let mut list = cols.to_vec();
        list.sort_unstable();
        list.dedup();
        ColSet { list }
    }
}

/// Runs `f(column, text)` on the columns of `cols` in row `id`, in column
/// order ("" past the end of a short row): the record split once for all of them.
pub(crate) fn visit_cells(st: &State, id: RowId, cols: &ColSet, mut f: impl FnMut(usize, &str)) {
    st.visit_cols(id, &cols.list, |k, v| f(cols.list[k], v));
}

/// What an operation makes of one cell.
pub enum Outcome {
    /// Nothing to do (empty, not concerned, or unchanged).
    Skip,
    /// Recognised and already as wanted (a conversion counts those).
    Same,
    Change(String),
    /// Not recognised: left as it is, counted and shown.
    Bad,
}

/// A per-cell rewrite, run in parallel by `scan`.
pub trait CellOp: Sync {
    type Scratch: Default + Send;
    /// Whether the row can have a cell to change at all: a cheap test on the
    /// record's bytes lets most rows be skipped unsplit.
    fn row_may_change(&self, _st: &State, _id: RowId, _s: &mut Self::Scratch) -> bool {
        true
    }
    fn cell(&self, v: &str, col: usize, s: &mut Self::Scratch) -> Outcome;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    pub id: RowId,
    pub col: usize,
    pub old: String,
    pub new: String,
}

/// What an operation would do / did over the rows given.
#[derive(Debug, Default)]
pub struct Scan {
    /// Every change, in row order (empty for a preview).
    pub changes: Vec<Change>,
    pub changed: usize,
    /// The first EXAMPLES changes, with their old text.
    pub examples: Vec<Example>,
    pub same: usize,
    pub bad: usize,
    /// The first distinct values not recognised, up to 4: a 4th means "…".
    pub bad_examples: Vec<String>,
}

impl Scan {
    fn absorb(&mut self, p: Scan) {
        self.changes.extend(p.changes);
        self.changed += p.changed;
        self.same += p.same;
        self.bad += p.bad;
        let room = EXAMPLES - self.examples.len();
        self.examples.extend(p.examples.into_iter().take(room));
        for v in p.bad_examples {
            if self.bad_examples.len() < 4 && !self.bad_examples.contains(&v) {
                self.bad_examples.push(v);
            }
        }
    }
}

/// Runs `op` over `cols` of the rows `ids`. `collect`: keep every change
/// (to apply), else only count them and keep examples (a preview).
pub fn scan<O: CellOp>(st: &State, ids: &[RowId], cols: &[usize], op: &O, collect: bool, cancel: &AtomicBool) -> Option<Scan> {
    let set = ColSet::new(cols);
    if set.list.is_empty() {
        return Some(Scan::default());
    }
    let parts = par_chunks(ids.len(), cancel, |r, sc: &mut O::Scratch| {
        let mut p = Scan::default();
        for &id in &ids[r] {
            if !op.row_may_change(st, id, sc) {
                continue;
            }
            visit_cells(st, id, &set, |col, v| match op.cell(v, col, sc) {
                Outcome::Skip => {}
                Outcome::Same => p.same += 1,
                Outcome::Bad => {
                    p.bad += 1;
                    if p.bad_examples.len() < 4 && !p.bad_examples.iter().any(|b| b == v) {
                        p.bad_examples.push(v.to_string());
                    }
                }
                Outcome::Change(new) => {
                    p.changed += 1;
                    if p.examples.len() < EXAMPLES {
                        p.examples.push(Example { id, col, old: v.to_string(), new: new.clone() });
                    }
                    if collect {
                        p.changes.push((id, col, new));
                    }
                }
            });
        }
        p
    })?;
    let mut out = Scan::default();
    for p in parts {
        out.absorb(p);
    }
    Some(out)
}

/// Every change `op` makes over `cols` of the rows `ids`; None if cancelled.
pub fn changes<O: CellOp>(st: &State, ids: &[RowId], cols: &[usize], op: &O, cancel: &AtomicBool) -> Option<Vec<Change>> {
    scan(st, ids, cols, op, true, cancel).map(|s| s.changes)
}

/// Every data row, in the table's order.
pub fn all_rows(st: &State) -> Vec<RowId> {
    (0..st.len()).into_par_iter().with_min_len(CHUNK).map(|p| st.data_id(p)).collect()
}

/// Several `Table` calls as one undo step. `Table` cannot merge its undo
/// steps yet: until it can (see the module's notes), each call is its own step.
pub(crate) fn as_one_step(t: &mut Table, f: impl FnOnce(&mut Table)) {
    t.batch(f)
}

/* ---- Text as the web app's JavaScript sees it ---------------------- */

/// JavaScript's `\s` (and what `trim()` removes): Unicode spaces, line
/// terminators and the BOM — not U+0085, which Rust's `is_whitespace` has.
#[inline]
pub fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[inline]
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// Unicode's format characters (`\p{Cf}`): zero-width spaces and joiners,
/// direction marks, the BOM, the soft hyphen…
#[inline]
pub fn is_cf(c: char) -> bool {
    let u = c as u32;
    u >= 0xAD
        && matches!(u, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E | 0x200B..=0x200F
            | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD
            | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// A count as the app writes it (`toLocaleString('fr-FR')`): 1 234 567,
/// grouped with narrow no-break spaces.
pub fn fmt_int(n: usize) -> String {
    let d = n.to_string();
    let mut out = String::with_capacity(d.len() + d.len() / 3 * 3);
    for (i, ch) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(ch);
    }
    out
}

/// "1 cell", "2 cells".
pub fn plural(n: usize, word: &str) -> String {
    format!("{} {word}{}", fmt_int(n), if n == 1 { "" } else { "s" })
}

/// The status line after an edit kept in memory.
pub fn not_saved(what: &str) -> String {
    format!("{what} — not written yet, use Save.")
}

/// The app's slug of a text: lower case, accents and ligatures undone, any
/// run of other than letters and digits a single space ("Code postal" and
/// "code_postal" are both "code postal").
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    slugify_into(s, &mut out);
    out
}

pub fn slugify_into(s: &str, out: &mut String) {
    out.clear();
    let mut gap = false;
    let mut put = |c: char, out: &mut String| {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    };
    for c in s.chars() {
        if c.is_ascii() {
            put(c.to_ascii_lowercase(), out);
            continue;
        }
        for l in c.to_lowercase() {
            match l {
                'œ' => { put('o', out); put('e', out) }
                'æ' => { put('a', out); put('e', out) }
                'ß' => { put('s', out); put('s', out) }
                'ø' => put('o', out),
                'đ' | 'ð' => put('d', out),
                'ł' => put('l', out),
                'þ' => { put('t', out); put('h', out) }
                _ => unicode_normalization::char::decompose_canonical(l, |d| {
                    if !('\u{300}'..='\u{36f}').contains(&d) {
                        put(d, out)
                    }
                }),
            }
        }
    }
}

/// `Number.prototype.toFixed`: rounded on the double's exact value, a tie
/// going up (Rust's formatter rounds a tie to even).
pub fn to_fixed(x: f64, digits: usize) -> String {
    // A whole number needs no rounding (series of ids, counts): the fast path.
    if x.fract() == 0.0 && x.abs() < 9e15 {
        let i = x.abs() as u64;
        let mut s = if x < 0.0 { format!("-{i}") } else { i.to_string() };
        if digits > 0 {
            s.push('.');
            s.extend(std::iter::repeat_n('0', digits));
        }
        return s;
    }
    // Thirty more digits than kept: enough to tell a true tie of the exact value.
    let r = round_digits(&format!("{:.*}", digits + 30, x.abs()), digits);
    if x < 0.0 { format!("-{r}") } else { r }
}

/// A decimal string ("123.4567", no sign) rounded half up to `digits` places.
pub(crate) fn round_digits(s: &str, digits: usize) -> String {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut d: Vec<u8> = int.bytes().chain(frac.bytes().chain(std::iter::repeat(b'0')).take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|&c| c >= b'5') {
        let mut i = d.len();
        loop {
            if i == 0 {
                d.insert(0, b'1');
                break;
            }
            i -= 1;
            if d[i] == b'9' {
                d[i] = b'0';
            } else {
                d[i] += 1;
                break;
            }
        }
    }
    let cut = d.len() - digits;
    let mut out = String::from_utf8(d[..cut].to_vec()).unwrap();
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&d[cut..]).unwrap());
    }
    out
}

/// `String(x)` for a number: the shortest text that reads back the same, no
/// exponent below 1e21, "0" for -0.
pub fn js_num(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    if x.is_nan() {
        return "NaN".into();
    }
    format!("{x}")
}

/// `Math.round`: halves go up, -2.5 → -2.
#[inline]
pub fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use std::sync::Arc;

    pub fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    pub fn col(t: &Table, c: usize) -> Vec<String> {
        (0..t.st.len()).map(|p| t.st.cell(t.st.data_id(p), c)).collect()
    }

    pub static NO: AtomicBool = AtomicBool::new(false);

    #[test]
    fn cf_table_matches_unicode() {
        let re = regex::Regex::new(r"^\p{Cf}$").unwrap();
        let mut buf = [0u8; 4];
        for u in 0..=0x10FFFFu32 {
            if let Some(c) = char::from_u32(u) {
                assert_eq!(is_cf(c), re.is_match(c.encode_utf8(&mut buf)), "U+{u:04X}");
            }
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(fmt_int(1234567), "1\u{202f}234\u{202f}567");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(plural(1, "cell"), "1 cell");
        assert_eq!(slugify("  Code_Postal (Ville) "), "code postal ville");
        assert_eq!(slugify("Œuvre Éléonore Straße"), "oeuvre eleonore strasse");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(-1.25, 1), "-1.3");
        assert_eq!(to_fixed(9.995, 2), "9.99");
        assert_eq!(to_fixed(99.5, 0), "100");
        assert_eq!(js_num(-0.0), "0");
        assert_eq!(js_num(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_trim("\u{feff} a\u{a0}"), "a");
    }

    #[test]
    fn scan_many_columns_and_short_rows() {
        let t = table("a,b,c,d\n1,2,3,4\n5,6\n");
        struct Up;
        impl CellOp for Up {
            type Scratch = ();
            fn cell(&self, v: &str, _: usize, _: &mut ()) -> Outcome {
                if v.is_empty() { Outcome::Bad } else { Outcome::Change(format!("<{v}>")) }
            }
        }
        let ids = all_rows(&t.st);
        let s = scan(&t.st, &ids, &[3, 0, 2], &Up, true, &NO).unwrap();
        assert_eq!(s.changes.len(), 4);
        assert_eq!(s.changes[0], (ids[0], 0, "<1>".into()));
        assert_eq!(s.bad, 2);
        assert_eq!(s.bad_examples, vec![""]);
    }
}
