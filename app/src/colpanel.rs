//! The column panel, under a title's ▾ (docs/look.md, "Column panel"): tools,
//! profile, the values with their counts to filter by value.
//!
//! As the JS: the profile and the values come from one count over the rows the
//! other filters let through (this column's own value filter left out, so its
//! hidden values still show, unticked), in a `Job`; ticking values and Apply
//! hides the unticked ones (`Tab::hide_values`). The title's right click gives
//! the column's structure instead (`title_menu`). The column inspector (inspector.rs)
//! docks the same count, histogram and tools.

use crate::grid::{self, Action};
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use crate::widgets::{head, sep, width, Item};
use csvfab_engine::stats::{self, PanelProfile, ValueCounts};
use csvfab_engine::view::{Kind, SortKey};
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Rect, Sense, Stroke, StrokeKind, Ui};
use std::collections::HashSet;
use std::sync::Arc;

/// Values listed at once; the search reaches the others.
const MAX_LISTED: usize = 300;

/// Up to this many values, a search or a recount runs at once; past it, in a job.
const AT_ONCE: usize = 20_000;

/// The count the panel and the inspector show.
pub struct Counted {
    pub(crate) vc: ValueCounts,
    pub(crate) prof: PanelProfile,
    /// Value indices by count, most frequent first.
    pub(crate) order: Arc<Vec<u32>>,
    /// Rows counted, and the file's.
    pub(crate) scope: usize,
    pub(crate) total: usize,
    /// The values that read as empty, and those not of the column's kind, as written: what
    /// a value filter keeps to show them.
    pub(crate) blanks: Arc<HashSet<String>>,
    pub(crate) odd: Arc<HashSet<String>>,
}

/// The open panel's state (kept in `GridState::panel` while its popup is open).
pub struct Panel {
    pub col: usize,
    job: Option<Job<Option<Counted>>>,
    counted: Option<Arc<Counted>>,
    query: String,
    /// `order` filtered by the query, and the query it was for.
    matching: Arc<Vec<u32>>,
    matched: Option<String>,
    /// A search among many values under way, for this query (the list shown meanwhile is the last one).
    search: Option<(String, Job<Vec<u32>>)>,
    /// Values unticked, exactly as written (the filter compares the cells as they are); shared
    /// with the jobs reading it.
    excluded: Arc<HashSet<String>>,
    /// Bumped at every change of `excluded`.
    ex_ver: u64,
    /// Whether every value listed is ticked, for (the list, `ex_ver`) — never a look at every
    /// value each frame; the look under way among many values.
    all_on: Option<((usize, u64), bool)>,
    all_job: Option<((usize, u64), Job<bool>)>,
    /// "(Select all)" over many values, in the background: what the values listed become
    /// (ticked or not), and the job making the new `excluded`.
    toggling: Option<(bool, Job<Option<HashSet<String>>>)>,
    add_open: bool,
    focus: bool,
}

impl Panel {
    /// The values are counted (the list can be drawn).
    pub fn counted(&self) -> bool {
        self.counted.is_some()
    }

    /// The list matches the search typed, and the ticks are all known.
    pub fn searched(&self) -> bool {
        self.matched.as_deref() == Some(self.query.as_str()) && self.search.is_none() && self.toggling.is_none() && self.all_job.is_none()
    }

    fn new(tab: &Tab, col: usize, ctx: &egui::Context) -> Panel {
        // The values hidden now, shared (not copied: they may be millions).
        let excluded = tab.filter.hidden.iter().find(|(c, _)| *c == col).map(|(_, s)| s.clone()).unwrap_or_default();
        let mut p = Panel { col, job: None, counted: None, query: String::new(), matching: Arc::default(), matched: None, search: None, excluded, ex_ver: 0, all_on: None, all_job: None, toggling: None, add_open: false, focus: true };
        p.job = count_job(tab, col, ctx);
        p
    }

    fn set_excluded(&mut self, e: Arc<HashSet<String>>) {
        self.excluded = e;
        self.ex_ver += 1;
    }

    /// The list for the query typed: at once among few values, else from a job (the last list
    /// stays meanwhile; typing faster than it drops the stale one).
    fn update_search(&mut self, c: &Arc<Counted>, ctx: &egui::Context) {
        if let Some((q, j)) = &mut self.search {
            if let Some(m) = j.poll() {
                let q = std::mem::take(q);
                self.search = None;
                self.matching = Arc::new(m);
                self.matched = Some(q);
            }
        }
        if self.matched.as_deref() == Some(self.query.as_str()) || self.search.as_ref().is_some_and(|s| s.0 == self.query) {
            if self.matched.as_deref() == Some(self.query.as_str()) { self.search = None; }
            return;
        }
        if self.query.trim().is_empty() {
            self.matching = c.order.clone();
            self.matched = Some(self.query.clone());
            self.search = None;
        } else if c.order.len() <= AT_ONCE {
            self.matching = Arc::new(c.vc.search(&c.order, &self.query));
            self.matched = Some(self.query.clone());
            self.search = None;
        } else {
            let (c2, q) = (c.clone(), self.query.clone());
            self.search = Some((q.clone(), Job::spawn(ctx, move |_, _| c2.vc.search(&c2.order, &q))));
        }
    }

    /// Whether every value listed is ticked (None: being looked at).
    fn all_ticked(&mut self, c: &Arc<Counted>, ctx: &egui::Context) -> Option<bool> {
        let key = (Arc::as_ptr(&self.matching) as usize, self.ex_ver);
        if let Some((k, j)) = &mut self.all_job {
            if let Some(v) = j.poll() {
                self.all_on = Some((*k, v));
                self.all_job = None;
            }
        }
        if let Some((k, v)) = self.all_on { if k == key { return Some(v); } }
        if self.excluded.is_empty() {
            self.all_on = Some((key, true));
            return Some(true);
        }
        if self.matching.len() <= AT_ONCE {
            let v = self.matching.iter().all(|&i| !self.excluded.contains(c.vc.value(i as usize)));
            self.all_on = Some((key, v));
            return Some(v);
        }
        if self.all_job.as_ref().is_none_or(|j| j.0 != key) {
            let (c2, m, ex) = (c.clone(), self.matching.clone(), self.excluded.clone());
            self.all_job = Some((key, Job::spawn(ctx, move |_, _| {
                use rayon::prelude::*;
                !m.par_iter().with_min_len(4096).any(|&i| ex.contains(c2.vc.value(i as usize)))
            })));
        }
        None
    }

