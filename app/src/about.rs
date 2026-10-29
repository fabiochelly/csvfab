//! About and What's new (10-welcome.js `openAbout`, 57-whats-new.js): the version, what
//! csvfab is, its links; the features each version brought, newest first. The notes are
//! assets/releases.js (the 1.x web app's format), read at build time.

use crate::grid::semibold;
use crate::icons::{self, Icon};
use crate::theme::{alpha, Palette};
use crate::widgets::{self, Kind};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Ui};
use egui::text::{LayoutJob, TextFormat};
use std::sync::OnceLock;

const RELEASES_JS: &str = include_str!("../assets/releases.js");
const SITE: &str = "https://github.com/fabiochelly/csvfab";
/// csvfab's version: the workspace's (Cargo.toml), the one a release is tagged with.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub struct Release {
    pub version: String,
    pub date: String,
    /// (title, one sentence)
    pub items: Vec<(String, String)>,
    /// The first published version (tagged so).
    pub first: bool,
}

/// The single-quoted strings of `s`, `\'` and `\\` unescaped, in order.
fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\'' { continue; }
        let mut v = String::new();
        while let Some(c) = it.next() {
            match c {
                '\\' => { if let Some(n) = it.next() { v.push(n); } }
                '\'' => break,
                c => v.push(c),
            }
        }
        out.push(v);
    }
    out
}

/// Parsed from `window.CSVFAB_RELEASES = [ { v: '…', date: '…', items: [ ['…', '…'], … ] }, … ]`.
pub fn releases() -> &'static [Release] {
    static R: OnceLock<Vec<Release>> = OnceLock::new();
    R.get_or_init(|| parse(RELEASES_JS))
}

fn parse(js: &str) -> Vec<Release> {
    let Some(start) = js.find("CSVFAB_RELEASES") else { return Vec::new() };
    js[start..].split("{ v: ").skip(1).filter_map(|chunk| {
        let mut q = quoted(chunk).into_iter();
        let version = q.next()?;
        let date = q.next()?;
        let rest: Vec<String> = q.collect();
        let items = rest.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect();
        let first = chunk.split("items").next().is_some_and(|head| head.contains("first: true"));
        Some(Release { version, date, items, first })
    }).collect()
}

/// "2026-10-10" → "10 October 2026".
fn day(d: &str) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mut p = d.split('-').map(|x| x.parse::<usize>().unwrap_or(0));
    match (p.next(), p.next(), p.next()) {
        (Some(y), Some(m), Some(dd)) if (1..=12).contains(&m) => format!("{dd} {} {y}", MONTHS[m - 1]),
        _ => d.to_string(),
    }
}

/// What a dialog's button asks for.
pub enum Pick {
    Close,
    News,
    About,
}

fn centered(ui: &mut Ui, text: &str, font: FontId, col: Color32) {
    let w = ui.available_width();
    let mut job = LayoutJob::simple(text.to_string(), font, col, w);
    job.halign = egui::Align::Center;
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.center().x, r.min.y), g, col);
}

fn link(ui: &mut Ui, p: &Palette, r: Rect, text: &str, url: &str) {
    let resp = ui.interact(r, egui::Id::unique(("about-link", url)), Sense::click());
    let col = if resp.hovered() { p.accent } else { p.accent_2 };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, FontId::proportional(12.5), col);
    if resp.hovered() {
        let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(12.5), col);
        let y = r.center().y + g.size().y / 2.0;
        ui.painter().hline(r.center().x - g.size().x / 2.0..=r.center().x + g.size().x / 2.0, y, egui::Stroke::new(1.0, alpha(col, 0.6)));
    }
    if resp.on_hover_text(url).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

pub fn about(ui: &mut Ui, p: &Palette) -> Option<Pick> {
    let mut pick = None;
    ui.add_space(4.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
    let disc = Rect::from_center_size(r.center(), vec2(64.0, 64.0));
    icons::paint(ui.painter(), Icon::Logo, disc, Color32::WHITE);
    ui.add_space(12.0);
    centered(ui, "csvfab", semibold(22.0), p.txt_hi);
    ui.add_space(4.0);
    let ver = format!("Version {}", version());
    centered(ui, &ver, FontId::proportional(12.5), p.txt_dim);
    ui.add_space(14.0);
    centered(ui, "A CSV editor that rewrites the file in place.\nOpen, filter, clean, save — nothing leaves your machine.", FontId::proportional(13.0), p.txt);
    ui.add_space(16.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_center_size(r.center(), vec2(120.0, 30.0))));
    if widgets::button(&mut child, p, Kind::Outline, Some(Icon::Sparkles), "What's new", true, 30.0, 12.0, 13.0).clicked() { pick = Some(Pick::News); }
    ui.add_space(16.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), Sense::hover());
    let third = w / 3.0;
    for (i, (text, url)) in [("Website & source", SITE.to_string()), ("Release notes", format!("{SITE}/releases")), ("Report a problem", format!("{SITE}/issues"))].iter().enumerate() {
        link(ui, p, Rect::from_min_size(pos2(r.min.x + third * i as f32, r.min.y), vec2(third, 20.0)), text, url);
    }
    ui.add_space(16.0);
    centered(ui, &format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH), FontId::proportional(11.5), p.txt_faint);
    ui.add_space(2.0);
    centered(ui, "© 2026 · MIT license · Fabio Chelly", FontId::proportional(11.5), p.txt_faint);
    let (r, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    link(ui, p, r, "fabio@chelly.net", "mailto:fabio@chelly.net");
    ui.add_space(18.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_center_size(r.center(), vec2(80.0, 32.0))));
    if widgets::button(&mut child, p, Kind::Primary, None, "Close", true, 32.0, 18.0, 13.0).clicked() { pick = Some(Pick::Close); }
    pick
}

