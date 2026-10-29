//! Split a column into several, merge several into one. Every data row is
//! rewritten (filtered or not), the header gets the new titles.

use super::{as_one_step, fmt_int, js_space, js_trim, not_saved, par_chunks, Change};
use crate::table::{RowId, State, Table};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

/* ---- Split ---------------------------------------------------------- */

/// The dialog's chips: (separator, label, regex).
pub const SPLIT_CHIPS: [(&str, &str, bool); 9] = [
    (" ", "space", false), (",", ",", false), (";", ";", false), ("-", "-", false), ("/", "/", false),
    ("|", "|", false), ("@", "@", false), ("\\t", "\\t", false), ("\\s+", "spaces", true),
];

pub const TYPE_SEPARATOR: &str = "Type a separator, or pick one above.";

enum Sep {
    Lit(String),
    Re(Regex),
}

/// How a value is cut: the separator (literal text, `\t` for a tab, or a
/// regex), whether touching separators count as one — and then split
/// nothing off at either end — and whether the parts are trimmed.
pub struct Splitter {
    sep: Sep,
    collapse: bool,
    trim: bool,
}

impl Splitter {
    /// Ok(None): no separator, say TYPE_SEPARATOR. Err: the message for a bad regex.
    pub fn new(sep: &str, regex: bool, collapse: bool, trim: bool) -> Result<Option<Splitter>, String> {
        let sep = if regex {
            Sep::Re(Regex::new(sep).map_err(|e| format!("Invalid regex: {e}"))?)
        } else {
            let lit = sep.replace("\\t", "\t");
            if lit.is_empty() {
                return Ok(None);
            }
            Sep::Lit(lit)
        };
        Ok(Some(Splitter { sep, collapse, trim }))
    }

    /// Where the separators are in `v` (`sepAt`).
    fn occ(&self, v: &str, out: &mut Vec<(usize, usize)>) {
        out.clear();
        match &self.sep {
            Sep::Lit(l) => out.extend(v.match_indices(l.as_str()).map(|(i, m)| (i, i + m.len()))),
            // An empty match separates nothing.
            Sep::Re(re) => out.extend(re.find_iter(v).filter(|m| !m.is_empty()).map(|m| (m.start(), m.end()))),
        }
        if !self.collapse || out.is_empty() {
            return;
        }
        let mut k = 0;
        for i in 0..out.len() {
            if k > 0 && out[i].0 == out[k - 1].1 {
                out[k - 1].1 = out[i].1;
            } else {
                out[k] = out[i];
                k += 1;
            }
        }
        out.truncate(k);
        out.retain(|&(a, b)| a > 0 && b < v.len());
    }

    /// `v` cut into `n` parts (`cutAt`), as byte ranges: past n−1 separators
    /// the rest stays whole in the last part; missing parts are empty.
    fn cut(&self, v: &str, n: usize, occ: &[(usize, usize)], out: &mut Vec<(usize, usize)>) {
        out.clear();
        let mut pos = 0;
        for &(a, b) in occ.iter().take(n - 1) {
            out.push((pos, a));
            pos = b;
        }
        out.push((pos, v.len()));
        out.resize(n, (0, 0));
        if self.trim {
            for r in out.iter_mut() {
                let s = v[r.0..r.1].trim_start_matches(js_space);
                r.0 = r.1 - s.len();
                r.1 = r.0 + s.trim_end_matches(js_space).len();
            }
        }
    }

    /// The parts of one value, for a preview.
    pub fn parts(&self, v: &str, n: usize) -> Vec<String> {
        let (mut occ, mut out) = (Vec::new(), Vec::new());
        self.occ(v, &mut occ);
        self.cut(v, n, &occ, &mut out);
        out.into_iter().map(|(a, b)| v[a..b].to_string()).collect()
    }
}

/// How many parts the non-empty cells of the column would give.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitStats {
    /// (parts, cells), by parts.
    pub dist: Vec<(usize, usize)>,
    pub filled: usize,
    pub max: usize,
}

impl SplitStats {
    /// The column count proposed: the largest one, unless a few outliers would
    /// blow it up (more than 10) — then the count that covers 99 % of the cells.
    pub fn proposed(&self) -> usize {
        let mut cover = self.max;
        let mut acc = 0;
        for &(k, c) in &self.dist {
            acc += c;
            if acc as f64 >= self.filled as f64 * 0.99 {
                cover = k;
                break;
            }
        }
        (if self.max <= 10 { self.max } else { cover }).clamp(2, 50)
    }

    /// Whether Split can run.
    pub fn ready(&self) -> bool {
        self.filled > 0 && self.max > 1
    }
}

