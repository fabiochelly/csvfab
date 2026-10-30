//! The column inspector (docs/look.md, "Column inspector"): the column panel docked at the
//! right, on the column in hand (`GridState::focus_col`): name and type, key figures, a
//! histogram or the top values, what is worth a look, the panel's tools.
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
    add_open: bool,
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
        if self.job.is_none() && !tab.busy() && self.tried != Some(key) && !self.kept.iter().any(|k| k.0 == key) {
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
    if ui.available_width() < m.inspector_w + 300.0 { return actions; }
    let ctx = ui.ctx().clone();
    let col = tab.grid.focus_col.filter(|c| *c < ncols).or_else(|| (0..ncols).find(|c| !tab.grid.hidden.contains(c))).unwrap_or(0);
    tab.grid.focus_col = Some(col);
    let mut me = tab.inspector.take().unwrap_or_default();
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
fn section(ui: &mut Ui, text: &str, p: &Palette) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).font(FontId::new(11.5, FontFamily::Name(crate::fonts::UI_MEDIUM.into()))).color(p.txt_dim));
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
    let mut sub = kind_name[..1].to_uppercase() + &kind_name[1..];
    if let Some(c) = &counted {
        sub += &format!(" · {} distinct", grid::fmt_int(c.prof.distinct));
        if c.scope != c.total { sub += &format!(" · {} of {} rows", grid::fmt_int(c.scope), grid::fmt_int(c.total)); }
    }
    ui.painter().galley(pos2(r.min.x, r.min.y + 21.0), cut_line(ui, sub, ui_font(12.0), p.txt_mut, w), p.txt_mut);
    let xresp = ui.interact(xr, ui.scope_id().with("in-close"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Close the inspector");
    if xresp.hovered() { ui.painter().rect_filled(xr, 6.0, p.surf_2); }
    icons::cross(ui.painter(), Rect::from_center_size(xr.center(), vec2(14.0, 14.0)), 2.0, if xresp.hovered() { p.txt_hi } else { p.txt_mut });
    if xresp.clicked() { actions.push(Action::ToggleInspector); }

    egui::ScrollArea::vertical().id_salt("in-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(w);
        ui.add_space(10.0);
        match &counted {
            Some(c) => {
                figures(ui, tab, col, c, p, m);
                if !c.prof.hist.is_empty() {
                    spread(ui, c, p, m);
                } else if c.prof.distinct < c.prof.filled as usize {
                    top_values(ui, tab, col, c, p, m);
                }
                worth_a_look(ui, me, tab, col, c, p);
            }
            None => {
                let (r, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "Counting…", ui_font(12.0), p.txt_dim);
            }
        }
        section(ui, "Actions", p);
        let mut add_open = me.add_open;
        colpanel::tools(ui, tab, col, p, &mut add_open, false, actions);
        me.add_open = add_open;
        if tab.kinds.get(col) == Some(&Kind::Number) {
            let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
            let b1 = Rect::from_min_size(pos2(r.min.x + 2.0, r.min.y + 2.0), vec2(28.0, 26.0));
            colpanel::look_tools(ui, tab, col, b1, b1.translate(vec2(27.0, 0.0)), p);
        }
        ui.add_space(14.0);
    });
}

/// How a number column's figures are written, all alike: two decimals when its values have some.
fn figure(pr: &csvfab_engine::stats::PanelProfile) -> impl Fn(f64) -> String {
    let dec = [&pr.min, &pr.max].iter().any(|s| s.as_ref().is_some_and(|x| x.1.contains(['.', ','])));
    move |v| if dec { csvfab_engine::stats::fmt_num(v, 2, 2, true) } else { colpanel::num(v) }
}

