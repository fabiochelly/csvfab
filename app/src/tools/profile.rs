//! File profile (24-profile.js): every column at a glance, one line each —
//! kind, filled and empty cells, distinct values, min / max (or shortest /
//! longest text), sum and average. Read in the background column by column
//! (the lines fill in as they come), exactly up to SAMPLE rows, then from an
//! evenly spaced sample of them (counts as percentages). Kept per tab until
//! its rows change. A click on a line selects that column in the grid.

use super::analysis_kit::{self as kit, Btn};
use super::{Cmd, Command, Cx, Tool};
use crate::grid::semibold;
use crate::job::Job;
use crate::theme::{alpha, Palette};
use csvfab_engine::group::fr_num;
use csvfab_engine::stats::{self, ColProfile};
use csvfab_engine::view::Kind;
use csvfab_engine::{kinds, RowId, State};
use eframe::egui::{self, pos2, text::LayoutJob, vec2, Color32, Rect, Sense, Stroke, TextFormat, Ui};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

/// Past this many rows, an evenly spaced sample of them is read.
const SAMPLE: usize = 1_000_000;
/// Columns profiled together in one pass over the rows.
const GROUP: usize = 4;

#[derive(Clone)]
struct Done {
    version: u64,
    ncols: usize,
    cols: Arc<Vec<ColProfile>>,
    mojibake: bool,
}

thread_local! {
    /// The last profile of each tab: reopened at once while its rows are the same.
    static CACHE: RefCell<HashMap<u64, Done>> = RefCell::default();
}

struct Profile {
    job: Option<Job<Option<bool>>>,
    /// Columns read so far (the job fills them in).
    parts: Arc<Mutex<Vec<Option<ColProfile>>>>,
    done: Option<Done>,
    rows: usize,
    sampled: bool,
    headers: Vec<String>,
    version: u64,
}

/// The rows read: all of them, or SAMPLE evenly spaced.
fn sample(st: &State) -> (Vec<RowId>, bool) {
    let n = st.len();
    if n <= SAMPLE { return ((0..n).map(|p| st.data_id(p)).collect(), false); }
    let step = n as f64 / SAMPLE as f64;
    ((0..SAMPLE).map(|i| st.data_id((i as f64 * step) as usize)).collect(), true)
}

impl Profile {
    fn new(cx: &Cx) -> Profile {
        let t = cx.tab.table.as_ref().unwrap();
        let st = t.st.clone();
        let version = t.version();
        let headers = st.headers();
        let cached = CACHE.with(|c| c.borrow().get(&cx.tab.uid).cloned()).filter(|d| d.version == version && d.ncols == st.ncols);
        let parts = Arc::new(Mutex::new(vec![None; st.ncols]));
        let (rows, sampled) = (st.len().min(SAMPLE), st.len() > SAMPLE);
        let job = cached.is_none().then(|| {
            let parts = parts.clone();
            Job::spawn(cx.ctx, move |cancel, progress| {
                let (ids, _) = sample(&st);
                let ctx_mojibake = kinds::file_mojibake(&st);
                // A few columns a pass: each record read once for them (the pass is bound
                // by reading records), their distinct values held together (memory).
                let cols: Vec<usize> = (0..st.ncols).collect();
                for group in cols.chunks(GROUP) {
                    let ps = stats::profile_cols(&st, &ids, group, cancel)?;
                    let mut parts = parts.lock().unwrap();
                    for (&c, p) in group.iter().zip(ps) { parts[c] = Some(p); }
                    progress.fetch_add(group.len(), Ordering::Relaxed);
                }
                Some(ctx_mojibake)
            })
        });
        Profile { job, parts, done: cached, rows, sampled, headers, version }
    }

