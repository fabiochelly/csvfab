//! Edit filtered rows: one column, every row shown — set, clear, trim, change
//! case. Only the cells that actually change are written, as one undo step.

use super::edit_kit::{self as form, Btn, Live, Snap};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use csvfab_engine::edit::replace::{bulk_done, bulk_note, Bulk};
use csvfab_engine::edit::{self, Change};
use eframe::egui::{vec2, Align, Id, Layout, Rect, Sense};

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Edit filtered rows…", group: "Rows", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(BulkEdit::new(cx.tab)))) }]
}

const OPS: [&str; 6] = ["Set to", "Clear", "Trim, and collapse repeated spaces", "UPPER CASE", "lower case", "Capitalized Words"];

pub struct BulkEdit {
    col: usize,
    op: usize,
    value: String,
    /// Cells that would change, for (column, edit, data, rows shown).
    count: Live<(usize, Bulk, u64, usize), usize>,
    apply: Option<Job<Option<Vec<Change>>>>,
}

impl BulkEdit {
    pub fn new(tab: &crate::tab::Tab) -> BulkEdit {
        BulkEdit { col: form::cursor_col(tab), op: 0, value: String::new(), count: Live::default(), apply: None }
    }

    fn bulk(&self) -> Bulk {
        match self.op {
            0 => Bulk::Set(self.value.clone()),
            1 => Bulk::Clear,
            2 => Bulk::Trim,
            3 => Bulk::Upper,
            4 => Bulk::Lower,
            _ => Bulk::Title,
        }
    }
}

impl Tool for BulkEdit {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let heads = snap.st.headers();
        self.col = self.col.min(heads.len().saturating_sub(1));
        let key = (self.col, self.bulk(), cx.tab.generation, cx.tab.shown());
        if self.count.stale(&key) {
            let (col, op) = (self.col, self.bulk());
            self.count.run(cx.ctx, key.clone(), move |c| edit::scan(&snap.st, &snap.rows(), &[col], &op, false, c).map(|s| s.changed));
        }
        self.count.poll();
        if let Some(r) = self.apply.as_mut().and_then(|j| j.poll()) {
            self.apply = None;
            if let Some(ch) = r {
                let n = ch.len();
                cx.tab.apply_changes(&ch, cx.ctx);
                cx.toast(bulk_done(n, heads.get(self.col).map_or("", |h| h.as_str())));
            }
            return false;
        }
        let (p, m) = (cx.p, cx.m);
        let shown = cx.tab.shown();
        let filtered = form::filtered(cx.tab);
        let fresh = self.count.fresh(&key).copied();
        let mut go = false;
        let mut cancel = false;
        let open = form::modal(cx.ctx, p, m, "bulk", 380.0, "Edit filtered rows", |ui| {
            form::label(ui, p, "Column", 0.0);
            form::select(ui, p, m, Id::unique("bulk-col"), &mut self.col, &heads, f32::INFINITY);
            form::label(ui, p, "Change", 12.0);
            for (k, t) in OPS.iter().enumerate() {
                let w = ui.available_width();
                ui.allocate_ui_with_layout(vec2(w, 30.0), Layout::left_to_right(Align::Center), |ui| {
                    if form::radio(ui, p, self.op == k, t) {
                        self.op = k;
                    }
                    if k == 0 {
                        // The value field fills the line, 8 px after "Set to".
                        let x = ui.cursor().min.x + 8.0;
                        let r = Rect::from_min_max(egui_pos(x, ui.max_rect().center().y - 13.0), egui_pos(ui.max_rect().max.x, ui.max_rect().center().y + 13.0));
                        ui.allocate_rect(r, Sense::hover());
                        if form::field_at(ui, p, Id::unique("bulk-val"), r, &mut self.value, "value", form::ui_font(13.0), 8.0, false).changed() {
                            self.op = 0;
                        }
                    }
                });
                ui.add_space(2.0);
            }
            ui.add_space(10.0);
            let note = bulk_note(fresh.or(self.count.val).unwrap_or(0), shown, filtered);
            form::stat(ui, p, &note, p.warn);
            ui.add_space(16.0);
            form::actions(ui, |ui| {
                go = form::button(ui, p, Btn::Prim, "Apply", fresh.is_some_and(|n| n > 0) && self.apply.is_none()).clicked();
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
        });
        if go {
            let Some(snap) = Snap::of(cx.tab) else { return false };
            let (col, op) = (self.col, self.bulk());
            self.apply = Some(Job::spawn(cx.ctx, move |c, _| edit::changes(&snap.st, &snap.rows(), &[col], &op, c)));
        }
        (open && !cancel) || self.apply.is_some()
    }
}

fn egui_pos(x: f32, y: f32) -> eframe::egui::Pos2 {
    eframe::egui::pos2(x, y)
}
