//! Group by (26-group-by.js): the rows shown, grouped on the columns ticked
//! (in tick order), a count per group and, for the columns ticked below, a
//! sum, average, min, max or number of distinct values. A preview on the
//! first rows follows every change; the result covers every row shown and
//! opens as a new tab (`Cmd::OpenBytes`), not on disk until saved.

use super::analysis_kit as kit;
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::grid::semibold;
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Palette};
use csvfab_engine::group::{self, Aggs, GroupSpec, Match};
use csvfab_engine::view::Kind;
use csvfab_engine::{RowId, State};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// The preview groups the first rows only: it follows every tick at once.
const PREVIEW_ROWS: usize = 50_000;
/// Lines of the preview drawn.
const PREVIEW_LINES: usize = 8;
const FNS: [&str; 5] = ["Sum", "Average", "Min", "Max", "Distinct"];

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Group by…", group: "Rows", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(open(cx.tab, None))) }]
}

/// The dialog on `tab`, grouping on `col` first when given (the column panel's "Group by").
pub fn open(tab: &Tab, col: Option<usize>) -> Box<dyn Tool> {
    Box::new(GroupBy::new(tab, col))
}

struct Preview {
    header: Vec<String>,
    lines: Vec<Vec<String>>,
    groups: usize,
    rows: usize,
}

/// The choices made last on each tab (its uid; the column count they were for): the dialog
/// opens with them again, as the web's kept its settings.
static LAST: std::sync::Mutex<Option<std::collections::HashMap<u64, (usize, Vec<usize>, Vec<Aggs>, Match)>>> = std::sync::Mutex::new(None);

struct GroupBy {
    uid: u64,
    st: Option<State>,
    headers: Vec<String>,
    kinds: Vec<Kind>,
    /// The rows shown when the dialog opened.
    ids: Arc<Vec<RowId>>,
    filtered: bool,
    /// Columns grouped on, in tick order.
    order: Vec<usize>,
    /// Per column, what is computed.
    aggs: Vec<Aggs>,
    how: Match,
    preview: Option<Preview>,
    pjob: Option<Job<Option<Preview>>>,
    stale: bool,
    /// The whole result being made: (tab name, bytes, groups).
    run: Option<Job<Option<(String, Vec<u8>, usize)>>>,
}

impl GroupBy {
    fn new(tab: &Tab, col: Option<usize>) -> GroupBy {
        let st = tab.st().cloned();
        let headers = st.as_ref().map(|s| s.headers()).unwrap_or_default();
        let kinds = tab.kinds.clone();
        let mut aggs: Vec<Aggs> = (0..headers.len())
            .map(|c| if Some(c) != col && st.as_ref().is_some_and(|s| group::sum_by_default(s, &headers, &kinds, c)) { Aggs::SUM } else { Aggs::default() })
            .collect();
        let (mut order, mut how): (Vec<usize>, Match) = (col.into_iter().collect(), Match::Loose);
        if let Some((_, o, a, h)) = LAST.lock().ok().and_then(|m| m.as_ref()?.get(&tab.uid).cloned()).filter(|l| l.0 == headers.len()) {
            how = h;
            aggs = a;
            // A column asked for (its panel's "Group by") goes first; else the last grouping.
            order = match col { Some(c) => { let mut v = vec![c]; v.extend(o.into_iter().filter(|&k| k != c)); v } None => o };
            if let Some(c) = col { aggs[c] = Aggs::default(); }
        }
        GroupBy {
            uid: tab.uid,
            st,
            headers,
            kinds,
            ids: tab.shown_ids(),
            filtered: tab.filtered(),
            order,
            aggs,
            how,
            preview: None,
            pjob: None,
            stale: true,
            run: None,
        }
    }

    fn spec(&self) -> GroupSpec {
        spec_of(&self.order, &self.aggs, self.how)
    }

    fn delim(&self) -> u8 {
        self.st.as_ref().map_or(b',', |s| s.base.dialect.delim)
    }

    /// A new preview in the background; the one running is dropped (cancelled).
    fn refresh(&mut self, ctx: &egui::Context) {
        self.stale = false;
        self.pjob = None;
        let Some(st) = self.st.clone() else { return };
        if self.order.is_empty() { self.preview = None; return; }
        let (spec, kinds, ids, delim) = (self.spec(), self.kinds.clone(), self.ids.clone(), self.delim());
        self.pjob = Some(Job::spawn(ctx, move |cancel, _| {
            let sample = &ids[..ids.len().min(PREVIEW_ROWS)];
            let g = group::group_by(&st, sample, &spec, &kinds, cancel, None)?;
            let lines = (0..g.len().min(PREVIEW_LINES)).map(|i| { let mut r = Vec::new(); g.row(i, delim, &mut r); r }).collect();
            Some(Preview { header: g.header().to_vec(), lines, groups: g.len(), rows: sample.len() })
        }));
    }

