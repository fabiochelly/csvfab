//! Everything around the grid (docs/look.md): the tab bar, the toolbar with
//! the search, its toggles and the filter chips, the status bar with its
//! format pills and switches, the main menu, the command palette, toasts and
//! the welcome screen.
//!
//! Motion (look.md, "Motion"): hover fills ease in, chips and the Save badge
//! pop, the counts run to their new values, toasts rise and slide away, the
//! palette's selection glides. Each asks for frames only while it moves.

use crate::app::{App, Dialog};
use crate::grid::{fmt_int, semibold};
use crate::icons::{self, Icon};
use crate::theme::{alpha, Palette as Colors};
use crate::tools;
use crate::widgets::{blend as mix, ease, snap};
use csvfab_engine::{Enc, RowId};
use eframe::egui;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Rect, RichText, Sense, Stroke, StrokeKind, Ui};
use std::collections::HashMap;

pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+P, Ctrl+Shift+P", "Command palette: every action, tab and recent file"),
    ("Ctrl+O / Ctrl+S", "Open / save"),
    ("Ctrl+Shift+S", "Save as"),
    ("Ctrl+W / Ctrl+Q", "Close the tab / quit"),
    ("Ctrl+Z / Ctrl+Y, Ctrl+Shift+Z", "Undo / redo"),
    ("Ctrl+F, F3 / Shift+F3", "Find & replace, next / previous match"),
    ("Ctrl+C / Ctrl+V", "Copy / paste a range (Excel format)"),
    ("Ctrl+A, arrows, Shift+arrows", "Select"),
    ("Ctrl+arrows", "To the first / last row or column"),
    ("PgUp / PgDn", "A page up / down"),
    ("Home / End, Ctrl+Home / Ctrl+End", "First / last cell of the row, of the file"),
    ("Esc", "Close the find bar, then drop the selection"),
    ("Enter, F2, double-click, or type", "Edit the cell"),
    ("Shift+Enter (editing)", "A line break in the value"),
    ("Enter / Ctrl+Enter (on a block)", "The value in every selected cell / a series from it"),
    ("Delete", "Clear the selected cells"),
    ("Ctrl+D", "Fill a series down the selection (1, 2, 3… or the same value)"),
    ("Ctrl+E", "Fill by example: type one or two results, the column follows"),
    ("Ctrl+M", "File map: the whole file as one picture"),
    ("Ctrl+I", "Row card"),
    ("Alt+↑ / Alt+↓", "Row card: previous / next row"),
    ("Ctrl+G", "Go to a row"),
    ("Alt+← / Alt+→", "Switch tab"),
    ("Alt+1 … Alt+9", "Go to that tab"),
    ("F1, ?", "These shortcuts"),
];

/// The mouse on the columns (the web's "Columns, with the mouse").
pub const MOUSE: &[(&str, &str)] = &[
    ("Click a title", "Sort; again: reverse"),
    ("Shift+click a title", "Sort by one more column"),
    ("Double-click a title", "Rename the column"),
    ("Drag a title", "Move the column"),
    ("Double-click a title's edge", "Fit the column to its content"),
    ("Drag across a column chart", "Keep the rows in that range"),
    ("Right-click a title", "The column's menu"),
    ("Drag a row number", "Move the row"),
    ("Right-click a row number", "The row's menu"),
    ("Right-click a cell", "The cell's menu: copy, filter by it, fill…"),
    ("Rest on a cut-off cell", "Its whole value"),
    ("Drag the selection's corner", "Fill a series (Ctrl+drag: copy the values)"),
    ("Ctrl+click a link", "Open it"),
];

fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

fn medium(size: f32) -> FontId {
    crate::widgets::medium(size)
}

