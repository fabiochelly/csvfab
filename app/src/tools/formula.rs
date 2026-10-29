//! The computed column (a formula run on every row, into a new column or over
//! a column's values in the rows shown) and the expression box's helpers:
//! the autocomplete under the search box in Expression mode, shared with the
//! formula editor. The language, its checks and its run are the engine's
//! (`csvfab_engine::formula`); this is the editor around it.

use super::fx_picker::{self, button, byte_to_char, char_to_byte, marked, mono, mono_bold, ui_font, Out, Picker};
use super::{Cmd, Command, Cx, Tool};
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use csvfab_engine::formula::{self, doc};
use csvfab_engine::view::SortKey;
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, text::LayoutJob, vec2, Align2, Color32, FontId, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use std::time::{Duration, Instant};

// ---- Autocomplete -----------------------------------------------------------

/// The suggestions for the text before the caret (`doc::complete`): inside an
/// open brace the columns, after a value and a dot its methods, elsewhere the
/// helpers once a word is begun. ↑ ↓ move, Enter or Tab insert, Escape closes.
#[derive(Clone, Default)]
pub struct Ac {
    comp: Option<doc::Completion>,
    act: usize,
    /// The text and caret (byte) the list is for.
    at: Option<(String, usize)>,
    /// Closed by Escape for this text and caret.
    dismissed: Option<(String, usize)>,
    /// Where the list was drawn last frame: a click there keeps it open though
    /// the box loses the focus on the press.
    rect: Option<Rect>,
}

impl Ac {
    fn update(&mut self, text: &str, caret: usize, headers: &[String]) {
        let key = (text.to_string(), caret);
        if self.at.as_ref() == Some(&key) {
            return;
        }
        self.comp = doc::complete(text, caret, headers);
        self.act = 0;
        if self.dismissed.as_ref().is_some_and(|d| *d != key) {
            self.dismissed = None;
        }
        self.at = Some(key);
    }

    pub fn is_open(&self) -> bool {
        self.comp.is_some() && self.dismissed.is_none()
    }

    fn close(&mut self) {
        self.comp = None;
        self.at = None;
        self.rect = None;
    }

    /// The list's keys, taken before the text box sees them: the item to insert, if one is.
    fn keys(&mut self, ctx: &egui::Context) -> Option<usize> {
        if self.dismissed.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            self.dismissed = None;
            return None;
        }
        if !self.is_open() {
            return None;
        }
        let n = self.comp.as_ref().map_or(0, |c| c.items.len());
        let mut pick = None;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                self.act = (self.act + 1) % n;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.act = (self.act + n - 1) % n;
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                pick = Some(self.act);
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                self.dismissed = self.at.clone();
            }
        });
        pick
    }

    /// Inserts item `i` into `text`; returns the caret after it (chars).
    fn accept(&mut self, text: &mut String, i: usize) -> Option<usize> {
        let c = self.comp.take()?;
        let it = c.items.get(i)?;
        let (s, e) = (c.start.min(text.len()), c.end.min(text.len()));
        text.replace_range(s..e, &it.insert);
        self.at = None;
        Some(byte_to_char(text, s + it.insert.len()))
    }

    /// Draws the list with its top-left at `at`; returns the item clicked.
    fn popup(&mut self, ctx: &egui::Context, id: Id, at: Pos2, min_w: f32, p: &Palette) -> Option<usize> {
        let comp = self.comp.as_ref()?;
        let mut pick = None;
        let lab = |s: &doc::Suggestion| match s.kind {
            doc::SuggestKind::Column => marked(&format!("{{{}}}", s.label), &s.hits.iter().map(|h| h + 1).collect::<Vec<_>>(), mono(12.5), mono_bold(12.5), p.txt_hi, p.accent_2),
            _ => marked(&s.label, &s.hits, mono(12.5), mono_bold(12.5), p.txt_hi, p.accent_2),
        };
        let kind = |s: &doc::Suggestion| match &s.kind {
            doc::SuggestKind::Column => "column".to_string(),
            doc::SuggestKind::Function(sig) => sig.clone(),
        };
        let frame = egui::Frame::new()
            .fill(p.surf)
            .stroke(Stroke::new(1.0, p.brd))
            .corner_radius(8)
            .inner_margin(4)
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(128) });
        let r = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
            frame.show(ui, |ui| {
                // As wide as its widest line (label, 18, signature), at least the box.
                let w = comp.items.iter().map(|s| {
                    let a = ui.painter().layout_job(lab(s)).size().x;
                    let b = ui.painter().layout_no_wrap(kind(s), mono(11.0), p.txt_dim).size().x;
                    a + 18.0 + b + 20.0
                }).fold(min_w - 10.0, f32::max);
                egui::ScrollArea::vertical().max_height(280.0 - 10.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    for (i, s) in comp.items.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(w, 27.5), Sense::click());
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            self.act = i;
                        }
                        if i == self.act {
                            ui.painter().rect_filled(r, 5.0, alpha(p.accent, 0.14));
                        }
                        let g = ui.painter().layout_job(lab(s));
                        ui.painter().galley(pos2(r.min.x + 10.0, (r.center().y - g.size().y / 2.0).round()), g, p.txt_hi);
                        ui.painter().text(pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, kind(s), mono(11.0), p.txt_dim);
                        if resp.clicked() {
                            pick = Some(i);
                        }
                    }
                });
            });
        });
        self.rect = Some(r.response.rect);
        pick
    }

    /// Whether the pointer is on the list (pressing it takes the focus from the box).
    fn holds_pointer(&self, ctx: &egui::Context) -> bool {
        self.rect.is_some_and(|r| ctx.pointer_latest_pos().is_some_and(|p| r.contains(p)))
    }
}

