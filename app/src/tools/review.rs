//! Review changes (19-review.js): what Save would write, against the file as
//! read — columns added, deleted or renamed, cells changed (before → after),
//! rows added and deleted. A click on a change goes to its row; "Show changed
//! rows" marks them (the changed cells tinted, their former value on hover)
//! and shows only those rows.

use super::form::{self, b, muted, Btn, PvCell, PvRow, Sty};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::{Marks, Tab};
use csvfab_engine::review::{self, Changes};
use csvfab_engine::{RowId, State};
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Review changes…", group: "Edit", keys: None, enabled: has_table, run: |cx| { if let Some(t) = Review::new(cx) { cx.out.push(Cmd::Tool(Box::new(t))) } } }]
}

/// Cells listed at most, and rows added or deleted.
const REVIEW_MAX: usize = 500;
const ROWS_MAX: usize = 100;

struct Review {
    st: State,
    /// The changes, and whether the file on disk is no longer the one read (another program
    /// changed it since: the comparison is with the file as it was read).
    job: Option<Job<Option<(Changes, bool)>>>,
    res: Option<Changes>,
    changed_since: bool,
    /// Going to this cell once the rows shown are ready.
    pending: Option<(RowId, usize)>,
    /// The edits Undo can step back through, most recent first: what each did (the toasts'
    /// words), or None for a cell typed.
    log: Vec<Option<String>>,
}

/// The row number the grid shows for a row.
fn rec(st: &State, id: RowId) -> usize {
    (id as usize + 1).saturating_sub(st.header as usize)
}

impl Review {
    fn new(cx: &mut Cx) -> Option<Review> {
        let st = cx.tab.st()?.clone();
        let s2 = st.clone();
        let job = Job::spawn(cx.ctx, move |cancel, _| {
            let since = s2.base.path.as_deref().is_some_and(|p| csvfab_engine::Stamp::of(p) != s2.base.stamp);
            review::changes(&s2, cancel).map(|c| (c, since))
        });
        let uid = cx.tab.uid;
        let log = cx.tab.table.as_ref().map_or(Vec::new(), |t| t.undo_versions().into_iter().rev().map(|v| crate::app::edit_what::get((uid, v))).collect());
        Some(Review { st, job: Some(job), res: None, changed_since: false, pending: None, log })
    }
}

/// Changed and added rows as marks — the changed cells tinted, their former
/// value as the tip — and only those rows shown.
fn mark(tab: &mut Tab, st: &State, res: &Changes, ctx: &eframe::egui::Context) {
    let mut m = Marks::default();
    m.rows.extend(res.added.iter().copied());
    for c in &res.changed {
        m.rows.insert(c.id);
        for (col, before) in &c.cells {
            m.cells.insert((c.id, *col), format!("Before: {}", if before.is_empty() { "(empty)" } else { before }));
        }
    }
    // In the table's order.
    let ids: Vec<RowId> = st.in_order(&m.rows);
    tab.marks = Arc::new(m);
    tab.restrict_to("changed rows", ids, ctx);
}

/// Selects a cell of the rows shown and scrolls to it; false if the row is not shown.
pub fn go_to(tab: &mut Tab, id: RowId, col: usize) -> bool {
    let Some(pos) = tab.position_of(id) else { return false };
    tab.grid.go_to_col(pos, col);
    tab.grid.set_top(pos.saturating_sub(tab.grid.page / 2));
    true
}

