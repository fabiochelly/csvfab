//! File map (46-file-map.js, Ctrl+M): the rows shown as one picture. Each
//! column a vertical band, each pixel row a run of rows; a run painted in its
//! column's type colour (numbers `cyan_txt`, dates `violet_txt`, text
//! `txt_mut`), as opaque as it is filled — empty stays dark — and `danger`
//! where values are not of the column's type. A gutter on the left shows the
//! irregular rows (`warn`) and the edited ones (`orange_txt`).
//!
//! Counted in the background in passes: one row of every run first (the
//! whole map at once, coarse), then the next one of each, until every row is
//! counted (EXACT rows at most: past that, as many rows of each run as fit).
//! The picture is a texture rebuilt as the counts come. Hovering says what a
//! run holds; a click goes there — to the run's first value not of its
//! column's type, if any.

use super::analysis_kit as kit;
use super::{Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::theme::Palette;
use csvfab_engine::kinds::{self, Kind};
use csvfab_engine::{RowId, State};
use eframe::egui::{self, pos2, vec2, Align2, Color32, ColorImage, Rect, Sense, Stroke, TextureHandle, TextureOptions};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Rows counted one by one; past that, sampled evenly.
const EXACT: usize = 3_000_000;
/// Points a column band takes at most.
const COL_MAX: f32 = 120.0;
/// The gutter's width (two lanes of 5 and a gap), then 4 before the bands.
const GUT: f32 = 12.0;
/// The row labels' room on the left.
const LABELS: f32 = 46.0;

/// One run of rows: what its rows read so far hold.
#[derive(Clone, Default)]
struct Bin {
    seen: u32,
    irr: u32,
    edit: u32,
    fill: Vec<u32>,
    bad: Vec<u32>,
    /// The first position (in the view) of a value not of its column's type.
    first_bad: Vec<Option<usize>>,
}

/// The counts, as published by the job.
#[derive(Clone, Default)]
struct Counts {
    bins: Vec<Bin>,
    round: usize,
    rounds: usize,
    version: u64,
}

/// Geometry, from the body's size.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Geom {
    w: f32,
    h: f32,
    cw: f32,
    nc: usize,
    all: usize,
    n: usize,
    nb: usize,
}

impl Geom {
    fn new(body: egui::Vec2, ncols: usize, n: usize) -> Geom {
        let w = body.x - LABELS;
        let cw = ((w - GUT - 4.0) / ncols.max(1) as f32).floor().clamp(2.0, COL_MAX);
        // A file of thousands of columns: the first ones.
        let nc = (((w - GUT - 4.0) / cw).floor() as usize).min(ncols);
        let nb = n.min(body.y.floor() as usize).max(1);
        Geom { w: GUT + 4.0 + nc as f32 * cw, h: body.y.floor(), cw, nc, all: ncols, n, nb }
    }

    /// Run b is the view's positions lo(b)..lo(b + 1).
    fn lo(&self, b: usize) -> usize {
        b * self.n / self.nb
    }
}