    fn apply(&mut self, cx: &mut Cx) {
        let Some(st) = self.st.clone() else { return };
        let (spec, kinds, ids, delim) = (self.spec(), self.kinds.clone(), self.ids.clone(), self.delim());
        let stem = csvfab_engine::schema::stem(&cx.tab.name).to_string();
        self.run = Some(Job::spawn(cx.ctx, move |cancel, progress| {
            let g = group::group_by(&st, &ids, &spec, &kinds, cancel, Some(progress))?;
            Some((g.tab_name(&stem), g.to_csv(delim), g.len()))
        }));
    }
}

/// The engine's spec from the dialog's ticks: grouped columns are not aggregated.
fn spec_of(order: &[usize], aggs: &[Aggs], how: Match) -> GroupSpec {
    GroupSpec {
        by: order.to_vec(),
        aggs: aggs.iter().enumerate().filter(|(c, a)| **a != Aggs::default() && !order.contains(c)).map(|(c, a)| (c, *a)).collect(),
        how,
    }
}

fn fn_flag(a: &mut Aggs, k: usize) -> &mut bool {
    match k { 0 => &mut a.sum, 1 => &mut a.avg, 2 => &mut a.min, 3 => &mut a.max, _ => &mut a.distinct }
}

impl Tool for GroupBy {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if self.st.is_none() { return false; }
        if self.stale { self.refresh(cx.ctx); }
        if let Some(r) = self.pjob.as_mut().and_then(|j| j.poll()) {
            self.pjob = None;
            if let Some(p) = r { self.preview = Some(p); }
        }
        if let Some(r) = self.run.as_mut().and_then(|j| j.poll()) {
            self.run = None;
            if let Some((name, bytes, groups)) = r {
                cx.out.push(Cmd::OpenBytes { name, bytes });
                cx.toast(format!("{} — a new tab, not on disk yet: Save as… writes it.", kit::plural(groups, "group", "groups")));
                return false;
            }
        }
        if let Some(c) = crate::shot::take("group-tick") { self.order = vec![c.parse().unwrap_or(0)]; self.stale = true; }
        let (keep, open) = kit::modal(cx, "group-by", 560.0, |ui, cx| self.body(ui, cx));
        open && keep
    }
}