/// What lets Tab, the arrows and Escape reach the list instead of moving the focus.
const LIST_KEYS: egui::EventFilter = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };

fn ac_id(tab: &Tab) -> Id {
    Id::unique(("expr-ac", tab.uid))
}

/// Before the search box is drawn (Expression mode): the list's keys — ↑ ↓,
/// Enter / Tab to insert, Escape to close the list only.
pub fn search_keys(ui: &mut Ui, tab: &mut Tab) {
    let ctx = ui.ctx().clone();
    let id = fx_picker::search_id();
    if !tab.expr_mode || !ctx.memory(|m| m.has_focus(id)) {
        return;
    }
    let mut ac: Ac = ctx.data(|d| d.get_temp(ac_id(tab))).unwrap_or_default();
    if let Some(i) = ac.keys(&ctx) {
        if let Some(c) = ac.accept(&mut tab.filter.global, i) {
            fx_picker::select(&ctx, id, (c, c));
            tab.grid.scroll = 0.0;
            tab.refresh(&ctx);
        }
    }
    ctx.data_mut(|d| d.insert_temp(ac_id(tab), ac));
}

/// After the search box is drawn (`field` its response): the autocomplete
/// list under it in Expression mode.
pub fn search_assist(ui: &mut Ui, tab: &mut Tab, field: &egui::Response, p: &Palette, m: &Metrics) {
    let ctx = ui.ctx().clone();
    let id = fx_picker::search_id();
    let mut ac: Ac = ctx.data(|d| d.get_temp(ac_id(tab))).unwrap_or_default();
    let focused = ctx.memory(|mem| mem.has_focus(id));
    let Some(headers) = tab.st().map(|s| s.headers()) else { return };
    if !tab.expr_mode || !(focused || ac.holds_pointer(&ctx)) {
        ac.close();
        ctx.data_mut(|d| d.insert_temp(ac_id(tab), ac));
        return;
    }
    let text = tab.filter.global.clone();
    let caret = char_to_byte(&text, fx_picker::selection(&ctx, id, &text).1);
    ac.update(&text, caret, &headers);
    if ac.is_open() {
        // The box: the field's text area plus its 12 px padding, the toolbar's field height.
        let r = Rect::from_center_size(field.rect.center(), vec2(field.rect.width() + 24.0, m.input_h));
        if let Some(i) = ac.popup(&ctx, Id::unique(("expr-ac-list", tab.uid)), pos2(r.min.x - 1.5, r.max.y + 4.0), 0.0, p) {
            if let Some(c) = ac.accept(&mut tab.filter.global, i) {
                fx_picker::select(&ctx, id, (c, c));
                tab.grid.scroll = 0.0;
                tab.refresh(&ctx);
            }
        }
        if focused {
            ctx.memory_mut(|mem| mem.set_focus_lock_filter(id, LIST_KEYS));
        }
    } else {
        ac.rect = None;
    }
    ctx.data_mut(|d| d.insert_temp(ac_id(tab), ac));
}

// ---- The computed column -----------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dest {
    /// A new column after this one.
    New(usize),
    /// Over this column's values, in the rows shown.
    Set(usize),
}

struct Stats {
    key: (String, Dest, u64),
    rows: usize,
    n: formula::Outcome<usize>,
}

/// What the preview shows: the formula's columns (4 at most) and its value on the first rows.
struct Preview {
    key: (String, Dest, u64, String),
    cols: Vec<usize>,
    rows: Vec<(Vec<String>, Result<String, String>)>,
}

type Applied = (Dest, Vec<(RowId, usize, String)>, usize);

pub struct Formula {
    src: String,
    name: String,
    dest: Dest,
    open: bool,
    first: bool,
    ac: Ac,
    picker: Option<(Picker, (usize, usize))>,
    /// The formula compiled, for the text it was compiled from (its error underlined).
    compiled: Option<(String, Result<(), formula::Error>)>,
    edited: Instant,
    stats: Option<Stats>,
    stats_job: Option<(Job<Option<Stats>>, (String, Dest, u64))>,
    preview: Option<Preview>,
    applying: Option<Job<Option<Result<Applied, String>>>>,
}

/// The computed column's dialog, its new column after `after` (else after the last).
pub fn open(tab: &Tab, after: Option<usize>) -> Box<dyn Tool> {
    open_with(tab, after, "")
}