/// The key figures, three to a line: a small label over its value.
fn figures(ui: &mut Ui, tab: &mut Tab, col: usize, c: &Counted, p: &Palette, m: &Metrics) {
    let pr = &c.prof;
    let share = pr.filled as f64 / (pr.filled + pr.empty).max(1) as f64;
    // Not 100 % while a cell is empty.
    let filled = if share < 1.0 && share >= 0.995 { format!("{:.1} %", (share * 1000.0).floor() / 10.0).replace('.', ",") } else { spark_ui::pct(share) };
    let mut items: Vec<(&str, String)> = vec![("Filled", filled), ("Distinct", grid::fmt_int(pr.distinct)), ("Empty", grid::fmt_int(pr.empty as usize))];
    let side = |s: &Option<(f64, String)>| s.as_ref().map(|x| grid::display(&x.1).into_owned());
    let n = figure(pr);
    match pr.kind {
        Some(Kind::Number) => {
            if let Some(v) = &pr.min { items.push(("Min", n(v.0))); }
            if let Some(v) = pr.median { items.push(("Median", n(v))); }
            if let Some(v) = pr.avg() { items.push(("Mean", n(v))); }
            if let Some(v) = &pr.max { items.push(("Max", n(v.0))); }
            if pr.count > 0 { items.push(("Sum", n(pr.sum))); }
        }
        Some(Kind::Date) => {
            if let Some(v) = side(&pr.min) { items.push(("First", v)); }
            if let Some(v) = &pr.date_median { items.push(("Median", grid::display(v).into_owned())); }
            if let Some(v) = &pr.date_mean { items.push(("Mean", v.clone())); }
            if let Some(v) = side(&pr.max) { items.push(("Last", v)); }
        }
        _ => {
            let chars = |s: &Option<(f64, String)>| s.as_ref().map(|x| format!("{} char{}", grid::fmt_int(x.0 as usize), if x.0 == 1.0 { "" } else { "s" }));
            if let Some(v) = chars(&pr.min) { items.push(("Shortest", v)); }
            if let Some(v) = chars(&pr.max) { items.push(("Longest", v)); }
        }
    }
    let w = ui.available_width();
    let (gap, line) = (8.0, 40.0);
    let cw = (w - 2.0 * gap) / 3.0;
    let lines = items.len().div_ceil(3);
    let (r, _) = ui.allocate_exact_size(vec2(w, lines as f32 * line), Sense::hover());
    let mut go = None;
    for (k, (label, value)) in items.iter().enumerate() {
        let cell = Rect::from_min_size(r.min + vec2((k % 3) as f32 * (cw + gap), (k / 3) as f32 * line), vec2(cw, line - 4.0));
        // The median goes to the row closest to it, as the panel's.
        let med = *label == "Median" && pr.median.is_some();
        let resp = med.then(|| ui.interact(cell, ui.scope_id().with("in-median"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand));
        if resp.as_ref().is_some_and(|r| r.hovered()) { ui.painter().rect_filled(cell.expand(2.0), 4.0, alpha(p.hi, 0.06)); }
        if resp.as_ref().is_some_and(|r| r.clicked()) { go = pr.median; }
        ui.painter().text(cell.min, Align2::LEFT_TOP, *label, ui_font(11.5), p.txt_dim);
        let g = cut_line(ui, value.clone(), mono_medium(12.5), p.txt_hi, cw);
        let cut = g.elided;
        let vr = Rect::from_min_size(cell.min + vec2(0.0, 17.0), g.size());
        ui.painter().galley(vr.min, g, p.txt_hi);
        if cut && ui.rect_contains_pointer(vr) { grid::tip(ui.ctx(), p, m, vr, value); }
    }
    if let Some(x) = go { colpanel::go_to_value(tab, col, x, &ui.ctx().clone()); }
}

/// Numbers or dates: the panel's histogram, tall, its ends written under it.
fn spread(ui: &mut Ui, c: &Counted, p: &Palette, m: &Metrics) {
    let w = ui.available_width();
    ui.add_space(8.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 92.0 + 6.0 + 14.0), Sense::hover());
    colpanel::histogram(ui, Rect::from_min_size(r.min, vec2(w, 92.0)), &c.prof, p, m);
    let (from, to) = (c.prof.hist.iter().find(|b| b.count > 0), c.prof.hist.iter().rfind(|b| b.count > 0));
    let font = mono_medium(11.0);
    let n = figure(&c.prof);
    let end = |s: &str| match c.prof.kind {
        Some(Kind::Number) => csvfab_engine::text::parse_number(s).map_or_else(|| grid::display(s).into_owned(), &n),
        _ => grid::display(s).into_owned(),
    };
    if let Some(b) = from { ui.painter().galley(pos2(r.min.x, r.max.y - 14.0), cut_line(ui, end(&b.first), font.clone(), p.txt_mut, w / 2.0 - 4.0), p.txt_mut); }
    if let Some(b) = to {
        let g = cut_line(ui, end(&b.last), font, p.txt_mut, w / 2.0 - 4.0);
        ui.painter().galley(pos2(r.max.x - g.size().x, r.max.y - 14.0), g, p.txt_mut);
    }
}

/// The most frequent values with a bar of their share; a click keeps the rows holding the value.
fn top_values(ui: &mut Ui, tab: &mut Tab, col: usize, c: &Counted, p: &Palette, m: &Metrics) {
    section(ui, "Top values", p);
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

/// A card that speaks of this column; empty cells have their own line.
fn about(it: &Insight, col: usize) -> bool {
    it.icon != InIcon::Empty && it.title.0.iter().any(|x| matches!(x, Part::Col(c) if *c == col))
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
        if look_line(ui, &format!("{} empty cell{}", grid::fmt_int(n), if n == 1 { "" } else { "s" }), Some("Show"), p) {
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
    let heads = tab.st().map(|s| s.headers()).unwrap_or_default();
    let report = me.report.as_ref().map(|r| r.1.clone());
    if let Some(r) = &report {
        for it in r.items.iter().filter(|it| about(it, col)).take(CARDS) {
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
