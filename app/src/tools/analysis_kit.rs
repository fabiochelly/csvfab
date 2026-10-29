//! Parts the analysis tools share (duplicates, group by, pivot, Insights,
//! schemas, profile, map, diagnosis), drawn to the web app's measures
//! (docs/look.md, "Buttons and fields", "Dialogs"): egui's own widgets have
//! neither the paddings nor the focus ring. A form dialog (`.modal-box`), its
//! title, labels and notes, buttons, fields, selects, checkboxes, radios, a
//! progress line and a scrolling table with a sticky head (`.pv`).

// A kit: not every part is used by every tool.
#![allow(dead_code)]

use super::Cx;
use crate::grid::semibold;
use crate::theme::{alpha, Palette};
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui};

pub fn ui(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::fonts::UI_MEDIUM.into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// On a whole physical pixel (texts, fills); `+ 0.5 / ppp` for a 1 px rule.
pub fn snap(ui: &Ui, x: f32) -> f32 {
    let ppp = ui.ctx().pixels_per_point();
    (x * ppp).round() / ppp
}

/// A form dialog (`.modal-box`): `surf`, 1 px `brd`, radius 8, padding 20,
/// shadow 0 10 30 / .5, over the backdrop; at most 80 % of the window tall
/// (its content scrolls). Escape or a click beside it closes it: returns the
/// content's result and whether the dialog stays open.
pub fn modal<R>(cx: &mut Cx, id: &str, width: f32, add: impl FnOnce(&mut Ui, &mut Cx) -> R) -> (R, bool) {
    let p = cx.p;
    let frame = crate::widgets::dialog_frame(p, egui::Margin::same(20));
    let ctx = cx.ctx.clone();
    let screen = ctx.content_rect();
    let w = (width.min(screen.width() - 32.0) - 40.0).max(0.0);
    let maxh = (screen.height() * 0.8 - 40.0).max(0.0);
    let r = crate::widgets::modal(&ctx, egui::Id::unique(("analysis-modal", id)), p, frame, |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        egui::ScrollArea::vertical().max_height(maxh).min_scrolled_height(maxh).auto_shrink([false, true]).show(ui, |ui| add(ui, cx)).inner
    });
    let close = r.should_close();
    (r.inner, !close)
}

/// A form dialog of a fixed size (the file profile, the map: a table or a
/// picture that takes the room left): `size` includes the padding `pad`
/// (left/right, top, bottom); the content gets exactly the rest, no scroll.
pub fn modal_fixed<R>(cx: &mut Cx, id: &str, size: egui::Vec2, pad: (i8, i8, i8), add: impl FnOnce(&mut Ui, &mut Cx) -> R) -> (R, bool) {
    let p = cx.p;
    let frame = crate::widgets::dialog_frame(p, egui::Margin { left: pad.0, right: pad.0, top: pad.1, bottom: pad.2 });
    let ctx = cx.ctx.clone();
    // A window smaller than the padding: no room, never a negative one.
    let inner = (size - vec2(2.0 * pad.0 as f32 + 2.0, (pad.1 + pad.2) as f32 + 2.0)).max(egui::Vec2::ZERO);
    let r = crate::widgets::modal(&ctx, egui::Id::unique(("analysis-modal", id)), p, frame, |ui| {
        ui.set_width(inner.x);
        ui.set_height(inner.y);
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        add(ui, cx)
    });
    let close = r.should_close();
    (r.inner, !close)
}

/// A dialog's title (`h4`): ui 600 16 `txt_hi`, 14 below.
pub fn title(ui: &mut Ui, p: &Palette, text: &str) {
    crate::widgets::dialog_title(ui, p, text);
}

/// A field's label (`.fld-label`): ui 12 `txt_mut`, 5 above the field.
pub fn label(ui: &mut Ui, p: &Palette, text: &str) {
    note_colored(ui, p, text, p.txt_mut, p.txt_hi, 12.0);
    ui.add_space(5.0);
}

/// A note (`#…-note`): ui 12 `txt_mut`, line 1.45; `**bold**` parts in ui 600 `warn`.
pub fn note(ui: &mut Ui, p: &Palette, text: &str) {
    note_colored(ui, p, text, p.txt_mut, p.warn, 12.0);
}

/// Wrapped text of `size`; `**…**` parts in ui 600 and `strong`.
pub fn note_colored(ui: &mut Ui, _p: &Palette, text: &str, color: Color32, strong: Color32, size: f32) {
    let job = rich_job(text, color, strong, size, ui.available_width());
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.min.x, snap(ui, r.min.y)), g, color);
}