pub fn fmt_size(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// A key cap (`Ctrl`, `P`, `F1`): mono 11, padding 1 × 5, radius 4, 1 px `alpha(hi, .2)`,
/// fill `alpha(hi, .06)`, text `txt_mut`. Drawn with its right edge at `right`; returns its rect.
fn key_cap(pt: &egui::Painter, p: &Colors, right: f32, cy: f32, text: &str) -> Rect {
    let g = pt.layout_no_wrap(text.to_string(), FontId::monospace(11.0), p.txt_mut);
    let r = Rect::from_min_size(pos2(right - g.size().x - 10.0, cy - 9.0), vec2(g.size().x + 10.0, 18.0));
    pt.rect(r, 4.0, alpha(p.hi, 0.06), Stroke::new(1.0, alpha(p.hi, 0.2)), StrokeKind::Inside);
    pt.galley(pos2(r.min.x + 5.0, snap(pt.ctx(), cy - g.size().y / 2.0)), g, p.txt_mut);
    r
}

/// A key combination as caps, right-aligned at `right` ("Ctrl+Shift+S" → three caps); returns
/// the left edge.
fn key_caps(pt: &egui::Painter, p: &Colors, right: f32, cy: f32, keys: &str) -> f32 {
    let mut x = right;
    let parts: Vec<&str> = if keys.len() > 1 { keys.split('+').collect() } else { vec![keys] };
    for k in parts.iter().rev() {
        x = key_cap(pt, p, x, cy, k).min.x - 3.0;
    }
    x + 3.0
}

/// The screenshot hooks of the shell: `theme:<id>` (without saving it), `welcome` (no tab),
/// `palette[:query]`, `palette-recent:a;b`, `toast:<text>`, `dirty:<n>` (n edits).
fn shots(app: &mut App, ctx: &egui::Context) {
    if std::env::var_os("CSVFAB_SHOT").is_none() {
        return;
    }
    if let Some(id) = crate::shot::take("theme") {
        app.theme = crate::theme::by_id(&id);
        crate::theme::apply(ctx, app.theme, &app.m);
    }
    if crate::shot::take("welcome").is_some() {
        while !app.tabs.is_empty() {
            app.close_tab(0, true);
        }
    }
    if app.tabs.get(app.active).is_some_and(|t| t.table.is_none() || t.busy()) {
        return;
    }
    if let Some(r) = crate::shot::take("palette-recent") {
        *recent_commands().lock().unwrap() = r.split(';').map(str::to_string).collect();
    }
    if let Some(q) = crate::shot::take("palette") {
        app.palette = Some(Launcher { query: q, ..Default::default() });
    }
    if let Some(n) = crate::shot::take("dirty") {
        if let Some(tab) = app.tabs.get_mut(app.active) {
            for k in 0..n.parse::<usize>().unwrap_or(1) {
                let id = tab.shown_id(k);
                tab.apply_changes(&[(id, 2, format!("edited {k}"))], ctx);
            }
        }
    }
    if let Some(t) = crate::shot::take("toast") {
        // Held for a minute: the renderer's clock runs faster than the wall's.
        let now = ctx.input(|i| i.time);
        for (k, m) in t.split(';').enumerate() {
            // "… +undo": with its Undo button.
            let mut t = crate::app::Toast::new(m.trim_end_matches(" +undo").to_string(), now + 60.0 + k as f64);
            if m.ends_with(" +undo") { t.undo = Some((0, 0)); }
            app.toasts.push(t);
        }
    }
}

// ---------------------------------------------------------------- tab bar

pub fn tab_bar(app: &mut App, ui: &mut Ui) {
    shots(app, &ui.ctx().clone());
    let (p, m) = (&app.theme.p, &app.m);
    let mut close = None;
    let mut open = false;
    let mut pick = None;
    let frame = egui::Frame::new().fill(p.bg_deep).inner_margin(egui::Margin { left: 10, right: 10, top: 5, bottom: 0 });
    egui::Panel::top("tabs").exact_size(m.tab_bar_h).frame(frame).show_separator_line(false).show(ui, |ui| {
        let ctx = ui.ctx().clone();
        let bar = ui.max_rect();
        let bottom = bar.max.y;
        let rule_y = ui.painter().round_to_pixel_center(bottom - 0.5);
        ui.painter().hline(bar.x_range().expand(10.0), rule_y, Stroke::new(1.0, p.brd));
        let t_now = ui.input(|i| i.time);
        let mut loading = false;
        // Many tabs: the names shrink (down to 60 px), then the tabs scroll sideways (the wheel
        // over them, the active one kept in view) and ▾ lists them all, as the web's bar scrolled.
        let width = |t: &crate::tab::Tab, cap: f32| -> f32 {
            let name = ui.painter().layout_no_wrap(t.name.clone(), ui_font(12.5), p.txt_hi).size().x;
            let meta = ui.painter().layout_no_wrap(t.st().map(|s| fmt_int(s.len())).unwrap_or_default(), ui_font(11.0), p.txt_dim).size().x;
            (10.0_f32 + 7.0 + 8.0 + name.min(cap) + 8.0 + meta + if t.dirty() { 14.0 } else { 0.0 } + 8.0 + 16.0 + 8.0).min(m.tab_max_w).round()
        };
        let total = |cap: f32| app.tabs.iter().map(|t| width(t, cap) + 4.0).sum::<f32>();
        let room = bar.width() - 24.0 - 4.0;
        let mut cap = 150.0;
        while cap > 60.0 && total(cap) > room { cap -= 10.0; }
        let over = total(cap) > room;
        let room = if over { room - 28.0 } else { room };
        let ws: Vec<f32> = app.tabs.iter().map(|t| width(t, cap)).collect();
        let sid = Id::unique("tab-scroll");
        let mut off: f32 = ctx.data(|d| d.get_temp(sid)).unwrap_or(0.0);
        if over {
            if ui.rect_contains_pointer(bar) {
                let d = ui.input(|i| i.smooth_scroll_delta);
                off -= if d.x != 0.0 { d.x } else { d.y };
            }
            // The active tab in view.
            let left: f32 = ws[..app.active.min(ws.len())].iter().map(|w| w + 4.0).sum();
            let right = left + ws.get(app.active).copied().unwrap_or(0.0);
            // Brought into view when it changes (a click, Alt+→, a new tab), not against the wheel.
            let seen = (app.active, app.tabs.len());
            if ctx.data(|d| d.get_temp::<(usize, usize)>(sid.with("seen"))) != Some(seen) {
                off = off.min(left).max(right - room);
                ctx.data_mut(|d| d.insert_temp(sid.with("seen"), seen));
            }
            off = off.clamp(0.0, (total(cap) - room).max(0.0));
        } else {
            off = 0.0;
        }
        ctx.data_mut(|d| d.insert_temp(sid, off));
        let clip = Rect::from_min_max(pos2(bar.min.x - 10.0, bar.min.y - 5.0), pos2(bar.min.x + room, bar.max.y + 2.0));
        let pt = ui.painter().with_clip_rect(clip);
        let mut x = bar.min.x - off;
        for (i, t) in app.tabs.iter().enumerate() {
            let active = i == app.active;
            let meta = t.st().map(|s| fmt_int(s.len())).unwrap_or_default();
            let meta_g = pt.layout_no_wrap(meta, ui_font(11.0), if active { p.accent_2 } else { p.txt_dim });
            let dirty = t.dirty();
            let w = ws[i];
            let r = Rect::from_min_max(pos2(x, bottom - m.tab_h - 1.0), pos2(x + w, bottom + if active { 1.0 } else { 0.0 }));
            let resp = ui.interact(r.intersect(clip), Id::unique(("tab", t.uid)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let h = ease(&ctx, resp.id, resp.hovered() && !active);
            let fill = if active { p.surf } else { mix(p.surf_2, p.surf, h) };
            let cr = CornerRadius { nw: m.r_tab, ne: m.r_tab, sw: 0, se: 0 };
            pt.rect_filled(r, cr, fill);
            if active {
                // A breath of the accent at the top of the open tab, fading down.
                let g = Rect::from_min_max(r.min + vec2(1.0, 1.0), pos2(r.max.x - 1.0, r.min.y + r.height() * 0.7));
                let mut mesh = egui::Mesh::default();
                let top = alpha(p.prim, 0.10);
                mesh.colored_vertex(g.left_top(), top);
                mesh.colored_vertex(g.right_top(), top);
                mesh.colored_vertex(g.right_bottom(), Color32::TRANSPARENT);
                mesh.colored_vertex(g.left_bottom(), Color32::TRANSPARENT);
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                pt.with_clip_rect(r.shrink2(vec2(m.r_tab as f32 * 0.5, 0.0))).add(mesh);
            }
            let border = if active { p.prim } else { mix(p.brd_2, p.brd, h) };
            // Top, left and right borders only, with true rounded corners: the outline of the
            // whole rounded rectangle, its bottom edge cut off so the active tab opens into the toolbar.
            let outline = Rect::from_min_max(r.min, pos2(r.max.x, r.max.y + 4.0));
            pt.with_clip_rect(Rect::from_min_max(r.min - vec2(1.0, 1.0), pos2(r.max.x + 1.0, r.max.y)))
                .rect_stroke(outline, cr, Stroke::new(1.0, border), StrokeKind::Inside);
            let cy = r.center().y;
            let mut cx = r.min.x + 10.0;
            // State dot: loaded, or breathing while reading (1 s, as the web's pulse).
            let reading = t.table.is_none() && t.error.is_none();
            loading |= reading;
            let dot_c = pos2(cx + 3.5, cy);
            if reading {
                let k = 0.5 + 0.5 * (t_now * std::f64::consts::TAU).sin() as f32;
                pt.circle_filled(dot_c, 3.5 + 3.0 * (1.0 - k), alpha(p.prim, 0.18 * k));
                pt.circle_filled(dot_c, 3.5, alpha(p.prim, 0.25 + 0.75 * k));
            } else {
                pt.circle_filled(dot_c, 3.5, if t.error.is_some() { p.danger } else { p.ok });
            }
            cx += 7.0 + 8.0;
            let mut job = egui::text::LayoutJob::simple_singleline(t.name.clone(), ui_font(12.5), if active { p.txt_hi } else { p.txt_mut });
            job.wrap = egui::text::TextWrapping { max_width: cap, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let g = pt.layout_job(job);
            let gw = g.size().x;
            let name_c = if active { p.txt_hi } else { mix(p.txt_hi, p.txt_mut, h) };
            pt.galley_with_override_text_color(pos2(cx, snap(&ctx, cy - g.size().y / 2.0)), g, name_c);
            cx += gw + 8.0;
            let mw = meta_g.size().x;
            pt.galley(pos2(cx, snap(&ctx, cy - meta_g.size().y / 2.0)), meta_g, p.txt_dim);
            cx += mw;
            if dirty {
                pt.circle_filled(pos2(cx + 7.0, cy), 3.0, p.warn);
            }
            let xr = Rect::from_center_size(pos2(r.max.x - 8.0 - 8.0, cy), vec2(16.0, 16.0));
            let xresp = ui.interact(xr.intersect(clip), Id::unique(("tab-x", t.uid)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let xh = ease(&ctx, xresp.id, xresp.hovered());
            if xh > 0.0 {
                pt.rect_filled(xr, 4.0, p.danger.gamma_multiply(xh));
            }
            icons::cross(&pt, xr.shrink(4.0), 2.0, mix(p.on_danger, if active || h > 0.5 { p.txt_mut } else { p.txt_dim }, xh));
            if xresp.clicked() {
                close = Some(i);
            } else if resp.clicked() {
                pick = Some(i);
            }
            if resp.middle_clicked() {
                close = Some(i);
            }
            // The path, the size, and the edits not saved yet (as the web's).
            let mut tip = t.path.as_ref().map_or_else(|| format!("{} — not on disk yet", t.name), |p| p.display().to_string());
            if let Some(st) = t.st() {
                let size = st.base.size as f64;
                let sz = if size >= 1e9 { format!("{:.1} GB", size / 1e9) } else if size >= 1e6 { format!("{:.0} MB", size / 1e6) } else if size >= 1e3 { format!("{:.0} KB", size / 1e3) } else { format!("{size} B") };
                tip += &format!("\n{sz} · {} rows", fmt_int(st.len()));
            }
            if let Some(x) = t.table.as_ref().filter(|x| x.dirty()) {
                let n = x.pending_edits();
                tip += &format!("\n● {} pending edit{} — Ctrl+S saves", fmt_int(n), if n == 1 { "" } else { "s" });
            }
            resp.on_hover_text(tip);
            x = r.max.x + 4.0;
        }
        if loading {
            // The breathing dot: 30 frames a second is smooth enough (not every frame the screen can).
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
        let x = x.min(bar.min.x + room);
        if over {
            // ▾: every tab, by name.
            let lr = Rect::from_min_size(pos2(bar.max.x - 24.0, bottom - 24.0 - 4.0), vec2(24.0, 24.0));
            let lresp = ui.interact(lr, Id::unique("tab-list"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let h = ease(&ctx, lresp.id, lresp.hovered());
            if h > 0.0 { ui.painter().rect(lr, 5.0, p.surf_2.gamma_multiply(h), Stroke::new(1.0, p.brd.gamma_multiply(h)), StrokeKind::Inside); }
            icons::paint(ui.painter(), Icon::ChevronDown, lr.shrink(5.0), mix(p.txt_strong, p.txt_mut, h));
            let lresp = lresp.on_hover_text(format!("All {} tabs", app.tabs.len()));
            egui::Popup::menu(&lresp).frame(crate::theme::menu_frame(p, m)).show(|ui| {
                crate::widgets::width(ui, 260.0);
                for (i, t) in app.tabs.iter().enumerate() {
                    let label = if t.dirty() { format!("● {}", t.name) } else { t.name.clone() };
                    if crate::widgets::Item::new(&label).checked(i == app.active).show(ui, p).clicked() { pick = Some(i); ui.close(); }
                }
            });
        }
        let plus = Rect::from_min_size(pos2(x, bottom - 24.0 - 4.0), vec2(24.0, 24.0));
        let r = ui.interact(plus, Id::unique("tab-add"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = ease(&ctx, r.id, r.hovered());
        if h > 0.0 {
            ui.painter().rect(plus, 5.0, p.surf_2.gamma_multiply(h), Stroke::new(1.0, p.brd.gamma_multiply(h)), StrokeKind::Inside);
        }
        icons::plus(ui.painter(), plus.shrink(5.0), 2.0, mix(p.txt_strong, p.txt_mut, h));
        if r.on_hover_text("Open a file (Ctrl+O)").clicked() {
            open = true;
        }
    });
    if let Some(i) = pick {
        app.active = i;
    }
    if let Some(i) = close {
        app.close_tab(i, false);
    }
    if open {
        app.pick_files(ui.ctx());
    }
}

// ---------------------------------------------------------------- toolbar

/// An icon-only control over `r`; `draw` gets the hover share (0 → 1, eased).
fn icon_button_at(ui: &mut Ui, r: Rect, id: Id, tip: &str, draw: impl FnOnce(&egui::Painter, Rect, f32)) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    let h = ease(ui.ctx(), resp.id, resp.hovered());
    draw(ui.painter(), r, h);
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One button of a joined outline group: fill `surf_2` easing in on hover (`surf_3` pressed),
/// border `brd` → `brd_2`, or `color` when given (the Save group with pending edits).
fn seg(ui: &Ui, p: &Colors, r: Rect, resp: &egui::Response, cr: CornerRadius, color: Option<Color32>) {
    let h = ease(ui.ctx(), resp.id, resp.hovered());
    let fill = if resp.is_pointer_button_down_on() { p.surf_3 } else { p.surf_2.gamma_multiply(h) };
    let brd = color.unwrap_or(mix(p.brd_2, p.brd, h));
    ui.painter().rect(r, cr, fill, Stroke::new(1.0, brd), StrokeKind::Inside);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// When each filter chip first showed (by its label), for the pop.
#[derive(Clone, Default)]
struct Born(HashMap<String, f64>);

/// A filter in force, as its chip says it: (what, value, kind, column).
type Chip = (String, String, u8, usize);

/// A chip's width: 9 + what (ui 600 12) + 6 + value (ui 12) + 4 + × 18 + 3.
fn chip_w(pt: &egui::Painter, p: &Colors, c: &Chip) -> f32 {
    let v = chip_value(&c.1);
    let gw = pt.layout_no_wrap(c.0.clone(), semibold(12.0), p.txt_strong).size().x;
    let gv = if v.is_empty() { 0.0 } else { 6.0 + pt.layout_no_wrap(v, ui_font(12.0), p.txt_hi).size().x };
    (9.0 + gw + gv + 4.0 + 18.0 + 3.0).round()
}

fn chip_value(v: &str) -> String {
    if v.chars().count() > 28 { v.chars().take(28).collect::<String>() + "…" } else { v.to_string() }
}

/// The toolbar (look.md, "Toolbar"), laid out by hand so it holds at any width down to
/// ~700 px: the right group is measured first; the search field takes what is left after
/// the chips, shrinking to 120; then the chips give way (those that do not fit fold into a
/// "+N" chip listing them); then Save loses its label.
pub fn toolbar(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let (p, m) = (app.theme.p.clone(), app.m.clone());
    let mut refresh = false;
    let mut cmd: Option<&str> = None;
    let focus = std::mem::take(&mut app.focus_search);
    let mut menu_run: Option<fn(&mut tools::Cx)> = None;
    let mut dialog = None;
    let saving = app.saving();
    let frame = egui::Frame::new().fill(p.surf).inner_margin(egui::Margin::symmetric(m.toolbar_pad_x as i8, 8));
    egui::Panel::top("toolbar").exact_size(m.toolbar_h).frame(frame).show_separator_line(false).show(ui, |ui| {
        let bar = ui.max_rect();
        let bottom = bar.max.y + 8.0;
        let full = bar.x_range().expand(m.toolbar_pad_x);
        ui.painter().hline(full, ui.painter().round_to_pixel_center(bottom - 0.5), Stroke::new(1.0, p.brd));
        // The progress line: 2 px of `prim` with its glow along the bottom edge while a file
        // is read, a view computed or a file written.
        if let Some(tab) = app.tabs.get(app.active).filter(|t| t.busy() || saving) {
            let w = full.max - full.min;
            let t = ctx.input(|i| i.time) as f32;
            let (a, b) = match tab.load_progress() {
                Some(f) => (0.0, w * f.clamp(0.0, 1.0)),
                None => {
                    let x = (t * 0.9).fract() * (w + 240.0) - 240.0;
                    (x.max(0.0), (x + 240.0).min(w))
                }
            };
            let line = Rect::from_min_max(pos2(full.min + a, bottom - 2.0), pos2(full.min + b, bottom));
            ui.painter().add(egui::Shadow { offset: [0, 0], blur: 8, spread: 0, color: alpha(p.prim, 0.7) }.as_shape(line, 1));
            ui.painter().rect_filled(line, 1.0, p.prim);
            // A busy line: 30 frames a second (not every frame the screen can draw).
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
        let cy = bar.center().y;
        let gap = m.toolbar_gap;
        let at = |x: f32, w: f32, h: f32| Rect::from_min_size(pos2(x, (cy - h / 2.0).round()), vec2(w, h));

        // Menu button.
        let mr = at(bar.min.x, 38.0, 32.0);
        let resp = ui.interact(mr, Id::unique("tb-menu"), Sense::click());
        let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&resp));
        let h = ease(&ctx, resp.id, resp.hovered() || open);
        ui.painter().rect(mr, 6.0, mix(p.menu_bg_hover, p.menu_bg, h), Stroke::new(1.0, alpha(p.hi, 0.1)), StrokeKind::Inside);
        icons::hamburger(ui.painter(), Rect::from_center_size(mr.center(), vec2(18.0, 18.0)), p.menu_fg);
        if !app.tabs.is_empty() && app.tabs[app.active].table.is_some() && crate::shot::take("menu").is_some() {
            egui::Popup::open_id(&ctx, egui::Popup::default_response_id(&resp));
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&resp).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
            main_menu(ui, app, &p, &mut cmd, &mut menu_run, &mut dialog);
        });

        let Some(tab) = app.tabs.get_mut(app.active) else { return };
        let pt = ui.painter().clone();

        // The filters in force, as chips.
        let mut chips: Vec<Chip> = Vec::new();
        if !tab.filter.global.is_empty() {
            chips.push((if tab.expr_mode { "Expression".into() } else { "Search".into() }, tab.filter.global.clone(), 0, 0));
        }
        let heads = tab.heads();
        for (c, f) in tab.col_filters.iter().enumerate() {
            if !f.is_empty() {
                chips.push((heads.get(c).cloned().unwrap_or_default(), f.clone(), 1, c));
            }
        }
        // Values hidden from a column's panel, a chart's part or a cell's menu.
        for (c, set) in &tab.filter.hidden {
            if set.is_empty() { continue; }
            let what = match (set.len(), set.iter().next()) {
                (1, Some(v)) if v.is_empty() => "not empty".to_string(),
                (1, Some(v)) => format!("≠ {v}"),
                (n, _) => format!("{} values hidden", fmt_int(n)),
            };
            chips.push((heads.get(*c).cloned().unwrap_or_default(), what, 3, *c));
        }
        // Values kept: a chart's part, a cell's value.
        for pk in &tab.filter.picks {
            let what = match &pk.keep {
                csvfab_engine::view::Keeps::Values(v) if v.len() == 1 => match v.iter().next().map(String::as_str) { Some("") => "empty".to_string(), Some(v) => format!("= {v}"), None => String::new() },
                csvfab_engine::view::Keeps::Values(v) => format!("{} values", fmt_int(v.len())),
                csvfab_engine::view::Keeps::Part(..) => tab.spark.picks.get(&pk.col).map_or("a chart's part".to_string(), |p| p.label.clone()),
            };
            chips.push((heads.get(pk.col).cloned().unwrap_or_default(), what, 3, pk.col));
        }
        if let Some(r) = &tab.restrict {
            chips.push((r.label.clone(), String::new(), 2, 0));
        }
        // What each one keeps, said in full on hover when the chip cuts it short.
        // The search's options said too (as the web's chips): regex, case and accents, inverted.
        let f = &tab.filter;
        let opts: String = [(f.regex, "regular expression"), (f.match_case, "accents and case matched"), (f.invert, "inverted: the rows that do NOT match")].iter().filter(|o| o.0).map(|o| format!(" · {}", o.1)).collect();
        let tips: Vec<String> = chips.iter().map(|(what, value, kind, _)| match kind {
            0 if tab.expr_mode => format!("Rows where: {value}{}", if f.invert { " · inverted" } else { "" }),
            0 => format!("Rows holding: {value}{opts}"),
            1 => format!("Column \"{what}\" holds: {value}{opts}"),
            3 => format!("Column \"{what}\": {value} — set in its panel or a chart"),
            _ => format!("Only the rows: {what}"),
        }).collect();
        let widths: Vec<f32> = chips.iter().map(|c| chip_w(&pt, &p, c)).collect();
        let clear_w = if chips.len() >= 2 { 6.0 + 24.0 } else { 0.0 };
        let chips_need = if chips.is_empty() { 0.0 } else { widths.iter().sum::<f32>() + 6.0 * (chips.len() - 1) as f32 + clear_w };
        let more_chip_w = 52.0;

        // The right group: [Discard]  [Save n | ▾], with or without labels (Extract is in ▾).
        let dirty = tab.dirty();
        let n = tab.table.as_ref().map_or(0, |t| t.pending_edits());
        let badge = (dirty && n > 0).then(|| fmt_int(n));
        let label_w = |s: &str| pt.layout_no_wrap(s.to_string(), medium(13.0), p.txt).size().x;
        let bw = badge.as_ref().map_or(0.0, |b| pt.layout_no_wrap(b.clone(), semibold(11.0), p.on_warn).size().x + 12.0 + 8.0);
        let save_full = (36.0 + label_w("Save") + bw + 14.0).round();
        let discard_w = if dirty { 34.0 + 10.0 } else { 0.0 };
        let right_w = |compact: bool| discard_w + if compact { 34.0 + 30.0 - 1.0 } else { save_full + 30.0 - 1.0 };
        let field_pref = if tab.expr_mode { m.search_expr_w } else { m.search_w };
        let left_fixed = 38.0 + gap + 40.0 + gap + 118.0 + gap;
        // Width for the field and the chips, with the right group in full or compact.
        let split = |compact: bool| {
            let room = (bar.width() - left_fixed - right_w(compact) - gap).max(0.0);
            let field = (room - chips_need).clamp(120.0_f32.min(room), field_pref);
            (field, room - field)
        };
        let (mut field_w, mut chip_room) = split(false);
        let mut compact = false;
        if chip_room + 0.5 < chips_need && chip_room < more_chip_w + clear_w + 40.0 {
            compact = true;
            (field_w, chip_room) = split(true);
        }
        // The chips that fit (the others fold into "+N"); what they leave goes back to the field.
        let fits = if chips_need <= chip_room + 0.5 {
            chips.len()
        } else {
            let budget = chip_room - clear_w - more_chip_w - 6.0;
            let mut used = 0.0;
            widths.iter().take_while(|w| {
                used += *w + 6.0;
                used - 6.0 <= budget
            }).count()
        };
        if fits < chips.len() {
            let used = widths[..fits].iter().map(|w| w + 6.0).sum::<f32>() + more_chip_w + clear_w;
            field_w = (field_w + (chip_room - used).max(0.0)).min(field_pref);
        }
        let field_w = field_w.round();

        // Search group: ƒx | field.
        let mut x = mr.max.x + gap;
        let fr = at(x, 40.0, m.input_h);
        let sr = at(fr.max.x, field_w, m.input_h);
        let fx = ui.interact(fr, Id::unique("tb-fx"), Sense::click());
        let on = tab.expr_mode;
        let fh = ease(&ctx, fx.id, fx.hovered() || on);
        let id = Id::unique("global-search");
        let focused = ctx.memory(|mem| mem.has_focus(id));
        let ff = ease(&ctx, id.with("ring"), focused);
        let bad = tab.filter_error.is_some() && tab.filter_error_col.is_none();
        let sh = ease(&ctx, id.with("hover"), ui.rect_contains_pointer(sr));
        if ff > 0.0 {
            ui.painter().rect_stroke(Rect::from_min_max(fr.min, sr.max), 6.0, Stroke::new(4.0, p.focus_ring.gamma_multiply(ff)), StrokeKind::Outside);
        }
        ui.painter().rect(fr, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, mix(alpha(p.accent, 0.1), p.addon_bg, fh), Stroke::new(1.0, mix(p.prim, p.brd, fh.max(ff))), StrokeKind::Inside);
        icons::fx(ui.painter(), Rect::from_center_size(fr.center(), vec2(22.0, 19.0)), 1.8, mix(p.txt_hi, p.txt_mut, fh));
        if fx.on_hover_text("Insert a function: filter with a formula").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            menu_run = Some(crate::tools::fx_picker::open_search);
        }
        let brd = if bad { p.danger } else { mix(p.prim, mix(p.brd_2, p.brd, sh), ff) };
        ui.painter().rect(sr, CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, p.bg_deep, Stroke::new(1.0, brd), StrokeKind::Inside);
        if let Some(t) = crate::shot::take("search") {
            tab.filter.global = t;
            refresh = true;
            ctx.memory_mut(|mem| mem.request_focus(Id::unique("global-search")));
        }
        crate::tools::formula::search_keys(ui, tab);
        let font = if tab.expr_mode { FontId::monospace(13.0) } else { ui_font(14.0) };
        let te = egui::TextEdit::singleline(&mut tab.filter.global)
            .id(id)
            .font(font.clone())
            .text_color(p.txt_hi)
            .hint_text(RichText::new(if tab.expr_mode { "num({Amount}) > 1000" } else { "Filter..." }).font(font).color(p.txt_sub))
            .frame(egui::Frame::NONE);
        let r = crate::widgets::centered_edit(ui, sr.shrink2(vec2(12.0, 0.0)), te);
        crate::tools::formula::search_assist(ui, tab, &r, &p, &m);
        if focus {
            r.request_focus();
        }
        if r.changed() {
            refresh = true;
        }
        if let (Some(e), None) = (&tab.filter_error, tab.filter_error_col) {
            r.on_hover_text(e.clone());
        }
        x = sr.max.x + gap;

        // Toggles: ignore accents, regex, invert, expression (no transition: they switch at once).
        let tr = at(x, 118.0, 32.0);
        ui.painter().rect(tr, 7.0, p.bg_deep, Stroke::new(1.0, p.brd), StrokeKind::Inside);
        let toggles: [(&str, bool, Color32); 4] = [
            ("Match accents and case", tab.filter.match_case, p.accent_2),
            ("Regular expression", tab.filter.regex, p.accent_2),
            ("Invert: the rows that do NOT match", tab.filter.invert, p.orange_txt),
            ("Expression: num({Amount}) > 1000 && contains({City}, \"lyon\")", tab.expr_mode, p.accent_2),
        ];
        for (k, (tip, on, oncol)) in toggles.into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(tr.min.x + 3.0 + k as f32 * 28.0, tr.min.y + 2.0), vec2(26.0, 28.0));
            let resp = ui.interact(r, Id::unique(("tgl", k)), Sense::click());
            let c = if on { oncol } else if resp.hovered() { p.txt_hi } else { p.txt_dim };
            let g = Rect::from_center_size(r.center(), vec2(18.0, 18.0));
            match k {
                0 => icons::accent_e(ui.painter(), g, c, if on { p.danger_txt } else { p.tgl_acc_off }),
                1 => icons::regex(ui.painter(), g, 1.35, c),
                2 => icons::not_equal(ui.painter(), g, 1.35, c),
                _ => icons::paint(ui.painter(), Icon::Fx, Rect::from_center_size(r.center(), vec2(20.0, 17.0)), c),
            }
            if resp.has_focus() {
                ui.painter().rect_stroke(r, 4.0, Stroke::new(2.0, alpha(p.prim, 0.45)), StrokeKind::Inside);
            }
            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                match k {
                    0 => tab.filter.match_case = !tab.filter.match_case,
                    1 => tab.filter.regex = !tab.filter.regex,
                    2 => tab.filter.invert = !tab.filter.invert,
                    _ => tab.expr_mode = !tab.expr_mode,
                }
                refresh = true;
            }
        }
        x = tr.max.x + gap;

        // Filter chips, each popping in (scale .96 → 1, fade, 150 ms); those that do not fit
        // fold into "+N", which lists them.
        let now = ctx.input(|i| i.time);
        let born_id = Id::unique("chip-born");
        let mut born = ctx.data(|d| d.get_temp::<Born>(born_id)).unwrap_or_default();
        born.0.retain(|k, _| chips.iter().any(|c| format!("{}{}{}", c.2, c.3, c.0) == *k));
        let mut popping = false;
        let mut remove = None;
        for (k, (what, value, kind, c)) in chips.iter().enumerate().take(fits) {
            let t0 = *born.0.entry(format!("{kind}{c}{what}")).or_insert(now);
            let e = if crate::widgets::reduced_motion(&ctx) { 1.0 } else { egui::emath::easing::cubic_out((((now - t0) / 0.15) as f32).clamp(0.0, 1.0)) };
            popping |= e < 1.0;
            let v = chip_value(value);
            let cut = v != *value;
            let gw = pt.layout_no_wrap(what.clone(), semibold(12.0), p.txt_strong).size().x;
            let slot = at(x, widths[k], m.chip_h);
            x = slot.max.x + 6.0;
            let r = Rect::from_center_size(slot.center(), slot.size() * (0.96 + 0.04 * e));
            let fade = |col: Color32| col.gamma_multiply(e);
            // A test Invert flips: its chip outlined in the Invert toggle's orange.
            let flipped = tab.filter.invert && *kind <= 1;
            pt.rect(r, 13.0, fade(p.chip_bg), Stroke::new(1.0, fade(if flipped { p.orange_txt } else { p.chip_brd })), StrokeKind::Inside);
            pt.text(pos2(r.min.x + 9.0, r.center().y), Align2::LEFT_CENTER, what, semibold(12.0), fade(p.txt_strong));
            if !v.is_empty() {
                pt.text(pos2(r.min.x + 9.0 + gw + 6.0, r.center().y), Align2::LEFT_CENTER, v, ui_font(12.0), fade(p.txt_hi));
            }
            if cut || (*kind <= 1 && !opts.is_empty()) { let _ = ui.interact(r, Id::unique(("chip", k)), Sense::hover()).on_hover_text(&tips[k]); }
            let xr = Rect::from_center_size(pos2(r.max.x - 3.0 - 9.0, r.center().y), vec2(18.0, 18.0));
            let xresp = ui.interact(xr, Id::unique(("chip-x", k)), Sense::click());
            let xh = ease(&ctx, xresp.id, xresp.hovered());
            if xh > 0.0 {
                pt.circle_filled(xr.center(), 9.0, alpha(p.danger, 0.15 * xh));
            }
            icons::cross(&pt, xr.shrink(3.0), 2.0, fade(mix(p.danger_txt, p.txt_mut, xh)));
            if xresp.on_hover_text("Remove this filter").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                remove = Some((*kind, *c));
            }
        }
        if fits < chips.len() {
            let r = at(x, more_chip_w, m.chip_h);
            x = r.max.x + 6.0;
            let resp = ui.interact(r, Id::unique("chip-more"), Sense::click());
            let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&resp));
            let h = ease(&ctx, resp.id, resp.hovered() || open);
            pt.rect(r, 13.0, mix(alpha(p.accent, 0.18), p.chip_bg, h), Stroke::new(1.0, if open { p.prim } else { p.chip_brd }), StrokeKind::Inside);
            pt.text(r.center(), Align2::CENTER_CENTER, format!("+{}", chips.len() - fits), semibold(12.0), p.txt_strong);
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::Popup::menu(&resp).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                crate::widgets::width(ui, 260.0);
                crate::widgets::head(ui, &p, "Filters · click to remove");
                for (what, value, kind, c) in chips.iter().skip(fits) {
                    let label = if value.is_empty() { what.clone() } else { format!("{what}  {}", chip_value(value)) };
                    if crate::widgets::Item::new(&label).icon(Icon::Cross).show(ui, &p).clicked() {
                        remove = Some((*kind, *c));
                        ui.close();
                    }
                }
            });
        }
        ctx.data_mut(|d| d.insert_temp(born_id, born));
        if popping {
            ctx.request_repaint();
        }
        if chips.len() >= 2 {
            let r = at(x, 24.0, 24.0);
            let resp = icon_button_at(ui, r, Id::unique("chip-clear"), "Remove every filter", |pt, r, h| {
                icons::paint(pt, Icon::FunnelX, Rect::from_center_size(r.center(), vec2(16.0, 16.0)), mix(p.danger_txt, p.txt_mut, h));
            });
            if resp.clicked() {
                remove = Some((9, 0));
            }
        }
        if let Some((kind, c)) = remove {
            match kind {
                0 => tab.filter.global.clear(),
                1 => tab.col_filters[c].clear(),
                // Leaving the marked rows keeps their tints (the status bar's ◆ chip drops them).
                2 => tab.restrict = None,
                3 => {
                    tab.filter.hidden.retain(|(hc, _)| *hc != c);
                    tab.filter.picks.retain(|pk| pk.col != c);
                    tab.spark.picks.remove(&c);
                }
                _ => {
                    tab.filter.global.clear();
                    tab.col_filters.iter_mut().for_each(|s| s.clear());
                    tab.filter.hidden.clear();
                    tab.filter.picks.clear();
                    tab.spark.picks.clear();
                    tab.restrict = None;
                }
            }
            refresh = true;
        }

        // Right group: [Discard]  [Save n | ▾], joined (1 px over each other).
        let warn = if dirty { p.warn } else { p.txt };
        // The badge runs to its new count and pops when it changes.
        let bid = Id::unique(("save-badge", tab.uid));
        let shown_n = ctx.animate_value_with_time(bid, n as f32, if crate::widgets::reduced_motion(&ctx) { 0.0 } else { 0.35 });
        let shown_n = if (shown_n - n as f32).abs() < 0.5 { n } else { shown_n.round() as usize };
        let (last_n, changed_at) = ctx.data(|d| d.get_temp::<(usize, f64)>(bid)).unwrap_or((n, f64::NEG_INFINITY));
        let changed_at = if last_n != n { now } else { changed_at };
        ctx.data_mut(|d| d.insert_temp(bid, (n, changed_at)));
        let pop = (((now - changed_at) / 0.25) as f32).clamp(0.0, 1.0);
        if pop < 1.0 {
            ctx.request_repaint();
        }
        let save_w = if compact { 34.0 } else { save_full };
        let gx = bar.max.x - (save_w + 30.0 - 1.0);
        let save_r = at(gx, save_w, m.btn_h);
        let more_r = at(save_r.max.x - 1.0, 30.0, m.btn_h);
        let save = ui.interact(save_r, Id::unique("tb-save"), Sense::click());
        let more = ui.interact(more_r, Id::unique("tb-save-more"), Sense::click());
        let colored = dirty.then_some(p.warn);
        seg(ui, &p, save_r, &save, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, colored);
        seg(ui, &p, more_r, &more, CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, colored);
        let icon_at = |r: Rect| if compact { Rect::from_center_size(r.center(), vec2(16.0, 16.0)) } else { Rect::from_min_size(pos2(r.min.x + 14.0, r.center().y - 8.0), vec2(16.0, 16.0)) };
        icons::paint(&pt, Icon::Save, icon_at(save_r), warn);
        if !compact {
            pt.text(pos2(save_r.min.x + 36.0, save_r.center().y), Align2::LEFT_CENTER, "Save", medium(13.0), warn);
        }
        if let Some(b) = &badge {
            let s = 1.0 + 0.18 * (1.0 - egui::emath::easing::cubic_out(pop));
            let (font, h) = if compact { (semibold(9.5), 14.0) } else { (semibold(11.0), 18.0) };
            let gb = pt.layout_no_wrap(fmt_int(shown_n), font.clone(), p.on_warn);
            let full_w = (pt.layout_no_wrap(b.clone(), font, p.on_warn).size().x + if compact { 8.0 } else { 12.0 }).max(h);
            // Beside "Save"; compact, on the icon's top-right corner, ringed with the toolbar.
            let c = if compact { pos2(save_r.center().x + 7.0 + full_w / 2.0 - 4.0, save_r.min.y + 6.0) } else { pos2(save_r.min.x + 36.0 + label_w("Save") + 8.0 + full_w / 2.0, save_r.center().y) };
            let br = Rect::from_center_size(c, vec2(full_w, h) * s);
            if compact {
                pt.rect_filled(br.expand(1.5), h / 2.0 + 1.5, p.surf);
            }
            pt.rect_filled(br, h / 2.0 * s, p.warn);
            pt.galley(pos2(snap(&ctx, c.x - gb.size().x / 2.0), snap(&ctx, c.y - gb.size().y / 2.0)), gb, p.on_warn);
        }
        icons::paint(&pt, Icon::ChevronDownWide, Rect::from_center_size(more_r.center(), vec2(16.0, 12.0)), warn);
        egui::Popup::menu(&more).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
            crate::widgets::width(ui, 230.0);
            if crate::widgets::Item::new("Save as…").icon(Icon::Save).keys("Ctrl+Shift+S").show(ui, &p).clicked() {
                cmd = Some("save_as");
                ui.close();
            }
            for c in tools::commands().into_iter().filter(|c| c.group == "Save") {
                if command_item(c.name, c.keys.unwrap_or("")).enabled((c.enabled)(tab)).show(ui, &p).clicked() {
                    menu_run = Some(c.run);
                    ui.close();
                }
            }
        });
        let save_tip = match (&badge, compact) {
            (Some(b), _) => format!("Save {b} edit{} (Ctrl+S)", if n == 1 { "" } else { "s" }),
            _ => "Save (Ctrl+S)".to_string(),
        };
        // Labelled, it says it all; compact (the icon alone), the tip names it.
        let save = if compact { save.on_hover_text(save_tip) } else { save };
        if save.clicked() {
            cmd = Some("save");
        }
        if dirty {
            let r = at(gx - 10.0 - 34.0, 34.0, m.btn_h);
            let d = ui.interact(r, Id::unique("tb-discard"), Sense::click());
            let h = ease(&ctx, d.id, d.hovered());
            pt.rect(r, 6.0, alpha(p.danger, 0.12 * h), Stroke::new(1.0, mix(p.danger, alpha(p.danger, 0.5), h)), StrokeKind::Inside);
            icons::paint(&pt, Icon::Discard, Rect::from_center_size(r.center(), vec2(16.0, 16.0)), p.danger);
            if d.on_hover_text("Discard the edits: read the file again").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                cmd = Some("discard");
            }
        }
    });
    if let Some(d) = dialog {
        app.dialog = Some(d);
    }
    if refresh {
        if let Some(t) = app.tab() {
            t.refresh(&ctx);
        }
    }
    if let Some(run) = menu_run {
        app.run_command(run, &ctx);
    }
    match cmd {
        Some("open") => app.pick_files(&ctx),
        Some("save") => app.save(None, false, &ctx),
        Some("save_as") => app.save_as(&ctx),
        Some("undo") => app.undo(false, &ctx),
        Some("redo") => app.undo(true, &ctx),
        Some("palette") => app.palette = Some(Default::default()),
        Some("card") => app.card.toggle(),
        Some("quit") => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        Some("discard") => app.open_tool(tools::export::discard()),
        Some(band @ ("charts" | "filters" | "totals")) => {
            if let Some(t) = app.tabs.get_mut(app.active) {
                match band {
                    "charts" => t.show_charts = !t.show_charts,
                    "filters" => t.show_filters = !t.show_filters,
                    _ => t.show_totals = !t.show_totals,
                }
            }
        }
        _ => {}
    }
}

