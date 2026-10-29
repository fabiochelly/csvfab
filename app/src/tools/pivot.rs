//! Pivot table (43-pivot.js): the rows shown, one column's values down the
//! side, another's across the top (or none), and in each cell a count, sum,
//! average, min, max or number of distinct values — totals, a colour scale,
//! dates grouped by month, quarter, year or day. Computed again in the
//! background at every change (the last result stays drawn meanwhile); a
//! click on a title sorts, a click on a number shows the rows it counts
//! (`Tab::restrict_to`). Nothing in the tab changes otherwise: the table is
//! copied (tab-separated) or opened as a new tab.

use super::analysis_kit as kit;
use super::group::{italic, stat_line};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::grid::semibold;
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Palette};
use csvfab_engine::pivot::{self, Dates, Fun, Pivot, PivotError, PivotSort, PivotSpec, SortBy, SHOW_COLS, SHOW_ROWS};
use csvfab_engine::view::Kind;
use csvfab_engine::{RowId, State};
use eframe::egui::{self, pos2, vec2, Color32, Rect, Sense, Stroke, StrokeKind, Ui};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

const DATES: [(Dates, &str); 5] = [(Dates::Month, "by month"), (Dates::Quarter, "by quarter"), (Dates::Year, "by year"), (Dates::Day, "by day"), (Dates::AsWritten, "as written")];
const ROW_H: f32 = 24.0;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Pivot table…", group: "Rows", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(open(cx.tab, None))) }]
}

thread_local! {
    /// The last pivot of each tab (by uid): the menu brings it back after a drill-down.
    static LAST: RefCell<HashMap<u64, (PivotSpec, PivotSort, bool)>> = RefCell::new(HashMap::new());
}

/// The dialog on `tab`, `col` down the side when given (the column panel's "Pivot table by").
pub fn open(tab: &Tab, col: Option<usize>) -> Box<dyn Tool> {
    Box::new(PivotTool::new(tab, col))
}

/// The rows drawn, as text, with what the colour scale needs.
struct Drawn {
    /// Every row, in the order asked (the drill-down, the copy and the new tab follow it).
    order: Arc<Vec<u32>>,
    /// The columns across drawn (the largest totals when too many).
    cols: Vec<usize>,
    /// Per row drawn: its label, its cells (as `cols`), its total.
    labels: Vec<String>,
    cells: Vec<Vec<Option<(f64, String)>>>,
    totals: Vec<(f64, String)>,
    col_totals: Vec<String>,
    grand: String,
    heat: Option<(f64, f64)>,
    /// Column widths, measured once drawn: label, each of `cols`, total.
    widths: Option<(f32, Vec<f32>, f32)>,
}

enum Export {
    Copy(String),
    Tab(String, Vec<u8>, usize),
    /// The job panicked (told by a toast).
    Failed,
}

impl crate::job::Failed for Export {
    fn failed(_: &str) -> Self { Export::Failed }
}

struct PivotTool {
    uid: u64,
    st: State,
    headers: Vec<String>,
    kinds: Vec<Kind>,
    ids: Arc<Vec<RowId>>,
    filtered: bool,
    spec: PivotSpec,
    sort: PivotSort,
    heat: bool,
    res: Option<Arc<Pivot>>,
    drawn: Option<Drawn>,
    no_value: bool,
    job: Option<Job<Result<Pivot, PivotError>>>,
    prep: Option<Job<Option<Drawn>>>,
    drill: Option<Job<Option<(Vec<RowId>, String)>>>,
    export: Option<Job<Export>>,
    stale: bool,
}

impl PivotTool {
    fn new(tab: &Tab, col: Option<usize>) -> PivotTool {
        let st = tab.st().cloned().expect("a loaded tab");
        let ids = tab.shown_ids();
        let kinds = tab.kinds.clone();
        let last = LAST.with(|m| m.borrow().get(&tab.uid).cloned()).filter(|l| l.0.rows < st.ncols && l.0.cols.is_none_or(|c| c < st.ncols) && l.0.value.is_none_or(|c| c < st.ncols));
        let (mut spec, mut sort, heat) = last.unwrap_or_else(|| (PivotSpec::guess(&st, &ids, &kinds, col), PivotSort::default(), true));
        if let Some(c) = col {
            if spec.rows != c { spec.rows = c; sort = PivotSort::default(); }
            if spec.cols == Some(c) { spec.cols = None; }
        }
        PivotTool {
            uid: tab.uid,
            headers: st.headers(),
            st,
            kinds,
            ids,
            filtered: tab.filtered(),
            spec,
            sort,
            heat,
            res: None,
            drawn: None,
            no_value: false,
            job: None,
            prep: None,
            drill: None,
            export: None,
            stale: true,
        }
    }