/// The same with a formula already typed.
pub fn open_with(tab: &Tab, after: Option<usize>, src: &str) -> Box<dyn Tool> {
    let n = tab.st().map_or(0, |s| s.ncols);
    Box::new(Formula {
        src: src.to_string(),
        name: "Computed".into(),
        dest: Dest::New(after.unwrap_or(n.saturating_sub(1)).min(n.saturating_sub(1))),
        open: true,
        first: true,
        ac: Ac::default(),
        picker: None,
        compiled: None,
        edited: Instant::now(),
        stats: None,
        stats_job: None,
        preview: None,
        applying: None,
    })
}

fn expr_id() -> Id {
    Id::unique("fx-expr")
}

impl Formula {
    fn compile(&mut self, tab: &Tab) -> Option<formula::Expr> {
        let st = tab.st()?;
        let r = formula::compile(&self.src, &st.headers()).map(|mut e| {
            e.set_decimal_comma(formula::decimal_comma(st));
            e
        });
        if self.compiled.as_ref().is_none_or(|c| c.0 != self.src) {
            self.compiled = Some((self.src.clone(), r.as_ref().map(|_| ()).map_err(|e| e.clone())));
        }
        r.ok()
    }

    /// Counts in the background, 120 ms after the last keystroke (a formula runs on every row).
    fn run_stats(&mut self, tab: &Tab, ctx: &egui::Context) {
        let key = (self.src.clone(), self.dest, tab.generation);
        if let Some(r) = self.stats_job.as_mut().and_then(|(j, _)| j.poll()) {
            self.stats_job = None;
            if let Some(s) = r { self.stats = Some(s); }
        }
        if self.stats.as_ref().is_some_and(|s| s.key == key) || self.stats_job.as_ref().is_some_and(|j| j.1 == key) {
            return;
        }
        let wait = Duration::from_millis(120).saturating_sub(self.edited.elapsed());
        if !wait.is_zero() {
            ctx.request_repaint_after(wait);
            return;
        }
        let Some(e) = self.compile(tab) else { return };
        let Some(st) = tab.st().cloned() else { return };
        let (rows, col) = match self.dest {
            Dest::New(_) => (None, None),
            Dest::Set(c) => (tab.view.clone(), Some(c)),
        };
        let k = key.clone();
        let job = Job::spawn(ctx, move |cancel, _| {
            let n = formula::tally(&st, &e, rows.as_deref().map(|v| v.as_slice()), col, cancel)?;
            Some(Stats { key: k, rows: rows.map_or(st.len(), |v| v.len()), n })
        });
        self.stats_job = Some((job, key));
    }

    fn update_preview(&mut self, tab: &Tab) {
        let name = self.result_name(tab);
        let key = (self.src.clone(), self.dest, tab.generation, name);
        if self.preview.as_ref().is_some_and(|p| p.key == key) {
            return;
        }
        let e = self.compile(tab);
        let Some(st) = tab.st() else { return };
        let Some(e) = e else {
            self.preview = Some(Preview { key, cols: Vec::new(), rows: Vec::new() });
            return;
        };
        let cols: Vec<usize> = e.columns().iter().copied().take(4).collect();
        let ids: Vec<RowId> = match self.dest {
            Dest::New(_) => (0..st.len().min(8)).map(|p| st.data_id(p)).collect(),
            Dest::Set(_) => (0..tab.shown().min(8)).map(|p| tab.shown_id(p)).collect(),
        };
        let rows = ids.iter().map(|&id| (cols.iter().map(|&c| st.cell(id, c)).collect(), e.eval_row(st, id))).collect();
        self.preview = Some(Preview { key, cols, rows });
    }

    fn result_name(&self, tab: &Tab) -> String {
        match self.dest {
            Dest::Set(c) => tab.st().and_then(|s| s.headers().get(c).cloned()).unwrap_or_default(),
            Dest::New(_) => if self.name.trim().is_empty() { "Computed".into() } else { self.name.trim().to_string() },
        }
    }

    fn can_apply(&self, tab: &Tab) -> bool {
        let ok = self.compiled.as_ref().is_some_and(|c| c.0 == self.src && c.1.is_ok()) && self.applying.is_none();
        ok && match self.dest {
            Dest::New(_) => tab.st().is_some_and(|s| s.len() > 0),
            Dest::Set(_) => self.stats.as_ref().is_none_or(|s| s.key.0 != self.src || s.n.out > 0),
        }
    }

    fn apply(&mut self, tab: &Tab, ctx: &egui::Context) {
        let Some(e) = self.compile(tab) else { return };
        let Some(st) = tab.st().cloned() else { return };
        let dest = self.dest;
        let view = tab.view.clone();
        self.applying = Some(Job::spawn(ctx, move |cancel, _| {
            Some(Ok(match dest {
                Dest::New(at) => {
                    let o = formula::column(&st, &e, None, cancel)?;
                    let ch = o.out.into_iter().enumerate().filter(|(_, v)| !v.is_empty()).map(|(p, v)| (st.data_id(p), at + 1, v)).collect();
                    (dest, ch, o.errors)
                }
                Dest::Set(c) => {
                    let o = formula::changes(&st, &e, view.as_deref().map(|v| v.as_slice()), c, cancel)?;
                    (dest, o.out, o.errors)
                }
            }))
        }));
    }

