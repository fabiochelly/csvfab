//! "What I noticed" (Insights): the engine reads the rows (an even sample
//! past 100 000) and its detectors make cards — the key, columns that decide
//! others, values written otherwise, outliers, copies, hidden arithmetic,
//! empty or constant columns, stray spaces, irregular rows, garbled accents.
//! A card's "Show the rows" counts them again on every row (`insights::marks`,
//! in a job) and shows them alone, the cell at fault marked with what was
//! expected there. Nothing in the file changes. As 49-insights.js.
//!
//! The report is kept per tab and table version: reopening the dialog on
//! unchanged rows shows it at once.

use super::analysis_kit::{self as kit, Btn};
use super::{Command, Cx, Tool};
use crate::grid::semibold;
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::Marks;
use crate::theme::{alpha, Palette};
use csvfab_engine::insights::{self as eng, Act, Insight, Part, Report, Sev};
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Insights", group: "View", keys: None, enabled: |t| super::has_table(t) && !t.st().is_some_and(|st| st.base.dialect.lines), run: |cx| cx.out.push(super::Cmd::Tool(Box::new(Insights::new(cx.tab)))) }]
}

/// The last report: (tab uid, table version, report).
fn cache() -> &'static Mutex<Option<(u64, u64, Arc<Report>)>> {
    static C: std::sync::OnceLock<Mutex<Option<(u64, u64, Arc<Report>)>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

pub struct Insights {
    /// The rows to read, until the job starts (at the first frame, which has the context).
    start: Option<csvfab_engine::State>,
    job: Option<Job<Option<Report>>>,
    report: Option<Arc<Report>>,
    /// "Show the rows" being counted: the card's index and its job.
    show: Option<(usize, Job<Option<eng::Marks>>)>,
    version: u64,
    /// The cards appear one after the other once the report is in (as the web's `inin`).
    shown_at: f64,
}

impl Insights {
    pub fn new(tab: &crate::tab::Tab) -> Insights {
        let version = tab.table.as_ref().map_or(0, |t| t.version());
        let cached = kept(tab.uid, version);
        let start = if cached.is_none() { tab.st().cloned() } else { None };
        Insights { start, job: None, report: cached, show: None, version, shown_at: -1.0 }
    }

    /// What the summary says while reading, from the engine's progress (0–1000).
    fn step(p: usize) -> &'static str {
        const STEPS: [&str; 8] = ["Looking at the file…", "Looking at each column…", "Looking at how values are written…", "Looking for outliers…",
            "Looking for hidden arithmetic…", "Looking for columns that decide others…", "Looking for copied rows…", "Looking for the key…"];
        if p < 250 { "Reading the rows…" } else { STEPS[((p - 250) * 8 / 750).min(7)] }
    }
}

/// What a click in the dialog asks for, carried out after drawing.
enum Do {
    Close,
    GoCol(usize),
    Show(usize),
    Act(Act),
}