/// The web menu's icon for a tool command.
fn command_icon(name: &str) -> Option<Icon> {
    Some(match name {
        "Fill series down" => Icon::FillDown,
        "Show or hide columns…" => Icon::Columns,
        "Convert formats…" => Icon::Convert,
        "Look up from another tab…" => Icon::Link,
        "Merge columns…" => Icon::MergeColumns,
        "Split column…" => Icon::SplitColumn,
        "Computed column…" => Icon::Formula,
        "Find & replace" => Icon::Find,
        "Review changes…" => Icon::FileDiff,
        "Compare with another tab or file…" => Icon::Compare,
        "Delete hidden rows" => Icon::EyeOff,
        "Edit filtered rows…" => Icon::Edit,
        "Fill by example" => Icon::Sparkle,
        "Group by…" => Icon::GroupBy,
        "Pivot table…" => Icon::Pivot,
        "Mark duplicates…" | "Remove duplicates…" | "Show only the duplicate rows" => Icon::Duplicates,
        "Discard all edits and reload from disk" => Icon::Discard,
        "Extract the rows shown to a new file" => Icon::Extract,
        "Save as Excel workbook" | "Save as SQLite database" => Icon::File,
        "Anonymise…" => Icon::Anonymise,
        "Clean up…" => Icon::Clean,
        "Combine files…" => Icon::CombineFiles,
        "Remove noise words…" => Icon::Noise,
        "Split into files…" => Icon::SplitFiles,
        "File profile…" => Icon::Profile,
        "Insights" => Icon::Sparkle,
        "Map" => Icon::Map,
        "Insert a function (ƒx)" => Icon::Fx,
        "Create a schema…" => Icon::Schema,
        "Validate against a schema…" => Icon::Check,
        _ => return None,
    })
}

/// A menu item for a tool command, with its icon.
fn command_item<'a>(name: &'a str, keys: &'a str) -> crate::widgets::Item<'a> {
    let it = crate::widgets::Item::new(name).keys(keys);
    match command_icon(name) {
        Some(i) => it.icon(i),
        None => it,
    }
}

/// Leaving a tool's rows also drops its marks; the duplicates' have their own chip.
fn drop_marks(tab: &mut crate::tab::Tab) {
    let dups = tab.marks.dups.clone();
    tab.marks = std::sync::Arc::new(crate::tab::Marks { dups, ..Default::default() });
}

/// The main menu (docs/look.md, "Main menu"): the shell's own items, then the tools' by group.
fn main_menu(ui: &mut Ui, app: &App, p: &Colors, cmd: &mut Option<&str>, run: &mut Option<fn(&mut tools::Cx)>, dialog: &mut Option<Dialog>) {
    use crate::widgets::{sep, width, Item};
    width(ui, 300.0);
    let tab = app.tabs.get(app.active);
    let has = tab.is_some_and(|t| t.table.is_some());
    let table = tab.and_then(|t| t.table.as_ref());
    if Item::new("Command palette…").icon(Icon::Command).keys("Ctrl+P").show(ui, p).clicked() {
        *cmd = Some("palette");
        ui.close();
    }
    sep(ui, p);
    let all = tools::commands();
    let find = |name: &str| all.iter().find(|c| c.name == name);
    let ok = |name: &str| find(name).is_some_and(|c| tab.is_some_and(|t| (c.enabled)(t)));
    let mut go = |name: &str| {
        if let Some(c) = find(name) {
            *run = Some(c.run);
        }
    };
    if Item::new("Open").icon(Icon::Open).keys("Ctrl+O").show(ui, p).clicked() {
        *cmd = Some("open");
        ui.close();
    }
    match crate::widgets::row_buttons(ui, p, Some(Icon::Row), "Row", &[(Some(Icon::Goto), "Go to…", has), (Some(Icon::Card), "Card", has)]) {
        Some(0) => {
            *dialog = Some(Dialog::GoTo(String::new()));
            ui.close();
        }
        Some(_) => {
            *cmd = Some("card");
            ui.close();
        }
        None => {}
    }
    if Item::new("File profile…").icon(Icon::Profile).enabled(ok("File profile…")).show(ui, p).clicked() {
        go("File profile…");
        ui.close();
    }
    match crate::widgets::row_buttons(ui, p, Some(Icon::Magnifier), "Look", &[(Some(Icon::Map), "Map", ok("Map")), (Some(Icon::Sparkle), "Insights", ok("Insights"))]) {
        Some(0) => {
            go("Map");
            ui.close();
        }
        Some(_) => {
            go("Insights");
            ui.close();
        }
        None => {}
    }
    match crate::widgets::row_buttons(ui, p, Some(Icon::Schema), "Schema", &[(Some(Icon::FilePlus), "Create…", ok("Create a schema…")), (Some(Icon::Check), "Validate…", ok("Validate against a schema…"))]) {
        Some(0) => {
            go("Create a schema…");
            ui.close();
        }
        Some(_) => {
            go("Validate against a schema…");
            ui.close();
        }
        None => {}
    }
    sep(ui, p);
    let (u, r) = (table.is_some_and(|t| t.can_undo()), table.is_some_and(|t| t.can_redo()));
    match crate::widgets::row_buttons(ui, p, Some(Icon::History), "Edit", &[(Some(Icon::Undo), "Undo", u), (Some(Icon::Redo), "Redo", r)]) {
        Some(0) => {
            *cmd = Some("undo");
            ui.close();
        }
        Some(_) => {
            *cmd = Some("redo");
            ui.close();
        }
        None => {}
    }
    if Item::new("Review changes…").icon(Icon::FileDiff).enabled(ok("Review changes…")).show(ui, p).clicked() {
        go("Review changes…");
        ui.close();
    }
    sep(ui, p);
    if Item::new("Find & replace").icon(Icon::Find).keys("Ctrl+F").enabled(ok("Find & replace")).show(ui, p).clicked() {
        go("Find & replace");
        ui.close();
    }
    // The tools' groups, as submenus.
    for g in ["Transform the file", "Columns", "Rows"] {
        if g == "Columns" {
            sep(ui, p);
        }
        let icon = match g {
            "Columns" => Icon::Columns,
            "Rows" => Icon::Rows,
            _ => Icon::File,
        };
        let resp = Item::new(g).icon(icon).submenu().enabled(tab.is_some()).show(ui, p);
        egui::containers::menu::SubMenu::new().show(ui, &resp, |ui| {
            width(ui, 240.0);
            let mut any = false;
            for c in all.iter().filter(|c| c.group == g) {
                any = true;
                let enabled = tab.is_some_and(|t| (c.enabled)(t));
                if command_item(c.name, c.keys.unwrap_or("")).enabled(enabled).show(ui, p).clicked() {
                    *run = Some(c.run);
                    note_command(c.name);
                    ui.close();
                }
            }
            if !any {
                Item::new("Nothing here yet").enabled(false).show(ui, p);
            }
            // The bands, on and off (also in the status bar and the gutter).
            if g == "Columns" {
                if let Some(t) = tab {
                    sep(ui, p);
                    for (label, on, key) in [("Column charts", t.show_charts, "charts"), ("Filter row", t.show_filters, "filters"), ("Totals row", t.show_totals, "totals")] {
                        if Item::new(label).checked(on).show(ui, p).clicked() {
                            *cmd = Some(key);
                            ui.close();
                        }
                    }
                }
            }
        });
    }
    sep(ui, p);
    match crate::widgets::row_buttons(ui, p, Some(Icon::Help), "Help", &[(Some(Icon::Keyboard), "Shortcuts", true), (Some(Icon::Info), "About…", true)]) {
        Some(0) => {
            *dialog = Some(Dialog::Shortcuts);
            ui.close();
        }
        Some(_) => {
            *dialog = Some(Dialog::About);
            ui.close();
        }
        None => {}
    }
    if Item::new("Quit").icon(Icon::Quit).keys("Ctrl+Q").danger().show(ui, p).clicked() {
        *cmd = Some("quit");
        ui.close();
    }
}

// ---------------------------------------------------------------- status bar

/// A format pill: rounded, coloured by what it says, its fill easing up on hover,
/// a `prim` ring while its menu is open.
#[allow(clippy::too_many_arguments)]
fn pill(ui: &mut Ui, id: &str, cap: Option<&str>, text: &str, auto: bool, fg: Color32, bg: Color32, brd: Color32, h: f32) -> egui::Response {
    let font = medium(11.5);
    let tw = ui.painter().layout_no_wrap(text.to_string(), font.clone(), fg).size().x;
    let capw = cap.map_or(0.0, |c| ui.painter().layout_no_wrap(c.to_string(), FontId::monospace(12.0), fg).size().x.max(8.0) + 8.0 + 6.0);
    let aw = if auto { ui.painter().layout_no_wrap("AUTO".into(), semibold(9.5), fg).size().x + 6.0 + 2.4 } else { 0.0 };
    let (r, resp) = ui.allocate_exact_size(vec2((11.0 + capw + tw + aw + 11.0).round(), h), Sense::click());
    let ctx = ui.ctx().clone();
    let hot = ease(&ctx, resp.id, resp.hovered());
    let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&resp));
    let (ring, prim) = ui.ctx().data(|d| d.get_temp::<(Color32, Color32)>(Id::unique("pill-ring"))).unwrap_or((Color32::TRANSPARENT, fg));
    if open {
        ui.painter().rect_stroke(r, h / 2.0, Stroke::new(3.0, ring), StrokeKind::Outside);
    }
    ui.painter().rect(r, h / 2.0, mix(bg.gamma_multiply(1.8), bg, hot), Stroke::new(1.0, if open { prim } else { mix(brd.gamma_multiply(1.6), brd, hot) }), StrokeKind::Inside);
    let cy = r.center().y;
    let mut x = r.min.x + 11.0;
    if let Some(c) = cap {
        let cr = Rect::from_min_size(pos2(x, cy - 7.0), vec2(capw - 6.0, 14.0));
        ui.painter().rect_stroke(cr, 4.0, Stroke::new(1.0, alpha(fg, 0.45)), StrokeKind::Inside);
        ui.painter().text(cr.center(), Align2::CENTER_CENTER, c, FontId::new(12.0, egui::FontFamily::Name(crate::fonts::MONO_BOLD.into())), fg);
        x += capw;
    }
    ui.painter().text(pos2(x, cy), Align2::LEFT_CENTER, text, font, fg);
    x += tw + 6.0;
    if auto {
        let mut job = egui::text::LayoutJob::simple_singleline("AUTO".into(), semibold(9.5), alpha(fg, 0.6));
        job.sections[0].format.extra_letter_spacing = 0.57;
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(x, snap(&ctx, cy - g.size().y / 2.0)), g, alpha(fg, 0.6));
    }
    let _ = id;
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `n` running from its last value to the new one (350 ms); the first time a tab is seen,
/// from 0 (the counts fill up as the file opens).
fn counting(ctx: &egui::Context, id: Id, n: usize) -> usize {
    let dur = if crate::widgets::reduced_motion(ctx) { 0.0 } else { 0.35 };
    if ctx.data(|d| d.get_temp::<bool>(id.with("seen")).is_none()) {
        ctx.data_mut(|d| d.insert_temp(id.with("seen"), true));
        ctx.animate_value_with_time(id, 0.0, dur);
    }
    let v = ctx.animate_value_with_time(id, n as f32, dur);
    // f32 holds every count up to 2^24 exactly; past it the run ends on the exact value.
    if (v - n as f32).abs() < 0.5 { n } else { v.max(0.0).round() as usize }
}

