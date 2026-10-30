//! The column panel, under a title's ▾ (docs/look.md, "Column panel"): tools,
//! profile, the values with their counts to filter by value.
//!
//! As the JS: the profile and the values come from one count over the rows the
//! other filters let through (this column's own value filter left out, so its
//! hidden values still show, unticked), in a `Job`; ticking values and Apply
//! hides the unticked ones (`Tab::hide_values`). The title's right click gives
//! the column's structure instead (`title_menu`). The column inspector (inspector.rs)
//! docks the same count, histogram and tools (named, `tools_named`).

use crate::grid::{self, Action};
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use crate::widgets::{head, sep, width, Item};
use csvfab_engine::stats::{self, PanelProfile, ValueCounts};
use csvfab_engine::view::{Kind, SortKey};
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
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

/// A small outline button of the tools row, `icon` drawn 14 px, alone or before `text`; joined
/// to its neighbours by `corners` (left, right rounded). `tip`: "" for none.
#[allow(clippy::too_many_arguments)]
fn tool(ui: &mut Ui, p: &Palette, r: Rect, id: Id, draw: &dyn Fn(&egui::Painter, Rect, Color32), text: &str, tip: &str, enabled: bool, on: bool, danger: bool, corners: (bool, bool)) -> egui::Response {
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
    if text.is_empty() {
        draw(pt, Rect::from_center_size(r.center(), vec2(14.0, 14.0)), fg);
    } else {
        draw(pt, Rect::from_center_size(pos2(r.min.x + 15.0, r.center().y), vec2(14.0, 14.0)), fg);
        pt.text(pos2(r.min.x + 27.0, r.center().y), Align2::LEFT_CENTER, text, FontId::proportional(12.0), fg);
    }
    if hot && !tip.is_empty() {
        grid::tip(ui.ctx(), p, &crate::theme::Metrics::default(), r, tip);
    }
    resp
}

fn ic(icon: Icon) -> impl Fn(&egui::Painter, Rect, Color32) {
    move |pt, r, c| icons::paint(pt, icon, r, c)
}

/// The column's tools, in the order of the panel's row.
#[derive(Clone, Copy, PartialEq)]
enum T { Left, Right, Split, Merge, Convert, Asc, Desc, Freeze, Add, Del, Group, Pivot }

impl T {
    /// Drawn 14 px.
    fn draw(self, pt: &egui::Painter, r: Rect, c: Color32) {
        match self {
            T::Left => icons::paint(pt, Icon::ChevronLeft, r, c),
            T::Right => icons::paint(pt, Icon::ChevronRight, r, c),
            T::Split => icons::paint(pt, Icon::Split, r, c),
            T::Merge => icons::paint(pt, Icon::Merge, r, c),
            T::Convert => icons::paint(pt, Icon::ConvertColumn, r, c),
            T::Asc => icons::arrow(pt, r, true, 2.0, c),
            T::Desc => icons::arrow(pt, r, false, 2.0, c),
            T::Freeze => icons::paint(pt, Icon::FreezeColumn, r, c),
            T::Add => icons::plus(pt, r, 2.0, c),
            T::Del => { pt.hline(r.x_range().shrink(2.5), (r.center().y * 2.0).round() / 2.0, Stroke::new(1.6, c)); }
            T::Group => icons::paint(pt, Icon::GroupBy, r, c),
            T::Pivot => icons::paint(pt, Icon::Pivot, r, c),
        }
    }