impl Tool for Insights {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if let Some(st) = self.start.take() {
            self.job = Some(Job::spawn(cx.ctx, move |cancel, progress| eng::insights(&st, cancel, Some(progress))));
        }
        // The progress bar moves while the job runs.
        if self.job.is_some() { cx.ctx.request_repaint_after(std::time::Duration::from_millis(50)); }
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            match r {
                Some(r) => {
                    let r = Arc::new(r);
                    keep(cx.tab.uid, self.version, r.clone());
                    self.report = Some(r);
                }
                None => return false,
            }
        }
        if let Some(m) = self.show.as_mut().and_then(|(_, j)| j.poll()) {
            self.show = None;
            let Some(m) = m else { return true };
            if m.rows.is_empty() { cx.toast("Those rows are no longer there."); return false; }
            show_marks(cx, m);
            return false;
        }
        if self.report.is_some() && self.shown_at < 0.0 { self.shown_at = cx.ctx.input(|i| i.time); }

        let p = cx.p;
        let ctx = cx.ctx.clone();
        let screen = ctx.content_rect();
        let w = 860.0_f32.min(screen.width() - 40.0);
        let h = (screen.height() * 0.86).min(900.0);
        let frame = egui::Frame::new().fill(p.surf).stroke(Stroke::new(1.0, p.brd)).corner_radius(8)
            .inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 14 })
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(128) });
        let headers = cx.tab.st().map(|s| s.headers()).unwrap_or_default();
        let name = cx.tab.name.clone();
        let mut todo = None;
        let modal = crate::widgets::modal(&ctx, egui::Id::unique("insights"), p, frame, |ui| {
            ui.set_width((w - 40.0).max(0.0));
            ui.set_height((h - 32.0).max(0.0));
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            // Head: the sparkles, the title, the summary.
            let progress = self.job.as_ref().map(|j| j.progress.load(Ordering::Relaxed));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::hover());
                icons::paint(ui.painter(), Icon::Sparkles, Rect::from_center_size(r.center(), vec2(20.0, 20.0)), p.violet_txt);
                ui.label(egui::RichText::new("What I noticed").font(semibold(16.0)).color(p.txt_hi));
                let sum = match (&self.report, progress) {
                    (Some(r), _) => r.summary(),
                    (None, Some(x)) => Self::step(x).to_string(),
                    _ => String::new(),
                };
                ui.label(egui::RichText::new(sum).font(kit::ui(12.5)).color(p.txt_mut));
            });
            // Progress: 2 px, prim → violet, there only while reading.
            ui.add_space(12.0);
            let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.0), Sense::hover());
            if let Some(x) = progress {
                ui.painter().rect_filled(bar, 1.0, p.surf_3);
                let fw = bar.width() * x as f32 / 1000.0;
                gradient(ui.painter(), Rect::from_min_size(bar.min, vec2(fw, 2.0)), p.prim, p.violet_txt);
            }
            ui.add_space(10.0);
            // The cards.
            let list_h = (ui.available_height() - 30.0 - 14.0).max(0.0);
            egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_width(ui.available_width() - 4.0);
                if let Some(r) = &self.report {
                    if r.items.is_empty() {
                        nothing(ui, p, &eng::Report::nothing(&name));
                    }
                    let t0 = self.shown_at;
                    let now = ui.input(|i| i.time);
                    for (k, it) in r.items.iter().enumerate() {
                        if k > 0 { ui.add_space(8.0); }
                        // Arrival: each card 45 ms after the previous, 380 ms to rise 10 px and fade in.
                        let a = (((now - t0 - k as f64 * 0.045) / 0.38).clamp(0.0, 1.0)) as f32;
                        if a < 1.0 { ui.ctx().request_repaint(); }
                        let ease = 1.0 - (1.0 - a).powi(3);
                        let counting = self.show.as_ref().map(|s| s.0) == Some(k);
                        if let Some(d) = card(ui, p, it, k, &headers, ease, counting, self.show.is_some()) { todo = Some(d); }
                    }
                }
            });
            kit::actions(ui, 14.0, |ui| {
                if kit::button(ui, p, "Close", Btn::Primary, true).clicked() { todo = Some(Do::Close); }
            });
        });
        // Screenshot hook `inshow:K`: card K's "Show the rows", once the cards are in.
        if todo.is_none() && self.show.is_none() && self.report.as_ref().is_some_and(|r| !r.items.is_empty()) {
            if let Some(k) = crate::shot::take("inshow").and_then(|s| s.parse::<usize>().ok()) { todo = Some(Do::Show(k)); }
        }
        let closed = modal.should_close();
        match todo {
            Some(Do::Close) => return false,
            Some(Do::GoCol(c)) => { go_col(cx, c); return false; }
            Some(Do::Act(a)) => { act(cx, a); return false; }
            Some(Do::Show(k)) => {
                if let (Some(r), Some(st)) = (&self.report, cx.tab.st()) {
                    if let Some(s) = r.items.get(k).and_then(|i| i.show.clone()) {
                        let st = st.clone();
                        self.show = Some((k, Job::spawn(cx.ctx, move |cancel, _| eng::marks(&st, &s, cancel))));
                    }
                }
            }
            None => {}
        }
        !closed
    }
}

/// The marked rows shown alone; each cell at fault says what was expected there.
fn show_marks(cx: &mut Cx, m: eng::Marks) {
    let said = mark_rows(cx.tab, m, cx.ctx);
    cx.toast(said);
}

/// The rows of a card (`marks`) marked and shown alone; what to tell of it. Also the inspector's.
pub(crate) fn mark_rows(tab: &mut crate::tab::Tab, m: eng::Marks, ctx: &egui::Context) -> String {
    let n = m.rows.len();
    let marks = to_marks(&m);
    tab.marks = Arc::new(Marks { dups: tab.marks.dups.clone(), ..marks });
    let label = format!("{} {}", kit::plural(n, "row", "rows"), m.label);
    tab.restrict_to(label.clone(), m.rows, ctx);
    // The ◆ chip counts the rows itself: its label is what they are, as written.
    tab.marks_label = m.label.clone();
    format!("{label} — shown alone; the chip shows every row again.")
}

