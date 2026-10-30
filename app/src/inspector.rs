//! The column inspector (docs/look.md, "Column inspector"): the column panel docked at the
//! right, on the column in hand (`GridState::focus_col`): name and type, key figures, a
//! histogram or the top values, what is worth a look; at its foot the panel's tools, named.
//!
//! Off, it does not exist: one flag read a frame. On, every figure comes from jobs: the
//! panel's count (`colpanel::count_job`, with the column's dictionary cache) once the column
//! has been rested on, the last few kept; the Insights report, shared with its dialog. A
//! frame only draws what is there.

use crate::app::drop_later;
use crate::colpanel::{self, Counted};
use crate::grid::{self, semibold, Action};
use crate::icons;
use crate::job::Job;
use crate::spark_ui;
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use crate::tools::insights;
use csvfab_engine::insights::{self as eng, Icon as InIcon, Insight, Part, Report};
use csvfab_engine::view::Kind;
use eframe::egui::{self, pos2, text::LayoutJob, vec2, Align2, Color32, FontFamily, FontId, Rect, RichText, Sense, Stroke, Ui};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a column must stay in hand before it is counted: keys running across the columns start nothing.
const REST: Duration = Duration::from_millis(150);
/// How long the table must stay as it is before Insights reads it again.
const QUIET: Duration = Duration::from_millis(1500);
/// Counts kept, for going back to a column at once.
const KEPT: usize = 4;
/// A count past this many values is not kept once out of view (its memory).
const KEPT_VALUES: usize = 1_000_000;
/// Values listed, cards listed.
const TOP: usize = 8;
const CARDS: usize = 4;

/// What a count was made from: the column, the bytes, the edits, the rows counted.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    col: usize,
    /// The file's field under that column: a column moved or inserted is another one.
    field: Option<usize>,
    base: usize,
    version: u64,
    rows: u64,
}

/// What the inspector keeps between frames, per tab, while it is on.
#[derive(Default)]
pub struct Inspector {
    /// What is wanted, and since when.
    want: Option<(Key, Instant)>,
    job: Option<(Key, Job<Option<Counted>>)>,
    /// The last count started (one that failed is not started again).
    tried: Option<Key>,
    kept: Vec<(Key, Arc<Counted>)>,
    /// The table's bytes and version, and since when it stands so (None: since before the
    /// inspector opened).
    table: Option<((usize, u64), Option<Instant>)>,
    report: Option<((usize, u64), Arc<Report>)>,
    report_job: Option<((usize, u64), Job<Option<Report>>)>,
    report_tried: Option<(usize, u64)>,
    /// A card's "Show the rows" being counted.
    show: Option<Job<Option<eng::Marks>>>,
    pub add_open: bool,
    /// The window left no room, and that was said.
    aside: bool,
    /// A shot of the loading state: no count starts.
    pub hold: bool,
}