impl GroupBy {
    /// The dialog's content; false once closed.
    fn body(&mut self, ui: &mut Ui, cx: &mut Cx) -> bool {
        let p = cx.p;
        kit::title(ui, p, "Group by");
        kit::label(ui, p, "Group the rows shown by — tick the columns in order");
        self.columns(ui, p);
        ui.add_space(10.0);
        // .opts: the three ways of matching keys, wrapping.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(18.0, 4.0);
            for (how, text) in [(Match::Exact, "Exact values"), (Match::Loose, "Ignoring case and surrounding spaces"), (Match::Slug, "Slugified")] {
                let r = kit::radio(ui, p, self.how == how, text);
                let r = if how == Match::Slug { r.on_hover_text("No accents, case or symbols") } else { r };
                if r.clicked() && self.how != how { self.how = how; self.stale = true; }
            }
        });
        ui.add_space(12.0);
        kit::label(ui, p, "Per group: the row count, and for the columns ticked below");
        self.agg_table(ui, p);
        ui.add_space(10.0);
        self.stats(ui, p);
        if let Some(pv) = &self.preview {
            if !self.order.is_empty() {
                ui.add_space(10.0);
                preview_table(ui, p, pv, self.order.len());
            }
        }
        if let Some(j) = &self.run {
            ui.add_space(10.0);
            let done = j.progress.load(Ordering::Relaxed) as f32 / self.ids.len().max(1) as f32;
            kit::progress(ui, p, Some(done), &format!("Grouping {} rows…", kit::n(self.ids.len())));
        }
        let mut keep = true;
        kit::actions(ui, 16.0, |ui| {
            let can = !self.order.is_empty() && self.run.is_none();
            if kit::button(ui, p, "Open the result in a new tab", kit::Btn::Primary, can).clicked() { self.apply(cx); }
            if kit::button(ui, p, "Cancel", kit::Btn::Outline, true).clicked() { keep = false; }
        });
        keep
    }

    /// The tick list of columns (`#gb-cols`): max 150 tall, the tick order in a badge.
    fn columns(&mut self, ui: &mut Ui, p: &Palette) {
        let frame = egui::Frame::new().stroke(Stroke::new(1.0, p.brd)).corner_radius(6).inner_margin(egui::Margin { left: 10, right: 10, top: 6, bottom: 0 });
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().id_salt("gb-cols").max_height(150.0 - 8.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for c in 0..self.headers.len() {
                    let k = self.order.iter().position(|&x| x == c);
                    let w = ui.available_width();
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
                    let b = Rect::from_min_size(pos2(r.min.x, kit::snap(ui, r.center().y - 8.0)), vec2(16.0, 16.0));
                    kit::tick_box(ui.painter(), p, b, k.is_some(), false, resp.hovered(), true);
                    let badge = 26.0;
                    kit::cell_text(ui, ui.painter(), Rect::from_min_max(pos2(b.max.x, r.min.y), pos2(r.max.x - badge, r.max.y)), &self.headers[c], kit::ui(13.0), p.txt_hi, false);
                    if let Some(k) = k { ord_badge(ui, p, pos2(r.max.x, r.center().y), k + 1); }
                    if resp.clicked() {
                        self.order.retain(|&x| x != c);
                        if k.is_none() { self.order.push(c); }
                        self.stale = true;
                    }
                }
                ui.add_space(5.0);
            });
        });
    }

    /// Column | Sum | Average | Min | Max | Distinct (`.pv.gb`, max 190 tall): the columns not grouped on.
    fn agg_table(&mut self, ui: &mut Ui, p: &Palette) {
        let cols: Vec<usize> = (0..self.headers.len()).filter(|c| !self.order.contains(c)).collect();
        let (rh, fw) = (25.0, 64.0);
        let h = (rh * (cols.len() + 1) as f32 + 2.0).min(190.0);
        let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        ui.painter().rect_stroke(outer, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
        let inner = outer.shrink(1.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("gb-aggs"));
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        let mut changed = false;
        egui::ScrollArea::vertical().id_salt("gb-aggs").auto_shrink([false, false]).show_viewport(&mut child, |ui, vp| {
            let w = inner.width();
            ui.set_min_size(vec2(w, rh * (cols.len() + 1) as f32));
            let o = ui.min_rect().min;
            let name_w = w - fw * 5.0;
            let pt = ui.painter().clone();
            let first = ((vp.min.y - rh).max(0.0) / rh) as usize;
            let last = (((vp.max.y - rh).max(0.0) / rh) as usize + 1).min(cols.len());
            for (i, &c) in cols.iter().enumerate().take(last).skip(first) {
                let y = o.y + rh * (i + 1) as f32;
                let nr = Rect::from_min_size(pos2(o.x, y), vec2(name_w, rh));
                let icon = type_icon(ui, p, pos2(nr.min.x + 8.0, nr.center().y), self.kinds.get(c).copied().unwrap_or(Kind::Text));
                kit::cell_text(ui, &pt, Rect::from_min_max(pos2(icon - 8.0, nr.min.y), nr.max), &self.headers[c], kit::medium(12.0), p.txt_hi, false);
                pt.vline(kit::snap(ui, nr.max.x) - 0.5, nr.y_range(), Stroke::new(1.0, p.brd));
                for k in 0..5 {
                    let cr = Rect::from_min_size(pos2(o.x + name_w + fw * k as f32, y), vec2(fw, rh));
                    pt.vline(kit::snap(ui, cr.max.x) - 0.5, cr.y_range(), Stroke::new(1.0, p.brd));
                    // Sum and average: numbers only.
                    if k < 2 && self.kinds.get(c) != Some(&Kind::Number) { continue; }
                    let b = Rect::from_center_size(pos2(kit::snap(ui, cr.center().x), kit::snap(ui, cr.center().y)), vec2(16.0, 16.0));
                    let resp = ui.interact(b, egui::Id::unique(("gb-agg", c, k)), Sense::click());
                    let on = *fn_flag(&mut self.aggs[c], k);
                    kit::tick_box(&pt, p, b, on, false, resp.hovered(), true);
                    if resp.clicked() { *fn_flag(&mut self.aggs[c], k) = !on; changed = true; }
                }
                pt.hline(o.x..=o.x + w, kit::snap(ui, y + rh) - 0.5, Stroke::new(1.0, p.brd));
            }
            // The head, stuck to the top.
            let hy = o.y + vp.min.y;
            let hr = Rect::from_min_size(pos2(o.x, hy), vec2(w, rh));
            pt.rect_filled(hr, 0.0, p.surf);
            let heads = std::iter::once(("Column", name_w)).chain(FNS.iter().map(|f| (*f, fw)));
            let mut x = o.x;
            for (t, cw) in heads {
                let cr = Rect::from_min_size(pos2(x, hy), vec2(cw, rh));
                kit::cell_text(ui, &pt, cr, t, semibold(12.0), p.txt, false);
                pt.vline(kit::snap(ui, cr.max.x) - 0.5, cr.y_range(), Stroke::new(1.0, p.brd));
                x += cw;
            }
            pt.hline(hr.x_range(), kit::snap(ui, hr.max.y) - 0.5, Stroke::new(1.0, p.brd));
        });
        if changed { self.stale = true; }
    }

    /// `.stat-line`: what the preview found.
    fn stats(&self, ui: &mut Ui, p: &Palette) {
        let text = if self.order.is_empty() {
            "Tick the column(s) to group by.".to_string()
        } else if let Some(pv) = &self.preview {
            let mut s = format!("**{}** {} in {} {}rows", kit::n(pv.groups), if pv.groups == 1 { "group" } else { "groups" }, kit::n(pv.rows), if self.filtered { "filtered " } else { "" });
            if pv.rows < self.ids.len() { s += &format!(" — preview on the first {}; the result covers all {}", kit::n(PREVIEW_ROWS), kit::n(self.ids.len())); }
            s + &format!(" · {} columns in the result", pv.header.len())
        } else {
            "Counting…".to_string()
        };
        stat_line(ui, p, &text);
    }
}

