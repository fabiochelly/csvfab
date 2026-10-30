//! Form parts for the editing tools (find & replace, bulk edits, convert,
//! look-up, clean-up, split / merge, noise words), drawn to the web app's
//! measures (`.modal-box`, `.bs-input`, `.btn`, `.col-label`, `.chips .dk`,
//! `.stat-line`, `.pv`); and `Live`, a background computation redone whenever
//! its inputs change (the dialogs' counts and previews).

use crate::icons::Icon;
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use csvfab_engine::{RowId, State};
use eframe::egui::{
    self, pos2, text::LayoutJob, vec2, Align, Color32, FontFamily, FontId, Galley, Id, Layout, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextFormat, Ui,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::fonts::UI_MEDIUM.into()))
}

pub fn semibold(size: f32) -> FontId {
    crate::grid::semibold(size)
}

/// `n` as the app writes counts: 1 234 567.
pub fn n(n: usize) -> String {
    csvfab_engine::edit::fmt_int(n)
}

/// One line, cut with "…" past `max_w`.
pub fn line(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.replace(['\n', '\r'], "↵"), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// Text drawn vertically centred at `y`, its top on a whole pixel.
fn put(ui: &Ui, g: Arc<Galley>, x: f32, y: f32, color: Color32) {
    let ppp = ui.ctx().pixels_per_point();
    let top = ((y - g.size().y / 2.0) * ppp).round() / ppp;
    ui.painter().galley(pos2(x, top), g, color);
}

/* ---- The dialog ---------------------------------------------------- */

/// A form dialog (`.modal-box`): content `width` wide (+ padding 20: the web's box is content-box), centred,
/// radius 8, at most 80 % of the window tall (its content scrolls), the title
/// in ui 600 16. Escape or a click beside it closes it: returns false then.
pub fn modal(ctx: &egui::Context, p: &Palette, m: &Metrics, id: &str, width: f32, title: &str, add: impl FnOnce(&mut Ui)) -> bool {
    let _ = m;
    let frame = crate::widgets::dialog_frame(p, egui::Margin::same(20));
    let r = crate::widgets::modal(ctx, Id::unique(("tools-modal", id)), p, frame, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        // Rows as tall as their content (egui gives each line a 30 px minimum).
        ui.spacing_mut().interact_size.y = 0.0;
        crate::widgets::dialog_title(ui, p, title);
        // No scroll area around the content: in a centred area it would be sized to the half
        // screen below the centre on the first frame. The dialogs' long parts scroll themselves.
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            add(ui)
        });
    });
    !r.should_close()
}

/// A message dialog (`#dlg`): a message in ui 600 14.5, a detail under it, and
/// buttons; returns which was clicked (0: OK, 1: Cancel) — Escape is Cancel.
pub fn message(ctx: &egui::Context, p: &Palette, m: &Metrics, id: &str, text: &str, ok: &str, cancel: Option<&str>, danger: bool) -> Option<usize> {
    let (head, detail) = text.split_once("\n\n").unwrap_or((text, ""));
    let mut hit = None;
    let w = (ctx.content_rect().width() - 32.0).min(440.0);
    let _ = m;
    let frame = crate::widgets::dialog_frame(p, egui::Margin { left: 22, right: 22, top: 22, bottom: 18 });
    let r = crate::widgets::modal(ctx, Id::unique(("tools-msg", id)), p, frame, |ui| {
        ui.set_width((w - 44.0).max(0.0));
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        let wrap = |text: &str, font: FontId, color: Color32, lh: f32| {
            let mut job = LayoutJob::simple(text.to_string(), font, color, w - 44.0);
            job.sections.iter_mut().for_each(|s| s.format.line_height = Some(lh));
            job
        };
        let g = ui.painter().layout_job(wrap(head, semibold(14.5), p.txt_hi, 21.0));
        let (r, _) = ui.allocate_exact_size(vec2(w - 44.0, g.size().y), Sense::hover());
        ui.painter().galley(r.min, g, p.txt_hi);
        if !detail.is_empty() {
            ui.add_space(8.0);
            let g = ui.painter().layout_job(wrap(detail, ui_font(13.0), p.txt_mut, 19.5));
            let (r, _) = ui.allocate_exact_size(vec2(w - 44.0, g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, p.txt_mut);
        }
        ui.add_space(20.0);
        actions(ui, |ui| {
            if button(ui, p, if danger { Btn::Danger } else { Btn::Prim }, ok, true).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                hit = Some(0);
            }
            if let Some(c) = cancel {
                if button(ui, p, Btn::Outline, c, true).clicked() {
                    hit = Some(1);
                }
            }
        });
    });
    if hit.is_none() && r.should_close() {
        hit = Some(1);
    }
    hit
}

