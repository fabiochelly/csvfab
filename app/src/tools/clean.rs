//! Clean up: one pass over every cell (filtered or not) and the header line —
//! garbled accents, invisible characters, odd spaces, line breaks — then the
//! empty rows and columns. Each option shows what it would touch, counted
//! once when the dialog opens.

use super::edit_kit::{self as form, Btn, Snap};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::edit::clean::{self, CleanOp, CleanPlan, CleanScan, CLEAN_OPS};
use eframe::egui::{vec2, Align, Layout, Sense};

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Clean up…", group: "Transform the file", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(CleanTool::new(cx.tab)))) }]
}

pub struct CleanTool {
    on: [bool; 8],
    generation: Option<u64>,
    scan: Option<Job<Option<CleanScan>>>,
    counts: Option<CleanScan>,
    apply: Option<Job<Option<CleanPlan>>>,
}

impl CleanTool {
    pub fn new(_tab: &Tab) -> CleanTool {
        // The options ticked last time (this session), as the web's dialog keeps them.
        let on = LAST_ON.get().unwrap_or(CLEAN_OPS.map(|o| o.2));
        CleanTool { on, generation: None, scan: None, counts: None, apply: None }
    }

    fn ticked(&self) -> Vec<CleanOp> {
        CLEAN_OPS.iter().zip(self.on).filter(|(_, on)| *on).map(|(o, _)| o.0).collect()
    }
}

thread_local! {
    static LAST_ON: std::cell::Cell<Option<[bool; 8]>> = const { std::cell::Cell::new(None) };
}

impl Tool for CleanTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        LAST_ON.set(Some(self.on));
        let Some(snap) = Snap::of(cx.tab) else { return false };
        // Counted at opening, and again after an edit (an undo while it is open).
        if self.generation != Some(cx.tab.generation) && self.apply.is_none() {
            self.generation = Some(cx.tab.generation);
            let s = snap.clone();
            self.scan = Some(Job::spawn(cx.ctx, move |c, _| clean::scan(&s.st, c)));
        }
        if let Some(r) = self.scan.as_mut().and_then(|j| j.poll()) {
            self.scan = None;
            self.counts = r;
        }
        if let Some(r) = self.apply.as_mut().and_then(|j| j.poll()) {
            self.apply = None;
            if let (Some(plan), Some(t)) = (r, cx.tab.table.as_mut()) {
                let old_n = t.st.ncols;
                let gone = plan.cols.clone();
                let msg = clean::apply(t, plan);
                if gone.is_empty() {
                    cx.tab.after_edit(true, cx.ctx);
                } else {
                    form::remap_columns(cx.tab, old_n, |c| form::after_delete(&gone, c), cx.ctx);
                }
                cx.toast(msg);
            }
            return false;
        }
        let (p, m) = (cx.p, cx.m);
        let heads = snap.st.headers();
        let (mut go, mut cancel) = (false, false);
        let ticked = self.ticked();
        let any = self.counts.as_ref().is_some_and(|s| ticked.iter().any(|&o| s.count(o) > 0));
        let open = form::modal(cx.ctx, p, m, "clean", 520.0, "Clean up", |ui| {
            form::label(ui, p, "Every row, filtered or not, and the header line.", 0.0);
            for (k, (op, label, _)) in CLEAN_OPS.iter().enumerate() {
                let n = self.counts.as_ref().map(|s| s.count(*op));
                let none = n == Some(0);
                let w = ui.available_width();
                ui.allocate_ui_with_layout(vec2(w, 28.0), Layout::left_to_right(Align::Center), |ui| {
                    let r = form::checkbox_with(ui, p, &mut self.on[k], label, if none { p.txt_dim } else { p.txt_hi }, 8.0);
                    let text = n.map_or_else(|| "…".into(), |n| op.count_label(n));
                    let col = if none || n.is_none() { p.txt_faint } else { p.warn };
                    let g = ui.painter().layout_no_wrap(text, form::ui_font(12.0), col);
                    let x = ui.max_rect().max.x - g.size().x;
                    ui.painter().galley(eframe::egui::pos2(x, (ui.max_rect().center().y - g.size().y / 2.0).round()), g, col);
                    let _ = ui.allocate_exact_size(vec2(0.0, 28.0), Sense::hover());
                    if *op == CleanOp::Cols {
                        if let Some(s) = self.counts.as_ref().filter(|s| !s.empty_cols.is_empty()) {
                            let names: Vec<&str> = s.empty_cols.iter().filter_map(|&c| heads.get(c).map(|h| h.as_str())).collect();
                            r.on_hover_text(names.join(", "));
                        }
                    }
                });
                ui.add_space(6.0);
            }
            ui.add_space(6.0);
            let note = match &self.counts { Some(s) => clean::scan_note(s, &ticked), None => "Counting…" };
            form::stat(ui, p, note, p.txt_hi);
            ui.add_space(16.0);
            form::actions(ui, |ui| {
                go = form::button(ui, p, Btn::Prim, "Clean", any && self.apply.is_none()).clicked();
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
        });
        if go {
            if let Some(s) = self.counts.clone() {
                let on = ticked.clone();
                self.apply = Some(Job::spawn(cx.ctx, move |c, _| clean::plan(&snap.st, &on, &s, c)));
            }
        }
        (open && !cancel) || self.apply.is_some()
    }
}