    /// Its tooltip without a label.
    fn tip(self, frozen: bool) -> &'static str {
        match self {
            T::Left => "Move left",
            T::Right => "Move right",
            T::Split => "Split",
            T::Merge => "Merge",
            T::Convert => "Convert: rewrite its dates, numbers or phones into one format (the values change)",
            T::Asc => "Sort ascending",
            T::Desc => "Sort descending",
            T::Freeze if frozen => "Unfreeze the columns",
            T::Freeze => "Freeze up to this column: it stays in view when scrolling sideways",
            T::Add => "Insert a column",
            T::Del => "Delete this column",
            T::Group => "Group by this column…",
            T::Pivot => "Pivot table by this column…",
        }
    }

    /// Its label where there is room, and what a tooltip adds to it.
    fn named(self, frozen: bool) -> (&'static str, &'static str) {
        match self {
            T::Convert => ("Convert…", "Rewrite its dates, numbers or phones into one format (the values change)"),
            T::Asc => ("Ascending", ""),
            T::Desc => ("Descending", ""),
            T::Split => ("Split…", ""),
            T::Merge => ("Merge…", ""),
            T::Freeze if frozen => ("Unfreeze", ""),
            T::Freeze => ("Freeze", "Up to this column: it stays in view when scrolling sideways"),
            T::Add => ("Insert…", ""),
            T::Del => ("Delete", ""),
            T::Group => ("Group by…", ""),
            T::Pivot => ("Pivot…", ""),
            T::Left | T::Right => ("", self.tip(false)),
        }
    }
}

/// One tool in `r`, and what its click does. `popup`: in the panel, which a tool closes.
#[allow(clippy::too_many_arguments)]
fn tool_at(ui: &mut Ui, t: T, r: Rect, corners: (bool, bool), named: bool, tab: &mut Tab, col: usize, p: &Palette, add_open: &mut bool, popup: bool, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let ncols = tab.st().map_or(0, |s| s.ncols);
    let sorted = tab.sort.first().filter(|k| k.col == col).map(|k| k.desc);
    let frozen = tab.grid.frozen == col + 1;
    let (enabled, on) = match t {
        T::Left => (col > 0, false),
        T::Right => (col + 1 < ncols, false),
        T::Asc => (true, sorted == Some(false)),
        T::Desc => (true, sorted == Some(true)),
        T::Freeze => (true, frozen),
        T::Add => (true, *add_open),
        _ => (true, false),
    };
    let (text, tip) = if named { t.named(frozen) } else { ("", t.tip(frozen)) };
    let id = ui.scope_id().with(("cp-tool", t as u8));
    if !tool(ui, p, r, id, &|pt, r, c| t.draw(pt, r, c), text, tip, enabled, on, t == T::Del, corners).clicked() { return; }
    match t {
        T::Left => grid::move_column(tab, col, col - 1, &ctx),
        T::Right => grid::move_column(tab, col, col + 1, &ctx),
        T::Split => actions.push(Action::SplitColumn(col)),
        T::Merge => actions.push(Action::MergeColumns(col)),
        T::Convert => actions.push(Action::ConvertColumn(col)),
        T::Asc => actions.push(Action::Sort(vec![SortKey { col, desc: false }])),
        T::Desc => actions.push(Action::Sort(vec![SortKey { col, desc: true }])),
        T::Freeze => grid::freeze_to(tab, col),
        T::Add => { *add_open = !*add_open; return; }
        T::Del => actions.push(Action::DeleteColumns(vec![col])),
        T::Group => actions.push(Action::Tool(crate::tools::group::open(tab, Some(col)))),
        T::Pivot => actions.push(Action::Tool(crate::tools::pivot::open(tab, Some(col)))),
    }
    if popup { ui.close(); }
}

/// The insert menu, right-aligned on `at` under it (`up`: over it). A click elsewhere closes it.
#[allow(clippy::too_many_arguments)]
fn add_menu(ui: &mut Ui, at: Pos2, up: bool, button: Rect, col: usize, p: &Palette, add_open: &mut bool, popup: bool, actions: &mut Vec<Action>) {
    if !*add_open { return; }
    let ctx = ui.ctx().clone();
    let pos = pos2(at.x - 210.0, at.y);
    let area = egui::Area::new(ui.scope_id().with("cp-add")).order(egui::Order::Foreground).fixed_pos(pos).pivot(if up { Align2::LEFT_BOTTOM } else { Align2::LEFT_TOP }).show(&ctx, |ui| {
        crate::theme::menu_frame(p, &Metrics::default()).inner_margin(4).show(ui, |ui| {
            width(ui, 210.0);
            let close = |ui: &Ui| if popup { ui.close(); };
            if Item::new("Insert a column before").icon(Icon::InsertBefore).show(ui, p).clicked() { actions.push(Action::InsertColumn(col)); *add_open = false; close(ui); }
            if Item::new("Insert a column after").icon(Icon::InsertAfter).show(ui, p).clicked() { actions.push(Action::InsertColumn(col + 1)); *add_open = false; close(ui); }
        });
    });
    let away = ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|q| !area.response.rect.contains(q) && !button.contains(q)));
    if away { *add_open = false; }
}