    /// Ticks (or unticks) every value listed: among many, the new set made in a job.
    fn tick_all(&mut self, c: &Arc<Counted>, on: bool, ctx: &egui::Context) {
        let apply = move |ex: &mut HashSet<String>, c: &Counted, m: &[u32]| {
            for &i in m {
                let v = c.vc.value(i as usize);
                if on { ex.remove(v); } else if !ex.contains(v) { ex.insert(v.to_string()); }
            }
        };
        if self.matching.len() <= AT_ONCE {
            let m = self.matching.clone();
            apply(Arc::make_mut(&mut self.excluded), c, &m);
            self.ex_ver += 1;
            return;
        }
        let (c2, m, ex) = (c.clone(), self.matching.clone(), self.excluded.clone());
        self.toggling = Some((on, Job::spawn(ctx, move |_, _| {
            let mut ex = Arc::try_unwrap(ex).unwrap_or_else(|a| (*a).clone());
            if !on { ex.reserve(m.len()); }
            apply(&mut ex, &c2, &m);
            Some(ex)
        })));
    }
}

/// The count in the background: the rows the other filters let through, this column's
/// value filters aside (the expression and Invert as the view has them).
pub(crate) fn count_job(tab: &Tab, col: usize, ctx: &egui::Context) -> Option<Job<Option<Counted>>> {
    let st = tab.st()?.clone();
    let own = tab.has_values(col);
    let view = tab.view.clone();
    let q = if own { tab.query(Some(col)).ok() } else { None };
    let sorted_only = tab.view.as_ref().is_some_and(|v| v.len() == st.len()) && tab.restrict.is_none();
    Some(Job::spawn(ctx, move |cancel, _| {
        let all = || -> Vec<RowId> { (0..st.len()).map(|p| st.data_id(p)).collect() };
        let ids: Arc<Vec<RowId>> = match (q, view) {
            (Some(q), _) => Arc::new(q.run(&st, None, cancel)?),
            (None, Some(v)) if !sorted_only => v,
            _ => Arc::new(all()),
        };
        // The column's dictionary, built once (kept for the file): this count and the next
        // ones (another filter, the panel opened again) count its codes instead of hashing
        // every cell. A column that does not repeat itself gets none.
        if !st.base.partial { csvfab_engine::colcache::ensure(&st, col, csvfab_engine::colcache::Need::Dict, cancel); }
        let vc = stats::value_counts(&st, &ids, col, false, cancel)?;
        let prof = stats::panel_profile(&vc);
        let order = Arc::new(vc.by_count());
        let mut blanks = HashSet::new();
        let mut odd = HashSet::new();
        let typed = prof.kind.filter(|k| *k != Kind::Text && prof.count < prof.filled);
        for (v, _) in vc.iter() {
            if v.trim().is_empty() {
                blanks.insert(v.to_string());
            } else if typed.is_some_and(|k| stats::key_of(k, v.trim()).is_none_or(f64::is_nan)) {
                odd.insert(v.to_string());
            }
        }
        Some(Counted { vc, prof, order, scope: ids.len(), total: st.len(), blanks: Arc::new(blanks), odd: Arc::new(odd) })
    }))
}

/// A small outline button of the tools row, `icon` drawn 14 px (or `text`); joined to its
/// neighbours by `corners` (left, right rounded).
#[allow(clippy::too_many_arguments)]
fn tool(ui: &mut Ui, p: &Palette, r: Rect, id: Id, draw: &dyn Fn(&egui::Painter, Rect, Color32), tip: &str, enabled: bool, on: bool, danger: bool, corners: (bool, bool)) -> egui::Response {
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    let hot = enabled && resp.hovered();
    let rad = |b: bool| if b { 6 } else { 0 };
    let cr = CornerRadius { nw: rad(corners.0), sw: rad(corners.0), ne: rad(corners.1), se: rad(corners.1) };
    let (fill, brd, fg) = if on {
        (alpha(p.accent, 0.12), p.prim, p.accent_2)
    } else if hot && danger {
        (p.surf_2, alpha(p.danger, 0.6), p.danger_txt)
    } else if hot {
        (p.surf_2, p.brd_2, p.txt)
    } else {
        (Color32::TRANSPARENT, p.brd, p.txt)
    };
    let fg = if enabled { fg } else { fg.gamma_multiply(0.45) };
    let pt = ui.painter();
    pt.rect(r, cr, fill, Stroke::new(1.0, if enabled { brd } else { brd.gamma_multiply(0.45) }), StrokeKind::Inside);
    draw(pt, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), fg);
    if hot {
        grid::tip(ui.ctx(), p, &crate::theme::Metrics::default(), r, tip);
    }
    resp
}

fn ic(icon: Icon) -> impl Fn(&egui::Painter, Rect, Color32) {
    move |pt, r, c| icons::paint(pt, icon, r, c)
}

