//! Fill by example: type the result wanted in empty cells of a column — twice
//! — and a rule taking it from the row's other cells is looked for
//! (`csvfab_engine::fill_example`). Its values show in the column's empty
//! cells as grey ghosts (`Tab::ghost`, drawn by the grid, computed here for
//! the rows on screen), a bar spells the rule out; Tab writes them in the rows
//! shown (one undo step), Escape forgets them. Ctrl+E asks for a rule on the
//! selected column with the examples typed so far, or, with none, with the
//! column's filled cells — which fills a partly empty column the way the rest was.
//!
//! The app calls `typed` with a cell edit before applying it (the examples)
//! and `keys` before the grid takes its keys (Tab and Escape are the offer's).

use super::fx_picker::{mono, ui_font};
use super::{Cmd, Command, Cx, Tool};
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::{Ghost, Tab};
use crate::theme::{alpha, Palette};
use csvfab_engine::fill_example::{self as ff, Chip, Rule};
use csvfab_engine::{RowId, State};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Id, Key, Modifiers, Rect, Sense, Shape, Stroke};
use std::collections::HashMap;
use std::sync::Arc;

/// The cells typed in a column that was empty there: the examples (the web's t.ffEx).
#[derive(Clone, Default)]
struct Typed {
    col: usize,
    name: String,
    rows: Vec<RowId>,
}

#[derive(Clone, Copy)]
enum FfKey {
    Accept,
    Dismiss,
}

fn typed_id(uid: u64) -> Id {
    Id::unique(("ff-typed", uid))
}
fn epoch_id(uid: u64) -> Id {
    Id::unique(("ff-epoch", uid))
}
fn key_id(uid: u64) -> Id {
    Id::unique(("ff-key", uid))
}
fn active_id(uid: u64) -> Id {
    Id::unique(("ff-active", uid))
}

/// To call with a cell edit *before* it is applied (`Action::SetCells`): a value
/// typed into an empty cell is an example; from the second in a column, a rule
/// is looked for — the returned command opens the offer (push it to `pending`).
pub fn typed(tab: &Tab, changes: &[(RowId, usize, String)], ctx: &egui::Context) -> Option<Cmd> {
    let [(id, col, v)] = changes else { return None };
    let st = tab.st()?;
    // A text file has no columns to fill (as the web's: no offer there).
    if st.base.dialect.lines { return None; }
    if v.trim().is_empty() || !st.cell(*id, *col).trim().is_empty() {
        return None;
    }
    let name = st.headers().get(*col).cloned().unwrap_or_default();
    let n = ctx.data_mut(|d| {
        let t: &mut Typed = d.get_temp_mut_or_default(typed_id(tab.uid));
        if t.col != *col || t.name != name || t.rows.is_empty() {
            *t = Typed { col: *col, name, rows: Vec::new() };
        }
        if !t.rows.contains(id) {
            t.rows.push(*id);
        }
        if t.rows.len() > ff::MAX_EX {
            t.rows.remove(0);
        }
        t.rows.len()
    });
    (n >= 2).then(|| Cmd::Tool(Box::new(Offer::new(tab, *col, true, ctx))))
}

/// To call each frame for the active tab, before the grid: while values are
/// offered, Tab takes them and Escape drops them (not while a cell or a field is edited).
pub fn keys(tab: &Tab, ctx: &egui::Context) {
    // Read before: the context is locked inside `data_mut`.
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(active_id(tab.uid), pass));
    if tab.ghost.is_none() || tab.grid.editing.is_some() || ctx.egui_wants_keyboard_input() {
        return;
    }
    let k = ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Tab) {
            Some(FfKey::Accept)
        } else if i.consume_key(Modifiers::NONE, Key::Escape) {
            Some(FfKey::Dismiss)
        } else {
            None
        }
    });
    if let Some(k) = k {
        ctx.data_mut(|d| d.insert_temp(key_id(tab.uid), k));
    }
}

enum Found {
    /// Offered on its own after a typed example, with fewer than two left (an undo emptied them).
    TooFew,
    NoExamples,
    NoRule(usize),
    Rule(Rule),
}