/// A field's label (`.fld-label`): ui 12 `txt_mut`, `top` above, 5 below.
pub fn label(ui: &mut Ui, p: &Palette, text: &str, top: f32) {
    ui.add_space(top);
    ui.label(RichText::new(text).font(ui_font(12.0)).color(p.txt_mut));
    ui.add_space(5.0);
}

/// A note or stat line (`.stat-line`, ui 12 / 1.6, `txt_mut`): a count at the
/// very start in ui 600 `bold`, the lines after the first in `warn`.
pub fn stat(ui: &mut Ui, p: &Palette, text: &str, bold: Color32) {
    let font = ui_font(12.0);
    let fmt = |c: Color32, f: &FontId| TextFormat { font_id: f.clone(), color: c, line_height: Some(19.0), ..Default::default() };
    let mut job = LayoutJob::default();
    for (k, l) in text.split('\n').enumerate() {
        if k > 0 {
            job.append("\n", 0.0, fmt(p.warn, &font));
            job.append(l, 0.0, fmt(p.warn, &font));
            continue;
        }
        let num = l.find(|c: char| !(c.is_ascii_digit() || c == '\u{202f}')).unwrap_or(l.len());
        if num > 0 {
            job.append(&l[..num], 0.0, fmt(bold, &semibold(12.0)));
        }
        job.append(&l[num..], 0.0, fmt(p.txt_mut, &font));
    }
    job.wrap.max_width = ui.available_width();
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
    ui.painter().galley(r.min, g, p.txt_mut);
}

/* ---- Buttons ------------------------------------------------------- */

#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    Prim,
    Outline,
    Warn,
    Danger,
}

/// `.btn`: ui 500 13, padding 6 × 14 (30 tall), radius 6, disabled at .45.
pub fn button(ui: &mut Ui, p: &Palette, kind: Btn, text: &str, enabled: bool) -> Response {
    button_icon(ui, p, kind, None, text, enabled)
}

pub fn button_icon(ui: &mut Ui, p: &Palette, kind: Btn, icon: Option<Icon>, text: &str, enabled: bool) -> Response {
    let kind = match kind {
        Btn::Prim => crate::widgets::Kind::Primary,
        Btn::Outline => crate::widgets::Kind::Outline,
        Btn::Warn => crate::widgets::Kind::Warn,
        Btn::Danger => crate::widgets::Kind::Danger,
    };
    crate::widgets::button(ui, p, kind, icon, text, enabled, 30.0, 14.0, 13.0)
}

/// The dialog's buttons, right-aligned 8 apart, `top` below the content: add them right to left.
pub fn actions(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 30.0), Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        add(ui)
    });
}

/* ---- Fields -------------------------------------------------------- */

/// The look of a `.bs-input` over `r`: `bg_deep`, 1 px `brd`, radius 6; focused:
/// border `prim` and a 4 px ring; `bad`: border `danger` (`widgets::field_frame`).
pub fn input_frame(ui: &Ui, p: &Palette, r: Rect, focused: bool, bad: bool) {
    let id = Id::unique(("edit-input", (r.min.x * 4.0) as i32, (r.min.y * 4.0) as i32));
    crate::widgets::field_frame(ui, p, id, r, focused, bad, 6.0);
}

/// A text field over `r` (`.bs-input`): text in `font`, `pad` from the sides.
#[allow(clippy::too_many_arguments)]
pub fn field_at(ui: &mut Ui, p: &Palette, id: Id, r: Rect, text: &mut String, hint: &str, font: FontId, pad: f32, bad: bool) -> Response {
    crate::widgets::text_field(ui, p, id, r, text, hint, font, pad, bad)
}