/// The report kept for this tab and table version, if there is one.
pub(crate) fn kept(uid: u64, version: u64) -> Option<Arc<Report>> {
    cache().lock().ok().and_then(|c| c.as_ref().filter(|c| c.0 == uid && c.1 == version).map(|c| c.2.clone()))
}

/// Keeps a report made elsewhere (the inspector's): the dialog opens on it at once.
pub(crate) fn keep(uid: u64, version: u64, r: Arc<Report>) {
    if let Ok(mut c) = cache().lock() { *c = Some((uid, version, r)); }
}

/// The engine's marks as the grid's: rows, and (row, column) → the tooltip "Expected: 75011".
fn to_marks(m: &eng::Marks) -> Marks {
    let tip = if m.tip.is_empty() { "Expected" } else { m.tip.as_str() };
    Marks {
        rows: m.rows.iter().copied().collect(),
        cells: m.cells.iter().map(|(id, c, v)| ((*id, *c), format!("{tip}: {}", if v.is_empty() { "(empty)" } else { v }))).collect(),
        ..Default::default()
    }
}

fn go_col(cx: &mut Cx, c: usize) {
    let g = &mut cx.tab.grid;
    let row = g.cursor.map_or(0, |x| x.row);
    g.go_to_col(row, c);
}

fn act(cx: &mut Cx, a: Act) {
    match a {
        Act::ShowIrregular => {
            let Some(st) = cx.tab.st() else { return };
            let h = st.header as RowId;
            let ids: Vec<RowId> = st.base.odd.iter().map(|&(r, _)| r).filter(|&r| r >= h).collect();
            cx.tab.restrict_to("Irregular rows", ids, cx.ctx);
        }
        // Tools of other files, found by their command's name.
        Act::CleanUp => run_named(cx, "Clean up"),
        Act::RemoveDuplicates => run_named(cx, "Remove duplicates"),
    }
}

fn run_named(cx: &mut Cx, prefix: &str) {
    match super::commands().into_iter().find(|c| c.name.starts_with(prefix)) {
        Some(c) if (c.enabled)(cx.tab) => (c.run)(cx),
        _ => cx.toast(format!("{prefix} is not available here yet.")),
    }
}

fn gradient(pt: &egui::Painter, r: Rect, a: Color32, b: Color32) {
    if r.width() <= 0.0 { return; }
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), a);
    mesh.colored_vertex(r.right_top(), b);
    mesh.colored_vertex(r.right_bottom(), b);
    mesh.colored_vertex(r.left_bottom(), a);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    pt.add(mesh);
}

fn nothing(ui: &mut Ui, p: &Palette, text: &str) {
    let job = kit::rich_job(text, p.txt_mut, p.txt_hi, 13.0, ui.available_width() - 14.0 - 28.0);
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 36.0), Sense::hover());
    let ic = Rect::from_center_size(pos2(r.min.x + 14.0 + 9.0, r.center().y), vec2(18.0, 18.0));
    icons::paint(ui.painter(), Icon::InNone, ic, p.ok);
    ui.painter().galley(pos2(ic.max.x + 10.0, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, p.txt_mut);
}

fn card_icon(i: eng::Icon) -> Icon {
    match i {
        eng::Icon::Key => Icon::InKey,
        eng::Icon::Dep => Icon::InDep,
        eng::Icon::Shape => Icon::InShape,
        eng::Icon::Far => Icon::InFar,
        eng::Icon::Dup => Icon::InDup,
        eng::Icon::Rel => Icon::InRel,
        eng::Icon::Empty => Icon::DashedSquare,
        eng::Icon::Space => Icon::InSpace,
        eng::Icon::Warn => Icon::Warning,
    }
}

fn act_label(a: Act) -> &'static str {
    match a {
        Act::ShowIrregular => "Show them",
        Act::CleanUp => "Clean up…",
        Act::RemoveDuplicates => "Remove duplicates…",
    }
}

/// What a section of a card's text is: plain, a column (a chip, clickable), a value.
#[derive(Clone, Copy, PartialEq)]
enum Sect {
    Text,
    Col(usize),
    Val,
}

