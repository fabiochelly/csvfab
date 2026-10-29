//! Compare with another tab: two versions of one export (Monday's and
//! today's), rows matched on a key column, the other columns paired by name.
//! Keys and values compare as in the lookup (exact, loose, slug). When a key
//! repeats in the other tab, its first row is the one used.
//!
//! The other tab's keys go into a hash table over one arena of key bytes (no
//! string per row); this tab's rows are then looked up and compared in parallel.

use crate::dupes::{js_space, slug};
use crate::group::{Match, Reader};
use crate::table::{RowId, State};
use crate::group::{HashMap, Hasher};
use hashbrown::HashTable;
use rayon::prelude::*;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub id: RowId,
    /// (column here, the other tab's value) for each column that differs.
    pub cells: Vec<(usize, String)>,
}

#[derive(Debug, Default)]
pub struct Compare {
    pub key: usize,
    pub other_key: usize,
    /// (column here, column there) of the same name, the keys aside.
    pub pairs: Vec<(usize, usize)>,
    pub only_here_cols: Vec<String>,
    pub only_there_cols: Vec<String>,
    /// Rows whose key the other tab lacks, in this tab's order.
    pub only_here: Vec<RowId>,
    /// The other tab's rows whose key is not here (first of each key), in its order.
    pub only_there: Vec<RowId>,
    pub changed: Vec<Changed>,
    pub same: usize,
    /// Rows here without a key.
    pub no_key: usize,
    /// Rows there whose key an earlier row already has.
    pub dup_there: usize,
}

/// The key columns to start with: `key` and the other tab's column of the same
/// name; else the first column here whose name the other tab has; else `key` and 0.
pub fn guess_keys(here: &[String], there: &[String], key: usize) -> (usize, usize) {
    let slugs: Vec<String> = there.iter().map(|h| slug(h)).collect();
    let find = |c: usize| here.get(c).and_then(|h| { let s = slug(h); slugs.iter().position(|x| *x == s) });
    if let Some(sk) = find(key) { return (key, sk) }
    for c in 0..here.len() {
        if let Some(sk) = find(c) { return (c, sk) }
    }
    (key, 0)
}

const CH: usize = 1 << 14;