/// A field `w` wide (`f32::INFINITY`: the room left), 31 tall, ui 14, padding 6 × 12.
pub fn field(ui: &mut Ui, p: &Palette, id: Id, text: &mut String, hint: &str, w: f32, bad: bool) -> Response {
    let w = if w.is_finite() { w } else { ui.available_width() };
    let (r, _) = ui.allocate_exact_size(vec2(w, 31.0), Sense::hover());
    field_at(ui, p, id, r, text, hint, ui_font(14.0), 12.0, bad)
}

/// A text area (`textarea.bs-input`): `rows` lines of ui 13 / 1.5.
pub fn text_area(ui: &mut Ui, p: &Palette, id: Id, text: &mut String, hint: &str, rows: usize) -> Response {
    let h = rows as f32 * 19.5 + 14.0;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let focused = ui.memory(|m| m.has_focus(id));
    crate::widgets::field_frame(ui, p, id, r, focused, false, 6.0);
    let inner = r.shrink2(vec2(12.0, 7.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    let resp = egui::ScrollArea::vertical()
        .id_salt(id.with("scroll"))
        .max_height(inner.height())
        .show(&mut child, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .id(id)
                    .font(ui_font(13.0))
                    .text_color(p.txt_hi)
                    .hint_text(RichText::new(hint).font(ui_font(13.0)).color(p.txt_sub))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(inner.width())
                    .desired_rows(rows),
            )
        })
        .inner;
    resp
}

/// A select (`.bs-input.bs-select`): the chosen option, a chevron 12 px from the
/// right, a menu of the options under it. Returns true when the choice changes.
pub fn select(ui: &mut Ui, p: &Palette, m: &Metrics, id: Id, sel: &mut usize, opts: &[String], w: f32) -> bool {
    let _ = m;
    let w = if w.is_finite() { w } else { ui.available_width() };
    let (r, resp) = ui.allocate_exact_size(vec2(w, 31.0), Sense::click());
    crate::widgets::select(ui, p, id, r, &resp, sel, opts, 14.0)
}

/* ---- Checkboxes and radios ------------------------------------------ */

fn tick(ui: &mut Ui, p: &Palette, on: bool, text: &str, color: Color32, gap: f32, round: bool) -> Response {
    crate::widgets::tick_row(ui, p, on, round, text, color, gap, 20.0, "", true)
}

/// `.col-label` with a checkbox: ui 13 `txt_hi`, the box 8 px before the text.
pub fn checkbox(ui: &mut Ui, p: &Palette, on: &mut bool, text: &str) -> Response {
    checkbox_with(ui, p, on, text, p.txt_hi, 8.0)
}

pub fn checkbox_with(ui: &mut Ui, p: &Palette, on: &mut bool, text: &str, color: Color32, gap: f32) -> Response {
    let mut r = tick(ui, p, *on, text, color, gap, false);
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r
}

/// A radio button; true when clicked (it then becomes the choice).
pub fn radio(ui: &mut Ui, p: &Palette, on: bool, text: &str) -> bool {
    tick(ui, p, on, text, p.txt_hi, 8.0, true).clicked()
}

/// A wrapped row of options (`.opts`): 18 apart, lines 4 apart.
pub fn opts(ui: &mut Ui, top: f32, add: impl FnOnce(&mut Ui)) {
    ui.add_space(top);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(18.0, 4.0);
        add(ui)
    });
}

