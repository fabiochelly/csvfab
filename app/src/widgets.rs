//! The shell's widget kit, one look everywhere (docs/look.md, "Menus", "Buttons
//! and fields", "Dialogs", "Motion"): menu items, buttons, fields, selects,
//! checkboxes and radios, the dialog with its frosted backdrop, and the motion
//! that brings menus and dialogs in. The tools' three kits (`tools/form.rs`,
//! `edit_kit.rs`, `analysis_kit.rs`) draw through these.
//!
//! Motion runs only while it moves: every transition here is an egui
//! animation (which asks for frames until it settles) or a pop-in timed from
//! the moment its area appeared. An idle window draws nothing.

use crate::grid::semibold;
use crate::icons::{self, Icon};
use crate::theme::{alpha, Palette};
use eframe::egui::{
    self, emath::TSTransform, pos2, vec2, Align2, Color32, Context, CornerRadius, FontFamily, FontId, Id, LayerId, Margin, Order, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui, UiBuilder,
};

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::fonts::UI_MEDIUM.into()))
}

/// On a whole physical pixel (text tops, fills); add `0.5 / ppp` for a 1 px rule.
pub fn snap(ctx: &Context, x: f32) -> f32 {
    let ppp = ctx.pixels_per_point();
    (x * ppp).round() / ppp
}

/// `t` of `a`, the rest of `b` (as `theme::mix`), keeping translucency: for the hover and
/// focus transitions between colours that may be see-through.
pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    b.lerp_to_gamma(a, t.clamp(0.0, 1.0))
}

/// Light text on dark surfaces: the dark themes (tunes shadows and highlights).
pub fn is_dark(p: &Palette) -> bool {
    let c = p.surf;
    (c.r() as u32 * 3 + c.g() as u32 * 6 + c.b() as u32) < 1280
}

// ---------------------------------------------------------------- motion

/// Menus and popups pop in over this long; dialogs take a little longer (look.md, "Motion").
pub const POP_MENU: f32 = 0.14;
pub const POP_DIALOG: f32 = 0.16;
/// Hover and focus colour transitions.
pub const EASE_S: f32 = 0.09;

/// The user asked for no motion (`animation_time` 0).
pub fn reduced_motion(ctx: &Context) -> bool {
    ctx.global_style().animation_time <= 0.0
}

/// 0 → 1 as `on` turns true (and back), over `EASE_S`: hover and focus transitions.
pub fn ease(ctx: &Context, id: Id, on: bool) -> f32 {
    ctx.animate_bool_with_time(id.with("ease"), on, if reduced_motion(ctx) { 0.0 } else { EASE_S })
}

/// 0 → 1 over `dur` (cubic ease-out) since area `id` became visible; 0 before it was ever shown.
pub fn appear(ctx: &Context, id: Id, dur: f32) -> f32 {
    if reduced_motion(ctx) {
        return 1.0;
    }
    let now = ctx.input(|i| i.time);
    match egui::AreaState::load(ctx, id).and_then(|s| s.last_became_visible_at) {
        Some(t0) => egui::emath::easing::cubic_out((((now - t0) as f32) / dur).clamp(0.0, 1.0)),
        None => 0.0,
    }
}

/// Scale `s` about `origin`, then move by `dy`.
fn scale_about(origin: Pos2, s: f32, dy: f32) -> TSTransform {
    TSTransform::new(origin.to_vec2() * (1.0 - s) + vec2(0.0, dy), s)
}