    /// Writes the result: one undo step either way.
    fn finish(&mut self, cx: &mut Cx, (dest, ch, errors): Applied) {
        let tab = &mut *cx.tab;
        let Some(t) = &mut tab.table else { return };
        let fmt = crate::grid::fmt_int;
        match dest {
            Dest::New(at) => {
                let name = self.result_name(tab);
                let at = at + 1;
                let t = tab.table.as_mut().unwrap();
                t.batch(|t| {
                    t.insert_column(at, &name);
                    t.set_cells(&ch);
                });
                tab.col_filters.insert(at.min(tab.col_filters.len()), String::new());
                let w = new_width(cx.ctx, cx.m, &name, ch.iter().take(80).map(|c| c.2.as_str()));
                tab.grid.widths.insert(at.min(tab.grid.widths.len()), w);
                tab.sort = tab.sort.iter().map(|k| SortKey { col: if k.col >= at { k.col + 1 } else { k.col }, desc: k.desc }).collect();
                tab.after_edit(true, cx.ctx);
                let e = if errors > 0 { format!(", {} rows with an error left empty", fmt(errors)) } else { String::new() };
                cx.toast(format!("Column \"{name}\" computed{e}"));
            }
            Dest::Set(c) => {
                if ch.is_empty() {
                    cx.toast("Nothing changed.");
                    return;
                }
                let name = t.st.headers().get(c).cloned().unwrap_or_default();
                tab.apply_changes(&ch, cx.ctx);
                let e = if errors > 0 { format!(", {} errors left empty", fmt(errors)) } else { String::new() };
                cx.toast(format!("{} cells computed in {name}{e}", fmt(ch.len())));
            }
        }
    }
}

/// The new column's width, as the grid sizes them: its widest value or its title.
fn new_width<'a>(ctx: &egui::Context, m: &Metrics, title: &str, values: impl Iterator<Item = &'a str>) -> f32 {
    let cw = ctx.fonts_mut(|f| f.glyph_width(&FontId::monospace(m.font_grid), '0'));
    let chars = values.map(|v| v.chars().count().min(200)).max().unwrap_or(0);
    let t = title.chars().count() as f32 * 7.2 + 16.0 + 26.0 + 22.0;
    (chars as f32 * cw + 21.0).max(t.min(m.col_auto_max)).clamp(m.col_min_w, m.col_auto_max)
}