    /// The table as tab-separated text.
    fn tsv(&self, cols: &[Option<ColProfile>]) -> String {
        let mut lines = vec!["Column\tKind\tFilled\tEmpty\tDistinct\tMin\tMax\tSum\tAverage".to_string()];
        for (h, s) in self.headers.iter().zip(cols) {
            let Some(s) = s else { continue };
            let k = s.kind();
            let typed = matches!(k, Some(Kind::Number | Kind::Date));
            let lo = if typed { s.min.as_ref().map(|m| m.1.clone()) } else { s.short.clone() }.unwrap_or_default();
            let hi = if typed { s.max.as_ref().map(|m| m.1.clone()) } else { s.long.clone() }.unwrap_or_default();
            let num = k == Some(Kind::Number) && s.cnt > 0;
            lines.push([h.clone(), kind_label(k).0.into(), s.filled.to_string(), s.empty.to_string(), s.distinct.to_string(), lo, hi,
                if num { csvfab_engine::group::js_num(s.sum) } else { String::new() }, if num { csvfab_engine::group::js_num(s.sum / s.cnt as f64) } else { String::new() }].join("\t"));
        }
        if self.sampled { lines.push(format!("(sample of {} rows)", self.rows)); }
        lines.join("\n")
    }
}

/// A kind's badge (`.cp-type`): its label and colours (text, fill, border).
fn kind_label(k: Option<Kind>) -> (&'static str, fn(&Palette) -> (Color32, Color32, Color32)) {
    match k {
        Some(Kind::Number) => ("Numbers", |p| (p.ok_txt, alpha(p.ok_rgb, 0.13), alpha(p.ok_rgb, 0.4))),
        Some(Kind::Date) => ("Dates", |p| (p.accent_2, alpha(p.accent, 0.13), alpha(p.accent, 0.4))),
        Some(Kind::Text) => ("Text", |p| (p.violet_txt, alpha(p.violet, 0.13), alpha(p.violet, 0.4))),
        None => ("Empty", |p| (p.violet_txt, alpha(p.violet, 0.13), alpha(p.violet, 0.4))),
    }
}

/// The summary line (`#pf-sum`): rows, columns, size, encoding, delimiter, line ends, problems.
fn summary(st: &State, mojibake: bool, p: &Palette, width: f32) -> LayoutJob {
    let b = &st.base;
    let mut job = LayoutJob::default();
    job.wrap.max_width = width;
    let f = |c: Color32, bold: bool| TextFormat { font_id: if bold { semibold(12.0) } else { kit::ui(12.0) }, color: c, line_height: Some(19.0), ..Default::default() };
    let sep = |job: &mut LayoutJob| job.append(" · ", 0.0, f(p.txt_mut, false));
    job.append(&kit::n(st.len()), 0.0, f(p.txt_hi, true));
    job.append(" rows", 0.0, f(p.txt_mut, false));
    sep(&mut job);
    job.append(&kit::n(st.ncols), 0.0, f(p.txt_hi, true));
    job.append(" columns", 0.0, f(p.txt_mut, false));
    let delim = match b.dialect.delim { b';' => "semicolon".to_string(), b',' => "comma".into(), b'\t' => "tab".into(), b'|' => "pipe".into(), d => (d as char).to_string() };
    let eol = match b.eol { b"\r\n" => "CRLF", b"\r" => "CR", _ => "LF" };
    let mut parts = vec![size(b.size), format!("{}{}", b.enc.label(), if b.bom { " BOM" } else { "" }), delim, eol.into()];
    if !st.header { parts.push("no header line".into()); }
    for s in parts { sep(&mut job); job.append(&s, 0.0, f(p.txt_mut, false)); }
    let mut warn = Vec::new();
    if !b.odd.is_empty() { warn.push(kit::plural(b.odd.len(), "irregular row", "irregular rows")); }
    if b.qerr > 0 { warn.push(kit::plural(b.qerr as usize, "quote error", "quote errors")); }
    if mojibake { warn.push("garbled accents".into()); }
    for s in warn { sep(&mut job); job.append(&s, 0.0, f(p.warn, false)); }
    job
}

/// 1 234 567 bytes → "1.2 MB" (the web app's fmtBytes).
fn size(b: u64) -> String {
    let b = b as f64;
    if b < 1024.0 { format!("{b} B") } else if b < 1024.0 * 1024.0 { format!("{:.1} KB", b / 1024.0) } else if b < 1024f64.powi(3) { format!("{:.1} MB", b / 1024f64.powi(2)) } else { format!("{:.2} GB", b / 1024f64.powi(3)) }
}

/// Text cut to n characters with an ellipsis (`cutTxt`).
fn cut(v: &str, n: usize) -> String {
    if v.chars().count() > n { v.chars().take(n - 1).collect::<String>() + "…" } else { v.to_string() }
}

