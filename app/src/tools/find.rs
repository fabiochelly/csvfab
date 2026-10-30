//! The find & replace bar (Ctrl+F), under the top band: finds in the rows shown
//! without filtering them (the grid highlights `tab.find.text`), steps through
//! the matches (‹ ›, Enter / Shift+Enter, F3 / Shift+F3), replaces in the rows
//! shown. The matches are listed by a background job, in view order, whenever
//! the query, the column, the rows shown or the data change.

use super::edit_kit::{self as form, n, Btn, Snap};
use super::{has_table, Command, Cx};
use crate::app::App;
use crate::grid::Cell;
use crate::icons::Icon;
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{Metrics, Palette};
use csvfab_engine::edit::replace::{replace_done, Replace, ReplaceSpec};
use csvfab_engine::edit::{self, CellOp, Change};
use eframe::egui::{self, pos2, vec2, Id, Key, Modifiers, Sense, Stroke, Ui};
use rayon::prelude::*;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// Past this many matches the list stops growing (the count goes on).
const LIST_MAX: usize = 2_000_000;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Find & replace", group: "Edit", keys: Some("Ctrl+F"), enabled: has_table, run: open }]
}

/// Ctrl+F: the bar opens on its find field — seeded with the active cell's
/// value when the field is empty and one cell is selected.
fn open(cx: &mut Cx) {
    let tab = &mut *cx.tab;
    if tab.find.text.is_empty() {
        if let (Some(c), Some(st)) = (tab.grid.cursor, tab.st()) {
            if tab.grid.anchor.is_none_or(|a| a == c) && c.row < tab.shown() {
                let v = st.cell(tab.shown_id(c.row), c.col);
                let v = v.trim();
                if !v.is_empty() && v.chars().count() <= 80 && !v.contains('\n') {
                    tab.find.text = v.to_string();
                }
            }
        }
    }
    tab.find.open = true;
    tab.find.focus = true;
}

/// What the matches were listed for.
#[derive(Clone, PartialEq)]
struct Listed {
    spec: ReplaceSpec,
    col: Option<usize>,
    generation: u64,
    shown: usize,
    /// Columns hidden in the grid: not searched (as the web's, which searched what it showed).
    hidden: Vec<usize>,
}

/// The bar's work for one tab (the tab's `Find` holds what the grid needs).
#[derive(Default)]
struct Work {
    key: Option<Listed>,
    count: Option<Job<Option<Vec<(usize, usize)>>>>,
    /// Matches counted so far (the list stops at LIST_MAX).
    found: Arc<AtomicUsize>,
    total: usize,
    error: Option<String>,
    /// A step asked for while counting: done once the list is in.
    pending: Option<i32>,
    replace: Option<Job<Option<Vec<Change>>>>,
    done: String,
}

thread_local! {
    static WORK: RefCell<HashMap<u64, Work>> = RefCell::new(HashMap::new());
}

