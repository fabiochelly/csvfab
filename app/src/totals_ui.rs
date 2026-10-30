//! The totals row held at the grid's bottom (docs/look.md, "Totals row"): per
//! column a function (sum, average, median…) over the rows shown, recomputed in
//! the background as they change.
//!
//! As the JS: a sum by default under the columns that read as quantities
//! (`stats::sum_by_default`, decided in a job), nothing elsewhere; a click on a
//! total picks another function. The pass waits for a quiet moment (typing a
//! filter keeps postponing it), reads every row shown — a total is exact or it
//! is not one —, and is incremental: each column's totals are kept with what
//! they were counted on (the rows shown, the column's cells), so a new function
//! only counts its column, an edit only the columns it touched, and a few cells
//! typed are told to their totals without counting again (`edited`). Until a
//! pass ends the previous values stay, dimmed.

use crate::grid::{self, Action};
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{Metrics, Palette};
use crate::widgets::{head, sep, width, Item};
use csvfab_engine::stats::{self, TotalFn, TotalReq, Totals};
use csvfab_engine::table::{Layer, Layers};
use csvfab_engine::view::Kind;
use csvfab_engine::{Base, RowId};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontFamily, FontId, Mesh, Painter, Rect, RectAlign, Sense, Shape, Stroke, Ui};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

/// ms without a filter, sort or edit before a pass starts.
const QUIET: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DefKey {
    base: usize,
    ncols: usize,
    header: bool,
}

/// What a column's totals were counted on: the rows shown (the view, else the table's order)
/// and the column's cells (its edit layers, its field of the file, its kind). Held weakly and
/// compared by address: an address held here is not handed out again meanwhile, and an
/// `Arc::make_mut` on a list held here makes a new one.
#[derive(Clone)]
struct Basis {
    base: Weak<Base>,
    rows: Option<Weak<Vec<RowId>>>,
    len: usize,
    header: bool,
    cells: Weak<Vec<Arc<Layer>>>,
    field: Option<usize>,
    kind: Kind,
}

/// The table and rows shown now, to compare with a `Basis` (addresses only: nothing held).
struct Now<'a> {
    st: &'a csvfab_engine::State,
    rows: Option<&'a Arc<Vec<RowId>>>,
    len: usize,
    kinds: &'a [Kind],
}

impl<'a> Now<'a> {
    fn of(tab: &'a Tab, st: &'a csvfab_engine::State) -> Now<'a> {
        Now { st, rows: tab.view.as_ref().or(st.order.as_ref()), len: tab.shown(), kinds: &tab.kinds }
    }

    fn kind(&self, c: usize) -> Kind {
        self.kinds.get(c).copied().unwrap_or(Kind::Text)
    }

    /// What column `c`'s totals count now.
    fn basis(&self, c: usize) -> Option<Basis> {
        Some(Basis {
            base: Arc::downgrade(&self.st.base),
            rows: self.rows.map(Arc::downgrade),
            len: self.len,
            header: self.st.header,
            cells: Arc::downgrade(self.st.cols.get(c)?),
            field: csvfab_engine::colcache::field_of(self.st, c),
            kind: self.kind(c),
        })
    }

    fn same_rows(&self, base: &Weak<Base>, rows: &Option<Weak<Vec<RowId>>>, len: usize) -> bool {
        base.as_ptr() == Arc::as_ptr(&self.st.base) && rows.as_ref().map_or(std::ptr::null(), Weak::as_ptr) == self.rows.map_or(std::ptr::null(), Arc::as_ptr) && len == self.len
    }

    /// Whether `b` is what column `c`'s totals count now.
    fn is(&self, b: &Basis, c: usize) -> bool {
        self.same_rows(&b.base, &b.rows, b.len)
            && b.header == self.st.header
            && self.st.cols.get(c).is_some_and(|l: &Layers| b.cells.as_ptr() == Arc::as_ptr(l))
            && b.field == csvfab_engine::colcache::field_of(self.st, c)
            && b.kind == self.kind(c)
    }
}

