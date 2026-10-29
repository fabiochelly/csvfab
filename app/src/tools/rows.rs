//! Rows: delete the rows the filters hide (asked first), and the paste that
//! grows the table (Ctrl+V, called by the app).

use super::edit_kit::{self as form, Snap};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::grid::Cell;
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::edit::clipboard::{apply_paste, parse_tsv, paste as plan_paste};
use csvfab_engine::edit::replace::{hidden_confirm, hidden_done, hidden_rows};
use csvfab_engine::edit::series::Rect;
use csvfab_engine::view;
use csvfab_engine::RowId;
use eframe::egui;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "Delete hidden rows", group: "Rows", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(DeleteHidden::default()))) },
        Command {
            name: "Clear all filters",
            group: "Rows",
            keys: None,
            enabled: |t| t.table.is_some() && (!t.filter.is_empty() || t.col_filters.iter().any(|s| !s.is_empty()) || t.restrict.is_some()),
            run: |cx| {
                clear_filters(cx.tab);
                cx.tab.refresh(cx.ctx);
            },
        },
        // A click on a title sorts the view only: this makes its order the file's.
        Command { name: "Write the sorted order to the file", group: "Rows", keys: None, enabled: |t| t.table.is_some() && !t.sort.is_empty(), run: |cx| cx.out.push(Cmd::Tool(Box::new(WriteSort::default()))) },
        Command { name: "Fill empty cells from above", group: "Rows", keys: None, enabled: has_table, run: fill_blanks },
        // Ctrl+D is the grid's own (it reads the selection there): these ask it for one.
        Command { name: "Fill series down", group: "Rows", keys: Some("Ctrl+D"), enabled: |t| t.table.is_some() && t.grid.range().is_some_and(|(r, _)| r.end() > r.start()), run: |cx| cx.tab.grid.want_fill_down = true },
        Command { name: "Show or hide columns…", group: "Columns", keys: None, enabled: has_table, run: |cx| cx.tab.grid.open_manager = true },
        Command { name: "Freeze the first column", group: "Columns", keys: None, enabled: |t| t.table.is_some() && t.grid.frozen == 0, run: |cx| crate::grid::freeze_to(cx.tab, 0) },
        Command { name: "Unfreeze the columns", group: "Columns", keys: None, enabled: |t| t.table.is_some() && t.grid.frozen > 0, run: |cx| cx.tab.grid.frozen = 0 },
        Command {
            name: "Fit every column to its content",
            group: "Columns",
            keys: None,
            enabled: has_table,
            run: |cx| {
                let Some(st) = cx.tab.st().cloned() else { return };
                for c in cx.tab.grid.shown_cols(st.ncols) {
                    let to = crate::grid::fit_width(cx.ctx, &st, cx.tab, c, cx.m);
                    if let Some(w) = cx.tab.grid.widths.get_mut(c) { *w = to; }
                }
            },
        },
        Command {
            name: "Reset the column widths",
            group: "Columns",
            keys: None,
            enabled: has_table,
            run: |cx| {
                if let Some(st) = cx.tab.st().cloned() { cx.tab.grid.reset_widths(&st, cx.ctx, cx.m); }
            },
        },
    ]
}

/// Fill empty cells from above (13-editor-series.js `fillBlanks`): the layout of exports
/// and pivot tables, where a category is written once at the top of its group. In the
/// selected columns (one selected cell: its whole column), in the order shown, each
/// blank cell takes the nearest value above it — above the selection too; blanks before
/// any value stay blank. One undo step.
fn fill_blanks(cx: &mut Cx) {
    let Some((rows, cols)) = cx.tab.grid.range() else {
        cx.out.push(Cmd::Toast("Select the column to fill first: one cell selects its whole column, a range limits the fill to its rows.".into()));
        return;
    };
    let Some(st) = cx.tab.st().cloned() else { return };
    let n = cx.tab.shown();
    if n == 0 { return; }
    let (r0, r1) = if rows.start() == rows.end() { (0, n - 1) } else { (*rows.start(), (*rows.end()).min(n - 1)) };
    let ids = cx.tab.shown_ids();
    let mut tmp = Vec::new();
    let mut changes = Vec::new();
    let blank = |st: &csvfab_engine::State, id: RowId, c: usize, tmp: &mut Vec<u8>| st.with_cell(id, c, tmp, |s| s.trim().is_empty());
    for c in cols.filter(|c| !cx.tab.grid.hidden.contains(c)) {
        // The value to copy: read once per run of blanks.
        let mut last: Option<RowId> = (0..r0).rev().map(|r| ids[r]).find(|&id| !blank(&st, id, c, &mut tmp));
        let mut value: Option<(RowId, String)> = None;
        for &id in &ids[r0..=r1] {
            if !blank(&st, id, c, &mut tmp) {
                last = Some(id);
            } else if let Some(from) = last {
                if value.as_ref().is_none_or(|v| v.0 != from) { value = Some((from, st.cell(from, c))); }
                changes.push((id, c, value.as_ref().unwrap().1.clone()));
            }
        }
    }
    if changes.is_empty() {
        cx.out.push(Cmd::Toast("No empty cell with a value above it.".into()));
        return;
    }
    let k = changes.len();
    cx.tab.apply_changes(&changes, cx.ctx);
    cx.out.push(Cmd::Toast(format!("Filled {} empty cell{} from above", crate::grid::fmt_int(k), if k > 1 { "s" } else { "" })));
}