/// The tools row: move, split / merge, convert, sort, freeze, insert / delete. `popup`: in the
/// panel, which a tool closes (the inspector stays); `add_open`: the insert menu is open.
pub(crate) fn tools(ui: &mut Ui, tab: &mut Tab, col: usize, p: &Palette, add_open: &mut bool, popup: bool, actions: &mut Vec<Action>) {
    let close = |ui: &Ui| if popup { ui.close(); };
    let ncols = tab.st().map_or(0, |s| s.ncols);
    let w = ui.available_width();
    // The groups' rects, wrapped onto more lines when the row is full.
    const SIZES: [usize; 7] = [2, 2, 1, 2, 1, 2, 2];
    let left = ui.cursor().min.x + 2.0;
    let mut placed: Vec<(f32, usize, usize)> = Vec::new();
    let (mut x, mut line) = (left, 0usize);
    for n in SIZES {
        let gw = n as f32 * 25.0 + 1.0;
        if x + gw > left + w - 4.0 && x > left { x = left; line += 1; }
        placed.push((x, line, n));
        x += gw + 4.0;
    }
    let (row, _) = ui.allocate_exact_size(vec2(w, 2.0 + (line + 1) as f32 * 28.0 - 4.0 + 6.0), Sense::hover());
    let slots: Vec<Vec<Rect>> = placed
        .iter()
        .map(|&(x, li, n)| (0..n).map(|k| Rect::from_min_size(pos2(x + k as f32 * 25.0, row.min.y + 2.0 + li as f32 * 28.0), vec2(26.0, 24.0))).collect())
        .collect();
    let mut groups = slots.into_iter();
    let mut at = |_n: usize, _x: &mut f32| -> Vec<Rect> { groups.next().unwrap_or_default() };
    let sid = ui.scope_id();
    let id = |s: &str| sid.with(("cp-tool", s));
    let g = at(2, &mut x);
    let (idl, idr) = (id("left"), id("right"));
    let ctx = ui.ctx().clone();
    if tool(ui, p, g[0], idl, &ic(Icon::ChevronLeft), "Move left", col > 0, false, false, (true, false)).clicked() {
        grid::move_column(tab, col, col - 1, &ctx);
        close(ui);
    }
    if tool(ui, p, g[1], idr, &ic(Icon::ChevronRight), "Move right", col + 1 < ncols, false, false, (false, true)).clicked() {
        grid::move_column(tab, col, col + 1, &ctx);
        close(ui);
    }
    let g = at(2, &mut x);
    if tool(ui, p, g[0], id("split"), &ic(Icon::Split), "Split", true, false, false, (true, false)).clicked() { actions.push(Action::SplitColumn(col)); close(ui); }
    if tool(ui, p, g[1], id("merge"), &ic(Icon::Merge), "Merge", true, false, false, (false, true)).clicked() { actions.push(Action::MergeColumns(col)); close(ui); }
    let g = at(1, &mut x);
    if tool(ui, p, g[0], id("convert"), &ic(Icon::ConvertColumn), "Convert: rewrite its dates, numbers or phones into one format (the values change)", true, false, false, (true, true)).clicked() {
        actions.push(Action::ConvertColumn(col));
        close(ui);
    }
    let g = at(2, &mut x);
    let sorted = tab.sort.first().filter(|k| k.col == col).map(|k| k.desc);
    let up = |pt: &egui::Painter, r: Rect, c: Color32| icons::arrow(pt, r, true, 2.0, c);
    let down = |pt: &egui::Painter, r: Rect, c: Color32| icons::arrow(pt, r, false, 2.0, c);
    if tool(ui, p, g[0], id("asc"), &up, "Sort ascending", true, sorted == Some(false), false, (true, false)).clicked() { actions.push(Action::Sort(vec![SortKey { col, desc: false }])); close(ui); }
    if tool(ui, p, g[1], id("desc"), &down, "Sort descending", true, sorted == Some(true), false, (false, true)).clicked() { actions.push(Action::Sort(vec![SortKey { col, desc: true }])); close(ui); }
    let g = at(1, &mut x);
    let frozen = tab.grid.frozen == col + 1;
    if tool(ui, p, g[0], id("freeze"), &ic(Icon::FreezeColumn), if frozen { "Unfreeze the columns" } else { "Freeze up to this column: it stays in view when scrolling sideways" }, true, frozen, false, (true, true)).clicked() {
        grid::freeze_to(tab, col);
        close(ui);
    }
    let g = at(2, &mut x);
    let plus = |pt: &egui::Painter, r: Rect, c: Color32| icons::plus(pt, r, 2.0, c);
    let minus = |pt: &egui::Painter, r: Rect, c: Color32| { pt.hline(r.x_range().shrink(2.5), (r.center().y * 2.0).round() / 2.0, Stroke::new(1.6, c)); };
    if tool(ui, p, g[0], id("add"), &plus, "Insert a column", true, *add_open, false, (true, false)).clicked() {
        *add_open = !*add_open;
    }
    if tool(ui, p, g[1], id("del"), &minus, "Delete this column", true, false, true, (false, true)).clicked() { actions.push(Action::DeleteColumns(vec![col])); close(ui); }
    let gp = at(2, &mut x);
    if tool(ui, p, gp[0], id("group"), &ic(Icon::GroupBy), "Group by this column…", true, false, false, (true, false)).clicked() { actions.push(Action::Tool(crate::tools::group::open(tab, Some(col)))); close(ui); }
    if tool(ui, p, gp[1], id("pivot"), &ic(Icon::Pivot), "Pivot table by this column…", true, false, false, (false, true)).clicked() { actions.push(Action::Tool(crate::tools::pivot::open(tab, Some(col)))); close(ui); }
    if *add_open {
        // The insert menu: under the + −, right-aligned.
        let pos = pos2(g[1].max.x - 180.0, g[1].max.y + 4.0);
        egui::Area::new(ui.scope_id().with("cp-add")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
            crate::theme::menu_frame(p, &Metrics::default()).inner_margin(4).show(ui, |ui| {
                width(ui, 180.0);
                if Item::new("Insert a column before").icon(Icon::InsertBefore).show(ui, p).clicked() { actions.push(Action::InsertColumn(col)); *add_open = false; close(ui); }
                if Item::new("Insert a column after").icon(Icon::InsertAfter).show(ui, p).clicked() { actions.push(Action::InsertColumn(col + 1)); *add_open = false; close(ui); }
            });
        });
    }
}

/// A number's two looks: data bars and the colour scale, in `b1` and `b2`. Whether one was toggled.
pub(crate) fn look_tools(ui: &mut Ui, tab: &mut Tab, col: usize, b1: Rect, b2: Rect, p: &Palette) -> bool {
    let ctx = ui.ctx().clone();
    let (bars_on, heat_on) = (tab.grid.bars.contains_key(&col), tab.grid.heat.contains_key(&col));
    let mut hit = false;
    if tool(ui, p, b1, ui.scope_id().with("cp-bars"), &ic(Icon::DataBars), "Data bars: each value drawn as a bar at the foot of its cell", true, bars_on, false, (true, false)).clicked() {
        grid::toggle_look(tab, col, false, &ctx);
        hit = true;
    }
    if tool(ui, p, b2, ui.scope_id().with("cp-heat"), &ic(Icon::ColorScale), "Colour scale: each cell tinted by its value", true, heat_on, false, (false, true)).clicked() {
        grid::toggle_look(tab, col, true, &ctx);
        hit = true;
    }
    hit
}

/// A number as the panel writes it: `1 234,5`, two decimals at most.
pub(crate) fn num(x: f64) -> String {
    stats::fmt_num(x, 0, 2, true)
}