/// How many columns the grid hides (the grid's hidden-columns set).
fn hidden_columns(tab: &crate::tab::Tab) -> usize {
    tab.grid.hidden.len()
}

/// A small busy ring: a 3/4 arc of `prim` turning, 14 px.
fn spinner(ui: &mut Ui, p: &Colors) {
    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let c = r.center();
    ui.painter().circle_stroke(c, 5.5, Stroke::new(1.6, alpha(p.prim, 0.18)));
    let a0 = t * 5.5;
    let pts: Vec<egui::Pos2> = (0..=20).map(|k| {
        let a = a0 + k as f32 / 20.0 * std::f32::consts::PI * 1.4;
        c + vec2(a.cos(), a.sin()) * 5.5
    }).collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.6, p.prim)));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// The colour level switch, as its level: `ALL` the palette's hues around the disc from the
/// top, clockwise, each blended into the next (`.sb-kcol`'s conic gradient); `NO_TYPES` the disc
/// halved, grey and blue; `RAW` grey, "raw" across it in white. Ringed in each case.
fn look_switch(pt: &egui::Painter, r: Rect, level: u8, p: &Colors) {
    let (c, rad) = (r.center(), r.width() / 2.0);
    match level {
        crate::theme::ALL => {
            let hues = [p.danger_txt, p.orange_txt, p.warn, p.ok_txt, p.cyan_txt, p.prim, p.violet_txt, p.pink_txt];
            let at = |t: f32| {
                let f = t * hues.len() as f32;
                let (i, u) = (f.floor() as usize % hues.len(), f.fract());
                mix(hues[(i + 1) % hues.len()], hues[i], u)
            };
            // Wedges, each with its own centre in its middle hue: the hues meet at the centre unblurred.
            const STEPS: usize = 64;
            let rim = |t: f32| { let a = std::f32::consts::TAU * t - std::f32::consts::FRAC_PI_2; c + vec2(a.cos(), a.sin()) * rad };
            let mut mesh = egui::Mesh::default();
            for k in 0..STEPS {
                let (t0, t1) = (k as f32 / STEPS as f32, (k + 1) as f32 / STEPS as f32);
                let n = mesh.vertices.len() as u32;
                mesh.colored_vertex(c, at((t0 + t1) / 2.0));
                mesh.colored_vertex(rim(t0), at(t0));
                mesh.colored_vertex(rim(t1), at(t1));
                mesh.add_triangle(n, n + 1, n + 2);
            }
            pt.add(egui::Shape::mesh(mesh));
        }
        crate::theme::NO_TYPES => {
            pt.circle_filled(c, rad, p.gray_hi);
            // The right half, blue: a half disc from the top round to the bottom.
            let mut pts: Vec<egui::Pos2> = (0..=24).map(|k| {
                let a = std::f32::consts::PI * (-0.5 + k as f32 / 24.0);
                c + vec2(a.cos(), a.sin()) * rad
            }).collect();
            pts.push(c);
            pt.add(egui::Shape::convex_polygon(pts, p.prim, Stroke::NONE));
        }
        _ => {
            pt.circle_filled(c, rad, p.gray);
            pt.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "raw", semibold(r.width() * 0.4), Color32::WHITE);
        }
    }
    pt.circle_stroke(c, rad, Stroke::new(1.0, alpha(p.hi, 0.3)));
}