/// Draws the bar when the active tab's `find.open` is set; F3 / Shift+F3 step from anywhere.
pub fn bar(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    // Drawn every frame before the tools: the look-up's list of the other tabs is kept here.
    super::lookup::publish(&app.tabs);
    let (p, m) = (app.theme.p.clone(), app.m.clone());
    let free = app.dialog.is_none() && app.palette.is_none();
    let Some(tab) = app.tabs.get_mut(app.active) else { return };
    if tab.table.is_none() {
        return;
    }
    if let Some(q) = crate::shot::take("find") {
        tab.find.open = true;
        tab.find.text = if q.is_empty() { "Paris".into() } else { q };
    }
    // `findre:<pattern>`: the same in Regex mode.
    if let Some(q) = crate::shot::take("findre") {
        tab.find.open = true;
        tab.find.regex = true;
        tab.find.text = q;
    }
    // Screenshot hook: `tool:bulk`, `tool:clean`… opens that dialog.
    if let Some(name) = crate::shot::take("tool") {
        if let Some(t) = super::edit_kit::by_name(&name, tab) {
            app.tools.push((tab.uid, t));
        }
        return;
    }
    let mut toast = None;
    let mut bad: Option<(u64, String)> = None;
    WORK.with_borrow_mut(|all| {
        let w = all.entry(tab.uid).or_default();
        if free {
            let back = ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::F3));
            let fwd = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F3));
            if back || fwd {
                tab.find.open = true;
                if tab.find.text.is_empty() { tab.find.focus = true } else { w.pending = Some(if back { -1 } else { 1 }) }
            }
        }
        if !tab.find.open {
            if !tab.find.matches.is_empty() || w.key.is_some() {
                tab.find.matches = Arc::default();
                tab.find.current = None;
                *w = Work::default();
            }
            return;
        }
        recount(tab, w, &ctx);
        if let Some(r) = w.count.as_mut().and_then(|j| j.poll()) {
            w.count = None;
            if let Some(list) = r {
                w.total = w.found.load(Ordering::Relaxed);
                tab.find.matches = Arc::new(list);
            }
        }
        if w.count.is_none() {
            if let Some(dir) = w.pending.take() {
                if let Some(s) = step(tab, dir, &ctx, &m) { w.done = s.into(); }
            }
        }
        if let Some(r) = w.replace.as_mut().and_then(|j| j.poll()) {
            w.replace = None;
            if let Some(ch) = r {
                let k = ch.len();
                tab.apply_changes(&ch, &ctx);
                w.done = replace_done(k);
                toast = Some(w.done.clone());
            }
        }
        show(ui, tab, w, &p, &m, &ctx);
        // What was typed this frame is counted now, not a frame later.
        recount(tab, w, &ctx);
        // A pattern that does not compile: why, in the status line (as the web's).
        if let Some(e) = &w.error { bad = Some((tab.uid, e.clone())); }
        if w.count.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    });
    if let Some(t) = toast {
        app.toast(t, &ctx);
    }
    if let Some((uid, e)) = bad {
        if app.status.as_ref().is_none_or(|s| s.1 != e) { app.status = Some((uid, e, ctx.input(|i| i.time))); }
    }
}

fn spec(tab: &Tab) -> ReplaceSpec {
    ReplaceSpec { find: tab.find.text.clone(), replace: tab.find.replace.clone(), regex: tab.find.regex, match_case: tab.find.match_case }
}

/// Lists the matches again when what they depend on changed.
fn recount(tab: &mut Tab, w: &mut Work, ctx: &egui::Context) {
    let mut s = spec(tab);
    s.replace.clear();
    let key = Listed { spec: s, col: tab.find.col, generation: tab.generation, shown: tab.shown(), hidden: tab.grid.hidden.iter().copied().collect() };
    if w.key.as_ref() == Some(&key) {
        return;
    }
    w.key = Some(key.clone());
    w.count = None;
    w.error = None;
    w.total = 0;
    tab.find.matches = Arc::default();
    tab.find.current = None;
    let Some(snap) = Snap::of(tab) else { return };
    let op = match Replace::new(&key.spec, &snap.st) {
        Ok(Some(op)) => op,
        Ok(None) => return,
        Err(e) => {
            w.error = Some(e);
            return;
        }
    };
    let found = Arc::new(AtomicUsize::new(0));
    w.found = found.clone();
    let (col, hidden) = (key.col, key.hidden.clone());
    w.count = Some(Job::spawn(ctx, move |cancel, _| find_all(&snap, &op, col, &hidden, &found, cancel)));
}

/// Every (position in the view, column) whose cell holds a match, in order.
fn find_all(snap: &Snap, op: &Replace, col: Option<usize>, hidden: &[usize], found: &AtomicUsize, cancel: &AtomicBool) -> Option<Vec<(usize, usize)>> {
    let st = &snap.st;
    let rows = snap.rows();
    let ncols = st.ncols;
    let parts: Vec<Option<Vec<(usize, usize)>>> = (0..rows.len().div_ceil(4096))
        .into_par_iter()
        .map(|k| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let (mut out, mut tmp) = (Vec::new(), Vec::new());
            for pos in k * 4096..((k + 1) * 4096).min(rows.len()) {
                let id = rows[pos];
                if !op.row_may_change(st, id, &mut ()) {
                    continue;
                }
                let mut hit = |c: usize, v: &str| {
                    if c < ncols && !hidden.contains(&c) && !v.is_empty() && matches!(op.apply(v), Cow::Owned(_)) {
                        out.push((pos, c));
                    }
                };
                match col {
                    Some(c) => st.with_cell(id, c, &mut tmp, |v| hit(c, v)),
                    None => st.visit(id, &mut hit),
                }
            }
            found.fetch_add(out.len(), Ordering::Relaxed);
            Some(out)
        })
        .collect();
    let mut list = Vec::new();
    for p in parts {
        let p = p?;
        let room = LIST_MAX - list.len();
        list.extend(p.into_iter().take(room));
    }
    Some(list)
}