/// The profile: type badge, distinct values, bounds, sum / average / median, histogram.
/// Returns the median when it is clicked (the grid then goes to the row closest to it).
fn profile(ui: &mut Ui, c: &Counted, p: &Palette, m: &Metrics) -> Option<f64> {
    let mut go = None;
    let pr = &c.prof;
    let w = ui.available_width();
    let ui12 = FontId::proportional(12.0);
    let b12 = grid::semibold(12.0);
    let line_h = 19.0;
    let kind = pr.kind.unwrap_or(Kind::Text);
    let mut rows: Vec<Vec<(String, String, Option<String>)>> = Vec::new();   // per line, 1 or 2 (key, value, after)
    if let (Some(lo), Some(hi)) = (&pr.min, &pr.max) {
        match kind {
            Kind::Number => {
                rows.push(vec![("min".into(), lo.1.clone(), None), ("max".into(), hi.1.clone(), None)]);
                if let Some(avg) = pr.avg() {
                    rows.push(vec![("sum".into(), num(pr.sum), None), ("avg".into(), num(avg), None)]);
                }
                if let Some(med) = pr.median { rows.push(vec![("med".into(), num(med), None)]); }
            }
            Kind::Date => {
                rows.push(vec![("min".into(), lo.1.clone(), None)]);
                rows.push(vec![("max".into(), hi.1.clone(), None)]);
            }
            Kind::Text => {
                let chars = |n: f64| format!("({} char{})", grid::fmt_int(n as usize), if n == 1.0 { "" } else { "s" });
                rows.push(vec![("min".into(), lo.1.clone(), Some(chars(lo.0)))]);
                rows.push(vec![("max".into(), hi.1.clone(), Some(chars(hi.0)))]);
            }
        }
    }
    let note = (c.scope != c.total).then(|| format!("{} of {} rows (filtered)", grid::fmt_int(c.scope), grid::fmt_int(c.total)));
    let hist = !pr.hist.is_empty();
    let h = 8.0 + 20.0 + 4.0 + note.as_ref().map_or(0.0, |_| 18.0) + rows.len() as f32 * line_h + if hist { 8.0 + 44.0 } else { 0.0 } + 8.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let pt = ui.painter();
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let x0 = r.min.x + 10.0;
    let inner_w = w - 20.0;
    let mut y = r.min.y + 8.0;
    // The badge and "5 distinct values on 8 261".
    let (label, fg, base) = match kind {
        Kind::Number => ("Numbers", p.ok_txt, p.ok_rgb),
        Kind::Date => ("Dates", p.accent_2, p.accent),
        Kind::Text => ("Text", p.violet_txt, p.violet),
    };
    let bg = pt.layout_no_wrap(label.into(), grid::semibold(11.0), fg);
    let br = Rect::from_min_size(pos2(x0, y), vec2(bg.size().x + 16.0, 20.0));
    pt.rect(br, 9.0, alpha(base, 0.13), Stroke::new(1.0, alpha(base, 0.4)), StrokeKind::Inside);
    pt.galley(pos2(snap(br.min.x + 8.0), snap(br.center().y - bg.size().y / 2.0)), bg, fg);
    let mut job = egui::text::LayoutJob::default();
    let fmt = |f: &FontId, c: Color32| egui::TextFormat::simple(f.clone(), c);
    job.append(&grid::fmt_int(pr.distinct), 0.0, fmt(&b12, p.txt_hi));
    job.append(&format!(" distinct value{} on ", if pr.distinct == 1 { "" } else { "s" }), 0.0, fmt(&ui12, p.txt_mut));
    job.append(&grid::fmt_int(pr.filled as usize), 0.0, fmt(&b12, p.txt_hi));
    job.wrap = egui::text::TextWrapping { max_width: inner_w - br.width() - 8.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = pt.layout_job(job);
    pt.galley(pos2(br.max.x + 8.0, snap(br.center().y - g.size().y / 2.0)), g, p.txt_mut);
    y += 20.0 + 4.0;
    if let Some(n) = note {
        pt.text(pos2(x0, snap(y + 9.0)), Align2::LEFT_CENTER, n, FontId::proportional(11.5), p.txt_dim);
        y += 18.0;
    }
    for line in &rows {
        let col_w = if line.len() == 2 { (inner_w - 12.0) / 2.0 } else { inner_w };
        // The median: a click goes to the first row closest to it (as the web's).
        if let (Some(med), true) = (pr.median, line.first().is_some_and(|l| l.0 == "med")) {
            let lr = Rect::from_min_size(pos2(x0 - 4.0, y), vec2(inner_w + 8.0, line_h));
            let resp = ui.interact(lr, ui.scope_id().with("cp-median"), Sense::click());
            if resp.hovered() { ui.painter().rect_filled(lr, 4.0, alpha(p.hi, 0.06)); }
            if resp.on_hover_text("Go to the row closest to the median").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { go = Some(med); }
        }
        let pt = ui.painter();
        for (k, (key, val, after)) in line.iter().enumerate() {
            let kx = x0 + k as f32 * (col_w + 12.0);
            let cy = snap(y + line_h / 2.0);
            pt.text(pos2(kx, cy), Align2::LEFT_CENTER, key, ui12.clone(), p.txt_dim);
            let after_g = after.as_ref().map(|a| pt.layout_no_wrap(a.clone(), ui12.clone(), p.txt_dim));
            let aw = after_g.as_ref().map_or(0.0, |g| g.size().x + 6.0);
            let mut vj = egui::text::LayoutJob::simple_singleline(grid::display(val).into_owned(), b12.clone(), p.txt_hi);
            vj.wrap = egui::text::TextWrapping { max_width: (col_w - 38.0 - aw).max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let vg = pt.layout_job(vj);
            let vw = vg.size().x;
            pt.galley(pos2(kx + 38.0, snap(cy - vg.size().y / 2.0)), vg, p.txt_hi);
            if let Some(ag) = after_g {
                pt.galley(pos2(kx + 38.0 + vw + 4.0, snap(cy - ag.size().y / 2.0)), ag, p.txt_dim);
            }
        }
        y += line_h;
    }
    if hist {
        y += 8.0;
        histogram(ui, Rect::from_min_size(pos2(x0, y), vec2(inner_w, 44.0)), pr, p, m);
    }
    go
}

/// Numbers or dates spread in `hr`: bars 1 apart, tops rounded; each says its values on hover.
pub(crate) fn histogram(ui: &mut Ui, hr: Rect, pr: &PanelProfile, p: &Palette, m: &Metrics) {
    let n = pr.hist.len() as f32;
    let bw = (hr.width() - (n - 1.0)) / n;
    let max = pr.hist.iter().map(|b| b.count).max().unwrap_or(1).max(1) as f32;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    for (i, b) in pr.hist.iter().enumerate() {
        let slot = Rect::from_min_size(pos2(hr.min.x + i as f32 * (bw + 1.0), hr.min.y), vec2(bw, hr.height()));
        if b.count == 0 { continue; }
        let bh = (b.count as f32 / max * 100.0).max(4.0) / 100.0 * hr.height();
        let hot = pointer.is_some_and(|q| slot.contains(q)) && ui.rect_contains_pointer(slot);
        let bar = Rect::from_min_max(pos2(slot.min.x, slot.max.y - bh), slot.max);
        ui.painter().rect_filled(bar, CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, p.prim.gamma_multiply(if hot { 1.0 } else { 0.75 }));
        if hot {
            let vals = if b.first == b.last { b.first.clone() } else { format!("{} – {}", b.first, b.last) };
            grid::tip(ui.ctx(), p, m, slot, &format!("{vals} · {} value{}", grid::fmt_int(b.count as usize), if b.count == 1 { "" } else { "s" }));
        }
    }
}

/// The first row shown whose number in `col` is closest to `x`, selected in the middle of
/// the view (the median may be no cell's value): looked for in the background
/// (`GridState::goto_job`), every row shown read.
pub(crate) fn go_to_value(tab: &mut Tab, col: usize, x: f64, ctx: &egui::Context) {
    let Some(st) = tab.st().cloned() else { return };
    let view = tab.view.clone();
    let n = tab.shown();
    tab.grid.goto_job = Some((Job::spawn(ctx, move |cancel, _| {
        let id = |p: usize| view.as_ref().map_or_else(|| st.data_id(p), |v| v[p]);
        let found = stats::closest(&st, n, id, col, x, cancel)?;
        found.map(|row| (row, col))
    }), col));
}

/// A checkbox 16 × 16: `bg_deep`, 1 px `brd_2`, radius 4; checked: `prim` with a white check.
pub fn checkbox(pt: &egui::Painter, r: Rect, on: bool, hot: bool, p: &Palette) {
    if on {
        pt.rect_filled(r, 4.0, p.prim);
        icons::paint(pt, Icon::Checkbox, r, Color32::WHITE);
    } else {
        pt.rect(r, 4.0, p.bg_deep, Stroke::new(1.0, if hot { p.txt_dim } else { p.brd_2 }), StrokeKind::Inside);
    }
}

/// The panel's content, inside its popup. `ui.close()` closes it.
pub fn show(ui: &mut Ui, tab: &mut Tab, col: usize, p: &Palette, m: &Metrics, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    width(ui, 300.0);
    if tab.grid.panel.as_ref().is_none_or(|x| x.col != col) {
        tab.grid.panel = Some(Panel::new(tab, col, &ctx));
    }
    let mut add_open = tab.grid.panel.as_ref().is_some_and(|x| x.add_open);
    tools(ui, tab, col, p, &mut add_open, true, actions);
    sep(ui, p);
    let Some(pn) = tab.grid.panel.as_mut() else { return };
    pn.add_open = add_open;
    if let Some(r) = pn.job.as_mut().and_then(|j| j.poll()) {
        pn.job = None;
        pn.counted = r.map(Arc::new);
        pn.matched = None;
        pn.search = None;
    }
    if let Some((_, j)) = &mut pn.toggling {
        if let Some(ex) = j.poll() {
            pn.toggling = None;
            if let Some(ex) = ex { pn.set_excluded(Arc::new(ex)); }
        }
    }
    let Some(c) = pn.counted.clone() else {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "Counting…", FontId::proportional(12.0), p.txt_dim);
        return;
    };
    if let Some(x) = profile(ui, &c, p, m) {
        go_to_value(tab, col, x, &ctx);
        ui.close();
        return;
    }
    sep(ui, p);
    let kind = c.prof.kind.unwrap_or(Kind::Text);
    // The search, and for numbers the data bars and colour scale.
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 4.0 + 30.0 + 6.0), Sense::hover());
    let bw = if kind == Kind::Number { 2.0 * 28.0 - 1.0 + 6.0 } else { 0.0 };
    let field = Rect::from_min_max(pos2(row.min.x, row.min.y + 4.0), pos2(row.max.x - bw, row.min.y + 34.0));
    let fid = ui.scope_id().with("cp-search");
    let focused = ctx.memory(|mem| mem.has_focus(fid));
    if focused { ui.painter().rect_stroke(field, 7.0, Stroke::new(4.0, p.focus_ring), StrokeKind::Outside); }
    ui.painter().rect(field, 6.0, p.bg_deep, Stroke::new(1.0, if focused { p.prim } else { p.brd }), StrokeKind::Inside);
    let te = egui::TextEdit::singleline(&mut pn.query)
        .id(fid)
        .hint_text(egui::RichText::new("Search values…").font(FontId::proportional(13.0)).color(p.txt_sub))
        .font(FontId::proportional(13.0))
        .text_color(p.txt_hi)
        .frame(egui::Frame::NONE);
    let resp = crate::widgets::centered_edit(ui, field.shrink2(vec2(10.0, 1.0)), te);
    if pn.focus { resp.request_focus(); pn.focus = false; }
    if kind == Kind::Number {
        let b1 = Rect::from_min_size(pos2(field.max.x + 6.0, field.min.y), vec2(28.0, 30.0));
        let b2 = b1.translate(vec2(27.0, 0.0));
        if look_tools(ui, tab, col, b1, b2, p) { ui.close(); }
    }
    let Some(pn) = tab.grid.panel.as_mut() else { return };
    pn.update_search(&c, &ctx);
    // A job under way: frames come when it ends.
    let all_on = match pn.toggling.as_ref() { Some((on, _)) => Some(*on), None => pn.all_ticked(&c, &ctx) };
    // The values: (Select all), then the most frequent first, a bar of its share behind.
    let max_count = c.order.first().map_or(1, |&i| c.vc.count(i as usize)).max(1) as f32;
    let n = pn.matching.len().min(MAX_LISTED);
    let more = pn.matching.len().saturating_sub(MAX_LISTED);
    let row_h = 30.0;
    let font = FontId::proportional(13.0);
    let cnt_font = FontId::proportional(11.5);
    let q = !pn.query.trim().is_empty();
    if pn.matching.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 6.0, r.center().y), Align2::LEFT_CENTER, "No value matches.", cnt_font.clone(), p.txt_dim);
    } else {
        let mut toggle: Option<(Option<u32>, bool)> = None;
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show_rows(ui, row_h, n + 1 + (more > 0) as usize, |ui, range| {
            for k in range {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                let pt = ui.painter();
                if k == n + 1 {
                    pt.text(pos2(r.min.x + 6.0, r.center().y), Align2::LEFT_CENTER, format!("{} more values — search to reach them.", grid::fmt_int(more)), cnt_font.clone(), p.txt_dim);
                    continue;
                }
                let (value, count, on, label_color, italic) = if k == 0 {
                    // Not known yet (looked at among millions): drawn as it was.
                    (if q { "(Select all matches)".to_string() } else { "(Select all)".to_string() }, pn.matching.len(), all_on.unwrap_or(true), p.txt_hi, false)
                } else {
                    let i = pn.matching[k - 1] as usize;
                    let v = c.vc.value(i);
                    let n = c.vc.count(i) as usize;
                    // The share of the most frequent value, behind the row.
                    let fw = (r.width() * n as f32 / max_count).max(1.0);
                    pt.rect_filled(Rect::from_min_size(r.min, vec2(fw, r.height())), 4.0, alpha(p.accent, 0.11));
                    let shown = if v.trim().is_empty() { if v.is_empty() { "(Empty)".to_string() } else { "(Blank)".to_string() } } else { grid::display(v).into_owned() };
                    let on = pn.toggling.as_ref().map_or_else(|| !pn.excluded.contains(v), |t| t.0);
                    (shown, n, on, if v.trim().is_empty() { p.txt_mut } else { p.txt_hi }, v.trim().is_empty())
                };
                if resp.hovered() { pt.rect_filled(r, 4.0, p.surf_2); }
                let cb = Rect::from_min_size(pos2(r.min.x + 6.0, r.center().y - 8.0), vec2(16.0, 16.0));
                checkbox(pt, cb, on, resp.hovered(), p);
                let cg = pt.layout_no_wrap(grid::fmt_int(count), cnt_font.clone(), p.txt_dim);
                let cw = cg.size().x;
                pt.galley(pos2(r.max.x - 6.0 - cw, r.center().y - cg.size().y / 2.0), cg, p.txt_dim);
                let mut fmt = egui::TextFormat::simple(font.clone(), label_color);
                fmt.italics = italic;
                let mut job = egui::text::LayoutJob::single_section(value, fmt);
                job.wrap = egui::text::TextWrapping { max_width: (r.width() - 30.0 - cw - 14.0).max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = pt.layout_job(job);
                pt.galley(pos2(cb.max.x + 8.0, ((r.center().y - g.size().y / 2.0) * 2.0).round() / 2.0), g, label_color);
                if resp.clicked() && pn.toggling.is_none() {
                    toggle = Some((if k == 0 { None } else { Some(pn.matching[k - 1]) }, !on));
                }
            }
        });
        if let Some((which, on)) = toggle {
            match which {
                Some(i) => {
                    let v = c.vc.value(i as usize);
                    let ex = Arc::make_mut(&mut pn.excluded);
                    if on { ex.remove(v); } else { ex.insert(v.to_string()); }
                    pn.ex_ver += 1;
                }
                None => pn.tick_all(&c, on, &ctx),
            }
        }
    }
    // Clear / Apply, right-aligned under a rule.
    let (r, _) = ui.allocate_exact_size(vec2(w, 6.0 + 8.0 + 30.0 + 2.0), Sense::hover());
    ui.painter().hline(r.x_range(), r.min.y + 6.0 + 0.5, Stroke::new(1.0, p.brd));
    let by = r.min.y + 6.0 + 8.0;
    let apply = Rect::from_min_size(pos2(r.max.x - 4.0 - 64.0, by), vec2(64.0, 30.0));
    let clear = Rect::from_min_size(pos2(apply.min.x - 8.0 - 60.0, by), vec2(60.0, 30.0));
    let bfont = FontId::new(13.0, egui::FontFamily::Name(crate::fonts::UI_MEDIUM.into()));
    let cr = ui.interact(clear, ui.scope_id().with("cp-clear"), Sense::click());
    ui.painter().rect(clear, 6.0, if cr.hovered() { p.surf_2 } else { Color32::TRANSPARENT }, Stroke::new(1.0, if cr.hovered() { p.brd_2 } else { p.brd }), StrokeKind::Inside);
    ui.painter().text(clear.center(), Align2::CENTER_CENTER, "Clear", bfont.clone(), p.txt);
    let ar = ui.interact(apply, ui.scope_id().with("cp-apply"), Sense::click());
    ui.painter().rect_filled(apply, 6.0, if ar.hovered() { p.prim_hover } else { p.prim });
    ui.painter().text(apply.center(), Align2::CENTER_CENTER, "Apply", bfont, p.on_prim);
    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    // Not while "(Select all)" is being worked out: Apply would hide what was there before.
    let ready = tab.grid.panel.as_ref().is_some_and(|x| x.toggling.is_none());
    if cr.clicked() {
        tab.hide_values(col, HashSet::new(), &ctx);
        tab.grid.panel = None;
        ui.close();
    } else if (ar.clicked() || enter) && ready {
        // The set itself, not a copy of it (it may hold millions of values).
        let set = tab.grid.panel.take().map(|x| x.excluded).unwrap_or_default();
        tab.hide_values(col, set, &ctx);
        ui.close();
    }
}