/// `**bold**` markup as a wrapped layout, line height 1.45.
pub fn rich_job(text: &str, color: Color32, strong: Color32, size: f32, width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    for (k, part) in text.split("**").enumerate() {
        if part.is_empty() { continue; }
        let bold = k % 2 == 1;
        let fmt = egui::TextFormat {
            font_id: if bold { semibold(size) } else { ui(size) },
            color: if bold { strong } else { color },
            line_height: Some((size * 1.45).round()),
            valign: egui::Align::Center,
            ..Default::default()
        };
        job.append(part, 0.0, fmt);
    }
    job
}

#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    /// `.btn`: `prim`, `on_prim` text.
    Primary,
    /// `.btn-outline`: 1 px `brd`, `txt`.
    Outline,
    Danger,
    Warn,
}

/// A button (`.btn`): ui 500 13, padding 6 × 14 (30 tall), radius 6; disabled at .45.
pub fn button(ui: &mut Ui, p: &Palette, text: &str, kind: Btn, enabled: bool) -> Response {
    button_sized(ui, p, text, kind, enabled, 30.0, 14.0, 13.0)
}

/// A smaller button: `h` tall, `pad` each side, ui 500 `size`.
pub fn button_sized(ui: &mut Ui, p: &Palette, text: &str, kind: Btn, enabled: bool, h: f32, pad: f32, size: f32) -> Response {
    let kind = match kind {
        Btn::Primary => crate::widgets::Kind::Primary,
        Btn::Outline => crate::widgets::Kind::Outline,
        Btn::Danger => crate::widgets::Kind::Danger,
        Btn::Warn => crate::widgets::Kind::Warn,
    };
    crate::widgets::button(ui, p, kind, None, text, enabled, h, pad, size)
}

/// Buttons right-aligned, 8 apart (`.modal-actions`), `top` below what precedes.
/// `add` lays them out right to left: the first added is the rightmost.
pub fn actions<R>(ui: &mut Ui, top: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(top);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 30.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        add(ui)
    }).inner
}

/// The frame of a field (`.bs-input`): `bg_deep`, 1 px `brd`, radius 6; focused:
/// border `prim` and a 4 px ring; invalid: border `danger` (`widgets::field_frame`).
pub fn field_frame(ui: &Ui, p: &Palette, r: Rect, focused: bool, bad: bool, radius: f32) {
    let id = egui::Id::unique(("analysis-field", (r.min.x * 4.0) as i32, (r.min.y * 4.0) as i32));
    crate::widgets::field_frame(ui, p, id, r, focused, bad, radius);
}

/// A text field of `size` (`.bs-input`): text ui `font` (or mono), padding
/// `pad_x`, placeholder `txt_sub`. `bad`: a red border (a formula that does not compile).
#[allow(clippy::too_many_arguments)]
pub fn text_field(ui: &mut Ui, p: &Palette, id: egui::Id, size: egui::Vec2, text: &mut String, hint: &str, font: FontId, bad: bool) -> Response {
    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
    let pad = if size.y < 26.0 { 6.0 } else { 12.0 };
    crate::widgets::text_field(ui, p, id, r, text, hint, font, pad, bad)
}

/// A select (`.bs-select`): a field showing `options[*sel]` with the chevron
/// 16 × 12 12 px from its right; a click lists the options (a menu, scrolling
/// past 320). True when the choice changed.
pub fn select(ui: &mut Ui, p: &Palette, id: egui::Id, size: egui::Vec2, sel: &mut usize, options: &[String]) -> bool {
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let font = if size.y < 26.0 { 12.0 } else if size.y < 30.0 { 13.0 } else { 14.0 };
    crate::widgets::select(ui, p, id, r, &resp, sel, options, font)
}

/// The select's chevron: `M2 5l6 6 6-6` of 16, stroke 2.
pub fn chevron(pt: &egui::Painter, r: Rect, c: Color32) {
    let s = r.width() / 16.0;
    let at = |x: f32, y: f32| pos2(r.min.x + x * s, r.center().y + (y - 8.0) * s);
    pt.add(egui::Shape::line(vec![at(2.0, 5.0), at(8.0, 11.0), at(14.0, 5.0)], Stroke::new(2.0, c)));
}

/// A checkbox (16 × 16, radius 4) and its label 8 px after (ui 13 `txt_hi`). Toggles `on`.
pub fn checkbox(ui: &mut Ui, p: &Palette, on: &mut bool, text: &str) -> Response {
    let r = tick(ui, p, *on, false, text, true);
    if r.clicked() { *on = !*on; }
    r
}