/// Moves the active cell to the next match after it (`dir` −1: before it), wrapping around.
/// Returns what to say when it went round the end of the rows shown (as the web's).
fn step(tab: &mut Tab, dir: i32, ctx: &egui::Context, m: &Metrics) -> Option<&'static str> {
    let list = tab.find.matches.clone();
    if list.is_empty() {
        return None;
    }
    let at = tab.grid.cursor.map(|c| (c.row, c.col));
    // No active cell: from the top of the rows in view (backwards, their bottom), as the web's.
    let k = match at {
        Some(_) => next(&list, at, dir),
        None if dir > 0 => list.partition_point(|&x| x < (tab.grid.top, 0)) % list.len(),
        None => list.partition_point(|&x| x <= (tab.grid.top + tab.grid.page.max(1) - 1, usize::MAX)).checked_sub(1).unwrap_or(list.len() - 1),
    };
    // Round the end: past the last match forwards, before the first backwards.
    let wrapped = at.and_then(|a| match dir > 0 {
        true if list.partition_point(|&x| x <= a) == list.len() => Some("Wrapped to the top of the rows shown"),
        false if list.partition_point(|&x| x < a) == 0 => Some("Wrapped to the bottom of the rows shown"),
        _ => None,
    });
    let (row, col) = list[k];
    tab.find.current = Some(k);
    tab.grid.select(Cell { row, col }, false);
    // A row out of sight lands in the middle of the view.
    let page = tab.grid.page.max(1);
    if row < tab.grid.top || row + 1 >= tab.grid.top + page {
        tab.grid.set_top(row.saturating_sub(page / 2));
    }
    let x0: f32 = tab.grid.widths.iter().take(col).sum();
    let x1 = x0 + tab.grid.widths.get(col).copied().unwrap_or(0.0);
    let view_w = ctx.content_rect().width() - m.gutter_w - m.strip_w;
    if x0 < tab.grid.xoff {
        tab.grid.xoff = x0;
    } else if x1 > tab.grid.xoff + view_w {
        tab.grid.xoff = (x1 - view_w).min(x0);
    }
    ctx.request_repaint();
    wrapped
}

/// The match after the cell `at` (`dir` −1: before it), wrapping around; from the
/// top (the bottom) without an active cell. `list` is sorted and not empty.
fn next(list: &[(usize, usize)], at: Option<(usize, usize)>, dir: i32) -> usize {
    match at {
        Some(at) if dir > 0 => list.partition_point(|&x| x <= at) % list.len(),
        Some(at) => list.partition_point(|&x| x < at).checked_sub(1).unwrap_or(list.len() - 1),
        None if dir > 0 => 0,
        None => list.len() - 1,
    }
}

/// "3 / 128" when the active cell is on a match, else the count; true: in `warn`.
fn position(tab: &Tab, w: &Work) -> (String, bool) {
    if tab.find.text.is_empty() {
        return (String::new(), false);
    }
    if w.error.is_some() {
        return ("invalid".into(), true);
    }
    if w.count.is_some() {
        return (format!("{}…", n(w.found.load(Ordering::Relaxed))), false);
    }
    let total = w.total;
    if total == 0 {
        return ("No match".into(), true);
    }
    let list = &tab.find.matches;
    if let Some(c) = tab.grid.cursor {
        if let Ok(k) = list.binary_search(&(c.row, c.col)) {
            return (format!("{} / {}", n(k + 1), n(total)), false);
        }
    }
    (format!("{}{} match{}", n(total), if total > list.len() { "+" } else { "" }, if total == 1 { "" } else { "es" }), false)
}