/// Every menu, popup and window that has just appeared grows from .96 to 1 about the point it
/// hangs from (egui fades areas in by itself): `.dd-menu`'s pop. Called once per frame after
/// everything is drawn. The top modal dialog is left out: its backdrop must not move
/// (`modal` pops its own box). Asks for frames only while something moves.
pub fn motion(ctx: &Context) {
    let key = Id::unique("csvfab-motion");
    let mut moving: Vec<LayerId> = Vec::new();
    if !reduced_motion(ctx) {
        let now = ctx.input(|i| i.time);
        let top_modal = ctx.memory(|m| m.top_modal_layer());
        let layers: Vec<LayerId> = ctx.memory(|m| m.areas().visible_layer_ids().into_iter().collect());
        for l in layers {
            if !matches!(l.order, Order::Middle | Order::Foreground) || Some(l) == top_modal {
                continue;
            }
            let Some(st) = egui::AreaState::load(ctx, l.id) else { continue };
            let (Some(t0), Some(_)) = (st.last_became_visible_at, st.size) else { continue };
            let t = ((now - t0) as f32 / POP_MENU).clamp(0.0, 1.0);
            if t >= 1.0 {
                continue;
            }
            // Windows grow from their middle, menus from the corner they hang from.
            let origin = if l.order == Order::Middle { st.rect().center() } else { st.pivot_pos.unwrap_or(st.rect().center()) };
            let e = egui::emath::easing::cubic_out(t);
            ctx.set_transform_layer(l, scale_about(origin, 0.96 + 0.04 * e, 0.0));
            moving.push(l);
        }
    }
    let before = ctx.data_mut(|d| std::mem::replace(d.get_temp_mut_or_default::<Vec<LayerId>>(key), moving.clone()));
    for l in before.into_iter().filter(|l| !moving.contains(l)) {
        ctx.set_transform_layer(l, TSTransform::IDENTITY);
    }
    if !moving.is_empty() {
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------- menus

pub struct Item<'a> {
    icon: Option<Icon>,
    label: &'a str,
    keys: &'a str,
    enabled: bool,
    danger: bool,
    checked: bool,
    submenu: bool,
}

impl<'a> Item<'a> {
    pub fn new(label: &'a str) -> Self {
        Item { icon: None, label, keys: "", enabled: true, danger: false, checked: false, submenu: false }
    }
    /// A 16 px icon 10 px before the label, `txt_mut` (`txt_hi` on hover).
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn keys(mut self, k: &'a str) -> Self {
        self.keys = k;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    pub fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }

    /// A row 30 tall, padding 7 × 10, radius 5: label left, shortcut (or ✓ / ›) right.
    /// The hover fill eases in (90 ms).
    pub fn show(self, ui: &mut Ui, p: &Palette) -> Response {
        let w = ui.available_width().max(ui.min_size().x);
        let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), if self.enabled { Sense::click() } else { Sense::hover() });
        let hot = self.enabled && (resp.hovered() || resp.has_focus());
        let t = ease(ui.ctx(), resp.id, hot);
        let pt = ui.painter();
        if t > 0.0 {
            let fill = if self.danger { alpha(p.danger, 0.14) } else { p.surf_2 };
            pt.rect_filled(r, 5.0, fill.gamma_multiply(t));
        }
        let fade = |c: Color32| if self.enabled { c } else { c.gamma_multiply(0.4) };
        let fg = fade(if self.danger { p.danger_txt } else { p.txt_hi });
        let y = r.center().y;
        let mut x = r.min.x + 10.0;
        if let Some(i) = self.icon {
            let ic = if self.danger { fg } else { fade(blend(p.txt_hi, p.txt_mut, t)) };
            icons::paint(pt, i, Rect::from_min_size(pos2(x, y - 8.0), vec2(16.0, 16.0)), ic);
            x += 16.0 + 10.0;
        }
        let mut job = egui::text::LayoutJob::simple_singleline(self.label.to_string(), FontId::proportional(13.0), fg);
        job.wrap = egui::text::TextWrapping { max_width: r.max.x - x - 10.0 - 24.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = pt.layout_job(job);
        let gy = snap(ui.ctx(), y - g.size().y / 2.0);
        pt.galley(pos2(x, gy), g, fg);
        let right = r.max.x - 10.0;
        if self.submenu {
            pt.text(pos2(right, y), Align2::RIGHT_CENTER, "›", FontId::proportional(12.0), blend(p.txt_hi, p.txt_dim, t));
        } else if self.checked {
            icons::paint(pt, Icon::MenuCheck, Rect::from_center_size(pos2(right - 7.0, y), vec2(14.0, 14.0)), p.accent_2);
        } else if !self.keys.is_empty() {
            pt.text(pos2(right, y), Align2::RIGHT_CENTER, self.keys, FontId::proportional(11.5), fade(p.txt_dim));
        }
        if self.enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
    }
}