/// Compares every row of `here` with every row of `there`. None if cancelled.
/// `progress` counts rows read, both tabs.
pub fn compare(here: &State, there: &State, key: usize, other_key: usize, how: Match, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Compare> {
    let (h1, h2) = (here.headers(), there.headers());
    // Columns paired by name (slugs), each column of the other tab used once.
    let mut by_slug: HashMap<String, usize> = HashMap::default();
    for (i, h) in h2.iter().enumerate() {
        if i != other_key { by_slug.entry(slug(h)).or_insert(i); }
    }
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (c, h) in h1.iter().enumerate() {
        if c == key { continue }
        if let Some(&o) = by_slug.get(&slug(h)) {
            if !pairs.iter().any(|p| p.1 == o) { pairs.push((c, o)) }
        }
    }
    let only_here_cols = h1.iter().enumerate().filter(|&(c, _)| c != key && !pairs.iter().any(|p| p.0 == c)).map(|(_, h)| h.clone()).collect();
    let only_there_cols = h2.iter().enumerate().filter(|&(o, _)| o != other_key && !pairs.iter().any(|p| p.1 == o)).map(|(_, h)| h.clone()).collect();

    // The other tab's keys, chunk by chunk into arenas, then indexed: first row wins.
    let tick = |n: usize| if let Some(p) = progress { p.fetch_add(n, Ordering::Relaxed); };
    let n2 = there.len();
    let chunks: Vec<(Vec<u8>, Vec<u32>)> = (0..n2.div_ceil(CH))
        .into_par_iter()
        .map_init(|| (Vec::new(), String::new()), |(tmp, buf), k| {
            let range = k * CH..((k + 1) * CH).min(n2);
            let mut arena = Vec::new();
            let mut offs = Vec::with_capacity(range.len() + 1);
            offs.push(0u32);
            if !cancel.load(Ordering::Relaxed) {
                for p in range.clone() {
                    buf.clear();
                    there.with_cell(there.data_id(p), other_key, tmp, |v| how.apply(v, buf));
                    arena.extend_from_slice(buf.as_bytes());
                    offs.push(arena.len() as u32);
                }
            }
            offs.resize(range.len() + 1, arena.len() as u32);
            tick(range.len());
            (arena, offs)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None }
    let key2 = |p: usize| { let (a, o) = &chunks[p / CH]; &a[o[p % CH] as usize..o[p % CH + 1] as usize] };
    let blank = |k: &[u8]| std::str::from_utf8(k).is_ok_and(|s| s.trim_matches(js_space).is_empty());
    let hb = Hasher::default();
    let mut index: HashTable<u32> = HashTable::with_capacity(n2);
    let mut dup_there = 0;
    for p in 0..n2 {
        let k = key2(p);
        if blank(k) { continue }
        let h = hb.hash_one(k);
        if index.find(h, |&q| key2(q as usize) == k).is_some() { dup_there += 1; continue }
        index.insert_unique(h, p as u32, |&q| hb.hash_one(key2(q as usize)));
    }

    // This tab's rows: matched and compared in parallel.
    enum Row { NoKey, Here, Same(u32), Changed(u32, Vec<(usize, String)>) }
    let mut cols: Vec<usize> = vec![key];
    cols.extend(pairs.iter().map(|p| p.0));
    let ocols: Vec<usize> = pairs.iter().map(|p| p.1).collect();
    let n1 = here.len();
    let rows: Vec<Row> = (0..n1)
        .into_par_iter()
        .with_min_len(4096)
        .map_init(|| (Reader::new(&cols), Reader::new(&ocols), String::new(), String::new(), String::new()), |(r1, r2, k, a, b), p| {
            if cancel.load(Ordering::Relaxed) { return Row::NoKey }
            if p % CH == 0 { tick(CH.min(n1 - p)) }
            let id = here.data_id(p);
            r1.read(here, id);
            k.clear();
            how.apply(&r1.v[0], k);
            if k.trim_matches(js_space).is_empty() { return Row::NoKey }
            let Some(&o) = index.find(hb.hash_one(k.as_bytes()), |&q| key2(q as usize) == k.as_bytes()) else { return Row::Here };
            r2.read(there, there.data_id(o as usize));
            let mut cells = Vec::new();
            for (j, &(c, _)) in pairs.iter().enumerate() {
                let (x, y) = (&r1.v[j + 1], &r2.v[j]);
                if x == y { continue }
                a.clear();
                b.clear();
                how.apply(x, a);
                how.apply(y, b);
                if a != b { cells.push((c, y.clone())) }
            }
            if cells.is_empty() { Row::Same(o) } else { Row::Changed(o, cells) }
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None }
    let mut hit = vec![false; n2];
    let mut out = Compare { key, other_key, pairs, only_here_cols, only_there_cols, dup_there, ..Default::default() };
    for (p, r) in rows.into_iter().enumerate() {
        match r {
            Row::NoKey => out.no_key += 1,
            Row::Here => out.only_here.push(here.data_id(p)),
            Row::Same(o) => { hit[o as usize] = true; out.same += 1 }
            Row::Changed(o, cells) => { hit[o as usize] = true; out.changed.push(Changed { id: here.data_id(p), cells }) }
        }
    }
    let firsts: Vec<u32> = {
        let mut f: Vec<u32> = index.iter().copied().collect();
        f.par_sort_unstable();
        f
    };
    out.only_there = firsts.into_iter().filter(|&o| !hit[o as usize]).map(|o| there.data_id(o as usize)).collect();
    Some(out)
}

impl Compare {
    /// The status column, per row here (file order): "changed: A, B",
    /// "only here", "same", or "" for a row without a key.
    pub fn status_column(&self, here: &State) -> Vec<String> {
        let headers = here.headers();
        let mut status: HashMap<RowId, String> = HashMap::default();
        for &id in &self.only_here { status.insert(id, "only here".into()); }
        for c in &self.changed {
            let names: Vec<&str> = c.cells.iter().map(|&(col, _)| headers.get(col).map_or("", |s| s.as_str())).collect();
            status.insert(c.id, format!("changed: {}", names.join(", ")));
        }
        (0..here.len()).into_par_iter().map_init(Vec::new, |tmp, p| {
            let id = here.data_id(p);
            if let Some(s) = status.get(&id) { return s.clone() }
            // Not only here nor changed: same, or without a key.
            let keyed = here.with_cell(id, self.key, tmp, |v| !v.trim_matches(js_space).is_empty());
            if keyed { "same".into() } else { String::new() }
        }).collect()
    }

    /// The rows only the other tab has, laid out as this tab's columns (key and
    /// paired columns filled, the others empty), to append here.
    pub fn rows_to_append(&self, there: &State, ncols: usize) -> Vec<Vec<String>> {
        let map: Vec<Option<usize>> = (0..ncols).map(|c| if c == self.key { Some(self.other_key) } else { self.pairs.iter().find(|p| p.0 == c).map(|p| p.1) }).collect();
        self.only_there.par_iter().map_init(Vec::new, |tmp, &id| {
            map.iter().map(|o| o.map_or(String::new(), |o| there.with_cell(id, o, tmp, |s| s.to_string()))).collect()
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    #[test]
    fn compares_two_versions() {
        let monday = table("id,name,city,old\n1,Ana,Lyon,x\n2,Bob,Paris,y\n3,Cy,Nice,z\n3,Cy bis,Nice,z\n5,Eve,Metz,w\n,,,\n");
        let today = table("ID,Name,City,new\n1,ANA,Lyon,a\n2,Bob,Lille,b\n4,Dan,Brest,c\n ,x,y,z\n");
        assert_eq!(guess_keys(&today.st.headers(), &monday.st.headers(), 0), (0, 0));
        let no = AtomicBool::new(false);
        let c = compare(&today.st, &monday.st, 0, 0, Match::Loose, &no, None).unwrap();
        assert_eq!(c.pairs, vec![(1, 1), (2, 2)]);
        assert_eq!(c.only_here_cols, vec!["new"]);
        assert_eq!(c.only_there_cols, vec!["old"]);
        assert_eq!(c.same, 1);   // Ana: case ignored
        assert_eq!(c.changed, vec![Changed { id: today.st.data_id(1), cells: vec![(2, "Paris".into())] }]);
        assert_eq!(c.only_here, vec![today.st.data_id(2)]);
        assert_eq!(c.only_there.iter().map(|&id| monday.st.cell(id, 1)).collect::<Vec<_>>(), vec!["Cy", "Eve"]);
        assert_eq!((c.no_key, c.dup_there), (1, 1));
        assert_eq!(c.status_column(&today.st), vec!["same", "changed: City", "only here", ""]);
        assert_eq!(c.rows_to_append(&monday.st, 4), vec![vec!["3", "Cy", "Nice", ""], vec!["5", "Eve", "Metz", ""]]);
        let exact = compare(&today.st, &monday.st, 0, 0, Match::Exact, &no, None).unwrap();
        assert_eq!(exact.changed.len(), 2);   // ANA ≠ Ana
    }

    #[test]
    fn guesses_key_by_name() {
        let h = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(guess_keys(&h(&["Name", "E-mail"]), &h(&["x", "e-mail"]), 0), (1, 1));
        assert_eq!(guess_keys(&h(&["a"]), &h(&["b"]), 0), (0, 0));
    }
}