impl crate::job::Failed for Found {
    fn failed(_: &str) -> Self { Found::NoExamples }
}

struct Offer {
    uid: u64,
    epoch: u64,
    col: usize,
    name: String,
    auto: bool,
    infer: Option<Job<Found>>,
    rule: Option<Arc<Rule>>,
    chips: Vec<Chip>,
    version: u64,
    count: Option<usize>,
    count_job: Option<Job<Option<usize>>>,
    /// (first row on screen, rows on screen, generation) the ghosts are for.
    window: Option<(usize, usize, u64)>,
    ghost: Option<Arc<Ghost>>,
    filling: Option<Job<Option<Vec<(RowId, usize, String)>>>>,
}

/// The rows shown, as ids (the whole file's when nothing filters it).
fn rows_shown(st: &State, view: Option<&Arc<Vec<RowId>>>) -> Vec<RowId> {
    match view {
        Some(v) => v.to_vec(),
        None => (0..st.len()).map(|p| st.data_id(p)).collect(),
    }
}

impl Offer {
    /// A new offer replaces the one on the tab, if any.
    fn new(tab: &Tab, col: usize, auto: bool, ctx: &egui::Context) -> Offer {
        let epoch = ctx.data_mut(|d| {
            let e: &mut u64 = d.get_temp_mut_or_default(epoch_id(tab.uid));
            *e += 1;
            *e
        });
        let name = tab.st().and_then(|s| s.headers().get(col).cloned()).unwrap_or_default();
        Offer { uid: tab.uid, epoch, col, name, auto, infer: None, rule: None, chips: Vec::new(), version: 0, count: None, count_job: None, window: None, ghost: None, filling: None }
    }

    fn start(&mut self, tab: &Tab, ctx: &egui::Context) {
        let Some(t) = &tab.table else { return };
        let st = t.st.clone();
        let view = tab.view.clone();
        let (col, auto) = (self.col, self.auto);
        let typed: Vec<RowId> = ctx
            .data(|d| d.get_temp::<Typed>(typed_id(self.uid)))
            .filter(|t| t.col == col && t.name == self.name)
            .map(|t| t.rows)
            .unwrap_or_default();
        self.infer = Some(Job::spawn(ctx, move |cancel, _| {
            let mut ex: Vec<(RowId, String)> = typed.iter().map(|&id| (id, st.cell(id, col).trim().to_string())).filter(|x| !x.1.is_empty()).collect();
            if auto && ex.len() < 2 {
                return Found::TooFew;
            }
            if ex.is_empty() {
                // The column's filled cells shown, the first 40.
                let n = view.as_ref().map_or(st.len(), |v| v.len());
                let mut tmp = Vec::new();
                for p in 0..n {
                    if p % 4096 == 0 && cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let id = view.as_ref().map_or_else(|| st.data_id(p), |v| v[p]);
                    let v = st.with_cell(id, col, &mut tmp, |s| s.trim().to_string());
                    if !v.is_empty() {
                        ex.push((id, v));
                        if ex.len() >= ff::MAX_EX {
                            break;
                        }
                    }
                }
            }
            if ex.is_empty() {
                return Found::NoExamples;
            }
            match ff::infer(&ex, col, &st) {
                Some(r) => Found::Rule(r),
                None => Found::NoRule(ex.len()),
            }
        }));
    }

    /// Takes the ghosts off the tab (if they are still ours).
    fn clear(&mut self, tab: &mut Tab) {
        if let (Some(g), Some(mine)) = (&tab.ghost, &self.ghost) {
            if Arc::ptr_eq(g, mine) {
                tab.ghost = None;
            }
        }
        self.ghost = None;
    }

    /// Forgotten: two new examples bring a new offer.
    fn dismiss(&mut self, cx: &mut Cx) {
        cx.ctx.data_mut(|d| d.remove::<Typed>(typed_id(self.uid)));
        self.clear(cx.tab);
    }