/// A section head: padding 6 10 4, ui 600 11, upper case, letter-spacing .04em, `txt_dim`.
pub fn head(ui: &mut Ui, p: &Palette, text: &str) {
    let w = ui.available_width().max(ui.min_size().x);
    let (r, _) = ui.allocate_exact_size(vec2(w, 6.0 + 15.0 + 4.0), Sense::hover());
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_uppercase(), semibold(11.0), p.txt_dim);
    job.wrap = egui::text::TextWrapping { max_width: r.width() - 20.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job.sections.iter_mut().for_each(|s| s.format.extra_letter_spacing = 0.44);
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(r.min.x + 10.0, snap(ui.ctx(), r.min.y + 6.0)), g, p.txt_dim);
}

/// A separator: 1 px `brd` on a physical pixel, 5 above and below, 4 in from the sides.
pub fn sep(ui: &mut Ui, p: &Palette) {
    let w = ui.available_width().max(ui.min_size().x);
    let (r, _) = ui.allocate_exact_size(vec2(w, 11.0), Sense::hover());
    ui.painter().hline(r.min.x + 4.0..=r.max.x - 4.0, ui.painter().round_to_pixel_center(r.center().y), Stroke::new(1.0, p.brd));
}

/// The menus' content width: fixed, as the web's menus (egui's would grow to their widest item).
pub fn width(ui: &mut Ui, w: f32) {
    ui.set_min_width(w);
    ui.set_max_width(w);
    ui.spacing_mut().item_spacing.y = 0.0;
}

