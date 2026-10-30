//! The monospace font dialog (32-font.js; palette "Monospace font…"): the font of the
//! cells, formulas and codes, kept in the settings (`mono_font`). Picking one installs it
//! at once — the sample at the top and the grid behind show it — and every tab's column
//! widths scale by the change of the advance, so widths set by hand keep their proportions.

use crate::app::App;
use crate::grid::semibold;
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::theme::alpha;
use crate::widgets::{self, Kind};
use eframe::egui::{self, pos2, vec2, FontId, Id, Rect, Sense, Stroke};

const SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";
const SYMBOLS: &str = "0O Il1 {}[]()<> => != 12 345,67 € @#%& éàçô";
/// A family's line in the list: what tells monospace faces apart.
const SAMPLE: &str = "0O Il1 {}[] => != 12 345,67 € éàç";

pub struct Picker {
    /// The families and their regular faces (each shown in its own: `fonts::set_samples`).
    job: Option<Job<Vec<(String, Option<crate::fonts::Pick>)>>>,
    list: Vec<String>,
    q: String,
    /// The advance of '0' before a change: the widths scale once the new font is in.
    rescale: Option<f32>,
    focus: bool,
    /// A free font being installed: (its catalogue id, the download).
    installing: Option<(&'static str, Job<Result<crate::fontshop::Installed, String>>)>,
    /// The row the arrow keys are on (Enter picks it).
    hot: Option<usize>,
}

impl Picker {
    pub fn new(ctx: &egui::Context) -> Picker {
        // fc-list and a look into each family's file: off the UI thread.
        Picker { job: Some(Job::spawn(ctx, |_, _| crate::fonts::list_monospace_faces())), list: Vec::new(), q: String::new(), rescale: None, focus: true, installing: None, hot: None }
    }
}

fn advance(ctx: &egui::Context, m: &crate::theme::Metrics) -> f32 {
    ctx.fonts_mut(|f| f.glyph_width(&FontId::monospace(m.font_grid), '0'))
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(pk) = &mut app.font_picker else { return };
    if let Some(l) = pk.job.as_mut().and_then(|j| j.poll()) {
        pk.job = None;
        let faces = l.iter().filter_map(|x| x.1.clone()).collect();
        pk.list = l.into_iter().map(|x| x.0).collect();
        crate::fonts::set_samples(ctx, faces, app.config.mono_font.as_deref());
    }
    // The new font is in since this frame: the widths follow its advance.
    if let Some(old) = pk.rescale.take() {
        let k = advance(ctx, &app.m) / old;
        if (k - 1.0).abs() > 0.005 {
            for t in &mut app.tabs {
                for w in &mut t.grid.widths { *w = ((*w - 21.0) * k + 21.0).round().clamp(60.0, 900.0); }
            }
        }
    }
    // An install done: the list again (fontconfig knows it now), the font chosen.
    let mut said: Option<String> = None;
    let mut pick: Option<String> = None;
    if let Some((_, j)) = &mut pk.installing {
        if let Some(r) = j.poll() {
            pk.installing = None;
            match r {
                Ok(i) => {
                    pk.job = Some(Job::spawn(ctx, |_, _| crate::fonts::list_monospace_faces()));
                    pick = Some(i.family.to_string());
                    said = Some(format!("{} {}installed ({} files) and chosen.", i.family, i.version.map(|v| format!("{v} ")).unwrap_or_default(), i.files));
                }
                Err(e) => said = Some(format!("The font could not be installed: {e}")),
            }
        }
    }
    let p = &app.theme.p;
    let cur = app.config.mono_font.clone().unwrap_or_default();
    let mut close = false;
    let modal = widgets::modal(ctx, Id::unique("font-picker"), p, crate::theme::dialog_frame(p, &app.m), |ui| {
        ui.set_width(460.0);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.label(egui::RichText::new("Monospace font").font(semibold(15.0)).color(p.txt_hi));
        ui.add_space(3.0);
        ui.label(egui::RichText::new("The font of the cells, formulas and codes.").font(FontId::proportional(12.5)).color(p.txt_dim));
        ui.add_space(12.0);
        // The sample, in the font in use.
        egui::Frame::new().fill(p.bg_deep).stroke(Stroke::new(1.0, p.brd)).corner_radius(8).inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(SENTENCE).font(FontId::monospace(14.0)).color(p.txt_hi));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(SYMBOLS).font(FontId::monospace(14.0)).color(p.accent_2));
        });
        ui.add_space(12.0);
        let (sr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let sid = Id::unique("font-picker-q");
        let foc = ui.memory(|m| m.has_focus(sid));
        widgets::field_frame(ui, p, sid, sr, foc, false, 6.0);
        icons::paint(ui.painter(), Icon::Magnifier, Rect::from_center_size(pos2(sr.min.x + 15.0, sr.center().y), vec2(14.0, 14.0)), p.txt_dim);
        let te = egui::TextEdit::singleline(&mut pk.q).id(sid).font(FontId::proportional(13.0)).text_color(p.txt_hi)
            .hint_text(egui::RichText::new("Find a font…").font(FontId::proportional(13.0)).color(p.txt_sub)).frame(egui::Frame::NONE);
        let r = widgets::centered_edit(ui, Rect::from_min_max(pos2(sr.min.x + 29.0, sr.min.y), pos2(sr.max.x - 10.0, sr.max.y)), te);
        if std::mem::take(&mut pk.focus) { r.request_focus(); }
        ui.add_space(8.0);
        let q = csvfab_engine::text::fold(pk.q.trim());
        let matches = |s: &str| q.is_empty() || csvfab_engine::text::fold(s).contains(&q);
        let mut rows: Vec<(String, String, &str)> = Vec::new();
        if matches("Default") { rows.push((String::new(), "Default".into(), "MonoLisa, Cascadia Code, JetBrains Mono… the first one found")); }
        if matches("System monospace") { rows.push(("monospace".into(), "System monospace".into(), "the system's own")); }
        for f in pk.list.iter().filter(|f| matches(f)) { rows.push((f.clone(), f.clone(), "")); }
        // ↑ / ↓ move over the rows, Enter picks (as the web's cards, keyboard reachable).
        let (up, down, enter) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)));
        if !rows.is_empty() {
            let here = pk.hot.or_else(|| rows.iter().position(|r| r.0 == cur)).unwrap_or(0);
            if down { pk.hot = Some((here + 1).min(rows.len() - 1)); }
            if up { pk.hot = Some(here.saturating_sub(1)); }
            if enter { if let Some(r) = pk.hot.and_then(|k| rows.get(k)) { if r.0 != cur { pick = Some(r.0.clone()); } } }
        }
        let have: Vec<String> = pk.list.iter().map(|f| f.to_lowercase()).collect();
        let shop: Vec<&crate::fontshop::Entry> = crate::fontshop::catalog().iter().filter(|e| !have.contains(&e.family.to_lowercase()) && (matches(e.name) || matches(e.note))).collect();
        let mut install: Option<&'static str> = None;
        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
            let w = ui.available_width() - 10.0;
            for (k, (val, label, note)) in rows.iter().enumerate() {
                // An installed family: a line in its own face under its name (as the web's cards).
                let face = if val.is_empty() || val == "monospace" { None } else { crate::fonts::sample_family(ui.ctx(), val) };
                let h = if note.is_empty() && face.is_none() { 32.0 } else { 44.0 };
                let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                let on = *val == cur;
                let hot = resp.hovered() || pk.hot == Some(k);
                if pk.hot == Some(k) && (up || down) { resp.scroll_to_me(None); }
                if on || hot { ui.painter().rect_filled(r, 6.0, if on { alpha(p.prim, 0.14) } else { p.menu_bg_hover }); }
                let col = if on { p.accent_2 } else { p.txt_hi };
                let ty = if h < 44.0 { r.center().y } else { r.min.y + 15.0 };
                ui.painter().text(pos2(r.min.x + 12.0, ty), egui::Align2::LEFT_CENTER, label, if on { semibold(13.0) } else { FontId::proportional(13.0) }, col);
                if !note.is_empty() {
                    ui.painter().text(pos2(r.min.x + 12.0, r.min.y + 31.0), egui::Align2::LEFT_CENTER, *note, FontId::proportional(11.5), p.txt_dim);
                } else if let Some(f) = face {
                    let clip = Rect::from_min_max(r.min, pos2(r.max.x - 36.0, r.max.y)).intersect(ui.clip_rect());
                    ui.painter().with_clip_rect(clip).text(pos2(r.min.x + 12.0, r.min.y + 31.0), egui::Align2::LEFT_CENTER, SAMPLE, FontId::new(12.5, f), p.txt_mut);
                }
                if on { icons::paint(ui.painter(), Icon::Check, Rect::from_center_size(pos2(r.max.x - 18.0, r.center().y), vec2(14.0, 14.0)), p.accent_2); }
                if hot { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                if resp.clicked() && !on { pick = Some(val.clone()); }
            }
            // Free fonts not installed yet: downloaded from their projects on request.
            if pk.job.is_none() && !shop.is_empty() {
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Free fonts to install").font(semibold(12.5)).color(p.txt_hi));
                ui.label(egui::RichText::new("downloaded from their official repositories, for your user account only").font(FontId::proportional(11.5)).color(p.txt_dim));
                ui.add_space(4.0);
                for e in &shop {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
                    ui.painter().text(pos2(r.min.x + 12.0, r.min.y + 15.0), egui::Align2::LEFT_CENTER, e.name, FontId::proportional(13.0), p.txt_hi);
                    ui.painter().text(pos2(r.min.x + 12.0, r.min.y + 31.0), egui::Align2::LEFT_CENTER, format!("{} · {}", e.note, e.license), FontId::proportional(11.5), p.txt_dim);
                    let br = Rect::from_min_size(pos2(r.max.x - 96.0, r.center().y - 14.0), vec2(88.0, 28.0));
                    let busy = pk.installing.as_ref().is_some_and(|x| x.0 == e.id);
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
                    let label = if busy { "Installing…" } else { "Install" };
                    if widgets::button(&mut child, p, Kind::Outline, None, label, pk.installing.is_none(), 28.0, 12.0, 12.5).on_hover_text(format!("From github.com/{}, for your account only", e.repo)).clicked() {
                        install = Some(e.id);
                    }
                }
            }
            if pk.job.is_some() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Listing the installed fonts…").font(FontId::proportional(12.0)).color(p.txt_dim));
            } else if rows.is_empty() && shop.is_empty() {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("No installed font matches.").font(FontId::proportional(12.0)).color(p.txt_dim));
            }
        });
        if let Some(id) = install {
            pk.installing = Some((id, Job::spawn(ui.ctx(), move |_, _| crate::fontshop::install(id))));
        }
        ui.add_space(16.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::button(ui, p, Kind::Primary, None, "Close", true, 32.0, 18.0, 13.0).clicked() { close = true; }
        });
    });
    if modal.should_close() { close = true; }
    if let Some(v) = pick {
        pk.rescale = Some(advance(ctx, &app.m));
        crate::fonts::install(ctx, Some(&v));
        app.config.mono_font = (!v.is_empty()).then_some(v);
        if std::env::var_os("CSVFAB_SHOT").is_none() { app.config.save(); }
        ctx.request_repaint();
    }
    let done = close && pk.rescale.is_none() && pk.installing.is_none();
    if let Some(s) = said { app.toast(s, ctx); }
    if done {
        app.font_picker = None;
        // The samples' faces go: the fonts as they were, with the one chosen.
        crate::fonts::set_samples(ctx, Vec::new(), app.config.mono_font.as_deref());
    }
}
