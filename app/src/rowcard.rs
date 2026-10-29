//! The row card (Ctrl+I), as 22-row-card.js: the row under the cursor as a form docked
//! at the right of the grid — one field per column, its type badge before its name, a
//! search over the names and the values, the record as written in the file (Raw), the
//! row's diagnosis (Why?). Reading one record across 85 columns by scrolling sideways is
//! what it replaces. The card follows the cursor; a field left (Enter, Tab, a click
//! elsewhere) writes its cell as an inline edit, one undo step.

use crate::grid::{self, mono_bold, semibold, Action, Cell};
use crate::icons::{self, Icon};
use crate::tab::Tab;
use crate::theme::{alpha, mix, Metrics, Palette};
use crate::tools::Cmd;
use crate::widgets::{self, blend, ease, Item};
use csvfab_engine::look::Rich;
use csvfab_engine::text::fold;
use csvfab_engine::view::Kind;
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui};
use egui::text::{LayoutJob, TextFormat};
use std::collections::HashMap;

/// A field's height before it was laid out once: one line.
const FIELD_H: f32 = 35.0;
/// The panel's width, published for the windows anchored at the right (the diagnosis).
pub const WIDTH_KEY: &str = "row-card-w";

#[derive(Default)]
pub struct Card {
    pub open: bool,
    query: String,
    raw: bool,
    skip_empty: bool,
    /// The field being typed in.
    edit: Option<Field>,
    /// The value just written, shown until the table has it (the next frame).
    written: Option<(RowId, usize, String)>,
    /// Field heights as last laid out, by column: a field out of view takes that room unlaid.
    heights: HashMap<usize, f32>,
    /// Problems the row's bytes show (errors and warnings of "Why?"), for (row, version).
    probs: Option<((RowId, u64), usize)>,
    /// A field to give the focus to, laid out even out of view and scrolled to.
    focus: Option<usize>,
    focus_search: bool,
}

struct Field {
    tab: u64,
    id: RowId,
    col: usize,
    text: String,
    old: String,
}

impl Card {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Open on the raw view: the row as it sits in the file.
    pub fn open_raw(&mut self) {
        self.open = true;
        self.raw = true;
    }
}

fn field_id(c: usize) -> Id {
    Id::unique(("row-card-field", c))
}

fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Takes a press of `key` with exactly `shift` (no Ctrl, no Alt) out of the input.
fn take_key(ui: &Ui, key: Key, shift: bool) -> bool {
    ui.input_mut(|i| {
        let mut hit = false;
        i.events.retain(|e| match e {
            egui::Event::Key { key: k, pressed: true, modifiers: md, .. } if *k == key && md.shift == shift && !md.command && !md.ctrl && !md.alt => {
                hit = true;
                false
            }
            _ => true,
        });
        hit
    })
}

/// The card at the right of the grid, while open. Its edits come back as actions for
/// the active tab.
pub fn panel(card: &mut Card, ui: &mut Ui, tab: &mut Tab, p: &Palette, m: &Metrics, out: &mut Vec<Cmd>) -> Vec<Action> {
    let mut actions = Vec::new();
    let frame = egui::Frame::new().fill(p.surf).inner_margin(egui::Margin { left: 14, right: 4, top: 10, bottom: 0 });
    let resp = egui::Panel::right("row-card")
        .resizable(true)
        .default_size(430.0)
        .size_range(320.0..=760.0)
        .frame(frame)
        .show_separator_line(false)
        .show(ui, |ui| body(card, ui, tab, p, m, out, &mut actions));
    let r = resp.response.rect;
    ui.painter().vline(ui.painter().round_to_pixel_center(r.min.x + 0.5), r.y_range(), Stroke::new(1.0, p.brd));
    ui.ctx().data_mut(|d| d.insert_temp(Id::unique(WIDTH_KEY), r.width()));
    actions
}

/// Forgets the published width (the card closed).
pub fn closed(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<f32>(Id::unique(WIDTH_KEY)));
}