pub fn status_bar(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let (p, m) = (app.theme.p.clone(), app.m.clone());
    let saving = app.saving();
    let mut theme_pick: Option<&'static str> = None;
    // Another encoding or delimiter picked on a tab with unsaved edits: asked first.
    let mut reread_ask: Option<(crate::tab::Choice<Enc>, crate::tab::Choice<u8>)> = None;
    let mut open_clean = false;
    let mut follow_toggle = false;
    let frame = egui::Frame::new().inner_margin(egui::Margin { left: 18, right: 12, top: 0, bottom: 0 });
    egui::Panel::bottom("status").exact_size(m.status_h).frame(frame).show_separator_line(false).show(ui, |ui| {
        let r = ui.max_rect().expand2(vec2(18.0, 0.0));
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(r.left_top(), p.status_a);
        mesh.colored_vertex(r.right_top(), p.status_a);
        mesh.colored_vertex(r.right_bottom(), p.status_b);
        mesh.colored_vertex(r.left_bottom(), p.status_b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter().hline(r.x_range(), ui.painter().round_to_pixel_center(r.min.y + 0.5), Stroke::new(1.0, p.status_brd));
        // The pills' open ring, in this theme's colours.
        ctx.data_mut(|d| d.insert_temp(Id::unique("pill-ring"), (p.focus_ring_sm, p.prim)));
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.style_mut().override_font_id = Some(ui_font(12.0));
            // The theme switch is there with or without a file.
            let theme_switch = |ui: &mut Ui, theme_pick: &mut Option<&'static str>| {
                let (tr, tresp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&tresp));
                let h = ease(&ctx, tresp.id, tresp.hovered());
                let c = tr.center();
                let rad = 9.0 * (1.0 + 0.1 * h);
                if open {
                    ui.painter().circle_stroke(c, rad + 2.5, Stroke::new(4.0, alpha(p.prim, 0.25)));
                }
                ui.painter().circle_filled(c, rad, p.bg);
                let pts: Vec<egui::Pos2> = (0..=24).map(|k| {
                    let a = std::f32::consts::PI * (-0.25 + k as f32 / 24.0);
                    c + vec2(a.cos(), a.sin()) * rad
                }).collect();
                ui.painter().add(egui::Shape::convex_polygon(pts, app.theme.swatch, Stroke::NONE));
                ui.painter().circle_stroke(c, rad, Stroke::new(1.0, if open { p.prim } else { alpha(p.hi, 0.3 + 0.2 * h) }));
                let tresp = tresp.on_hover_text("Theme").on_hover_cursor(egui::CursorIcon::PointingHand);
                egui::Popup::menu(&tresp).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                    crate::widgets::width(ui, 210.0);
                    crate::widgets::head(ui, &p, "Theme");
                    for th in crate::theme::themes() {
                        if crate::widgets::Item::new(th.name).checked(th.id == app.theme.id).show(ui, &p).clicked() {
                            *theme_pick = Some(th.id);
                            ui.close();
                        }
                    }
                });
            };
            let Some(tab) = app.tabs.get_mut(app.active) else {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    theme_switch(ui, &mut theme_pick);
                    ui.add_space(12.0);
                    // How the next file is read, chosen before opening it (as the web's).
                    let gray = (p.txt_mut, alpha(p.hi, 0.04), alpha(p.hi, 0.15));
                    let o = &mut app.next_opts;
                    let elabel = o.enc.map_or("Encoding".to_string(), |e| e.label().to_string());
                    let er = pill(ui, "next-enc", None, &elabel, o.enc.is_none(), gray.0, gray.1, gray.2, m.sb_pill_h).on_hover_text("How the next file opened is read");
                    egui::Popup::menu(&er).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                        crate::widgets::width(ui, 210.0);
                        crate::widgets::head(ui, &p, "Read the next file as");
                        if crate::widgets::Item::new("Auto (detected)").checked(o.enc.is_none()).show(ui, &p).clicked() { o.enc = None; ui.close(); }
                        for e in Enc::ALL {
                            if crate::widgets::Item::new(e.label()).checked(o.enc == Some(e)).show(ui, &p).clicked() { o.enc = Some(e); ui.close(); }
                        }
                    });
                    // Line 1 of the next file: detected, titles, or data.
                    let nh = &mut app.next_header;
                    let hlabel = match nh { None => "Line 1", Some(true) => "Titles on line 1", Some(false) => "No titles" };
                    let hr = pill(ui, "next-head", None, hlabel, nh.is_none(), gray.0, gray.1, gray.2, m.sb_pill_h).on_hover_text("How line 1 of the next file opened is read");
                    egui::Popup::menu(&hr).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                        crate::widgets::width(ui, 240.0);
                        crate::widgets::head(ui, &p, "Line 1 of the next file");
                        for (v, l) in [(None, "Auto (detected)"), (Some(true), "Titles on line 1"), (Some(false), "No titles (columns numbered)")] {
                            if crate::widgets::Item::new(l).checked(*nh == v).show(ui, &p).clicked() { *nh = v; ui.close(); }
                        }
                    });
                    let o = &mut app.next_opts;
                    let dlabel = match o.delim { None => "Delimiter", Some(b',') => "Comma ,", Some(b';') => "Semicolon ;", Some(b'\t') => "Tab ⇥", Some(b'|') => "Pipe |", Some(0) => "Raw text ¶", _ => "Other" };
                    let dr = pill(ui, "next-delim", None, dlabel, o.delim.is_none(), gray.0, gray.1, gray.2, m.sb_pill_h).on_hover_text("How the next file opened is split");
                    egui::Popup::menu(&dr).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                        crate::widgets::width(ui, 210.0);
                        crate::widgets::head(ui, &p, "Split the next file on");
                        if crate::widgets::Item::new("Auto (detected)").checked(o.delim.is_none()).show(ui, &p).clicked() { o.delim = None; ui.close(); }
                        for (b, l) in [(b',', "Comma ,"), (b';', "Semicolon ;"), (b'\t', "Tab ⇥"), (b'|', "Pipe |"), (0u8, "Raw text ¶")] {
                            if crate::widgets::Item::new(l).checked(o.delim == Some(b)).show(ui, &p).clicked() { o.delim = Some(b); ui.close(); }
                        }
                    });
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Ready.").color(p.status_fg));
                    });
                });
                return;
            };
            let Some(t) = &tab.table else {
                let line = match &tab.error { Some(e) => format!("Cannot read {} ({e})", tab.name), None => format!("Reading {}…", tab.name) };
                ui.label(RichText::new(line).color(if tab.error.is_some() { p.danger_txt } else { p.status_fg }));
                return;
            };
            let st = t.st.clone();
            // Message: the file, the selection and what its numbers add up to.
            let mut msg = egui::text::LayoutJob::default();
            let tf = |c: Color32, f: FontId| egui::TextFormat::simple(f, c);
            msg.append(&tab.name, 0.0, tf(p.status_fg, ui_font(12.0)));
            let sepc = alpha(p.status_fg, 0.45);
            // Raw text: Ln / Col as an editor says it (tabs to the next multiple of 4).
            if st.base.dialect.lines {
                let line_of = |id: RowId| id as usize + 1;
                let pos = if let Some(e) = &tab.grid.editing {
                    let caret = egui::TextEdit::load_state(&ctx, Id::unique("cell-editor")).and_then(|s| s.cursor.char_range());
                    caret.map(|r| {
                        let (pi, si): (usize, usize) = (r.primary.index.into(), r.secondary.index.into());
                        let (a, b) = (pi.min(si), pi.max(si));
                        let before: String = e.text.chars().take(a).collect();
                        let extra = before.matches('\n').count();
                        let col = before.rsplit('\n').next().unwrap_or("").chars().fold(0, |c, ch| if ch == '\t' { (c / 4 + 1) * 4 } else { c + 1 });
                        format!("Ln {}, Col {}{}", fmt_int(line_of(e.id) + extra), fmt_int(col + 1), if b > a { format!(" ({} selected)", fmt_int(b - a)) } else { String::new() })
                    })
                } else {
                    tab.grid.range().filter(|(r, _)| *r.start() < tab.shown()).map(|(r, _)| {
                        let n = r.end() - r.start() + 1;
                        format!("Ln {}, Col 1{}", fmt_int(line_of(tab.shown_id(*r.start()))), if n > 1 { format!(" ({} lines selected)", fmt_int(n)) } else { String::new() })
                    })
                };
                if let Some(pos) = pos {
                    msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                    msg.append(&pos, 0.0, tf(p.txt_hi, ui_font(12.0)));
                }
            }
            // A message for this tab (Undo's, Redo's): 8 s, in place of the selection's numbers.
            let now = ctx.input(|i| i.time);
            let line = app.status.as_ref().filter(|(u, _, at)| *u == tab.uid && now - at < 8.0).map(|(_, s, at)| (s.clone(), *at));
            if let Some((s, at)) = line {
                msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                msg.append(&s, 0.0, tf(p.txt_hi, ui_font(12.0)));
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((at + 8.0 - now).max(0.0)));
            } else if tab.grid.editing.is_some() {
                // Typing in a cell: what the keys do (as the web's hint).
                let n = tab.grid.range().map_or(1, |(r, c)| (r.end() - r.start() + 1) * (c.end() - c.start() + 1));
                let hint = if n > 1 {
                    format!("Enter: this value in the {} selected cells · Ctrl+Enter: a series · Shift+Enter: a line break · Esc: cancel", fmt_int(n))
                } else {
                    if st.base.dialect.lines { "Enter: keep · Shift+Enter: a line break · Tab: a tab · Esc: cancel" } else { "Enter: keep · Shift+Enter: a line break · Esc: cancel" }.to_string()
                };
                msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                msg.append(&hint, 0.0, tf(p.status_fg, ui_font(12.0)));
            } else if let Some((rows, cols)) = tab.grid.range() {
                // The columns shown only (hidden ones are not copied, not summed).
                let (nr, nc) = (rows.end() - rows.start() + 1, cols.filter(|c| !tab.grid.hidden.contains(c)).count());
                if nr * nc > 1 {
                    msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                    msg.append(&format!("{} × {}", fmt_int(nr), fmt_int(nc)), 0.0, tf(p.txt_hi, semibold(12.0)));
                    msg.append(" selected", 0.0, tf(p.status_fg, ui_font(12.0)));
                    // Worked out in the background (a million cells at most), as the web's.
                    if let Some((sum, n)) = tab.selection_numbers(&ctx) {
                        {
                            for (k, v) in [("Sum", csvfab_engine::stats::fmt_num(sum, 0, 2, true)), ("Avg", csvfab_engine::stats::fmt_num(sum / n as f64, 0, 2, true))] {
                                msg.append("  ·  ", 0.0, tf(sepc, ui_font(12.0)));
                                msg.append(&format!("{k} "), 0.0, tf(p.txt_dim, ui_font(12.0)));
                                msg.append(&v, 0.0, tf(p.num_txt, semibold(12.0)));
                            }
                            msg.append("  ·  ", 0.0, tf(sepc, ui_font(12.0)));
                            msg.append(&format!("{} numbers", fmt_int(n)), 0.0, tf(p.status_fg, ui_font(12.0)));
                        }
                    }
                }
            } else if let Some(e) = &tab.filter_error {
                // A filter that does not compile: the rows of the last one that did stay; said.
                msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                msg.append(&format!("Filter not applied — {e}"), 0.0, tf(p.danger_txt, ui_font(12.0)));
            } else if tab.narrowed() {
                msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                msg.append("filtered", 0.0, tf(p.stat_filt, ui_font(12.0)));
            } else if !tab.sort.is_empty() {
                msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                msg.append("sorted", 0.0, tf(p.stat_filt, ui_font(12.0)));
            }
            // The expression filter's rows in error: hidden, and said (as the web's).
            if let Some((n, first)) = tab.expr_errors.lock().ok().and_then(|e| e.clone()).filter(|_| tab.expr_mode) {
                msg.append("  |  ", 0.0, tf(sepc, ui_font(12.0)));
                msg.append(&format!("{} {} hidden ({first})", fmt_int(n), if n > 1 { "rows raise an error and are" } else { "row raises an error and is" }), 0.0, tf(p.warn, ui_font(12.0)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Switches, right to left: theme, type colours, charts, filter row, totals, spaced
                // as the web's (#sb-fmt gap 8 + margin 6; 4 before the theme); sizes as its icons.
                ui.spacing_mut().item_spacing.x = 0.0;
                theme_switch(ui, &mut theme_pick);
                ui.add_space(12.0);
                let switch = |ui: &mut Ui, size: f32, grow: f32, on: bool, tip: &str, draw: &dyn Fn(&egui::Painter, Rect, Color32)| -> bool {
                    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
                    let h = ease(&ctx, resp.id, resp.hovered());
                    let r2 = Rect::from_center_size(r.center(), r.size() * (1.0 + grow * h));
                    draw(ui.painter(), r2, if on { Color32::WHITE } else { alpha(p.gray_hi, 0.45 + 0.25 * h) });
                    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                };
                // The colour level, three steps a click goes through: every colour (a wheel of the
                // palette's hues), no type colours (a disc half grey, half blue), raw values (a grey
                // disc reading "raw").
                let lv = crate::theme::look();
                let tip = match lv {
                    crate::theme::ALL => "All colours · click: no type colours",
                    crate::theme::NO_TYPES => "No type colours · click: raw values",
                    _ => "Raw values: no colours, pills, links nor scales · click: all colours",
                };
                if switch(ui, 18.0, 0.1, true, tip, &|pt, r, _| look_switch(pt, r, lv, &p)) {
                    let next = if lv == crate::theme::RAW { crate::theme::ALL } else { lv - 1 };
                    crate::theme::set_look(next);
                    app.config.look = Some(next);
                    app.config.save();
                }
                ui.add_space(14.0);
                if switch(ui, 16.0, 0.12, tab.show_charts, "Column charts", &|pt, r, c| icons::bars(pt, r, if c == Color32::WHITE { p.cyan_txt } else { c })) {
                    tab.show_charts = !tab.show_charts;
                }
                ui.add_space(14.0);
                if switch(ui, 15.0, 0.12, tab.show_filters, "Filter row", &|pt, r, c| icons::funnel(pt, r, true, if c == Color32::WHITE { p.accent_2 } else { c })) {
                    tab.show_filters = !tab.show_filters;
                }
                ui.add_space(14.0);
                if switch(ui, 15.0, 0.12, tab.show_totals, "Totals row", &|pt, r, c| icons::sigma(pt, r, if c == Color32::WHITE { p.orange_txt } else { c })) {
                    tab.show_totals = !tab.show_totals;
                }
                ui.add_space(14.0);

                // Format pills (right to left): line breaks, titles, encoding, delimiter.
                ui.spacing_mut().item_spacing.x = 8.0;
                let gray = (p.txt_mut, alpha(p.gray, 0.12), alpha(p.gray, 0.25));
                // Line endings: the file's, or the one chosen for the next save; a click switches
                // LF ↔ CRLF, an undoable edit written at the next save.
                let cur = st.eol.unwrap_or(st.base.eol);
                let eol = match cur {
                    b"\r\n" => "CRLF",
                    b"\r" => "CR",
                    _ => "LF",
                };
                let next: &'static [u8] = if cur == b"\r\n" { b"\n" } else { b"\r\n" };
                let er = pill(ui, "eol", None, eol, false, gray.0, gray.1, gray.2, m.sb_pill_h)
                    .on_hover_text(format!("Line endings · click: switch to {}", if next == b"\n" { "LF" } else { "CRLF" }));
                let toggle_eol = er.clicked().then_some(next);
                let head = if st.header { "Titles on line 1" } else { "No titles" };
                let detected = csvfab_engine::table::detected_header(&st.base);
                // Auto, titles, numbered: as the web's menu (a text file has no titles: no pill).
                let mut set_header: Option<bool> = None;
                if !st.base.dialect.lines {
                let hr = pill(ui, "head", None, head, st.header == detected, gray.0, gray.1, gray.2, m.sb_pill_h);
                egui::Popup::menu(&hr).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                    crate::widgets::width(ui, 240.0);
                    crate::widgets::head(ui, &p, "Line 1");
                    let auto = format!("Auto (detected: {})", if detected { "titles" } else { "data" });
                    if crate::widgets::Item::new(&auto).checked(st.header == detected).show(ui, &p).clicked() { set_header = Some(detected); ui.close(); }
                    if crate::widgets::Item::new("Titles on line 1").checked(st.header).show(ui, &p).clicked() { set_header = Some(true); ui.close(); }
                    if crate::widgets::Item::new("No titles (columns numbered)").checked(!st.header).show(ui, &p).clicked() { set_header = Some(false); ui.close(); }
                });
                }
                let toggle_header = set_header.is_some_and(|h| h != st.header);
                let enc = st.base.enc;
                let (ef, eb) = match enc {
                    Enc::Utf8 => (p.ok_txt, p.ok_rgb),
                    Enc::Windows1252 | Enc::Latin1 | Enc::Latin9 => (p.warn, p.warn_rgb),
                    Enc::MacRoman => (p.cyan_txt, p.cyan),
                    Enc::Utf16Le | Enc::Utf16Be => (p.accent_2, p.accent),
                };
                let etext = if st.base.bom && enc == Enc::Utf8 { "UTF-8 BOM".to_string() } else { enc.label().to_string() };
                let er = pill(ui, "enc", None, &etext, tab.open_opts.enc.is_none(), ef, alpha(eb, 0.13), alpha(eb, 0.4), m.sb_pill_h);
                let mut reopen: (crate::tab::Choice<Enc>, crate::tab::Choice<u8>) = (None, None);
                let enc_auto = tab.open_opts.enc.is_none();
                egui::Popup::menu(&er).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                    crate::widgets::width(ui, 210.0);
                    crate::widgets::head(ui, &p, "Read as");
                    // What was detected, named (as the web's "Auto (UTF-8 BOM)") while it is in force.
                    let auto_l = if enc_auto { format!("Auto ({etext})") } else { "Auto (detected)".to_string() };
                    if crate::widgets::Item::new(&auto_l).checked(enc_auto).show(ui, &p).clicked() {
                        if !enc_auto { reopen.0 = Some(None); }
                        ui.close();
                    }
                    for e in Enc::ALL {
                        if crate::widgets::Item::new(e.label()).checked(!enc_auto && e == enc).show(ui, &p).clicked() {
                            reopen.0 = Some(Some(e));
                            ui.close();
                        }
                    }
                });
                let d = if st.base.dialect.lines { 0 } else { st.base.dialect.delim };
                let (cap, name, fg, bg) = match d {
                    b',' => (",", "Comma", p.cyan_txt, p.cyan),
                    b';' => (";", "Semicolon", p.violet_txt, p.violet),
                    b'\t' => ("⇥", "Tab", p.orange_txt, p.orange),
                    0 => ("¶", "Raw text", p.ok_txt, p.ok_rgb),
                    b'|' => ("|", "Pipe", p.pink_txt, p.pink),
                    _ => ("?", "Other", p.pink_txt, p.pink),
                };
                // Raw text says its language (as the web's "Raw text · Python").
                let lang = crate::syntax::lang_of(&tab.name).label;
                let shown = if d == 0 && lang != "Text" { format!("{name} · {lang}") } else { name.to_string() };
                let dr = pill(ui, "delim", Some(cap), &shown, tab.open_opts.delim.is_none(), fg, alpha(bg, 0.13), alpha(bg, 0.4), m.sb_pill_h);
                egui::Popup::menu(&dr).frame(crate::theme::menu_frame(&p, &m)).show(|ui| {
                    crate::widgets::width(ui, 210.0);
                    crate::widgets::head(ui, &p, "Split on");
                    let auto = tab.open_opts.delim.is_none();
                    let auto_l = if auto { format!("Auto ({})", name.to_lowercase()) } else { "Auto (detected)".to_string() };
                    if crate::widgets::Item::new(&auto_l).checked(auto).show(ui, &p).clicked() {
                        if !auto { reopen.1 = Some(None); }
                        ui.close();
                    }
                    for (b, l) in [(b',', "Comma ,"), (b';', "Semicolon ;"), (b'\t', "Tab ⇥"), (b'|', "Pipe |"), (0u8, "Raw text ¶")] {
                        if crate::widgets::Item::new(l).checked(!auto && b == d).show(ui, &p).clicked() {
                            reopen.1 = Some(Some(b));
                            ui.close();
                        }
                    }
                });
                ui.spacing_mut().item_spacing.x = 14.0;
                let (sr, _) = ui.allocate_exact_size(vec2(5.0, 14.0), Sense::hover());
                ui.painter().vline(ui.painter().round_to_pixel_center(sr.center().x), sr.y_range(), Stroke::new(1.0, p.status_sep));

                // Counts, as a dashboard: the figures in ui 600, running to their new values;
                // under a filter, a slim gauge of the share shown.
                let (shown, total) = (tab.shown(), st.len());
                let total_a = counting(&ctx, Id::unique(("sb-total", tab.uid)), total);
                let shown_a = counting(&ctx, Id::unique(("sb-shown", tab.uid)), shown);
                let mut job = egui::text::LayoutJob::default();
                let part = tab.filtered() && shown != total;
                if part {
                    job.append(&fmt_int(shown_a), 0.0, tf(p.stat_filt, semibold(12.0)));
                    job.append(" / ", 0.0, tf(sepc, ui_font(12.0)));
                }
                job.append(&fmt_int(total_a), 0.0, tf(p.stat_total, semibold(12.0)));
                job.append(" rows  ·  ", 0.0, tf(p.status_fg, ui_font(12.0)));
                job.append(&st.ncols.to_string(), 0.0, tf(p.txt_hi, semibold(12.0)));
                job.append(" columns", 0.0, tf(p.status_fg, ui_font(12.0)));
                let hidden = hidden_columns(tab);
                if hidden > 0 {
                    job.append(&format!(" ({} hidden)", fmt_int(hidden)), 0.0, tf(p.txt_dim, ui_font(12.0)));
                }
                ui.label(job).on_hover_text(format!(
                    "{} · read in {:.0} ms{} · index {} · edits {}",
                    fmt_size(st.base.size),
                    st.base.load_time.as_secs_f64() * 1e3,
                    tab.view_time.map_or(String::new(), |v| format!(" · view in {:.0} ms", v.as_secs_f64() * 1e3)),
                    fmt_size(st.base.heap_bytes() as u64),
                    fmt_size(st.edit_bytes() as u64),
                ));
                if part && total > 0 {
                    let share = ctx.animate_value_with_time(Id::unique(("sb-share", tab.uid)), shown as f32 / total as f32, 0.35);
                    let (g, _) = ui.allocate_exact_size(vec2(36.0, 4.0), Sense::hover());
                    ui.painter().rect_filled(g, 2.0, alpha(p.stat_filt, 0.18));
                    ui.painter().rect_filled(Rect::from_min_size(g.min, vec2((g.width() * share).max(2.0), g.height())), 2.0, p.stat_filt);
                }
                if let Some(c) = tab.grid.cursor {
                    let mut job = egui::text::LayoutJob::default();
                    job.append("Row ", 0.0, tf(p.txt_dim, ui_font(12.0)));
                    job.append(&fmt_int(c.row + 1), 0.0, tf(p.status_fg, medium(12.0)));
                    job.append("  Col ", 0.0, tf(p.txt_dim, ui_font(12.0)));
                    job.append(&(c.col + 1).to_string(), 0.0, tf(p.status_fg, medium(12.0)));
                    ui.label(job);
                }
                if saving || tab.busy() {
                    spinner(ui, &p);
                }

                // Chips: irregular rows, quote errors.
                // Quote errors alone are said too (as the web's): an unclosed quote may swallow
                // the rest of the file into one field.
                if !st.base.odd.is_empty() || st.base.qerr > 0 {
                    let n = st.base.odd.len();
                    let on = tab.restrict.as_ref().is_some_and(|r| r.label.starts_with("Irregular"));
                    let q = st.base.qerr as usize;
                    let qs = format!("{} quote error{}", fmt_int(q), if q > 1 { "s" } else { "" });
                    let text = match (n, q) {
                        (0, _) => qs,
                        (_, 0) => format!("{} irregular row{}", fmt_int(n), if n > 1 { "s" } else { "" }),
                        _ => format!("{} irregular row{} · {qs}", fmt_int(n), if n > 1 { "s" } else { "" }),
                    };
                    let tip = if n == 0 { "A quote opened in a field is not closed where it should be: what follows it, up to the next quote, is read as one field." } else { "Rows whose field count differs from the titles'. Click to show only them." };
                    let r = pill(ui, "irr", None, &text, false, if on { p.on_warn } else { p.warn }, if on { p.warn } else { alpha(p.warn_rgb, 0.08) }, alpha(p.warn_rgb, 0.45), 20.0)
                        .on_hover_text(tip);
                    if r.clicked() && n > 0 {
                        if on {
                            tab.restrict = None;
                            tab.refresh(&ctx);
                        } else {
                            let h = st.header as RowId;
                            let ids: Vec<RowId> = st.base.odd.iter().map(|&(r, _)| r).filter(|&r| r >= h).collect();
                            tab.restrict_to("Irregular rows", ids, &ctx);
                        }
                    }
                }
                // A log (raw text): following it is offered here, as the web's pill.
                if crate::tools::tail::state(tab.uid).is_none() && st.base.dialect.lines && tab.path.is_some() {
                    if pill(ui, "follow-offer", None, "Follow", false, p.txt_mut, alpha(p.hi, 0.04), alpha(p.hi, 0.15), 20.0).on_hover_text("Follow the end of the file (tail -f): new lines as they are written").clicked() {
                        follow_toggle = true;
                    }
                }
                if let Some(f) = crate::tools::tail::state(tab.uid) {
                    let (text, fg, bg) = match f {
                        crate::tools::tail::Follow::On => ("Following", p.ok_txt, p.ok_rgb),
                        crate::tools::tail::Follow::Paused => ("Following: paused", p.warn, p.warn_rgb),
                        _ => ("Following: error", p.danger_txt, p.danger),
                    };
                    if pill(ui, "follow", None, text, false, fg, alpha(bg, 0.12), alpha(bg, 0.4), 20.0).clicked() {
                        follow_toggle = true;
                    }
                }
                // Garbled accents: a chip, a click opens Clean up (as the web's).
                if tab.garbled(&ctx) {
                    let r = pill(ui, "moji", None, "⚠ Garbled accents", false, p.warn, alpha(p.warn_rgb, 0.12), alpha(p.warn_rgb, 0.4), 20.0).on_hover_text("Ã© for é: click to repair (Clean up)");
                    if r.clicked() { open_clean = true; }
                }
                // Rows a tool marked (review, compare, schema, Insights): only them or all rows,
                // the tints kept; right-click drops the marks (as the web's ◆ chip).
                let marked = tab.marks.rows.len();
                if marked > 0 {
                    let label = if tab.marks_label.is_empty() { "marked rows".to_string() } else { tab.marks_label.clone() };
                    // Their view: named after them (with their count, from Insights).
                    let only = tab.restrict.as_ref().is_some_and(|r| r.label.eq_ignore_ascii_case(&label) || r.label.ends_with(&format!(" {label}")));
                    let text = format!("◆ {} {label}", fmt_int(marked));
                    let r = pill(ui, "marks", None, &text, false, if only { p.txt_strong } else { p.accent_2 }, if only { alpha(p.prim, 0.3) } else { alpha(p.prim, 0.08) }, alpha(p.prim, 0.45), 20.0)
                        .on_hover_text(format!("Click: {} · right-click: remove the marks", if only { "every row again, the tints kept" } else { "only these rows" }));
                    if r.clicked() {
                        if only {
                            tab.restrict = None;
                            tab.refresh(&ctx);
                        } else if let Some(st) = tab.st().cloned() {
                            let ids: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).filter(|id| tab.marks.rows.contains(id)).collect();
                            let l = if tab.marks_label.is_empty() { "Marked rows".to_string() } else { tab.marks_label.clone() };
                            tab.restrict_to(l, ids, &ctx);
                        }
                    }
                    if r.secondary_clicked() {
                        if only { tab.restrict = None; }
                        drop_marks(tab);
                        tab.refresh(&ctx);
                    }
                }
                if let Some((text, on, tip)) = crate::tools::dupes::chip(tab) {
                    let r = pill(ui, "dups", None, &text, false, if on { p.txt_strong } else { p.violet_txt }, if on { alpha(p.violet, 0.3) } else { alpha(p.violet, 0.08) }, alpha(p.violet, 0.45), 20.0).on_hover_text(tip);
                    if r.clicked() {
                        crate::tools::dupes::toggle_view(tab, &ctx);
                    }
                    if r.secondary_clicked() {
                        crate::tools::dupes::clear(tab, &ctx);
                    }
                }
                if toggle_header {
                    if let Some(t) = &mut tab.table {
                        t.set_header(!st.header);
                    }
                    tab.after_edit(true, &ctx);
                }
                if let Some(e) = toggle_eol {
                    if let Some(t) = &mut tab.table { t.set_eol(e); }
                    tab.after_edit(false, &ctx);
                    tab.grid.notice = Some(format!("Line endings set to {} — written at the next Save.", if e == b"\r\n" { "CRLF" } else { "LF" }));
                }
                if reopen.0.is_some() || reopen.1.is_some() {
                    if tab.dirty() { reread_ask = Some(reopen); } else { tab.reopen_with(reopen.0, reopen.1, &ctx); }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(msg).truncate());
                });
            });
        });
    });
    if let Some(id) = theme_pick {
        app.set_theme(id, &ctx);
    }
    if open_clean {
        if let Some(c) = tools::commands().into_iter().find(|c| c.name == "Clean up…") { app.run_command(c.run, &ctx); }
    }
    if let Some((enc, delim)) = reread_ask {
        app.dialog = Some(Dialog::Reread { tab: app.active, enc, delim });
    }
    if follow_toggle {
        app.run_command(
            |cx| {
                if let Some(t) = crate::tools::tail::toggle(cx) {
                    cx.out.push(tools::Cmd::Tool(t))
                }
            },
            &ctx,
        );
    }
}

// ---------------------------------------------------------------- command palette

#[derive(Default)]
pub struct Launcher {
    query: String,
    sel: usize,
    /// The selection moved by the keyboard: bring it into view once.
    reveal: bool,
}

enum Entry {
    Command(fn(&mut tools::Cx)),
    Shell(&'static str),
    /// Read the file again with this encoding or delimiter (asked first over pending edits).
    Reread(crate::tab::Choice<Enc>, crate::tab::Choice<u8>),
    Tab(usize),
    Recent(std::path::PathBuf),
    /// A column command: (what, column).
    Col(ColAct, usize),
    /// With no file open: how the next one is read (delimiter, encoding, line 1; None: detected).
    Next(Option<Option<u8>>, Option<Option<Enc>>, Option<Option<bool>>),
}

#[derive(Clone, Copy)]
enum ColAct { Go, Asc, Desc, ThenAsc, ThenDesc, Panel, Group, Pivot, Split }

/// The commands run from the palette or the menus, most recent first: shown first in the
/// palette and ranked up when typing. Kept in the state directory (not in screenshots).
fn recent_commands() -> &'static std::sync::Mutex<Vec<String>> {
    static R: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let v = if std::env::var_os("CSVFAB_SHOT").is_some() {
            Vec::new()
        } else {
            std::fs::read_to_string(crate::config::dir().join("palette-recent")).map(|s| s.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()).unwrap_or_default()
        };
        std::sync::Mutex::new(v)
    })
}

/// Remembers `name` as the latest command used.
fn note_command(name: &str) {
    let Ok(mut r) = recent_commands().lock() else { return };
    r.retain(|x| x != name);
    r.insert(0, name.to_string());
    r.truncate(8);
    if std::env::var_os("CSVFAB_SHOT").is_none() {
        let _ = std::fs::create_dir_all(crate::config::dir());
        let _ = std::fs::write(crate::config::dir().join("palette-recent"), r.join("\n"));
    }
}

fn fold_char(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        csvfab_engine::text::fold(c.encode_utf8(&mut [0; 4])).chars().next().unwrap_or(c)
    }
}

/// Fuzzy match: each word of `query` (already folded) must appear in `text`, its letters in
/// order (accents and case ignored). Returns a score — word starts, runs of adjacent letters
/// and an early start count up, gaps down — and the matched characters' indices.
pub fn fuzzy(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let t: Vec<char> = text.chars().map(fold_char).collect();
    let mut score = 0;
    let mut hits = Vec::new();
    for w in query.split_whitespace() {
        let w: Vec<char> = w.chars().collect();
        let mut best: Option<(i32, Vec<usize>)> = None;
        for start in (0..t.len()).filter(|&i| t[i] == w[0]) {
            let mut idx = vec![start];
            let mut i = start + 1;
            for &c in &w[1..] {
                // Prefer the next letter right after, then at a word start, then anywhere.
                let next = (i..t.len()).find(|&j| t[j] == c);
                let Some(mut j) = next else { break };
                if j != i {
                    if let Some(b) = (j..t.len()).find(|&k| t[k] == c && (k == 0 || !t[k - 1].is_alphanumeric())) {
                        j = b;
                    }
                }
                idx.push(j);
                i = j + 1;
            }
            if idx.len() < w.len() {
                continue;
            }
            let mut s = if start == 0 { 10 } else { 0 };
            for (k, &i) in idx.iter().enumerate() {
                if i == 0 || !t[i - 1].is_alphanumeric() {
                    s += 8;
                }
                if k > 0 {
                    let gap = i - idx[k - 1] - 1;
                    s += if gap == 0 { 6 } else { -(gap.min(6) as i32) };
                }
            }
            if best.as_ref().is_none_or(|b| s > b.0) {
                best = Some((s, idx));
            }
        }
        let (s, idx) = best?;
        score += s;
        hits.extend(idx);
    }
    hits.sort_unstable();
    hits.dedup();
    Some((score - t.len() as i32 / 8, hits))
}