/// Ticked columns in the order ticked (`#merge-cols`, `#lk-cols`): a box at most
/// 170 tall, each ticked one with its rank in a `prim` badge at the right.
pub fn ordered_list(ui: &mut Ui, p: &Palette, id: Id, items: &[(usize, String)], order: &mut Vec<usize>) -> bool {
    let mut changed = false;
    let frame = egui::Frame::new().stroke(Stroke::new(1.0, p.brd)).corner_radius(6).inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 0 });
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(id).max_height(170.0 - 8.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (c, h) in items {
                let k = order.iter().position(|x| x == c);
                ui.horizontal(|ui| {
                    let mut on = k.is_some();
                    if checkbox(ui, p, &mut on, h).clicked() {
                        order.retain(|x| x != c);
                        if on {
                            order.push(*c);
                        }
                        changed = true;
                    }
                    if let Some(k) = k {
                        let g = ui.painter().layout_no_wrap((k + 1).to_string(), ui_font(10.5), p.on_prim);
                        let w = (g.size().x + 6.0).max(16.0);
                        let right = ui.max_rect().max.x;
                        let r = Rect::from_min_size(pos2(right - w - 6.0, ui.min_rect().center().y - 8.0), vec2(w, 16.0));
                        ui.painter().rect_filled(r, 8.0, p.prim);
                        put(ui, g.clone(), r.center().x - g.size().x / 2.0, r.center().y, p.on_prim);
                    }
                });
            }
            ui.add_space(0.0);
        });
    });
    changed
}

/* ---- Chips --------------------------------------------------------- */

/// Quick picks (`.chips .dk`): key caps mono 12 / 18, 5 apart, 7 below the
/// field above. Returns the one clicked.
pub fn chips(ui: &mut Ui, p: &Palette, labels: &[String], tips: &[&str]) -> Option<usize> {
    let mut hit = None;
    ui.add_space(7.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
        for (k, l) in labels.iter().enumerate() {
            let g = ui.painter().layout_no_wrap(l.clone(), FontId::monospace(12.0), p.txt_strong);
            let (r, resp) = ui.allocate_exact_size(vec2((g.size().x + 16.0).max(22.0), 22.0), Sense::click());
            ui.painter().rect(r, 4.0, alpha(p.hi, if resp.hovered() { 0.2 } else { 0.1 }), Stroke::new(1.0, alpha(p.hi, 0.35)), StrokeKind::Inside);
            put(ui, g.clone(), r.center().x - g.size().x / 2.0, r.center().y, p.txt_strong);
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let resp = match tips.get(k) { Some(t) if !t.is_empty() => resp.on_hover_text(*t), _ => resp };
            if resp.clicked() {
                hit = Some(k);
            }
        }
    });
    hit
}

/* ---- Preview table ------------------------------------------------- */

/// A head cell of a preview (`.pv th`).
pub enum Th {
    Text(String),
    /// A column being made: in `prim`.
    New(String),
    /// A field editing `edits[k]` (the split's new titles).
    Edit(usize),
}

/// A body cell (`.pv td`).
pub enum Td {
    Text(String),
    /// What the value was (`td.src`): `txt_mut`.
    Src(String),
    /// A placeholder (`td.empty`): italic `txt_faint`.
    Empty(String),
    /// A source value with the parts taken out struck through.
    Marked(String, Vec<(usize, usize)>),
}