impl Tool for Formula {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if cx.tab.table.is_none() {
            return false;
        }
        if let Some(r) = self.applying.as_mut().and_then(|j| j.poll()) {
            self.applying = None;
            match r {
                Some(Ok(a)) => {
                    self.finish(cx, a);
                    return false;
                }
                Some(Err(e)) => cx.toast(e),
                None => {}
            }
        }
        // The picker first: its Escape must not close the dialog under it.
        if let Some((pk, sel)) = &mut self.picker {
            match pk.show(cx.ctx, cx.tab, cx.p, cx.m) {
                None => {}
                Some(Out::Close) => {
                    self.picker = None;
                    cx.ctx.memory_mut(|m| m.request_focus(expr_id()));
                }
                Some(Out::Insert(pick)) => {
                    let s = fx_picker::insert(&mut self.src, *sel, &pick, cx.tab);
                    fx_picker::select(cx.ctx, expr_id(), s);
                    self.edited = Instant::now();
                    self.picker = None;
                }
            }
        }
        self.compile(cx.tab);
        self.run_stats(cx.tab, cx.ctx);
        self.update_preview(cx.tab);
        let mut apply = false;
        let (p, m) = (cx.p, cx.m);
        let tab = &*cx.tab;
        // `.modal-box`: surf, 1 px brd, radius 8, padding 20, over a backdrop.
        let frame = egui::Frame::new()
            .fill(p.surf)
            .stroke(Stroke::new(1.0, p.brd))
            .corner_radius(8)
            .inner_margin(20)
            .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(128) });
        let modal = crate::widgets::modal(cx.ctx, Id::unique("modal-formula"), p, frame, |ui| {
            ui.set_width(560.0);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let g = ui.painter().layout_no_wrap("Computed column".into(), crate::grid::semibold(16.0), p.txt_hi);
            let (tr, _) = ui.allocate_exact_size(vec2(560.0, 19.0), Sense::hover());
            ui.painter().galley(pos2(tr.min.x, (tr.center().y - g.size().y / 2.0).round()), g, p.txt_hi);
            ui.add_space(14.0);
            // Label, and the Functions button at its right.
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Max), |ui| {
                    let mut job = LayoutJob::default();
                    job.append("Formula — ", 0.0, egui::TextFormat::simple(ui_font(12.0), p.txt_mut));
                    job.append("{Column}", 0.0, egui::TextFormat::simple(mono(12.0), p.txt_mut));
                    job.append(" is the row's cell, the rest is JavaScript", 0.0, egui::TextFormat::simple(ui_font(12.0), p.txt_mut));
                    ui.add_space(0.0);
                    ui.allocate_ui_with_layout(vec2(ui.available_width() - 120.0, 35.0), egui::Layout::left_to_right(egui::Align::Max), |ui| {
                        ui.add_space(0.0);
                        let g = ui.painter().layout_job(job);
                        let (r, _) = ui.allocate_exact_size(vec2(g.size().x, 35.0), Sense::hover());
                        ui.painter().galley(pos2(r.min.x, r.max.y - 5.0 - g.size().y), g, p.txt_mut);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                        if functions_button(ui, p).on_hover_text("Every function, with examples run on your data").clicked() {
                            let sel = fx_picker::selection(ui.ctx(), expr_id(), &self.src);
                            self.picker = Some((Picker::new(None, tab), sel));
                        }
                    });
                });
            });
            ui.add_space(5.0);
            // The formula.
            if self.picker.is_none() {
                if let Some(i) = self.ac.keys(ui.ctx()) {
                    if let Some(c) = self.ac.accept(&mut self.src, i) {
                        fx_picker::select(ui.ctx(), expr_id(), (c, c));
                        self.edited = Instant::now();
                    }
                }
            }
            let err = self.compiled.as_ref().filter(|c| c.0 == self.src).and_then(|c| c.1.as_ref().err()).filter(|e| !e.is_empty()).cloned();
            let w = ui.available_width();
            let lh = 19.5;
            let box_r = Rect::from_min_size(ui.cursor().min, vec2(w, 3.0 * lh + 12.0 + 2.0));
            let focused = ui.memory(|mem| mem.has_focus(expr_id()));
            if focused {
                ui.painter().rect_stroke(box_r, 6.0, Stroke::new(4.0, p.focus_ring), StrokeKind::Outside);
            }
            ui.painter().rect(box_r, 6.0, p.bg_deep, Stroke::new(1.0, if focused { p.prim } else { p.brd }), StrokeKind::Inside);
            let (txt, under) = (p.txt_hi, p.danger);
            let span = err.as_ref().map(|e| e.span.clone());
            let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                let s = buf.as_str();
                let mut job = LayoutJob::default();
                let fmt = |u: bool| {
                    let mut f = egui::TextFormat::simple(mono(13.0), txt);
                    f.line_height = Some(lh);
                    if u { f.underline = Stroke::new(1.5, under); }
                    f
                };
                match span.clone().map(|r| underline_range(s, r)) {
                    Some((a, b)) if a < b => {
                        job.append(&s[..a], 0.0, fmt(false));
                        job.append(&s[a..b], 0.0, fmt(true));
                        job.append(&s[b..], 0.0, fmt(false));
                    }
                    _ => job.append(s, 0.0, fmt(false)),
                }
                job.wrap.max_width = wrap;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let out = egui::TextEdit::multiline(&mut self.src)
                .id(expr_id())
                .font(mono(13.0))
                .text_color(p.txt_hi)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 7, bottom: 7 }))
                .desired_rows(3)
                .desired_width(w)
                .min_size(vec2(w, box_r.height()))
                .lock_focus(true)
                .event_filter(if self.ac.is_open() { LIST_KEYS } else { egui::EventFilter { escape: false, ..LIST_KEYS } })
                .hint_text(egui::RichText::new("e.g.  {firstName} + \" \" + {lastName}").font(mono(13.0)).color(p.txt_sub))
                .layouter(&mut layouter)
                .show(ui);
            if self.first {
                out.response.request_focus();
                self.first = false;
            }
            if out.response.changed() {
                self.edited = Instant::now();
            }
            // The autocomplete under the caret.
            let headers = tab.st().map(|s| s.headers()).unwrap_or_default();
            if (out.response.has_focus() || self.ac.holds_pointer(ui.ctx())) && self.picker.is_none() {
                let caret = out.cursor_range.map_or(self.src.chars().count(), |r| usize::from(r.primary.index));
                self.ac.update(&self.src, char_to_byte(&self.src, caret), &headers);
                if self.ac.is_open() {
                    let cr = out.galley.pos_from_cursor(egui::text::CCursor::new(caret)).translate(out.galley_pos.to_vec2());
                    if let Some(i) = self.ac.popup(ui.ctx(), Id::unique("fx-expr-ac"), pos2(cr.min.x - 10.0, cr.max.y + 4.0), 220.0, p) {
                        if let Some(c) = self.ac.accept(&mut self.src, i) {
                            fx_picker::select(ui.ctx(), expr_id(), (c, c));
                            self.edited = Instant::now();
                        }
                    }
                }
            } else {
                self.ac.close();
            }
            // The columns, to insert.
            ui.add_space(7.0);
            egui::ScrollArea::vertical().id_salt("fx-cols").max_height(64.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                    for h in &headers {
                        if chip(ui, p, h).on_hover_text("Insert this column").clicked() {
                            let sel = fx_picker::selection(ui.ctx(), expr_id(), &self.src);
                            let s = fx_picker::insert(&mut self.src, sel, &fx_picker::Pick::Text(format!("{{{h}}}")), tab);
                            fx_picker::select(ui.ctx(), expr_id(), s);
                            self.edited = Instant::now();
                        }
                    }
                });
            });
            ui.add_space(1.0);
            let mut job = LayoutJob::default();
            job.append("Numbers are written with a decimal comma in a ", 0.0, egui::TextFormat::simple(ui_font(12.0), p.txt_dim));
            job.append(";", 0.0, egui::TextFormat::simple(mono(12.0), p.txt_dim));
            job.append(" file, dates as yyyy-mm-dd.", 0.0, egui::TextFormat::simple(ui_font(12.0), p.txt_dim));
            job.wrap.max_width = w;
            ui.label(job);
            // Name, Write to.
            ui.add_space(24.0);
            let half = (w - 10.0) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                ui.vertical(|ui| {
                    ui.set_width(half);
                    field_label(ui, p, "Name");
                    let r = Rect::from_min_size(ui.cursor().min, vec2(half, m.input_h));
                    let set = matches!(self.dest, Dest::Set(_));
                    ui.allocate_rect(r, Sense::hover());
                    let fid = Id::unique("fx-name");
                    let foc = ui.memory(|mem| mem.has_focus(fid));
                    if foc { ui.painter().rect_stroke(r, 6.0, Stroke::new(4.0, p.focus_ring), StrokeKind::Outside); }
                    ui.painter().rect(r, 6.0, p.bg_deep, Stroke::new(1.0, if foc { p.prim } else { p.brd }), StrokeKind::Inside);
                    let te = egui::TextEdit::singleline(&mut self.name).id(fid).font(ui_font(14.0)).text_color(if set { p.txt_dim } else { p.txt_hi }).frame(egui::Frame::NONE);
                    ui.add_enabled_ui(!set, |ui| crate::widgets::centered_edit(ui, r.shrink2(vec2(12.0, 0.0)), te));
                });
                ui.vertical(|ui| {
                    ui.set_width(half);
                    field_label(ui, p, "Write to");
                    let headers = &headers;
                    let label = match self.dest {
                        Dest::New(c) => format!("after {}", headers.get(c).map_or("", |s| s.as_str())),
                        Dest::Set(c) => headers.get(c).cloned().unwrap_or_default(),
                    };
                    let (r, resp) = ui.allocate_exact_size(vec2(half, m.input_h), Sense::click());
                    ui.painter().rect(r, 6.0, p.bg_deep, Stroke::new(1.0, if resp.hovered() { p.brd_2 } else { p.brd }), StrokeKind::Inside);
                    let mut job = LayoutJob::simple_singleline(label, ui_font(14.0), p.txt_hi);
                    job.wrap = egui::text::TextWrapping { max_width: half - 12.0 - 36.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                    let g = ui.painter().layout_job(job);
                    ui.painter().galley(pos2(r.min.x + 12.0, (r.center().y - g.size().y / 2.0).round()), g, p.txt_hi);
                    icons::paint(ui.painter(), Icon::ChevronDownWide, Rect::from_min_size(pos2(r.max.x - 12.0 - 16.0, r.center().y - 6.0), vec2(16.0, 12.0)), Color32::from_rgb(0x8b, 0x94, 0x9e));
                    egui::Popup::menu(&resp).frame(crate::theme::menu_frame(p, m)).show(|ui| {
                        crate::widgets::width(ui, half.max(240.0));
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            crate::widgets::head(ui, p, "New column");
                            for (i, h) in headers.iter().enumerate() {
                                if crate::widgets::Item::new(&format!("after {h}")).checked(self.dest == Dest::New(i)).show(ui, p).clicked() {
                                    self.dest = Dest::New(i);
                                }
                            }
                            crate::widgets::sep(ui, p);
                            crate::widgets::head(ui, p, "Replace the values of (rows shown)");
                            for (i, h) in headers.iter().enumerate() {
                                if crate::widgets::Item::new(h).checked(self.dest == Dest::Set(i)).show(ui, p).clicked() {
                                    self.dest = Dest::Set(i);
                                }
                            }
                        });
                    });
                });
            });
            // What it gives.
            ui.add_space(10.0);
            stats_line(ui, p, self, tab, w);
            if let Some(pv) = &self.preview {
                if !pv.rows.is_empty() {
                    ui.add_space(10.0);
                    preview_table(ui, p, pv, &headers, &self.result_name(tab), w);
                }
            }
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                if button(ui, p, "Apply", true, self.can_apply(tab)).clicked() {
                    apply = true;
                }
                if button(ui, p, "Cancel", false, true).clicked() {
                    self.open = false;
                }
            });
        });
        // Escape and a click beside it close it — not while its list or the picker is open.
        if self.picker.is_none() && modal.should_close() {
            self.open = false;
        }
        if apply {
            self.apply(cx.tab, cx.ctx);
        }
        if !self.open {
            // Cancelled: an apply under way stops with it.
            self.applying = None;
        }
        self.open || self.applying.is_some()
    }
}