type Hidden = Result<HashSet<RowId>, &'static str>;

/// Keeps only the rows shown, once confirmed; the filters are then cleared
/// (they would now match every row, and leaving them on would hide that).
#[derive(Default)]
pub struct DeleteHidden {
    job: Option<Job<Option<Hidden>>>,
    found: Option<Hidden>,
    started: bool,
}

impl Tool for DeleteHidden {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let shown = cx.tab.shown();
        if !self.started {
            self.started = true;
            let filtered = shown < snap.st.len();
            self.job = Some(Job::spawn(cx.ctx, move |c, _| hidden_rows(&snap.st, &snap.rows(), filtered, c)));
        }
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            match r {
                Some(h) => self.found = Some(h),
                None => return false,
            }
        }
        let (p, m) = (cx.p, cx.m);
        match &self.found {
            None => true,
            Some(Err(msg)) => form::message(cx.ctx, p, m, "hidden", msg, "OK", None, false).is_none(),
            Some(Ok(set)) => match form::message(cx.ctx, p, m, "hidden", &hidden_confirm(set.len(), shown), "Delete rows", Some("Cancel"), true) {
                None => true,
                Some(1) => false,
                Some(_) => {
                    let n = set.len();
                    let tab = &mut *cx.tab;
                    if let Some(t) = &mut tab.table {
                        t.delete_rows(set);
                    }
                    clear_filters(tab);
                    tab.after_edit(true, cx.ctx);
                    let kept = tab.st().map_or(0, |s| s.len());
                    cx.toast(hidden_done(n, kept));
                    false
                }
            },
        }
    }
}

/// The sort shown, written as the rows' order: every row — the filters hide some, they
/// move too — sorted in the background, then one undo step. The view's sort is done
/// with: the rows now are in its order.
#[derive(Default)]
pub struct WriteSort {
    /// The table's version it sorted: an edit meanwhile, and the order is not applied.
    job: Option<(u64, Job<Option<Vec<RowId>>>)>,
    started: bool,
}

impl Tool for WriteSort {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let Some(version) = cx.tab.table.as_ref().map(|t| t.version()) else { return false };
        if !self.started {
            self.started = true;
            let (keys, kinds) = (cx.tab.sort.clone(), cx.tab.kinds.clone());
            if keys.is_empty() { return false; }
            self.job = Some((version, Job::spawn(cx.ctx, move |c, _| {
                let mut ids = snap.all();
                view::sort(&snap.st, &mut ids, &keys, &kinds, c).then_some(ids)
            })));
        }
        let Some((v, job)) = &mut self.job else { return false };
        let Some(r) = job.poll() else { return true };
        let Some(ids) = r else { return false };
        if *v != version {
            cx.toast("The table changed while sorting: the order was not written.");
            return false;
        }
        let heads = cx.tab.st().map(|st| st.headers()).unwrap_or_default();
        let by: Vec<String> = cx.tab.sort.iter().map(|k| {
            let h = heads.get(k.col).cloned().unwrap_or_default();
            if k.desc { format!("\"{h}\" (descending)") } else { format!("\"{h}\"") }
        }).collect();
        let tab = &mut *cx.tab;
        let moved = tab.table.as_mut().is_some_and(|t| {
            let before = t.version();
            t.reorder(&ids) && t.version() != before
        });
        tab.sort.clear();
        tab.after_edit(true, cx.ctx);
        let name = tab.name.clone();
        cx.toast(if moved {
            format!("{name} | rows sorted by {} — not written yet, use Save.", by.join(", then "))
        } else {
            format!("{name} | the rows already were in this order.")
        });
        false
    }
}

pub(crate) fn clear_filters(tab: &mut Tab) {
    tab.filter.global.clear();
    tab.filter.hidden.clear();
    tab.col_filters.iter_mut().for_each(String::clear);
    tab.restrict = None;
    tab.grid.scroll = 0.0;
    tab.grid.cursor = None;
    tab.grid.anchor = None;
}