    fn accept(&mut self, cx: &mut Cx) {
        let (Some(rule), Some(t)) = (self.rule.clone(), &cx.tab.table) else { return };
        if self.filling.is_some() {
            return;
        }
        let st = t.st.clone();
        let view = cx.tab.view.clone();
        self.filling = Some(Job::spawn(cx.ctx, move |cancel, progress| {
            let rows = rows_shown(&st, view.as_ref());
            ff::apply(&st, &rule, &rows, cancel, Some(progress))
        }));
    }

    /// The ghosts of the rows on screen, recomputed when the view scrolls or changes.
    fn ghosts(&mut self, tab: &mut Tab) {
        let (Some(rule), Some(t)) = (&self.rule, &tab.table) else { return };
        let n = tab.shown();
        let top = tab.grid.top.min(n);
        let page = tab.grid.page.max(1) + 2;
        let key = (top, page, tab.generation);
        if self.window == Some(key) && tab.ghost.as_ref().zip(self.ghost.as_ref()).is_some_and(|(a, b)| Arc::ptr_eq(a, b)) {
            return;
        }
        let st = &t.st;
        let mut tmp = Vec::new();
        let mut values = HashMap::new();
        for pos in top..(top + page).min(n) {
            let id = tab.shown_id(pos);
            if let Some(v) = rule.fill(st, id, &mut tmp) {
                values.insert(id, v);
            }
        }
        let g = Arc::new(Ghost { col: self.col, values, rule: rule.describe(&st.headers()) });
        tab.ghost = Some(g.clone());
        self.ghost = Some(g);
        self.window = Some(key);
    }
}

impl Tool for Offer {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let ctx = cx.ctx.clone();
        let current = ctx.data(|d| d.get_temp::<u64>(epoch_id(self.uid))).unwrap_or(0);
        if current != self.epoch || cx.tab.table.is_none() {
            self.clear(cx.tab);
            return false;
        }
        if self.infer.is_none() && self.rule.is_none() {
            self.start(cx.tab, &ctx);
        }
        if let Some(found) = self.infer.as_mut().and_then(|j| j.poll()) {
            self.infer = None;
            match found {
                Found::TooFew => return false,
                Found::NoExamples => {
                    cx.toast(format!("Type the result you want in one or two cells of \"{}\", then Ctrl+E: the rest of the column follows.", self.name));
                    return false;
                }
                Found::NoRule(n) => {
                    if !self.auto {
                        cx.toast(format!("No rule found that gives {} from the other columns of the row. Type one more example and try again.", if n == 1 { "this example" } else { "these examples" }));
                    }
                    return false;
                }
                Found::Rule(r) => {
                    let st = cx.tab.st().unwrap().clone();
                    self.chips = r.chips(&st.headers());
                    let rule = Arc::new(r);
                    self.rule = Some(rule.clone());
                    self.version = cx.tab.table.as_ref().unwrap().version();
                    let view = cx.tab.view.clone();
                    self.count_job = Some(Job::spawn(&ctx, move |cancel, _| ff::count(&st, &rule, &rows_shown(&st, view.as_ref()), cancel)));
                }
            }
        }
        if self.rule.is_none() {
            return true;
        }
        if let Some(r) = self.filling.as_mut().and_then(|j| j.poll()) {
            self.filling = None;
            self.dismiss(cx);
            if let Some(ch) = r {
                if ch.is_empty() {
                    cx.toast("Nothing changed.");
                } else {
                    cx.tab.apply_changes(&ch, &ctx);
                    cx.toast(format!("{} cells filled by example", crate::grid::fmt_int(ch.len())));
                }
            }
            return false;
        }
        // Still the column and the edit the rule was found on? Anything else drops it.
        let t = cx.tab.table.as_ref().unwrap();
        if t.version() != self.version || t.st.headers().get(self.col) != Some(&self.name) {
            self.clear(cx.tab);
            return false;
        }
        let key = ctx.data_mut(|d| {
            let k = d.get_temp::<FfKey>(key_id(self.uid));
            d.remove::<FfKey>(key_id(self.uid));
            k
        });
        match key {
            Some(FfKey::Accept) => self.accept(cx),
            Some(FfKey::Dismiss) => {
                self.dismiss(cx);
                return false;
            }
            None => {}
        }
        if let Some(n) = self.count_job.as_mut().and_then(|j| j.poll()) {
            self.count_job = None;
            if let Some(n) = n {
                self.count = Some(n);
                if n == 0 {
                    if !self.auto {
                        cx.toast(format!("The rule found gives nothing for the empty cells of \"{}\" shown.", self.name));
                    }
                    self.clear(cx.tab);
                    return false;
                }
            }
        }
        self.ghosts(cx.tab);
        // The bar, only over its own tab.
        let active = ctx.data(|d| d.get_temp::<u64>(active_id(self.uid))) == Some(ctx.cumulative_pass_nr());
        if active {
            match self.bar(cx) {
                Some(FfKey::Accept) => self.accept(cx),
                Some(FfKey::Dismiss) => {
                    self.dismiss(cx);
                    return false;
                }
                None => {}
            }
        }
        true
    }
}