/// The rows shown and the table's cells, as last seen: the quiet clock restarts when they change.
struct Seen {
    base: Weak<Base>,
    rows: Option<Weak<Vec<RowId>>>,
    len: usize,
    cols: Weak<Vec<Layers>>,
}

impl Seen {
    fn of(now: &Now) -> Seen {
        Seen { base: Arc::downgrade(&now.st.base), rows: now.rows.map(Arc::downgrade), len: now.len, cols: Arc::downgrade(&now.st.cols) }
    }

    fn is(&self, now: &Now) -> bool {
        now.same_rows(&self.base, &self.rows, self.len) && self.cols.as_ptr() == Arc::as_ptr(&now.st.cols)
    }
}

#[derive(Default)]
pub struct State {
    /// The functions picked, by column (None: no total); a column not here takes its default.
    choice: HashMap<usize, Option<TotalFn>>,
    defaults: Option<(DefKey, Arc<Vec<bool>>)>,
    def_job: Option<(Job<Vec<bool>>, DefKey)>,
    /// Per column, its totals and what they counted.
    vals: HashMap<usize, (Totals, Basis)>,
    /// The pass under way, and what each of its columns counts.
    job: Option<(Job<Option<Vec<Totals>>>, Vec<(usize, Basis)>)>,
    /// The rows and cells last seen and since when: the pass waits for QUIET.
    seen: Option<(Seen, Instant)>,
}

/// The functions picked, by column (`State::choices`).
pub type Choices = HashMap<usize, Option<TotalFn>>;

impl State {
    /// The functions picked, by column: carried over a structural edit, kept for Undo.
    pub fn choices(&self) -> Choices {
        self.choice.clone()
    }

    pub fn set_choices(&mut self, c: Choices) {
        self.choice = c;
        self.vals.clear();
    }

    /// Column `c`'s function: the one picked, else the default (a sum for quantities).
    pub fn func(&self, c: usize) -> Option<TotalFn> {
        match self.choice.get(&c) {
            Some(f) => *f,
            None => self.defaults.as_ref().and_then(|d| d.1.get(c).copied()).unwrap_or(false).then_some(TotalFn::Sum),
        }
    }

    /// A sum under every number column (the Σ menu's first item; the palette).
    pub fn sum_all(&mut self, kinds: &[Kind], ncols: usize) {
        self.choice = (0..ncols).map(|c| (c, (kinds.get(c) == Some(&Kind::Number)).then_some(TotalFn::Sum))).collect();
    }

    /// No total under any column (the palette's "clear every total").
    pub fn clear_all(&mut self, ncols: usize) {
        self.choice = (0..ncols).map(|c| (c, None)).collect();
    }

    /// Forgets the choices tied to column positions (columns inserted, moved, deleted).
    pub fn reset_columns(&mut self) {
        self.choice.clear();
        self.vals.clear();
    }

    /// Whether column `c` has the totals `f` needs, counted on what it holds now.
    fn fresh(&self, now: &Now, c: usize, f: TotalFn) -> bool {
        self.vals.get(&c).is_some_and(|(t, b)| t.has(f) && now.is(b, c))
    }
}

fn kind_of(tab: &Tab, c: usize) -> Kind {
    tab.kinds.get(c).copied().unwrap_or(Kind::Text)
}

