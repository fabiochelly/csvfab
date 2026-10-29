//! Copy and paste a block of cells the way spreadsheets (Excel, Google
//! Sheets, LibreOffice) exchange it as clipboard text: tabs between cells,
//! CRLF between rows, a cell holding a tab, a line break or a quote quoted
//! with "" doubling. Hidden columns are skipped both ways.

use super::series::Rect;
use super::{as_one_step, fmt_int, not_saved, par_chunks, Change};
use crate::table::{RowId, State, Table};
use std::fmt::Write;
use std::sync::atomic::AtomicBool;

/// One cell as clipboard text, appended to `out`.
pub fn tsv_quote(v: &str, out: &mut String) {
    if v.contains(['\t', '\n', '\r', '"']) {
        out.push('"');
        out.push_str(&v.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(v);
    }
}

/// Clipboard text to rows of cells. A quote opens a quoted cell only at its
/// start; a CR, an LF or a CRLF ends a row; a last line without its break counts.
pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let b = text.as_bytes();
    let (mut rows, mut row, mut cell, mut q) = (Vec::new(), Vec::new(), String::new(), false);
    let mut i = 0;
    while i < b.len() {
        if q {
            // Up to the next quote in one go: doubled, it is one; alone, it closes.
            match memchr::memchr(b'"', &b[i..]) {
                None => { cell.push_str(&text[i..]); i = b.len(); }
                Some(k) => {
                    cell.push_str(&text[i..i + k]);
                    i += k + 1;
                    if b.get(i) == Some(&b'"') { cell.push('"'); i += 1 } else { q = false }
                }
            }
            continue;
        }
        match b[i] {
            b'"' if cell.is_empty() => { q = true; i += 1 }
            b'\t' => { row.push(std::mem::take(&mut cell)); i += 1 }
            c @ (b'\r' | b'\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                i += if c == b'\r' && b.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
            }
            _ => {
                // A run of plain text, to the next tab or line break (a quote inside it is text).
                let k = memchr::memchr3(b'\t', b'\r', b'\n', &b[i + 1..]).map_or(b.len(), |k| i + 1 + k);
                cell.push_str(&text[i..k]);
                i = k;
            }
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

pub struct Copied {
    pub text: String,
    pub rows: usize,
    pub cols: usize,
}

impl Copied {
    pub fn note(&self) -> String {
        format!("{} × {} cells copied.", fmt_int(self.rows), fmt_int(self.cols))
    }
}

/// The selection as clipboard text (Ctrl+C): `rows` are the rows shown, `vis`
/// the visible columns in order.
pub fn copy(st: &State, rows: &[RowId], vis: &[usize], sel: Rect, cancel: &AtomicBool) -> Option<Copied> {
    let cols: Vec<usize> = vis.iter().copied().filter(|c| (sel.c0..=sel.c1).contains(c)).collect();
    let n = sel.r1 + 1 - sel.r0;
    let parts = par_chunks(n, cancel, |r, tmp: &mut Vec<u8>| {
        let mut out = String::new();
        for p in r {
            if p > 0 {
                out.push_str("\r\n");
            }
            for (k, &c) in cols.iter().enumerate() {
                if k > 0 {
                    out.push('\t');
                }
                st.with_cell(rows[sel.r0 + p], c, tmp, |v| tsv_quote(v, &mut out));
            }
        }
        out
    })?;
    let mut text = parts.concat();
    if n > 1 {
        text.push_str("\r\n");
    }
    Some(Copied { text, rows: n, cols: cols.len() })
}

/// What a paste does.
#[derive(Debug, Default, PartialEq)]
pub struct Paste {
    /// Cells of rows already there that change.
    pub changes: Vec<Change>,
    /// Rows added at the end of the table, as (column, text) of their non-empty cells.
    pub new_rows: Vec<Vec<(usize, String)>>,
    /// Lines of the block left out: past the rows shown, a filter being on.
    pub dropped_rows: usize,
    pub dropped_cols: usize,
    /// The block's bottom-right cell (row shown, column), for the selection.
    pub end: (usize, usize),
}

/// Writes `grid` at the selection (Ctrl+V): one value — or `fill_only` — fills
/// the whole selection; a block starts at its top-left corner, and grows the
/// table at its end when `can_grow` (nothing filtered out).
pub fn paste(st: &State, rows: &[RowId], vis: &[usize], sel: Rect, grid: &[Vec<String>], fill_only: bool, can_grow: bool, cancel: &AtomicBool) -> Option<Paste> {
    let mut p = Paste { end: (sel.r1, sel.c1), ..Default::default() };
    if grid.is_empty() {
        return Some(p);
    }
    if fill_only || (grid.len() == 1 && grid[0].len() == 1) {
        let v = grid[0].first().map_or("", |s| s.as_str());
        let cols: Vec<usize> = vis.iter().copied().filter(|c| (sel.c0..=sel.c1).contains(c)).collect();
        let parts = par_chunks(sel.r1 + 1 - sel.r0, cancel, |r, tmp: &mut Vec<u8>| {
            let mut out = Vec::new();
            for pos in r {
                let id = rows[sel.r0 + pos];
                for &c in &cols {
                    if !st.with_cell(id, c, tmp, |x| x == v) {
                        out.push((id, c, v.to_string()));
                    }
                }
            }
            out
        })?;
        p.changes = parts.concat();
        return Some(p);
    }
    let k0 = vis.iter().position(|&c| c == sel.c0).or_else(|| vis.iter().position(|&c| c > sel.c0)).unwrap_or(0);
    let room = vis.len().saturating_sub(k0);
    let widest = grid.iter().map(Vec::len).max().unwrap_or(0);
    p.dropped_cols = widest.saturating_sub(room);
    let have = rows.len().saturating_sub(sel.r0).min(grid.len());
    let parts = par_chunks(have, cancel, |r, tmp: &mut Vec<u8>| {
        let mut out = Vec::new();
        for gi in r {
            let id = rows[sel.r0 + gi];
            for (gj, v) in grid[gi].iter().enumerate().take(room) {
                let c = vis[k0 + gj];
                if !st.with_cell(id, c, tmp, |x| x == v) {
                    out.push((id, c, v.clone()));
                }
            }
        }
        out
    })?;
    p.changes = parts.concat();
    for line in &grid[have..] {
        if !can_grow {
            p.dropped_rows += 1;
            continue;
        }
        p.new_rows.push(line.iter().take(room).enumerate().filter(|(_, v)| !v.is_empty()).map(|(gj, v)| (vis[k0 + gj], v.clone())).collect());
    }
    let total = rows.len() + p.new_rows.len();
    let last_col = vis.get((k0 + widest.max(1) - 1).min(vis.len().saturating_sub(1))).copied().unwrap_or(sel.c1);
    p.end = ((sel.r0 + grid.len() - 1).min(total.saturating_sub(1)), last_col);
    Some(p)
}

/// Carries the paste out; `verb`: "pasted". Returns the status line.
pub fn apply_paste(t: &mut Table, p: Paste, verb: &str) -> String {
    let added = p.new_rows.len();
    let n = p.changes.len() + p.new_rows.iter().map(Vec::len).sum::<usize>();
    if n == 0 && added == 0 {
        return "Nothing changed.".into();
    }
    as_one_step(t, |t| {
        let mut changes = p.changes;
        if added > 0 {
            let ids = t.insert_rows(t.st.len(), added);
            for (id, cells) in ids.into_iter().zip(p.new_rows) {
                changes.extend(cells.into_iter().map(|(c, v)| (id, c, v)));
            }
        }
        t.set_cells(&changes);
    });
    let mut what = format!("{} cells {verb}", fmt_int(n));
    if added > 0 {
        let _ = write!(what, ", {} rows added", fmt_int(added));
    }
    if p.dropped_rows > 0 {
        let _ = write!(what, " · {} rows left out (a filter is active)", fmt_int(p.dropped_rows));
    }
    if p.dropped_cols > 0 {
        let _ = write!(what, " · {} columns past the last one left out", fmt_int(p.dropped_cols));
    }
    not_saved(&what)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::all_rows;
    use crate::edit::tests::{col, table, NO};

    #[test]
    fn tsv_round_trip() {
        let cells = ["a", "tab\there", "line\nbreak", "say \"hi\"", ""];
        let mut text = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i > 0 {
                text.push('\t')
            }
            tsv_quote(c, &mut text);
        }
        assert_eq!(text, "a\t\"tab\there\"\t\"line\nbreak\"\t\"say \"\"hi\"\"\"\t");
        assert_eq!(parse_tsv(&(text.clone() + "\r\nx\ty\r\n")), vec![cells.iter().map(|s| s.to_string()).collect::<Vec<_>>(), vec!["x".into(), "y".into()]]);
        assert_eq!(parse_tsv("1\t2\n3"), vec![vec!["1", "2"], vec!["3"]]);
        assert_eq!(parse_tsv("a\"b\""), vec![vec!["a\"b\""]]);
        assert!(parse_tsv("").is_empty());
    }

    #[test]
    fn copy_paste() {
        let mut t = table("a,b,c\n1,2,3\n4,5,6\n");
        let rows = all_rows(&t.st);
        let vis = [0, 2];
        let c = copy(&t.st, &rows, &vis, Rect { r0: 0, r1: 1, c0: 0, c1: 2 }, &NO).unwrap();
        assert_eq!(c.text, "1\t3\r\n4\t6\r\n");
        assert_eq!(c.note(), "2 × 2 cells copied.");
        let one = copy(&t.st, &rows, &vis, Rect { r0: 1, r1: 1, c0: 0, c1: 0 }, &NO).unwrap();
        assert_eq!(one.text, "4");

        let grid = parse_tsv("x\ty\tz\n4\t9\nnew\t\n");
        let p = paste(&t.st, &rows, &vis, Rect { r0: 0, r1: 0, c0: 0, c1: 0 }, &grid, false, true, &NO).unwrap();
        assert_eq!(p.dropped_cols, 1);
        assert_eq!(p.new_rows, vec![vec![(0, "new".to_string())]]);
        assert_eq!(p.end, (2, 2));
        assert_eq!(apply_paste(&mut t, p, "pasted"), "4 cells pasted, 1 rows added · 1 columns past the last one left out — not written yet, use Save.");
        assert_eq!(col(&t, 0), vec!["x", "4", "new"]);
        assert_eq!(col(&t, 1), vec!["2", "5", ""]);
        assert_eq!(col(&t, 2), vec!["y", "9", ""]);
        t.undo();
        t.undo();
        assert_eq!(t.st.len(), 2);

        let p = paste(&t.st, &rows, &vis, Rect { r0: 1, r1: 1, c0: 0, c1: 2 }, &grid, false, false, &NO).unwrap();
        assert_eq!(p.dropped_rows, 2);
        let p = paste(&t.st, &rows, &vis, Rect { r0: 0, r1: 1, c0: 0, c1: 2 }, &[vec!["7".into()]], false, true, &NO).unwrap();
        assert_eq!(p.changes.len(), 4);
        assert_eq!(apply_paste(&mut t, Paste::default(), "pasted"), "Nothing changed.");
    }
}
