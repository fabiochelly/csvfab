//! The column charts band under the titles (docs/look.md, "Column charts band"):
//! a density curve and fill gauge for numbers and dates, a ribbon of the most
//! frequent values for text; the rows shown drawn over the whole file's.
//!
//! The whole file's picture is computed once per table version in a `Job`
//! (an even sample, as the JS: `Sample::Js`); the rows shown's overlay again
//! in the background each time the view settles. A frame only draws: the old
//! pictures stay up until the new ones arrive, so nothing flickers.

use crate::grid;
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use csvfab_engine::look;
use csvfab_engine::sparkline::{self as sp, Col, ColF, Part, Pick, Sample, Spark, SparkF};
use csvfab_engine::view::{Keeps, Kind};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the whole file's picture was made from: the bytes, the edits, the columns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BaseKey {
    base: usize,
    version: u64,
    ncols: usize,
    header: bool,
}

/// What the overlay was made from: the rows shown and the picture it counts into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FiltKey {
    view: usize,
    len: usize,
    gen: u64,
    base: usize,
}

/// A part clicked: the value filter it set on its column (`view::Keeps::Part`, with the picture
/// it was picked on), and what it keeps, said (the filter's chip).
#[derive(Clone)]
pub struct Picked {
    pick: Pick,
    spark: Arc<Spark>,
    pub label: String,
}

/// What the band keeps between frames (the charts computed, the jobs computing them).
#[derive(Default)]
pub struct State {
    pub base: Option<Arc<Spark>>,
    base_key: Option<BaseKey>,
    base_job: Option<(Job<Option<Spark>>, BaseKey)>,
    filt: Option<(Arc<SparkF>, FiltKey)>,
    filt_job: Option<(Job<Option<SparkF>>, FiltKey)>,
    /// When the first picture came (the bars rise once), when the overlay last did.
    born: Option<Instant>,
    filt_born: Option<Instant>,
    /// The part under the pointer, last frame: the grid lights its cells.
    pub lit: Option<(usize, Pick)>,
    lit_next: Option<(usize, Pick)>,
    /// A drag across a curve's bars: (column, first bar, bar under the pointer).
    brush: Option<(usize, usize, usize)>,
    pub picks: HashMap<usize, Picked>,
}

impl State {
    /// The rows shown's overlay, when the view is filtered.
    pub fn filtered(&self) -> Option<&SparkF> {
        self.filt.as_ref().map(|f| &*f.0)
    }
}

/// Picks up finished jobs and starts the ones the table or the view need. Once a frame,
/// while the band is shown.
pub fn poll(tab: &mut Tab, ctx: &egui::Context) {
    let Some(t) = &tab.table else { return };
    let st = t.st.clone();
    let key = BaseKey { base: Arc::as_ptr(&st.base) as usize, version: t.version(), ncols: st.ncols, header: st.header };
    let busy = tab.busy();
    let s = &mut tab.spark;
    let mut fresh = false;
    if let Some((j, k)) = &mut s.base_job {
        if let Some(r) = j.poll() {
            let k = *k;
            s.base_job = None;
            if let Some(pic) = r {
                if s.base.is_none() { s.born = Some(Instant::now()); }
                s.base = Some(Arc::new(pic));
                s.base_key = Some(k);
                s.filt_job = None;
                fresh = true;
            }
        }
    }
    if s.base_key != Some(key) && s.base_job.as_ref().map(|x| x.1) != Some(key) {
        // Another column count: the old picture would be another column's.
        if s.base.as_ref().is_some_and(|b| b.cols.len() != st.ncols) {
            s.base = None;
            s.filt = None;
        }
        let (st2, kinds) = (st.clone(), tab.kinds.clone());
        s.base_job = Some((Job::spawn(ctx, move |c, _| sp::spark_base(&st2, &kinds, Sample::Js, c)), key));
    }
    // The rows shown, counted into the whole file's bars and values.
    let want = match (&tab.view, &s.base) {
        (Some(v), Some(b)) if v.len() != st.len() || tab.restrict.is_some() => {
            Some((v.clone(), FiltKey { view: Arc::as_ptr(v) as usize, len: v.len(), gen: tab.generation, base: Arc::as_ptr(b) as usize }))
        }
        _ => None,
    };
    match want {
        None if !busy => {
            s.filt = None;
            s.filt_job = None;
        }
        None => {}
        Some((ids, k)) => {
            if let Some((j, jk)) = &mut s.filt_job {
                if let Some(r) = j.poll() {
                    let jk = *jk;
                    s.filt_job = None;
                    if let Some(f) = r {
                        s.filt = Some((Arc::new(f), jk));
                        s.filt_born = Some(Instant::now());
                    }
                }
            }
            // An overlay counted into another picture's bars would draw wrong.
            if s.filt.as_ref().is_some_and(|f| f.1.base != k.base) { s.filt = None; }
            let have = s.filt.as_ref().map(|f| f.1) == Some(k) || s.filt_job.as_ref().map(|x| x.1) == Some(k);
            if !have && !busy {
                let (st2, base) = (st.clone(), s.base.clone().unwrap());
                s.filt_job = Some((Job::spawn(ctx, move |c, _| sp::spark_filtered(&st2, &base, &ids, Sample::Js, c)), k));
            }
        }
    }
    // The grid's pills take the ribbons' values and colours.
    if fresh {
        if let Some(b) = &tab.spark.base {
            if tab.kinds.len() == b.cols.len() {
                tab.pills = look::pills_from_spark(b, &tab.kinds, &tab.rich);
            }
        }
    }
}