/// A label with a joined group of buttons at its right (`Row: [Go to… | Card]`):
/// returns the index of the button clicked. Icons as the menu items' (14 px in the buttons).
pub fn row_buttons(ui: &mut Ui, p: &Palette, icon: Option<Icon>, label: &str, buttons: &[(Option<Icon>, &str, bool)]) -> Option<usize> {
    let w = ui.available_width().max(ui.min_size().x);
    let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let y = r.center().y;
    let mut lx = r.min.x + 10.0;
    if let Some(i) = icon {
        icons::paint(ui.painter(), i, Rect::from_min_size(pos2(lx, y - 8.0), vec2(16.0, 16.0)), p.txt_mut);
        lx += 26.0;
    }
    ui.painter().text(pos2(lx, y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), p.txt_hi);
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = buttons.iter().map(|(i, t, _)| (ui.painter().layout_no_wrap(t.to_string(), font.clone(), p.txt_hi).size().x + 20.0 + if i.is_some() { 20.0 } else { 0.0 }).round()).collect();
    let total: f32 = widths.iter().sum();
    let group = Rect::from_min_size(pos2(snap(ui.ctx(), r.max.x - 4.0 - total), y - 12.0), vec2(total, 24.0));
    let mut x = group.min.x;
    let mut hit = None;
    for (k, ((i, t, enabled), bw)) in buttons.iter().zip(&widths).enumerate() {
        let br = Rect::from_min_size(pos2(x, group.min.y), vec2(*bw, 24.0));
        let resp = ui.interact(br, ui.scope_id().with(("rowbtn", label, k)), if *enabled { Sense::click() } else { Sense::hover() });
        let hot = *enabled && resp.hovered();
        let e = ease(ui.ctx(), resp.id, hot);
        if e > 0.0 {
            let cr = CornerRadius { nw: if k == 0 { 6 } else { 0 }, sw: if k == 0 { 6 } else { 0 }, ne: if k + 1 == buttons.len() { 6 } else { 0 }, se: if k + 1 == buttons.len() { 6 } else { 0 } };
            ui.painter().rect_filled(br, cr, p.surf_2.gamma_multiply(e));
        }
        let mut tx = br.min.x + 10.0;
        if let Some(i) = i {
            icons::paint(ui.painter(), *i, Rect::from_min_size(pos2(tx, y - 7.0), vec2(14.0, 14.0)), if !*enabled { p.txt_faint } else { blend(p.txt_hi, p.txt_mut, e) });
            tx += 14.0 + 6.0;
        }
        ui.painter().text(pos2(tx, y), Align2::LEFT_CENTER, t, font.clone(), if *enabled { p.txt_hi } else { p.txt_faint });
        if k > 0 {
            ui.painter().vline(ui.painter().round_to_pixel_center(x), br.y_range(), Stroke::new(1.0, p.brd));
        }
        if resp.clicked() {
            hit = Some(k);
        }
        if *enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        x += bw;
    }
    ui.painter().rect_stroke(group, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
    hit
}

/// The menus' frame (`theme::menu_frame` at the default metrics).
pub fn menu_frame(p: &Palette) -> egui::Frame {
    crate::theme::menu_frame(p, &crate::theme::Metrics::default())
}

// ---------------------------------------------------------------- fields

/// A single-line field whose text sits in the middle of `rect` (egui gives text
/// fields a minimum height and draws their text at its top).
pub fn centered_edit(ui: &mut Ui, rect: Rect, edit: egui::TextEdit) -> Response {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child.spacing_mut().interact_size.y = 0.0;
    child.add(edit.margin(egui::Margin::ZERO).desired_width(rect.width()))
}

/// The `.bs-input` frame over `r`: `bg_deep`, 1 px `brd` (`brd_2` under the pointer),
/// `radius`; focused: border `prim` and a 4 px `focus_ring` ring, both easing in;
/// `bad`: border `danger` and a 3 px `alpha(danger, .2)` ring. `id`: the field's.
pub fn field_frame(ui: &Ui, p: &Palette, id: Id, r: Rect, focused: bool, bad: bool, radius: f32) {
    let ctx = ui.ctx();
    let f = ease(ctx, id.with("focus"), focused);
    let h = ease(ctx, id.with("hover"), ui.rect_contains_pointer(r));
    let pt = ui.painter();
    if bad {
        pt.rect_stroke(r, radius, Stroke::new(3.0, alpha(p.danger, 0.2)), StrokeKind::Outside);
    } else if f > 0.0 {
        pt.rect_stroke(r, radius, Stroke::new(4.0, p.focus_ring.gamma_multiply(f)), StrokeKind::Outside);
    }
    let brd = if bad { p.danger } else { blend(p.prim, blend(p.brd_2, p.brd, h), f) };
    pt.rect(r, radius, p.bg_deep, Stroke::new(1.0, brd), StrokeKind::Inside);
}

/// A text field over `r`: text in `font`, `pad` from the sides, placeholder `txt_sub`.
#[allow(clippy::too_many_arguments)]
pub fn text_field(ui: &mut Ui, p: &Palette, id: Id, r: Rect, text: &mut String, hint: &str, font: FontId, pad: f32, bad: bool) -> Response {
    let focused = ui.memory(|m| m.has_focus(id));
    field_frame(ui, p, id, r, focused, bad, 6.0);
    let te = egui::TextEdit::singleline(text)
        .id(id)
        .font(font.clone())
        .text_color(p.txt_hi)
        .hint_text(egui::RichText::new(hint).font(font).color(p.txt_sub))
        .frame(egui::Frame::NONE);
    centered_edit(ui, r.shrink2(vec2(pad, 0.0)), te)
}

/// A select (`.bs-select`) over `r` (`resp` its click): the field look, `options[*sel]`
/// in ui `size`, the chevron 16 × 12 12 px from the right; a click lists the options as
/// menu items (scrolling past 320, the current one in view). True when the choice changed.
#[allow(clippy::too_many_arguments)]
pub fn select(ui: &mut Ui, p: &Palette, id: Id, r: Rect, resp: &Response, sel: &mut usize, options: &[String], size: f32) -> bool {
    let pid = id.with("select-menu");
    let open = egui::Popup::is_id_open(ui.ctx(), pid);
    field_frame(ui, p, id, r, open, false, 6.0);
    let pad = if r.height() < 26.0 { 8.0 } else { 12.0 };
    let text = options.get(*sel).map_or("", |s| s.as_str());
    let mut job = egui::text::LayoutJob::simple_singleline(text.replace(['\n', '\r'], "↵"), FontId::proportional(size), p.txt_hi);
    job.wrap = egui::text::TextWrapping { max_width: (r.width() - pad - 36.0).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(r.min.x + pad, snap(ui.ctx(), r.center().y - g.size().y / 2.0)), g, p.txt_hi);
    let chev = Rect::from_center_size(pos2(r.max.x - 12.0 - 8.0, r.center().y), vec2(16.0, 12.0));
    let hot = ease(ui.ctx(), id.with("chev"), open || resp.hovered());
    icons::paint(ui.painter(), Icon::SelectChevron, chev, blend(p.txt_hi, Color32::from_rgb(0x8b, 0x94, 0x9e), hot));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let mut changed = false;
    // The menu's first frames: bring the current option into view.
    let fresh = appear(ui.ctx(), pid, 0.05) < 1.0;
    egui::Popup::menu(resp).id(pid).frame(menu_frame(p)).width(r.width().max(160.0)).show(|ui| {
        width(ui, r.width().max(160.0) - 12.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (k, o) in options.iter().enumerate() {
                let it = Item::new(o).checked(k == *sel).show(ui, p);
                if k == *sel && fresh {
                    ui.scroll_to_rect(it.rect, Some(egui::Align::Center));
                }
                if it.clicked() {
                    changed = k != *sel;
                    *sel = k;
                    ui.close();
                }
            }
        });
    });
    changed
}