/// Ctrl+V at the selection: one value fills the whole selection; a block starts
/// at its top-left cell, and — when no filter or sort reorders the rows — adds
/// the rows and columns it needs past the end of the table. One undo step;
/// the selection then covers what was pasted. Returns the status line (None:
/// nothing selected).
///
/// For app.rs: `Action::Paste(text) => if let Some(s) = tools::rows::paste(tab, &text, ctx) { toast(s) }`.
pub fn paste(tab: &mut Tab, text: &str, ctx: &egui::Context) -> Option<String> {
    let (rows, cols) = tab.grid.range()?;
    let grid = parse_tsv(text);
    let st = tab.st()?.clone();
    let can_grow = tab.view.is_none();
    let widest = grid.iter().map(Vec::len).max().unwrap_or(0);
    // Hidden columns are stepped over, as in the web: a block lands on the columns shown.
    let shown: Vec<usize> = (0..st.ncols).filter(|c| !tab.grid.hidden.contains(c)).collect();
    // Columns past the last one: added (titled "Column N") when the table may grow.
    let room = shown.iter().filter(|&&c| c >= *cols.start()).count();
    let need = if grid.len() > 1 || widest > 1 { widest.saturating_sub(room) } else { 0 };
    let extra = if can_grow { need } else { 0 };
    let vis: Vec<usize> = shown.into_iter().chain(st.ncols..st.ncols + extra).collect();
    let sel = Rect { r0: *rows.start(), r1: *rows.end(), c0: *cols.start(), c1: *cols.end() };
    let ids = tab.shown_ids();
    let plan = plan_paste(&st, &ids, &vis, sel, &grid, false, can_grow, &AtomicBool::new(false))?;
    let end = plan.end;
    let added = plan.new_rows.len();
    let t = tab.table.as_mut()?;
    let old_n = t.st.ncols;
    let mut msg = String::new();
    t.batch(|t| {
        msg = apply_paste(t, plan, "pasted");
        if t.st.header && t.st.ncols > old_n {
            let h = t.st.all_id(0);
            let titles: Vec<_> = (old_n..t.st.ncols).map(|c| (h, c, format!("Column {}", c + 1))).collect();
            t.set_cells(&titles);
        }
    });
    let grew = tab.st().map_or(0, |s| s.ncols) > old_n;
    tab.after_edit(grew || added > 0, ctx);
    if grid.len() > 1 || widest > 1 {
        // A block's last columns empty add no column: the selection stops at the last there is.
        let last = tab.st().map_or(0, |s| s.ncols.saturating_sub(1));
        tab.grid.anchor = Some(Cell { row: sel.r0, col: sel.c0.min(last) });
        tab.grid.cursor = Some(Cell { row: end.0, col: end.1.min(last) });
    }
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context whose fonts are loaded (a tab measures its columns).
    fn ctx() -> egui::Context {
        let ctx = egui::Context::default();
        // The first pass uploads the font atlas: hand its texture changes over, as a renderer would.
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        ctx
    }

    fn cells(tab: &Tab) -> Vec<Vec<String>> {
        let st = tab.st().unwrap();
        let mut out = vec![st.headers()];
        for p in 0..st.len() {
            out.push((0..st.ncols).map(|c| st.cell(st.data_id(p), c)).collect());
        }
        out
    }

    #[test]
    fn paste_grows_rows_and_columns() {
        let ctx = ctx();
        let m = crate::theme::Metrics::default();
        let mut tab = Tab::memory("t".into(), b"a,b\n1,2\n3,4\n".to_vec(), &ctx, &m);
        tab.grid.select(Cell { row: 1, col: 1 }, false);
        let msg = paste(&mut tab, "x\ty\nz\tw\n", &ctx).unwrap();
        assert!(msg.starts_with("4 cells pasted, 1 rows added"), "{msg}");
        assert_eq!(cells(&tab), [vec!["a", "b", "Column 3"], vec!["1", "2", ""], vec!["3", "x", "y"], vec!["", "z", "w"]]);
        assert_eq!(tab.grid.cursor, Some(Cell { row: 2, col: 2 }));
        // One undo step takes it all back.
        tab.table.as_mut().unwrap().undo();
        assert_eq!(cells(&tab), [vec!["a", "b"], vec!["1", "2"], vec!["3", "4"]]);
    }

    #[test]
    fn one_value_fills_the_selection() {
        let ctx = ctx();
        let m = crate::theme::Metrics::default();
        let mut tab = Tab::memory("t".into(), b"a,b\n1,2\n3,4\n".to_vec(), &ctx, &m);
        tab.grid.select(Cell { row: 0, col: 0 }, false);
        tab.grid.select(Cell { row: 1, col: 1 }, true);
        paste(&mut tab, "z", &ctx).unwrap();
        assert_eq!(cells(&tab)[1..], [vec!["z", "z"], vec!["z", "z"]]);
    }

    /// A hidden column is stepped over: the block lands on the columns shown.
    #[test]
    fn paste_skips_hidden_columns() {
        let ctx = ctx();
        let m = crate::theme::Metrics::default();
        let mut tab = Tab::memory("t".into(), b"a,b,c\n1,2,3\n".to_vec(), &ctx, &m);
        tab.grid.hidden.insert(1);
        tab.grid.select(Cell { row: 0, col: 0 }, false);
        paste(&mut tab, "x\ty\n", &ctx).unwrap();
        assert_eq!(cells(&tab), [vec!["a", "b", "c"], vec!["x", "2", "y"]]);
    }
}