impl Offer {
    /// The bar at the bottom: ✦ Fill <name> by example · the rule · n empty cells · [Fill Tab] ×.
    fn bar(&self, cx: &mut Cx) -> Option<FfKey> {
        let p = cx.p;
        let ctx = cx.ctx;
        let screen = ctx.content_rect();
        let mut out = None;
        let frame = egui::Frame::new()
            .fill(p.surf)
            .stroke(Stroke::new(1.0, p.brd))
            .corner_radius(10)
            .inner_margin(egui::Margin { left: 12, right: 6, top: 6, bottom: 6 })
            .shadow(egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(115) });
        let r = egui::Area::new(Id::unique(("ff-bar", self.uid)))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -46.0))
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                    ui.horizontal(|ui| {
                        let max_w = screen.width() - 32.0 - 18.0;
                        let pt = ui.painter().clone();
                        // Sparkles.
                        let (ir, _) = ui.allocate_exact_size(vec2(18.0, 22.0), Sense::hover());
                        icons::paint(&pt, Icon::Sparkles, Rect::from_center_size(ir.center(), vec2(18.0, 18.0)), p.violet_txt);
                        // Fill <name> by example.
                        let mut job = egui::text::LayoutJob::default();
                        job.append("Fill ", 0.0, egui::TextFormat::simple(ui_font(13.0), p.txt_hi));
                        job.append(&self.name, 0.0, egui::TextFormat::simple(crate::grid::semibold(13.0), p.txt_hi));
                        job.append(" by example", 0.0, egui::TextFormat::simple(ui_font(13.0), p.txt_hi));
                        let title = pt.layout_job(job);
                        let count = match (self.filling.as_ref(), self.count) {
                            (Some(_), _) => "filling…".to_string(),
                            (None, None) => "counting…".to_string(),
                            (None, Some(n)) => format!("{} empty cell{}", crate::grid::fmt_int(n), if n == 1 { "" } else { "s" }),
                        };
                        let count = pt.layout_no_wrap(count, ui_font(12.0), p.txt_mut);
                        let fixed = 18.0 + title.size().x + count.size().x + 62.0 + 22.0 + 5.0 * 10.0;
                        let (tr, _) = ui.allocate_exact_size(vec2(title.size().x, 22.0), Sense::hover());
                        let tw = title.size().y;
                        pt.galley(pos2(tr.min.x, (tr.center().y - tw / 2.0).round()), title, p.txt_hi);
                        // The rule, chip by chip, cut at the room left.
                        let chips: Vec<_> = self.chips.iter().map(|c| chip_layout(&pt, p, c)).collect();
                        let natural: f32 = chips.iter().map(|c| c.0).sum::<f32>() + 3.0 * chips.len().saturating_sub(1) as f32;
                        let rw = natural.min((max_w - fixed).max(40.0));
                        let (rr, _) = ui.allocate_exact_size(vec2(rw, 22.0), Sense::hover());
                        let cp = pt.with_clip_rect(rr);
                        let mut x = rr.min.x;
                        for (w, draw) in chips {
                            draw(&cp, pos2(x, rr.center().y));
                            x += w + 3.0;
                        }
                        let (cr, _) = ui.allocate_exact_size(vec2(count.size().x, 22.0), Sense::hover());
                        let ch = count.size().y;
                        pt.galley(pos2(cr.min.x, (cr.center().y - ch / 2.0).round()), count, p.txt_mut);
                        // Fill [Tab].
                        let fg = pt.layout_no_wrap("Fill".into(), crate::grid::semibold(12.5), p.on_prim);
                        let kg = pt.layout_no_wrap("Tab".into(), mono(10.5), p.on_prim);
                        let bw = 10.0 + fg.size().x + 6.0 + kg.size().x + 8.0 + 6.0;
                        let (br, b) = ui.allocate_exact_size(vec2(bw, 22.0), Sense::click());
                        pt.rect_filled(br, 6.0, if b.hovered() { p.prim_hover } else { p.prim });
                        let fw = fg.size().x;
                        pt.galley(pos2(br.min.x + 10.0, (br.center().y - fg.size().y / 2.0).round()), fg, p.on_prim);
                        let kr = Rect::from_min_size(pos2(br.min.x + 10.0 + fw + 6.0, br.center().y - 8.0), vec2(kg.size().x + 8.0, 16.0));
                        pt.rect_filled(kr, 4.0, alpha(p.hi, 0.18));
                        pt.galley(pos2(kr.min.x + 4.0, (kr.center().y - kg.size().y / 2.0).round()), kg, p.on_prim);
                        if b.on_hover_text("Write these values (Tab)").clicked() {
                            out = Some(FfKey::Accept);
                        }
                        // ×.
                        let (xr, x) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                        if x.hovered() {
                            pt.rect_filled(xr, 5.0, p.surf_2);
                        }
                        pt.text(xr.center(), Align2::CENTER_CENTER, "×", ui_font(15.0), if x.hovered() { p.txt_hi } else { p.txt_dim });
                        if x.on_hover_text("Dismiss (Esc)").clicked() {
                            out = Some(FfKey::Dismiss);
                        }
                    });
                });
            });
        // The 3 px left border in violet.
        let br = r.response.rect;
        ctx.layer_painter(r.response.layer_id).rect_filled(Rect::from_min_max(br.min, pos2(br.min.x + 3.0, br.max.y)), egui::CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 }, p.violet_txt);
        out
    }
}