// ---------------------------------------------------------------- checkboxes and radios

/// A checkbox (radius 4) or radio (`round`) box, 16 × 16: `bg_deep`, 1 px `brd_2` (hover
/// `txt_dim`); on: `prim` with a white check (a dot), easing in with a little growth.
pub fn tick_box(pt: &egui::Painter, p: &Palette, b: Rect, on: bool, round: bool, hot: bool, enabled: bool) {
    let ctx = pt.ctx();
    // Keyed by place: a box that scrolls simply shows its state, one that stays animates.
    let key = Id::unique(("tick", (b.min.x * 4.0) as i32, (b.min.y * 4.0) as i32, pt.layer_id()));
    let t = ease(ctx, key, on);
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let cr = if round { 8.0 } else { 4.0 };
    let off_brd = if hot { p.txt_dim } else { p.brd_2 };
    pt.rect(b, cr, fade(blend(p.prim, p.bg_deep, t)), Stroke::new(1.0, fade(blend(p.prim, off_brd, t))), StrokeKind::Inside);
    if t <= 0.0 {
        return;
    }
    let white = fade(Color32::WHITE).gamma_multiply(t);
    if round {
        pt.circle_filled(b.center(), 4.0 * (0.5 + 0.5 * t), white);
    } else {
        // The web's check: `m6 10 3 3 6-6` in a 20-unit box, stroke 3; grows from .6.
        let s = b.width() / 20.0 * (0.6 + 0.4 * t);
        let c = b.center();
        let at = |x: f32, y: f32| pos2(c.x + (x - 10.0) * s, c.y + (y - 10.0) * s);
        pt.add(Shape::line(vec![at(6.0, 10.0), at(9.0, 13.0), at(15.0, 7.0)], Stroke::new(3.0 * b.width() / 20.0, white)));
    }
}