/// Picks up finished passes; starts the one the row needs once things are quiet: only the
/// columns whose totals are missing or were counted on other rows or cells.
fn poll(tab: &mut Tab, ctx: &egui::Context) {
    let Some(st) = tab.st().cloned() else { return };
    let busy = tab.busy();
    let dkey = DefKey { base: Arc::as_ptr(&st.base) as usize, ncols: st.ncols, header: st.header };
    let kinds = tab.kinds.clone();
    let view = tab.view.clone();
    let now = Now { st: &st, rows: view.as_ref().or(st.order.as_ref()), len: tab.shown(), kinds: &kinds };
    let s = &mut tab.totals;
    // The quiet clock runs from the last change of the rows or cells, defaults known or not;
    // the row just shown counts what is there (nothing changing: no wait).
    match &s.seen {
        None => s.seen = Some((Seen::of(&now), Instant::now().checked_sub(QUIET).unwrap_or_else(Instant::now))),
        Some(x) if !x.0.is(&now) => s.seen = Some((Seen::of(&now), Instant::now())),
        _ => {}
    }
    if let Some((j, k)) = &mut s.def_job {
        if let Some(d) = j.poll() {
            s.defaults = Some((*k, Arc::new(d)));
            s.def_job = None;
        }
    }
    if s.defaults.as_ref().map(|d| d.0) != Some(dkey) && s.def_job.as_ref().map(|d| d.1) != Some(dkey) {
        let (st2, kinds2) = (st.clone(), kinds.clone());
        s.def_job = Some((Job::spawn(ctx, move |_, _| {
            let heads = st2.headers();
            (0..st2.ncols).map(|c| stats::sum_by_default(&st2, c, kinds2.get(c).copied().unwrap_or(Kind::Text), &heads[c])).collect()
        }), dkey));
    }
    if s.defaults.as_ref().is_none_or(|d| d.0 != dkey) {
        return;
    }
    if let Some((j, keys)) = &mut s.job {
        if let Some(r) = j.poll() {
            let keys = std::mem::take(keys);
            s.job = None;
            for t in r.into_iter().flatten() {
                if let Some((_, b)) = keys.iter().find(|k| k.0 == t.col) { s.vals.insert(t.col, (t, b.clone())); }
            }
            ctx.request_repaint();
        }
    }
    let todo: Vec<(TotalReq, Basis)> = (0..st.ncols)
        .filter_map(|c| {
            let f = s.func(c)?;
            if s.fresh(&now, c, f) { return None; }
            Some((TotalReq { col: c, kind: now.kind(c), median: f == TotalFn::Median, distinct: f == TotalFn::Distinct }, now.basis(c)?))
        })
        .collect();
    // Nothing missing, or the pass under way counts exactly that.
    let running = s.job.as_ref().is_some_and(|(_, keys)| todo.iter().all(|(r, _)| keys.iter().any(|(c, b)| *c == r.col && now.is(b, *c))));
    if todo.is_empty() || running {
        return;
    }
    let quiet = s.seen.as_ref().map_or(Duration::ZERO, |x| x.1.elapsed());
    if busy || quiet < QUIET {
        ctx.request_repaint_after(QUIET.saturating_sub(quiet) + Duration::from_millis(5));
        return;
    }
    let (reqs, keys): (Vec<TotalReq>, Vec<(usize, Basis)>) = todo.into_iter().map(|(r, b)| (r, (r.col, b))).unzip();
    // A pass for other rows or columns: no use finishing it (dropped, it stops).
    s.job = Some((Job::spawn(ctx, move |cancel, _| {
        let all: Vec<RowId>;
        let ids: &[RowId] = match &view {
            Some(v) => v,
            None => {
                all = (0..st.len()).map(|p| st.data_id(p)).collect();
                &all
            }
        };
        stats::totals(&st, ids, &reqs, cancel)
    }), keys));
}