/// The tools row: move, split / merge, convert, sort, freeze, insert / delete. `popup`: in the
/// panel, which a tool closes; `add_open`: the insert menu is open.
pub(crate) fn tools(ui: &mut Ui, tab: &mut Tab, col: usize, p: &Palette, add_open: &mut bool, popup: bool, actions: &mut Vec<Action>) {
    const GROUPS: [&[T]; 7] = [&[T::Left, T::Right], &[T::Split, T::Merge], &[T::Convert], &[T::Asc, T::Desc], &[T::Freeze], &[T::Add, T::Del], &[T::Group, T::Pivot]];
    let w = ui.available_width();
    // The groups' places, wrapped onto more lines when the row is full.
    let left = ui.cursor().min.x + 2.0;
    let mut placed: Vec<(f32, usize)> = Vec::new();
    let (mut x, mut line) = (left, 0usize);
    for g in GROUPS {
        let gw = g.len() as f32 * 25.0 + 1.0;
        if x + gw > left + w - 4.0 && x > left { x = left; line += 1; }
        placed.push((x, line));
        x += gw + 4.0;
    }
    let (row, _) = ui.allocate_exact_size(vec2(w, 2.0 + (line + 1) as f32 * 28.0 - 4.0 + 6.0), Sense::hover());
    let mut add = Rect::NOTHING;
    for (g, (x, li)) in GROUPS.iter().zip(placed) {
        for (k, &t) in g.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x + k as f32 * 25.0, row.min.y + 2.0 + li as f32 * 28.0), vec2(26.0, 24.0));
            if t == T::Add { add = r; }
            tool_at(ui, t, r, (k == 0, k + 1 == g.len()), false, tab, col, p, add_open, popup, actions);
        }
    }
    // Under the + −.
    add_menu(ui, pos2(add.max.x + 25.0, add.max.y + 4.0), false, add, col, p, add_open, popup, actions);
}

/// Height of `tools_named`.
pub(crate) const NAMED_H: f32 = 4.0 * 30.0 - 4.0;

/// The same tools where there is room for words: three a line, the pairs joined, sort first,
/// then what reads the column, what reshapes it, what adds or takes one away; for numbers its
/// two looks end the last line. Every column gets the same four lines, nothing moves from
/// one to the next.
pub(crate) fn tools_named(ui: &mut Ui, tab: &mut Tab, col: usize, w: f32, p: &Palette, add_open: &mut bool, actions: &mut Vec<Action>) {
    let (row, _) = ui.allocate_exact_size(vec2(w, NAMED_H), Sense::hover());
    let (gap, h) = (4.0, 26.0);
    let cw = ((w - 2.0 * gap) / 3.0).floor();
    // Three cells: a tool takes one, a joined pair two.
    let cell = |x: usize, y: usize, n: f32| Rect::from_min_size(pos2(row.min.x + x as f32 * (cw + gap), row.min.y + y as f32 * 30.0), vec2(cw * n + gap * (n - 1.0), h));
    let joined = |r: Rect| { let m = r.center().x.round(); (Rect::from_min_max(r.min, pos2(m + 1.0, r.max.y)), Rect::from_min_max(pos2(m, r.min.y), r.max)) };
    let mut add = Rect::NOTHING;
    for (y, (a, b)) in [(T::Asc, T::Desc), (T::Group, T::Pivot), (T::Split, T::Merge), (T::Add, T::Del)].into_iter().enumerate() {
        let (ra, rb) = joined(cell(0, y, 2.0));
        if a == T::Add { add = ra; }
        tool_at(ui, a, ra, (true, false), true, tab, col, p, add_open, false, actions);
        tool_at(ui, b, rb, (false, true), true, tab, col, p, add_open, false, actions);
    }
    tool_at(ui, T::Freeze, cell(2, 0, 1.0), (true, true), true, tab, col, p, add_open, false, actions);
    tool_at(ui, T::Convert, cell(2, 1, 1.0), (true, true), true, tab, col, p, add_open, false, actions);
    let (l, r) = joined(cell(2, 2, 1.0));
    tool_at(ui, T::Left, l, (true, false), true, tab, col, p, add_open, false, actions);
    tool_at(ui, T::Right, r, (false, true), true, tab, col, p, add_open, false, actions);
    if tab.kinds.get(col) == Some(&Kind::Number) {
        let (a, b) = joined(cell(2, 3, 1.0));
        look_tools(ui, tab, col, a, b, p);
    }
    // Over the buttons: the inspector's foot is at the window's bottom.
    add_menu(ui, pos2(add.min.x + 210.0, add.min.y - 4.0), true, add, col, p, add_open, false, actions);
}