/// Over `ids` (all the data rows, in the web app).
pub fn analyse(st: &State, ids: &[RowId], col: usize, sp: &Splitter, cancel: &AtomicBool) -> Option<SplitStats> {
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, Vec<(usize, usize)>)| {
        let mut d: BTreeMap<usize, usize> = BTreeMap::new();
        for &id in &ids[r] {
            st.with_cell(id, col, &mut s.0, |v| {
                if !js_trim(v).is_empty() {
                    sp.occ(v, &mut s.1);
                    *d.entry(s.1.len() + 1).or_default() += 1;
                }
            });
        }
        d
    })?;
    let mut d: BTreeMap<usize, usize> = BTreeMap::new();
    for p in parts {
        for (k, c) in p {
            *d.entry(k).or_default() += c;
        }
    }
    let filled = d.values().sum();
    let max = d.keys().next_back().copied().unwrap_or(1);
    Some(SplitStats { dist: d.into_iter().collect(), filled, max })
}

/// The column count from the field (`parseInt`; None: empty or not a number).
pub fn split_n(typed: Option<i64>) -> usize {
    typed.filter(|&x| x != 0).unwrap_or(2).clamp(2, 50) as usize
}

/// The dialog's two notes: the parts found, and about the column count.
pub fn split_notes(s: &SplitStats, typed: Option<i64>) -> (String, String) {
    let n = split_n(typed);
    let stats = if s.filled > 0 {
        let parts: Vec<String> = s.dist.iter().map(|&(k, c)| format!("{k} part{} × {}", if k > 1 { "s" } else { "" }, fmt_int(c))).collect();
        format!("{} non-empty cells: {}{}", fmt_int(s.filled), parts.join(" · "), if s.max == 1 { "\nThe separator appears in no cell." } else { "" })
    } else {
        "This column is empty.".into()
    };
    let over: usize = s.dist.iter().filter(|d| d.0 > n).map(|d| d.1).sum();
    let note = if typed.is_some_and(|t| t < 2) {
        format!("A split makes 2 columns at least (before and after the separator): {n} will be created.")
    } else if over > 0 {
        format!("{} cells have more parts: the rest stays joined in column {n}.", fmt_int(over))
    } else {
        String::new()
    };
    (stats, note)
}

/// The new columns' titles: what the user typed, else "<column> 1…n".
pub fn split_names(title: &str, typed: &[Option<String>], n: usize) -> Vec<String> {
    (0..n).map(|i| match typed.get(i).and_then(|t| t.as_deref()).map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => format!("{title} {}", i + 1),
    }).collect()
}

/// The first `rows` (6) values of `ids` holding a separator, and their parts.
pub fn split_preview(st: &State, ids: &[RowId], col: usize, sp: &Splitter, n: usize, rows: usize) -> Vec<(String, Vec<String>)> {
    let (mut tmp, mut occ, mut out) = (Vec::new(), Vec::new(), Vec::new());
    for &id in ids {
        st.with_cell(id, col, &mut tmp, |v| {
            if !js_trim(v).is_empty() {
                sp.occ(v, &mut occ);
                if !occ.is_empty() {
                    out.push((v.to_string(), sp.parts(v, n)));
                }
            }
        });
        if out.len() >= rows {
            break;
        }
    }
    out
}

pub struct SplitPlan {
    col: usize,
    keep: bool,
    names: Vec<String>,
    title: String,
    /// Cells in the final layout (header title included), the empty new ones left out.
    pub changes: Vec<Change>,
}

/// `n` columns from column `col` of every row of `ids` (all the data rows);
/// `keep`: the original column stays before them; `names` from `split_names`.
pub fn split_plan(st: &State, ids: &[RowId], col: usize, sp: &Splitter, n: usize, keep: bool, names: Vec<String>, cancel: &AtomicBool) -> Option<SplitPlan> {
    let first = if keep { col + 1 } else { col };
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, Vec<(usize, usize)>)| {
        let mut out = Vec::new();
        let mut cut = Vec::new();
        for &id in &ids[r] {
            st.with_cell(id, col, &mut s.0, |v| {
                sp.occ(v, &mut s.1);
                sp.cut(v, n, &s.1, &mut cut);
                for (i, &(a, b)) in cut.iter().enumerate() {
                    // The first part takes the original's place: a change only if it differs.
                    let same = if !keep && i == 0 { b - a == v.len() } else { a == b };
                    if !same {
                        out.push((id, first + i, v[a..b].to_string()));
                    }
                }
            });
        }
        out
    })?;
    let mut changes: Vec<Change> = parts.concat();
    if !keep && st.header && st.all_len() > 0 {
        changes.push((st.all_id(0), col, names[0].clone()));
    }
    Some(SplitPlan { col, keep, names, title: st.headers().get(col).cloned().unwrap_or_default(), changes })
}