    fn remember(&self) {
        LAST.with(|m| m.borrow_mut().insert(self.uid, (self.spec.clone(), self.sort, self.heat)));
    }

    fn is_date(&self, c: usize) -> bool {
        self.kinds.get(c) == Some(&Kind::Date)
    }

    /// Computes again; the job running is dropped (cancelled), the last table stays drawn.
    fn compute(&mut self, ctx: &egui::Context) {
        self.stale = false;
        self.job = None;
        self.no_value = self.spec.fun != Fun::Count && self.spec.value.is_none();
        if self.no_value { self.res = None; self.drawn = None; self.prep = None; return; }
        let (st, ids, spec, kinds) = (self.st.clone(), self.ids.clone(), self.spec.clone(), self.kinds.clone());
        self.job = Some(Job::spawn(ctx, move |cancel, progress| pivot::pivot(&st, &ids, &spec, &kinds, cancel, Some(progress))));
        self.remember();
    }

    /// The rows in the order asked, as text, in the background (millions of rows sort there).
    fn prepare(&mut self, ctx: &egui::Context) {
        let Some(res) = self.res.clone() else { return };
        let sort = self.sort;
        self.prep = Some(Job::spawn(ctx, move |cancel, _| {
            let order = res.order(sort);
            if cancel.load(Ordering::Relaxed) { return None; }
            let cols = res.shown_cols(SHOW_COLS);
            let shown = &order[..order.len().min(SHOW_ROWS)];
            let heat = res.heat_range(shown, &cols);
            let labels = shown.iter().map(|&r| res.row_keys[r as usize].clone()).collect();
            let cells = shown.iter().map(|&r| cols.iter().map(|&c| res.value(r as usize, c).map(|x| (x, res.display(x)))).collect()).collect();
            let totals = shown.iter().map(|&r| { let x = res.row_totals[r as usize]; (x, res.display(x)) }).collect();
            let col_totals = cols.iter().map(|&c| res.display(res.col_totals[c])).collect();
            let grand = res.total.map_or(String::new(), |x| res.display(x));
            Some(Drawn { order: Arc::new(order), cols, labels, cells, totals, col_totals, grand, heat, widths: None })
        }));
    }

    /// The rows behind a cell: row `i` drawn (None: every row), column `j` across (None: any).
    fn drill_down(&mut self, ctx: &egui::Context, i: Option<usize>, j: Option<usize>) {
        let (Some(res), Some(d)) = (&self.res, &self.drawn) else { return };
        let row = i.map(|i| res.row_keys[d.order[i] as usize].clone());
        let col = j.map(|j| res.col_keys[j].clone());
        let label = drill_label(&self.headers, &self.spec, row.as_deref(), col.as_deref());
        let (st, ids, spec, kinds) = (self.st.clone(), self.ids.clone(), self.spec.clone(), self.kinds.clone());
        self.drill = Some(Job::spawn(ctx, move |cancel, _| {
            pivot::drill(&st, &ids, &spec, &kinds, row.as_deref(), col.as_deref(), cancel).map(|v| (v, label))
        }));
    }

    fn export(&mut self, ctx: &egui::Context, tab: bool, stem: String) {
        let (Some(res), Some(d)) = (self.res.clone(), &self.drawn) else { return };
        let order = d.order.clone();
        let delim = self.st.base.dialect.delim;
        self.export = Some(Job::spawn(ctx, move |_, _| {
            if tab { Export::Tab(res.tab_name(&stem), res.to_csv(&order, delim), order.len()) } else { Export::Copy(res.to_tsv(&order, delim == b';')) }
        }));
    }
}

/// "Region = Nord · Status = (empty)": what the rows of a drilled cell share.
fn drill_label(headers: &[String], spec: &PivotSpec, row: Option<&str>, col: Option<&str>) -> String {
    let part = |c: usize, k: &str| format!("{} = {}", headers.get(c).map_or("", String::as_str), if k.is_empty() { "(empty)" } else { k });
    let parts: Vec<String> = [row.map(|k| part(spec.rows, k)), col.and_then(|k| spec.cols.map(|c| part(c, k)))].into_iter().flatten().collect();
    if parts.is_empty() { "pivot rows".into() } else { parts.join(" · ") }
}

/// A click on a title: the same title flips the direction, another sorts from its start.
fn next_sort(cur: PivotSort, by: SortBy) -> PivotSort {
    PivotSort { by, flip: cur.by == by && !cur.flip }
}

