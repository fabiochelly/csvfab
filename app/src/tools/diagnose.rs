//! "Why does this row look wrong?": one record read as it is in the file —
//! its bytes, not only its text — and what is off and why (engine/diagnose.rs):
//! the field count against the usual one and the probable cause, quote
//! anomalies, encoding evidence, invisible characters, formulas; each finding
//! with the bytes around it, in hex. A repair, when one is safe, applies as
//! one undo step. As 37-diagnose.js (the web showed it in the row card; here
//! a panel docked at the right, the grid still in view).

use super::analysis_kit::{self as kit, Btn};
use super::{Command, Cx, Tool};
use crate::grid::semibold;
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Palette};
use csvfab_engine::diagnose::{self as eng, Diagnosis, Finding, Near, RepairKind, Severity};
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, Ui};

pub fn commands() -> Vec<Command> {
    vec![Command {
        name: "Why does this row look wrong?",
        group: "Rows",
        keys: None,
        enabled: |t| t.table.is_some() && t.grid.cursor.is_some_and(|c| c.row < t.shown()),
        run: |cx| {
            // `diagrow:N`: the row shown at N, for a screenshot without a cursor.
            let pos = cx.tab.grid.cursor.map(|c| c.row).or_else(|| crate::shot::take("diagrow").and_then(|s| s.parse().ok()));
            if let Some(pos) = pos.filter(|&p| p < cx.tab.shown()) {
                let id = cx.tab.shown_id(pos);
                cx.out.push(super::Cmd::Tool(open(cx.tab, id)));
            }
        },
    }]
}

/// The diagnosis of row `id` (the grid's row menu: `Action::Diagnose(id)`).
pub fn open(tab: &Tab, id: RowId) -> Box<dyn Tool> {
    Box::new(Diagnose { id, state: tab.st().cloned(), job: None, d: None, number: None, at: tab.grid.cursor.map(|c| c.row) })
}

struct Diagnose {
    id: RowId,
    /// The rows to read, until the job starts (on the first frame, which has the context).
    state: Option<csvfab_engine::State>,
    job: Option<Job<Diagnosis>>,
    d: Option<Diagnosis>,
    /// The row's number as the gutter shows it.
    number: Option<usize>,
    /// The cursor's row when last looked at: moved (the row card's ▲/▼, a click), the diagnosis
    /// follows it, as the web's did inside the card.
    at: Option<usize>,
}

impl Diagnose {
    fn run(&mut self, cx: &Cx) {
        if let Some(st) = self.state.take().or_else(|| cx.tab.st().cloned()) {
            let id = self.id;
            // Cheap for most files, but the usual width walks every irregular row: off the UI thread.
            self.job = Some(Job::spawn(cx.ctx, move |_, _| eng::diagnose(&st, id)));
        }
    }
}