pub fn apply_split(t: &mut Table, p: SplitPlan) -> String {
    let n = p.names.len();
    as_one_step(t, |t| {
        let skip = if p.keep { 0 } else { 1 };
        for (i, name) in p.names.iter().enumerate().skip(skip) {
            t.insert_column(p.col + i + p.keep as usize, name);
        }
        t.set_cells(&p.changes);
    });
    not_saved(&format!("column \"{}\" split into {n}", p.title))
}

/* ---- Merge ---------------------------------------------------------- */

/// The dialog's chips: (separator, label); the default separator is a space.
pub const MERGE_CHIPS: [(&str, &str); 5] = [(" ", "space"), (", ", ", "), (" - ", " - "), (";", ";"), ("", "none")];

pub const PICK_TWO: &str = "Tick at least two columns.";

/// The default title: the merged columns' titles, space-separated.
pub fn merge_name(headers: &[String], order: &[usize]) -> String {
    order.iter().map(|&c| headers.get(c).map_or("", |h| h.as_str())).collect::<Vec<_>>().join(" ")
}

/// Columns `order` of row `id` joined by `sep` into `out`; `skip`: blank values left out.
fn merge_into(st: &State, id: RowId, order: &[usize], sep: &str, skip: bool, tmp: &mut Vec<u8>, out: &mut String) {
    out.clear();
    let mut first = true;
    for &c in order {
        st.with_cell(id, c, tmp, |v| {
            if skip && js_trim(v).is_empty() {
                return;
            }
            if !first {
                out.push_str(sep);
            }
            first = false;
            out.push_str(v);
        });
    }
}

/// The first `rows` (6) rows: their values and the merged one.
pub fn merge_preview(st: &State, ids: &[RowId], order: &[usize], sep: &str, skip: bool, rows: usize) -> Vec<(Vec<String>, String)> {
    let (mut tmp, mut buf) = (Vec::new(), String::new());
    ids.iter().take(rows).map(|&id| {
        merge_into(st, id, order, sep, skip, &mut tmp, &mut buf);
        (order.iter().map(|&c| st.cell(id, c)).collect(), buf.clone())
    }).collect()
}

pub struct MergePlan {
    at: usize,
    keep: bool,
    name: String,
    delete: Vec<usize>,
    count: usize,
    pub changes: Vec<Change>,
}

/// Columns `order` (in that order, two at least) of every row of `ids` (all
/// the data rows) merged into one titled `name` (empty: `merge_name`). It
/// goes after the last merged column when they stay (`keep`), in place of
/// the first when they go. Err: PICK_TWO.
pub fn merge_plan(st: &State, ids: &[RowId], order: &[usize], sep: &str, skip: bool, keep: bool, name: &str, cancel: &AtomicBool) -> Option<Result<MergePlan, &'static str>> {
    if order.len() < 2 {
        return Some(Err(PICK_TWO));
    }
    let name = match name.trim() { "" => merge_name(&st.headers(), order), n => n.to_string() };
    let at = if keep { order.iter().max().unwrap() + 1 } else { *order.iter().min().unwrap() };
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, String)| {
        let mut out = Vec::new();
        for &id in &ids[r] {
            merge_into(st, id, order, sep, skip, &mut s.0, &mut s.1);
            let same = if keep { s.1.is_empty() } else { st.with_cell(id, at, &mut s.0, |v| v == s.1) };
            if !same {
                out.push((id, at, s.1.clone()));
            }
        }
        out
    })?;
    let mut changes: Vec<Change> = parts.concat();
    if !keep && st.header && st.all_len() > 0 {
        changes.push((st.all_id(0), at, name.clone()));
    }
    let delete = if keep { Vec::new() } else { order.iter().copied().filter(|&c| c != at).collect() };
    Some(Ok(MergePlan { at, keep, name, delete, count: order.len(), changes }))
}