/// A card's text laid out: columns as chips (ui 500, padding 0 6), values in mono 12 (padding 0 4).
fn rich(ui: &Ui, p: &Palette, rich: &eng::Rich, headers: &[String], size: f32, color: Color32, line: f32, width: f32) -> (Arc<egui::Galley>, Vec<(Sect, usize)>) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    let mut sects = Vec::new();
    // Where each section ends, in characters: glyphs are matched to sections by it.
    let mut ends = Vec::new();
    let mut pad = 0.0;
    let fmt = |font: FontId, c: Color32| egui::TextFormat { font_id: font, color: c, line_height: Some(line), valign: egui::Align::Center, ..Default::default() };
    for part in &rich.0 {
        match part {
            Part::Text(s) => { job.append(s, pad, fmt(kit::ui(size), color)); sects.push(Sect::Text); ends.push(job.text.chars().count()); pad = 0.0; }
            Part::Col(c) => {
                // Not cut across lines: the chip is one block.
                let name = headers.get(*c).map_or_else(|| format!("column {}", c + 1), |h| h.replace(' ', "\u{a0}"));
                job.append(&name, pad + 6.0, fmt(kit::medium(size), p.txt_strong));
                sects.push(Sect::Col(*c));
                ends.push(job.text.chars().count());
                pad = 6.0;
            }
            Part::Val(v) => {
                job.append(&v.replace(' ', "\u{a0}"), pad + 4.0, fmt(kit::mono(12.0), p.txt_hi));
                sects.push(Sect::Val);
                ends.push(job.text.chars().count());
                pad = 4.0;
            }
        }
    }
    (ui.painter().layout_job(job), sects.into_iter().zip(ends).collect())
}

/// The boxes of each chip and value of a laid-out text (at `at`): (section, rect).
fn sect_rects(g: &egui::Galley, ends: &[usize], at: egui::Pos2) -> Vec<(usize, Rect)> {
    let mut out: Vec<(usize, Rect)> = Vec::new();
    let mut ci = 0;
    for row in &g.rows {
        let y0 = at.y + row.pos.y;
        let mut cur: Option<(usize, f32, f32)> = None;
        let mut flush = |c: Option<(usize, f32, f32)>| if let Some((s, a, b)) = c { out.push((s, Rect::from_min_max(pos2(a, y0), pos2(b, y0 + row.row.size.y)))); };
        for (k, gl) in row.row.glyphs.iter().enumerate() {
            let s = ends.partition_point(|&e| e <= ci + k);
            let x0 = at.x + row.pos.x + gl.pos.x;
            let x1 = x0 + gl.advance_width;
            match &mut cur {
                Some((cs, _, b)) if *cs == s => *b = x1,
                _ => { flush(cur.take()); cur = Some((s, x0, x1)); }
            }
        }
        flush(cur);
        ci += row.char_count_including_newline().0;
    }
    out
}

/// Draws a text with its chips; returns the column clicked.
fn draw_rich(ui: &mut Ui, p: &Palette, g: Arc<egui::Galley>, sects: &[(Sect, usize)], at: egui::Pos2, color: Color32, fade: f32) -> Option<usize> {
    let mut hit = None;
    let ends: Vec<usize> = sects.iter().map(|x| x.1).collect();
    for (s, r) in sect_rects(&g, &ends, at) {
        match sects.get(s).map(|x| x.0) {
            Some(Sect::Col(c)) => {
                let r = Rect::from_center_size(r.center(), vec2(r.width() + 12.0, 19.0));
                let resp = ui.interact(r, ui.scope_id().with(("in-c", at.x as i32, at.y as i32, s)), Sense::click());
                ui.painter().rect_filled(r, 5.0, alpha(p.accent, if resp.hovered() { 0.28 } else { 0.14 } * fade));
                if resp.clicked() { hit = Some(c); }
                if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
            }
            Some(Sect::Val) => {
                let r = Rect::from_center_size(r.center(), vec2(r.width() + 8.0, 17.0));
                ui.painter().rect_filled(r, 4.0, alpha(p.hi, 0.07 * fade));
            }
            _ => {}
        }
    }
    ui.painter().galley(pos2(kit::snap(ui, at.x), kit::snap(ui, at.y)), g, color);
    hit
}

