//! Look up (VLOOKUP between open files): the rows of this table are matched
//! to the rows of another on a key column, and the picked columns of the
//! matching row are copied in, as new columns right after the key. The first
//! source row wins when a key repeats there.

use super::{as_one_step, fmt_int, js_trim, not_saved, par_chunks, slugify, slugify_into, Change};
use crate::table::{RowId, State, Table};
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::AtomicBool;

pub const NO_OTHER_FILE: &str = "Open the file that holds the values first.\n\nThe lookup reads another open tab: its rows are matched to this tab's rows on a key column (an ID, an e-mail…), and the columns you pick are copied over.";

/// How keys compare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Match {
    Exact,
    /// Case and surrounding spaces ignored (the dialog's default).
    #[default]
    Loose,
    Slug,
}

fn norm_into(m: Match, v: &str, out: &mut String) {
    out.clear();
    match m {
        Match::Exact => out.push_str(v),
        Match::Loose => {
            let v = js_trim(v);
            if v.is_ascii() {
                out.extend(v.bytes().map(|b| b.to_ascii_lowercase() as char));
            } else {
                out.push_str(&v.to_lowercase());
            }
        }
        Match::Slug => slugify_into(v, out),
    }
}

/// The source's key → its first row with it.
pub struct Index {
    map: HashMap<Box<str>, RowId>,
    /// Source rows whose key was already seen.
    pub dups: usize,
    mode: Match,
}

impl Index {
    /// Over the source's data rows `ids` (in its order: the first one wins); empty keys are left out.
    pub fn build(src: &State, ids: &[RowId], skey: usize, mode: Match, cancel: &AtomicBool) -> Option<Index> {
        let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, String)| {
            let mut m: HashMap<Box<str>, RowId> = HashMap::with_capacity(r.len());
            let mut dups = 0;
            for &id in &ids[r] {
                src.with_cell(id, skey, &mut s.0, |v| norm_into(mode, v, &mut s.1));
                if js_trim(&s.1).is_empty() {
                    continue;
                }
                if m.contains_key(s.1.as_str()) { dups += 1 } else { m.insert(s.1.as_str().into(), id); }
            }
            (m, dups)
        })?;
        let mut it = parts.into_iter();
        let (mut map, mut dups) = it.next().unwrap_or_default();
        for (m, d) in it {
            dups += d;
            map.reserve(m.len());
            for (k, id) in m {
                match map.entry(k) {
                    std::collections::hash_map::Entry::Occupied(_) => dups += 1,
                    std::collections::hash_map::Entry::Vacant(e) => { e.insert(id); }
                }
            }
        }
        Some(Index { map, dups, mode })
    }

    /// The source row whose key matches `v`.
    pub fn get(&self, v: &str, buf: &mut String) -> Option<RowId> {
        norm_into(self.mode, v, buf);
        self.map.get(buf.as_str()).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The key columns when the source is picked: a column of the same name on
/// both sides if there is one — the one given (`key`) first, then, unless
/// the user chose it, any. Returns (key, source key).
pub fn pick_keys(headers: &[String], src_headers: &[String], key: usize, user_key: bool) -> (usize, usize) {
    let slugs: Vec<String> = src_headers.iter().map(|h| slugify(h)).collect();
    let find = |h: &str| { let s = slugify(h); slugs.iter().position(|x| *x == s) };
    let mut k = key;
    let mut sk = headers.get(k).and_then(|h| find(h));
    if sk.is_none() && !user_key {
        if let Some(c) = headers.iter().position(|h| find(h).is_some()) {
            k = c;
            sk = find(&headers[c]);
        }
    }
    (k, sk.unwrap_or(0))
}

/// Titles of the new columns: one this table already has gets the source's
/// name after it ("Ville (clients)").
pub fn new_names(headers: &[String], src_headers: &[String], cols: &[usize], src_name: &str) -> Vec<String> {
    let tag = match src_name.rfind('.') { Some(i) if i + 1 < src_name.len() => &src_name[..i], _ => src_name };
    let mut taken: std::collections::HashSet<String> = headers.iter().cloned().collect();
    cols.iter().map(|&c| {
        let h = src_headers.get(c).cloned().unwrap_or_default();
        let n = if taken.contains(&h) { format!("{h} ({tag})") } else { h };
        taken.insert(n.clone());
        n
    }).collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub hit: usize,
    pub miss: usize,
    pub empty: usize,
}

/// How the rows `ids` (all of them, in the web app) fare against the index.
pub fn stats(st: &State, ids: &[RowId], key: usize, idx: &Index, cancel: &AtomicBool) -> Option<Stats> {
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, String)| {
        let mut out = Stats::default();
        for &id in &ids[r] {
            st.with_cell(id, key, &mut s.0, |v| norm_into(idx.mode, v, &mut s.1));
            if js_trim(&s.1).is_empty() {
                out.empty += 1;
            } else if idx.map.contains_key(s.1.as_str()) {
                out.hit += 1;
            } else {
                out.miss += 1;
            }
        }
        out
    })?;
    Some(parts.into_iter().fold(Stats::default(), |a, b| Stats { hit: a.hit + b.hit, miss: a.miss + b.miss, empty: a.empty + b.empty }))
}