#[allow(clippy::too_many_arguments)]
fn body(card: &mut Card, ui: &mut Ui, tab: &mut Tab, p: &Palette, m: &Metrics, out: &mut Vec<Cmd>, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    // The panel keeps its width whatever its content (a resizable panel shrinks to it).
    ui.expand_to_include_rect(ui.max_rect());
    let ctx = ui.ctx().clone();
    let Some(st) = tab.table.as_ref().map(|t| t.st.clone()) else { return };
    let rows = tab.shown();
    let cur = tab.grid.cursor.filter(|c| c.row < rows);
    let id = cur.map(|c| tab.shown_id(c.row));

    // A field left (the focus elsewhere, another row, another tab): its value written —
    // unless by Escape (egui drops the focus on it before the field sees the key).
    if let Some(f) = &card.edit {
        let focused = ctx.memory(|mem| mem.focused());
        if focused != Some(field_id(f.col)) || id != Some(f.id) || f.tab != tab.uid {
            let f = card.edit.take().unwrap();
            let esc = ui.input(|i| i.key_pressed(Key::Escape));
            if f.text != f.old && f.tab == tab.uid && !esc {
                card.written = Some((f.id, f.col, f.text.clone()));
                actions.push(Action::SetCells(vec![(f.id, f.col, f.text)]));
            }
        }
    }
    let written = card.written.take();
    if !actions.is_empty() { card.written = written.clone(); }

    // Alt+↑ / Alt+↓: the previous / next row, wherever the focus is. Ctrl+I in the card closes it.
    let step = ui.input_mut(|i| if i.consume_key(Modifiers::ALT, Key::ArrowUp) { -1 } else if i.consume_key(Modifiers::ALT, Key::ArrowDown) { 1 } else { 0 });
    let in_card = card.edit.is_some() || ctx.memory(|mem| mem.has_focus(Id::unique("row-card-search")));
    if in_card && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::I)) { card.open = false; }
    let go = |tab: &mut Tab, d: i64| {
        if let Some(c) = cur {
            let row = (c.row as i64 + d).clamp(0, rows as i64 - 1) as usize;
            let to = Cell { row, col: c.col };
            tab.grid.anchor = Some(to);
            tab.grid.cursor = Some(to);
            tab.grid.reveal_col = true;
        }
    };
    if step != 0 { go(tab, step); }

    let w = ui.available_width() - 10.0;

    // ---- head: the title, then ▲ ▼, Copy, Raw, Why?, ×
    let (hr, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let mut x = hr.max.x;
    let mut place = |bw: f32, gap: f32| { let r = Rect::from_min_max(pos2(x - bw, hr.min.y + 1.0), pos2(x, hr.max.y - 1.0)); x -= bw + gap; r };
    let base_row = id.is_some_and(|i| (i as usize) < st.base.n);
    let close_r = place(24.0, 8.0);
    let why_r = place(46.0, 5.0);
    let raw_r = place(42.0, 10.0);
    let copy_r = place(28.0, 5.0);
    let next_r = place(28.0, 4.0);
    let prev_r = place(28.0, 0.0);
    if icon_btn(ui, p, close_r, "rc-close", Icon::Cross, false, true, "Close (Ctrl+I)").clicked() { card.open = false; }
    if text_btn(ui, p, why_r, "rc-why", "Why?", false, base_row, "Why does this row look wrong? Its bytes, examined").clicked() {
        if let Some(i) = id { actions.push(Action::Diagnose(i)); }
    }
    if text_btn(ui, p, raw_r, "rc-raw", "Raw", card.raw, cur.is_some(), "The record as written in the file, delimiters, quotes and invisible characters shown").clicked() { card.raw = !card.raw; }
    let heads = st.headers();
    let n = heads.len();
    let mut vals = Vec::new();
    if let Some(i) = id { st.row(i, &mut vals); }
    vals.resize(n, String::new());
    if let Some((wid, wc, v)) = &written {
        if id == Some(*wid) && *wc < n { vals[*wc] = v.clone(); }
    }
    if icon_btn(ui, p, copy_r, "rc-copy", Icon::Copy, false, cur.is_some(), "Copy the row: one \"name: value\" line per field").clicked() {
        let text: Vec<String> = (0..n).map(|c| format!("{}: {}", heads[c], vals[c])).collect();
        ctx.copy_text(text.join("\n"));
        out.push(Cmd::Toast(format!("Row copied: {} fields", grid::fmt_int(n))));
    }
    if icon_btn(ui, p, next_r, "rc-next", Icon::ChevronDownBold, false, cur.is_some_and(|c| c.row + 1 < rows), "Next row (Alt+↓)").clicked() { go(tab, 1); }
    if icon_btn(ui, p, prev_r, "rc-prev", Icon::ChevronUpBold, false, cur.is_some_and(|c| c.row > 0), "Previous row (Alt+↑)").clicked() { go(tab, -1); }
    let title = match (cur, id) {
        (Some(_), Some(i)) if (i as usize) < st.base.n => format!("Row {}", grid::fmt_int((i as usize + 1).saturating_sub(st.header as usize))),
        (Some(_), Some(_)) => "New row".to_string(),
        _ => "Row card".to_string(),
    };
    let tg = ui.painter().layout(title, semibold(14.5), p.txt_hi, f32::INFINITY);
    let tg = if tg.size().x > x - hr.min.x - 6.0 {
        let mut job = LayoutJob::simple_singleline(tg.text().to_string(), semibold(14.5), p.txt_hi);
        job.wrap = egui::text::TextWrapping::truncate_at_width(x - hr.min.x - 6.0);
        ui.painter().layout_job(job)
    } else { tg };
    ui.painter().galley(pos2(hr.min.x, hr.center().y - tg.size().y / 2.0), tg, p.txt_hi);

    let (Some(c), Some(id)) = (cur, id) else {
        empty_state(ui, tab, p, w, rows);
        return;
    };
    let edited = st.row_edited(id);

    // ---- the line under the title: where the row is, what is odd about it
    let raw_text = base_row.then(|| st.base.text(st.base.record(id as usize)).into_owned());
    let delim = st.base.dialect.delim as char;
    let fields_in_file = raw_text.as_deref().map(|t| count_fields(t, delim));
    let mut sub = LayoutJob::default();
    let f = ui_font(11.5);
    let dim = TextFormat { font_id: f.clone(), color: p.txt_dim, ..Default::default() };
    let warn = TextFormat { font_id: f.clone(), color: p.warn, ..Default::default() };
    let sep = |job: &mut LayoutJob| job.append("  ·  ", 0.0, TextFormat { font_id: f.clone(), color: p.txt_faint, ..Default::default() });
    sub.append(&format!("{} of {} shown", grid::fmt_int(c.row + 1), grid::fmt_int(rows)), 0.0, dim.clone());
    sep(&mut sub);
    if base_row {
        let what = if st.base.dialect.lines { "line" } else { "record" };
        sub.append(&format!("{what} {} in the file", grid::fmt_int(id as usize + 1)), 0.0, dim.clone());
    } else {
        sub.append("new row, not in the file yet", 0.0, warn.clone());
    }
    if let Some(k) = fields_in_file.filter(|&k| k != n) {
        sep(&mut sub);
        sub.append(&format!("{} fields for {} columns", grid::fmt_int(k), grid::fmt_int(n)), 0.0, warn.clone());
    }
    if edited {
        sep(&mut sub);
        sub.append("edited", 0.0, TextFormat { font_id: f.clone(), color: p.accent_2, ..Default::default() });
    }
    // Problems in the line's bytes: counted here, explained by "Why?" (as the web's link).
    let version = tab.table.as_ref().map_or(0, |t| t.version());
    let probs = match card.probs {
        Some((k, n)) if k == (id, version) => n,
        _ => {
            let n = if base_row { csvfab_engine::diagnose::diagnose(&st, id).findings.iter().filter(|f| !matches!(f.severity, csvfab_engine::diagnose::Severity::Info)).count() } else { 0 };
            card.probs = Some(((id, version), n));
            n
        }
    };
    if probs > 0 {
        sep(&mut sub);
        sub.append(&format!("{} problem{} — why?", grid::fmt_int(probs), if probs > 1 { "s" } else { "" }), 0.0, TextFormat { underline: Stroke::new(1.0, alpha(p.warn, 0.5)), ..warn.clone() });
    }
    sub.wrap.max_width = w;
    ui.add_space(4.0);
    let sg = ui.painter().layout_job(sub);
    let (sr, sresp) = ui.allocate_exact_size(vec2(w, sg.size().y), if probs > 0 { Sense::click() } else { Sense::hover() });
    ui.painter().galley(sr.min, sg, p.txt_dim);
    if probs > 0 && sresp.on_hover_text("What is wrong in this line's bytes").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::Diagnose(id));
    }
    ui.add_space(9.0);
    let full = ui.max_rect();
    ui.painter().hline(full.min.x - 14.0..=full.max.x + 4.0, ui.painter().round_to_pixel_center(ui.cursor().min.y), Stroke::new(1.0, p.brd));
    ui.add_space(10.0);

    // ---- Raw: the record as written in the file
    if card.raw {
        raw_box(ui, p, &st, id, raw_text.as_deref(), edited, w, full.height());
        ui.add_space(10.0);
    }

    // ---- the search, and the switch for the empty fields
    let q = fold(card.query.trim());
    let editing_c = card.edit.as_ref().filter(|f| f.id == id && f.tab == tab.uid).map(|f| f.col);
    let empties = vals.iter().filter(|v| v.trim().is_empty()).count();
    let shown: Vec<usize> = (0..n)
        .filter(|&k| {
            let v = if editing_c == Some(k) { card.edit.as_ref().unwrap().text.as_str() } else { vals[k].as_str() };
            (!card.skip_empty || !v.trim().is_empty() || editing_c == Some(k) || card.focus == Some(k))
                && (q.is_empty() || fold(&heads[k]).contains(&q) || fold(v).contains(&q))
        })
        .collect();
    let (row_r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let eye_r = Rect::from_min_max(pos2(row_r.max.x - 30.0, row_r.min.y), row_r.max);
    let sr = Rect::from_min_max(row_r.min, pos2(eye_r.min.x - 6.0, row_r.max.y));
    let tip = if card.skip_empty { "Show the empty fields again".to_string() } else { format!("Hide the empty fields ({})", grid::fmt_int(empties)) };
    if icon_btn(ui, p, eye_r, "rc-empty", if card.skip_empty { Icon::EyeOff } else { Icon::Eye }, card.skip_empty, true, &tip).clicked() { card.skip_empty = !card.skip_empty; }
    let sid = Id::unique("row-card-search");
    let sfocus = ctx.memory(|mem| mem.has_focus(sid));
    widgets::field_frame(ui, p, sid, sr, sfocus, false, 6.0);
    icons::paint(ui.painter(), Icon::Magnifier, Rect::from_center_size(pos2(sr.min.x + 15.0, sr.center().y), vec2(14.0, 14.0)), p.txt_dim);
    let filtered = !q.is_empty() || card.skip_empty;
    let count = filtered.then(|| ui.painter().layout_no_wrap(format!("{} of {}", grid::fmt_int(shown.len()), grid::fmt_int(n)), ui_font(11.5), p.txt_dim));
    let mut right = sr.max.x - 10.0;
    if !card.query.is_empty() {
        let xr = Rect::from_center_size(pos2(sr.max.x - 15.0, sr.center().y), vec2(20.0, 20.0));
        let xresp = ui.interact(xr, Id::unique("rc-search-clear"), Sense::click());
        if xresp.hovered() { ui.painter().rect_filled(xr, 4.0, p.surf_2); }
        icons::paint(ui.painter(), Icon::Cross, Rect::from_center_size(xr.center(), vec2(12.0, 12.0)), if xresp.hovered() { p.txt_hi } else { p.txt_dim });
        if xresp.on_hover_text("Clear the search").clicked() { card.query.clear(); }
        right = xr.min.x - 4.0;
    }
    if let Some(g) = count {
        right -= g.size().x;
        ui.painter().galley(pos2(right, sr.center().y - g.size().y / 2.0), g, p.txt_dim);
        right -= 8.0;
    }
    let te = egui::TextEdit::singleline(&mut card.query)
        .id(sid)
        .font(ui_font(13.0))
        .text_color(p.txt_hi)
        .hint_text(egui::RichText::new("Find a field…").font(ui_font(13.0)).color(p.txt_sub))
        .frame(egui::Frame::NONE);
    let sresp = widgets::centered_edit(ui, Rect::from_min_max(pos2(sr.min.x + 29.0, sr.min.y), pos2(right, sr.max.y)), te);
    if std::mem::take(&mut card.focus_search) { sresp.request_focus(); }
    if sresp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        if let Some(&k) = shown.first() { card.focus = Some(k); }
    }
    if sfocus && ui.input(|i| i.key_pressed(Key::Escape)) { card.query.clear(); }
    if let Some(s) = crate::shot::take("card-q") { card.query = s; }
    if crate::shot::take("card-raw").is_some() { card.raw = true; }
    if crate::shot::take("card-empty").is_some() { card.skip_empty = true; }
    ui.add_space(8.0);

    // ---- the fields, in their own scroll
    if shown.is_empty() {
        ui.add_space(14.0);
        let msg = if q.is_empty() { "Every field of this row is empty.".to_string() } else { format!("No field matches \u{201c}{}\u{201d}.", card.query.trim()) };
        let g = ui.painter().layout(msg, ui_font(12.5), p.txt_dim, w);
        let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x - g.size().x / 2.0, r.min.y), g, p.txt_dim);
        return;
    }
    let scroll = &mut ui.spacing_mut().scroll;
    scroll.floating = false;
    scroll.bar_width = 8.0;
    scroll.bar_inner_margin = 4.0;
    scroll.bar_outer_margin = 2.0;
    scroll.dormant_background_opacity = 0.5;
    scroll.active_background_opacity = 0.7;
    scroll.interact_background_opacity = 0.8;
    let kinds = tab.kinds.clone();
    let rich = tab.rich.clone();
    let hidden = tab.grid.hidden.clone();
    let uid = tab.uid;
    let focus_to = card.focus.take();
    egui::ScrollArea::vertical()
        .id_salt("row-card-fields")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show_viewport(ui, |ui, vp| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let fw = ui.available_width() - 4.0;
            // The names' column as wide as the widest name needs, up to 40 % of the card.
            let widest = shown.iter().take(400).map(|&k| ctx.fonts_mut(|f| f.layout_no_wrap(heads[k].clone(), ui_font(12.5), Color32::WHITE).size().x)).fold(0.0, f32::max);
            let lw = (widest + 16.0 + 7.0 + 16.0).clamp(96.0, (fw * 0.38).max(96.0));
            let mut y = 0.0;
            for (i, &k) in shown.iter().enumerate() {
                let h = card.heights.get(&k).copied().unwrap_or(FIELD_H);
                if focus_to != Some(k) && editing_c != Some(k) && (y + h < vp.min.y - 120.0 || y > vp.max.y + 120.0) {
                    ui.allocate_space(vec2(fw, h));
                    y += h;
                    continue;
                }
                let top = ui.cursor().min;
                let fid = field_id(k);
                let has = ctx.memory(|mem| mem.has_focus(fid));
                // Enter, Tab: the next field (Shift+Tab the one before). Escape: see the top.
                let (enter, tabk, back) = if has { (take_key(ui, Key::Enter, false), take_key(ui, Key::Tab, false), take_key(ui, Key::Tab, true)) } else { (false, false, false) };
                let is_edit = editing_c == Some(k) && card.edit.is_some();
                let mut text = if is_edit { card.edit.as_ref().unwrap().text.clone() } else { vals[k].clone() };
                let kind = kinds.get(k).copied().unwrap_or(Kind::Text);
                let link = !crate::theme::raw() && matches!(rich.get(k).and_then(|r| r.as_ref()), Some(Rich::Url | Rich::Email));
                let vcol = match kind {
                    Kind::Number if crate::theme::type_colors() => p.num_txt,
                    Kind::Date if crate::theme::type_colors() => p.date_txt,
                    _ if link => p.accent_2,
                    _ => p.txt_hi,
                };
                let is_cur = c.col == k;
                let is_hidden = hidden.contains(&k);
                let cell_edited = st.edited(id, k).is_some();

                // The value's box: bg_deep, its border on hover, prim and a ring under focus.
                let vx = top.x + lw + 10.0;
                let vw = fw - lw - 10.0;
                let bg_at = ui.painter().add(egui::Shape::Noop);
                let pre = Rect::from_min_size(pos2(vx, top.y + 4.0), vec2(vw, card.heights.get(&k).map_or(26.0, |h| h - 9.0)));
                let box_resp = ui.interact(pre, fid.with("box"), Sense::click());
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(vx + 9.0, top.y + 4.0 + 5.0), vec2(vw - 18.0, 4000.0))).layout(egui::Layout::top_down(egui::Align::Min)));
                child.spacing_mut().interact_size.y = 0.0;
                let te = egui::TextEdit::multiline(&mut text)
                    .id(fid)
                    .return_key(egui::KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                    .desired_rows(1)
                    .desired_width(vw - 18.0)
                    .font(FontId::monospace(12.5))
                    .text_color(vcol)
                    .hint_text(egui::RichText::new("empty").font(ui_font(12.0)).color(alpha(p.txt_faint, 0.7)))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .show(&mut child);
                let vresp = te.response.clone();
                if box_resp.clicked() { vresp.request_focus(); }
                let vbox = Rect::from_min_max(pos2(vx, top.y + 4.0), pos2(vx + vw, (te.response.rect.max.y + 5.0).max(top.y + 4.0 + 26.0)));
                let focused = vresp.has_focus();
                let fe = ease(&ctx, fid.with("focus"), focused);
                let he = ease(&ctx, fid.with("hover"), ui.rect_contains_pointer(vbox));
                let brd = blend(p.prim, blend(p.brd_2, alpha(p.brd, 0.45), he), fe);
                let fill = mix(p.bg_deep, p.surf, 0.45 * (1.0 - he.max(fe)));
                let mut shapes = vec![egui::Shape::rect_filled(vbox, 5.0, fill)];
                if fe > 0.0 { shapes.push(egui::Shape::rect_stroke(vbox, 5.0, Stroke::new(3.0, alpha(p.prim, 0.22 * fe)), StrokeKind::Outside)); }
                shapes.push(egui::Shape::rect_stroke(vbox, 5.0, Stroke::new(1.0, brd), StrokeKind::Inside));
                ui.painter().set(bg_at, egui::Shape::Vec(shapes));

                // Typing: the field held as an edit; the grid's cursor on its column.
                if focused && !is_edit {
                    // The field left in this same frame (Enter, Tab) is written first.
                    if let Some(f) = card.edit.take().filter(|f| f.text != f.old && f.tab == uid) {
                        card.written = Some((f.id, f.col, f.text.clone()));
                        actions.push(Action::SetCells(vec![(f.id, f.col, f.text)]));
                    }
                    card.edit = Some(Field { tab: uid, id, col: k, old: text.clone(), text: text.clone() });
                    let to = Cell { row: c.row, col: k };
                    tab.grid.anchor = Some(to);
                    tab.grid.cursor = Some(to);
                } else if focused {
                    if let Some(f) = card.edit.as_mut() { f.text = text.clone(); }
                }
                if enter || tabk || back {
                    let next = if back { i.checked_sub(1).map(|j| shown[j]) } else { shown.get(i + 1).copied() };
                    match next {
                        Some(nk) => { card.focus = Some(nk); ctx.memory_mut(|mem| mem.request_focus(field_id(nk))); }
                        None => vresp.surrender_focus(),
                    }
                }
                if focus_to == Some(k) {
                    vresp.request_focus();
                    vresp.scroll_to_me(None);
                }

                // The name: badge, title (hidden: faint; the cursor's column: accent), edited dot.
                let fh = vbox.max.y + 5.0 - top.y;
                let lr = Rect::from_min_size(top, vec2(lw, fh));
                let lresp = ui.interact(Rect::from_min_size(top, vec2(lw, 34.0)), fid.with("label"), Sense::click());
                let pt = ui.painter();
                if is_cur {
                    pt.rect_filled(Rect::from_min_max(pos2(top.x - 14.0, top.y), pos2(top.x + fw + 4.0, top.y + fh)), 0.0, alpha(p.prim, 0.07));
                    pt.rect_filled(Rect::from_min_size(pos2(top.x - 14.0, top.y + 3.0), vec2(3.0, fh - 6.0)), 1.5, p.prim);
                }
                let (glyph, base, gcol, kind_name) = grid::kind_style(tab, k, p);
                let cy = top.y + 4.0 + 13.0;
                let gg = glyph.map(|g| pt.layout_no_wrap(g.into(), mono_bold(9.0), gcol));
                let gw = gg.as_ref().map_or(10.0, |g| g.size().x);
                let bw = (gw + 10.0).max(16.0).round();
                let br = Rect::from_min_size(pos2(lr.min.x, (cy - 8.0).round()), vec2(bw, 16.0));
                let bf = mix(base, p.surf, if is_cur { 0.22 } else { 0.10 });
                pt.rect_filled(br, 8.0, bf);
                let gc = mix(gcol, bf, 0.75);
                match gg {
                    Some(g) => pt.galley_with_override_text_color(pos2((br.center().x - gw / 2.0).round(), (cy - g.size().y / 2.0).round()), g, gc),
                    None => icons::calendar(pt, Rect::from_center_size(br.center(), vec2(10.0, 10.0)), 1.6, gc),
                }
                let ncol = if is_cur { p.accent_2 } else if is_hidden { p.txt_faint } else if lresp.hovered() { p.txt_strong } else { p.txt_mut };
                let mark_w = if is_hidden || cell_edited { 14.0 } else { 0.0 };
                let mut job = LayoutJob::simple_singleline(heads[k].clone(), if is_cur { semibold(12.5) } else { ui_font(12.5) }, ncol);
                job.wrap = egui::text::TextWrapping::truncate_at_width(lw - bw - 7.0 - mark_w);
                let ng = pt.layout_job(job);
                let nx = br.max.x + 7.0;
                let nw = ng.size().x;
                pt.galley(pos2(nx, (cy - ng.size().y / 2.0).round()), ng, ncol);
                if is_hidden {
                    icons::paint(pt, Icon::EyeOff, Rect::from_center_size(pos2(nx + nw + 9.0, cy), vec2(12.0, 12.0)), p.txt_faint);
                } else if cell_edited {
                    pt.circle_filled(pos2(nx + nw + 8.0, cy), 3.0, p.accent);
                }
                if fh > 0.0 && i + 1 < shown.len() {
                    pt.hline(top.x..=top.x + fw, pt.round_to_pixel_center(top.y + fh - 0.5), Stroke::new(1.0, alpha(p.brd, 0.6)));
                }
                if lresp.hovered() { crate::theme::pointing_hand(&ctx); }
                let mut tip = format!("{} · column {} · {kind_name}", heads[k], k + 1);
                if is_hidden { tip.push_str(" · hidden in the grid"); }
                if cell_edited { tip.push_str(" · edited"); }
                tip.push_str("\nClick: show in the grid · right click: more");
                let lresp = lresp.on_hover_text(tip);
                if lresp.clicked() { show_in_grid(tab, c.row, k); }
                egui::Popup::context_menu(&lresp).frame(crate::theme::menu_frame(p, m)).show(|ui| {
                    widgets::width(ui, 220.0);
                    if Item::new("Show in the grid").icon(Icon::Goto).show(ui, p).clicked() { show_in_grid(tab, c.row, k); ui.close(); }
                    if Item::new("Copy the value").icon(Icon::Copy).show(ui, p).clicked() { ctx.copy_text(vals[k].clone()); ui.close(); }
                    if Item::new("Copy the name").icon(Icon::Copy).show(ui, p).clicked() { ctx.copy_text(heads[k].clone()); ui.close(); }
                    widgets::sep(ui, p);
                    let label = if is_hidden { "Show the column" } else { "Hide the column" };
                    if Item::new(label).icon(if is_hidden { Icon::Eye } else { Icon::EyeOff }).show(ui, p).clicked() {
                        if is_hidden { tab.grid.hidden.remove(&k); } else if tab.grid.hidden.len() + 1 < n { tab.grid.hidden.insert(k); }
                        ui.close();
                    }
                });
                ui.allocate_rect(Rect::from_min_size(top, vec2(fw, fh)), Sense::hover());
                card.heights.insert(k, fh);
                y += fh;
            }
            ui.add_space(14.0);
        });
}