/// Counts every run's rows, pass after pass, publishing the counts as they grow.
fn count(st: State, view: Option<Arc<Vec<RowId>>>, g: Geom, kinds: Vec<Kind>, out: Arc<Mutex<Counts>>, cancel: &AtomicBool, progress: &AtomicUsize) {
    let (nb, nc) = (g.nb, g.nc);
    let sampled = g.n > EXACT;
    let most = (0..nb).map(|b| g.lo(b + 1) - g.lo(b)).max().unwrap_or(0);
    let rounds = if sampled { (EXACT / nb).max(1) } else { most };
    let mut bins = vec![Bin { fill: vec![0; nc], bad: vec![0; nc], first_bad: vec![None; nc], ..Default::default() }; nb];
    let width = kinds::expected_width(&st);
    let id_at = |i: usize| view.as_ref().map_or_else(|| st.data_id(i), |v| v[i]);
    let (mut round, mut version) = (0, 0);
    let mut published = Instant::now() - Duration::from_secs(1);
    while round < rounds {
        if cancel.load(Ordering::Relaxed) { return; }
        // About 64k rows a batch, every run in parallel.
        let k = (65_536 / nb).max(1).min(rounds - round);
        bins.par_iter_mut().enumerate().for_each(|(b, bin)| {
            let (lo, size) = (g.lo(b), g.lo(b + 1) - g.lo(b));
            let (mut tmp, mut row) = (Vec::new(), Vec::new());
            for r in round..round + k {
                let off = if sampled { ((r as f64 + 0.5) * size as f64 / rounds as f64) as usize } else { r };
                if off >= size { continue; }
                let i = lo + off;
                let id = id_at(i);
                bin.seen += 1;
                let mut fields = 0;
                kinds::visit_row(&st, id, &mut tmp, &mut row, |c, v| {
                    fields = c + 1;
                    if c >= nc { return; }
                    let ty = kinds::cell_type(v);
                    if ty == Kind::Empty { return; }
                    bin.fill[c] += 1;
                    let kind = kinds[c];
                    if matches!(kind, Kind::Num | Kind::Date) && ty != kind {
                        bin.bad[c] += 1;
                        if bin.first_bad[c].is_none_or(|f| i < f) { bin.first_bad[c] = Some(i); }
                    }
                });
                let edited = id as usize >= st.base.n || st.row_edited(id);
                if fields != width { bin.irr += 1; }
                if edited { bin.edit += 1; }
            }
        });
        round += k;
        progress.store(round * 1000 / rounds.max(1), Ordering::Relaxed);
        if round >= rounds || published.elapsed() > Duration::from_millis(120) {
            version += 1;
            *out.lock().unwrap() = Counts { bins: bins.clone(), round, rounds, version };
            published = Instant::now();
        }
    }
}

struct Map {
    geom: Option<Geom>,
    kinds: Vec<Kind>,
    counts: Arc<Mutex<Counts>>,
    job: Option<Job<()>>,
    tex: Option<(TextureHandle, u64)>,
    view: Option<Arc<Vec<RowId>>>,
    generation: u64,
    resized: Option<Instant>,
}

impl Map {
    fn new(cx: &Cx) -> Map {
        let st = cx.tab.st().unwrap();
        Map { geom: None, kinds: kinds::column_kinds(st), counts: Default::default(), job: None, tex: None, view: cx.tab.view.clone(), generation: cx.tab.generation, resized: None }
    }

    fn start(&mut self, cx: &Cx, g: Geom) {
        let st = cx.tab.st().unwrap().clone();
        self.geom = Some(g);
        self.counts = Default::default();
        let (view, out) = (self.view.clone(), self.counts.clone());
        let kinds: Vec<Kind> = (0..g.nc).map(|c| self.kinds.get(c).copied().unwrap_or(Kind::Text)).collect();
        self.job = Some(Job::spawn(cx.ctx, move |cancel, progress| count(st, view, g, kinds, out, cancel, progress)));
    }

    /// The picture, at the screen's pixel density.
    fn image(&self, c: &Counts, g: Geom, p: &Palette, ppp: f32, max_side: usize) -> ColorImage {
        // Never past the largest texture the app allows (the glyph atlas cap, 4096, or less on
        // an old GPU): a maximised 5K window draws the map at a little less than its native density.
        let side = max_side.min(4096) as f32;
        let ppp = ppp.min(side / g.w.max(1.0)).min(side / g.h.max(1.0));
        let (w, h) = ((g.w * ppp).round() as usize, (g.h * ppp).round().max(1.0) as usize);
        let mut img = ColorImage::filled([w, h], Color32::TRANSPARENT);
        let mut rect = |x0: f32, y0: f32, x1: f32, y1: f32, col: Color32, a: f32| {
            let (x0, x1) = (((x0 * ppp).round() as usize).min(w), ((x1 * ppp).round() as usize).min(w));
            let y0 = (y0 * ppp).round() as usize;
            let y1 = ((y1 * ppp).round() as usize).max(y0 + 1).min(h);
            let c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (a.clamp(0.0, 1.0) * 255.0).round() as u8);
            for y in y0..y1 { img.pixels[y * w + x0..y * w + x1.max(x0)].fill(c); }
        };
        let gap = if g.cw >= 6.0 { 1.0 } else { 0.0 };
        let bh = g.h / g.nb as f32;
        // The gutter's two lanes, so a lone mark reads as one.
        rect(0.0, 0.0, 5.0, g.h, p.surf_3, 0.35);
        rect(6.0, 0.0, 11.0, g.h, p.surf_3, 0.35);
        for (b, bin) in c.bins.iter().enumerate() {
            let s = bin.seen as f32;
            if s == 0.0 { continue; }
            let (y0, y1) = (b as f32 * bh, (b + 1) as f32 * bh);
            if bin.irr > 0 { rect(0.0, y0, 5.0, y1.max(y0 + 2.0), p.warn, 0.6 + 0.4 * (bin.irr as f32 / s).sqrt()); }
            if bin.edit > 0 { rect(6.0, y0, 11.0, y1.max(y0 + 2.0), p.orange_txt, 0.6 + 0.4 * (bin.edit as f32 / s).sqrt()); }
            for k in 0..g.nc {
                let x0 = GUT + 4.0 + k as f32 * g.cw;
                let x1 = x0 + g.cw - gap;
                let (fill, bad) = (bin.fill[k] as f32, bin.bad[k] as f32);
                if bad > 0.0 {
                    rect(x0, y0, x1, y1, p.danger, 0.6 + 0.4 * (bad / s).sqrt());
                } else if fill > 0.0 {
                    // Text quieter: the typed columns and the misfits stand out.
                    match self.kinds.get(k) {
                        Some(Kind::Num) => rect(x0, y0, x1, y1, p.cyan_txt, 0.3 + 0.7 * fill / s),
                        Some(Kind::Date) => rect(x0, y0, x1, y1, p.violet_txt, 0.3 + 0.7 * fill / s),
                        _ => rect(x0, y0, x1, y1, p.txt_mut, 0.15 + 0.35 * fill / s),
                    }
                } else {
                    rect(x0, y0, x1, y1, p.surf_3, 0.55);
                }
            }
        }
        img
    }
}