/// The dialog's lines; Apply needs columns picked and at least one hit.
pub fn stats_note(s: &Stats, src_name: &str, dups: usize) -> String {
    let mut out = format!("{} of {} rows find a match in {src_name}", fmt_int(s.hit), fmt_int(s.hit + s.miss + s.empty));
    if s.miss > 0 {
        let _ = write!(out, " · {} don't", fmt_int(s.miss));
    }
    if s.empty > 0 {
        let _ = write!(out, " · {} have an empty key", fmt_int(s.empty));
    }
    if s.miss > 0 || s.empty > 0 {
        out.push_str(" — their new cells stay empty");
    }
    if dups > 0 {
        let _ = write!(out, "\n{} rows of {src_name} repeat a key already seen: the first one is used.", fmt_int(dups));
    }
    if s.hit == 0 {
        out.push_str("\nNo key matches: check the two key columns and the comparison.");
    }
    out
}

pub const PICK_COLUMNS: &str = "Tick the columns to bring over.";

/// A preview cell.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Value(String),
    /// Matched, but the source cell is empty.
    Empty,
    NoMatch,
    EmptyKey,
}

impl Shown {
    pub fn text(&self) -> &str {
        match self {
            Shown::Value(v) => v,
            Shown::Empty => "empty",
            Shown::NoMatch => "no match",
            Shown::EmptyKey => "empty key",
        }
    }
}

/// The first rows of `ids` (8 in the dialog): their key and what each new column would get.
pub fn preview(st: &State, ids: &[RowId], key: usize, src: &State, idx: &Index, cols: &[usize], rows: usize) -> Vec<(String, Vec<Shown>)> {
    let mut buf = String::new();
    ids.iter().take(rows).map(|&id| {
        let k = st.cell(id, key);
        let m = idx.get(&k, &mut buf);
        let shown = cols.iter().map(|&c| match m {
            None if js_trim(&k).is_empty() => Shown::EmptyKey,
            None => Shown::NoMatch,
            Some(sid) => match src.cell(sid, c) { v if v.is_empty() => Shown::Empty, v => Shown::Value(v) },
        }).collect();
        (k, shown)
    }).collect()
}

/// What Apply does: the new columns, inserted at `at`, and their cells.
pub struct LookupPlan {
    pub at: usize,
    pub names: Vec<String>,
    /// Cells of the new columns (their final indexes), the empty ones left out.
    pub changes: Vec<Change>,
    pub hit: usize,
    pub rows: usize,
}

/// `names`: from `new_names`. The rows `ids` are all the data rows.
pub fn plan(st: &State, ids: &[RowId], key: usize, src: &State, idx: &Index, cols: &[usize], names: Vec<String>, cancel: &AtomicBool) -> Option<LookupPlan> {
    let at = key + 1;
    let parts = par_chunks(ids.len(), cancel, |r, s: &mut (Vec<u8>, String)| {
        let (mut out, mut hit) = (Vec::new(), 0);
        for &id in &ids[r] {
            let m = st.with_cell(id, key, &mut s.0, |v| idx.get(v, &mut s.1));
            let Some(sid) = m else { continue };
            hit += 1;
            for (i, &c) in cols.iter().enumerate() {
                if let Some(v) = src.with_cell(sid, c, &mut s.0, |v| (!v.is_empty()).then(|| v.to_string())) {
                    out.push((id, at + i, v));
                }
            }
        }
        (out, hit)
    })?;
    let hit = parts.iter().map(|p| p.1).sum();
    let changes = parts.into_iter().flat_map(|p| p.0).collect();
    Some(LookupPlan { at, names, changes, hit, rows: ids.len() })
}