/// (title, width) of the table's columns.
const COLS: [(&str, f32); 8] = [("Column", 190.0), ("Kind", 86.0), ("Filled", 80.0), ("Empty", 80.0), ("Distinct", 116.0), ("Min", 170.0), ("Max", 170.0), ("Sum · avg", 150.0)];
const ROW_H: f32 = 26.0;
const HEAD_H: f32 = 28.0;

impl Tool for Profile {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(t) = cx.tab.table.as_ref() else { return false };
        // The rows changed under it (an edit, an undo): read again.
        if t.version() != self.version || t.st.ncols != self.headers.len() { *self = Profile::new(cx); }
        if let Some(j) = &mut self.job {
            if let Some(r) = j.poll() {
                self.job = None;
                if let Some(moji) = r {
                    let cols: Vec<ColProfile> = self.parts.lock().unwrap().iter().map(|c| c.clone().unwrap_or_default()).collect();
                    let d = Done { version: self.version, ncols: cols.len(), cols: Arc::new(cols), mojibake: moji };
                    CACHE.with(|c| c.borrow_mut().insert(cx.tab.uid, d.clone()));
                    self.done = Some(d);
                }
            }
        }
        let cols: Vec<Option<ColProfile>> = match &self.done {
            Some(d) => d.cols.iter().cloned().map(Some).collect(),
            None => self.parts.lock().unwrap().clone(),
        };
        let p = cx.p;
        let screen = cx.ctx.content_rect();
        let size = vec2(1040f32.min(screen.width() - 40.0), screen.height() * 0.94);
        let mut close = false;
        let mut go = None;
        let mut copy = false;
        let read = self.job.as_ref().map(|j| j.progress.load(Ordering::Relaxed));
        let mojibake = self.done.as_ref().is_some_and(|d| d.mojibake);
        let (_, open) = kit::modal_fixed(cx, "profile", size, (20, 20, 20), |ui, cx| {
            kit::title(ui, p, "File profile");
            let st = cx.tab.st().unwrap();
            let g = ui.painter().layout_job(summary(st, mojibake, p, ui.available_width()));
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, p.txt_mut);
            ui.add_space(10.0);
            let note_h = if self.sampled || read.is_some() { 29.0 } else { 0.0 };
            let table_h = (ui.available_height() - note_h - 16.0 - 30.0).max(0.0);
            go = self.table(ui, p, &cols, table_h);
            if let Some(k) = read {
                ui.add_space(4.0);
                kit::progress(ui, p, Some(k as f32 / self.headers.len().max(1) as f32), &format!("Profiling… {} of {} columns", k, self.headers.len()));
            } else if self.sampled {
                ui.add_space(10.0);
                kit::note(ui, p, &format!("Read from an evenly spaced sample of {} rows: filled and empty as percentages, distinct values within the sample.", kit::n(self.rows)));
            }
            kit::actions(ui, 16.0, |ui| {
                if kit::button(ui, p, "Close", Btn::Primary, true).clicked() { close = true; }
                if kit::button(ui, p, "Copy as text", Btn::Outline, read.is_none()).clicked() { copy = true; }
            });
        });
        if copy {
            cx.ctx.copy_text(self.tsv(&cols));
            cx.toast(format!("Profile copied — {} columns.", kit::n(self.headers.len())));
        }
        if let Some(c) = go {
            // Its column in the grid, its panel open (as the web's).
            let row = cx.tab.grid.cursor.map_or(0, |k| k.row);
            cx.tab.grid.select(crate::grid::Cell { row, col: c }, false);
            cx.tab.grid.reveal_col = true;
            cx.tab.grid.open_panel = Some(c);
            return false;
        }
        open && !close
    }
}