/// The arrow after a sorted title: labels A→Z ▲, values largest first ▼.
fn arrow(cur: PivotSort, by: SortBy) -> &'static str {
    if cur.by != by { return ""; }
    match (by == SortBy::Label, cur.flip) {
        (true, false) | (false, true) => " ▲",
        _ => " ▼",
    }
}

/// The colour scale (a spreadsheet's): from the smallest number drawn to the largest.
fn heat_alpha(x: f64, (lo, hi): (f64, f64)) -> f32 {
    (0.06 + 0.6 * if hi > lo { (x - lo) / (hi - lo) } else { 0.5 }) as f32
}

/// Where a pointer is on the table.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    /// A title: the corner (labels), column j across, the total.
    Head(SortBy),
    /// Row i drawn: its label, its cell j, its total.
    Label(usize),
    Cell(usize, usize),
    Total(usize),
    /// The total line: column j across.
    ColTotal(usize),
    Nothing,
}

impl Tool for PivotTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if cx.tab.table.is_none() { return false; }
        if self.stale { self.compute(cx.ctx); }
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            match r {
                Ok(p) => { self.res = Some(Arc::new(p)); self.prepare(cx.ctx); }
                Err(PivotError::NoValue) => { self.no_value = true; self.res = None; self.drawn = None; }
                Err(PivotError::Cancelled) => {}
            }
        }
        if let Some(r) = self.prep.as_mut().and_then(|j| j.poll()) {
            self.prep = None;
            if let Some(d) = r { self.drawn = Some(d); }
        }
        if let Some(r) = self.drill.as_mut().and_then(|j| j.poll()) {
            self.drill = None;
            if let Some((ids, label)) = r {
                cx.toast(format!("{}: {label}", kit::plural(ids.len(), "row", "rows")));
                cx.tab.restrict_to(label, ids, cx.ctx);
                return false;
            }
        }
        if let Some(e) = self.export.as_mut().and_then(|j| j.poll()) {
            self.export = None;
            match e {
                Export::Copy(text) => { cx.ctx.copy_text(text); cx.toast("Pivot table copied — paste it into a spreadsheet."); }
                Export::Tab(name, bytes, rows) => {
                    cx.out.push(Cmd::OpenBytes { name, bytes });
                    cx.toast(format!("{} — a new tab, not on disk yet: Save as… writes it.", kit::plural(rows, "row", "rows")));
                    return false;
                }
                Export::Failed => {}
            }
        }
        if self.drawn.is_some() && self.drill.is_none() && crate::shot::take("pivot-drill").is_some() {
            let j = self.drawn.as_ref().and_then(|d| d.cols.first().copied()).filter(|_| self.spec.cols.is_some());
            self.drill_down(cx.ctx, Some(0), j);
        }

        let p = cx.p;
        let frame = egui::Frame::new().fill(p.surf).stroke(Stroke::new(1.0, p.brd)).corner_radius(8).inner_margin(20)
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(128) });
        let screen = cx.ctx.content_rect();
        let (w, h) = (((1200f32).min(screen.width() - 40.0) - 40.0).max(0.0), (screen.height() * 0.9 - 40.0).max(0.0));
        let ctx = cx.ctx.clone();
        let r = crate::widgets::modal(&ctx, egui::Id::unique("pivot-modal"), p, frame, |ui| {
            ui.set_width(w);
            ui.set_height(h);
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            self.body(ui, cx, h)
        });
        r.inner && !r.should_close()
    }
}

impl PivotTool {
    fn body(&mut self, ui: &mut Ui, cx: &mut Cx, h: f32) -> bool {
        let p = cx.p;
        let top = ui.min_rect().min.y;
        ui.label(egui::RichText::new("Pivot table").font(semibold(16.0)).color(p.txt_hi));
        ui.add_space(12.0);
        self.controls(ui, p);
        ui.add_space(10.0);
        self.stats(ui, p);
        ui.add_space(10.0);
        let left = (top + h - ui.cursor().min.y - 14.0 - 30.0).max(120.0);
        let (wrap, _) = ui.allocate_exact_size(vec2(ui.available_width(), left), Sense::hover());
        if let Some(spot) = self.table(ui, p, wrap) {
            match spot {
                Spot::Head(by) => { self.sort = next_sort(self.sort, by); self.remember(); self.prepare(cx.ctx); }
                Spot::Label(i) | Spot::Total(i) => self.drill_down(cx.ctx, Some(i), None),
                Spot::Cell(i, j) => self.drill_down(cx.ctx, Some(i), Some(j)),
                Spot::ColTotal(j) => self.drill_down(cx.ctx, None, Some(j)),
                Spot::Nothing => {}
            }
        }
        let mut keep = true;
        let ready = self.drawn.is_some() && self.export.is_none();
        kit::actions(ui, 14.0, |ui| {
            if kit::button(ui, p, "Close", kit::Btn::Primary, true).clicked() { keep = false; }
            let stem = csvfab_engine::schema::stem(&cx.tab.name).to_string();
            if kit::button(ui, p, "Open in a new tab", kit::Btn::Outline, ready).clicked() { self.export(cx.ctx, true, stem.clone()); }
            if kit::button(ui, p, "Copy", kit::Btn::Outline, ready).clicked() { self.export(cx.ctx, false, stem); }
        });
        keep
    }