/// What a command does, in the palette's words (the web's labels): searched with its name.
fn about(name: &str) -> Option<&'static str> {
    Some(match name {
        "Map" => "the file map: the whole file as one picture",
        "Insights" => "what I noticed: keys, values written otherwise, outliers, copies, hidden rules",
        "File profile…" => "every column at a glance",
        "Clean up…" => "spaces, invisible characters, garbled accents, empty rows…",
        "Remove noise words…" => "a list of words taken out of a column or every column",
        "Anonymise…" => "names, e-mails, phones, postal codes",
        "Group by…" => "count, sum, average per value, in a new tab",
        "Pivot table…" => "rows × columns, counts or sums, colour scale, click to see the rows",
        "Split into files…" => "one per value of a column, in a new folder",
        "Combine files…" => "open tabs into one new file, columns matched by title",
        "Fill by example" => "type one or two results, the rest of the column follows",
        "Compare with another tab or file…" => "changed rows, rows only on one side",
        "Convert formats…" => "dates, numbers, phone numbers",
        "Freeze the first column" => "it stays in view when scrolling sideways",
        "Review changes…" => "what Save would write",
        "Why does this row look wrong?" => "its bytes examined",
        "Create a schema…" => "the rules this file follows, as name.schema.json",
        "Validate against a schema…" => "check this file against its rules",
        "Insert a function (ƒx)" => "every helper of formulas, with examples",
        "Computed column…" => "a formula over the other columns",
        "Mark duplicates…" => "rows written twice, even differently",
        "Look up from another tab…" => "a column brought in from another table by a key",
        "Follow the end of the file (tail -f)" => "new lines as they are written",
        "Save as Excel workbook" => "an .xlsx beside the file",
        "Save as SQLite database" => "one table, typed columns",
        "Delete hidden rows" => "the rows the filters hide go from the file",
        _ => return None,
    })
}

pub fn palette(app: &mut App, ctx: &egui::Context) {
    let Some(l) = &mut app.palette else { return };
    let p = &app.theme.p;
    // Every entry: shell actions, tools, tabs, recent files.
    let mut entries: Vec<(String, &str, Option<&str>, Option<Icon>, Entry)> = vec![
        ("Open a file…".into(), "File", Some("Ctrl+O"), Some(Icon::Open), Entry::Shell("open")),
        ("Save".into(), "File", Some("Ctrl+S"), Some(Icon::Save), Entry::Shell("save")),
        ("Save as…".into(), "File", Some("Ctrl+Shift+S"), Some(Icon::Save), Entry::Shell("save_as")),
        ("Go to a row…".into(), "Rows", Some("Ctrl+G"), Some(Icon::Goto), Entry::Shell("goto")),
        ("Row card".into(), "Rows", Some("Ctrl+I"), Some(Icon::Card), Entry::Shell("card")),
        ("Undo".into(), "Edit", Some("Ctrl+Z"), Some(Icon::Undo), Entry::Shell("undo")),
        ("Redo".into(), "Edit", Some("Ctrl+Y"), Some(Icon::Redo), Entry::Shell("redo")),
        ("Keyboard shortcuts".into(), "Help", Some("F1"), Some(Icon::Keyboard), Entry::Shell("keys")),
        (format!("Monospace font…{}", app.config.mono_font.as_deref().map(|f| format!(" — {}", if f == "monospace" { "system monospace" } else { f })).unwrap_or_default()), "Look", None, Some(Icon::FontCheck), Entry::Shell("font")),
        ("About csvfab… — version, licence, website".into(), "Help", None, Some(Icon::Info), Entry::Shell("about")),
        ("What's new — the features of each version".into(), "Help", None, Some(Icon::Sparkles), Entry::Shell("news")),
    ];
    let tab = app.tabs.get(app.active);
    for c in tools::commands() {
        if tab.is_some_and(|t| (c.enabled)(t)) {
            // With what it does, as the web's palette said it: searched too ("noticed", "accents").
            let label = match about(c.name) { Some(a) => format!("{} — {a}", c.name), None => c.name.to_string() };
            entries.push((label, c.group, c.keys, command_icon(c.name), Entry::Command(c.run)));
        }
    }
    for th in crate::theme::themes() {
        entries.push((format!("Theme: {}", th.name), "Look", None, Some(Icon::ColorScale), Entry::Shell(th.id)));
    }
    // The shell's own, as the web's palette lists them.
    let tk = |on: bool| on.then_some("✓");
    entries.push(("Quit csvfab".into(), "File", Some("Ctrl+Q"), Some(Icon::Quit), Entry::Shell("quit")));
    if let Some(t) = tab {
        entries.push(("Close this tab".into(), "File", Some("Ctrl+W"), Some(Icon::Cross), Entry::Shell("close-tab")));
        entries.push((if t.show_charts { "Hide the column charts" } else { "Column charts — each column's values drawn under its title" }.into(), "View", tk(t.show_charts), Some(Icon::Chart), Entry::Shell("charts")));
        entries.push((if t.show_filters { "Hide the filter row — the boxes under the titles" } else { "Show the filter row — the boxes under the titles" }.into(), "View", tk(t.show_filters), Some(Icon::Funnel), Entry::Shell("filters")));
        entries.push((if t.show_totals { "Hide the totals row" } else { "Totals row — a sum, average, count… under each column" }.into(), "View", tk(t.show_totals), Some(Icon::Sigma), Entry::Shell("totals")));
    }
    if let Some(t) = tab.filter(|t| t.table.is_some()) {
        // Undo names the edit it would take back (when a toast named it).
        if let Some(w) = t.table.as_ref().filter(|x| x.can_undo()).and_then(|x| crate::app::edit_what::get((t.uid, x.version()))) {
            if let Some(e) = entries.iter_mut().find(|e| e.0 == "Undo") { e.0 = format!("Undo — {w}"); }
        }
        if let Some(w) = t.table.as_ref().and_then(|x| x.redo_version()).and_then(|v| crate::app::edit_what::get((t.uid, v))) {
            if let Some(e) = entries.iter_mut().find(|e| e.0 == "Redo") { e.0 = format!("Redo — {w}"); }
        }
        if t.show_totals {
            entries.push(("Totals: sum every number column".into(), "Columns", None, Some(Icon::Sigma), Entry::Shell("totals-sum")));
            entries.push(("Totals: clear every total".into(), "Columns", None, Some(Icon::Sigma), Entry::Shell("totals-clear")));
        }
        entries.push((if t.expr_mode { "Filter by text again (expression off)" } else { "Filter by expression — {Amount}.num() > 1000 && …" }.into(), "View", None, Some(Icon::Fx), Entry::Shell("expr")));
        entries.push(("Select all rows shown".into(), "Edit", Some("Ctrl+A"), Some(Icon::Rows), Entry::Shell("select-all")));
        entries.push(("Find — step through the matches without filtering".into(), "Edit", Some("Ctrl+F"), Some(Icon::Find), Entry::Shell("find")));
        entries.push(("Raw line — the selected row as it sits in the file".into(), "View", None, Some(Icon::Code), Entry::Shell("raw")));
        if t.st().is_some_and(|st| !st.base.odd.is_empty()) {
            let on = t.restrict.as_ref().is_some_and(|r| r.label.starts_with("Irregular"));
            entries.push((if on { "Show all rows again" } else { "Show only irregular rows" }.into(), "Rows", None, Some(Icon::Warning), Entry::Shell("irregular")));
        }
        if let Some(r) = &t.restrict {
            entries.push((format!("Show all rows again (leave the {})", r.label.to_lowercase()), "Rows", None, Some(Icon::Rows), Entry::Shell("unrestrict")));
        }
        if !t.marks.rows.is_empty() {
            // The marked rows' view, left: entered again (as the web's "Show only the …").
            if t.restrict.is_none() {
                let what = if t.marks_label.is_empty() { "marked rows".to_string() } else { t.marks_label.clone() };
                entries.push((format!("Show only the {}", what.to_lowercase()), "Rows", None, Some(Icon::Rows), Entry::Shell("marked")));
            }
            entries.push(("Remove the marks".into(), "Rows", None, Some(Icon::Cross), Entry::Shell("unmark")));
        }
        // Per column, as the web's: go, sort, then sort, its panel, group, pivot, split.
        let heads = t.st().map(|st| st.headers()).unwrap_or_default();
        for (c, h) in heads.iter().enumerate() {
            let h = if h.trim().is_empty() { format!("Column {}", c + 1) } else { h.clone() };
            let sorted = t.sort.iter().any(|k| k.col == c);
            entries.push((format!("Go to column: {h}"), "Column", None, Some(Icon::Goto), Entry::Col(ColAct::Go, c)));
            entries.push((format!("Sort ascending: {h}"), "Column", None, Some(Icon::ArrowUp), Entry::Col(ColAct::Asc, c)));
            entries.push((format!("Sort descending: {h}"), "Column", None, Some(Icon::ChevronDown), Entry::Col(ColAct::Desc, c)));
            if !t.sort.is_empty() && !sorted {
                entries.push((format!("Then sort ascending: {h}"), "Column", None, Some(Icon::ArrowUp), Entry::Col(ColAct::ThenAsc, c)));
                entries.push((format!("Then sort descending: {h}"), "Column", None, Some(Icon::ChevronDown), Entry::Col(ColAct::ThenDesc, c)));
            }
            entries.push((format!("Profile & filter by value: {h}"), "Column", None, Some(Icon::Profile), Entry::Col(ColAct::Panel, c)));
            entries.push((format!("Group by: {h}"), "Column", None, Some(Icon::GroupBy), Entry::Col(ColAct::Group, c)));
            entries.push((format!("Pivot table by: {h}"), "Column", None, Some(Icon::Pivot), Entry::Col(ColAct::Pivot, c)));
            entries.push((format!("Split into files by: {h}"), "Column", None, Some(Icon::SplitFiles), Entry::Col(ColAct::Split, c)));
        }
    }
    // No file open: the reading choices are for the next one (as the web's palette).
    if tab.and_then(|t| t.st()).is_none() {
        let tick = |on: bool| on.then_some("✓");
        let (o, h) = (app.next_opts, app.next_header);
        entries.push(("Delimiter: Auto (detected)".into(), "Reading", tick(o.delim.is_none()), Some(Icon::Split), Entry::Next(Some(None), None, None)));
        for (b, l) in [(b',', "Comma ,"), (b';', "Semicolon ;"), (b'\t', "Tab ⇥"), (b'|', "Pipe |"), (0u8, "Raw text ¶")] {
            entries.push((format!("Delimiter: {l}"), "Reading", tick(o.delim == Some(b)), Some(Icon::Split), Entry::Next(Some(Some(b)), None, None)));
        }
        entries.push(("Encoding: Auto (detected)".into(), "Reading", tick(o.enc.is_none()), Some(Icon::FontCheck), Entry::Next(None, Some(None), None)));
        for e in Enc::ALL {
            entries.push((format!("Encoding: {}", e.label()), "Reading", tick(o.enc == Some(e)), Some(Icon::FontCheck), Entry::Next(None, Some(Some(e)), None)));
        }
        for (v, l) in [(None, "Auto (detected)"), (Some(true), "the column titles"), (Some(false), "data, not titles")] {
            entries.push((format!("Line 1: {l}"), "Reading", tick(h == v), Some(Icon::Rows), Entry::Next(None, None, Some(v))));
        }
    }
    // Reading: what the status bar's pills choose, by name.
    if let Some(st) = tab.and_then(|t| t.st()) {
        let tick = |on: bool| on.then_some("✓");
        let d = if st.base.dialect.lines { 0 } else { st.base.dialect.delim };
        let (enc_auto, delim_auto) = tab.map_or((true, true), |t| (t.open_opts.enc.is_none(), t.open_opts.delim.is_none()));
        entries.push(("Delimiter: Auto (detected)".into(), "Reading", tick(delim_auto), Some(Icon::Split), Entry::Reread(None, Some(None))));
        entries.push(("Encoding: Auto (detected)".into(), "Reading", tick(enc_auto), Some(Icon::FontCheck), Entry::Reread(Some(None), None)));
        let detected = csvfab_engine::table::detected_header(&st.base);
        entries.push((format!("Line 1: Auto (detected: {})", if detected { "titles" } else { "data" }), "Reading", tick(st.header == detected), Some(Icon::Rows), Entry::Shell("header-auto")));
        for (b, l) in [(b',', "Comma ,"), (b';', "Semicolon ;"), (b'\t', "Tab ⇥"), (b'|', "Pipe |"), (0u8, "Raw text ¶")] {
            entries.push((format!("Delimiter: {l}"), "Reading", tick(tab.is_some_and(|t| t.open_opts.delim.is_some()) && b == d), Some(Icon::Split), Entry::Reread(None, Some(Some(b)))));
        }
        for e in Enc::ALL {
            entries.push((format!("Encoding: {}", e.label()), "Reading", tick(tab.is_some_and(|t| t.open_opts.enc.is_some()) && e == st.base.enc), Some(Icon::FontCheck), Entry::Reread(Some(Some(e)), None)));
        }
        entries.push(((if st.header { "Line 1: data, not titles" } else { "Line 1: the column titles" }).into(), "Reading", None, Some(Icon::Rows), Entry::Shell("header")));
        let eol = st.eol.unwrap_or(st.base.eol);
        entries.push((format!("Line endings: switch to {}", if eol == b"\r\n" { "LF" } else { "CRLF" }), "Reading", None, Some(Icon::Convert), Entry::Shell("eol")));
    }
    for (i, t) in app.tabs.iter().enumerate() {
        entries.push((t.name.clone(), "Tab", None, Some(Icon::FileText), Entry::Tab(i)));
    }
    for r in app.config.recent.iter() {
        entries.push((r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), "Recent", None, Some(Icon::History), Entry::Recent(r.clone())));
    }
    let recent = recent_commands().lock().map(|r| r.clone()).unwrap_or_default();
    let rank = |name: &str| recent.iter().position(|r| r == name);
    // Rows: (entry, matched chars) — with no query, the recent commands first under their
    // own head, then everything; with one, the best matches first.
    let q = csvfab_engine::text::fold(l.query.trim());
    let mut shown: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut n_recent = 0;
    if q.is_empty() {
        let mut rec: Vec<usize> = (0..entries.len()).filter(|&i| matches!(entries[i].4, Entry::Command(_) | Entry::Shell(_)) && rank(&entries[i].0).is_some()).collect();
        rec.sort_by_key(|&i| rank(&entries[i].0));
        n_recent = rec.len();
        shown.extend(rec.iter().map(|&i| (i, Vec::new())));
        // The per-column commands wait for a query (a wide file would list hundreds).
        shown.extend((0..entries.len()).filter(|i| !rec.contains(i) && entries[*i].1 != "Column").map(|i| (i, Vec::new())));
    } else {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = entries.iter().enumerate().filter_map(|(i, e)| {
            let bonus = rank(&e.0).map_or(0, |r| 24 - 2 * r as i32);
            // A recent file is found by its folder too.
            let dir = || match &e.4 { Entry::Recent(p) => p.parent().map(|d| d.display().to_string()), _ => None };
            fuzzy(&q, &e.0).map(|(s, h)| (s + bonus, i, h)).or_else(|| fuzzy(&q, e.1).map(|(s, _)| (s - 20 + bonus, i, Vec::new())))
                .or_else(|| dir().and_then(|d| fuzzy(&q, &d)).map(|(s, _)| (s - 30 + bonus, i, Vec::new())))
        }).collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        shown.extend(scored.into_iter().map(|(_, i, h)| (i, h)));
    }
    shown.truncate(80);
    l.sel = l.sel.min(shown.len().saturating_sub(1));
    let mut pick = None;
    let mut close = false;
    let dark = crate::widgets::is_dark(p);
    let frame = egui::Frame::new()
        .fill(p.surf)
        .stroke(Stroke::new(1.0, p.brd_2))
        .corner_radius(14)
        .inner_margin(0)
        .shadow(egui::Shadow { offset: [0, 28], blur: 70, spread: 0, color: Color32::from_black_alpha(if dark { 153 } else { 56 }) });
    let top = (ctx.content_rect().height() * 0.11).round();
    let modal = crate::widgets::modal_at(ctx, Id::unique("palette"), p, frame, Some(top), |ui| {
        ui.set_width(660.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // The input row, 52 tall.
        let (row, _) = ui.allocate_exact_size(vec2(660.0, 52.0), Sense::hover());
        let typing = !l.query.is_empty();
        icons::magnifier(ui.painter(), Rect::from_center_size(pos2(row.min.x + 16.0 + 9.0, row.center().y), vec2(18.0, 18.0)), 2.0, if typing { p.accent_2 } else { p.txt_dim });
        let esc = key_cap(ui.painter(), p, row.max.x - 16.0, row.center().y, "Esc");
        let te = egui::TextEdit::singleline(&mut l.query)
            .id(Id::unique("palette-query"))
            .font(ui_font(16.0))
            .text_color(p.txt_hi)
            .frame(egui::Frame::NONE)
            .hint_text(RichText::new("Search an action, a tab, a recent file…").font(ui_font(16.0)).color(p.txt_dim));
        let field = Rect::from_min_max(pos2(row.min.x + 50.0, row.min.y), pos2(esc.min.x - 12.0, row.max.y));
        let r = crate::widgets::centered_edit(ui, field, te);
        r.request_focus();
        if r.changed() {
            l.sel = 0;
            l.reveal = true;
        }
        ui.painter().hline(row.x_range(), ui.painter().round_to_pixel_center(row.max.y - 0.5), Stroke::new(1.0, p.brd));
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                l.sel = (l.sel + 1).min(shown.len().saturating_sub(1));
                l.reveal = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                l.sel = l.sel.saturating_sub(1);
                l.reveal = true;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                pick = shown.get(l.sel).map(|s| s.0);
            }
            // Escape, or the keys that open it (Ctrl+P, Ctrl+Shift+P) again.
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::P) || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::P) {
                close = true;
            }
        });
        const ROW: f32 = 36.0;
        const HEAD: f32 = 28.0;
        egui::ScrollArea::vertical().max_height(480.0).auto_shrink([false, true]).id_salt("palette-list").show(ui, |ui| {
            egui::Frame::new().inner_margin(6).show(ui, |ui| {
                ui.set_width(648.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                if shown.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(648.0, 84.0), Sense::hover());
                    icons::magnifier(ui.painter(), Rect::from_center_size(pos2(r.center().x, r.min.y + 28.0), vec2(22.0, 22.0)), 2.0, p.txt_faint);
                    ui.painter().text(pos2(r.center().x, r.min.y + 58.0), Align2::CENTER_CENTER, format!("Nothing matches “{}”", l.query.trim()), ui_font(13.0), p.txt_dim);
                    return;
                }
                // The selection's highlight glides between rows (90 ms): painted under them.
                let under = ui.painter().add(egui::Shape::Noop);
                let origin = ui.cursor().min;
                let mut sel_y = 0.0;
                let mut y = 0.0;
                for (k, (i, hits)) in shown.iter().enumerate() {
                    let head = if q.is_empty() && n_recent > 0 && k == 0 {
                        Some("Recently used")
                    } else if q.is_empty() && n_recent > 0 && k == n_recent {
                        Some("Everything")
                    } else {
                        None
                    };
                    if let Some(h) = head {
                        crate::widgets::head(ui, p, h);
                        y += HEAD - 3.0;
                    }
                    let (name, group, keys, icon, _) = &entries[*i];
                    let (r, resp) = ui.allocate_exact_size(vec2(648.0, ROW), Sense::click());
                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let sel = k == l.sel;
                    if sel {
                        sel_y = r.min.y - origin.y;
                        if l.reveal {
                            ui.scroll_to_rect(r, None);
                        }
                    }
                    // Rows scrolled out of view: their place only.
                    if !ui.is_rect_visible(r) {
                        y += ROW;
                        continue;
                    }
                    let h = ease(ctx, resp.id, resp.hovered() && !sel);
                    if h > 0.0 {
                        ui.painter().rect_filled(r, 8.0, p.surf_2.gamma_multiply(h * 0.7));
                    }
                    let cy = r.center().y;
                    let mut x = r.min.x + 12.0;
                    if let Some(ic) = icon {
                        icons::paint(ui.painter(), *ic, Rect::from_min_size(pos2(x, cy - 8.0), vec2(16.0, 16.0)), if sel { p.accent_2 } else { p.txt_mut });
                        x += 16.0 + 12.0;
                    }
                    // The name, its matched letters in ui 600 `accent_2`.
                    let base = if sel { p.txt_hi } else { p.txt };
                    let mut job = egui::text::LayoutJob::default();
                    // In runs of matched / unmatched letters (one section each).
                    let mut run = String::new();
                    let mut run_hit = false;
                    for (ci, ch) in name.chars().enumerate() {
                        let hit = hits.binary_search(&ci).is_ok();
                        if hit != run_hit && !run.is_empty() {
                            job.append(&run, 0.0, egui::TextFormat::simple(if run_hit { semibold(13.5) } else { ui_font(13.5) }, if run_hit { p.accent_2 } else { base }));
                            run.clear();
                        }
                        run_hit = hit;
                        run.push(ch);
                    }
                    job.append(&run, 0.0, egui::TextFormat::simple(if run_hit { semibold(13.5) } else { ui_font(13.5) }, if run_hit { p.accent_2 } else { base }));
                    // A recent file: its folder after it, faint (two files may share a name).
                    if let Entry::Recent(path) = &entries[*i].4 {
                        if let Some(d) = path.parent() {
                            job.append(&format!("   {}", short_dir(d)), 0.0, egui::TextFormat::simple(ui_font(12.0), p.txt_faint));
                        }
                    }
                    job.wrap = egui::text::TextWrapping { max_width: 400.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                    let g = ui.painter().layout_job(job);
                    ui.painter().galley(pos2(x, snap(ctx, cy - g.size().y / 2.0)), g, base);
                    let mut x = r.max.x - 12.0;
                    if let Some(k) = keys {
                        x = key_caps(ui.painter(), p, x, cy, k) - 10.0;
                    }
                    let g = ui.painter().layout_no_wrap(group.to_string(), ui_font(11.0), p.txt_dim);
                    let gr = Rect::from_min_size(pos2(x - g.size().x - 14.0, cy - 9.0), vec2(g.size().x + 14.0, 18.0));
                    ui.painter().rect_filled(gr, 9.0, alpha(p.gray, 0.15));
                    ui.painter().galley(pos2(gr.min.x + 7.0, snap(ctx, cy - g.size().y / 2.0)), g, p.txt_dim);
                    if resp.clicked() {
                        pick = Some(*i);
                    }
                    if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                        l.sel = k;
                    }
                    y += ROW;
                }
                let _ = y;
                let gy = ctx.animate_value_with_time(Id::unique("palette-sel-y"), sel_y, if crate::widgets::reduced_motion(ctx) { 0.0 } else { 0.09 });
                let hr = Rect::from_min_size(pos2(origin.x, origin.y + gy), vec2(648.0, ROW));
                ui.painter().set(under, egui::Shape::rect_filled(hr, 8.0, alpha(p.accent, 0.14)));
                let bar = Rect::from_min_max(pos2(hr.min.x, hr.min.y + 9.0), pos2(hr.min.x + 3.0, hr.max.y - 9.0));
                ui.painter().rect_filled(bar, 2.0, p.prim);
            });
        });
        l.reveal = false;
        // Footer: the keys, and how many there are.
        let (f, _) = ui.allocate_exact_size(vec2(660.0, 36.0), Sense::hover());
        ui.painter().hline(f.x_range(), ui.painter().round_to_pixel_center(f.min.y + 0.5), Stroke::new(1.0, p.brd));
        let pt = ui.painter();
        let cy = f.center().y;
        let mut x = f.min.x + 16.0;
        for (keys, what) in [(&["↑", "↓"][..], "to choose"), (&["↵"][..], "to run"), (&["Esc"][..], "to close")] {
            for k in keys {
                let g = pt.layout_no_wrap(k.to_string(), FontId::monospace(11.0), p.txt_mut);
                let w = (g.size().x + 10.0).max(18.0);
                key_cap(pt, p, x + w, cy, k);
                x += w + 3.0;
            }
            let g = pt.layout_no_wrap(what.to_string(), ui_font(11.5), p.txt_dim);
            pt.galley(pos2(x + 3.0, snap(ctx, cy - g.size().y / 2.0)), g.clone(), p.txt_dim);
            x += 3.0 + g.size().x + 16.0;
        }
        let count = if shown.len() == 1 { "1 result".to_string() } else { format!("{} results", shown.len()) };
        pt.text(pos2(f.max.x - 16.0, cy), Align2::RIGHT_CENTER, count, ui_font(11.5), p.txt_faint);
    });
    if modal.should_close() {
        close = true;
    }
    if let Some(i) = pick {
        app.palette = None;
        let (name, _, _, _, entry) = entries.swap_remove(i);
        if matches!(entry, Entry::Command(_) | Entry::Shell(_)) {
            note_command(&name);
        }
        match entry {
            Entry::Command(run) => app.run_command(run, ctx),
            Entry::Tab(i) => app.active = i,
            Entry::Recent(p) => app.open(p, ctx),
            Entry::Reread(enc, delim) => {
                if let Some(tab) = app.tabs.get_mut(app.active) {
                    if tab.dirty() { app.dialog = Some(Dialog::Reread { tab: app.active, enc, delim }); } else { tab.reopen_with(enc, delim, ctx); }
                }
            }
            Entry::Next(d, e, h) => {
                if let Some(d) = d { app.next_opts.delim = d; }
                if let Some(e) = e { app.next_opts.enc = e; }
                if let Some(h) = h { app.next_header = h; }
            }
            Entry::Col(act, c) => {
                if let Some(t) = app.tabs.get_mut(app.active) {
                    let row = t.grid.cursor.map_or(0, |x| x.row);
                    let key = |desc| csvfab_engine::view::SortKey { col: c, desc };
                    match act {
                        ColAct::Go => t.grid.go_to_col(row, c),
                        ColAct::Asc | ColAct::Desc => { t.sort = vec![key(matches!(act, ColAct::Desc))]; t.refresh(ctx); }
                        ColAct::ThenAsc | ColAct::ThenDesc => { t.sort.push(key(matches!(act, ColAct::ThenDesc))); t.refresh(ctx); }
                        ColAct::Panel => {
                            t.grid.go_to_col(row, c);
                            t.grid.open_panel = Some(c);
                        }
                        ColAct::Group => { let tool = tools::group::open(t, Some(c)); app.open_tool(tool); }
                        ColAct::Pivot => { let tool = tools::pivot::open(t, Some(c)); app.open_tool(tool); }
                        ColAct::Split => {
                            thread_local! { static COL: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
                            COL.with(|x| x.set(c));
                            app.run_command(|cx| tools::split::open_by(cx, COL.with(|x| x.get())), ctx);
                        }
                    }
                }
            }
            Entry::Shell(s) => match s {
                "open" => app.pick_files(ctx),
                "quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                "close-tab" => app.close_tab(app.active, false),
                "charts" | "filters" | "totals" | "expr" | "totals-sum" | "totals-clear" | "select-all" | "irregular" | "unrestrict" | "unmark" | "marked" => {
                    if let Some(t) = app.tabs.get_mut(app.active) {
                        match s {
                            "charts" => t.show_charts = !t.show_charts,
                            "filters" => t.show_filters = !t.show_filters,
                            "totals" => t.show_totals = !t.show_totals,
                            "expr" => { t.expr_mode = !t.expr_mode; t.refresh(ctx); }
                            "totals-sum" => { let n = t.st().map_or(0, |st| st.ncols); let k = t.kinds.clone(); t.totals.sum_all(&k, n); }
                            "totals-clear" => { let n = t.st().map_or(0, |st| st.ncols); t.totals.clear_all(n); }
                            "select-all" => {
                                let (rows, cols) = (t.shown(), t.st().map_or(0, |st| st.ncols));
                                if rows > 0 && cols > 0 {
                                    t.grid.anchor = Some(crate::grid::Cell { row: 0, col: 0 });
                                    t.grid.cursor = Some(crate::grid::Cell { row: rows - 1, col: cols - 1 });
                                }
                            }
                            "irregular" => {
                                if t.restrict.as_ref().is_some_and(|r| r.label.starts_with("Irregular")) {
                                    t.restrict = None;
                                    t.refresh(ctx);
                                } else if let Some(st) = t.st().cloned() {
                                    let h = st.header as RowId;
                                    let ids: Vec<RowId> = st.base.odd.iter().map(|&(r, _)| r).filter(|&r| r >= h).collect();
                                    t.restrict_to("Irregular rows", ids, ctx);
                                }
                            }
                            "unmark" => { drop_marks(t); t.restrict = None; t.refresh(ctx); }
                            "marked" => {
                                // In the table's order.
                                if let Some(st) = t.st().cloned() {
                                    let ids: Vec<csvfab_engine::RowId> = (0..st.len()).map(|p| st.data_id(p)).filter(|id| t.marks.rows.contains(id)).collect();
                                    let label = if t.marks_label.is_empty() { "marked rows".to_string() } else { t.marks_label.clone() };
                                    t.restrict_to(label, ids, ctx);
                                }
                            }
                            _ => { t.restrict = None; t.refresh(ctx); }
                        }
                    }
                }
                "find" => { if let Some(c) = tools::commands().into_iter().find(|c| c.name == "Find & replace") { app.run_command(c.run, ctx); } }
                "raw" => app.card.open_raw(),
                "save" => app.save(None, false, ctx),
                "save_as" => app.save_as(ctx),
                "goto" => app.dialog = Some(Dialog::GoTo(String::new())),
                "card" => app.card.toggle(),
                "undo" => app.undo(false, ctx),
                "redo" => app.undo(true, ctx),
                "keys" => app.dialog = Some(Dialog::Shortcuts),
                "about" => app.dialog = Some(Dialog::About),
                "font" => app.font_picker = Some(crate::fontpick::Picker::new(ctx)),
                "news" => app.dialog = Some(Dialog::WhatsNew),
                "header" | "header-auto" | "eol" => {
                    if let Some(tab) = app.tabs.get_mut(app.active) {
                        if let Some(t) = &mut tab.table {
                            if s == "header-auto" {
                                let h = csvfab_engine::table::detected_header(&t.st.base);
                                t.set_header(h);
                            } else if s == "header" {
                                let h = t.st.header;
                                t.set_header(!h);
                            } else {
                                let eol = t.st.eol.unwrap_or(t.st.base.eol);
                                t.set_eol(if eol == b"\r\n" { b"\n" } else { b"\r\n" });
                                tab.grid.notice = Some(format!("Line endings set to {} — written at the next Save.", if eol == b"\r\n" { "LF" } else { "CRLF" }));
                            }
                        }
                        tab.after_edit(s != "eol", ctx);
                    }
                }
                theme => app.set_theme(theme, ctx),
            },
        }
    } else if close {
        app.palette = None;
    }
}