impl Inspector {
    /// Picks up what ended, starts what is due.
    fn step(&mut self, tab: &mut Tab, col: usize, ctx: &egui::Context) {
        let Some(t) = &tab.table else { return };
        let st = t.st.clone();
        let (base, version) = (Arc::as_ptr(&st.base) as usize, t.version());
        // The rows the count reads: as `count_job` picks them (a sort alone counts the same).
        let sorted_only = tab.view.as_ref().is_none_or(|v| v.len() == st.len()) && tab.restrict.is_none();
        let key = Key { col, field: csvfab_engine::colcache::field_of(&st, col), base, version, rows: if sorted_only && !tab.has_values(col) { 0 } else { tab.generation } };
        if self.want.is_none_or(|w| w.0 != key) { self.want = Some((key, Instant::now())); }
        if let Some((k, j)) = &mut self.job {
            if let Some(r) = j.poll() {
                let k = *k;
                self.job = None;
                if let Some(c) = r { self.keep(k, Arc::new(c)); }
            }
        }
        if self.job.as_ref().is_some_and(|j| j.0 != key) { drop_later(self.job.take()); }
        if self.job.is_none() && !self.hold && !tab.busy() && self.tried != Some(key) && !self.kept.iter().any(|k| k.0 == key) {
            let rest = self.want.map_or(Duration::ZERO, |w| w.1.elapsed());
            if rest >= REST {
                self.tried = Some(key);
                self.job = colpanel::count_job(tab, col, ctx).map(|j| (key, j));
            } else {
                ctx.request_repaint_after(REST - rest + Duration::from_millis(5));
            }
        }
        // The report: the dialog's, else made here once the table stands still (a text file has none).
        if st.base.dialect.lines { return; }
        let at = (base, version);
        if self.table.as_ref().is_some_and(|t| t.0 != at) { self.table = Some((at, Some(Instant::now()))); }
        let since = self.table.get_or_insert((at, None)).1;
        if let Some((k, j)) = &mut self.report_job {
            if let Some(r) = j.poll() {
                let k = *k;
                self.report_job = None;
                if let Some(r) = r.map(Arc::new) {
                    if k.1 == version { insights::keep(tab.uid, version, r.clone()); }
                    self.report = Some((k, r));
                }
            }
        }
        if self.report_job.as_ref().is_some_and(|j| j.0 != at) { drop_later(self.report_job.take()); }
        if self.report.as_ref().is_none_or(|r| r.0 != at) && self.report_job.is_none() && self.report_tried != Some(at) {
            if let Some(r) = insights::kept(tab.uid, version) {
                self.report = Some((at, r));
            } else if !tab.busy() {
                let quiet = since.map_or(QUIET, |t| t.elapsed());
                if quiet >= QUIET {
                    self.report_tried = Some(at);
                    self.report_job = Some((at, Job::spawn(ctx, move |cancel, _| eng::insights(&st, cancel, None))));
                } else {
                    ctx.request_repaint_after(QUIET - quiet + Duration::from_millis(5));
                }
            }
        }
    }

    /// Nothing under way any more.
    pub fn settled(&self) -> bool {
        self.job.is_none() && self.report_job.is_none() && self.show.is_none()
    }

    /// The window has no room for it.
    pub fn is_aside(&self) -> bool {
        self.aside
    }

    /// The count of the column shown is there, for these rows.
    pub fn counted_now(&self) -> bool {
        self.want.is_some_and(|w| self.kept.iter().any(|k| k.0 == w.0))
    }

    fn keep(&mut self, k: Key, c: Arc<Counted>) {
        self.kept.retain(|x| x.0 != k);
        self.kept.insert(0, (k, c));
        // The heavy ones go first: only the one on screen stays past the limit.
        let mut n = 0;
        let (stay, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.kept).into_iter().partition(|x| {
            n += 1;
            n == 1 || (n <= KEPT && x.1.vc.len() <= KEPT_VALUES)
        });
        self.kept = stay;
        drop_later(gone);
    }

    /// What to show of `col`: its count for these rows, else the last one it had.
    pub fn counted(&self) -> Option<Arc<Counted>> {
        let k = self.want.map(|w| w.0);
        self.kept.iter().find(|x| Some(x.0) == k).or_else(|| self.kept.iter().find(|x| k.is_some_and(|k| (k.col, k.field, k.base) == (x.0.col, x.0.field, x.0.base)))).map(|x| x.1.clone())
    }
}