/// A preview table (`.pv` in `.pv-wrap`): ui 12, cells padding 4 × 8, 1 px
/// `brd` rules, at most 220 wide each (ellipsis), as wide as the dialog at
/// least, scrolling past 220 tall.
pub fn table(ui: &mut Ui, p: &Palette, id: Id, head: &[Th], rows: &[Vec<Td>], edits: &mut [String]) {
    ui.add_space(10.0);
    let font = ui_font(12.0);
    let bold = semibold(12.0);
    let ncols = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0)).max(1);
    let mut widths = vec![0.0f32; ncols];
    let painter = ui.painter().clone();
    let measure = |t: &str, f: &FontId| painter.layout_no_wrap(t.replace('\n', "↵"), f.clone(), Color32::WHITE).size().x;
    for (c, h) in head.iter().enumerate() {
        widths[c] = widths[c].max(match h { Th::Text(t) | Th::New(t) => measure(t, &bold) + 16.0, Th::Edit(_) => 138.0 });
    }
    for r in rows {
        for (c, d) in r.iter().enumerate() {
            let t = match d { Td::Text(t) | Td::Src(t) | Td::Empty(t) | Td::Marked(t, _) => t };
            widths[c] = widths[c].max(measure(t, &font) + 16.0);
        }
    }
    for w in &mut widths {
        *w = w.min(220.0).ceil() + 1.0;
    }
    let row_h = 24.0;
    let head_h = if head.iter().any(|h| matches!(h, Th::Edit(_))) { 30.0 } else if head.is_empty() { 0.0 } else { 24.0 };
    let h = head_h + rows.len() as f32 * row_h;
    let frame = egui::Frame::new().stroke(Stroke::new(1.0, p.brd)).corner_radius(6);
    frame.show(ui, |ui| {
        let avail = ui.available_width();
        let total: f32 = widths.iter().sum();
        if total < avail {
            // As a table at 100 %: the room left shared in proportion.
            let k = avail / total;
            for w in &mut widths {
                *w = (*w * k).floor();
            }
            let rest = avail - widths.iter().sum::<f32>();
            widths[ncols - 1] += rest;
        }
        let scroll = h > 218.0 || widths.iter().sum::<f32>() > avail + 0.5;
        let mut body = |ui: &mut Ui| {
            let (r, _) = ui.allocate_exact_size(vec2(widths.iter().sum(), h), Sense::hover());
            // The frame closes the table: no rule of its own on the last column or row.
            let rule = |ui: &Ui, a: Rect| {
                let pt = ui.painter();
                if a.max.y < r.max.y - 0.5 {
                    pt.hline(a.x_range(), a.max.y - 0.5, Stroke::new(1.0, p.brd));
                }
                if a.max.x < r.max.x - 0.5 {
                    pt.vline(a.max.x - 0.5, a.y_range(), Stroke::new(1.0, p.brd));
                }
            };
            let mut x = r.min.x;
            if head_h > 0.0 {
                ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width(), head_h)), 0.0, p.surf);
                for (c, hd) in head.iter().enumerate() {
                    let cell = Rect::from_min_size(pos2(x, r.min.y), vec2(widths[c], head_h));
                    match hd {
                        Th::Text(t) | Th::New(t) => {
                            let col = if matches!(hd, Th::New(_)) { p.prim } else { p.txt_hi };
                            let g = line(ui, t, bold.clone(), col, cell.width() - 17.0);
                            put(ui, g, cell.min.x + 8.0, cell.center().y - 0.5, col);
                        }
                        Th::Edit(k) => {
                            if let Some(s) = edits.get_mut(*k) {
                                let fr = Rect::from_min_size(pos2(cell.min.x + 4.0, cell.min.y + 4.0), vec2(130.0f32.min(cell.width() - 9.0), head_h - 9.0));
                                field_at(ui, p, id.with(("th", *k)), fr, s, "", ui_font(12.0), 6.0, false);
                            }
                        }
                    }
                    rule(ui, cell);
                    x += widths[c];
                }
            }
            for (i, row) in rows.iter().enumerate() {
                let y = r.min.y + head_h + i as f32 * row_h;
                let mut x = r.min.x;
                for c in 0..ncols {
                    let cell = Rect::from_min_size(pos2(x, y), vec2(widths[c], row_h));
                    let cy = cell.center().y - 0.5;
                    let maxw = cell.width() - 17.0;
                    match row.get(c) {
                        Some(Td::Text(t)) => put(ui, line(ui, t, font.clone(), p.txt, maxw), cell.min.x + 8.0, cy, p.txt),
                        Some(Td::Src(t)) => put(ui, line(ui, t, font.clone(), p.txt_mut, maxw), cell.min.x + 8.0, cy, p.txt_mut),
                        Some(Td::Empty(t)) => {
                            let it = FontId::proportional(12.0);
                            let mut job = LayoutJob::simple_singleline(t.clone(), it, p.txt_faint);
                            job.sections[0].format.italics = true;
                            job.wrap = egui::text::TextWrapping { max_width: maxw.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                            let g = ui.painter().layout_job(job);
                            put(ui, g, cell.min.x + 8.0, cy, p.txt_faint);
                        }
                        Some(Td::Marked(t, marks)) => {
                            let mut job = LayoutJob::default();
                            let plain = TextFormat { font_id: font.clone(), color: p.txt_mut, ..Default::default() };
                            let mark = TextFormat { font_id: font.clone(), color: p.mark_txt, background: p.match_bg, strikethrough: Stroke::new(1.0, p.mark_txt), ..Default::default() };
                            let mut at = 0;
                            for &(a, b) in marks {
                                if a < at || b > t.len() { continue; }
                                job.append(&t[at..a], 0.0, plain.clone());
                                job.append(&t[a..b], 0.0, mark.clone());
                                at = b;
                            }
                            job.append(&t[at..], 0.0, plain);
                            job.wrap = egui::text::TextWrapping { max_width: maxw.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                            let g = ui.painter().layout_job(job);
                            put(ui, g, cell.min.x + 8.0, cy, p.txt_mut);
                        }
                        None => {}
                    }
                    if let Some(Td::Text(t) | Td::Src(t) | Td::Marked(t, _)) = row.get(c) {
                        if measure(t, &font) > maxw {
                            ui.interact(cell, id.with(("td", i, c)), Sense::hover()).on_hover_text(t.as_str());
                        }
                    }
                    rule(ui, cell);
                    x += widths[c];
                }
            }
        };
        if scroll {
            egui::ScrollArea::both().id_salt(id).max_height(218.0).auto_shrink([false, true]).show(ui, body);
        } else {
            body(ui);
        }
    });
}