/// Inserts the columns and fills them; returns the status line.
pub fn apply(t: &mut Table, p: LookupPlan, src_name: &str) -> String {
    let n = p.names.len();
    as_one_step(t, |t| {
        for (i, name) in p.names.iter().enumerate() {
            t.insert_column(p.at + i, name);
        }
        t.set_cells(&p.changes);
    });
    let what = format!("{n} column{} looked up in {src_name}", if n > 1 { "s" } else { "" });
    not_saved(&format!("{what}: {} of {} rows matched", fmt_int(p.hit), fmt_int(p.rows)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::all_rows;
    use crate::edit::tests::{col, table, NO};

    #[test]
    fn looks_up() {
        let mut t = table("id;name;city\nA1;Ana;Lyon\n a2 ;Bob;Paris\n;Cy;Nice\nZ9;Dan;Brest\n");
        let src = table("Code,Ville,Pays,Tel\na1,Lyon 3e,FR,0102\nA2,,FR,0304\nA1,Dup,BE,0506\n,X,Y,Z\n");
        let (k, sk) = pick_keys(&t.st.headers(), &src.st.headers(), 0, false);
        assert_eq!((k, sk), (0, 0));
        let sids = all_rows(&src.st);
        let idx = Index::build(&src.st, &sids, 0, Match::Loose, &NO).unwrap();
        assert_eq!((idx.len(), idx.dups), (2, 1));
        let ids = all_rows(&t.st);
        let s = stats(&t.st, &ids, 0, &idx, &NO).unwrap();
        assert_eq!(s, Stats { hit: 2, miss: 1, empty: 1 });
        assert_eq!(stats_note(&s, "src.csv", idx.dups), "2 of 4 rows find a match in src.csv · 1 don't · 1 have an empty key — their new cells stay empty\n1 rows of src.csv repeat a key already seen: the first one is used.");
        let cols = [1, 3];
        let pv = preview(&t.st, &ids, 0, &src.st, &idx, &cols, 8);
        assert_eq!(pv[1].1, vec![Shown::Empty, Shown::Value("0304".into())]);
        assert_eq!(pv[2].1[0], Shown::EmptyKey);
        assert_eq!(pv[3].1[0].text(), "no match");
        let names = new_names(&t.st.headers(), &src.st.headers(), &cols, "src.csv");
        assert_eq!(names, vec!["Ville", "Tel"]);
        let p = plan(&t.st, &ids, 0, &src.st, &idx, &cols, names, &NO).unwrap();
        assert_eq!(apply(&mut t, p, "src.csv"), "2 columns looked up in src.csv: 2 of 4 rows matched — not written yet, use Save.");
        assert_eq!(t.st.headers(), vec!["id", "Ville", "Tel", "name", "city"]);
        assert_eq!(col(&t, 1), vec!["Lyon 3e", "", "", ""]);
        assert_eq!(col(&t, 2), vec!["0102", "0304", "", ""]);
        assert_eq!(col(&t, 4), vec!["Lyon", "Paris", "Nice", "Brest"]);
        assert_eq!(new_names(&t.st.headers(), &src.st.headers(), &[1], "src.csv"), vec!["Ville (src)"]);
        // Exact keys: " a2 " no longer matches, "A1" finds the source's third row.
        let idx = Index::build(&src.st, &sids, 0, Match::Exact, &NO).unwrap();
        assert_eq!(stats(&t.st, &ids, 0, &idx, &NO).unwrap().hit, 1);
        assert_eq!(idx.get("A1", &mut String::new()), Some(sids[2]));
        let idx = Index::build(&src.st, &sids, 0, Match::Slug, &NO).unwrap();
        assert_eq!(stats(&t.st, &ids, 0, &idx, &NO).unwrap().hit, 2);
    }

    #[test]
    fn keys_by_name() {
        let h = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_keys(&h(&["Nom", "E-mail"]), &h(&["x", "email", "e mail"]), 0, false), (1, 2));
        assert_eq!(pick_keys(&h(&["Nom", "E-mail"]), &h(&["x", "e mail"]), 0, true), (0, 0));
    }
}