/// Docks the inspector at the right of the grid. Nothing when the window has no room for both.
pub fn panel(ui: &mut Ui, tab: &mut Tab, p: &Palette, m: &Metrics) -> Vec<Action> {
    let mut actions = Vec::new();
    let Some(ncols) = tab.st().map(|s| s.ncols).filter(|n| *n > 0) else { return actions };
    if ui.available_width() < m.inspector_w + 300.0 {
        // Said once: the switch is on and nothing shows.
        if let Some(me) = &mut tab.inspector { if me.aside { return actions; } }
        tab.inspector.get_or_insert_default().aside = true;
        tab.grid.notice = Some("No room for the column inspector beside the grid: widen the window.".into());
        return actions;
    }
    let ctx = ui.ctx().clone();
    let col = tab.grid.focus_col.filter(|c| *c < ncols).or_else(|| (0..ncols).find(|c| !tab.grid.hidden.contains(c))).unwrap_or(0);
    tab.grid.focus_col = Some(col);
    let mut me = tab.inspector.take().unwrap_or_default();
    me.aside = false;
    me.step(tab, col, &ctx);
    if let Some(r) = me.show.as_mut().and_then(|j| j.poll()) {
        me.show = None;
        if let Some(m) = r.filter(|m| !m.rows.is_empty()) {
            tab.grid.notice = Some(insights::mark_rows(tab, m, &ctx));
        } else {
            tab.grid.notice = Some("Those rows are no longer there.".into());
        }
    }
    let frame = egui::Frame::new().fill(p.surf).inner_margin(egui::Margin { left: 16, right: 6, top: 12, bottom: 0 });
    let resp = egui::Panel::right("inspector")
        .exact_size(m.inspector_w)
        .frame(frame)
        .show_separator_line(false)
        .show(ui, |ui| body(ui, &mut me, tab, col, p, m, &mut actions));
    let r = resp.response.rect;
    ui.painter().vline(ui.painter().round_to_pixel_center(r.min.x + 0.5), r.y_range(), Stroke::new(1.0, p.brd));
    tab.inspector = Some(me);
    actions
}

fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