/// Files dragged over an open table: the window says where they go (the web's drop
/// overlay). The welcome screen has its own drop card.
pub fn drop_overlay(app: &App, ctx: &egui::Context) {
    let n = ctx.input(|i| i.raw.hovered_files.len());
    if n == 0 || app.tabs.is_empty() { return; }
    let p = &app.theme.p;
    let r = ctx.content_rect().shrink(14.0);
    let pt = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("drop-overlay")));
    pt.rect_filled(ctx.content_rect(), 0.0, alpha(p.bg, 0.72));
    // A dashed frame: dashes of 10 every 16 px.
    let stroke = Stroke::new(2.0, alpha(p.prim, 0.8));
    for (a, b) in [(r.left_top(), r.right_top()), (r.right_top(), r.right_bottom()), (r.right_bottom(), r.left_bottom()), (r.left_bottom(), r.left_top())] {
        let len = (b - a).length();
        let dir = (b - a) / len.max(1.0);
        let mut t = 0.0;
        while t < len {
            pt.line_segment([a + dir * t, a + dir * (t + 10.0).min(len)], stroke);
            t += 16.0;
        }
    }
    let c = r.center();
    let disc = Rect::from_center_size(pos2(c.x, c.y - 30.0), vec2(56.0, 56.0));
    pt.circle_filled(disc.center(), 28.0, p.prim);
    icons::paint(&pt, Icon::Upload, disc.shrink(16.0), p.on_prim);
    let what = if n == 1 { "Drop the file to open it".to_string() } else { format!("Drop the {n} files to open them") };
    pt.text(pos2(c.x, c.y + 18.0), Align2::CENTER_TOP, what, crate::grid::semibold(16.0), p.txt_hi);
    pt.text(pos2(c.x, c.y + 44.0), Align2::CENTER_TOP, "Each opens in its own tab — CSV, TSV, Excel, JSON, SQLite or a folder", ui_font(12.5), p.txt_mut);
    ctx.request_repaint();
}

// ---------------------------------------------------------------- toasts

/// A toast's kind, read from its words (the app passes plain text).
fn toast_kind(text: &str, p: &Colors) -> (Color32, Icon) {
    let t = text.to_lowercase();
    if ["could not", "cannot", "can't", "error", "failed", "invalid"].iter().any(|w| t.contains(w)) {
        (p.danger, Icon::Warning)
    } else if ["deleted", "removed"].iter().any(|w| t.contains(w)) {
        (p.danger, Icon::ToastDanger)
    } else if ["warning", "nothing", "no ", "skipped", "irregular"].iter().any(|w| t.starts_with(w) || t.contains(&format!(" {w}"))) {
        (p.warn, Icon::ToastWarn)
    } else if ["saved", "written", "copied", "done", "added", "created", "replaced", "merged", "split", "filled"].iter().any(|w| t.contains(w)) {
        (p.ok, Icon::ToastOk)
    } else {
        (p.prim, Icon::ToastInfo)
    }
}

/// Toasts, bottom right above the status bar, newest at the bottom: each rises 12 px with a
/// slight overshoot as it fades in (260 ms), slides 24 px right as it fades out (200 ms), and
/// the others glide to make room. Also runs the menus' and windows' pop (`widgets::motion`):
/// this is drawn last in the frame.
pub fn toasts(app: &mut App, ctx: &egui::Context) {
    crate::widgets::motion(ctx);
    let now = ctx.input(|i| i.time);
    let born_id = Id::unique("toast-born");
    let gone_id = Id::unique("toast-dismissed");
    let key = |t: &crate::app::Toast| t.id.to_string();
    // Dismissed by their ×: they leave as when their time is up.
    let mut gone = ctx.data(|d| d.get_temp::<Born>(gone_id)).unwrap_or_default();
    app.toasts.retain(|t| t.until > now && gone.0.get(&key(t)).is_none_or(|&d| now < d + 0.2));
    gone.0.retain(|k, _| app.toasts.iter().any(|t| key(t) == *k));
    if app.toasts.is_empty() {
        ctx.data_mut(|d| {
            d.remove::<Born>(born_id);
            d.remove::<Born>(gone_id);
        });
        return;
    }
    let p = app.theme.p.clone();
    let reduced = crate::widgets::reduced_motion(ctx);
    let mut born = ctx.data(|d| d.get_temp::<Born>(born_id)).unwrap_or_default();
    born.0.retain(|k, _| app.toasts.iter().any(|t| key(t) == *k));
    let screen = ctx.content_rect();
    let mut y = screen.max.y - 48.0;
    let mut moving = false;
    let mut wake = f64::INFINITY;
    let mut dismiss = None;
    let mut undo = None;
    let mut hold = Vec::new();
    for (k, t) in app.toasts.iter().enumerate().rev() {
        let t0 = *born.0.entry(key(t)).or_insert(now);
        let tin = if reduced { 1.0 } else { (((now - t0) / 0.26) as f32).clamp(0.0, 1.0) };
        let out_at = gone.0.get(&key(t)).copied().unwrap_or(t.until - 0.2).min(t.until - 0.2);
        let tout = if reduced { 0.0 } else { (((now - out_at) / 0.2) as f32).clamp(0.0, 1.0) };
        wake = wake.min(out_at - now);
        let g = ctx.fonts_mut(|f| f.layout(t.text.clone(), ui_font(13.0), p.txt_hi, 400.0));
        let h = (g.size().y + 20.0).max(40.0);
        let uw = if t.undo.is_some() { 58.0 } else { 0.0 };
        let w = (g.size().x + 12.0 + 16.0 + 10.0 + 10.0 + 20.0 + 8.0 + uw).clamp(260.0, 440.0 + uw).round();
        // The stack glides when a toast comes or goes.
        let ty = ctx.animate_value_with_time(Id::unique(("toast-y", key(t))), y, if reduced { 0.0 } else { 0.2 });
        moving |= tin < 1.0 || tout > 0.0 || (ty - y).abs() > 0.1;
        let dy = 12.0 * (1.0 - egui::emath::easing::back_out(tin));
        let dx = 24.0 * egui::emath::easing::cubic_in(tout);
        let a = egui::emath::easing::quadratic_out(tin) * (1.0 - tout);
        let r = Rect::from_min_size(pos2(screen.max.x - 16.0 - w + dx, ty - h + dy), vec2(w, h));
        let (kc, icon) = toast_kind(&t.text, &p);
        egui::Area::new(Id::unique(("toast", key(t)))).fixed_pos(r.min).order(egui::Order::Tooltip).fade_in(false).show(ctx, |ui| {
            ui.multiply_opacity(a);
            // Sized by what it holds: an empty area would stay in egui's invisible sizing pass.
            let (r, body) = ui.allocate_exact_size(r.size(), Sense::hover());
            // Read while hovered: it stays, and leaves 2.5 s after the pointer does.
            if body.hovered() && tout <= 0.0 { hold.push(k); }
            let pt = ui.painter();
            pt.add(egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(if crate::widgets::is_dark(&p) { 115 } else { 40 }) }.as_shape(r, 9));
            // The 3 px left border in the kind's colour, following the corner's curve.
            pt.rect_filled(r, 9.0, kc);
            pt.rect_filled(Rect::from_min_max(pos2(r.min.x + 3.0, r.min.y), r.max), CornerRadius { nw: 7, sw: 7, ne: 9, se: 9 }, p.surf);
            pt.rect_stroke(r, 9.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
            icons::paint(pt, icon, Rect::from_min_size(pos2(r.min.x + 12.0, snap(ctx, r.center().y - 8.0)), vec2(16.0, 16.0)), kc);
            pt.galley(pos2(r.min.x + 12.0 + 16.0 + 10.0, snap(ctx, r.center().y - g.size().y / 2.0)), g, p.txt_hi);
            let xr = Rect::from_center_size(pos2(r.max.x - 10.0 - 10.0, r.center().y), vec2(20.0, 20.0));
            let xresp = ui.interact(xr, Id::unique(("toast-x", k)), Sense::click());
            let xh = ease(ctx, xresp.id, xresp.hovered());
            if xh > 0.0 {
                ui.painter().rect_filled(xr, 5.0, p.surf_2.gamma_multiply(xh));
            }
            icons::cross(ui.painter(), xr.shrink(5.0), 2.0, mix(p.txt_hi, p.txt_dim, xh));
            if t.undo.is_some() {
                // Undo: a small outline button before the ×.
                let ur = Rect::from_min_max(pos2(xr.min.x - 8.0 - 50.0, r.center().y - 12.0), pos2(xr.min.x - 8.0, r.center().y + 12.0));
                let uresp = ui.interact(ur, Id::unique(("toast-undo", key(t))), Sense::click());
                let uh = ease(ctx, uresp.id, uresp.hovered());
                ui.painter().rect(ur, 6.0, mix(p.surf, p.surf_2, uh), Stroke::new(1.0, mix(p.brd_2, p.prim, uh)), StrokeKind::Inside);
                ui.painter().text(ur.center(), egui::Align2::CENTER_CENTER, "Undo", ui_font(12.5), mix(p.accent_2, p.txt_hi, uh));
                if uresp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    undo = Some(k);
                }
            }
            if xresp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                dismiss = Some(k);
            }
        });
        y -= h + 8.0;
    }
    ctx.data_mut(|d| d.insert_temp(born_id, born));
    for k in hold {
        let t = &mut app.toasts[k];
        t.until = t.until.max(now + 2.5);
    }
    // Undo once: the toast leaves as by its ×, its button no longer answers.
    let undo = undo.filter(|&k| !gone.0.contains_key(&key(&app.toasts[k]))).and_then(|k| {
        gone.0.insert(key(&app.toasts[k]), now);
        app.toasts[k].undo
    });
    if let Some(k) = dismiss {
        gone.0.entry(key(&app.toasts[k])).or_insert(now);
    }
    ctx.data_mut(|d| d.insert_temp(gone_id, gone));
    if let Some(u) = undo { app.undo_from_toast(u, ctx); }
    if moving || dismiss.is_some() || undo.is_some() {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wake.max(0.0)));
    }
}