/// The error's byte range, on char boundaries and at least one character wide (an
/// empty span is the character it points at, or the one before at the end).
fn underline_range(s: &str, r: std::ops::Range<usize>) -> (usize, usize) {
    let fix = |mut i: usize| {
        i = i.min(s.len());
        while !s.is_char_boundary(i) { i -= 1; }
        i
    };
    let (mut a, mut b) = (fix(r.start), fix(r.end));
    if a >= b {
        if a < s.len() {
            b = a + s[a..].chars().next().map_or(0, char::len_utf8);
        } else if let Some(c) = s[..a].chars().next_back() {
            a -= c.len_utf8();
            b = s.len();
        }
    }
    (a, b)
}

/// `.fld-label`: ui 12 `txt_mut`, 5 above the field.
fn field_label(ui: &mut Ui, p: &Palette, t: &str) {
    let g = ui.painter().layout_no_wrap(t.to_string(), ui_font(12.0), p.txt_mut);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 5.0), Sense::hover());
    ui.painter().galley(r.min, g, p.txt_mut);
}

/// The Functions button (`.fx-btn.fx-big`): ƒx 20 × 17 and "Functions", ui 600 12.5 in the accent.
fn functions_button(ui: &mut Ui, p: &Palette) -> egui::Response {
    let f = crate::grid::semibold(12.5);
    let g = ui.painter().layout_no_wrap("Functions".into(), f, p.accent_2);
    let (r, resp) = ui.allocate_exact_size(vec2(10.0 + 20.0 + 7.0 + g.size().x + 12.0, 30.0), Sense::click());
    let hot = resp.hovered();
    ui.painter().rect(r, 6.0, alpha(p.accent, if hot { 0.22 } else { 0.12 }), Stroke::new(1.0, if hot { p.accent } else { alpha(p.accent, 0.55) }), StrokeKind::Inside);
    let c = if hot { p.txt_strong } else { p.accent_2 };
    icons::paint(ui.painter(), Icon::Fx, Rect::from_min_size(pos2(r.min.x + 10.0, r.center().y - 8.5), vec2(20.0, 17.0)), c);
    ui.painter().galley(pos2(r.min.x + 37.0, (r.center().y - g.size().y / 2.0).round()), g, c);
    resp
}