type ChipDraw<'a> = Box<dyn Fn(&egui::Painter, egui::Pos2) + 'a>;

/// A chip of the rule: a column (`.ff-a`: its name, what is taken of it) or
/// constant text (`.ff-k`: mono, dashed, spaces shown as ␣). Width, and how to draw it at (left, centre y).
fn chip_layout<'a>(pt: &egui::Painter, p: &'a Palette, c: &Chip) -> (f32, ChipDraw<'a>) {
    match c {
        Chip::Column { name, detail } => {
            let n = pt.layout_no_wrap(name.clone(), crate::grid::semibold(13.0), p.txt_hi);
            let d = (!detail.is_empty()).then(|| pt.layout_no_wrap(detail.clone(), ui_font(11.5), p.txt_mut));
            let w = 7.0 + n.size().x + d.as_ref().map_or(0.0, |d| 5.0 + d.size().x) + 7.0;
            (w, Box::new(move |pt: &egui::Painter, at: egui::Pos2| {
                let r = Rect::from_min_size(pos2(at.x, at.y - 11.0), vec2(w, 22.0));
                pt.rect_filled(r, 6.0, alpha(p.violet, 0.14));
                let ny = (at.y - n.size().y / 2.0).round();
                pt.galley(pos2(r.min.x + 7.0, ny), n.clone(), p.txt_hi);
                if let Some(d) = &d {
                    // On the name's baseline, as `align-items: baseline`.
                    pt.galley(pos2(r.min.x + 7.0 + n.size().x + 5.0, ny + ((n.size().y - d.size().y) * 0.8).round()), d.clone(), p.txt_mut);
                }
            }))
        }
        Chip::Text(t) => {
            let mut job = egui::text::LayoutJob::default();
            for (i, part) in t.split(' ').enumerate() {
                if i > 0 {
                    job.append("␣", 0.0, egui::TextFormat::simple(mono(12.5), p.txt_dim));
                }
                job.append(part, 0.0, egui::TextFormat::simple(mono(12.5), p.txt));
            }
            let g = pt.layout_job(job);
            let w = 7.0 + g.size().x + 7.0;
            (w, Box::new(move |pt: &egui::Painter, at: egui::Pos2| {
                let r = Rect::from_min_size(pos2(at.x, at.y - 11.0), vec2(w, 22.0)).shrink(0.5);
                let s = Stroke::new(1.0, alpha(p.hi, 0.22));
                for e in [[r.left_top(), r.right_top()], [r.right_top(), r.right_bottom()], [r.right_bottom(), r.left_bottom()], [r.left_bottom(), r.left_top()]] {
                    pt.extend(Shape::dashed_line(&e, s, 3.0, 3.0));
                }
                pt.galley(pos2(r.min.x + 7.0, (at.y - g.size().y / 2.0).round()), g.clone(), p.txt);
            }))
        }
    }
}