/// After cells were set (`changes`, applied to `before` already): the totals of their columns,
/// when they counted `before`, follow the few cells changed instead of being counted again —
/// a cell typed under a sum over millions of rows.
pub fn edited(tab: &mut Tab, before: &csvfab_engine::State, changes: &[(RowId, usize, String)]) {
    let Some(st) = tab.st().cloned() else { return };
    // Rows hidden by the filters do not count: a cell of the view, or of every data row.
    let shown = |id: RowId| match &tab.view {
        Some(v) => v.contains(&id),
        None => before.header.then(|| before.all_id(0)) != Some(id),
    };
    let ins: Vec<bool> = changes.iter().map(|c| shown(c.0)).collect();
    let kinds = tab.kinds.clone();
    let now = Now::of(tab, before);
    let mut cols: Vec<usize> = changes.iter().map(|c| c.1).collect();
    cols.sort_unstable();
    cols.dedup();
    let mut keep = Vec::new();
    for &c in &cols {
        let Some((mut t, b)) = tab.totals.vals.get(&c).cloned() else { continue };
        if !now.is(&b, c) { continue; }
        // In order, a cell given twice reading what the first one wrote.
        let mut cur: HashMap<RowId, &str> = HashMap::new();
        let mut tmp = Vec::new();
        let ok = changes.iter().zip(&ins).filter(|(ch, &i)| ch.1 == c && i).all(|(ch, _)| {
            let old = cur.get(&ch.0).map(|s| s.to_string()).unwrap_or_else(|| before.with_cell(ch.0, c, &mut tmp, |v| v.to_string()));
            cur.insert(ch.0, &ch.2);
            t.edit(&old, &ch.2)
        });
        if ok { keep.push((c, t)); }
    }
    let now = Now { st: &st, rows: tab.view.as_ref().or(st.order.as_ref()), len: tab.shown(), kinds: &kinds };
    for (c, mut t) in keep {
        if t.kind == Kind::Number {
            // Which decimals the first numbers write: a few thousand cells read at most.
            t.dec = match &tab.view {
                Some(v) => stats::total_decimals(&st, v.iter().copied(), c),
                None => stats::total_decimals(&st, (0..st.len()).map(|p| st.data_id(p)), c),
            };
        }
        if let Some(b) = now.basis(c) { tab.totals.vals.insert(c, (t, b)); }
    }
}

/// The text under column `c` (a test's look at the row).
#[cfg(test)]
pub fn value(tab: &Tab, c: usize) -> Option<String> {
    let f = tab.totals.func(c)?;
    tab.totals.vals.get(&c)?.0.value(f).map(|v| v.text)
}

/// Whether a total is still to come (counting, or waiting for a quiet moment).
pub fn pending(tab: &Tab) -> bool {
    tab.show_totals && stale(tab)
}

/// Whether some values shown are for other rows or cells, or are still missing.
fn stale(tab: &Tab) -> bool {
    let Some(st) = tab.st() else { return false };
    let now = Now::of(tab, st);
    let s = &tab.totals;
    (0..st.ncols).any(|c| s.func(c).is_some_and(|f| !s.fresh(&now, c, f)))
}

/// The row's frame and its Σ cell (under the row numbers); the cells come from `cells`.
pub fn row(ui: &mut Ui, tab: &mut Tab, painter: &Painter, r: Rect, p: &Palette, m: &Metrics, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    poll(tab, &ctx);
    // A shadow upwards (0 −4 12 rgba(0,0,0,.18)), then the row.
    let sh = Rect::from_min_max(pos2(r.min.x, r.min.y - 10.0), pos2(r.max.x, r.min.y));
    let mut mesh = Mesh::default();
    let (c0, c1) = (Color32::TRANSPARENT, Color32::from_black_alpha(34));
    mesh.colored_vertex(sh.left_top(), c0);
    mesh.colored_vertex(sh.right_top(), c0);
    mesh.colored_vertex(sh.right_bottom(), c1);
    mesh.colored_vertex(sh.left_bottom(), c1);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    painter.rect_filled(r, 0.0, p.surf);
    painter.hline(r.x_range(), r.min.y + 0.5, Stroke::new(1.0, p.brd_2));
    let gr = Rect::from_min_size(pos2(r.min.x, r.min.y + 1.0), vec2(m.gutter_w - m.gutter_rule_w, r.height() - 1.0));
    let resp = ui.interact(gr, ui.scope_id().with("tot-sigma"), Sense::click());
    if resp.hovered() { painter.rect_filled(gr, 0.0, p.surf_2); }
    painter.text(gr.center(), Align2::CENTER_CENTER, "Σ", grid::semibold(15.0), p.txt_mut);
    let shown = tab.shown();
    if resp.hovered() && !egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&resp)) {
        grid::tip(&ctx, p, m, gr, "Totals of the rows shown\nClick: sum every number column, clear, hide");
    }
    if resp.secondary_clicked() { egui::Popup::open_id(&ctx, egui::Popup::default_response_id(&resp)); }
    let menu = egui::Popup::menu(&resp).align(RectAlign::TOP_START).gap(4.0).frame(crate::theme::menu_frame(p, m));
    menu.show(|ui| {
        width(ui, 230.0);
        head(ui, p, &format!("Totals · {} rows shown", grid::fmt_int(shown)));
        let ncols = tab.st().map_or(0, |s| s.ncols);
        if Item::new("Sum every number column").show(ui, p).clicked() {
            let kinds = tab.kinds.clone();
            tab.totals.sum_all(&kinds, ncols);
            ui.close();
        }
        if Item::new("Back to the usual totals").show(ui, p).clicked() { tab.totals.choice.clear(); ui.close(); }
        if Item::new("Clear every total").show(ui, p).clicked() { tab.totals.choice = (0..ncols).map(|c| (c, None)).collect(); ui.close(); }
        sep(ui, p);
        if Item::new("Hide the totals row").icon(crate::icons::Icon::Totals).show(ui, p).clicked() { actions.push(Action::ToggleTotals); ui.close(); }
    });
}