/// A line of text cut at `max` wide: its galley.
fn cut_line(ui: &Ui, text: String, font: FontId, c: Color32, max: f32) -> Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text, font, c);
    job.wrap = egui::text::TextWrapping { max_width: max.max(4.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// A section's small head.
fn head(ui: &mut Ui, text: &str, p: &Palette) {
    ui.label(RichText::new(text).font(FontId::new(11.5, FontFamily::Name(crate::fonts::UI_MEDIUM.into()))).color(p.txt_dim));
}

/// The same, set off from what is above.
fn section(ui: &mut Ui, text: &str, p: &Palette) {
    ui.add_space(14.0);
    head(ui, text, p);
    ui.add_space(6.0);
}

#[allow(clippy::too_many_arguments)]
fn body(ui: &mut Ui, me: &mut Inspector, tab: &mut Tab, col: usize, p: &Palette, m: &Metrics, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    ui.expand_to_include_rect(ui.max_rect());
    let ctx = ui.ctx().clone();
    let w = ui.available_width() - 10.0;
    let counted = me.counted();
    // Name, type, and × at the right.
    let title = grid::titles(tab.st().unwrap(), tab).get(col).cloned().unwrap_or_default();
    let (r, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let xr = Rect::from_min_size(pos2(r.max.x - 26.0, r.min.y), vec2(26.0, 26.0));
    let g = cut_line(ui, title.clone(), semibold(15.0), p.txt_hi, w - 34.0);
    let cut = g.elided;
    let tr = Rect::from_min_size(r.min, g.size());
    ui.painter().galley(r.min, g, p.txt_hi);
    if cut && ui.rect_contains_pointer(tr) { grid::tip(&ctx, p, m, tr, &title); }
    let kind_name = grid::kind_style(tab, col, p).3;
    let mut sub = if counted.as_ref().is_some_and(|c| c.prof.filled == 0) { "Empty".to_string() } else { kind_name[..1].to_uppercase() + &kind_name[1..] };
    if let Some(c) = &counted {
        sub += &if c.scope != c.total { format!(" · {} of {} rows", grid::fmt_int(c.scope), grid::fmt_int(c.total)) } else { format!(" · {} row{}", grid::fmt_int(c.total), if c.total == 1 { "" } else { "s" }) };
    }
    ui.painter().galley(pos2(r.min.x, r.min.y + 21.0), cut_line(ui, sub, ui_font(12.0), p.txt_mut, w), p.txt_mut);
    let xresp = ui.interact(xr, ui.scope_id().with("in-close"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Close the inspector (Ctrl+Shift+I)");
    if xresp.hovered() { ui.painter().rect_filled(xr, 6.0, p.surf_2); }
    icons::cross(ui.painter(), Rect::from_center_size(xr.center(), vec2(14.0, 14.0)), 2.0, if xresp.hovered() { p.txt_hi } else { p.txt_mut });
    if xresp.clicked() { actions.push(Action::ToggleInspector); }

    // The tools stay at the foot, whatever is counted above them; a low window scrolls them.
    let foot_h = 8.0 + 15.0 + 6.0 + colpanel::NAMED_H + 10.0;
    let pinned = ui.available_height() >= foot_h + 160.0;
    let room = ui.available_height() - if pinned { foot_h } else { 0.0 };
    let stale = counted.is_some() && !me.counted_now();
    egui::ScrollArea::vertical().id_salt("in-scroll").auto_shrink([false, false]).max_height(room).show(ui, |ui| {
        ui.set_width(w);
        ui.add_space(10.0);
        // A count being made again: the last one, fainter.
        if stale { ui.multiply_opacity(0.6); }
        match &counted {
            Some(c) => {
                figures(ui, tab, col, Some(c), p, m);
                if !c.prof.hist.is_empty() {
                    spread(ui, tab, col, c, p, m);
                } else if c.prof.filled > 0 {
                    top_values(ui, tab, col, c, p, m);
                }
                worth_a_look(ui, me, tab, col, c, p);
            }
            None => {
                figures(ui, tab, col, None, p, m);
                waiting(ui, tab, col, p);
            }
        }
        if !pinned { foot(ui, me, tab, col, w, p, actions); }
        ui.add_space(14.0);
    });
    if pinned {
        let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
        ui.painter().hline(r.x_range(), ui.painter().round_to_pixel_center(r.min.y), Stroke::new(1.0, p.brd));
        ui.add_space(7.0);
        foot(ui, me, tab, col, w, p, actions);
    }
}

/// "Actions" and the tools.
fn foot(ui: &mut Ui, me: &mut Inspector, tab: &mut Tab, col: usize, w: f32, p: &Palette, actions: &mut Vec<Action>) {
    head(ui, "Actions", p);
    ui.add_space(6.0);
    let mut add_open = me.add_open;
    colpanel::tools_named(ui, tab, col, w, p, &mut add_open, actions);
    me.add_open = add_open;
}

/// While the first count is made: the chart's or the values' place, kept.
fn waiting(ui: &mut Ui, tab: &Tab, col: usize, p: &Palette) {
    let w = ui.available_width();
    if tab.kinds.get(col).is_some_and(|k| *k != Kind::Text) {
        ui.add_space(8.0);
        let (r, _) = ui.allocate_exact_size(vec2(w, CHART_H + 6.0 + 14.0), Sense::hover());
        let chart = Rect::from_min_size(r.min, vec2(w, CHART_H));
        ui.painter().rect_filled(chart, 4.0, alpha(p.surf_2, 0.6));
        ui.painter().text(chart.center(), Align2::CENTER_CENTER, "Counting…", ui_font(12.0), p.txt_dim);
    } else {
        section(ui, "Top values", p);
        for k in 0..3 {
            let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
            ui.add_space(2.0);
            ui.painter().rect_filled(r, 6.0, alpha(p.surf_2, 0.6));
            if k == 1 { ui.painter().text(r.center(), Align2::CENTER_CENTER, "Counting…", ui_font(12.0), p.txt_dim); }
        }
    }
    section(ui, "Worth a look", p);
    look_line(ui, "Looking…", None, p);
}

/// A figure: its label, its value in full, and shorter ways to write it when there is no room.
struct Fig {
    label: &'static str,
    full: String,
    short: Vec<String>,
}

/// The figures of a kind, line by line; their place depends on the kind alone, never on the
/// values, nothing moves as they change. Two to the line, a number or a date needs the room,
/// the sum alone; the counts share the left half. Before the count (`None`) the same
/// labels, no value yet.
fn lines(kind: Option<Kind>, c: Option<&Counted>) -> Vec<Vec<Fig>> {
    let fig = |label, v: Option<String>| Fig { label, full: v.unwrap_or_default(), short: Vec::new() };
    let pr = c.map(|c| &c.prof);
    let mut out = vec![vec![
        fig("Filled", pr.map(|pr| {
            let share = pr.filled as f64 / (pr.filled + pr.empty).max(1) as f64;
            // Not 100 % while a cell is empty.
            if share < 1.0 && share >= 0.995 { format!("{:.1} %", (share * 1000.0).floor() / 10.0).replace('.', ",") } else { spark_ui::pct(share) }
        })),
        Fig { label: "Empty", full: pr.map_or(String::new(), |pr| grid::fmt_int(pr.empty as usize)), short: pr.map_or(Vec::new(), |pr| vec![sci(pr.empty as f64)]) },
        Fig { label: "Distinct", full: pr.map_or(String::new(), |pr| grid::fmt_int(pr.distinct)), short: pr.map_or(Vec::new(), |pr| vec![sci(pr.distinct as f64)]) },
    ]];
    let side = |s: Option<&Option<(f64, String)>>| s.and_then(|s| s.as_ref()).map(|x| grid::display(&x.1).into_owned());
    match kind {
        Some(Kind::Number) => {
            // Its decimals go first, then its digits.
            let of = |label, v: Option<f64>| Fig { label, full: v.map_or(String::new(), |v| pr.map_or(String::new(), |pr| colpanel::figure(pr)(v))), short: v.map_or(Vec::new(), |v| vec![csvfab_engine::stats::fmt_num(v, 0, 0, true), sci(v)]) };
            out.push(vec![of("Min", pr.and_then(|pr| pr.min.as_ref()).map(|v| v.0)), of("Max", pr.and_then(|pr| pr.max.as_ref()).map(|v| v.0))]);
            out.push(vec![of("Median", pr.and_then(|pr| pr.median)), of("Mean", pr.and_then(|pr| pr.avg()))]);
            out.push(vec![of("Sum", pr.filter(|pr| pr.count > 0).map(|pr| pr.sum))]);
        }
        Some(Kind::Date) => {
            // A date and its time: the date alone where the time does not fit.
            let d = |label, v: Option<String>| Fig { label, short: v.as_deref().map(date_alone).into_iter().collect(), full: v.unwrap_or_default() };
            out.push(vec![d("First", side(pr.map(|pr| &pr.min))), d("Last", side(pr.map(|pr| &pr.max)))]);
            out.push(vec![d("Median", pr.and_then(|pr| pr.date_median.as_deref()).map(|v| grid::display(v).into_owned())), d("Mean", pr.and_then(|pr| pr.date_mean.clone()))]);
        }
        // An empty column has neither.
        None if c.is_some() => {}
        _ => {
            let chars = |s: Option<&Option<(f64, String)>>| s.and_then(|s| s.as_ref()).map(|x| format!("{} char{}", grid::fmt_int(x.0 as usize), if x.0 == 1.0 { "" } else { "s" }));
            out.push(vec![fig("Shortest", chars(pr.map(|pr| &pr.min))), fig("Longest", chars(pr.map(|pr| &pr.max)))]);
        }
    }
    out
}

/// A date without its time.
fn date_alone(v: &str) -> String {
    v.split([' ', 'T']).next().unwrap_or(v).to_string()
}

/// A number written short: `6,01e12`.
fn sci(v: f64) -> String {
    format!("{v:.2e}").replace('.', ",")
}

/// The key figures: a small label over its value. A value too long for its cell is written
/// shorter, or cut, and is whole in its tooltip.
fn figures(ui: &mut Ui, tab: &mut Tab, col: usize, c: Option<&Counted>, p: &Palette, m: &Metrics) {
    let kind = match c { Some(c) => c.prof.kind, None => tab.kinds.get(col).copied() };
    let lines = lines(kind, c);
    let w = ui.available_width();
    let (gap, line) = (8.0, 40.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, lines.len() as f32 * line), Sense::hover());
    let font = mono_medium(12.5);
    let mut go = None;
    let half = (w - gap) / 2.0;
    for (y, figs) in lines.iter().enumerate() {
        // Every line keeps the middle: (x, width) of each.
        let cells: &[(f32, f32)] = match figs.len() {
            3 => &[(0.0, 58.0), (66.0, half - 66.0), (half + gap, half)],
            2 => &[(0.0, half), (half + gap, half)],
            _ => &[(0.0, w)],
        };
        for (f, &(x, cw)) in figs.iter().zip(cells) {
            let cell = Rect::from_min_size(r.min + vec2(x, y as f32 * line), vec2(cw, line - 4.0));
            // The median goes to the row closest to it, as the panel's.
            let med = f.label == "Median" && c.is_some_and(|c| c.prof.median.is_some());
            let resp = med.then(|| ui.interact(cell, ui.scope_id().with("in-median"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand));
            if resp.as_ref().is_some_and(|r| r.hovered()) { ui.painter().rect_filled(cell.expand(2.0), 4.0, alpha(p.hi, 0.06)); }
            if resp.as_ref().is_some_and(|r| r.clicked()) { go = c.and_then(|c| c.prof.median); }
            ui.painter().text(cell.min, Align2::LEFT_TOP, f.label, ui_font(11.5), p.txt_dim);
            if c.is_none() {
                // Its place, while it is counted.
                ui.painter().rect_filled(Rect::from_min_size(cell.min + vec2(0.0, 19.0), vec2(cw.min(64.0), 10.0)), 3.0, alpha(p.surf_2, 0.8));
                continue;
            }
            let fits = |t: &String| ui.painter().layout_no_wrap(t.clone(), font.clone(), p.txt_hi).size().x <= cw;
            let text = std::iter::once(&f.full).chain(&f.short).find(|t| fits(t)).or(f.short.last()).unwrap_or(&f.full).clone();
            let g = cut_line(ui, text.clone(), font.clone(), p.txt_hi, cw);
            let whole = text == f.full && !g.elided;
            let vr = Rect::from_min_size(cell.min + vec2(0.0, 17.0), g.size());
            ui.painter().galley(vr.min, g, p.txt_hi);
            let what = match (whole, med) {
                (true, true) => Some("Go to the row closest to the median".to_string()),
                (false, true) => Some(format!("{}\nGo to the row closest to the median", f.full)),
                (false, false) => Some(f.full.clone()),
                _ => None,
            };
            if let Some(t) = what.filter(|_| ui.rect_contains_pointer(cell)) { grid::tip(ui.ctx(), p, m, cell, &t); }
        }
    }
    if let Some(x) = go { colpanel::go_to_value(tab, col, x, &ui.ctx().clone()); }
}

/// The histogram's height.
const CHART_H: f32 = 92.0;

/// Numbers or dates: the panel's histogram, tall, the column's true ends written under it and,
/// when its range is cut, what its end bars gather.
fn spread(ui: &mut Ui, tab: &mut Tab, col: usize, c: &Counted, p: &Palette, m: &Metrics) {
    let w = ui.available_width();
    let pr = &c.prof;
    ui.add_space(8.0);
    let note = colpanel::cut_note(ui, pr, ui_font(11.5), p.txt_dim, w);
    let (r, _) = ui.allocate_exact_size(vec2(w, CHART_H + 6.0 + 14.0 + note.as_ref().map_or(0.0, |g| 4.0 + g.size().y)), Sense::hover());
    colpanel::histogram(ui, tab, col, Rect::from_min_size(r.min, vec2(w, CHART_H)), pr, p, m);
    let (from, to) = (pr.hist.iter().find(|b| b.count > 0), pr.hist.iter().rfind(|b| b.count > 0));
    let font = mono_medium(11.0);
    let y = r.min.y + CHART_H + 6.0;
    // An end that does not fit: a date without its time, or cut.
    let end = |ui: &Ui, s: &str| {
        let mut t = colpanel::figure_of(pr, s);
        if pr.kind == Some(Kind::Date) && ui.painter().layout_no_wrap(t.clone(), font.clone(), p.txt_mut).size().x > w / 2.0 - 4.0 { t = date_alone(&t); }
        cut_line(ui, t, font.clone(), p.txt_mut, w / 2.0 - 4.0)
    };
    if let Some(b) = from { ui.painter().galley(pos2(r.min.x, y), end(ui, &b.first), p.txt_mut); }
    if let Some(b) = to {
        let g = end(ui, &b.last);
        ui.painter().galley(pos2(r.max.x - g.size().x, y), g, p.txt_mut);
    }
    if let Some(g) = note { ui.painter().galley(pos2(r.min.x, y + 14.0 + 4.0), g, p.txt_dim); }
}

/// The most frequent values with a bar of their share; a click keeps the rows holding the value.
fn top_values(ui: &mut Ui, tab: &mut Tab, col: usize, c: &Counted, p: &Palette, m: &Metrics) {
    section(ui, "Top values", p);
    if c.prof.distinct > 1 && c.prof.distinct == c.prof.filled as usize {
        look_line(ui, "Every value is different.", None, p);
        return;
    }
    let w = ui.available_width();
    let ctx = ui.ctx().clone();
    let max = c.order.first().map_or(1, |&i| c.vc.count(i as usize)).max(1) as f32;
    let font = ui_font(13.0);
    let cnt_font = mono_medium(12.0);
    let mut hit = None;
    for &i in c.order.iter().take(TOP) {
        let (v, n) = (c.vc.value(i as usize), c.vc.count(i as usize));
        let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
        ui.add_space(2.0);
        let one: HashSet<String> = [v.to_string()].into_iter().collect();
        let kept = spark_ui::values_kept(tab, col, &one);
        let pt = ui.painter();
        pt.rect_filled(r, 6.0, if resp.hovered() { p.surf_3 } else { p.surf_2 });
        pt.rect_filled(Rect::from_min_size(r.min, vec2((r.width() * n as f32 / max).max(2.0), r.height())), 6.0, alpha(p.accent, if kept { 0.32 } else { 0.14 }));
        let pills = tab.pills.get(col).and_then(|x| x.as_ref());
        let mut x = r.min.x + 8.0;
        if let Some(s) = pills.and_then(|pl| pl.slot(v)) {
            pt.circle_filled(pos2(x + m.dot_d / 2.0, r.center().y), m.dot_d / 2.0, p.pill_colors(s).0);
        }
        if pills.is_some() { x += m.dot_d + 6.0; }
        let cg = pt.layout_no_wrap(grid::fmt_int(n as usize), cnt_font.clone(), p.txt_mut);
        let cw = cg.size().x;
        pt.galley(pos2(r.max.x - 8.0 - cw, r.center().y - cg.size().y / 2.0), cg, p.txt_mut);
        let blank = v.trim().is_empty();
        let shown = if blank { if v.is_empty() { "(Empty)".to_string() } else { "(Blank)".to_string() } } else { grid::display(v).into_owned() };
        let g = cut_line(ui, shown.clone(), font.clone(), if blank { p.txt_mut } else { p.txt }, r.max.x - 8.0 - cw - 8.0 - x);
        let cut = g.elided;
        ui.painter().galley(pos2(x, r.center().y - g.size().y / 2.0), g, p.txt);
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            if cut { grid::tip(&ctx, p, m, r, &shown); }
        }
        if resp.clicked() { hit = Some((one, shown)); }
    }
    let more = c.order.len().saturating_sub(TOP);
    if more > 0 {
        ui.add_space(2.0);
        ui.label(RichText::new(format!("and {} more", grid::fmt_int(more))).font(ui_font(11.5)).color(p.txt_dim));
    }
    if let Some((one, shown)) = hit { spark_ui::pick_values(tab, col, Arc::new(one), &format!("“{shown}”"), &ctx); }
}

/// A line of "worth a look": its text, and at the right what a click on it does.
fn look_line(ui: &mut Ui, text: &str, action: Option<&str>, p: &Palette) -> bool {
    let w = ui.available_width();
    let ag = action.map(|a| ui.painter().layout_no_wrap(a.to_string(), FontId::new(12.5, FontFamily::Name(crate::fonts::UI_MEDIUM.into())), p.accent_2));
    let aw = ag.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    let mut job = LayoutJob::simple(text.to_string(), ui_font(12.5), p.txt, w - aw);
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    let g = ui.painter().layout_job(job);
    let h = (g.size().y + 8.0).max(26.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), if action.is_some() { Sense::click() } else { Sense::hover() });
    let hot = action.is_some() && resp.hovered();
    if hot { ui.painter().rect_filled(r.expand2(vec2(4.0, 0.0)), 4.0, p.surf_2); ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    ui.painter().galley(pos2(r.min.x, r.center().y - g.size().y / 2.0), g, p.txt);
    if let Some(a) = ag { ui.painter().galley(pos2(r.max.x - a.size().x, r.center().y - a.size().y / 2.0), a, p.accent_2); }
    resp.clicked()
}

/// A card that speaks of this column; empty cells have their own line, and so have values
/// far out once the histogram sets them apart (`far`: the card says the same of fewer rows).
fn about(it: &Insight, col: usize, far: bool) -> bool {
    it.icon != InIcon::Empty && !(far && it.is_far()) && it.title.0.iter().any(|x| matches!(x, Part::Col(c) if *c == col))
}

/// What the count and the Insights report say of the column, each with the way to its rows.
fn worth_a_look(ui: &mut Ui, me: &mut Inspector, tab: &mut Tab, col: usize, c: &Counted, p: &Palette) {
    section(ui, "Worth a look", p);
    let ctx = ui.ctx().clone();
    let mut any = false;
    let pr = &c.prof;
    if pr.empty > 0 {
        any = true;
        let n = pr.empty as usize;
        // Nothing but empty cells: there is nothing to tell them from.
        let what = if pr.filled == 0 { format!("Every cell is empty ({})", grid::fmt_int(n)) } else { format!("{} empty cell{}", grid::fmt_int(n), if n == 1 { "" } else { "s" }) };
        if look_line(ui, &what, (pr.filled > 0).then_some("Show"), p) {
            spark_ui::pick_values(tab, col, c.blanks.clone(), "the empty rows", &ctx);
        }
    }
    let odd = pr.filled - pr.count.min(pr.filled);
    if let Some(k) = pr.kind.filter(|k| *k != Kind::Text && odd > 0) {
        any = true;
        let n = odd as usize;
        let what = if k == Kind::Number { "numbers" } else { "dates" };
        if look_line(ui, &format!("{} value{} not {what}", grid::fmt_int(n), if n == 1 { "" } else { "s" }), Some("Show"), p) {
            spark_ui::pick_values(tab, col, c.odd.clone(), &format!("the values not {what}"), &ctx);
        }
    }
    // What the histogram sets apart, as a click on its end bar keeps it.
    let far = colpanel::far_bars(pr);
    for (bar, line) in &far {
        any = true;
        if look_line(ui, line, Some("Show"), p) { colpanel::keep_bars(tab, col, pr, *bar, *bar, &ctx); }
    }
    let heads = tab.st().map(|s| s.headers()).unwrap_or_default();
    let report = me.report.as_ref().map(|r| r.1.clone());
    if let Some(r) = &report {
        for it in r.items.iter().filter(|it| about(it, col, !far.is_empty())).take(CARDS) {
            any = true;
            let action = it.show.is_some().then_some(if me.show.is_some() { "Counting…" } else { "Show" });
            // Its column goes without saying here.
            let line = it.title.plain(&heads);
            let line = heads.get(col).and_then(|h| line.strip_prefix(h.as_str())).and_then(|l| l.strip_prefix(": ")).unwrap_or(&line);
            if look_line(ui, line, action, p) && me.show.is_none() {
                if let (Some(s), Some(st)) = (it.show.clone(), tab.st().cloned()) {
                    me.show = Some(Job::spawn(&ctx, move |cancel, _| eng::marks(&st, &s, cancel)));
                }
            }
        }
    }
    if !any {
        let done = report.is_some() || tab.st().is_some_and(|s| s.base.dialect.lines);
        look_line(ui, if done { "Nothing stands out." } else { "Looking…" }, None, p);
    }
}