/// The column shown in the grid (shown again if hidden), its cell under the cursor.
fn show_in_grid(tab: &mut Tab, row: usize, col: usize) {
    tab.grid.hidden.remove(&col);
    let to = Cell { row, col };
    tab.grid.anchor = Some(to);
    tab.grid.cursor = Some(to);
    tab.grid.reveal_col = true;
}

/// No row under the cursor: what to do, and the first row a click away.
fn empty_state(ui: &mut Ui, tab: &mut Tab, p: &Palette, w: f32, rows: usize) {
    ui.add_space(14.0);
    let full = ui.max_rect();
    ui.painter().hline(full.min.x - 14.0..=full.max.x + 4.0, ui.painter().round_to_pixel_center(ui.cursor().min.y), Stroke::new(1.0, p.brd));
    ui.add_space((ui.available_height() * 0.28).max(30.0));
    let (r, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
    let disc = Rect::from_center_size(r.center(), vec2(64.0, 64.0));
    ui.painter().circle_filled(disc.center(), 32.0, alpha(p.prim, 0.10));
    icons::paint(ui.painter(), Icon::Card, Rect::from_center_size(disc.center(), vec2(28.0, 28.0)), p.accent_2);
    ui.add_space(16.0);
    let line = |ui: &mut Ui, text: &str, font: FontId, col: Color32| {
        let mut job = LayoutJob::simple(text.to_string(), font, col, w - 40.0);
        job.halign = egui::Align::Center;
        let g = ui.painter().layout_job(job);
        let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x, r.min.y), g, col);
    };
    line(ui, "No row selected", semibold(15.0), p.txt_hi);
    ui.add_space(6.0);
    line(ui, "Click a cell or a row number in the grid: its fields show here, one per column, to read and edit.", ui_font(12.5), p.txt_dim);
    if rows > 0 {
        ui.add_space(18.0);
        let bw = 150.0;
        let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        let br = Rect::from_center_size(r.center(), vec2(bw, 30.0));
        if text_btn(ui, p, br, "rc-first", "Show the first row", false, true, "").clicked() {
            let col = (0..tab.kinds.len().max(1)).find(|c| !tab.grid.hidden.contains(c)).unwrap_or(0);
            let to = Cell { row: 0, col };
            tab.grid.anchor = Some(to);
            tab.grid.cursor = Some(to);
            tab.grid.reveal_col = true;
        }
    }
    ui.add_space(18.0);
    line(ui, "Alt+↑ / Alt+↓ step through the rows · Ctrl+I closes the card", ui_font(11.5), p.txt_faint);
}