/// The cells of columns `(col, x, width)`, clipped to `clip` (right of the gutter, or of the frozen columns).
#[allow(clippy::too_many_arguments)]
pub fn cells(ui: &mut Ui, tab: &mut Tab, painter: &Painter, r: Rect, clip: Rect, vis_cols: &[(usize, f32, f32)], p: &Palette, m: &Metrics, opaque: bool) {
    let ctx = ui.ctx().clone();
    let clip = clip.intersect(painter.clip_rect());
    let pt = painter.with_clip_rect(clip);
    let dim = stale(tab);
    let label_font = grid::semibold(10.0);
    let value_font = FontId::new(13.0, FontFamily::Name(crate::fonts::MONO_BOLD.into()));
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let st = tab.st().cloned();
    let fresh: Vec<bool> = st.as_ref().map(|st| { let now = Now::of(tab, st); (0..st.ncols).map(|c| tab.totals.vals.get(&c).is_some_and(|(_, b)| now.is(b, c))).collect() }).unwrap_or_default();
    let heads = st.as_ref().map(|s| s.headers()).unwrap_or_default();
    for &(c, x, w) in vis_cols {
        let cr = Rect::from_min_size(pos2(x, r.min.y + 1.0), vec2(w, r.height() - 1.0));
        if cr.max.x <= clip.min.x {
            continue;
        }
        let resp = ui.interact(cr.intersect(clip), ui.scope_id().with(("tot", c)), Sense::click());
        let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&resp));
        if opaque || resp.hovered() || open { pt.rect_filled(cr, 0.0, if resp.hovered() || open { p.surf_2 } else { p.surf }); }
        pt.vline(x + w - 0.5, cr.y_range(), Stroke::new(1.0, p.brd));
        let kind = kind_of(tab, c);
        let f = tab.totals.func(c);
        let cy = cr.center().y;
        let mut tip = String::new();
        match f {
            None => {
                if resp.hovered() {
                    pt.text(cr.center(), Align2::CENTER_CENTER, "Σ", grid::semibold(13.0), p.txt_faint);
                    tip = "Click: a total for this column".into();
                }
            }
            Some(f) => {
                let v = tab.totals.vals.get(&c).and_then(|t| t.0.value(f));
                let lab = f.labels(kind).0.to_uppercase();
                let mut lj = egui::text::LayoutJob::single_section(lab.clone(), egui::TextFormat { font_id: label_font.clone(), color: p.txt_dim, extra_letter_spacing: 0.5, ..Default::default() });
                lj.wrap.max_rows = 1;
                let lg = pt.layout_job(lj);
                let numeric = matches!(f, TotalFn::Sum | TotalFn::Avg | TotalFn::Median | TotalFn::Min | TotalFn::Max);
                let vc = match (kind, numeric) {
                    (Kind::Number, true) if crate::theme::type_colors() => p.num_txt,
                    (Kind::Date, true) if crate::theme::type_colors() => p.date_txt,
                    _ => p.txt_strong,
                };
                let vc = if dim { vc.gamma_multiply(0.45) } else { vc };
                let text = v.as_ref().map_or("…".to_string(), |v| v.text.clone());
                let room = w - 2.0 * m.cell_pad_x - 1.0;
                let full_w = pt.layout_no_wrap(text.clone(), value_font.clone(), vc).size().x;
                // The name gives way to the value when both do not fit (still in the tooltip).
                let with_label = full_w <= room - lg.size().x - 8.0;
                let mut vj = egui::text::LayoutJob::simple_singleline(text.clone(), value_font.clone(), vc);
                vj.wrap = egui::text::TextWrapping { max_width: if with_label { room - lg.size().x - 8.0 } else { room }.max(4.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let vg = pt.layout_job(vj);
                if with_label {
                    pt.galley(pos2(snap(x + m.cell_pad_x), snap(cy - lg.size().y / 2.0)), lg, p.txt_dim);
                }
                pt.galley(pos2(snap(x + w - 1.0 - m.cell_pad_x - vg.size().x), snap(cy - vg.size().y / 2.0)), vg, vc);
                tip = match &v {
                    // The name and value when the cell only has room for the value, and what it is made of.
                    Some(v) => [if with_label { String::new() } else { format!("{}: {}", f.labels(kind).0, v.text) }, v.tip.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" — "),
                    None => String::new(),
                };
            }
        }
        if resp.hovered() && !open && !tip.is_empty() {
            grid::tip(&ctx, p, m, cr, &tip);
        }
        // The menu: every function the column's kind takes, with its value when known.
        // A right click opens it too (as the web's).
        if resp.secondary_clicked() { egui::Popup::open_id(&ctx, egui::Popup::default_response_id(&resp)); }
        let menu = egui::Popup::menu(&resp).align(RectAlign::TOP_START).gap(4.0).frame(crate::theme::menu_frame(p, m));
        menu.show(|ui| {
            width(ui, 240.0);
            head(ui, p, &format!("{} · total of the rows shown", heads.get(c).map_or("", |s| s.as_str())));
            let t = if fresh.get(c) == Some(&true) { tab.totals.vals.get(&c).map(|t| &t.0) } else { None };
            let fns = TotalFn::for_kind(kind);
            let any_from = fns.iter().position(|f| *f == TotalFn::Filled).unwrap_or(0);
            for (i, &g) in fns.iter().enumerate() {
                if i == any_from && i > 0 { sep(ui, p); }
                let val = t.and_then(|t| t.value(g)).map(|v| v.text).unwrap_or_default();
                let on = f == Some(g);
                let item = Item::new(g.labels(kind).1).keys(&val).checked(on);
                if item.show(ui, p).clicked() {
                    tab.totals.choice.insert(c, Some(g));
                    ui.close();
                }
            }
            sep(ui, p);
            if Item::new("None").checked(f.is_none()).show(ui, p).clicked() {
                tab.totals.choice.insert(c, None);
                ui.close();
            }
            if let Some(v) = f.and_then(|f| t.and_then(|t| t.value(f))).filter(|v| !v.raw.is_empty()) {
                sep(ui, p);
                if Item::new("Copy the value").icon(crate::icons::Icon::Copy).keys(&v.raw).show(ui, p).clicked() {
                    ui.ctx().copy_text(v.raw.clone());
                    let name = f.map(|f| f.labels(kind).1).unwrap_or_default();
                    tab.grid.notice = Some(format!("{name} of \"{}\" copied: {}", heads.get(c).map_or("", |s| s.as_str()), v.raw));
                    ui.close();
                }
            }
        });
    }
}