pub fn news(ui: &mut Ui, p: &Palette) -> Option<Pick> {
    let mut pick = None;
    let w = ui.available_width();
    ui.label(egui::RichText::new("What's new in csvfab").font(semibold(15.0)).color(p.txt_hi));
    ui.add_space(2.0);
    ui.label(egui::RichText::new("The main features, version by version.").font(FontId::proportional(12.5)).color(p.txt_dim));
    ui.add_space(12.0);
    let rel = releases();
    egui::ScrollArea::vertical().max_height(440.0).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if rel.is_empty() {
            ui.label(egui::RichText::new("The release notes could not be read.").color(p.txt_dim));
        }
        for (i, r) in rel.iter().enumerate() {
            let tw = w - 16.0;
            // The version and its day; the one running and the first release tagged (as the web's).
            let (hr, _) = ui.allocate_exact_size(vec2(tw, 22.0), Sense::hover());
            let vg = ui.painter().layout_no_wrap(r.version.clone(), semibold(13.5), p.txt_hi);
            let vw = vg.size().x;
            ui.painter().galley(pos2(hr.min.x, hr.center().y - vg.size().y / 2.0), vg, p.txt_hi);
            let dg = ui.painter().layout_no_wrap(day(&r.date), FontId::proportional(12.0), p.txt_dim);
            let dw = dg.size().x;
            ui.painter().galley(pos2(hr.min.x + vw + 10.0, hr.center().y - dg.size().y / 2.0), dg, p.txt_dim);
            let mut tx = hr.min.x + vw + 10.0 + dw + 10.0;
            for (tag, on, col, bg) in [("Your version", r.version == version(), p.accent_2, alpha(p.prim, 0.16)), ("First release", r.first, p.violet_txt, alpha(p.violet, 0.16))] {
                if !on { continue; }
                let tg = ui.painter().layout_no_wrap(tag.into(), semibold(10.5), col);
                let tr = Rect::from_min_size(pos2(tx, hr.center().y - 9.0), vec2(tg.size().x + 14.0, 18.0));
                ui.painter().rect_filled(tr, 9.0, bg);
                ui.painter().galley(pos2(tr.min.x + 7.0, tr.center().y - tg.size().y / 2.0), tg, col);
                tx = tr.max.x + 6.0;
            }
            ui.add_space(4.0);
            for (title, how) in &r.items {
                let mut job = LayoutJob::default();
                job.append(title, 0.0, TextFormat { font_id: semibold(12.5), color: p.txt_strong, ..Default::default() });
                job.append(&format!("  {how}"), 0.0, TextFormat { font_id: FontId::proportional(12.5), color: p.txt_mut, ..Default::default() });
                job.wrap.max_width = tw - 14.0;
                let g = ui.painter().layout_job(job);
                let (lr, _) = ui.allocate_exact_size(vec2(tw, g.size().y + 6.0), Sense::hover());
                ui.painter().circle_filled(pos2(lr.min.x + 3.0, lr.min.y + 3.0 + 8.0), 2.0, alpha(p.prim, 0.8));
                ui.painter().galley(pos2(lr.min.x + 14.0, lr.min.y + 3.0), g, p.txt);
            }
            ui.add_space(12.0);
            if i + 1 < rel.len() {
                let y = ui.cursor().min.y;
                ui.painter().hline(ui.max_rect().min.x..=ui.max_rect().min.x + tw, y, egui::Stroke::new(1.0, p.brd));
                ui.add_space(12.0);
            }
        }
    });
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(130.0, 32.0), Sense::hover());
        link(ui, p, r, "Full release notes", &format!("{SITE}/releases"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if widgets::button(ui, p, Kind::Primary, None, "Close", true, 32.0, 18.0, 13.0).clicked() { pick = Some(Pick::Close); }
            if widgets::button(ui, p, Kind::Outline, None, "About", true, 32.0, 14.0, 13.0).clicked() { pick = Some(Pick::About); }
        });
    });
    pick
}

#[cfg(test)]
mod tests {
    #[test]
    fn releases_read() {
        let r = super::releases();
        assert!(r.len() > 10, "{}", r.len());
        assert!(r.iter().all(|x| !x.items.is_empty() && x.date.len() == 10));
        assert!(r.iter().flat_map(|x| &x.items).any(|(t, s)| t == "Totals row" && s.contains("bottom of the grid")));
        assert_eq!(super::day("2026-10-09"), "9 October 2026");
        assert!(super::version().split('.').count() == 3, "{}", super::version());
    }
}