/// `.stat-line`: ui 12 `txt_mut`, line 1.6, `**bold**` in `txt_hi`.
pub(super) fn stat_line(ui: &mut Ui, p: &Palette, text: &str) {
    let mut job = kit::rich_job(text, p.txt_mut, p.txt_hi, 12.0, ui.available_width());
    for s in &mut job.sections { s.format.line_height = Some(19.0); }
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.min.x, kit::snap(ui, r.min.y)), g, p.txt_mut);
}

/// The tick order (`.ord`): a `prim` capsule 16 tall, `on_prim` ui 10.5, right-aligned at `right`.
fn ord_badge(ui: &Ui, p: &Palette, right: egui::Pos2, k: usize) {
    let g = ui.painter().layout_no_wrap(k.to_string(), kit::ui(10.5), p.on_prim);
    let w = (g.size().x + 6.0).max(16.0);
    let r = Rect::from_min_size(pos2(right.x - w, kit::snap(ui, right.y - 8.0)), vec2(w, 16.0));
    ui.painter().rect_filled(r, 8.0, p.prim);
    ui.painter().galley(pos2(kit::snap(ui, r.center().x - g.size().x / 2.0), kit::snap(ui, r.center().y - g.size().y / 2.0)), g, p.on_prim);
}

/// A column's type icon (`.ty`), its left at `at`: returns where the text starts (7 after it).
pub(super) fn type_icon(ui: &Ui, p: &Palette, at: egui::Pos2, kind: Kind) -> f32 {
    let font = FontId::new(10.0, egui::FontFamily::Name(crate::fonts::MONO_BOLD.into()));
    let (glyph, fg, bg) = match kind {
        Kind::Number => ("#", p.cyan_txt, alpha(p.cyan, 0.13)),
        Kind::Date => ("", p.violet_txt, alpha(p.violet, 0.13)),
        Kind::Text => ("Aa", p.txt_dim, alpha(p.gray, 0.16)),
    };
    let g = ui.painter().layout_no_wrap(glyph.to_string(), font, fg);
    let w = (g.size().x + 6.0).max(16.0);
    let r = Rect::from_min_size(pos2(kit::snap(ui, at.x), kit::snap(ui, at.y - 8.0)), vec2(w, 16.0));
    ui.painter().rect_filled(r, 4.0, bg);
    if kind == Kind::Date {
        crate::icons::calendar(ui.painter(), Rect::from_center_size(r.center(), vec2(11.0, 11.0)), 1.8, fg);
    } else {
        ui.painter().galley(pos2(kit::snap(ui, r.center().x - g.size().x / 2.0), kit::snap(ui, r.center().y - g.size().y / 2.0)), g, fg);
    }
    r.max.x + 7.0
}

