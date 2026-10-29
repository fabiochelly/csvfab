//! Review changes: what Save would write, against the file as read. Every row
//! that came from the file is its record's index (an id below `base.n`) and
//! every column its field there (the column map), so the review compares
//! without any cooperation from the many kinds of edit: a row without a
//! record is new, a record no row claims was deleted, and only rows holding
//! edits can differ from their record.

use crate::table::{RowId, State};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnChanges {
    /// Titles of the columns added.
    pub added: Vec<String>,
    /// Titles (in the file) of the columns deleted.
    pub deleted: Vec<String>,
    /// (title in the file, title now).
    pub renamed: Vec<(String, String)>,
    pub moved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowChange {
    pub id: RowId,
    /// (column now, value in the file) of each cell that differs.
    pub cells: Vec<(usize, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub cols: ColumnChanges,
    /// Rows whose cells differ from their record, in the table's order.
    pub changed: Vec<RowChange>,
    /// Cells changed in all.
    pub cells: usize,
    /// Rows with no record, in the table's order.
    pub added: Vec<RowId>,
    /// Records (indexes in the file) no row holds any more, header excluded.
    pub deleted: Vec<usize>,
    /// The rows that came from the file are no longer in its order.
    pub reordered: bool,
}

fn plural(k: usize, w: &str) -> String {
    format!("{k} {w}{}", if k == 1 { "" } else { "s" })
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.cells == 0 && self.added.is_empty() && self.deleted.is_empty() && !self.reordered
            && self.cols.added.is_empty() && self.cols.deleted.is_empty() && self.cols.renamed.is_empty() && !self.cols.moved
    }

    /// One line: "3 cells changed in 2 rows · 1 row added · columns moved".
    pub fn summary(&self) -> String {
        let c = &self.cols;
        let mut parts = Vec::new();
        if self.cells > 0 { parts.push(format!("{} changed in {}", plural(self.cells, "cell"), plural(self.changed.len(), "row"))); }
        if !self.added.is_empty() { parts.push(format!("{} added", plural(self.added.len(), "row"))); }
        if !self.deleted.is_empty() { parts.push(format!("{} deleted", plural(self.deleted.len(), "row"))); }
        if self.reordered { parts.push("rows reordered".into()); }
        if !c.added.is_empty() { parts.push(format!("{} added", plural(c.added.len(), "column"))); }
        if !c.deleted.is_empty() { parts.push(format!("{} deleted", plural(c.deleted.len(), "column"))); }
        if !c.renamed.is_empty() { parts.push(format!("{} renamed", plural(c.renamed.len(), "column"))); }
        if c.moved { parts.push("columns moved".into()); }
        if parts.is_empty() { "The rows are the same as in the file.".into() } else { parts.join(" · ") }
    }
}

/// The changes the table holds against its file as read. None if cancelled.
pub fn changes(st: &State, cancel: &AtomicBool) -> Option<Changes> {
    let b = &st.base;
    let n = b.n;
    let cs: Vec<i32> = match &st.cmap {
        Some(m) => (**m).clone(),
        None => (0..st.ncols as i32).collect(),
    };
    let header_src = if st.header && st.all_len() > 0 && (st.all_id(0) as usize) < n { Some(st.all_id(0) as usize) } else { None };
    let mut dh = Vec::new();
    if let Some(h) = header_src { b.fields(h, &mut dh); }
    let width = if header_src.is_some() {
        dh.len()
    } else {
        b.odd.iter().map(|&(_, f)| f as usize).fold(b.width, usize::max)
    };
    let headers = st.headers();
    let old_name = |o: usize| if header_src.is_some() { dh.get(o).cloned().unwrap_or_default() } else { format!("Column {}", o + 1) };
    let kept: Vec<usize> = cs.iter().filter(|&&s| s >= 0).map(|&s| s as usize).collect();
    let cols = ColumnChanges {
        added: (0..cs.len()).filter(|&c| cs[c] < 0).map(|c| headers[c].clone()).collect(),
        deleted: (0..width).filter(|o| !kept.contains(o)).map(old_name).collect(),
        renamed: if header_src.is_some() {
            (0..cs.len()).filter(|&c| cs[c] >= 0).filter_map(|c| {
                let old = old_name(cs[c] as usize);
                (old != headers[c]).then(|| (old, headers[c].clone()))
            }).collect()
        } else {
            Vec::new()
        },
        moved: kept.windows(2).any(|w| w[1] < w[0]),
    };

    // One pass in the table's order: new rows, the order of the others, which records are held.
    let mut seen = vec![false; n];
    let (mut added, mut edited) = (Vec::new(), Vec::new());
    let mut last: Option<RowId> = None;
    let mut reordered = false;
    for p in 0..st.len() {
        if p & 0xFFFF == 0 && cancel.load(Ordering::Relaxed) { return None; }
        let id = st.data_id(p);
        let i = id as usize;
        if i >= n || seen[i] {
            added.push(id);
            continue;
        }
        seen[i] = true;
        if last.is_some_and(|l| id < l) { reordered = true; }
        last = Some(id);
        if st.row_edited(id) { edited.push(id); }
    }
    let deleted: Vec<usize> = if st.order.is_none() {
        Vec::new()
    } else {
        (0..n).filter(|&i| !seen[i] && Some(i) != header_src).collect()
    };

    // Only an edited row can differ from its record; compared in parallel, in order.
    let changed: Vec<RowChange> = edited
        .par_iter()
        .with_min_len(256)
        .map_init(|| (Vec::new(), Vec::new()), |(file, now), &id| {
            if cancel.load(Ordering::Relaxed) { return None; }
            b.fields(id as usize, file);
            st.row(id, now);
            let cells: Vec<(usize, String)> = cs.iter().enumerate().filter(|(_, &s)| s >= 0).filter_map(|(c, &s)| {
                let before = file.get(s as usize).map_or("", |v| v.as_str());
                let after = now.get(c).map_or("", |v| v.as_str());
                (before != after).then(|| (c, before.to_string()))
            }).collect();
            (!cells.is_empty()).then_some(RowChange { id, cells })
        })
        .flatten()
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let cells = changed.iter().map(|r| r.cells.len()).sum();
    Some(Changes { cols, changed, cells, added, deleted, reordered })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::collections::HashSet;
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    #[test]
    fn nothing_changed() {
        let t = table("a,b\n1,2\n3,4\n");
        let c = changes(&t.st, &AtomicBool::new(false)).unwrap();
        assert!(c.is_empty());
        assert_eq!(c.summary(), "The rows are the same as in the file.");
    }

    #[test]
    fn cells_rows_and_columns() {
        let no = AtomicBool::new(false);
        let mut t = table("a,b,c\n1,2,3\n4,5,6\n7,8,9\n");
        let ids: Vec<RowId> = (0..3).map(|p| t.st.data_id(p)).collect();
        t.set_cells(&[(ids[0], 1, "X".into())]);
        // An edit put back to the file's value is no change.
        t.set_cells(&[(ids[2], 0, "7".into())]);
        let c = changes(&t.st, &no).unwrap();
        assert_eq!(c.changed, vec![RowChange { id: ids[0], cells: vec![(1, "2".into())] }]);
        assert_eq!(c.summary(), "1 cell changed in 1 row");

        let new = t.insert_rows(0, 1);
        t.delete_rows(&HashSet::from([ids[1]]));
        t.insert_column(0, "z");
        t.delete_columns(&[3]);    // c
        t.move_column(2, 1);       // z a b → z b a
        let c = changes(&t.st, &no).unwrap();
        assert_eq!(c.added, new);
        assert_eq!(c.deleted, vec![2]);
        assert!(!c.reordered);
        assert_eq!(c.cols, ColumnChanges { added: vec!["z".into()], deleted: vec!["c".into()], renamed: vec![], moved: true });
        // Row 0's b is now column 1.
        assert_eq!(c.changed, vec![RowChange { id: ids[0], cells: vec![(1, "2".into())] }]);
        assert_eq!(c.summary(), "1 cell changed in 1 row · 1 row added · 1 row deleted · 1 column added · 1 column deleted · columns moved");
    }

    #[test]
    fn renamed_and_reordered() {
        let no = AtomicBool::new(false);
        let mut t = table("a,b\n1,2\n3,4\n");
        let h = t.st.all_id(0);
        t.set_cells(&[(h, 0, "A".into())]);
        let c = changes(&t.st, &no).unwrap();
        assert_eq!(c.cols.renamed, vec![("a".to_string(), "A".to_string())]);
        assert!(c.changed.is_empty());
        // Rows moved: a sort applied as a new order (delete + reinsert is a new row, not a move).
        let mut st = t.st.clone();
        st.order = Some(Arc::new(vec![0, 2, 1]));
        let c = changes(&st, &no).unwrap();
        assert!(c.reordered);
        assert!(c.added.is_empty() && c.deleted.is_empty());
    }
}