/// A checkbox or radio with its label `gap` px after (ui 13 `color`), `h` tall; with a
/// `key`, the row takes the full width and shows it at the right in mono 12 `txt_sub`.
/// Disabled at .45.
#[allow(clippy::too_many_arguments)]
pub fn tick_row(ui: &mut Ui, p: &Palette, on: bool, round: bool, text: &str, color: Color32, gap: f32, h: f32, key: &str, enabled: bool) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), color);
    let kg = (!key.is_empty()).then(|| ui.painter().layout_no_wrap(key.to_string(), FontId::monospace(12.0), p.txt_sub));
    let w = if kg.is_some() { ui.available_width() } else { 16.0 + if text.is_empty() { 0.0 } else { gap + g.size().x } };
    let (r, resp) = ui.allocate_exact_size(vec2(w, h.max(g.size().y)), if enabled { Sense::click() } else { Sense::hover() });
    let ctx = ui.ctx().clone();
    let b = Rect::from_min_size(pos2(r.min.x, snap(&ctx, r.center().y - 8.0)), vec2(16.0, 16.0));
    tick_box(ui.painter(), p, b, on, round, enabled && resp.hovered(), enabled);
    if !text.is_empty() {
        let c = if enabled { color } else { color.gamma_multiply(0.45) };
        ui.painter().galley_with_override_text_color(pos2(b.max.x + gap, snap(&ctx, r.center().y - g.size().y / 2.0)), g, c);
    }
    if let Some(kg) = kg {
        ui.painter().galley(pos2(r.max.x - kg.size().x, snap(&ctx, r.center().y - kg.size().y / 2.0)), kg, p.txt_sub);
    }
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp }
}

// ---------------------------------------------------------------- buttons

/// The kinds of `.btn`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `.btn`: `prim`, `on_prim` text.
    Primary,
    /// `.btn-outline`: 1 px `brd`, `txt`.
    Outline,
    Warn,
    Danger,
}

/// Paints a button's box over `r` for `resp`: the fill (and border) easing to the hover
/// colours, pressed a shade deeper, a hairline of light along the top of the solid kinds,
/// a 2 px `alpha(prim, .45)` ring under keyboard focus. Returns the text colour.
pub fn button_box(ui: &Ui, p: &Palette, kind: Kind, r: Rect, resp: &Response, enabled: bool, cr: CornerRadius) -> Color32 {
    let ctx = ui.ctx();
    let hovered = enabled && resp.hovered();
    let t = ease(ctx, resp.id, hovered);
    let down = enabled && resp.is_pointer_button_down_on();
    let solid = |base: Color32, hover: Color32| {
        let c = blend(hover, base, t);
        if down { blend(Color32::BLACK, c, 0.08) } else { c }
    };
    let (fill, stroke, fg) = match kind {
        Kind::Primary => (solid(p.prim, p.prim_hover), Color32::TRANSPARENT, p.on_prim),
        Kind::Warn => (solid(p.warn, p.warn_hover), Color32::TRANSPARENT, p.on_warn),
        Kind::Danger => (solid(p.danger, p.danger_hover), Color32::TRANSPARENT, p.on_danger),
        Kind::Outline => (if down { p.surf_3 } else { p.surf_2.gamma_multiply(t) }, blend(p.brd_2, p.brd, t), p.txt),
    };
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let pt = ui.painter();
    if resp.has_focus() {
        pt.rect_stroke(r.expand(2.0), cr + 2, Stroke::new(2.0, alpha(p.prim, 0.45)), StrokeKind::Outside);
    }
    pt.rect(r, cr, fade(fill), Stroke::new(1.0, fade(stroke)), StrokeKind::Inside);
    if kind != Kind::Outline && enabled {
        let inset = cr.nw.max(cr.ne) as f32;
        let y = pt.round_to_pixel_center(r.min.y + 0.5);
        pt.hline(r.min.x + inset..=r.max.x - inset, y, Stroke::new(1.0, alpha(Color32::WHITE, 0.16)));
    }
    if hovered {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    fade(fg)
}

/// A `.btn`: ui 500 `size`, `pad` each side, `h` tall, radius 6, an optional 14 px icon
/// 6 px before the text; disabled at .45.
#[allow(clippy::too_many_arguments)]
pub fn button(ui: &mut Ui, p: &Palette, kind: Kind, icon: Option<Icon>, text: &str, enabled: bool, h: f32, pad: f32, size: f32) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), medium(size), Color32::WHITE);
    let iw = match (icon.is_some(), text.is_empty()) {
        (true, true) => 14.0,
        (true, false) => 14.0 + 6.0,
        _ => 0.0,
    };
    let (r, resp) = ui.allocate_exact_size(vec2((g.size().x + iw + 2.0 * pad).round(), h), if enabled { Sense::click() } else { Sense::hover() });
    let fg = button_box(ui, p, kind, r, &resp, enabled, CornerRadius::same(6));
    let ctx = ui.ctx().clone();
    let x0 = snap(&ctx, r.center().x - (g.size().x + iw) / 2.0);
    if let Some(ic) = icon {
        icons::paint(ui.painter(), ic, Rect::from_center_size(pos2(x0 + 7.0, r.center().y), vec2(14.0, 14.0)), fg);
    }
    ui.painter().galley_with_override_text_color(pos2(x0 + iw, snap(&ctx, r.center().y - g.size().y / 2.0)), g, fg);
    resp
}