impl Tool for Diagnose {
    fn single(&self) -> Option<&'static str> {
        Some("diagnose")
    }

    fn show(&mut self, cx: &mut Cx) -> bool {
        let at = cx.tab.grid.cursor.map(|c| c.row).filter(|&r| r < cx.tab.shown());
        if at.is_some() && at != self.at {
            self.at = at;
            let id = cx.tab.shown_id(at.unwrap());
            if id != self.id {
                (self.id, self.job, self.d, self.number, self.state) = (id, None, None, None, None);
            }
        }
        if self.job.is_none() && self.d.is_none() { self.run(cx); }
        if let Some(d) = self.job.as_mut().and_then(|j| j.poll()) { self.job = None; self.d = Some(d); }
        if self.number.is_none() {
            self.number = cx.tab.st().map(|st| self.id as usize + usize::from(!st.header));
        }
        let p = cx.p;
        let ctx = cx.ctx.clone();
        let screen = ctx.content_rect();
        let w = 460.0_f32.min(screen.width() - 32.0);
        let frame = egui::Frame::new().fill(p.surf).stroke(Stroke::new(1.0, p.brd)).corner_radius(8).inner_margin(Margin { left: 14, right: 12, top: 10, bottom: 12 })
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(128) });
        let headers = cx.tab.st().map(|s| s.headers()).unwrap_or_default();
        let mut open = true;
        let mut apply = false;
        // Left of the row card when it is open.
        let card_w = ctx.data(|d| d.get_temp::<f32>(egui::Id::unique(crate::rowcard::WIDTH_KEY))).unwrap_or(0.0);
        egui::Window::new("diagnose").id(egui::Id::unique(("diagnose", cx.tab.uid))).title_bar(false).frame(frame).resizable(false).collapsible(false)
            .default_width(w - 26.0).min_width(w - 26.0).max_width(w - 26.0).anchor(egui::Align2::RIGHT_TOP, vec2(-16.0 - card_w, 100.0)).show(&ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.set_width(w - 26.0);
                // Head: what this is, the row, ×.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Why does this row look wrong?").font(semibold(14.0)).color(p.txt_hi));
                    ui.add_space(8.0);
                    if let Some(n) = self.number { ui.label(RichText::new(format!("row {}", kit::n(n))).font(kit::ui(12.0)).color(p.txt_dim)); }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                        if resp.hovered() { ui.painter().rect_filled(r, 5.0, p.surf_2); }
                        crate::icons::cross(ui.painter(), Rect::from_center_size(r.center(), vec2(10.0, 10.0)), 1.6, if resp.hovered() { p.txt_hi } else { p.txt_dim });
                        if resp.on_hover_text("Close").clicked() { open = false; }
                    });
                });
                ui.add_space(6.0);
                let maxh = screen.height() - 100.0 - 80.0;
                egui::ScrollArea::vertical().max_height(maxh).auto_shrink([false, true]).show(ui, |ui| {
                    ui.set_width(w - 26.0 - 6.0);
                    match &self.d {
                        None => kit::progress(ui, p, None, "Reading the row's bytes…"),
                        Some(d) => apply = body(ui, p, d, &headers),
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && ctx.memory(|m| m.focused().is_none()) { open = false; }
        if apply {
            if let Some(d) = self.d.take() { repair(cx, &d); }
            // Diagnosed again: the row now differs from its record (or the record is fixed).
            self.run(cx);
        }
        open
    }
}

/// The repair as one undo step; a Join also deletes the line joined in.
fn repair(cx: &mut Cx, d: &Diagnosis) {
    let Some(r) = &d.repair else { return };
    let Some(t) = &mut cx.tab.table else { return };
    let cells = r.cells(d.row, d.observed);
    let next = r.next;
    t.batch(|t| {
        t.set_cells(&cells);
        if let Some(n) = next { t.delete_rows(&std::iter::once(n).collect()); }
    });
    cx.tab.after_edit(true, cx.ctx);
    let what = match r.kind { RepairKind::Join => "joined", RepairKind::Pad => "padded", RepairKind::Trim => "trimmed", RepairKind::Merge => "realigned", RepairKind::Reencode => "read as UTF-8" };
    cx.toast(format!("Row {what}: {} — not written yet, use Save.", r.label.to_lowercase()));
}

/// The diagnosis (`#rc-diag`, ui 12): the shape, the cause, the findings, the repair. True: Apply clicked.
fn body(ui: &mut Ui, p: &Palette, d: &Diagnosis, headers: &[String]) -> bool {
    if let Some(none) = d.none {
        text(ui, none, p.txt_dim, p.txt_dim, 12.0);
        return false;
    }
    // .dg-shape: expected, observed, where in the file.
    let pl = |n: usize| if n > 1 { "s" } else { "" };
    ui.add_space(6.0);
    let job = |ui: &Ui, parts: &[(&str, Color32, bool)]| {
        let mut j = egui::text::LayoutJob::default();
        j.wrap.max_width = ui.available_width();
        for (s, c, b) in parts {
            j.append(s, 0.0, egui::TextFormat { font_id: if *b { semibold(12.0) } else { kit::ui(12.0) }, color: *c, line_height: Some(17.0), ..Default::default() });
        }
        j
    };
    let exp = kit::n(d.expected);
    let dom = format!(" (header; most rows: {})", kit::n(d.dominant));
    let mut parts: Vec<(&str, Color32, bool)> = vec![("Expected ", p.txt, false), (&exp, p.txt_strong, true)];
    let fields = format!(" field{}", pl(d.expected));
    parts.push((&fields, p.txt, false));
    if d.dominant != d.expected { parts.push((&dom, p.txt_dim, false)); }
    let obs = kit::n(d.observed);
    let diff = format!(" ({}{})", if d.observed > d.expected { "+" } else { "−" }, kit::n(d.observed.abs_diff(d.expected)));
    parts.extend([("      Observed ", p.txt, false), (&obs, if d.observed != d.expected { p.danger_txt } else { p.txt_strong }, true)]);
    if d.observed != d.expected { parts.push((&diff, p.txt_dim, false)); }
    let g = ui.painter().layout_job(job(ui, &parts));
    put(ui, g);
    let place = format!("line {} · bytes {}–{} · {}{}", d.line.map_or("—".into(), kit::n), kit::n(d.bytes.0 as usize), kit::n(d.bytes.1 as usize), d.encoding, if d.edited { " · edited since read" } else { "" });
    ui.add_space(4.0);
    text(ui, &place, p.txt_dim, p.txt_dim, 12.0);
    ui.add_space(6.0);
    if d.findings.is_empty() {
        text(ui, "Nothing wrong found in this line’s bytes.", p.txt_dim, p.txt_dim, 12.0);
        return false;
    }
    if let Some(c) = &d.cause {
        egui::Frame::new().fill(p.surf_2).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            text(ui, &format!("Probable cause: **{c}**"), p.txt, p.txt_strong, 12.0);
        });
        ui.add_space(6.0);
    }
    for f in &d.findings { finding(ui, p, f); }
    let mut apply = false;
    if let Some(r) = &d.repair {
        boxed(ui, p, p.ok, |ui| {
            text(ui, "**Suggested repair**", p.txt_strong, p.txt_strong, 12.0);
            ui.add_space(2.0);
            text(ui, &r.label, p.txt_mut, p.txt_mut, 12.0);
            ui.add_space(5.0);
            // The fields once repaired, each titled with its column.
            let mut j = egui::text::LayoutJob::default();
            j.wrap.max_width = ui.available_width();
            j.wrap.break_anywhere = true;
            let f = |c: Color32| egui::TextFormat { font_id: FontId::monospace(11.5), color: c, line_height: Some(17.0), ..Default::default() };
            for (k, v) in r.fields.iter().enumerate() {
                if k > 0 { j.append("│", 3.0, f(p.txt_faint)); }
                j.append(&shown(v), if k > 0 { 3.0 } else { 0.0 }, f(p.txt));
            }
            let g = ui.painter().layout_job(j);
            put(ui, g).on_hover_text(r.fields.iter().enumerate().map(|(c, v)| format!("{}: {v}", headers.get(c).cloned().unwrap_or_else(|| format!("field {}", c + 1)))).collect::<Vec<_>>().join("\n"));
            ui.add_space(7.0);
            let resp = kit::button_sized(ui, p, "Apply", Btn::Outline, true, 22.0, 10.0, 12.0);
            if resp.on_hover_text("An ordinary edit: Undo takes it back").clicked() { apply = true; }
        });
    }
    apply
}