/// A title's right click: what changes the column's structure, one item each.
pub fn title_menu(ui: &mut Ui, tab: &mut Tab, col: usize, p: &Palette, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    width(ui, 230.0);
    let title = tab.st().and_then(|s| s.headers().get(col).cloned()).unwrap_or_default();
    head(ui, p, &title);
    if Item::new("Split the column…").icon(Icon::Split).show(ui, p).clicked() { actions.push(Action::SplitColumn(col)); ui.close(); }
    if Item::new("Merge columns…").icon(Icon::Merge).show(ui, p).clicked() { actions.push(Action::MergeColumns(col)); ui.close(); }
    sep(ui, p);
    if Item::new("Rename…").icon(Icon::Edit).show(ui, p).clicked() { actions.push(Action::RenameColumn(col)); ui.close(); }
    if Item::new("Insert a column before").icon(Icon::InsertBefore).show(ui, p).clicked() { actions.push(Action::InsertColumn(col)); ui.close(); }
    if Item::new("Insert a column after").icon(Icon::InsertAfter).show(ui, p).clicked() { actions.push(Action::InsertColumn(col + 1)); ui.close(); }
    if tab.kinds.get(col) == Some(&Kind::Number) {
        let on = tab.grid.heat.contains_key(&col);
        if Item::new(if on { "Remove the colour scale" } else { "Colour the cells by value" }).icon(Icon::Heat).show(ui, p).clicked() { grid::toggle_look(tab, col, true, &ctx); ui.close(); }
    }
    if Item::new("Fit the width").show(ui, p).clicked() { actions.push(Action::FitWidth(col)); ui.close(); }
    sep(ui, p);
    if Item::new("Group by this column…").icon(Icon::GroupBy).show(ui, p).clicked() { actions.push(Action::Tool(crate::tools::group::open(tab, Some(col)))); ui.close(); }
    if Item::new("Pivot table by this column…").icon(Icon::Pivot).show(ui, p).clicked() { actions.push(Action::Tool(crate::tools::pivot::open(tab, Some(col)))); ui.close(); }
    sep(ui, p);
    let on = tab.grid.frozen == col + 1;
    let label = if on { "Unfreeze the columns" } else if col > 0 { "Freeze up to this column" } else { "Freeze this column" };
    if Item::new(label).icon(Icon::FreezeColumn).show(ui, p).clicked() { grid::freeze_to(tab, col); ui.close(); }
    if tab.grid.frozen > 0 && !on && Item::new("Unfreeze the columns").icon(Icon::FreezeColumn).show(ui, p).clicked() { tab.grid.frozen = 0; ui.close(); }
    sep(ui, p);
    if Item::new("Delete the column").icon(Icon::Trash).danger().show(ui, p).clicked() { actions.push(Action::DeleteColumns(vec![col])); ui.close(); }
}