/// Ctrl+E: a rule for the selected cell's column.
fn run(cx: &mut Cx) {
    // Not while a cell or a text field is being edited.
    if cx.tab.grid.editing.is_some() || cx.ctx.egui_wants_keyboard_input() {
        return;
    }
    if cx.tab.st().is_some_and(|st| st.base.dialect.lines) {
        cx.toast("Fill by example works on the columns of a CSV file.");
        return;
    }
    let Some(c) = cx.tab.grid.cursor else {
        cx.toast("Select a cell of the column to fill first.");
        return;
    };
    let o = Offer::new(cx.tab, c.col, false, cx.ctx);
    cx.out.push(Cmd::Tool(Box::new(o)));
}

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Fill by example", group: "Rows", keys: Some("Ctrl+E"), enabled: super::has_table, run }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use csvfab_engine::base::{Base, OpenOptions};
    use csvfab_engine::Table;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    /// Timings on a big file: `CSVFAB_BENCH=path cargo test --release -p csvfab bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench() {
        let Ok(path) = std::env::var("CSVFAB_BENCH") else { return };
        let mut t = Table::new(Arc::new(Base::open(std::path::Path::new(&path), OpenOptions::default(), None).unwrap()), None);
        let n = t.st.ncols;
        t.insert_column(n, "login");
        let st = t.st.clone();
        let ex = vec![(st.data_id(0), format!("{}.{}", st.cell(st.data_id(0), 1), st.cell(st.data_id(0), 2)).to_lowercase()), (st.data_id(1), format!("{}.{}", st.cell(st.data_id(1), 1), st.cell(st.data_id(1), 2)).to_lowercase())];
        let t0 = Instant::now();
        let rule = ff::infer(&ex, n, &st).unwrap();
        println!("infer: {:?} ({})", t0.elapsed(), rule.describe(&st.headers()));
        let ids = rows_shown(&st, None);
        let no = AtomicBool::new(false);
        let t0 = Instant::now();
        let k = ff::count(&st, &rule, &ids, &no).unwrap();
        println!("count {k}: {:?}", t0.elapsed());
        let t0 = Instant::now();
        let mut tmp = Vec::new();
        let g: usize = (0..40).filter(|&p| rule.fill(&st, ids[p], &mut tmp).is_some()).count();
        println!("40 ghosts ({g}): {:?}", t0.elapsed());
        let t0 = Instant::now();
        let ch = ff::apply(&st, &rule, &ids, &no, None).unwrap();
        println!("apply {}: {:?}", ch.len(), t0.elapsed());
        let t0 = Instant::now();
        t.set_cells(&ch);
        println!("set_cells (UI thread): {:?}", t0.elapsed());
    }
}