/// The fields in one record's text: its delimiters outside quotes, plus one.
fn count_fields(text: &str, delim: char) -> usize {
    let mut q = false;
    1 + text.chars().filter(|&ch| { if ch == '"' { q = !q; } !q && ch == delim }).count()
}

fn invisible_name(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0xA0 => "no-break space",
        0x202F => "narrow no-break space",
        0x2007 => "figure space",
        0x2009 => "thin space",
        0x200B => "zero-width space",
        0x200C => "zero-width non-joiner",
        0x200D => "zero-width joiner",
        0x200E => "left-to-right mark",
        0x200F => "right-to-left mark",
        0x202A..=0x202E => "direction formatting",
        0x2060..=0x2064 => "invisible format character",
        0x061C => "arabic letter mark",
        0xFEFF => "byte order mark",
        0xAD => "soft hyphen",
        0x7F => "delete character",
        c if c < 32 && c != 9 && c != 10 && c != 13 => "control character",
        _ => return None,
    })
}

/// The record as it is in the file: delimiters and quotes marked, line breaks and
/// invisible characters named; a legend of the invisible ones under it.
#[allow(clippy::too_many_arguments)]
fn raw_box(ui: &mut Ui, p: &Palette, st: &csvfab_engine::State, id: RowId, text: Option<&str>, edited: bool, w: f32, panel_h: f32) {
    let frame = egui::Frame::new().fill(p.bg_deep).stroke(Stroke::new(1.0, p.brd)).corner_radius(6).inner_margin(egui::Margin { left: 10, right: 6, top: 8, bottom: 8 });
    frame.show(ui, |ui| {
        ui.set_width(w - 18.0);
        let Some(text) = text else {
            ui.label(egui::RichText::new("This row was created here: it has no line in the file yet.").font(ui_font(12.0)).color(p.txt_dim));
            return;
        };
        let b = &st.base;
        let (s, e) = (b.starts.get(id as usize), b.starts.get(id as usize + 1));
        let delim = b.dialect.delim as char;
        let mut meta = format!("bytes {}–{} ({}) · {} fields", grid::fmt_int(s), grid::fmt_int(e), grid::fmt_int(e - s), grid::fmt_int(count_fields(text, delim)));
        if b.enc != csvfab_engine::enc::Enc::Utf8 { meta.push_str(&format!(" · read as {}", b.enc.label())); }
        let mut mj = LayoutJob::default();
        mj.append(&meta, 0.0, TextFormat { font_id: ui_font(11.5), color: p.txt_dim, ..Default::default() });
        if edited { mj.append("  ·  as read from disk: this row was edited since", 0.0, TextFormat { font_id: ui_font(11.5), color: p.warn, ..Default::default() }); }
        mj.wrap.max_width = w - 22.0;
        ui.label(mj);
        ui.add_space(6.0);

        let mono = FontId::monospace(12.0);
        let plain = TextFormat { font_id: mono.clone(), color: p.txt_hi, ..Default::default() };
        let dl = TextFormat { font_id: mono_bold(12.0), color: p.orange_txt, background: alpha(p.orange, 0.18), ..Default::default() };
        let qt = TextFormat { font_id: mono_bold(12.0), color: p.violet_txt, ..Default::default() };
        let inv = TextFormat { font_id: FontId::monospace(10.5), color: p.warn, background: alpha(p.warn, 0.14), ..Default::default() };
        let mut job = LayoutJob::default();
        let mut run = String::new();
        let mut q = false;
        let mut found: Vec<(u32, usize)> = Vec::new();
        const MAX: usize = 20_000;
        let flush = |job: &mut LayoutJob, run: &mut String| { if !run.is_empty() { job.append(run, 0.0, plain.clone()); run.clear(); } };
        for (n, ch) in text.chars().enumerate() {
            if n >= MAX {
                flush(&mut job, &mut run);
                job.append(" …", 0.0, TextFormat { font_id: ui_font(11.5), color: p.txt_dim, ..Default::default() });
                break;
            }
            let cp = ch as u32;
            if ch == '"' {
                q = !q;
                flush(&mut job, &mut run);
                job.append("\"", 0.0, qt.clone());
            } else if ch == delim && !q {
                flush(&mut job, &mut run);
                job.append(&if ch == '\t' { "⇥".to_string() } else { ch.to_string() }, 0.0, dl.clone());
            } else if ch == '\n' {
                flush(&mut job, &mut run);
                job.append("↵", 0.0, inv.clone());
                job.append("\n", 0.0, plain.clone());
            } else if ch == '\r' {
                flush(&mut job, &mut run);
                job.append("CR", 0.0, inv.clone());
            } else if let Some(_name) = invisible_name(cp) {
                flush(&mut job, &mut run);
                job.append(&format!("U+{cp:04X}"), 0.0, inv.clone());
                match found.iter_mut().find(|f| f.0 == cp) { Some(f) => f.1 += 1, None => found.push((cp, 1)) }
            } else {
                run.push(ch);
            }
        }
        flush(&mut job, &mut run);
        job.wrap.max_width = w - 22.0;
        egui::ScrollArea::vertical().id_salt("row-card-raw").max_height((panel_h * 0.36).max(80.0)).auto_shrink([false, true]).show(ui, |ui| {
            ui.label(job);
        });
        if !found.is_empty() {
            ui.add_space(6.0);
            let legend: Vec<String> = found.iter().map(|(cp, k)| format!("U+{cp:04X} {}{}", invisible_name(*cp).unwrap_or(""), if *k > 1 { format!(" ×{k}") } else { String::new() })).collect();
            let mut lj = LayoutJob::default();
            lj.append(&legend.join("  ·  "), 0.0, TextFormat { font_id: ui_font(11.5), color: p.warn, ..Default::default() });
            lj.wrap.max_width = w - 22.0;
            ui.label(lj);
        }
    });
}