fn show(ui: &mut Ui, tab: &mut Tab, w: &mut Work, p: &Palette, m: &Metrics, ctx: &egui::Context) {
    let frame = egui::Frame::new().fill(p.bar_2).inner_margin(egui::Margin::symmetric(20, 0));
    egui::Panel::top("find-bar").exact_size(51.0).frame(frame).show_separator_line(false).show(ui, |ui| {
        let bar = ui.max_rect();
        ui.painter().hline(bar.x_range().expand(20.0), bar.max.y - 0.5, Stroke::new(1.0, p.brd));
        let mut changed = false;
        let mut close = false;
        let mut go: Option<i32> = None;
        let mut apply = false;
        ui.allocate_ui_with_layout(vec2(bar.width(), 50.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = vec2(15.0, 0.0);
            // "Find & replace in the rows shown:"
            let mut job = egui::text::LayoutJob::default();
            let f = |font| egui::TextFormat { font_id: font, color: p.txt, ..Default::default() };
            job.append("Find & replace in the ", 0.0, f(form::ui_font(13.0)));
            job.append("rows shown", 0.0, f(form::semibold(13.0)));
            job.append(":", 0.0, f(form::ui_font(13.0)));
            let g = ui.painter().layout_job(job);
            let (r, _) = ui.allocate_exact_size(g.size(), Sense::hover());
            ui.painter().galley(r.min, g, p.txt);

            let fid = Id::unique(("find-field", tab.uid));
            let fr = form::field(ui, p, fid, &mut tab.find.text, "Find text...", 240.0, w.error.is_some());
            if std::mem::take(&mut tab.find.focus) {
                fr.request_focus();
                if let Some(mut s) = egui::TextEdit::load_state(ctx, fid) {
                    let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(tab.find.text.chars().count()));
                    s.cursor.set_char_range(Some(all));
                    s.store(ctx, fid);
                }
            }
            if let Some(e) = &w.error {
                fr.clone().on_hover_text(e.as_str());
            }
            changed |= fr.changed();
            if fr.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = Some(if ui.input(|i| i.modifiers.shift) { -1 } else { 1 });
                fr.request_focus();
            }
            // ‹ n / N ›: 7 px after the field (its margin −8), 2 apart.
            ui.add_space(-8.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            if arrow(ui, p, "‹").on_hover_text("Shift+F3").clicked() {
                go = Some(-1);
            }
            let (pos, none) = position(tab, w);
            let col = if none { p.warn } else { p.txt_mut };
            let g = ui.painter().layout_no_wrap(pos, form::ui_font(12.0), col);
            let (r, _) = ui.allocate_exact_size(vec2(g.size().x.max(56.0), 26.0), Sense::hover());
            ui.painter().galley(pos2((r.center().x - g.size().x / 2.0).round(), (r.center().y - g.size().y / 2.0).round()), g, col);
            if arrow(ui, p, "›").on_hover_text("Next match (Enter, F3)").clicked() {
                go = Some(1);
            }
            ui.spacing_mut().item_spacing.x = 15.0;
            ui.add_space(13.0);

            let rr = form::field(ui, p, Id::unique(("replace-field", tab.uid)), &mut tab.find.replace, "Replace with...", 240.0, false);
            let mut opts = vec!["All columns".to_string()];
            opts.extend(form::headers(tab));
            let mut sel = tab.find.col.map_or(0, |c| c + 1);
            if form::select(ui, p, m, Id::unique(("find-col", tab.uid)), &mut sel, &opts, 220.0) {
                tab.find.col = sel.checked_sub(1);
            }
            form::checkbox_with(ui, p, &mut tab.find.regex, "Regex", p.txt, 6.0).on_hover_text("$1, $2… in the replacement");
            form::checkbox_with(ui, p, &mut tab.find.match_case, "Match case", p.txt, 6.0);
            let can = !tab.find.text.is_empty() && w.error.is_none() && w.replace.is_none();
            apply = form::button_icon(ui, p, Btn::Warn, Some(Icon::Bolt), "Apply", can).clicked();
            // A pattern that does not compile: why, where the count of the last replace goes.
            let (note, col) = match &w.error {
                Some(e) => (e.as_str(), p.warn),
                None => (w.done.as_str(), p.txt_mut),
            };
            if !note.is_empty() {
                let g = form::line(ui, note, form::ui_font(12.0), col, ui.available_width());
                let (r, resp) = ui.allocate_exact_size(g.size(), Sense::hover());
                let cut = g.elided;
                ui.painter().galley(r.min, g, col);
                if cut { resp.on_hover_text(note); }
            }
            if (fr.has_focus() || rr.has_focus()) && ui.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
        });
        if changed {
            w.done.clear();
        }
        if let Some(d) = go {
            if w.count.is_some() { w.pending = Some(d) } else if let Some(s) = step(tab, d, ctx, m) { w.done = s.into(); } else { w.done.clear(); }
        }
        if apply {
            start_replace(tab, w, ctx);
        }
        if close {
            tab.find.open = false;
        }
    });
}