/// The run and the column (None: the gutter) at a point of the picture.
fn at(g: Geom, q: egui::Vec2) -> Option<(usize, Option<usize>)> {
    if q.x < 0.0 || q.y < 0.0 || q.y >= g.h || q.x >= g.w { return None; }
    let b = ((q.y / (g.h / g.nb as f32)) as usize).min(g.nb - 1);
    let k = if q.x < GUT + 4.0 { None } else { Some((((q.x - GUT - 4.0) / g.cw) as usize).min(g.nc.saturating_sub(1))) };
    Some((b, k))
}

/// The step between row labels: a power of ten (or half one) leaving 44 points between them.
fn tick_step(h: f32, n: usize) -> usize {
    let px = |k: f64| k * h as f64 / n.max(1) as f64;
    let mut step = 10f64.powf((44.0 / px(1.0)).log10().ceil().max(0.0));
    if step >= 10.0 && (n as f64) < 5.0 * step && px(step / 2.0) >= 44.0 { step /= 2.0; }
    step.max(1.0) as usize
}

impl Tool for Map {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(st) = cx.tab.st() else { return false };
        let n = cx.tab.shown();
        if n == 0 { return false; }
        // The rows shown changed (a filter, an edit): the map follows them.
        if cx.tab.generation != self.generation && !cx.tab.busy() {
            self.generation = cx.tab.generation;
            self.view = cx.tab.view.clone();
            self.kinds = kinds::column_kinds(st);
            self.geom = None;
        }
        if let Some(j) = &mut self.job { if j.poll().is_some() { self.job = None; } }
        let p = cx.p;
        let size = cx.ctx.content_rect().size() - vec2(60.0, 60.0);
        let ppp = cx.ctx.pixels_per_point();
        let mut close = cx.ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::M)));
        let mut go = None;
        let headers = st.headers();
        let ncols = st.ncols;
        let (_, open) = kit::modal_fixed(cx, "map", size, (20, 16, 12), |ui, cx| {
            let counts = self.counts.lock().unwrap().clone();
            let pt = ui.painter().clone();
            // Head: title, summary, legend (`.mp-head`, baseline-aligned).
            let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
            let base = head.min.y + 17.0;
            let tg = pt.layout_no_wrap("File map".into(), crate::grid::semibold(16.0), p.txt_hi);
            let tw = tg.size().x;
            pt.galley(pos2(head.min.x, kit::snap(ui, base - tg.size().y + 3.0)), tg, p.txt_hi);
            if let Some(g) = self.geom {
                let cols = if g.nc < g.all { format!("the first {} of {} columns", kit::n(g.nc), kit::n(g.all)) } else { format!("{} columns", kit::n(g.all)) };
                let state = if counts.rounds == 0 || counts.round < counts.rounds { format!(" · reading… {} %", 100 * counts.round / counts.rounds.max(1)) }
                    else if g.n > EXACT { format!(" · {} rows sampled", kit::n(counts.rounds * g.nb)) } else { String::new() };
                pt.text(pos2(head.min.x + tw + 14.0, base), Align2::LEFT_BOTTOM, format!("{} rows × {cols}{state}", kit::n(g.n)), kit::ui(12.0), p.txt_faint);
            }
            let legend: [(Color32, &str); 7] = [(p.cyan_txt, "Numbers"), (p.violet_txt, "Dates"), (p.txt_mut, "Text"), (p.surf_3, "Empty"), (p.danger, "Not the column's type"), (p.warn, "Irregular row"), (p.orange_txt, "Edited")];
            let mut x = head.max.x;
            for (c, label) in legend.iter().rev() {
                let g = pt.layout_no_wrap(label.to_string(), kit::ui(12.0), p.txt_mut);
                x -= g.size().x;
                pt.galley(pos2(x, kit::snap(ui, base - g.size().y + 2.0)), g, p.txt_mut);
                x -= 4.0 + 10.0;
                pt.rect_filled(Rect::from_min_size(pos2(x, base - 11.0), vec2(10.0, 10.0)), 2.0, *c);
                x -= 8.0 + 6.0;
            }
            ui.add_space(8.0);
            let (cols_r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            let (body, _) = ui.allocate_exact_size(vec2(ui.available_width(), (ui.available_height() - 8.0 - 16.0).max(0.0)), Sense::hover());
            let g = Geom::new(body.size(), ncols, n);
            // A new size: counted again once the window rests.
            if self.geom != Some(g) {
                if self.geom.is_none() || self.resized.is_some_and(|t| t.elapsed() > Duration::from_millis(150)) {
                    self.resized = None;
                    self.start(cx, g);
                } else {
                    self.resized.get_or_insert_with(Instant::now);
                    ui.ctx().request_repaint_after(Duration::from_millis(150));
                }
            }
            let g = self.geom.unwrap_or(g);
            let pic = Rect::from_min_size(pos2(body.min.x + LABELS, body.min.y), vec2(g.w, g.h));
            // Column names over their bands, when there is room (ui 11 `txt_mut`).
            if g.cw >= 28.0 {
                for (k, h) in headers.iter().enumerate().take(g.nc) {
                    let r = Rect::from_min_size(pos2(pic.min.x + GUT + 4.0 + k as f32 * g.cw - 8.0, cols_r.min.y), vec2(g.cw + 5.0, 18.0));
                    kit::cell_text(ui, &pt, r, h, kit::ui(11.0), p.txt_mut, false);
                }
            }
            // Row positions down the side (ui 600 10 `txt_dim`).
            let step = tick_step(g.h, g.n);
            for i in (step..g.n).step_by(step) {
                let y = body.min.y + i as f32 / g.n as f32 * g.h;
                pt.text(pos2(body.min.x + 40.0, y), Align2::RIGHT_CENTER, csvfab_engine::look::compact_fr(i as f64).replace(char::is_whitespace, ""), crate::grid::semibold(10.0), p.txt_dim);
            }
            // The picture.
            if counts.version > 0 && self.tex.as_ref().is_none_or(|t| t.1 != counts.version) {
                let img = self.image(&counts, g, p, ppp, ui.ctx().input(|i| i.max_texture_side));
                match &mut self.tex {
                    Some((t, v)) => { t.set(img, TextureOptions::NEAREST); *v = counts.version; }
                    None => self.tex = Some((ui.ctx().load_texture("file-map", img, TextureOptions::NEAREST), counts.version)),
                }
            }
            if let Some((t, _)) = &self.tex {
                pt.image(t.id(), pic, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            let resp = ui.interact(pic, ui.scope_id().with("map-pic"), Sense::click()).on_hover_cursor(egui::CursorIcon::Crosshair);
            let mut info = "Hover the map to read it, click to go there.".to_string();
            if let Some((b, k)) = resp.hover_pos().filter(|_| counts.bins.len() == g.nb).and_then(|q| at(g, q - pic.min)) {
                let bin = &counts.bins[b];
                let (a, z) = (g.lo(b) + 1, g.lo(b + 1));
                let span = if a == z { format!("Row {}", kit::n(a)) } else { format!("Rows {}–{}", kit::n(a), kit::n(z)) };
                let part = if (bin.seen as usize) < z + 1 - a { format!(" ({} read)", kit::n(bin.seen as usize)) } else { String::new() };
                let what = match k {
                    None => {
                        let v: Vec<String> = [(bin.irr, "irregular"), (bin.edit, "edited")].iter().filter(|x| x.0 > 0).map(|x| format!("{} {}", kit::n(x.0 as usize), x.1)).collect();
                        if v.is_empty() { "no irregular nor edited row".into() } else { v.join(" · ") }
                    }
                    Some(k) => {
                        let kind = match self.kinds.get(k) { Some(Kind::Num) => "numbers", Some(Kind::Date) => "dates", Some(Kind::Empty) => "empty", _ => "text" };
                        let filled = if bin.seen > 0 { (100.0 * bin.fill[k] as f64 / bin.seen as f64).round() } else { 0.0 };
                        let bad = if bin.bad[k] > 0 { format!(" · {} not {kind}", kit::n(bin.bad[k] as usize)) } else { String::new() };
                        format!("{} ({kind}) · {filled} % filled{bad}", headers.get(k).map_or("", |s| s.as_str()))
                    }
                };
                info = format!("{span}{part} · {what}");
                // The run outlined: 1 px `txt_hi`.
                let bh = g.h / g.nb as f32;
                let x0 = k.map_or(0.0, |k| GUT + 4.0 + k as f32 * g.cw);
                let r = Rect::from_min_size(pic.min + vec2(x0, b as f32 * bh), vec2(if k.is_none() { GUT - 1.0 } else { g.cw }, bh.max(2.0)));
                pt.rect_stroke(r, 0.0, Stroke::new(1.0, p.txt_hi), egui::StrokeKind::Outside);
                if resp.clicked() {
                    // The run's first value not of its column's type, else the row under the pointer.
                    let size = z + 1 - a;
                    let frac = (resp.interact_pointer_pos().map_or(pic.min.y, |q| q.y) - pic.min.y) / bh - b as f32;
                    let mut i = g.lo(b) + ((frac.max(0.0) * size as f32) as usize).min(size - 1);
                    if let Some(f) = k.and_then(|k| bin.first_bad[k]) { i = f; }
                    go = Some((i, k.unwrap_or(0)));
                }
            }
            ui.add_space(8.0);
            let (ir, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
            kit::cell_text(ui, &pt, ir.expand2(vec2(8.0, 0.0)), &info, kit::ui(12.0), p.txt_faint, false);
        });
        if let Some((i, c)) = go {
            let g = &mut cx.tab.grid;
            g.select(crate::grid::Cell { row: i, col: c }, false);
            let page = g.page;
            g.set_top(i.saturating_sub(page / 2));
            close = true;
        }
        open && !close
    }
}

fn has_rows(t: &crate::tab::Tab) -> bool {
    t.table.is_some() && t.shown() > 0
}

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Map", group: "View", keys: Some("Ctrl+M"), enabled: has_rows, run: |cx| { let m = Map::new(cx); cx.out.push(Cmd::Tool(Box::new(m))); } }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use csvfab_engine::{Base, OpenOptions, Table};

    #[test]
    fn counts_runs() {
        let t = Table::new(Arc::new(Base::from_bytes(b"A,B\n1,x\n2,\nzz,y\n4,w,extra\n".to_vec(), OpenOptions::default())), None);
        let g = Geom::new(vec2(LABELS + 200.0, 2.0), 2, 4);
        assert_eq!((g.nb, g.nc), (2, 2));
        let out = Arc::new(Mutex::new(Counts::default()));
        count(t.st.clone(), None, g, vec![Kind::Num, Kind::Text], out.clone(), &AtomicBool::new(false), &AtomicUsize::new(0));
        let c = out.lock().unwrap();
        assert_eq!(c.round, c.rounds);
        // Run 0: rows 1–2, B filled once. Run 1: "zz" is not a number; row 4 has three fields.
        assert_eq!((c.bins[0].seen, c.bins[0].fill[1], c.bins[0].bad[0]), (2, 1, 0));
        assert_eq!((c.bins[1].bad[0], c.bins[1].first_bad[0], c.bins[1].irr), (1, Some(2), 1));
        assert_eq!(at(g, vec2(1.0, 1.5)), Some((1, None)));
        assert_eq!(tick_step(500.0, 1000), 100);
    }
}