/* ---- Background computations --------------------------------------- */

/// A result computed off the UI thread from inputs `K`: redone (the previous
/// run cancelled) whenever they change; the last result stays shown meanwhile.
pub struct Live<K, T> {
    key: Option<K>,
    job: Option<Job<Option<T>>>,
    pub val: Option<T>,
}

impl<K, T> Default for Live<K, T> {
    fn default() -> Self {
        Live { key: None, job: None, val: None }
    }
}

impl<K: PartialEq, T: Send + 'static> Live<K, T> {
    /// Whether `key` differs from the inputs of the last run.
    pub fn stale(&self, key: &K) -> bool {
        self.key.as_ref() != Some(key)
    }

    pub fn run(&mut self, ctx: &egui::Context, key: K, f: impl FnOnce(&AtomicBool) -> Option<T> + Send + 'static) {
        self.key = Some(key);
        self.job = Some(Job::spawn(ctx, move |c, _| f(c)));
    }

    /// Picks up a finished run; true while one is running.
    pub fn poll(&mut self) -> bool {
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            if r.is_some() {
                self.val = r;
            }
        }
        self.job.is_some()
    }

    /// The result, once it is the one of `key`.
    pub fn fresh(&self, key: &K) -> Option<&T> {
        (self.job.is_none() && !self.stale(key)).then_some(self.val.as_ref()).flatten()
    }

    pub fn forget(&mut self) {
        self.key = None;
        self.job = None;
        self.val = None;
    }
}

/* ---- The tab ------------------------------------------------------- */

/// What a job needs of the tab: its snapshot and the rows shown (None: every
/// data row, listed in the job).
#[derive(Clone)]
pub struct Snap {
    pub st: State,
    pub view: Option<Arc<Vec<RowId>>>,
}

impl Snap {
    pub fn of(tab: &Tab) -> Option<Snap> {
        Some(Snap { st: tab.st()?.clone(), view: tab.view.clone() })
    }

    /// The rows shown, in order.
    pub fn rows(&self) -> Arc<Vec<RowId>> {
        self.view.clone().unwrap_or_else(|| Arc::new(csvfab_engine::edit::all_rows(&self.st)))
    }

    /// Every data row, in file order.
    pub fn all(&self) -> Vec<RowId> {
        csvfab_engine::edit::all_rows(&self.st)
    }
}

/// Whether a filter hides rows (a sort alone does not).
pub fn filtered(tab: &Tab) -> bool {
    tab.view.is_some() && tab.st().is_some_and(|st| tab.shown() < st.len())
}

/// The first `n` rows shown (a guess looks at a sample).
pub fn first_rows(tab: &Tab, n: usize) -> Vec<RowId> {
    (0..tab.shown().min(n)).map(|p| tab.shown_id(p)).collect()
}

/// The column of the active cell, or the first.
pub fn cursor_col(tab: &Tab) -> usize {
    let n = tab.st().map_or(0, |s| s.ncols);
    tab.grid.cursor.map_or(0, |c| c.col).min(n.saturating_sub(1))
}

/// The tab's titles.
pub fn headers(tab: &Tab) -> Vec<String> {
    tab.st().map(|s| s.headers()).unwrap_or_default()
}