/// The preview (`#gb-pv`): the result's first lines, the aggregates' titles in
/// `prim`, empty cells "empty" in italic `txt_faint`, "…and N more" under them.
fn preview_table(ui: &mut Ui, p: &Palette, pv: &Preview, nby: usize) {
    let rh = 24.0;
    let font = kit::ui(12.0);
    let more = pv.groups.saturating_sub(pv.lines.len());
    let widths: Vec<f32> = (0..pv.header.len()).map(|c| {
        let mut w = ui.painter().layout_no_wrap(pv.header[c].clone(), semibold(12.0), p.txt).size().x;
        for l in &pv.lines { w = w.max(ui.painter().layout_no_wrap(l.get(c).cloned().unwrap_or_default(), font.clone(), p.txt).size().x); }
        (w + 17.0).clamp(40.0, 220.0)
    }).collect();
    let lines = pv.lines.len() + 1 + usize::from(more > 0);
    let total: f32 = widths.iter().sum();
    let h = (rh * lines as f32 + 2.0).min(220.0) + 10.0;
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(outer).id_salt("gb-pv"));
    egui::ScrollArea::horizontal().id_salt("gb-pv").auto_shrink([false, false]).show(&mut child, |ui| {
        let w = total.max(outer.width());
        let (r, _) = ui.allocate_exact_size(vec2(w, rh * lines as f32 + 2.0), Sense::hover());
        let pt = ui.painter();
        pt.rect_stroke(r, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
        let o = r.min + vec2(1.0, 1.0);
        let scale = (w - 2.0) / total.max(1.0);
        for (i, line) in std::iter::once(&pv.header).chain(&pv.lines).enumerate() {
            let y = o.y + rh * i as f32;
            let mut x = o.x;
            for (c, cw) in widths.iter().enumerate() {
                let cw = cw * scale.max(1.0);
                let cr = Rect::from_min_size(pos2(x, y), vec2(cw, rh));
                let v = line.get(c).map_or("", String::as_str);
                if i == 0 {
                    kit::cell_text(ui, pt, cr, v, semibold(12.0), if c > nby { p.prim } else { p.txt }, false);
                } else if v.is_empty() {
                    italic(ui, pt, cr, "empty", p.txt_faint);
                } else {
                    kit::cell_text(ui, pt, cr, v, font.clone(), p.txt, false);
                }
                if c + 1 < widths.len() { pt.vline(kit::snap(ui, cr.max.x) - 0.5, cr.y_range(), Stroke::new(1.0, p.brd)); }
                x += cw;
            }
            if i + 1 < lines { pt.hline(o.x..=r.max.x - 1.0, kit::snap(ui, y + rh) - 0.5, Stroke::new(1.0, p.brd)); }
        }
        if more > 0 {
            let y = o.y + rh * (lines - 1) as f32;
            let cr = Rect::from_min_size(pos2(o.x, y), vec2(outer.width().min(w) - 2.0, rh));
            pt.text(pos2(cr.min.x + 8.0, cr.center().y), Align2::LEFT_CENTER, format!("…and {} more", kit::n(more)), font.clone(), p.txt_faint);
        }
    });
}

/// A cell's text in italic (an empty value said as such).
pub(super) fn italic(ui: &Ui, pt: &egui::Painter, r: Rect, text: &str, c: Color32) {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: kit::ui(12.0), color: c, italics: true, ..Default::default() });
    let g = pt.layout_job(job);
    pt.galley(pos2(r.min.x + 8.0, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, c);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_leaves_out_grouped_and_unticked_columns() {
        let mut aggs = vec![Aggs::default(); 4];
        aggs[0] = Aggs::SUM;
        aggs[2] = Aggs::SUM;
        *fn_flag(&mut aggs[3], 4) = true;
        let s = spec_of(&[0, 1], &aggs, Match::Slug);
        assert_eq!(s.by, vec![0, 1]);
        assert_eq!(s.aggs.iter().map(|a| a.0).collect::<Vec<_>>(), vec![2, 3]);
        assert!(s.aggs[1].1.distinct && !s.aggs[1].1.sum);
        assert_eq!(s.how, Match::Slug);
    }

    #[test]
    fn flags_map_to_the_dialog_columns() {
        let mut a = Aggs::default();
        for k in 0..5 { *fn_flag(&mut a, k) = true; }
        assert_eq!(a, Aggs { sum: true, avg: true, min: true, max: true, distinct: true });
    }
}

impl Drop for GroupBy {
    /// Its choices kept for the next opening on this tab.
    fn drop(&mut self) {
        if let Ok(mut m) = LAST.lock() {
            m.get_or_insert_with(Default::default).insert(self.uid, (self.headers.len(), self.order.clone(), self.aggs.clone(), self.how));
        }
    }
}