/// One card: padding 12 × 14, radius 9, `surf_2`, 1 px `brd` and a 3 px left
/// edge (`warn` / `accent_2`); the icon 18 × 18, the title ui 13.5 `txt_hi`
/// (line 1.45), the detail ui 12.5 `txt_mut` (line 1.5, 4 below), the buttons
/// stacked at the right (padding 4 × 10, ui 12, 6 apart).
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut Ui, p: &Palette, it: &Insight, k: usize, headers: &[String], ease: f32, counting: bool, busy: bool) -> Option<Do> {
    let w = ui.available_width();
    let edge = if it.sev == Sev::Warn { p.warn } else { p.accent_2 };
    let mut labels: Vec<(String, Option<Do>)> = Vec::new();
    if it.show.is_some() { labels.push((if counting { "Counting…".into() } else { it.show_label.to_string() }, Some(Do::Show(k)))); }
    for a in &it.acts { labels.push((act_label(*a).into(), Some(Do::Act(*a)))); }
    let bw = labels.iter().map(|(l, _)| ui.painter().layout_no_wrap(l.clone(), kit::medium(12.0), p.txt).size().x + 20.0).fold(0.0_f32, f32::max).ceil();
    let acts_h = if labels.is_empty() { 0.0 } else { labels.len() as f32 * 26.0 + (labels.len() - 1) as f32 * 6.0 };
    let body_w = w - 3.0 - 14.0 - 14.0 - 18.0 - 12.0 - if labels.is_empty() { 0.0 } else { bw + 12.0 };
    let (tg, ts) = rich(ui, p, &it.title, headers, 13.5, p.txt_hi, 19.5, body_w);
    let sub = (!it.sub.0.is_empty()).then(|| rich(ui, p, &it.sub, headers, 12.5, p.txt_mut, 19.0, body_w));
    let body_h = tg.size().y + sub.as_ref().map_or(0.0, |s| 4.0 + s.0.size().y);
    let h = body_h.max(acts_h) + 24.0 + 2.0;
    let (slot, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let r = slot.translate(vec2(0.0, (1.0 - ease) * 10.0));
    let fade = ease;
    let pt = ui.painter().clone();
    pt.rect(r, 9.0, p.surf_2.gamma_multiply(fade), Stroke::new(1.0, p.brd.gamma_multiply(fade)), StrokeKind::Inside);
    // The left edge, 3 px, inside the rounded corners.
    let er = Rect::from_min_size(r.min, vec2(3.0, r.height()));
    pt.rect_filled(er, egui::CornerRadius { nw: 9, sw: 9, ne: 0, se: 0 }, edge.gamma_multiply(fade));
    let x0 = r.min.x + 3.0 + 14.0;
    let y0 = r.min.y + 1.0 + 12.0;
    icons::paint(&pt, card_icon(it.icon), Rect::from_min_size(pos2(x0, y0 + 1.0), vec2(18.0, 18.0)), edge.gamma_multiply(fade));
    let bx = x0 + 18.0 + 12.0;
    let mut todo = None;
    if let Some(c) = draw_rich(ui, p, tg.clone(), &ts, pos2(bx, y0), p.txt_hi.gamma_multiply(fade), fade) { todo = Some(Do::GoCol(c)); }
    if let Some((sg, ss)) = sub {
        if let Some(c) = draw_rich(ui, p, sg, &ss, pos2(bx, y0 + tg.size().y + 4.0), p.txt_mut.gamma_multiply(fade), fade) { todo = Some(Do::GoCol(c)); }
    }
    let mut by = y0;
    let ax = r.max.x - 1.0 - 14.0 - bw;
    for (l, d) in labels {
        let br = Rect::from_min_size(pos2(ax, by), vec2(bw, 26.0));
        let enabled = !busy;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br));
        if kit::button_sized(&mut child, p, &l, Btn::Outline, enabled, 26.0, (bw - text_w(ui, &l)) / 2.0, 12.0).clicked() { todo = d; }
        by += 32.0;
    }
    todo
}

fn text_w(ui: &Ui, s: &str) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), kit::medium(12.0), Color32::WHITE).size().x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_follow_the_engine_progress() {
        assert_eq!(Insights::step(0), "Reading the rows…");
        assert_eq!(Insights::step(250), "Looking at the file…");
        assert_eq!(Insights::step(999), "Looking for the key…");
        assert_eq!(Insights::step(1000), "Looking for the key…");
    }

    #[test]
    fn marks_carry_what_was_expected() {
        let m = eng::Marks { label: "x".into(), tip: "Usually".into(), rows: vec![3, 7], cells: vec![(3, 1, "75011".into()), (7, 1, String::new())] };
        let g = to_marks(&m);
        assert!(g.rows.contains(&3) && g.rows.contains(&7));
        assert_eq!(g.cells[&(3, 1)], "Usually: 75011");
        assert_eq!(g.cells[&(7, 1)], "Usually: (empty)");
        let m = eng::Marks { tip: String::new(), ..m };
        assert_eq!(to_marks(&m).cells[&(3, 1)], "Expected: 75011");
    }
}