impl Profile {
    /// The table (`.pv.rv.pf`): a line per column, the head stuck on top; returns the column clicked.
    fn table(&self, ui: &mut Ui, p: &Palette, cols: &[Option<ColProfile>], h: f32) -> Option<usize> {
        let w = ui.available_width();
        let (outer, _) = ui.allocate_exact_size(vec2(w, h.max(120.0)), Sense::hover());
        ui.painter().rect_stroke(outer, 6.0, Stroke::new(1.0, p.brd), egui::StrokeKind::Inside);
        let inner = outer.shrink(1.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        child.set_clip_rect(inner);
        let total: f32 = COLS.iter().map(|c| c.1).sum();
        // The last column takes the room left.
        let widths: Vec<f32> = COLS.iter().enumerate().map(|(k, c)| if k + 1 == COLS.len() { c.1.max(inner.width() - (total - c.1)) } else { c.1 }).collect();
        let full: f32 = widths.iter().sum();
        let rows = self.rows.max(1) as f64;
        let mut hit = None;
        let n = self.headers.len();
        egui::ScrollArea::both().id_salt("pf").auto_shrink([false, false]).show_viewport(&mut child, |ui, vp| {
            ui.set_min_size(vec2(full, HEAD_H + n as f32 * ROW_H));
            let o = ui.min_rect().min;
            let pt = ui.painter().clone();
            let first = ((vp.min.y - HEAD_H).max(0.0) / ROW_H) as usize;
            let last = (((vp.max.y - HEAD_H).max(0.0) / ROW_H) as usize + 1).min(n);
            let sampled = self.sampled;
            let count = |k: u64| if sampled {
                let x = k as f64 / rows * 100.0;
                format!("{} %", if k > 0 && x < 1.0 { format!("{x:.1}") } else { format!("{x:.0}") })
            } else { kit::n(k as usize) };
            let num = |x: f64| fr_num(x, 0, 2);
            for i in first..last {
                let y = o.y + HEAD_H + i as f32 * ROW_H;
                let rr = Rect::from_min_size(pos2(o.x, y), vec2(full, ROW_H));
                let resp = ui.interact(rr, ui.scope_id().with(("pf-row", i)), Sense::click());
                if resp.hovered() { pt.rect_filled(rr, 0.0, alpha(p.hi, 0.06)); }
                if resp.clicked() { hit = Some(i); }
                let mut x = o.x;
                let cell = |k: usize, x: f32| Rect::from_min_size(pos2(x, y), vec2(widths[k], ROW_H));
                let s = &cols[i];
                // Column: its name, a 2 px bar of its filled share at the bottom.
                let c0 = cell(0, x);
                if let Some(s) = s {
                    let f = (s.filled as f64 / rows).min(1.0) as f32;
                    pt.rect_filled(Rect::from_min_size(pos2(c0.min.x, c0.max.y - 1.0 - 2.0), vec2(c0.width() * f, 2.0)), 0.0, alpha(p.prim, 0.6));
                }
                kit::cell_text(ui, &pt, c0, &self.headers[i], kit::medium(12.0), p.txt_hi, false);
                let mut texts: Vec<(String, Color32)> = Vec::new();
                match s {
                    None => texts.push(("…".into(), p.txt_faint)),
                    Some(s) => {
                        let k = s.kind();
                        texts.push((String::new(), p.txt));
                        texts.push((count(s.filled), p.txt_sub));
                        texts.push((count(s.empty), if s.empty > 0 { p.txt_sub } else { p.txt_faint }));
                        texts.push((if s.filled == 0 { String::new() } else { kit::n(s.distinct as usize) }, p.txt_sub));
                        let typed = matches!(k, Some(Kind::Number | Kind::Date)) && s.min.is_some();
                        let (lo, hi) = if typed {
                            (s.min.as_ref().map(|m| m.1.clone()), s.max.as_ref().map(|m| m.1.clone()))
                        } else if s.filled > 0 {
                            (s.short.as_ref().map(|v| format!("{} ({})", cut(v, 28), kit::n(kinds::utf16_len(v)))), s.long.as_ref().map(|v| format!("{} ({})", cut(v, 28), kit::n(kinds::utf16_len(v)))))
                        } else { (None, None) };
                        for v in [lo, hi] { texts.push(v.map_or(("—".into(), p.txt_faint), |v| (cut(&v, 40), p.txt))); }
                        texts.push((if k == Some(Kind::Number) && s.cnt > 0 { format!("{} · {}", num(s.sum), num(s.sum / s.cnt as f64)) } else { String::new() }, p.txt_sub));
                    }
                }
                x += widths[0];
                pt.vline(kit::snap(ui, c0.max.x) - 0.5, c0.y_range(), Stroke::new(1.0, p.brd));
                for (k, (text, color)) in texts.into_iter().enumerate() {
                    let c = cell(k + 1, x);
                    if k == 0 {
                        if let Some(s) = s { badge(ui, &pt, p, c, s.kind()); }
                    } else if k == 3 && s.as_ref().is_some_and(|s| s.unique() && !sampled) {
                        // "unique": every filled cell different, a key?
                        let g = pt.layout_no_wrap(text.clone(), kit::ui(12.0), color);
                        let gx = c.min.x + 8.0 + g.size().x + 5.0;
                        kit::cell_text(ui, &pt, c, &text, kit::ui(12.0), color, false);
                        pt.text(pos2(gx, c.center().y), egui::Align2::LEFT_CENTER, "unique", kit::ui(12.0), p.txt_faint);
                    } else {
                        kit::cell_text(ui, &pt, c, &text, kit::ui(12.0), color, false);
                    }
                    pt.vline(kit::snap(ui, c.max.x) - 0.5, c.y_range(), Stroke::new(1.0, p.brd));
                    x += widths[k + 1];
                }
                pt.hline(rr.x_range(), kit::snap(ui, rr.max.y) - 0.5, Stroke::new(1.0, p.brd));
            }
            // The head (`tr.sec`), stuck on top: ui 600 12 `txt_mut`, padding 6 × 8, on `surf`.
            let hy = o.y + vp.min.y;
            let hr = Rect::from_min_size(pos2(o.x, hy), vec2(full, HEAD_H));
            pt.rect_filled(hr, 0.0, p.surf);
            let mut x = o.x;
            for (k, (t, _)) in COLS.iter().enumerate() {
                let c = Rect::from_min_size(pos2(x, hy), vec2(widths[k], HEAD_H));
                kit::cell_text(ui, &pt, c, t, semibold(12.0), p.txt_mut, false);
                pt.vline(kit::snap(ui, c.max.x) - 0.5, c.y_range(), Stroke::new(1.0, p.brd));
                x += widths[k];
            }
            pt.hline(hr.x_range(), kit::snap(ui, hr.max.y) - 0.5, Stroke::new(1.0, p.brd));
        });
        hit
    }
}

/// The kind's badge: padding 0 8, radius 9, ui 600 11, 18 tall, 1 px border.
fn badge(ui: &Ui, pt: &egui::Painter, p: &Palette, c: Rect, k: Option<Kind>) {
    let (label, colors) = kind_label(k);
    let (fg, fill, brd) = colors(p);
    let g = pt.layout_no_wrap(label.to_string(), semibold(11.0), fg);
    let r = Rect::from_min_size(pos2(c.min.x + 8.0, kit::snap(ui, c.center().y - 9.0)), vec2(g.size().x + 16.0, 18.0));
    pt.rect(r, 9.0, fill, Stroke::new(1.0, brd), egui::StrokeKind::Inside);
    pt.galley(pos2(r.min.x + 8.0, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, fg);
}

pub fn commands() -> Vec<Command> {
    vec![Command { name: "File profile…", group: "View", keys: None, enabled: super::has_table, run: |cx| { let t = Profile::new(cx); cx.out.push(Cmd::Tool(Box::new(t))); } }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use csvfab_engine::{Base, OpenOptions, Table};

    #[test]
    fn sample_and_text() {
        let t = Table::new(Arc::new(Base::from_bytes(b"A,B\n1,x\n2,yy\n,zzz\n".to_vec(), OpenOptions::default())), None);
        let (ids, sampled) = sample(&t.st);
        assert_eq!((ids.len(), sampled), (3, false));
        assert_eq!(cut("abcdef", 4), "abc…");
        assert_eq!(size(2048), "2.0 KB");
        let cols: Vec<Option<ColProfile>> = (0..2).map(|c| stats::profile_col(&t.st, &ids, c, &Default::default())).collect();
        let pf = Profile { job: None, parts: Default::default(), done: None, rows: 3, sampled: false, headers: t.st.headers(), version: 0 };
        let tsv = pf.tsv(&cols);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(lines[1], "A\tNumbers\t2\t1\t2\t1\t2\t3\t1.5");
        assert_eq!(lines[2], "B\tText\t3\t0\t3\tx\tzzz\t\t");
    }
}