/// `‹` / `›`: 24 × 26, outline, radius 5, ui 18.
fn arrow(ui: &mut Ui, p: &Palette, t: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(24.0, 26.0), Sense::click());
    let hot = resp.hovered();
    ui.painter().rect(r, 5.0, if hot { p.surf_2 } else { egui::Color32::TRANSPARENT }, Stroke::new(1.0, if hot { p.brd_2 } else { p.brd }), egui::StrokeKind::Inside);
    let g = ui.painter().layout_no_wrap(t.to_string(), form::ui_font(18.0), p.txt);
    ui.painter().galley(pos2((r.center().x - g.size().x / 2.0).round(), (r.center().y - 1.0 - g.size().y / 2.0).round()), g, p.txt);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Replaces in the rows shown (the bar's column only, if one is picked), in the background.
fn start_replace(tab: &Tab, w: &mut Work, ctx: &egui::Context) {
    let Some(snap) = Snap::of(tab) else { return };
    let op = match Replace::new(&spec(tab), &snap.st) {
        Ok(Some(op)) => op,
        Ok(None) => return,
        Err(e) => {
            w.error = Some(e);
            return;
        }
    };
    let cols: Vec<usize> = match tab.find.col {
        Some(c) => vec![c],
        None => (0..snap.st.ncols).collect(),
    };
    w.replace = Some(Job::spawn(ctx, move |cancel, _| edit::changes(&snap.st, &snap.rows(), &cols, &op, cancel)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use csvfab_engine::base::{Base, OpenOptions};
    use csvfab_engine::Table;

    #[test]
    fn steps_wrap_around() {
        let l = [(0, 2), (3, 0), (3, 1), (7, 4)];
        assert_eq!(next(&l, None, 1), 0);
        assert_eq!(next(&l, None, -1), 3);
        assert_eq!(next(&l, Some((3, 0)), 1), 2);
        assert_eq!(next(&l, Some((3, 0)), -1), 0);
        assert_eq!(next(&l, Some((5, 0)), 1), 3);
        assert_eq!(next(&l, Some((7, 4)), 1), 0);
        assert_eq!(next(&l, Some((0, 0)), -1), 3);
    }

    #[test]
    fn lists_matches_in_view_order() {
        let t = Table::new(Arc::new(Base::from_bytes(b"a,b\nParis,x\nLyon,paris 2\nNice,y\n".to_vec(), OpenOptions::default())), None);
        let ids: Vec<_> = (0..3).rev().map(|p| t.st.data_id(p)).collect();
        let snap = Snap { st: t.st.clone(), view: Some(Arc::new(ids)) };
        let spec = ReplaceSpec { find: "paris".into(), ..Default::default() };
        let op = Replace::new(&spec, &t.st).unwrap().unwrap();
        let n = AtomicUsize::new(0);
        let no = AtomicBool::new(false);
        // Shown bottom up: "Lyon,paris 2" is at position 1, "Paris" at 2.
        assert_eq!(find_all(&snap, &op, None, &[], &n, &no).unwrap(), [(1, 1), (2, 0)]);
        assert_eq!(find_all(&snap, &op, Some(0), &[], &n, &no).unwrap(), [(2, 0)]);
        // A hidden column is not searched.
        assert_eq!(find_all(&snap, &op, None, &[1], &n, &no).unwrap(), [(2, 0)]);
        let spec = ReplaceSpec { find: "paris".into(), match_case: true, ..Default::default() };
        let op = Replace::new(&spec, &t.st).unwrap().unwrap();
        assert_eq!(find_all(&snap, &op, None, &[], &n, &no).unwrap(), [(1, 1)]);
    }
}