/// Called by the grid around the band's calls (twice with frozen columns).
pub fn frame_start(tab: &mut Tab) {
    tab.spark.lit_next = None;
}

pub fn frame_end(tab: &mut Tab) {
    tab.spark.lit = tab.spark.lit_next.take();
}

/// The gutter cell's tooltip.
pub fn gutter_tip(tab: &Tab) -> String {
    let from = match &tab.spark.base {
        None => "Column charts".to_string(),
        Some(s) if s.sampled => format!("Column charts — from {} rows taken evenly through the {}", grid::fmt_int(s.s), grid::fmt_int(s.n)),
        Some(s) => format!("Column charts — from all {} rows", grid::fmt_int(s.n)),
    };
    format!("{from}\nHover a bar to read it, click it to filter\nClick here to hide them")
}

/// `26 %`, `0,4 %`: one decimal under 10 %.
fn pct(x: f64) -> String {
    let v = x * 100.0;
    if x < 0.1 {
        let r = (v * 10.0).round() / 10.0;
        if r.fract() == 0.0 { format!("{} %", r as i64) } else { format!("{:.1} %", r).replace('.', ",") }
    } else {
        format!("{} %", v.round() as i64)
    }
}

/// The CSS easing of the arrival, cubic-bezier(.2, .9, .25, 1), near enough.
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powf(3.2)
}

fn kind_color(p: &Palette, c: &Col) -> Color32 {
    match c {
        Col::Curve(cv) if cv.bins.kind == Kind::Date => p.violet_txt,
        _ => p.cyan_txt,
    }
}