    /// `.pt-ctl`: Rows [col] [dates] ⇄ Columns [col] [dates] Show [fn] of [col] ☐ Colour scale — wrapping.
    fn controls(&mut self, ui: &mut Ui, p: &Palette) {
        let headers = self.headers.clone();
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            let lab = |ui: &mut Ui, t: &str| { ui.add(egui::Label::new(egui::RichText::new(t).font(kit::ui(12.0)).color(p.txt_mut)).selectable(false)); };
            let pick = |ui: &mut Ui, id: &str, sel: &mut usize, opts: &[String]| -> bool {
                let w = opts.iter().map(|o| ui.painter().layout_no_wrap(o.clone(), kit::ui(14.0), p.txt_hi).size().x).fold(0.0, f32::max);
                kit::select(ui, p, egui::Id::unique(("pt", id)), vec2((w + 48.0).clamp(90.0, 220.0), 31.0), sel, opts)
            };
            let dates: Vec<String> = DATES.iter().map(|d| d.1.to_string()).collect();
            let date_ix = |d: Dates| DATES.iter().position(|x| x.0 == d).unwrap_or(0);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                lab(ui, "Rows");
                let mut r = self.spec.rows;
                if pick(ui, "r", &mut r, &headers) { self.spec.rows = r; if self.spec.cols == Some(r) { self.spec.cols = None; } changed = true; }
            });
            if self.is_date(self.spec.rows) {
                let mut k = date_ix(self.spec.rows_by);
                if pick(ui, "rd", &mut k, &dates) { self.spec.rows_by = DATES[k].0; changed = true; }
            }
            if swap_button(ui, p).on_hover_text("Swap rows and columns").clicked() {
                if let Some(c) = self.spec.cols {
                    self.spec.cols = Some(self.spec.rows);
                    self.spec.rows = c;
                    std::mem::swap(&mut self.spec.rows_by, &mut self.spec.cols_by);
                    self.sort = PivotSort::default();
                    changed = true;
                }
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                lab(ui, "Columns");
                let opts: Vec<String> = std::iter::once("(nothing: one column)".to_string()).chain(headers.iter().cloned()).collect();
                let mut k = self.spec.cols.map_or(0, |c| c + 1);
                if pick(ui, "c", &mut k, &opts) { self.spec.cols = (k > 0 && k - 1 != self.spec.rows).then(|| k - 1); changed = true; }
            });
            if let Some(c) = self.spec.cols.filter(|&c| self.is_date(c)) {
                let _ = c;
                let mut k = date_ix(self.spec.cols_by);
                if pick(ui, "cd", &mut k, &dates) { self.spec.cols_by = DATES[k].0; changed = true; }
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                lab(ui, "Show");
                let opts: Vec<String> = Fun::ALL.iter().map(|f| f.label().to_string()).collect();
                let mut k = Fun::ALL.iter().position(|&f| f == self.spec.fun).unwrap_or(0);
                if pick(ui, "fn", &mut k, &opts) { self.spec.fun = Fun::ALL[k]; changed = true; }
            });
            if self.spec.fun != Fun::Count {
                lab(ui, "of");
                let opts: Vec<String> = std::iter::once("(choose a column)".to_string()).chain(headers.iter().cloned()).collect();
                let mut k = self.spec.value.map_or(0, |c| c + 1);
                if pick(ui, "v", &mut k, &opts) { self.spec.value = (k > 0).then(|| k - 1); changed = true; }
            }
            let mut heat = self.heat;
            if kit::checkbox(ui, p, &mut heat, "Colour scale").clicked() { self.heat = heat; self.remember(); }
        });
        if changed {
            // Sorted by a column across: that column may be gone.
            if matches!(self.sort.by, SortBy::Col(_)) { self.sort = PivotSort::default(); }
            self.stale = true;
        }
    }

    /// `.stat-line`: the table's size, what is left out, how to see the rows.
    fn stats(&self, ui: &mut Ui, p: &Palette) {
        if self.no_value {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 19.0), Sense::hover());
            ui.painter().text(pos2(r.min.x, r.center().y), egui::Align2::LEFT_CENTER, "Choose the column to compute.", kit::ui(12.0), p.warn);
            return;
        }
        if let Some(j) = &self.job {
            if self.ids.len() > 100_000 || self.res.is_none() {
                let pc = (j.progress.load(Ordering::Relaxed) as f64 / self.ids.len().max(1) as f64 * 100.0).round().min(100.0);
                stat_line(ui, p, &format!("Computing… {pc} %"));
                return;
            }
        }
        let (Some(res), Some(d)) = (&self.res, &self.drawn) else { stat_line(ui, p, "Computing…"); return; };
        let mut notes = Vec::new();
        if res.row_keys.len() > SHOW_ROWS { notes.push(format!("the first {} of {} rows drawn", kit::n(SHOW_ROWS), kit::n(res.row_keys.len()))); }
        if d.cols.len() < res.col_keys.len() { notes.push(format!("the {} largest of {} columns drawn", kit::n(SHOW_COLS), kit::n(res.col_keys.len()))); }
        if res.skipped > 0 { notes.push(format!("{} not a number left out", kit::plural(res.skipped, "value", "values"))); }
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = ui.available_width();
        let seg = |job: &mut egui::text::LayoutJob, t: &str, c: Color32, bold: bool| {
            job.append(t, 0.0, egui::TextFormat { font_id: if bold { semibold(12.0) } else { kit::ui(12.0) }, color: c, line_height: Some(19.0), ..Default::default() });
        };
        seg(&mut job, &kit::n(res.row_keys.len()), p.txt_hi, true);
        seg(&mut job, " × ", p.txt_mut, false);
        seg(&mut job, &kit::n(if res.across { res.col_keys.len() } else { 1 }), p.txt_hi, true);
        seg(&mut job, &format!(" from {} {}rows", kit::n(res.rows), if self.filtered { "filtered " } else { "" }), p.txt_mut, false);
        if !notes.is_empty() { seg(&mut job, &format!(" — {}; the copy and the new tab hold everything", notes.join(" · ")), p.txt_faint, false); }
        seg(&mut job, " · click a number to see its rows", p.txt_faint, false);
        let g = ui.painter().layout_job(job);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.min.x, kit::snap(ui, r.min.y)), g, p.txt_mut);
    }

    /// The table (`.pv.pt` in `.pt-wrap`): title row and label column held in
    /// place while it scrolls, the total line stuck to the bottom. Returns what was clicked.
    fn table(&mut self, ui: &mut Ui, p: &Palette, wrap: Rect) -> Option<Spot> {
        ui.painter().rect_stroke(wrap, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
        let (Some(res), Some(d)) = (self.res.clone(), self.drawn.as_mut()) else { return None };
        if self.no_value { return None; }
        let across = res.across;
        let mono = kit::mono(12.0);
        let titles = (
            self.headers.get(self.spec.rows).cloned().unwrap_or_default(),
            self.spec.cols.and_then(|c| self.headers.get(c).cloned()).unwrap_or_default(),
        );
        let total_title = if across { "Total".to_string() } else { self.spec.fun.label().to_string() };
        if d.widths.is_none() {
            let pt = ui.painter();
            let tw = |t: &str, f: egui::FontId| pt.layout_no_wrap(t.to_string(), f, p.txt).size().x;
            let digit = tw("0", mono.clone());
            let corner = tw(&titles.0, semibold(12.0)) + if across { tw(&format!(" \\ {}", titles.1), semibold(12.0)) } else { 0.0 } + 14.0;
            let lw = d.labels.iter().map(|l| tw(if l.is_empty() { "(empty)" } else { l }, semibold(12.0))).fold(corner, f32::max);
            let cw = d.cols.iter().enumerate().map(|(k, &c)| {
                let head = tw(if res.col_keys[c].is_empty() { "(empty)" } else { &res.col_keys[c] }, semibold(12.0)) + 14.0;
                let cells = d.cells.iter().filter_map(|l| l[k].as_ref().map(|x| x.1.chars().count())).max().unwrap_or(0).max(d.col_totals[k].chars().count());
                head.max(cells as f32 * digit) + 17.0
            }).collect();
            let tot = d.totals.iter().map(|t| t.1.chars().count()).max().unwrap_or(0).max(d.grand.chars().count()) as f32 * digit;
            let tot = tot.max(tw(&total_title, semibold(12.0)) + 14.0) + 17.0;
            d.widths = Some(((lw + 17.0).min(320.0), cw, tot));
        }
        let (lw, cws, tw) = d.widths.clone().unwrap();
        let cws: Vec<f32> = if across { cws } else { Vec::new() };
        let xs: Vec<f32> = cws.iter().scan(lw, |x, w| { let s = *x; *x += w; Some(s) }).collect();
        let tx = lw + cws.iter().sum::<f32>();
        let width = tx + tw;
        let n = d.labels.len();
        let inner = wrap.shrink(1.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("pt-table"));
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        let heat = if self.heat { d.heat } else { None };
        let sort = self.sort;
        let mut clicked = None;
        egui::ScrollArea::both().id_salt("pt-scroll").auto_shrink([false, false]).show_viewport(&mut child, |ui, vp| {
            ui.set_min_size(vec2(width, ROW_H * (n + 2) as f32));
            let o = ui.min_rect().min;
            let pt = ui.painter().clone();
            let (vx, vy) = (o.x + vp.min.x, o.y + vp.min.y);
            let vis = Rect::from_min_size(pos2(vx, vy), inner.size());
            // Where each band sits on screen: the title row at the view's top, the total
            // line under the last row or at the view's bottom, the labels at its left.
            let head_y = vy;
            let sum_y = (o.y + ROW_H * (n + 1) as f32).min(vy + inner.height() - ROW_H);
            let label_x = vx;
            let row_y = |i: usize| o.y + ROW_H * (i + 1) as f32;
            let resp = ui.interact(vis, egui::Id::unique("pt-hit"), Sense::click());
            let spot = |q: egui::Pos2| -> Spot {
                let col = |x: f32| -> Option<Option<usize>> {
                    // None: the label column; Some(None): the total; Some(Some(k)): column k drawn.
                    if x < label_x + lw { return None; }
                    let cx = x - o.x;
                    if cx >= tx { return Some(None); }
                    Some(xs.iter().zip(&cws).position(|(s, w)| cx >= *s && cx < s + w))
                };
                if q.y < head_y + ROW_H {
                    return match col(q.x) { None => Spot::Head(SortBy::Label), Some(None) => Spot::Head(SortBy::Total), Some(Some(k)) => Spot::Head(SortBy::Col(d.cols[k])) };
                }
                if q.y >= sum_y && q.y < sum_y + ROW_H {
                    return match col(q.x) { Some(Some(k)) => Spot::ColTotal(d.cols[k]), _ => Spot::Nothing };
                }
                let i = ((q.y - o.y) / ROW_H) as usize;
                if i == 0 || i > n { return Spot::Nothing; }
                let i = i - 1;
                match col(q.x) {
                    None => Spot::Label(i),
                    Some(None) => Spot::Total(i),
                    Some(Some(k)) => if d.cells[i][k].is_some() { Spot::Cell(i, k) } else { Spot::Nothing },
                }
            };
            let hover = resp.hover_pos().map(spot).unwrap_or(Spot::Nothing);
            if hover != Spot::Nothing { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
            if resp.clicked() {
                clicked = resp.interact_pointer_pos().map(spot).map(|s| match s {
                    // Column k drawn → its index among the columns across.
                    Spot::Cell(i, k) => Spot::Cell(i, d.cols[k]),
                    s => s,
                });
            }
            let rule = |x: f32, y: egui::Rangef| pt.vline(kit::snap(ui, x) - 0.5, y, Stroke::new(1.0, p.brd));
            let hrule = |x: egui::Rangef, y: f32| pt.hline(x, kit::snap(ui, y) - 0.5, Stroke::new(1.0, p.brd));
            let first = ((vp.min.y / ROW_H) as usize).saturating_sub(1);
            let last = ((vp.max.y / ROW_H) as usize + 1).min(n);
            let outline = |r: Rect| pt.rect_stroke(r, 0.0, Stroke::new(1.0, p.prim), StrokeKind::Inside);
            // Body rows: cells, then the total.
            for i in first..last {
                let y = row_y(i);
                for (k, (x, w)) in xs.iter().zip(&cws).enumerate() {
                    let r = Rect::from_min_size(pos2(o.x + x, y), vec2(*w, ROW_H));
                    if let Some((v, t)) = &d.cells[i][k] {
                        if let Some(hr) = heat { pt.rect_filled(r, 0.0, alpha(p.accent, heat_alpha(*v, hr))); }
                        kit::cell_text(ui, &pt, r, t, mono.clone(), p.txt, true);
                    }
                    if hover == Spot::Cell(i, k) { outline(r); }
                    rule(r.max.x, r.y_range());
                }
                let r = Rect::from_min_size(pos2(o.x + tx, y), vec2(tw, ROW_H));
                if !across { if let Some(hr) = heat { pt.rect_filled(r, 0.0, alpha(p.accent, heat_alpha(d.totals[i].0, hr))); } }
                kit::cell_text(ui, &pt, r, &d.totals[i].1, mono_bold(), p.txt, true);
                if hover == Spot::Total(i) { outline(r); }
                rule(r.max.x, r.y_range());
                hrule(egui::Rangef::new(o.x, o.x + width), y + ROW_H);
            }
            // The label column, held at the left.
            for i in first..last {
                let r = Rect::from_min_size(pos2(label_x, row_y(i)), vec2(lw, ROW_H));
                pt.rect_filled(r, 0.0, p.surf_2);
                label_cell(ui, &pt, p, r, &d.labels[i]);
                rule(r.max.x, r.y_range());
                hrule(r.x_range(), r.max.y);
            }
            // The total line, stuck to the bottom: 2 px `brd` on top, `surf_2`, ui 600.
            let sr = Rect::from_min_size(pos2(o.x, sum_y), vec2(width.max(inner.width()), ROW_H));
            pt.rect_filled(sr, 0.0, p.surf_2);
            pt.hline(sr.x_range(), kit::snap(ui, sum_y) + 1.0, Stroke::new(2.0, p.brd));
            for (k, (x, w)) in xs.iter().zip(&cws).enumerate() {
                let r = Rect::from_min_size(pos2(o.x + x, sum_y), vec2(*w, ROW_H));
                kit::cell_text(ui, &pt, r, &d.col_totals[k], mono_bold(), p.txt, true);
                if hover == Spot::ColTotal(d.cols[k]) { outline(r); }
                rule(r.max.x, r.y_range());
            }
            let r = Rect::from_min_size(pos2(o.x + tx, sum_y), vec2(tw, ROW_H));
            kit::cell_text(ui, &pt, r, &d.grand, mono_bold(), p.txt, true);
            rule(r.max.x, r.y_range());
            let r = Rect::from_min_size(pos2(label_x, sum_y), vec2(lw, ROW_H));
            pt.rect_filled(r, 0.0, p.surf_2);
            pt.hline(r.x_range(), kit::snap(ui, sum_y) + 1.0, Stroke::new(2.0, p.brd));
            kit::cell_text(ui, &pt, r, "Total", semibold(12.0), p.txt, false);
            rule(r.max.x, r.y_range());
            // The title row, held at the top.
            let hr = Rect::from_min_size(pos2(o.x, head_y), vec2(width.max(inner.width()), ROW_H));
            pt.rect_filled(hr, 0.0, p.surf_2);
            for (k, (x, w)) in xs.iter().zip(&cws).enumerate() {
                let r = Rect::from_min_size(pos2(o.x + x, head_y), vec2(*w, ROW_H));
                let key = &res.col_keys[d.cols[k]];
                let a = arrow(sort, SortBy::Col(d.cols[k]));
                if key.is_empty() { italic_right(ui, &pt, r, "(empty)", a, p) } else { kit::cell_text(ui, &pt, r, &format!("{key}{a}"), semibold(12.0), p.txt, true) }
                rule(r.max.x, r.y_range());
            }
            let r = Rect::from_min_size(pos2(o.x + tx, head_y), vec2(tw, ROW_H));
            kit::cell_text(ui, &pt, r, &format!("{total_title}{}", arrow(sort, SortBy::Total)), semibold(12.0), p.txt, true);
            rule(r.max.x, r.y_range());
            hrule(hr.x_range(), hr.max.y);
            // The corner: "Rows \ Columns", the second in `txt_faint`.
            let r = Rect::from_min_size(pos2(label_x, head_y), vec2(lw, ROW_H));
            pt.rect_filled(r, 0.0, p.surf_2);
            let mut job = egui::text::LayoutJob::default();
            let f = |c: Color32| egui::TextFormat { font_id: semibold(12.0), color: c, ..Default::default() };
            job.append(&titles.0, 0.0, f(p.txt));
            if across { job.append(&format!(" \\ {}", titles.1), 0.0, f(p.txt_faint)); }
            job.append(arrow(sort, SortBy::Label), 0.0, f(p.txt));
            job.wrap = egui::text::TextWrapping { max_width: lw - 16.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let g = pt.layout_job(job);
            pt.galley(pos2(r.min.x + 8.0, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, p.txt);
            rule(r.max.x, r.y_range());
            hrule(r.x_range(), r.max.y);
        });
        clicked
    }
}

/// ⇄ (`.pt-ctl .btn`, padding 4 × 10): two arrows drawn, the fonts may lack the glyph.
fn swap_button(ui: &mut Ui, p: &Palette) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 26.0), Sense::click());
    let hot = resp.hovered();
    ui.painter().rect(r, 6.0, if hot { p.surf_2 } else { Color32::TRANSPARENT }, Stroke::new(1.0, if hot { p.brd_2 } else { p.brd }), StrokeKind::Inside);
    let (c, s) = (r.center(), Stroke::new(1.4, p.txt));
    let pt = ui.painter();
    let (y1, y2) = (kit::snap(ui, c.y - 3.0), kit::snap(ui, c.y + 3.0));
    pt.line_segment([pos2(c.x - 7.0, y1), pos2(c.x + 7.0, y1)], s);
    pt.add(egui::Shape::line(vec![pos2(c.x + 4.0, y1 - 3.0), pos2(c.x + 7.0, y1), pos2(c.x + 4.0, y1 + 3.0)], s));
    pt.line_segment([pos2(c.x - 7.0, y2), pos2(c.x + 7.0, y2)], s);
    pt.add(egui::Shape::line(vec![pos2(c.x - 4.0, y2 - 3.0), pos2(c.x - 7.0, y2), pos2(c.x - 4.0, y2 + 3.0)], s));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Totals: mono 600 (`.pt-tot`).
fn mono_bold() -> egui::FontId {
    egui::FontId::new(12.0, egui::FontFamily::Name(crate::fonts::MONO_BOLD.into()))
}

/// A row's label (`th`): ui 600, "(empty)" in `txt_faint`.
fn label_cell(ui: &Ui, pt: &egui::Painter, p: &Palette, r: Rect, label: &str) {
    if label.is_empty() { italic(ui, pt, r, "(empty)", p.txt_faint) } else { kit::cell_text(ui, pt, r, label, semibold(12.0), p.txt, false) }
}

/// "(empty)" as a title across, right-aligned, its sort arrow after it.
fn italic_right(ui: &Ui, pt: &egui::Painter, r: Rect, text: &str, arrow: &str, p: &Palette) {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: semibold(12.0), color: p.txt_faint, ..Default::default() });
    job.append(arrow, 0.0, egui::TextFormat { font_id: semibold(12.0), color: p.txt, ..Default::default() });
    let g = pt.layout_job(job);
    pt.galley(pos2(kit::snap(ui, r.max.x - 8.0 - g.size().x), kit::snap(ui, r.center().y - g.size().y / 2.0)), g, p.txt);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_flips_on_the_same_title_only() {
        let s = PivotSort::default();
        let t = next_sort(s, SortBy::Total);
        assert_eq!(t, PivotSort { by: SortBy::Total, flip: false });
        assert_eq!(next_sort(t, SortBy::Total), PivotSort { by: SortBy::Total, flip: true });
        assert_eq!(next_sort(next_sort(t, SortBy::Total), SortBy::Col(2)), PivotSort { by: SortBy::Col(2), flip: false });
        assert_eq!(next_sort(s, SortBy::Label), PivotSort { by: SortBy::Label, flip: true });
    }

    #[test]
    fn arrows_as_the_web_draws_them() {
        let s = PivotSort::default();
        assert_eq!(arrow(s, SortBy::Label), " ▲");
        assert_eq!(arrow(PivotSort { by: SortBy::Label, flip: true }, SortBy::Label), " ▼");
        assert_eq!(arrow(PivotSort { by: SortBy::Total, flip: false }, SortBy::Total), " ▼");
        assert_eq!(arrow(PivotSort { by: SortBy::Total, flip: true }, SortBy::Total), " ▲");
        assert_eq!(arrow(s, SortBy::Total), "");
    }

    #[test]
    fn drill_labels() {
        let h: Vec<String> = ["Region", "Status", "Amount"].map(String::from).to_vec();
        let spec = PivotSpec { rows: 0, rows_by: Dates::Month, cols: Some(1), cols_by: Dates::Month, fun: Fun::Sum, value: Some(2) };
        assert_eq!(drill_label(&h, &spec, Some("Nord"), Some("")), "Region = Nord · Status = (empty)");
        assert_eq!(drill_label(&h, &spec, None, Some("Paid")), "Status = Paid");
        assert_eq!(drill_label(&h, &spec, None, None), "pivot rows");
    }

    #[test]
    fn heat_runs_from_the_smallest_to_the_largest() {
        assert!((heat_alpha(1.0, (1.0, 3.0)) - 0.06).abs() < 1e-6);
        assert!((heat_alpha(3.0, (1.0, 3.0)) - 0.66).abs() < 1e-6);
        assert!((heat_alpha(5.0, (5.0, 5.0)) - 0.36).abs() < 1e-6);
    }
}