/// The columns manager (the title row's ⫶ icon in the gutter): every column with a box to
/// show or hide it (Shift: a range), a search, show all / hide all; a name clicked scrolls to
/// its column; a row dragged by its grip moves the column (one undo step).
#[derive(Default)]
pub struct Manager {
    query: String,
    focus: bool,
    /// The column of the last box ticked (Shift+click: the range from it).
    last: Option<usize>,
    /// The column being dragged.
    drag: Option<usize>,
}

impl Manager {
    pub fn new() -> Manager {
        Manager { focus: true, ..Default::default() }
    }
}

/// The manager's content, inside its popup.
pub fn manager(ui: &mut Ui, tab: &mut Tab, p: &Palette, ctx: &egui::Context) {
    width(ui, 300.0);
    let Some(st) = tab.st().cloned() else { return };
    let n = st.ncols;
    let heads = st.headers();
    let mut mg = tab.grid.manager.take().unwrap_or_else(Manager::new);
    let shown = n - tab.grid.hidden.iter().filter(|&&c| c < n).count();
    head(ui, p, &format!("Columns · {} of {} shown", grid::fmt_int(shown), grid::fmt_int(n)));
    // The search.
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 4.0 + 30.0 + 6.0), Sense::hover());
    let field = Rect::from_min_max(pos2(row.min.x + 4.0, row.min.y + 4.0), pos2(row.max.x - 4.0, row.min.y + 34.0));
    let fid = ui.scope_id().with("cm-search");
    let focused = ctx.memory(|mem| mem.has_focus(fid));
    if focused { ui.painter().rect_stroke(field, 7.0, Stroke::new(4.0, p.focus_ring), StrokeKind::Outside); }
    ui.painter().rect(field, 6.0, p.bg_deep, Stroke::new(1.0, if focused { p.prim } else { p.brd }), StrokeKind::Inside);
    let te = egui::TextEdit::singleline(&mut mg.query)
        .id(fid)
        .hint_text(egui::RichText::new("Search columns…").font(FontId::proportional(13.0)).color(p.txt_sub))
        .font(FontId::proportional(13.0))
        .text_color(p.txt_hi)
        .frame(egui::Frame::NONE);
    let resp = crate::widgets::centered_edit(ui, field.shrink2(vec2(10.0, 1.0)), te);
    if mg.focus { resp.request_focus(); mg.focus = false; }
    let q = csvfab_engine::text::fold(mg.query.trim());
    let matching: Vec<usize> = (0..n).filter(|&c| q.is_empty() || csvfab_engine::text::fold(&heads[c]).contains(&q) || c.to_string() == q).collect();
    // Show all / hide all (of the columns matching; one always stays shown).
    match crate::widgets::row_buttons(ui, p, None, if q.is_empty() { "Every column" } else { "These columns" }, &[(Some(Icon::Eye), "Show all", true), (Some(Icon::EyeOff), "Hide all", true)]) {
        Some(0) => { for c in &matching { tab.grid.hidden.remove(c); } }
        Some(1) => {
            for &c in &matching { tab.grid.hidden.insert(c); }
            if tab.grid.hidden.len() >= n {
                let keep = tab.grid.cursor.map_or(0, |k| k.col).min(n - 1);
                tab.grid.hidden.remove(&keep);
            }
        }
        _ => {}
    }
    sep(ui, p);
    if matching.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, "No column matches.", FontId::proportional(11.5), p.txt_dim);
    }
    let row_h = 28.0;
    let font = FontId::proportional(13.0);
    let small = FontId::new(10.0, egui::FontFamily::Name(crate::fonts::MONO_BOLD.into()));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let mut toggle: Option<(usize, bool, bool)> = None;
    let mut goto: Option<usize> = None;
    let mut drop_at: Option<(usize, bool)> = None;
    egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show_rows(ui, row_h, matching.len(), |ui, range| {
        for k in range {
            let c = matching[k];
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
            let pt = ui.painter();
            let on = !tab.grid.hidden.contains(&c);
            let dragging = mg.drag.is_some();
            if (resp.hovered() && !dragging) || mg.drag == Some(c) { pt.rect_filled(r, 4.0, p.surf_2); }
            // The grip: six dots, the row's handle for a drag.
            let gx = r.min.x + 8.0;
            for (dx, dy) in [(0.0, -4.0), (0.0, 0.0), (0.0, 4.0), (4.0, -4.0), (4.0, 0.0), (4.0, 4.0)] {
                pt.circle_filled(pos2(gx + dx, r.center().y + dy), 1.1, if resp.hovered() { p.txt_mut } else { p.txt_faint });
            }
            let cb = Rect::from_min_size(pos2(r.min.x + 20.0, r.center().y - 8.0), vec2(16.0, 16.0));
            let on_box = pointer.is_some_and(|q| q.x < cb.max.x + 4.0 && r.contains(q));
            checkbox(pt, cb, on, resp.hovered() && on_box, p);
            let ng = pt.layout_no_wrap(c.to_string(), small.clone(), p.txt_faint);
            let nw = ng.size().x;
            pt.galley(pos2(r.max.x - 8.0 - nw, (r.center().y - ng.size().y / 2.0).round()), ng, p.txt_faint);
            let color = if on { p.txt_hi } else { p.txt_dim };
            let mut job = egui::text::LayoutJob::simple_singleline(heads[c].clone(), font.clone(), color);
            job.wrap = egui::text::TextWrapping { max_width: (r.max.x - 8.0 - nw - 10.0 - (cb.max.x + 8.0)).max(8.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let g = pt.layout_job(job);
            pt.galley(pos2(cb.max.x + 8.0, (r.center().y - g.size().y / 2.0).round()), g, color);
            if resp.hovered() && !dragging { ctx.set_cursor_icon(if on_box { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Grab }); }
            if resp.drag_started() { mg.drag = Some(c); }
            if let (Some(from), Some(q)) = (mg.drag, pointer) {
                if from != c && r.contains(q) {
                    // Where it would land: a 2 px rule above or below this row.
                    let before = q.y < r.center().y;
                    let y = if before { r.min.y } else { r.max.y - 2.0 };
                    pt.rect_filled(Rect::from_min_size(pos2(r.min.x + 4.0, y), vec2(r.width() - 8.0, 2.0)), 1.0, p.prim);
                    drop_at = Some((c, before));
                }
            }
            if resp.clicked() {
                if on_box { toggle = Some((c, !on, ui.input(|i| i.modifiers.shift))); } else { goto = Some(c); }
            }
        }
    });
    if mg.drag.is_some() { ctx.set_cursor_icon(egui::CursorIcon::Grabbing); }
    if let Some((c, show, shift)) = toggle {
        let range = match (shift, mg.last) { (true, Some(a)) => a.min(c)..=a.max(c), _ => c..=c };
        for k in range.filter(|k| matching.contains(k)) {
            if show { tab.grid.hidden.remove(&k); } else { tab.grid.hidden.insert(k); }
        }
        // One column always stays shown.
        if tab.grid.hidden.len() >= n { tab.grid.hidden.remove(&c); }
        mg.last = Some(c);
    }
    if let Some(c) = goto {
        tab.grid.hidden.remove(&c);
        let row = tab.grid.cursor.map_or(tab.grid.top, |k| k.row);
        tab.grid.select(grid::Cell { row, col: c }, false);
        tab.grid.reveal_col = true;
    }
    if let Some(from) = mg.drag {
        if !ui.input(|i| i.pointer.primary_down()) {
            mg.drag = None;
            if let Some((tc, before)) = drop_at {
                let mut to = if before { tc } else { tc + 1 };
                if to > from { to -= 1; }
                grid::move_column(tab, from, to, ctx);
            }
        }
    }
    tab.grid.manager = Some(mg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Until `done`, polling as frames would (a minute at most).
    fn wait(mut done: impl FnMut() -> bool) {
        let t0 = Instant::now();
        while !done() {
            assert!(t0.elapsed() < Duration::from_secs(60), "never done");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A column of 50 000 distinct values: the search, "(Select all)" and its tick are worked
    /// out in jobs, and give what the plain loops gave.
    #[test]
    fn many_values_in_jobs() {
        let ctx = egui::Context::default();
        // The first pass loads the fonts (a tab measures its columns).
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let m = crate::theme::Metrics::default();
        let mut csv = String::from("id,v\n");
        for i in 0..50_000 { csv += &format!("{i},{}\n", i % 3); }
        let tab = Tab::memory("t".into(), csv.into_bytes(), &ctx, &m);
        let mut pn = Panel::new(&tab, 0, &ctx);
        wait(|| { if let Some(r) = pn.job.as_mut().and_then(|j| j.poll()) { pn.counted = r.map(Arc::new); } pn.counted.is_some() });
        let c = pn.counted.clone().unwrap();
        assert_eq!(c.vc.len(), 50_000);
        pn.update_search(&c, &ctx);
        assert_eq!(pn.matching.len(), 50_000, "no query: every value, at once");
        assert_eq!(pn.all_ticked(&c, &ctx), Some(true));
        // A search among them: from a job, the same values the search gives.
        pn.query = "1".into();
        pn.update_search(&c, &ctx);
        assert!(pn.search.is_some());
        wait(|| { pn.update_search(&c, &ctx); pn.searched() });
        assert_eq!(*pn.matching, c.vc.search(&c.order, "1"));
        assert!(pn.matching.len() > AT_ONCE);
        // Unticked, all of them: the new set made in a job.
        pn.tick_all(&c, false, &ctx);
        let (_, j) = pn.toggling.as_mut().unwrap();
        let mut got = None;
        wait(|| { got = j.poll(); got.is_some() });
        pn.toggling = None;
        pn.set_excluded(Arc::new(got.unwrap().unwrap()));
        assert_eq!(pn.excluded.len(), pn.matching.len());
        assert!(pn.matching.iter().all(|&i| pn.excluded.contains(c.vc.value(i as usize))));
        let mut ticked = None;
        wait(|| { ticked = pn.all_ticked(&c, &ctx); ticked.is_some() });
        assert_eq!(ticked, Some(false));
        // Every value listed again: one value unticked among 50 000, looked for in a job.
        pn.query.clear();
        pn.update_search(&c, &ctx);
        Arc::make_mut(&mut pn.excluded).retain(|v| v == "4123");
        pn.ex_ver += 1;
        let mut ticked = None;
        wait(|| { ticked = pn.all_ticked(&c, &ctx); ticked.is_some() });
        assert_eq!(ticked, Some(false));
        Arc::make_mut(&mut pn.excluded).clear();
        pn.ex_ver += 1;
        assert_eq!(pn.all_ticked(&c, &ctx), Some(true));
    }
}