/// A number's two looks: data bars and the colour scale, in `b1` and `b2`. Whether one was toggled.
pub(crate) fn look_tools(ui: &mut Ui, tab: &mut Tab, col: usize, b1: Rect, b2: Rect, p: &Palette) -> bool {
    let ctx = ui.ctx().clone();
    let (bars_on, heat_on) = (tab.grid.bars.contains_key(&col), tab.grid.heat.contains_key(&col));
    let mut hit = false;
    if tool(ui, p, b1, ui.scope_id().with("cp-bars"), &ic(Icon::DataBars), "", "Data bars: each value drawn as a bar at the foot of its cell", true, bars_on, false, (true, false)).clicked() {
        grid::toggle_look(tab, col, false, &ctx);
        hit = true;
    }
    if tool(ui, p, b2, ui.scope_id().with("cp-heat"), &ic(Icon::ColorScale), "", "Colour scale: each cell tinted by its value", true, heat_on, false, (false, true)).clicked() {
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
fn profile(ui: &mut Ui, tab: &mut Tab, col: usize, c: &Counted, p: &Palette, m: &Metrics) -> Option<f64> {
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
    // What the end bars gather.
    let cut = cut_note(ui, pr, FontId::proportional(11.5), p.txt_dim, w - 20.0);
    let cut_h = cut.as_ref().map_or(0.0, |g| g.size().y + 4.0);
    let h = 8.0 + 20.0 + 4.0 + note.as_ref().map_or(0.0, |_| 18.0) + rows.len() as f32 * line_h + if hist { 8.0 + 44.0 } else { 0.0 } + cut_h + 8.0;
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
        histogram(ui, tab, col, Rect::from_min_size(pos2(x0, y), vec2(inner_w, 44.0)), pr, p, m);
        if let Some(g) = cut { ui.painter().galley(pos2(x0, y + 44.0 + 4.0), g, p.txt_dim); }
    }
    go
}

/// How a number column's figures are written, all alike: two decimals when its values have some.
pub(crate) fn figure(pr: &PanelProfile) -> impl Fn(f64) -> String {
    let dec = [&pr.min, &pr.max].iter().any(|s| s.as_ref().is_some_and(|x| x.1.contains(['.', ','])));
    move |v| if dec { stats::fmt_num(v, 2, 2, true) } else { num(v) }
}

/// A value of the column as the figures write it.
pub(crate) fn figure_of(pr: &PanelProfile, s: &str) -> String {
    match pr.kind {
        Some(Kind::Number) => csvfab_engine::text::parse_number(s).map_or_else(|| grid::display(s).into_owned(), figure(pr)),
        _ => grid::display(s).into_owned(),
    }
}

/// A bar's values.
fn bars_say(pr: &PanelProfile, first: &str, last: &str, to: &str) -> String {
    if first == last { figure_of(pr, first) } else { format!("{}{to}{}", figure_of(pr, first), figure_of(pr, last)) }
}

fn values(n: u64) -> String {
    format!("{} value{}", grid::fmt_int(n as usize), if n == 1 { "" } else { "s" })
}

/// The end bars of a cut range: for each, its index and the values it gathers, said.
pub(crate) fn far_bars(pr: &PanelProfile) -> Vec<(usize, String)> {
    let before = if pr.kind == Some(Kind::Date) { ("before", "after") } else { ("under", "over") };
    let (s, p) = if pr.kind == Some(Kind::Date) { ("date", "dates") } else { ("value", "values") };
    let body = pr.hist.iter().filter(|b| !b.tail && b.count > 0);
    let say = |n: u64, side: &str, x: String| format!("{} {} far out, {side} {x}", grid::fmt_int(n as usize), if n == 1 { s } else { p });
    let mut out = Vec::new();
    if let (Some(b), Some(x)) = (pr.hist.first().filter(|b| b.tail), body.clone().next()) { out.push((0, say(b.count, before.0, figure_of(pr, &x.first)))); }
    if let (Some(b), Some(x)) = (pr.hist.last().filter(|b| b.tail), body.clone().next_back()) { out.push((pr.hist.len() - 1, say(b.count, before.1, figure_of(pr, &x.last)))); }
    out
}

/// What the end bars of a cut range gather, on two lines at most (the bars alone would not say).
pub(crate) fn cut_note(ui: &Ui, pr: &PanelProfile, font: FontId, c: Color32, w: f32) -> Option<Arc<egui::Galley>> {
    let (head, tail) = (pr.hist.first().filter(|b| b.tail), pr.hist.last().filter(|b| b.tail));
    let body = pr.hist.iter().filter(|b| !b.tail && b.count > 0);
    let under = head.and_then(|b| body.clone().next().map(|x| (b.count, figure_of(pr, &x.first))));
    let over = tail.and_then(|b| body.clone().next_back().map(|x| (b.count, figure_of(pr, &x.last))));
    let text = match (under, over) {
        (Some((n, x)), Some((k, y))) => format!("End bars: {} under {x}, {} over {y}", values(n), values(k)),
        (Some((n, x)), None) => format!("First bar: {} under {x}", values(n)),
        (None, Some((k, y))) => format!("Last bar: {} over {y}", values(k)),
        _ => return None,
    };
    let mut job = egui::text::LayoutJob::simple(text, font, c, w);
    job.wrap.max_rows = 2;
    job.wrap.overflow_character = Some('…');
    Some(ui.painter().layout_job(job))
}

/// Numbers or dates spread in `hr`: bars 1 apart, tops rounded; the end bars of a cut range
/// stand apart, in `warn`. Each says its values on hover; a click on a bar, or a drag across
/// bars, keeps the rows whose value lies from the first to the last value they hold (a
/// value filter, again: off), and the stretch kept stays marked.
pub(crate) fn histogram(ui: &mut Ui, tab: &mut Tab, col: usize, hr: Rect, pr: &PanelProfile, p: &Palette, m: &Metrics) {
    let n = pr.hist.len();
    let ctx = ui.ctx().clone();
    let (low, high) = (pr.hist.first().is_some_and(|b| b.tail), pr.hist.last().is_some_and(|b| b.tail));
    // The gap that sets an end bar apart.
    const APART: f32 = 5.0;
    let bw = (hr.width() - (n as f32 - 1.0) - (low as u8 + high as u8) as f32 * APART) / n as f32;
    let slot_of = |i: usize| {
        let x = hr.min.x + i as f32 * (bw + 1.0) + if low && i > 0 { APART } else { 0.0 } + if high && i + 1 == n { APART } else { 0.0 };
        Rect::from_min_size(pos2(x, hr.min.y), vec2(bw, hr.height()))
    };
    let bar_at = |x: f32| (0..n).min_by(|&a, &b| (slot_of(a).center().x - x).abs().total_cmp(&(slot_of(b).center().x - x).abs())).unwrap_or(0);
    let max = pr.hist.iter().map(|b| b.count).max().unwrap_or(1).max(1) as f32;
    let id = ui.scope_id().with(("cp-hist", col));
    let resp = ui.interact(hr, id, Sense::click_and_drag());
    // The bar a drag began on.
    if resp.drag_started() {
        let from = ctx.input(|i| i.pointer.press_origin()).map(|q| bar_at(q.x));
        ctx.data_mut(|d| d.insert_temp(id, from));
    }
    let start = ctx.data(|d| d.get_temp::<Option<usize>>(id)).flatten();
    let at = resp.interact_pointer_pos().map(|q| bar_at(q.x));
    let drag = start.filter(|_| resp.dragged() || resp.drag_stopped()).zip(at).map(|(a, z)| (a.min(z), a.max(z)));
    // What a filter keeps now, and what the pointer goes for.
    let kept = crate::spark_ui::range_kept(tab, col);
    let held = |b: &stats::HistBin| kept.is_some_and(|(lo, hi)| b.count > 0 && b.lo >= lo && b.hi <= hi);
    if pr.hist.iter().any(held) {
        let bars: Vec<usize> = (0..n).filter(|&i| held(&pr.hist[i])).collect();
        let band = slot_of(bars[0]).union(slot_of(bars[bars.len() - 1]));
        ui.painter().rect_filled(band.expand2(vec2(0.5, 0.0)), 2.0, alpha(p.prim, 0.2));
    }
    if let Some((a, z)) = drag {
        ui.painter().rect_filled(slot_of(a).union(slot_of(z)).expand2(vec2(0.5, 0.0)), 2.0, alpha(p.prim, 0.2));
    }
    let hover = resp.hover_pos().filter(|_| !resp.dragged()).map(|q| bar_at(q.x)).filter(|&i| pr.hist[i].count > 0);
    let pt = ui.painter();
    for (i, b) in pr.hist.iter().enumerate() {
        let slot = slot_of(i);
        if b.count == 0 { continue; }
        // An end bar shows, however few it gathers.
        let least = if b.tail { 14.0 } else { 4.0 };
        let bh = (b.count as f32 / max * 100.0).max(least) / 100.0 * hr.height();
        let hot = hover == Some(i);
        let lit = drag.is_none_or(|(a, z)| (a..=z).contains(&i));
        let bar = Rect::from_min_max(pos2(slot.min.x, slot.max.y - bh), slot.max);
        let c = if b.tail { p.warn } else { p.prim };
        pt.rect_filled(bar, CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, c.gamma_multiply(if !lit { 0.3 } else if hot { 1.0 } else { 0.75 }));
        if hot {
            let vals = bars_say(pr, &b.first, &b.last, " – ");
            grid::tip(&ctx, p, m, slot, &format!("{vals} · {}{}", values(b.count), if b.tail { ", gathered: far from the others" } else { "" }));
        }
    }
    // The break in the line, each side of the bars that keep to scale.
    let dash = Stroke::new(1.0, alpha(p.warn, 0.7));
    let mark = |x: f32| { pt.add(egui::Shape::dashed_line(&[pos2(x, hr.min.y), pos2(x, hr.max.y)], dash, 3.0, 3.0)); };
    if low { mark(pt.round_to_pixel_center(slot_of(0).max.x + 3.0)); }
    if high { mark(pt.round_to_pixel_center(slot_of(n - 1).min.x - 3.0)); }
    if hover.is_some() { ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
    let picked = if resp.drag_stopped() { drag } else if resp.clicked() { hover.map(|i| (i, i)) } else { None };
    if let Some((a, z)) = picked { keep_bars(tab, col, pr, a, z, &ctx); }
}

/// The value filter on bars a..=z of a histogram, as a click or a drag across them sets it.
pub(crate) fn keep_bars(tab: &mut Tab, col: usize, pr: &PanelProfile, a: usize, z: usize, ctx: &egui::Context) {
    let some: Vec<&stats::HistBin> = pr.hist[a..=z].iter().filter(|b| b.count > 0).collect();
    let (Some(f), Some(l), Some(kind)) = (some.first(), some.last(), pr.kind) else {
        tab.grid.notice = Some("No row holds those values.".into());
        return;
    };
    crate::spark_ui::pick_range(tab, col, kind, f.lo, l.hi, &bars_say(pr, &f.first, &l.last, " to "), ctx);
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
    if let Some(x) = profile(ui, tab, col, &c, p, m) {
        go_to_value(tab, col, x, &ctx);
        ui.close();
        return;
    }
    let Some(pn) = tab.grid.panel.as_mut() else { return };
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