/// One finding (`.dg-f`): `bg_deep`, a 3 px left edge by severity, radius 0 6 6 0, padding 6 × 10.
fn finding(ui: &mut Ui, p: &Palette, f: &Finding) {
    let edge = match f.severity { Severity::Error => p.danger, Severity::Warn => p.warn, Severity::Info => p.prim };
    boxed(ui, p, edge, |ui| {
        text(ui, &format!("**{}**", f.title), p.txt_strong, p.txt_strong, 12.0);
        ui.add_space(2.0);
        text(ui, &f.detail, p.txt_mut, p.txt_mut, 12.0);
        if let Some(n) = &f.near { ui.add_space(5.0); near(ui, p, n); }
    });
}

fn boxed(ui: &mut Ui, p: &Palette, edge: Color32, add: impl FnOnce(&mut Ui)) {
    let r = egui::Frame::new().fill(p.bg_deep).corner_radius(CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }).inner_margin(Margin { left: 13, right: 10, top: 6, bottom: 6 })
        .show(ui, |ui| { ui.set_width(ui.available_width()); add(ui); }).response.rect;
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(3.0, r.height())), 0.0, edge);
    ui.add_space(6.0);
}

/// The bytes around a finding (`.dg-near`, mono 11.5): its offset, the hex
/// with the finding's bytes in `danger_txt` on `alpha(danger, .16)`, then the text there.
fn near(ui: &mut Ui, p: &Palette, n: &Near) {
    let mut j = egui::text::LayoutJob::default();
    j.wrap.max_width = ui.available_width();
    let f = |c: Color32, bold: bool, bg: Color32| egui::TextFormat {
        font_id: if bold { FontId::new(11.5, egui::FontFamily::Name(crate::fonts::MONO_BOLD.into())) } else { FontId::monospace(11.5) },
        color: c, background: bg, line_height: Some(17.0), ..Default::default()
    };
    j.append(&format!("@{}", kit::n(n.at as usize)), 0.0, f(p.txt_dim, false, Color32::TRANSPARENT));
    for (k, (b, hit)) in n.hex.iter().enumerate() {
        let lead = if k == 0 { 10.0 } else { 0.0 };
        if k > 0 { j.append(" ", 0.0, f(p.txt_mut, false, Color32::TRANSPARENT)); }
        let (c, bg) = if *hit { (p.danger_txt, alpha(p.danger, 0.16)) } else { (p.txt_mut, Color32::TRANSPARENT) };
        j.append(&format!("{b:02X}"), lead, f(c, *hit, bg));
    }
    let g = ui.painter().layout_job(j);
    put(ui, g);
    // The text, one line, its control characters as symbols.
    let mut t = egui::text::LayoutJob::simple_singleline(control_symbols(&n.text), FontId::monospace(11.5), p.txt);
    t.wrap = egui::text::TextWrapping { max_width: ui.available_width(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(t);
    put(ui, g);
}

/// Line breaks, tabs and other control characters as the web shows them: ↵ ␍ ⇥ ·.
fn control_symbols(s: &str) -> String {
    s.chars().map(|c| match c { '\n' => '↵', '\r' => '␍', '\t' => '⇥', c if (c as u32) < 0x20 => '·', c => c }).collect()
}

/// A field in the repair's preview: its line breaks shown.
fn shown(v: &str) -> String {
    control_symbols(v)
}

fn put(ui: &mut Ui, g: std::sync::Arc<egui::Galley>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.min.x, kit::snap(ui, r.min.y)), g, Color32::WHITE);
    resp
}

/// Wrapped ui text, line 1.45; `**…**` in ui 600 `strong`.
fn text(ui: &mut Ui, s: &str, c: Color32, strong: Color32, size: f32) {
    let job = kit::rich_job(s, c, strong, size, ui.available_width());
    let g = ui.painter().layout_job(job);
    put(ui, g);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_shown_as_symbols() {
        assert_eq!(control_symbols("a\nb\r\tc\u{0}é"), "a↵b␍⇥c·é");
    }
}