/// A radio (Ø 16): true when clicked.
pub fn radio(ui: &mut Ui, p: &Palette, on: bool, text: &str) -> Response {
    tick(ui, p, on, true, text, true)
}

/// A checkbox or radio with its label; `enabled` false draws it at .45.
pub fn tick(ui: &mut Ui, p: &Palette, on: bool, round: bool, text: &str, enabled: bool) -> Response {
    crate::widgets::tick_row(ui, p, on, round, text, p.txt_hi, 8.0, 22.0, "", enabled)
}

fn ui_font(size: f32) -> FontId {
    ui(size)
}

/// The box alone: `bg_deep`, 1 px `brd_2` (hover `txt_dim`); on: `prim` with a white check / dot.
pub fn tick_box(pt: &egui::Painter, p: &Palette, b: Rect, on: bool, round: bool, hot: bool, enabled: bool) {
    crate::widgets::tick_box(pt, p, b, on, round, hot, enabled);
}

/// A thin progress line (2 px `prim` on `surf_3`) with what is being done, ui 12 `txt_mut`.
pub fn progress(ui: &mut Ui, p: &Palette, frac: Option<f32>, text: &str) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let pt = ui.painter();
    pt.text(pos2(r.min.x, r.min.y + 8.0), Align2::LEFT_CENTER, text, ui_font(12.0), p.txt_mut);
    let bar = Rect::from_min_size(pos2(r.min.x, r.min.y + 20.0), vec2(w, 3.0));
    pt.rect_filled(bar, 1.5, p.surf_3);
    let t = ui.input(|i| i.time) as f32;
    let part = match frac {
        Some(f) => Rect::from_min_size(bar.min, vec2(w * f.clamp(0.0, 1.0), 3.0)),
        // Unknown share: a segment sweeping across.
        None => {
            let x = (t * 0.8).fract() * (w + 80.0) - 80.0;
            Rect::from_min_max(pos2(bar.min.x + x.max(0.0), bar.min.y), pos2((bar.min.x + x + 80.0).min(bar.max.x), bar.max.y))
        }
    };
    pt.rect_filled(part, 1.5, p.prim);
    ui.ctx().request_repaint();
}

/// A clickable text in `accent_2` (`.dk-add`), underlined on hover.
pub fn link(ui: &mut Ui, p: &Palette, text: &str, size: f32) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), ui_font(size), p.accent_2);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x, g.size().y + 4.0), Sense::click());
    let y = snap(ui, r.center().y - g.size().y / 2.0);
    ui.painter().galley(pos2(r.min.x, y), g.clone(), p.accent_2);
    if resp.hovered() {
        let ly = snap(ui, y + g.size().y - 1.0) + 0.5;
        ui.painter().hline(r.x_range(), ly, Stroke::new(1.0, p.accent_2));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A × button (`.dk-x`): 24 × 24, radius 5, ui 16 `txt_dim`; hover fill
/// `alpha(danger, .2)`, text `danger_txt`.
pub fn x_button(ui: &mut Ui, p: &Palette) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    let hot = resp.hovered();
    if hot { ui.painter().rect_filled(r, 5.0, alpha(p.danger, 0.2)); }
    crate::icons::cross(ui.painter(), Rect::from_center_size(r.center(), vec2(9.0, 9.0)), 1.6, if hot { p.danger_txt } else { p.txt_dim });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A preview table (`.pv` in `.pv-wrap`): ui 12, cells padding 4 × 8, 1 px
/// `brd` rules, the head sticky on `surf`, rows drawn only where seen.
pub struct Table<'a> {
    pub id: egui::Id,
    /// (title, width, right-aligned).
    pub cols: Vec<(String, f32, bool)>,
    pub rows: usize,
    pub max_h: f32,
    pub row_h: f32,
    #[allow(clippy::type_complexity)]
    pub cell: &'a mut dyn FnMut(usize, usize) -> (String, Color32),
}