pub fn apply_merge(t: &mut Table, p: MergePlan) -> String {
    as_one_step(t, |t| {
        if p.keep {
            t.insert_column(p.at, &p.name);
        }
        t.set_cells(&p.changes);
        if !p.delete.is_empty() {
            t.delete_columns(&p.delete);
        }
    });
    not_saved(&format!("{} columns merged into \"{}\"", p.count, p.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::all_rows;
    use crate::edit::tests::{col, table, NO};

    fn clients() -> Table {
        table("Nom;Adresse;Ville\nTardy;22 ALLEE ZEPHIRIN 45160 OLIVET;Rouen\nKoch;;Nancy\nGallois;5 Rue du Pré;Lyon\n")
    }

    fn split(t: &mut Table, sep: &str, regex: bool, keep: bool, typed: Option<i64>) {
        let sp = Splitter::new(sep, regex, true, true).unwrap().unwrap();
        let ids = all_rows(&t.st);
        let n = split_n(typed);
        let names = split_names("Adresse", &[], n);
        let p = split_plan(&t.st, &ids, 1, &sp, n, keep, names, &NO).unwrap();
        apply_split(t, p);
    }

    fn rows(t: &Table) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        for p in 0..t.st.len() {
            let mut r = Vec::new();
            t.st.row(t.st.data_id(p), &mut r);
            r.resize(t.st.ncols, String::new());
            out.push(r);
        }
        out
    }

    // tests/test_split_column.py, on the engine.
    #[test]
    fn regex_split_keeps_header_in_step() {
        let mut t = clients();
        split(&mut t, r"\b\d{5}\b", true, false, Some(1));
        assert_eq!(t.st.headers(), vec!["Nom", "Adresse 1", "Adresse 2", "Ville"]);
        let r = rows(&t);
        assert_eq!(r[0], vec!["Tardy", "22 ALLEE ZEPHIRIN", "OLIVET", "Rouen"]);
        assert_eq!(r[2], vec!["Gallois", "5 Rue du Pré", "", "Lyon"]);
    }

    #[test]
    fn keep_original_column() {
        let mut t = clients();
        split(&mut t, r"\b\d{5}\b", true, true, None);
        assert_eq!(t.st.headers(), vec!["Nom", "Adresse", "Adresse 1", "Adresse 2", "Ville"]);
        assert_eq!(rows(&t)[0], vec!["Tardy", "22 ALLEE ZEPHIRIN 45160 OLIVET", "22 ALLEE ZEPHIRIN", "OLIVET", "Rouen"]);
    }

    #[test]
    fn literal_split_rest_stays_joined() {
        let mut t = clients();
        let sp = Splitter::new(" ", false, true, true).unwrap().unwrap();
        let s = analyse(&t.st, &all_rows(&t.st), 1, &sp, &NO).unwrap();
        assert_eq!(s.dist, vec![(4, 1), (5, 1)]);
        assert_eq!(s.proposed(), 5);
        assert_eq!(split_notes(&s, Some(2)), ("2 non-empty cells: 4 parts × 1 · 5 parts × 1".into(), "2 cells have more parts: the rest stays joined in column 2.".into()));
        assert_eq!(split_notes(&s, Some(1)).1, "A split makes 2 columns at least (before and after the separator): 2 will be created.");
        assert_eq!(split_preview(&t.st, &all_rows(&t.st), 1, &sp, 2, 6)[1].1, vec!["5", "Rue du Pré"]);
        split(&mut t, " ", false, false, Some(2));
        assert_eq!(rows(&t)[0], vec!["Tardy", "22", "ALLEE ZEPHIRIN 45160 OLIVET", "Rouen"]);
        assert!(Splitter::new("", false, true, true).unwrap().is_none());
        assert!(Splitter::new("(", true, true, true).err().unwrap().starts_with("Invalid regex: "));
    }

    #[test]
    fn collapse_and_ends() {
        let sp = Splitter::new(",", false, true, false).unwrap().unwrap();
        // The end separators split nothing off, but stay in the text.
        assert_eq!(sp.parts(",a,,b,", 3), vec![",a", "b,", ""]);
        let sp = Splitter::new(",", false, false, false).unwrap().unwrap();
        assert_eq!(sp.parts(",a,,b,", 3), vec!["", "a", ",b,"]);
        let sp = Splitter::new("\\t", false, true, false).unwrap().unwrap();
        assert_eq!(sp.parts("a\tb", 2), vec!["a", "b"]);
    }

    #[test]
    fn merges() {
        let mut t = table("first,last,city\nAna,Lopez,Lyon\nBob,,Paris\n");
        let ids = all_rows(&t.st);
        assert_eq!(merge_name(&t.st.headers(), &[0, 1]), "first last");
        assert_eq!(merge_preview(&t.st, &ids, &[1, 0], ", ", true, 6)[0].1, "Lopez, Ana");
        let p = merge_plan(&t.st, &ids, &[0, 1], " ", true, true, "", &NO).unwrap().unwrap();
        assert_eq!(apply_merge(&mut t, p), "2 columns merged into \"first last\" — not written yet, use Save.");
        assert_eq!(t.st.headers(), vec!["first", "last", "first last", "city"]);
        assert_eq!(col(&t, 2), vec!["Ana Lopez", "Bob"]);
        t.undo();
        t.undo();
        let p = merge_plan(&t.st, &ids, &[2, 0], "-", false, false, "Who", &NO).unwrap().unwrap();
        apply_merge(&mut t, p);
        assert_eq!(t.st.headers(), vec!["Who", "last"]);
        assert_eq!(col(&t, 0), vec!["Lyon-Ana", "Paris-Bob"]);
        assert_eq!(merge_plan(&t.st, &ids, &[0], "", true, true, "", &NO).unwrap().err(), Some(PICK_TWO));
    }
}