/// A small `.btn-outline` with an icon: `on` (a switch that is on) in `prim`.
#[allow(clippy::too_many_arguments)]
fn icon_btn(ui: &mut Ui, p: &Palette, r: Rect, key: &str, icon: Icon, on: bool, enabled: bool, tip: &str) -> egui::Response {
    let resp = ui.interact(r, Id::unique(key), if enabled { Sense::click() } else { Sense::hover() });
    let fg = btn_box(ui, p, r, &resp, on, enabled);
    icons::paint(ui.painter(), icon, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), fg);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

/// A small `.btn-outline` with a word.
#[allow(clippy::too_many_arguments)]
fn text_btn(ui: &mut Ui, p: &Palette, r: Rect, key: &str, text: &str, on: bool, enabled: bool, tip: &str) -> egui::Response {
    let resp = ui.interact(r, Id::unique(key), if enabled { Sense::click() } else { Sense::hover() });
    let fg = btn_box(ui, p, r, &resp, on, enabled);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, widgets::medium(12.0), fg);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

fn btn_box(ui: &Ui, p: &Palette, r: Rect, resp: &egui::Response, on: bool, enabled: bool) -> Color32 {
    if on {
        let h = ease(ui.ctx(), resp.id, resp.hovered());
        ui.painter().rect(r, 6.0, alpha(p.prim, 0.14 + 0.06 * h), Stroke::new(1.0, p.prim), StrokeKind::Inside);
        if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
        return p.accent_2;
    }
    widgets::button_box(ui, p, widgets::Kind::Outline, r, resp, enabled, CornerRadius::same(6))
}