/// A smooth curve's outline (the monotone cubic of `sp::area`), flattened, in `r`; `rise` scales its height.
fn curve_points(cnt: &[u32], r: Rect, rise: f32) -> Vec<Pos2> {
    let b = cnt.len() as f32;
    let sx = r.width() / b;
    let (y0, segs) = sp::area(cnt);
    let y = |v: f64| r.max.y - (v as f32 / 100.0) * r.height() * rise;
    let x = |u: f64| r.min.x + u as f32 * sx;
    let mut pts = Vec::with_capacity(segs.len() * 8 + 4);
    pts.push(pos2(x(0.0), y(y0)));
    pts.push(pos2(x(0.5), y(y0)));
    let (mut px, mut py) = (0.5f64, y0);
    for s in &segs {
        for k in 1..=8 {
            let t = k as f64 / 8.0;
            let u = 1.0 - t;
            let (a, b1, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            pts.push(pos2(x(a * px + b1 * s[0] + c * s[2] + d * s[4]), y(a * py + b1 * s[1] + c * s[3] + d * s[5])));
        }
        px = s[4];
        py = s[5];
    }
    pts.push(pos2(x(b as f64), y(py)));
    pts
}

/// The area under `pts` down to `r`'s bottom, each vertex coloured by `col(y)`.
fn area_mesh(pts: &[Pos2], r: Rect, col: impl Fn(f32) -> Color32) -> Shape {
    let mut mesh = Mesh::default();
    let bottom = col(r.max.y);
    for (i, p) in pts.iter().enumerate() {
        mesh.colored_vertex(*p, col(p.y));
        mesh.colored_vertex(pos2(p.x, r.max.y), bottom);
        if i > 0 {
            let k = (i as u32) * 2;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    Shape::mesh(mesh)
}

/// The type colour's vertical gradient of the charts: `kc` at the top, `kc` at .28 at the bottom.
fn gradient(kc: Color32, r: Rect, op: f32) -> impl Fn(f32) -> Color32 {
    move |y: f32| {
        let f = ((y - r.min.y) / r.height()).clamp(0.0, 1.0);
        alpha(kc, 1.0 - 0.72 * f).gamma_multiply(op)
    }
}

/// A convex polygon (a bar with a rounded top), each vertex coloured by `col(y)`.
fn convex_mesh(pts: &[Pos2], col: impl Fn(f32) -> Color32) -> Shape {
    let mut mesh = Mesh::default();
    let c = (pts.iter().fold(vec2(0.0, 0.0), |a, p| a + p.to_vec2()) / pts.len() as f32).to_pos2();
    mesh.colored_vertex(c, col(c.y));
    for p in pts {
        mesh.colored_vertex(*p, col(p.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    Shape::mesh(mesh)
}

/// A bar's outline: its top corners rounded.
fn bar_outline(x0: f32, w: f32, top: f32, bottom: f32) -> Vec<Pos2> {
    let rad = (w * 0.3).min((bottom - top) * 0.5).min(3.0);
    let mut v = vec![pos2(x0, bottom)];
    for k in 0..=4 {
        let a = std::f32::consts::FRAC_PI_2 * k as f32 / 4.0;
        v.push(pos2(x0 + rad - rad * a.cos(), top + rad - rad * a.sin()));
    }
    for k in 0..=4 {
        let a = std::f32::consts::FRAC_PI_2 * (1.0 - k as f32 / 4.0);
        v.push(pos2(x0 + w - rad + rad * a.cos(), top + rad - rad * a.sin()));
    }
    v.push(pos2(x0 + w, bottom));
    v
}

/// Draws a curve's counts (a smooth area, or one bar a value: 76 % of its slot) in `r`, `op`
/// its opacity; `only`: bars a..=b alone, solid (the part under the pointer).
fn draw_counts(pt: &Painter, cnt: &[u32], r: Rect, kc: Color32, op: f32, rise: f32, only: Option<(usize, usize)>) {
    if cnt.iter().all(|&n| n == 0) || r.width() < 2.0 || op <= 0.0 {
        return;
    }
    let sx = r.width() / cnt.len() as f32;
    if sp::smooth(cnt) {
        let pts = curve_points(cnt, r, rise);
        match only {
            None => {
                pt.add(area_mesh(&pts, r, gradient(kc, r, 0.55 * op)));
                pt.add(Shape::line(pts, Stroke::new(1.6, kc.gamma_multiply(op))));
            }
            Some((a, z)) => {
                // The part alone, solid, through the curve's own shape.
                let clip = Rect::from_x_y_ranges(r.min.x + a as f32 * sx..=r.min.x + (z + 1) as f32 * sx, r.min.y - 2.0..=r.max.y).intersect(pt.clip_rect());
                let pc = pt.with_clip_rect(clip);
                pc.add(area_mesh(&pts, r, |_| kc));
                pc.add(Shape::line(pts, Stroke::new(1.6, kc)));
            }
        }
        return;
    }
    for (k, &hv) in sp::heights(cnt).iter().enumerate() {
        if hv <= 0.0 || only.is_some_and(|(a, z)| k < a || k > z) {
            continue;
        }
        let top = r.max.y - (hv as f32 / 100.0) * r.height() * rise;
        let pts = bar_outline(r.min.x + (k as f32 + 0.12) * sx, sx * 0.76, top, r.max.y);
        if only.is_some() { pt.add(convex_mesh(&pts, |_| kc)); } else { pt.add(convex_mesh(&pts, gradient(kc, r, op))); }
    }
}

/// Part `x0..x1` of a 6 px bar with rounded ends (rounded only at the bar's own ends).
fn bar_part(pt: &Painter, bar: Rect, x0: f32, x1: f32, c: Color32) {
    if x1 - x0 <= 0.0 {
        return;
    }
    let (first, last) = (x0 <= bar.min.x + 0.01, x1 >= bar.max.x - 0.01);
    let r = |on: bool| if on { 3 } else { 0 };
    pt.rect_filled(Rect::from_x_y_ranges(x0..=x1, bar.y_range()), CornerRadius { nw: r(first), sw: r(first), ne: r(last), se: r(last) }, c);
}

/// What the pointer is on in a column's box: the pick and its span in px from the box's left.
fn part_at(spark: &Spark, f: Option<&SparkF>, col: usize, bx: Rect, pos: Pos2) -> Option<(Pick, f32, f32)> {
    let w = bx.width();
    let x = (pos.x - bx.min.x).min(w - 0.01).max(0.0);
    let c = spark.cols.get(col)?;
    let gauge = !matches!(c, Col::Ribbon(_)) && pos.y > bx.max.y - 10.0;
    if gauge || matches!(c, Col::Empty { .. }) {
        let fill = spark.fill(col, f);
        let split = sp::gauge_split(fill, w as f64) as f32;
        return Some(if fill > 0.0 && (x < split || fill >= 1.0) { (Pick::Filled, 0.0, split) } else { (Pick::Empty, split, w) });
    }
    match c {
        Col::Curve(cv) => {
            let b = ((x / w * cv.bins.n as f32).floor() as usize).min(cv.bins.n - 1);
            let sw = w / cv.bins.n as f32;
            Some((Pick::Bars(b, b), b as f32 * sw, (b + 1) as f32 * sw))
        }
        Col::Ribbon(_) => {
            let parts = spark.ribbon(col, f);
            let (k, at, pw) = sp::ribbon_at(&parts, w as f64, x as f64)?;
            let pick = match parts[k].0 {
                Part::Seg(i) => Pick::Seg(i),
                Part::Other => Pick::Other,
                Part::Empty => Pick::Empty,
            };
            Some((pick, at as f32, (at + pw) as f32))
        }
        Col::Empty { .. } => None,
    }
}

/// The tooltip of a part, as the JS writes it (spkTip).
fn tip_text(tab: &Tab, spark: &Spark, col: usize, pick: Pick, brush: bool) -> String {
    let f = tab.spark.filtered();
    let title = tab.st().and_then(|s| s.headers().get(col).cloned()).unwrap_or_default();
    let est = |n: usize| format!("{}{}", if spark.sampled { "≈ " } else { "" }, grid::fmt_int(spark.estimate(n) as usize));
    let shown = tab.shown();
    let fest = |n: usize, f: &SparkF| format!("{}{}", if f.s < shown { "≈ " } else { "" }, grid::fmt_int((n as f64 * shown as f64 / f.s.max(1) as f64).round() as usize));
    let picked = tab.spark.picks.get(&col).is_some_and(|p| p.pick == pick && in_force(tab, col, p));
    let rows = |n: usize| if n == 1 && !spark.sampled { "row" } else { "rows" };
    let (n, fnum) = spark.count(col, pick, f);
    let fline = match (f, fnum) {
        (Some(f), Some(k)) => format!("\n{} of the rows shown", fest(k, f)),
        _ => String::new(),
    };
    let s = spark.s.max(1) as f64;
    if matches!(pick, Pick::Filled | Pick::Empty) && !matches!(spark.cols[col], Col::Ribbon(_)) {
        let e = pick == Pick::Empty;
        let has = fnum.unwrap_or(n) > 0;
        let hint = if !has { String::new() } else if picked { "\nClick: show all again".into() } else { format!("\nClick: show only the {} rows", if e { "empty" } else { "filled" }) };
        let bad = match &spark.cols[col] {
            Col::Curve(c) if c.bad > 0 && !e => format!("\n{} not {}", est(c.bad), if c.bins.kind == Kind::Number { "numbers" } else { "dates" }),
            _ => String::new(),
        };
        return format!("{title} — {}: {} {} · {}{fline}{bad}{hint}", if e { "empty" } else { "filled" }, est(n), rows(n), pct(n as f64 / s));
    }
    let mut what = spark.what(col, pick);
    if what.chars().count() > 120 {
        what = what.chars().take(120).collect::<String>() + "…";
    }
    let bars = matches!(pick, Pick::Bars(..));
    let hint = if brush {
        if n > 0 { "\nRelease: show only these".into() } else { String::new() }
    } else if n > 0 {
        format!("\n{}{}", if picked { "Click: show all again" } else { "Click: show only these" }, if bars { " · drag across: a range" } else { "" })
    } else {
        String::new()
    };
    format!("{what}\n{} {} · {}{fline}{hint}", est(n), rows(n), pct(n as f64 / s))
}

/// Whether a pick's filter is still the one in force on its column.
fn in_force(tab: &Tab, col: usize, p: &Picked) -> bool {
    tab.filter.picks.iter().any(|v| v.col == col && matches!(&v.keep, Keeps::Part(s, k) if *k == p.pick && Arc::ptr_eq(s, &p.spark)))
}

/// A click on a part (or a range dragged across): a value filter on its column keeping the
/// rows that hold it, as the web's (the column's other value filter replaced, the other
/// columns' and the typed filters kept); on the part already picked, the column shown whole
/// again. Said in a message.
fn pick(tab: &mut Tab, col: usize, pick: Pick, ctx: &egui::Context) {
    let Some(spark) = tab.spark.base.clone() else { return };
    let title = tab.st().and_then(|st| st.headers().get(col).cloned()).unwrap_or_default();
    if let Some(p) = tab.spark.picks.remove(&col) {
        if in_force(tab, col, &p) && p.pick == pick {
            tab.keep_values(col, None, ctx);
            tab.grid.notice = Some(format!("{title}: every value shown again."));
            return;
        }
    }
    let (n, _) = spark.count(col, pick, None);
    if n == 0 {
        tab.grid.notice = Some("No row holds those values.".into());
        return;
    }
    if !spark.sampled && n >= spark.n && !tab.has_values(col) {
        tab.grid.notice = Some(format!("{title}: every row already holds those values."));
        return;
    }
    let what = match pick {
        Pick::Filled => "the filled rows".to_string(),
        Pick::Empty => "the empty rows".to_string(),
        Pick::Other => "the other values".to_string(),
        Pick::Seg(_) => format!("“{}”", spark.what(col, pick)),
        Pick::Bars(a, b) => match spark.bar_values(col, a, b) {
            Some((lo, hi)) if lo == hi => lo.to_string(),
            Some((lo, hi)) => format!("{lo} to {hi}"),
            None => spark.what(col, pick).replace(" – ", " to "),
        },
    };
    tab.spark.picks.insert(col, Picked { pick, spark: spark.clone(), label: what.clone() });
    tab.keep_values(col, Some(Keeps::Part(spark, pick)), ctx);
    tab.grid.notice = Some(format!("{title}: only {what} — click it again to show all."));
}

/// Draws the band's columns `(col, x, width)` inside `clip` (the part right of the gutter,
/// or of the frozen columns). A click or drag on a chart filters.
#[allow(clippy::too_many_arguments)]
pub fn band(ui: &mut Ui, tab: &mut Tab, painter: &Painter, band: Rect, clip: Rect, vis_cols: &[(usize, f32, f32)], p: &Palette, m: &Metrics) {
    let ctx = ui.ctx().clone();
    let clip = clip.intersect(painter.clip_rect());
    let pt = painter.with_clip_rect(clip);
    let Some(spark) = tab.spark.base.clone() else { return };
    let f = tab.spark.filt.as_ref().map(|x| x.0.clone());
    let now = Instant::now();
    let mut animating = false;
    let label_font = FontId::new(11.5, egui::FontFamily::Name(crate::fonts::UI_MEDIUM.into()));
    // With a menu or a column's panel open, the band lights nothing (egui's rule for tooltips).
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|_| !egui::Popup::is_any_open(&ctx));
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let reduced = ctx.global_style().animation_time <= 0.0;
    for (vi, &(c, x, w)) in vis_cols.iter().enumerate() {
        let Some(col) = spark.cols.get(c) else { continue };
        let bx = Rect::from_min_size(pos2(x + m.cell_pad_x, band.min.y + 9.0), vec2((w - 2.0 * m.cell_pad_x - 1.0).max(0.0), m.spk_box_h));
        if bx.width() < 4.0 || bx.max.x < clip.min.x {
            continue;
        }
        // The arrival: bars rise, ribbons unroll, column after column; a new overlay comes in.
        let anim = |born: Option<Instant>, delay: f32, dur: f32, animating: &mut bool| match born {
            Some(b) if !reduced => {
                let t = (now.duration_since(b).as_secs_f32() - delay) / dur;
                if t < 1.0 { *animating = true; }
                ease(t)
            }
            _ => 1.0,
        };
        let rise = anim(tab.spark.born, vi.min(30) as f32 * 0.028, 0.7, &mut animating);
        let frise = anim(tab.spark.filt_born, 0.0, 0.35, &mut animating);
        let fc = f.as_deref().and_then(|f| f.cols.get(c));
        let resp = ui.interact(bx.intersect(clip), ui.scope_id().with(("spk", c)), Sense::click_and_drag());
        // Not through a menu or a dialog over the band: egui's own hit test, layers and all.
        let hov = pointer.filter(|q| resp.contains_pointer() && resp.rect.contains(*q)).and_then(|q| part_at(&spark, f.as_deref(), c, bx, q));
        // A drag across the bars: the range so far.
        let brush = tab.spark.brush.filter(|b| b.0 == c);
        let shown_part = match (brush, col) {
            (Some((_, a, z)), Col::Curve(cv)) => {
                let (lo, hi) = (a.min(z), a.max(z));
                let sw = bx.width() / cv.bins.n as f32;
                Some((Pick::Bars(lo, hi), lo as f32 * sw, (hi + 1) as f32 * sw))
            }
            _ => hov,
        };
        let lit = shown_part.filter(|x| x.2 > x.1);
        if let Some(l) = lit { tab.spark.lit_next = Some((c, l.0)); }
        let gauge_hov = matches!(lit, Some((Pick::Filled | Pick::Empty, ..))) && !matches!(col, Col::Ribbon(_));
        let cr = Rect::from_min_max(pos2(bx.min.x, bx.min.y + 1.0), pos2(bx.max.x, bx.max.y - 13.0));
        // The bars picked by a click, while their filter is the one in force.
        if let (Some(pk), Col::Curve(cv)) = (tab.spark.picks.get(&c), col) {
            if let (Pick::Bars(a, z), true) = (pk.pick, in_force(tab, c, pk)) {
                let sw = bx.width() / cv.bins.n as f32;
                pt.rect_filled(Rect::from_x_y_ranges(bx.min.x + a as f32 * sw..=bx.min.x + (z + 1) as f32 * sw, bx.min.y..=bx.max.y - 14.0), 0.0, alpha(p.prim, 0.2));
            }
        }
        match col {
            Col::Curve(cv) => {
                let kc = kind_color(p, col);
                let dim = if lit.is_some() && !gauge_hov { 0.35 } else { 1.0 };
                let hot = if resp.hovered() || brush.is_some() { 1.0 } else { 0.85 };
                let cnt = match fc {
                    Some(ColF::Curve { cnt, .. }) => {
                        draw_counts(&pt, &cv.cnt, cr, kc, 0.3 * dim, rise, None);
                        draw_counts(&pt, cnt, cr, kc, frise * dim, rise * (0.6 + 0.4 * frise), None);
                        cnt
                    }
                    _ => {
                        draw_counts(&pt, &cv.cnt, cr, kc, hot * dim, rise, None);
                        &cv.cnt
                    }
                };
                if let Some((Pick::Bars(a, z), ..)) = lit {
                    draw_counts(&pt, cnt, cr, kc, 1.0, rise, Some((a, z)));
                }
            }
            Col::Ribbon(_) => {
                let parts = spark.ribbon(c, f.as_deref());
                let rb = Rect::from_min_max(pos2(bx.min.x, bx.max.y - m.spk_gauge_h), bx.max);
                let ends = sp::ribbon_ends(&parts, bx.width() as f64);
                let color = |part: Part| match part {
                    Part::Seg(i) => p.pill.get(i).copied().unwrap_or(p.pill_other),
                    Part::Other => p.ribbon_other,
                    Part::Empty => p.ribbon_empty,
                };
                let full = bx.min.x + bx.width() * rise;
                let op = if lit.is_some() { 0.4 } else { 1.0 };
                let mut at = bx.min.x;
                for (k, (part, _)) in parts.iter().enumerate() {
                    let start = if k == 0 { at } else { at + 2.0 };
                    let end = (bx.min.x + ends[k] as f32).min(full);
                    if end > start {
                        let (first, last) = (k == 0, k + 1 == parts.len());
                        let r = |on: bool| if on { 3 } else { 0 };
                        let cr = CornerRadius { nw: r(first), sw: r(first), ne: r(last), se: r(last) };
                        pt.rect_filled(Rect::from_x_y_ranges(start..=end, rb.y_range()), cr, color(*part).gamma_multiply(op));
                    }
                    at = bx.min.x + ends[k] as f32;
                }
                if let Some((pk, x0, x1)) = lit {
                    let c2 = match pk { Pick::Seg(i) => p.pill.get(i).copied().unwrap_or(p.pill_other), _ => alpha(p.gray_hi, 0.55) };
                    let a = bx.min.x + x0 + if x0 > 0.01 { 2.0 } else { 0.0 };
                    bar_part(&pt, rb, a, bx.min.x + x1, c2);
                }
                ribbon_label(&pt, &spark, f.as_deref(), c, bx, p, &label_font, rise);
            }
            Col::Empty { .. } => {}
        }
        // The gauge of a number, date or empty column: one piece, as a progress bar.
        if !matches!(col, Col::Ribbon(_)) {
            let g = Rect::from_min_max(pos2(bx.min.x, bx.max.y - m.spk_gauge_h), bx.max);
            let fill = spark.fill(c, f.as_deref());
            let split = g.min.x + sp::gauge_split(fill, g.width() as f64) as f32;
            let gw = g.min.x + g.width() * rise;
            pt.with_clip_rect(Rect::from_x_y_ranges(g.min.x..=gw, g.y_range()).intersect(clip)).rect_filled(g, 3.0, p.gauge_empty);
            if split > g.min.x {
                pt.with_clip_rect(Rect::from_x_y_ranges(g.min.x..=split.min(gw), g.y_range()).intersect(clip)).rect_filled(g, 3.0, p.gauge_fill);
            }
            if let (Some((pk, x0, x1)), true) = (lit, gauge_hov) {
                bar_part(&pt, g, g.min.x + x0, g.min.x + x1, if pk == Pick::Filled { p.ok } else { alpha(p.gray_hi, 0.55) });
            }
        }
        // The pointer: a press on a bar starts a range, a click picks.
        if let Some((pk, ..)) = hov {
            if resp.hovered() { ctx.set_cursor_icon(egui::CursorIcon::PointingHand); }
            if resp.drag_started() {
                if let Pick::Bars(b, _) = pk { tab.spark.brush = Some((c, b, b)); }
            }
            if resp.clicked() && brush.is_none() {
                pick(tab, c, pk, &ctx);
            }
        }
        if let Some((_, b0, mut b1)) = brush {
            if let (Some(q), Col::Curve(cv)) = (ui.input(|i| i.pointer.interact_pos()), col) {
                b1 = (((q.x - bx.min.x) / bx.width() * cv.bins.n as f32).floor().max(0.0) as usize).min(cv.bins.n - 1);
            }
            tab.spark.brush = Some((c, b0, b1));
            if !primary_down {
                tab.spark.brush = None;
                pick(tab, c, Pick::Bars(b0.min(b1), b0.max(b1)), &ctx);
            }
        }
        if let Some((pk, x0, x1)) = lit {
            let text = tip_text(tab, &spark, c, pk, brush.is_some());
            let anchor = Rect::from_x_y_ranges(bx.min.x + x0..=bx.min.x + x1, bx.y_range());
            grid::tip(&ctx, p, m, anchor, &text);
        }
    }
    if animating {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
}

/// A text column's label above its ribbon: its most frequent value and share, or how many
/// values and how full.
#[allow(clippy::too_many_arguments)]
fn ribbon_label(pt: &Painter, spark: &Spark, f: Option<&SparkF>, col: usize, bx: Rect, p: &Palette, font: &FontId, rise: f32) {
    let Col::Ribbon(r) = &spark.cols[col] else { return };
    let fill = f.and_then(|f| f.cols.get(col)).map_or(r.fill, |x| x.fill());
    let op = ((rise - 0.2) / 0.8).clamp(0.0, 1.0);
    let base_y = bx.max.y - 12.0;
    let w = bx.width();
    let s_rows = f.map_or(spark.s, |f| f.s).max(1) as f64;
    let ppp = pt.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let line = |text: String, font: FontId, c: Color32, max: f32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text, font, c.gamma_multiply(op));
        job.wrap = egui::text::TextWrapping { max_width: max.max(4.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        pt.layout_job(job)
    };
    if fill == 0 {
        let g = line("empty".into(), font.clone(), p.txt_faint, w);
        pt.galley(pos2(bx.min.x, snap(base_y - 7.5 - g.size().y / 2.0)), g, p.txt_faint);
        return;
    }
    if let Some(top) = spark.top(col, f) {
        let seg = match f.and_then(|f| f.cols.get(col)) {
            Some(ColF::Ribbon { seg, .. }) => seg[top] as usize,
            _ => r.segs[top].1 as usize,
        };
        let cy = base_y - 7.5;
        pt.circle_filled(pos2(bx.min.x + 3.0, cy + 0.5), 3.0, p.pill.get(top).copied().unwrap_or(p.pill_other).gamma_multiply(op));
        let g = line(grid::display(r.segs[top].0.trim()).into_owned(), grid::semibold(11.5), p.txt_hi, w * 0.7 - 11.0);
        let gx = bx.min.x + 11.0;
        let gw = g.size().x;
        pt.galley(pos2(gx, snap(cy - g.size().y / 2.0)), g, p.txt_hi);
        let g2 = line(pct(seg as f64 / s_rows), font.clone(), p.txt_faint, w);
        pt.galley(pos2(gx + gw + 5.0, snap(cy - g2.size().y / 2.0)), g2, p.txt_faint);
        return;
    }
    // No value stands out: how many values, how full, on two lines.
    let small = FontId::new(10.0, egui::FontFamily::Name(crate::fonts::UI_MEDIUM.into()));
    let distinct = if r.distinct == r.fill && !spark.sampled { "all distinct".to_string() } else { format!("{}{} values", grid::fmt_int(r.distinct), if r.many || spark.sampled { "+" } else { "" }) };
    let g1 = line(distinct, small.clone(), p.txt_sub, w);
    let g2 = line(format!("{} filled", pct(fill as f64 / s_rows)), small, alpha(p.ok_rgb, 0.85), w);
    pt.galley(pos2(bx.min.x, snap(base_y - 12.0 - 2.0 - 6.0 - g1.size().y / 2.0)), g1, p.txt_sub);
    pt.galley(pos2(bx.min.x, snap(base_y - 6.0 - g2.size().y / 2.0)), g2, p.txt_sub);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_as_the_js() {
        assert_eq!(pct(0.26), "26 %");
        assert_eq!(pct(1.0), "100 %");
        assert_eq!(pct(0.004), "0,4 %");
        assert_eq!(pct(0.05), "5 %");
    }

    #[test]
    fn curve_stays_in_its_box() {
        let cnt: Vec<u32> = (0..32).map(|i| (i * 7 % 13) as u32).collect();
        let r = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 31.0));
        let pts = curve_points(&cnt, r, 1.0);
        assert!(pts.iter().all(|p| p.y >= r.min.y - 0.01 && p.y <= r.max.y + 0.01 && p.x >= r.min.x - 0.01 && p.x <= r.max.x + 0.01));
        assert_eq!(pts.first().unwrap().x, r.min.x);
        assert!((pts.last().unwrap().x - r.max.x).abs() < 0.01);
    }
}