impl Table<'_> {
    /// Draws the table; returns the (row, column) clicked.
    pub fn show(self, ui: &mut Ui, p: &Palette) -> Option<(usize, usize)> {
        let Table { id, cols, rows, max_h, row_h, cell } = self;
        let head_h = 25.0;
        let total_w: f32 = cols.iter().map(|c| c.1).sum();
        let avail = ui.available_width();
        let h = (head_h + rows as f32 * row_h + 2.0).min(max_h);
        let (outer, _) = ui.allocate_exact_size(vec2(avail, h), Sense::hover());
        ui.painter().rect_stroke(outer, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
        let inner = outer.shrink(1.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(id));
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        let mut hit = None;
        let font = ui_font(12.0);
        egui::ScrollArea::both().id_salt(id).auto_shrink([false, false]).max_height(inner.height()).show_viewport(&mut child, |ui, vp| {
            let w = total_w.max(inner.width());
            ui.set_min_size(vec2(w, head_h + rows as f32 * row_h));
            let origin = ui.min_rect().min;
            let first = ((vp.min.y - head_h).max(0.0) / row_h) as usize;
            let last = (((vp.max.y - head_h).max(0.0) / row_h) as usize + 1).min(rows);
            let pt = ui.painter().clone();
            let scale = w / total_w.max(1.0);
            let xs: Vec<f32> = cols.iter().scan(0.0, |x, c| { let s = *x; *x += c.1 * scale.max(1.0); Some(s) }).collect();
            for r in first..last {
                let y = origin.y + head_h + r as f32 * row_h;
                let rr = Rect::from_min_size(pos2(origin.x, y), vec2(w, row_h));
                let resp = ui.interact(rr, id.with(("row", r)), Sense::click());
                if resp.hovered() { pt.rect_filled(rr, 0.0, alpha(p.hi, 0.06)); }
                for (c, col) in cols.iter().enumerate() {
                    let cw = col.1 * scale.max(1.0);
                    let cr = Rect::from_min_size(pos2(origin.x + xs[c], y), vec2(cw, row_h));
                    let (text, color) = cell(r, c);
                    cell_text(ui, &pt, cr, &text, font.clone(), color, col.2);
                    pt.vline(snap(ui, cr.max.x) - 0.5, cr.y_range(), Stroke::new(1.0, p.brd));
                    if resp.clicked() && ui.input(|i| i.pointer.interact_pos()).is_some_and(|q| cr.contains(q)) { hit = Some((r, c)); }
                }
                pt.hline(rr.x_range(), snap(ui, rr.max.y) - 0.5, Stroke::new(1.0, p.brd));
            }
            // The head, stuck to the top of the view.
            let hy = vp.min.y + origin.y;
            let hr = Rect::from_min_size(pos2(origin.x, hy), vec2(w, head_h));
            pt.rect_filled(hr, 0.0, p.surf);
            for (c, col) in cols.iter().enumerate() {
                let cr = Rect::from_min_size(pos2(origin.x + xs[c], hy), vec2(col.1 * scale.max(1.0), head_h));
                cell_text(ui, &pt, cr, &col.0, semibold(12.0), p.txt_hi, col.2);
                pt.vline(snap(ui, cr.max.x) - 0.5, cr.y_range(), Stroke::new(1.0, p.brd));
            }
            pt.hline(hr.x_range(), snap(ui, hr.max.y) - 0.5, Stroke::new(1.0, p.brd));
        });
        hit
    }
}

/// A cell's text: padding 8, one line cut with an ellipsis, vertically centred.
pub fn cell_text(ui: &Ui, pt: &egui::Painter, r: Rect, text: &str, font: FontId, color: Color32, right: bool) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: (r.width() - 16.0).max(4.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = pt.layout_job(job);
    let x = if right { r.max.x - 8.0 - g.size().x } else { r.min.x + 8.0 };
    pt.galley(pos2(snap(ui, x), snap(ui, r.center().y - g.size().y / 2.0)), g, color);
}

/// A rounded box (`.dk-key`): 1 px `brd`, radius 8, padding 8 × 10, fill `alpha(gray, .05)`.
pub fn boxed<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new().stroke(Stroke::new(1.0, p.brd)).corner_radius(CornerRadius::same(8)).fill(alpha(p.gray, 0.05))
        .inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| { ui.set_width(ui.available_width()); add(ui) }).inner
}

/// A count as shown to the user: `1 234 567`.
pub fn n(x: usize) -> String {
    crate::grid::fmt_int(x)
}

/// "1 row" / "3 rows".
pub fn plural(x: usize, one: &str, many: &str) -> String {
    format!("{} {}", n(x), if x == 1 { one } else { many })
}

/// Screenshot hook `run:<name>`: runs the first command whose name starts
/// with `<name>` (`_` for a space, any case) once the file is shown —
/// `CSVFAB_SHOT=run:Pivot`. The app calls it each frame on the active tab.
#[allow(dead_code)]
pub fn shots(cx: &mut Cx) {
    if cx.tab.table.is_none() || cx.tab.busy() { return; }
    if let Some(name) = crate::shot::take("run") {
        let name = name.replace('_', " ").to_lowercase();
        if let Some(c) = super::commands().into_iter().find(|c| c.name.to_lowercase().starts_with(&name)) { (c.run)(cx); }
    }
}