#[cfg(test)]
mod tests {
    use super::field_id;
    use crate::grid::Cell;
    use eframe::egui::{self, Event, Key, Modifiers};
    use std::time::Duration;

    fn key(h: &mut egui_kittest::Harness<'_, crate::app::App>, k: Key, modifiers: Modifiers) {
        h.event(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
        h.event(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    }

    /// The real app off screen: a field typed in and left with Enter writes its cell; Escape
    /// gives the value back; Alt+↓ goes to the next row.
    #[test]
    fn fields_write_their_cells() {
        let dir = std::env::temp_dir().join(format!("csvfab-card-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.csv");
        std::fs::write(&path, "id,address,city\n1,12 rue de Paris,Lyon\n2,3 avenue Foch,Nice\n").unwrap();
        let p2 = path.clone();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 700.0)).build_eframe(move |cc| crate::app::App::new(cc, vec![p2]));
        for _ in 0..200 {
            h.step();
            if h.state().tabs.first().is_some_and(|t| t.table.is_some() && !t.busy()) { break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let cell = |h: &egui_kittest::Harness<'_, crate::app::App>, row: usize, col: usize| {
            let t = &h.state().tabs[0];
            t.table.as_ref().unwrap().st.cell(t.shown_id(row), col)
        };
        h.state_mut().tabs[0].grid.cursor = Some(Cell { row: 0, col: 0 });
        h.state_mut().card.open = true;
        h.run_steps(3);

        // Typed, then Enter: written, the focus on to the next field.
        h.ctx.memory_mut(|m| m.request_focus(field_id(1)));
        h.run_steps(2);
        key(&mut h, Key::A, Modifiers::COMMAND);
        h.event(Event::Text("8 place Bellecour".into()));
        h.run_steps(1);
        key(&mut h, Key::Enter, Modifiers::NONE);
        h.run_steps(3);
        assert_eq!(cell(&h, 0, 1), "8 place Bellecour");
        assert!(h.ctx.memory(|m| m.has_focus(field_id(2))));
        assert_eq!(h.state().tabs[0].grid.cursor.map(|c| c.col), Some(2));

        // Typed, then Escape: nothing written.
        key(&mut h, Key::A, Modifiers::COMMAND);
        h.event(Event::Text("Grenoble".into()));
        h.run_steps(1);
        key(&mut h, Key::Escape, Modifiers::NONE);
        h.run_steps(3);
        assert_eq!(cell(&h, 0, 2), "Lyon");

        // Shift+Enter: a line break in the value.
        h.ctx.memory_mut(|m| m.request_focus(field_id(2)));
        h.run_steps(2);
        key(&mut h, Key::A, Modifiers::COMMAND);
        h.event(Event::Text("a".into()));
        key(&mut h, Key::Enter, Modifiers::SHIFT);
        h.event(Event::Text("b".into()));
        h.run_steps(1);
        key(&mut h, Key::Enter, Modifiers::NONE);
        h.run_steps(3);
        assert_eq!(cell(&h, 0, 2), "a\nb");

        // Tab: the next field, its text whole (not a tab character).
        h.ctx.memory_mut(|m| m.request_focus(field_id(0)));
        h.run_steps(2);
        key(&mut h, Key::Tab, Modifiers::NONE);
        h.run_steps(2);
        assert!(h.ctx.memory(|m| m.has_focus(field_id(1))));
        key(&mut h, Key::Tab, Modifiers::SHIFT);
        h.run_steps(2);
        assert!(h.ctx.memory(|m| m.has_focus(field_id(0))));
        assert_eq!(cell(&h, 0, 0), "1");

        // Alt+↓: the next row under the cursor.
        key(&mut h, Key::ArrowDown, Modifiers::ALT);
        h.run_steps(2);
        assert_eq!(h.state().tabs[0].grid.cursor.map(|c| c.row), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