/// A column to insert (`.chips .dk`): mono 12, padding 1 × 7, radius 4.
fn chip(ui: &mut Ui, p: &Palette, t: &str) -> egui::Response {
    let g = ui.painter().layout_no_wrap(t.to_string(), mono(12.0), p.txt_strong);
    let (r, resp) = ui.allocate_exact_size(vec2((g.size().x + 16.0).max(18.0), 20.0), Sense::click());
    ui.painter().rect(r, 4.0, alpha(p.hi, if resp.hovered() { 0.2 } else { 0.1 }), Stroke::new(1.0, alpha(p.hi, 0.35)), StrokeKind::Inside);
    ui.painter().galley(pos2(r.min.x + 8.0, (r.center().y - g.size().y / 2.0).round()), g, p.txt_strong);
    resp
}

/// `.stat-line`: what the formula gives, or why it does not run.
fn stats_line(ui: &mut Ui, p: &Palette, f: &Formula, tab: &Tab, w: f32) {
    let fmt = crate::grid::fmt_int;
    let norm = |_: &str| egui::TextFormat { line_height: Some(12.0 * 1.6), ..egui::TextFormat::simple(ui_font(12.0), p.txt_mut) };
    let bold = egui::TextFormat { line_height: Some(12.0 * 1.6), ..egui::TextFormat::simple(crate::grid::semibold(12.0), p.txt_hi) };
    let warn = egui::TextFormat { line_height: Some(12.0 * 1.6), ..egui::TextFormat::simple(ui_font(12.0), p.warn) };
    let mut job = LayoutJob::default();
    let err = f.compiled.as_ref().filter(|c| c.0 == f.src).and_then(|c| c.1.as_ref().err());
    if f.applying.is_some() {
        job.append("Computing…", 0.0, norm(""));
    } else if f.src.trim().is_empty() {
        job.append("Type a formula, or pick an example below.", 0.0, norm(""));
    } else if let Some(e) = err {
        job.append(&e.msg, 0.0, warn.clone());
    } else if let Some(s) = f.stats.as_ref().filter(|s| s.key.1 == f.dest) {
        match f.dest {
            Dest::New(_) => {
                job.append(&fmt(s.n.out), 0.0, bold.clone());
                job.append(&format!(" of {} rows get a value", fmt(s.rows)), 0.0, norm(""));
            }
            Dest::Set(_) => {
                job.append(&fmt(s.n.out), 0.0, bold.clone());
                job.append(&format!(" cells will change in the {} {}rows", fmt(s.rows), if tab.filtered() { "filtered " } else { "" }), 0.0, norm(""));
            }
        }
        if s.n.errors > 0 {
            job.append(&format!("\n{} rows raise an error (their cell stays empty): {}", fmt(s.n.errors), s.n.first_error.clone().unwrap_or_default()), 0.0, warn);
        }
    } else {
        job.append("Counting…", 0.0, norm(""));
    }
    job.wrap.max_width = w;
    ui.label(job);
}