// ---------------------------------------------------------------- dialogs

/// The dialog box (`#dlg`, `.modal-box`): `surf`, 1 px `brd`, radius 10, `margin`; a deep
/// soft shadow (lighter on the light themes, where black would read as dirt).
pub fn dialog_frame(p: &Palette, margin: Margin) -> egui::Frame {
    let dark = is_dark(p);
    egui::Frame::new()
        .fill(p.surf)
        .stroke(Stroke::new(1.0, p.brd))
        .corner_radius(10)
        .inner_margin(margin)
        .shadow(egui::Shadow { offset: [0, 18], blur: 50, spread: 0, color: Color32::from_black_alpha(if dark { 140 } else { 46 }) })
}

/// A dialog's title (`h4`): ui 600 16 `txt_hi` on a 20 px line, 14 below.
pub fn dialog_title(ui: &mut Ui, p: &Palette, text: &str) {
    let w = ui.available_width();
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), semibold(16.0), p.txt_hi);
    job.wrap = egui::text::TextWrapping { max_width: w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    ui.painter().galley(pos2(r.min.x, snap(ui.ctx(), r.center().y - g.size().y / 2.0)), g, p.txt_hi);
    ui.add_space(14.0);
}

/// The frosted grain: 128 × 128 of white noise, tiled one texel per physical pixel.
fn grain(ctx: &Context) -> egui::TextureHandle {
    let id = Id::unique("csvfab-grain");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    const N: usize = 128;
    let mut px = Vec::with_capacity(N * N);
    let mut s: u32 = 0x9e37_79b9;
    for _ in 0..N * N {
        // xorshift: cheap, even, no visible pattern at this size.
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        px.push(Color32::from_white_alpha((s >> 24) as u8));
    }
    let img = egui::ColorImage::new([N, N], px);
    let tex = ctx.load_texture("csvfab-grain", img, egui::TextureOptions::NEAREST_REPEAT);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// A soft elliptic glow: `col` at the centre fading out at radii `rad`.
pub fn glow(pt: &egui::Painter, c: Pos2, rad: egui::Vec2, col: Color32) {
    let mut m = egui::Mesh::default();
    let rings = [(0.35, 0.72), (0.65, 0.3), (1.0, 0.0)];
    const SEG: u32 = 48;
    m.colored_vertex(c, col);
    for &(f, a) in &rings {
        for k in 0..SEG {
            let th = k as f32 / SEG as f32 * std::f32::consts::TAU;
            m.colored_vertex(c + vec2(th.cos() * rad.x, th.sin() * rad.y) * f, col.gamma_multiply(a));
        }
    }
    for k in 0..SEG {
        let n = (k + 1) % SEG;
        m.add_triangle(0, 1 + k, 1 + n);
        for ring in 0..2u32 {
            let (a, b) = (1 + ring * SEG, 1 + (ring + 1) * SEG);
            m.add_triangle(a + k, a + n, b + k);
            m.add_triangle(a + n, b + n, b + k);
        }
    }
    pt.add(m);
}

/// The backdrop behind a dialog, faking the web's frosted glass (egui cannot blur): the
/// window dimmed with `bg_deep`, a faint glow of the accent behind the box as light
/// caught in the glass, and a fine grain that reads as frost.
pub fn backdrop(pt: &egui::Painter, p: &Palette, screen: Rect, dialog: Rect) {
    let dark = is_dark(p);
    pt.rect_filled(screen, 0.0, alpha(p.bg_deep, if dark { 0.62 } else { 0.5 }));
    glow(pt, dialog.center(), dialog.size() * 0.62 + vec2(220.0, 180.0), alpha(p.accent, if dark { 0.10 } else { 0.07 }));
    let tex = grain(pt.ctx());
    let ppp = pt.ctx().pixels_per_point();
    let uv = Rect::from_min_size(Pos2::ZERO, screen.size() * ppp / 128.0);
    pt.image(tex.id(), screen, uv, alpha(p.hi, if dark { 0.045 } else { 0.035 }));
}

/// A modal dialog in the app's look, as `egui::Modal` (Escape, a click beside it, or
/// `ui.close()` ask to close) but with the frosted backdrop and the pop: the box grows
/// from .96 and rises 8 px while it fades in (160 ms), the backdrop staying put.
pub fn modal<T>(ctx: &Context, id: Id, p: &Palette, frame: egui::Frame, content: impl FnOnce(&mut Ui) -> T) -> egui::ModalResponse<T> {
    modal_at(ctx, id, p, frame, None, content)
}

/// `modal`, hung `top` px below the window's top (centred across) rather than centred:
/// the command palette, which grows and shrinks downwards as its list changes.
pub fn modal_at<T>(ctx: &Context, id: Id, p: &Palette, frame: egui::Frame, top: Option<f32>, content: impl FnOnce(&mut Ui) -> T) -> egui::ModalResponse<T> {
    let screen = ctx.content_rect();
    let (area, origin) = match top {
        Some(y) => (egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, y)), pos2(screen.center().x, screen.min.y + y)),
        None => (egui::Modal::default_area(id), screen.center()),
    };
    let layer = area.layer();
    let t = appear(ctx, id, POP_DIALOG);
    // The box grows from its anchor.
    let tf = scale_about(origin, 0.96 + 0.04 * t, 8.0 * (1.0 - t));
    let is_top_modal = ctx.memory_mut(|mem| {
        mem.set_modal_layer(layer);
        mem.top_modal_layer() == Some(layer)
    });
    let any_popup_open = egui::Popup::is_any_open(ctx);
    let last = egui::AreaState::load(ctx, id).and_then(|s| s.size.map(|_| s.rect())).unwrap_or(Rect::from_center_size(screen.center(), vec2(440.0, 220.0)));
    let dark = is_dark(p);
    let egui::InnerResponse { inner: (inner, backdrop_response), response } = area.show(ctx, |ui| {
        // The screen in the layer's own coordinates: covered whatever the box's scale.
        let bg = tf.inverse() * screen;
        let mut backdrop_ui = ui.new_child(UiBuilder::new().sense(Sense::CLICK | Sense::DRAG).max_rect(bg));
        backdrop_ui.set_min_size(bg.size());
        let mut pt = ui.painter().clone();
        pt.set_clip_rect(bg);
        backdrop(&pt, p, bg, last);
        let backdrop_response = backdrop_ui.response();
        // A second, tight shadow under the frame's wide one: the box sits on the glass.
        let tight = ui.painter().add(Shape::Noop);
        let shown = ui.scope_builder(UiBuilder::new().sense(Sense::CLICK | Sense::DRAG), |ui| frame.show(ui, content).inner);
        let r = shown.response.rect;
        let cr = frame.corner_radius;
        ui.painter().set(tight, egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(if dark { 60 } else { 18 }) }.as_shape(r, cr));
        if dark {
            // Light along the top edge, as on a pane of glass.
            let y = ui.painter().round_to_pixel_center(r.min.y + 1.0);
            let inset = cr.nw as f32;
            ui.painter().hline(r.min.x + inset..=r.max.x - inset, y, Stroke::new(1.0, alpha(Color32::WHITE, 0.06)));
        }
        (shown.inner, backdrop_response)
    });
    if t < 1.0 {
        ctx.set_transform_layer(layer, tf);
        ctx.request_repaint();
    } else if ctx.layer_transform_to_global(layer).is_some() {
        ctx.set_transform_layer(layer, TSTransform::IDENTITY);
    }
    egui::ModalResponse { response, backdrop_response, inner, is_top_modal, any_popup_open }
}