/// Column `old`'s index once the columns `gone` are deleted (None: it is one of them).
pub fn after_delete(gone: &[usize], old: usize) -> Option<usize> {
    (!gone.contains(&old)).then(|| old - gone.iter().filter(|&&g| g < old).count())
}

/// Column `old`'s index once `n` columns are inserted at `at`.
pub fn after_insert(at: usize, n: usize, old: usize) -> usize {
    if old >= at { old + n } else { old }
}

/// After columns were inserted, deleted or moved (the table edited already): the tab's
/// per-column settings (`tab::ColView`) follow them. `map`: an old column's new index
/// (None: gone); a new column gets none, 120 wide. The settings before and after are kept
/// by table version, for Undo / Redo. Then the view is recomputed.
pub fn remap_columns(tab: &mut Tab, old_n: usize, map: impl Fn(usize) -> Option<usize>, ctx: &egui::Context) {
    let new_n = tab.st().map_or(0, |s| s.ncols);
    let old = tab.col_view();
    // The version before this edit: the one Undo comes back to.
    if let Some(t) = &tab.table {
        let v = t.undo_versions();
        let before = if v.len() >= 2 { v[v.len() - 2] } else { 0 };
        tab.keep_col_view(before, old.clone());
    }
    let to = |c: usize| map(c).filter(|&k| k < new_n);
    let mut filters = vec![String::new(); new_n];
    let mut widths = vec![120.0; new_n];
    for c in 0..old_n {
        if let Some(k) = to(c) {
            if let Some(f) = old.filters.get(c) { filters[k] = f.clone(); }
            if let Some(w) = old.widths.get(c) { widths[k] = *w; }
        }
    }
    let l = &old.look;
    // The frozen block keeps what it held: up to where its last surviving column went.
    let frozen = (0..l.frozen).filter_map(to).max().map_or(0, |k| k + 1);
    let new = crate::tab::ColView {
        filters,
        widths,
        sort: old.sort.iter().filter_map(|k| to(k.col).map(|col| csvfab_engine::view::SortKey { col, desc: k.desc })).collect(),
        hidden_vals: old.hidden_vals.iter().filter_map(|(c, v)| to(*c).map(|k| (k, v.clone()))).collect(),
        find_col: old.find_col.and_then(to),
        look: crate::grid::ColLook {
            hidden: l.hidden.iter().filter_map(|&c| to(c)).collect(),
            frozen,
            heat: l.heat.iter().filter_map(|(&c, h)| to(c).map(|k| (k, *h))).collect(),
            bars: l.bars.iter().filter_map(|(&c, b)| to(c).map(|k| (k, *b))).collect(),
        },
        totals: old.totals.iter().filter_map(|(&c, f)| to(c).map(|k| (k, *f))).collect(),
    };
    tab.set_col_view(new.clone());
    if let Some(v) = tab.table.as_ref().map(|t| t.version()) { tab.keep_col_view(v, new); }
    if let Some(cur) = &mut tab.grid.cursor {
        cur.col = map(cur.col).unwrap_or(0).min(new_n.saturating_sub(1));
    }
    tab.grid.anchor = tab.grid.cursor;
    tab.after_columns(old_n, &to, ctx);
}

/// A dialog by its short name, for the screenshot hooks (`CSVFAB_SHOT=tool:bulk`).
pub fn by_name(name: &str, tab: &Tab) -> Option<Box<dyn super::Tool>> {
    Some(match name {
        "bulk" => Box::new(super::bulk::BulkEdit::new(tab)),
        "clean" => Box::new(super::clean::CleanTool::new(tab)),
        "noise" => Box::new(super::noise::NoiseTool::new(tab).words("SARL, SAS")),
        "hidden" => Box::new(super::rows::DeleteHidden::default()),
        "split" => Box::new(super::columns::SplitTool::new(tab, Some(1)).with_sep("T")),
        "merge" => Box::new(super::columns::MergeTool::new(Some(4))),
        "lookup" => Box::new(super::lookup::LookupTool::new(tab, None)),
        "convert" => Box::new(super::convert::ConvertTool::new(tab, None)),
        _ => return None,
    })
}