impl Tool for Review {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if let Some((id, col)) = self.pending {
            // The rows marked are being worked out: go there once they are.
            if cx.tab.busy() { return true; }
            go_to(cx.tab, id, col);
            return false;
        }
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            match r {
                Some((c, since)) => { self.res = Some(c); self.changed_since = since; }
                None => return false,
            }
        }
        let p = cx.p.clone();
        let heads = self.st.headers();
        let name = cx.tab.name.clone();
        let cx_edits = cx.tab.table.as_ref().map_or(0, |t| t.pending_edits());
        let st = self.st.clone();
        let mut go = None;
        let mut do_mark = false;
        let mut cancel = false;
        let (_, close) = form::modal(cx, "review", 640.0, "Pending changes", |ui, _| {
            let Some(res) = &self.res else {
                form::para(ui, &p, "Comparing with the file as it was read…", 12.0, p.txt_mut);
                ui.add_space(16.0);
                if form::actions(ui, &p, &[Btn::outline("Close")]).is_some() { cancel = true; }
                return;
            };
            let c = &res.cols;
            let nb = |k: usize, w: &str| format!("{} {w}{}", b(form::n(k)), if k == 1 { "" } else { "s" });
            let mut parts = Vec::new();
            if res.cells > 0 { parts.push(format!("{} changed in {}", nb(res.cells, "cell"), nb(res.changed.len(), "row"))); }
            if !res.added.is_empty() { parts.push(format!("{} added", nb(res.added.len(), "row"))); }
            if !res.deleted.is_empty() { parts.push(format!("{} deleted", nb(res.deleted.len(), "row"))); }
            if res.reordered { parts.push("rows reordered".into()); }
            if !c.added.is_empty() { parts.push(format!("{} added", nb(c.added.len(), "column"))); }
            if !c.deleted.is_empty() { parts.push(format!("{} deleted", nb(c.deleted.len(), "column"))); }
            if !c.renamed.is_empty() { parts.push(format!("{} renamed", nb(c.renamed.len(), "column"))); }
            if c.moved { parts.push("columns moved".into()); }
            let sum = if parts.is_empty() { "The rows are the same as in the file.".to_string() } else { parts.join(" · ") };
            let pending = cx_edits;
            let against = if self.changed_since { format!("compared with {name} as it was read: another program changed it since") } else { format!("compared with {name}") };
            form::stat_at(ui, &p, &format!("{sum}\n{}", muted(format!("{} edit{} pending — {against}.", form::n(pending), if pending == 1 { "" } else { "s" }))), 0.0);

            let mut rows: Vec<PvRow> = Vec::new();
            let mut targets: Vec<Option<(RowId, usize)>> = Vec::new();
            let mut push = |r: PvRow, t: Option<(RowId, usize)>| { rows.push(r); targets.push(t); };
            let more = |k: usize, shown: usize| PvRow::new(vec![PvCell::new(format!("…and {} more", form::n(k - shown)), Sty::Faint).span(4)]);
            if !c.added.is_empty() || !c.deleted.is_empty() || !c.renamed.is_empty() {
                push(PvRow::sec(vec![PvCell::txt("Columns").span(4)]), None);
                for h in &c.added { push(PvRow::new(vec![PvCell::new("added", Sty::Add), PvCell::txt(h).span(3)]), None); }
                for h in &c.deleted { push(PvRow::new(vec![PvCell::new("deleted", Sty::Del), PvCell::txt(h).span(3)]), None); }
                for (a, bb) in &c.renamed { push(PvRow::new(vec![PvCell::new("renamed", Sty::Chg), PvCell::new(a, Sty::Old).and(" → ", Sty::Txt).and(bb, Sty::Txt).span(3)]), None); }
            }
            if !res.changed.is_empty() {
                push(PvRow::sec(vec![PvCell::txt("Row"), PvCell::txt("Column"), PvCell::txt("Before"), PvCell::txt("After")]), None);
                let mut shown = 0;
                'outer: for ch in &res.changed {
                    for (col, before) in &ch.cells {
                        if shown >= REVIEW_MAX { break 'outer; }
                        shown += 1;
                        let after = st.cell(ch.id, *col);
                        push(PvRow::new(vec![
                            PvCell::new(form::n(rec(&st, ch.id)), Sty::Sub),
                            PvCell::txt(heads.get(*col).cloned().unwrap_or_default()),
                            PvCell::value(before, Sty::Old, 80),
                            PvCell::value(&after, Sty::New, 80),
                        ]).go(), Some((ch.id, *col)));
                    }
                }
                if res.cells > shown { push(more(res.cells, shown), None); }
            }
            let preview = |fields: &[String], sty: Sty| -> PvCell {
                let mut cell = PvCell::default();
                for (k, f) in fields.iter().take(4).enumerate() {
                    if k > 0 { cell.parts.push((" · ".into(), Sty::Faint)); }
                    cell.parts.push(form::value_part(f, sty, 80));
                }
                cell.span(3)
            };
            if !res.added.is_empty() {
                push(PvRow::sec(vec![PvCell::txt("Rows added").span(4)]), None);
                let mut row = Vec::new();
                for &id in res.added.iter().take(ROWS_MAX) {
                    st.row(id, &mut row);
                    push(PvRow::new(vec![PvCell::new(format!("row {}", form::n(rec(&st, id))), Sty::Add), preview(&row, Sty::Txt)]).go(), Some((id, 0)));
                }
                if res.added.len() > ROWS_MAX { push(more(res.added.len(), ROWS_MAX), None); }
            }
            if !res.deleted.is_empty() {
                push(PvRow::sec(vec![PvCell::txt("Rows deleted").span(4)]), None);
                let mut f = Vec::new();
                for &i in res.deleted.iter().take(ROWS_MAX) {
                    st.base.fields(i, &mut f);
                    push(PvRow::new(vec![PvCell::new(format!("line {}", form::n(i + 1)), Sty::Del), preview(&f, Sty::Old)]), None);
                }
                if res.deleted.len() > ROWS_MAX { push(more(res.deleted.len(), ROWS_MAX), None); }
            }
            // The edits themselves, as Undo will take them back.
            if !self.log.is_empty() {
                push(PvRow::sec(vec![PvCell::txt("Edits, most recent first (Ctrl+Z takes them back in this order)").span(4)]), None);
                let mut typed = 0;
                let flush = |typed: &mut usize, push: &mut dyn FnMut(PvRow, Option<(RowId, usize)>)| {
                    if *typed > 0 {
                        push(PvRow::new(vec![PvCell::new(if *typed == 1 { "a cell typed".to_string() } else { format!("{} cells typed", form::n(*typed)) }, Sty::Sub).span(4)]), None);
                        *typed = 0;
                    }
                };
                for (k, e) in self.log.iter().enumerate().take(ROWS_MAX) {
                    match e {
                        Some(w) => {
                            flush(&mut typed, &mut push);
                            push(PvRow::new(vec![PvCell::new(form::n(k + 1), Sty::Faint), PvCell::txt(w).span(3)]), None);
                        }
                        None => typed += 1,
                    }
                }
                flush(&mut typed, &mut push);
                if self.log.len() > ROWS_MAX { push(more(self.log.len(), ROWS_MAX), None); }
            }
            if !rows.is_empty() {
                if let Some(i) = form::pv(ui, &p, "rv-list", &[], &rows, 380.0, 260.0) { go = targets[i]; }
            }
            ui.add_space(16.0);
            let any = !res.changed.is_empty() || !res.added.is_empty();
            match form::actions(ui, &p, &[Btn::outline("Close"), Btn::primary("Show changed rows").enabled(any)]) {
                Some(0) => cancel = true,
                Some(_) => do_mark = true,
                None => {}
            }
        });
        if let (Some((id, col)), Some(res)) = (go, &self.res) {
            if go_to(cx.tab, id, col) { return false; }
            // Hidden by a filter: the changed rows are shown, then the row.
            mark(cx.tab, &self.st, res, cx.ctx);
            self.pending = Some((id, col));
            return true;
        }
        if do_mark {
            if let Some(res) = &self.res { mark(cx.tab, &self.st, res, cx.ctx); }
            return false;
        }
        !(cancel || close)
    }
}