// ---------------------------------------------------------------- welcome

/// "2 h ago", "3 d ago": how long since a file changed.
fn ago(t: std::time::SystemTime) -> String {
    let s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match s {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 86_400 * 30 => format!("{} d ago", s / 86_400),
        s if s < 86_400 * 365 => format!("{} mo ago", s / (86_400 * 30)),
        s => format!("{} y ago", s / (86_400 * 365)),
    }
}

/// The folder as people say it: `~/Work/csvfab`.
fn short_dir(d: &std::path::Path) -> String {
    let s = d.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

/// The welcome screen (no file): on the dotted `bg`, an aurora of the accent colours behind
/// the logo and the gradient name; the drop card; the palette and shortcuts keys; recent files.
pub fn welcome(app: &mut App, ui: &mut Ui) {
    let p = app.theme.p.clone();
    let ctx = ui.ctx().clone();
    let dark = crate::widgets::is_dark(&p);
    let full = ui.max_rect();
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(full));
    ui.set_clip_rect(full);
    let ui = &mut ui;
    let pt = ui.painter().clone();
    pt.rect_filled(full, 0.0, p.bg);
    // As many recent files as fit (at most 12, as the web's), the column kept clear of the
    // status bar; a missing one drawn faint (opening it says so and forgets it). Their sizes
    // are looked up in the background, not on every frame.
    let fixed = 54.0 + 44.0 + 170.0 + 28.0 + 34.0 + 28.0 + 22.0 + 12.0 + 60.0;
    let fit = (((full.height() * 0.94 - fixed) / 50.0).floor().max(0.0) as usize).min(12);
    let mut files = app.recent_files(&ctx);
    files.truncate(fit);
    let existing: Vec<std::path::PathBuf> = files.iter().map(|f| f.0.clone()).collect();
    // The column: 580 wide, centred, starting 8 % down (more when the window is tall and there
    // is little to show, so it sits in the eye line).
    let content_h = 54.0 + 44.0 + 170.0 + 28.0 + 34.0 + if existing.is_empty() { 0.0 } else { 28.0 + 22.0 + existing.len() as f32 * 50.0 + 12.0 } + 60.0;
    let top = (full.min.y + (full.height() * 0.08).max((full.height() - content_h) * 0.38)).round();
    let cx = full.center().x.round();
    // The dot texture, fading out towards the edges.
    let dot = alpha(p.hi, if dark { 0.06 } else { 0.08 });
    let (c0, reach) = (pos2(cx, top + 200.0), full.width().max(full.height()) * 0.62);
    let mut yy = (full.min.y + 12.0).round();
    while yy < full.max.y {
        let mut xx = (full.min.x + 12.0).round();
        while xx < full.max.x {
            let d = ((xx - c0.x).powi(2) + (yy - c0.y).powi(2)).sqrt() / reach;
            let f = (1.0 - d).clamp(0.0, 1.0);
            if f > 0.02 {
                pt.rect_filled(Rect::from_center_size(pos2(xx, yy), vec2(1.5, 1.5)), 0.75, dot.gamma_multiply(f));
            }
            xx += 24.0;
        }
        yy += 24.0;
    }
    let mut open = false;
    let mut recent = None;
    let mut forget = None;
    let mut gone_hit: Option<std::path::PathBuf> = None;
    let mut what: Option<&str> = None;
    // Title: the logo 54 and "csvfab" in a gradient prim → violet → pink, one colour per letter.
    let word = "csvfab";
    let stops = [p.prim, p.violet_txt, p.pink_txt];
    let mut job = egui::text::LayoutJob::default();
    for (i, ch) in word.chars().enumerate() {
        let t = i as f32 / (word.len() - 1) as f32 * 2.0;
        let (a, b, f) = if t < 1.0 { (stops[0], stops[1], t) } else { (stops[1], stops[2], t - 1.0) };
        let mut fmt = egui::TextFormat::simple(ui_font(40.0), mix(b, a, f));
        fmt.extra_letter_spacing = -0.6;
        job.append(ch.encode_utf8(&mut [0; 4]), 0.0, fmt);
    }
    let tg = pt.layout_job(job);
    let tw = 54.0 + 16.0 + tg.size().x;
    let lx = (cx - tw / 2.0).round();
    let logo = Rect::from_min_size(pos2(lx, top), vec2(54.0, 54.0));
    icons::paint(&pt, Icon::Logo, logo, Color32::WHITE);
    pt.galley(pos2(logo.max.x + 16.0, snap(&ctx, logo.center().y - tg.size().y / 2.0 - 1.0)), tg, p.txt_hi);
    let mut y = top + 54.0 + 44.0;

    // The drop card: glass, its border and glow warming up under the pointer.
    let card = Rect::from_min_size(pos2(cx - 290.0, y), vec2(580.0, 170.0));
    let resp = ui.interact(card, Id::unique("welcome-drop"), Sense::click());
    let dropping = ctx.input(|i| !i.raw.hovered_files.is_empty());
    let h = ease(&ctx, resp.id, resp.hovered() || dropping);
    if h > 0.0 {
        crate::widgets::glow(&pt, card.center(), vec2(380.0, 160.0), alpha(p.prim, 0.07 * h));
    }
    pt.rect(card, 20.0, mix(alpha(p.hi, 0.035), alpha(p.bg, 0.08), h), Stroke::new(1.0, mix(alpha(p.prim, 0.55), alpha(p.hi, 0.14), h)), StrokeKind::Inside);
    let c = pos2(card.center().x, card.min.y + 30.0 + 25.0);
    pt.circle_filled(c, 25.0 + 9.0, alpha(p.prim, 0.08 + 0.08 * h));
    pt.circle_filled(c, 25.0, p.prim);
    icons::paint(&pt, Icon::Upload, Rect::from_center_size(c - vec2(0.0, 2.0 * h), vec2(23.0, 23.0)), p.on_prim);
    pt.text(pos2(card.center().x, c.y + 25.0 + 22.0), Align2::CENTER_CENTER, if dropping { "Drop to open" } else { "Drop files to open them" }, semibold(15.5), p.txt_hi);
    pt.text(pos2(card.center().x, c.y + 25.0 + 43.0), Align2::CENTER_CENTER, "CSV, TSV, Excel or JSON — or click to choose", ui_font(12.5), p.txt_mut);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        open = true;
    }
    y = card.max.y + 28.0;

    // The keys: "Command palette Ctrl P", "Shortcuts F1".
    let keys: [(&str, &[&str], &str); 2] = [("Command palette", &["Ctrl", "P"], "palette"), ("Shortcuts", &["F1"], "keys")];
    let widths: Vec<f32> = keys.iter().map(|(l, caps, _)| {
        let lw = pt.layout_no_wrap(l.to_string(), ui_font(13.0), p.txt_mut).size().x;
        let cw: f32 = caps.iter().map(|c| pt.layout_no_wrap(c.to_string(), FontId::monospace(11.5), p.txt_mut).size().x + 10.0 + 4.0).sum();
        (12.0 + lw + 10.0 + cw - 4.0 + 12.0).round()
    }).collect();
    let mut x = (cx - (widths.iter().sum::<f32>() + 10.0) / 2.0).round();
    for ((label, caps, act), w) in keys.iter().zip(&widths) {
        let r = Rect::from_min_size(pos2(x, y), vec2(*w, 34.0));
        let resp = ui.interact(r, Id::unique(("welcome-key", *act)), Sense::click());
        let h = ease(&ctx, resp.id, resp.hovered());
        pt.rect(r, 8.0, alpha(p.hi, 0.02 + 0.03 * h), Stroke::new(1.0, alpha(p.hi, 0.14 + 0.1 * h)), StrokeKind::Inside);
        let col = mix(p.txt_hi, p.txt_mut, h);
        let g = pt.layout_no_wrap(label.to_string(), ui_font(13.0), col);
        let gw = g.size().x;
        pt.galley(pos2(r.min.x + 12.0, snap(&ctx, r.center().y - g.size().y / 2.0)), g, col);
        let mut kx = r.min.x + 12.0 + gw + 10.0;
        for cap in caps.iter() {
            let g = pt.layout_no_wrap(cap.to_string(), FontId::monospace(11.5), p.txt_mut);
            let kr = Rect::from_min_size(pos2(kx, r.center().y - 9.0), vec2(g.size().x + 10.0, 18.0));
            pt.rect(kr, 5.0, alpha(p.hi, 0.05), Stroke::new(1.0, alpha(p.hi, 0.18)), StrokeKind::Inside);
            pt.galley(pos2(kr.min.x + 5.0, snap(&ctx, kr.center().y - g.size().y / 2.0)), g, p.txt_mut);
            kx = kr.max.x + 4.0;
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            what = Some(act);
        }
        x += w + 10.0;
    }
    y += 34.0;

    // Recent files.
    if !existing.is_empty() {
        y += 28.0;
        let hg = pt.layout_no_wrap("RECENT FILES".into(), semibold(11.0), p.txt_dim);
        pt.galley(pos2(cx - 290.0 + 14.0, y), hg, p.txt_dim);
        y += 22.0;
        let box_r = Rect::from_min_size(pos2(cx - 290.0, y), vec2(580.0, existing.len() as f32 * 50.0 + 12.0));
        pt.rect(box_r, 14.0, alpha(p.hi, 0.02), Stroke::new(1.0, alpha(p.hi, 0.12)), StrokeKind::Inside);
        let mut ry = box_r.min.y + 6.0;
        for (k, path) in existing.iter().enumerate() {
            let r = Rect::from_min_size(pos2(box_r.min.x + 6.0, ry), vec2(568.0, 50.0));
            let resp = ui.interact(r, Id::unique(("welcome-recent", k)), Sense::click());
            let h = ease(&ctx, resp.id, resp.hovered());
            if h > 0.0 {
                pt.rect_filled(r, 10.0, alpha(p.hi, 0.05 * h));
            }
            let size = files[k].1;
            let gone = size.is_none();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let dir = path.parent().map(short_dir).unwrap_or_default();
            let tile = Rect::from_min_size(pos2(r.min.x + 10.0, r.center().y - 16.0), vec2(32.0, 32.0));
            pt.rect(tile, 8.0, alpha(p.ok_rgb, 0.12 + 0.06 * h), Stroke::new(1.0, alpha(p.ok_rgb, 0.25)), StrokeKind::Inside);
            icons::paint(&pt, Icon::FileText, Rect::from_center_size(tile.center(), vec2(16.0, 16.0)), p.ok_txt);
            let tx = tile.max.x + 12.0;
            let right_w = 150.0;
            let line = |t: &str, f: FontId, c: Color32, max: f32| {
                let mut job = egui::text::LayoutJob::simple_singleline(t.to_string(), f, c);
                job.wrap = egui::text::TextWrapping { max_width: max, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                pt.layout_job(job)
            };
            let name_c = if gone { p.txt_faint } else { p.txt_hi };
            let g = line(&name, medium(13.5), name_c, r.max.x - tx - right_w);
            pt.galley(pos2(tx, snap(&ctx, r.min.y + 8.0)), g, name_c);
            let g = line(&dir, ui_font(11.5), p.txt_dim, r.max.x - tx - right_w);
            pt.galley(pos2(tx, snap(&ctx, r.min.y + 27.0)), g, p.txt_dim);
            // Right: size and age; on hover a × to forget it.
            let xr = Rect::from_center_size(pos2(r.max.x - 12.0 - 10.0, r.center().y), vec2(20.0, 20.0));
            let mut rx = r.max.x - 14.0;
            if h > 0.0 {
                let xresp = ui.interact(xr, Id::unique(("welcome-forget", k)), Sense::click());
                let xh = ease(&ctx, xresp.id, xresp.hovered());
                pt.rect_filled(xr, 5.0, alpha(p.danger, 0.15 * xh));
                icons::cross(&pt, xr.shrink(5.0), 2.0, mix(p.danger_txt, p.txt_dim, xh).gamma_multiply(h));
                if xresp.on_hover_text("Remove from the list").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    forget = Some(path.clone());
                }
                rx -= 26.0 * h;
            }
            // Its size (or that it is gone), and when it was last opened here.
            let top = match size { None => Some("missing".to_string()), Some(u64::MAX) => None, Some(n) => Some(fmt_size(n)) };
            if let Some(t) = top {
                let g = pt.layout_no_wrap(t, medium(12.0), if gone { p.txt_faint } else { p.txt_mut });
                pt.galley(pos2(rx - g.size().x, snap(&ctx, r.min.y + 9.0)), g, p.txt_mut);
            }
            if let Some(&t) = app.config.opened.get(path) {
                let g = pt.layout_no_wrap(ago(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t)), ui_font(11.0), p.txt_faint);
                pt.galley(pos2(rx - g.size().x, snap(&ctx, r.min.y + 28.0)), g, p.txt_faint);
            }
            if forget.is_none() && resp.on_hover_text(path.display().to_string()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                if gone { gone_hit = Some(path.clone()); } else { recent = Some(path.clone()); }
            }
            ry += 50.0;
        }
        y = box_r.max.y;
    }
    // Footer: open · filter · clean · rewrite in place.
    y += 30.0;
    let mut job = egui::text::LayoutJob::default();
    for (i, w) in ["open", "filter", "clean", "rewrite in place"].iter().enumerate() {
        if i > 0 {
            let mut f = egui::TextFormat::simple(ui_font(12.0), alpha(p.accent, 0.6));
            f.extra_letter_spacing = 0.48;
            job.append("  ·  ", 0.0, f);
        }
        let mut f = egui::TextFormat::simple(ui_font(12.0), p.txt_faint);
        f.extra_letter_spacing = 0.48;
        job.append(w, 0.0, f);
    }
    let g = pt.layout_job(job);
    pt.galley(pos2((cx - g.size().x / 2.0).round(), snap(&ctx, y)), g, p.txt_faint);

    if open {
        app.pick_files(&ctx);
    }
    if let Some(r) = recent {
        app.open(r, &ctx);
    }
    // A file no longer there: said, and forgotten (as the web's).
    if let Some(g) = gone_hit {
        app.toast(format!("{} is no longer where it was — removed from the recent files.", g.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), &ctx);
        forget = Some(g);
    }
    if let Some(f) = forget {
        app.config.recent.retain(|x| *x != f);
        if std::env::var_os("CSVFAB_SHOT").is_none() {
            app.config.save();
        }
    }
    match what {
        Some("palette") => app.palette = Some(Default::default()),
        Some("keys") => app.dialog = Some(Dialog::Shortcuts),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy;

    #[test]
    fn fuzzy_ranks_word_starts_and_runs() {
        assert!(fuzzy("grp", "Group by…").is_some());
        assert!(fuzzy("xyz", "Group by…").is_none());
        // Accents and case are ignored, every word must match.
        assert!(fuzzy("eleve", "Élève").is_some());
        assert!(fuzzy("save as", "Save as…").is_some());
        assert!(fuzzy("save zz", "Save as…").is_none());
        // A word start beats letters in the middle of words.
        let a = fuzzy("sa", "Save as…").unwrap().0;
        let b = fuzzy("sa", "Keep last values").unwrap().0;
        assert!(a > b, "{a} vs {b}");
        // The matched letters are those shown in bold.
        assert_eq!(fuzzy("gb", "Group by…").unwrap().1, vec![0, 6]);
        assert_eq!(fuzzy("find", "Find & replace").unwrap().1, vec![0, 1, 2, 3]);
    }
}