/// The first rows: the columns the formula reads (`txt_mut`), its value (`prim` title).
fn preview_table(ui: &mut Ui, p: &Palette, pv: &Preview, headers: &[String], name: &str, w: f32) {
    let f = ui_font(12.0);
    let hf = crate::grid::semibold(12.0);
    let cell = |s: &str| s.replace(['\n', '\r'], "↵");
    let mut widths: Vec<f32> = pv.cols.iter().map(|&c| {
        let mut x = ui.painter().layout_no_wrap(headers.get(c).cloned().unwrap_or_default(), hf.clone(), p.txt).size().x + 8.0;
        for (vals, _) in &pv.rows {
            let i = pv.cols.iter().position(|&k| k == c).unwrap();
            x = x.max(ui.painter().layout_no_wrap(cell(&vals[i]), f.clone(), p.txt).size().x + 16.0);
        }
        x.min(220.0)
    }).collect();
    let mut last = ui.painter().layout_no_wrap(name.to_string(), hf.clone(), p.prim).size().x + 8.0;
    for (_, v) in &pv.rows {
        let s = match v { Ok(s) if !s.is_empty() => cell(s), _ => "empty".into() };
        last = last.max(ui.painter().layout_no_wrap(s, f.clone(), p.txt).size().x + 16.0);
    }
    widths.push(last.min(220.0));
    // `min-width: 100%`: the spare width shared out.
    let total: f32 = widths.iter().sum();
    if total < w - 2.0 {
        let k = (w - 2.0) / total;
        for x in &mut widths { *x *= k; }
    }
    let rh = 4.0 + 15.0 + 4.0 + 1.0;
    let h = rh * (pv.rows.len() + 1) as f32;
    let (r, _) = ui.allocate_exact_size(vec2(w, (h + 2.0).min(220.0)), Sense::hover());
    let pt = ui.painter().with_clip_rect(r.shrink(1.0));
    let mut y = r.min.y + 1.0;
    let row = |vals: Vec<(String, FontId, Color32, bool)>, y: f32, head: bool| {
        if head { pt.rect_filled(Rect::from_min_size(pos2(r.min.x, y), vec2(w, rh)), 0.0, p.surf); }
        let mut x = r.min.x + 1.0;
        for ((t, font, c, it), cw) in vals.into_iter().zip(&widths) {
            let mut job = LayoutJob::simple_singleline(t, font, c);
            job.sections[0].format.italics = it;
            job.wrap = egui::text::TextWrapping { max_width: cw - if head { 8.0 } else { 16.0 }, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let g = pt.layout_job(job);
            pt.galley(pos2(x + if head { 4.0 } else { 8.0 }, (y + (rh - 1.0) / 2.0 - g.size().y / 2.0).round()), g, c);
            x += cw;
            pt.vline((x - 0.5).round() + 0.5 - 1.0, y..=y + rh, Stroke::new(1.0, p.brd));
        }
        pt.hline(r.x_range(), (y + rh - 1.0).round() + 0.5, Stroke::new(1.0, p.brd));
    };
    let mut head: Vec<_> = pv.cols.iter().map(|&c| (headers.get(c).cloned().unwrap_or_default(), hf.clone(), p.txt, false)).collect();
    head.push((name.to_string(), hf.clone(), p.prim, false));
    row(head, y, true);
    y += rh;
    for (vals, v) in &pv.rows {
        let mut cells: Vec<_> = vals.iter().map(|s| (cell(s), f.clone(), p.txt_mut, false)).collect();
        cells.push(match v {
            Ok(s) if !s.is_empty() => (cell(s), f.clone(), p.txt, false),
            _ => ("empty".into(), f.clone(), p.txt_faint, true),
        });
        row(cells, y, false);
        y += rh;
    }
    ui.painter().rect_stroke(r, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
}

pub fn commands() -> Vec<Command> {
    vec![Command {
        name: "Computed column…",
        group: "Columns",
        keys: None,
        enabled: super::has_table,
        run: |cx| {
            let t = open(cx.tab, None);
            cx.out.push(Cmd::Tool(t));
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underline_is_one_char_at_least() {
        assert_eq!(underline_range("abc", 1..1), (1, 2));
        assert_eq!(underline_range("abé", 4..4), (2, 4));
        assert_eq!(underline_range("abé", 3..9), (2, 4));
        assert_eq!(underline_range("", 0..0), (0, 0));
    }

    #[test]
    fn accept_replaces_the_word() {
        let hs = vec!["Amount".to_string(), "City".to_string()];
        let mut ac = Ac::default();
        let mut s = String::from("{ci");
        ac.update(&s, 3, &hs);
        assert!(ac.is_open());
        assert_eq!(ac.accept(&mut s, 0), Some(6));
        assert_eq!(s, "{City}");
    }
}
