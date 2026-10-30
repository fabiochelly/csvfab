//! The parts of the file tools' dialogs, drawn to the web app's measures
//! (docs/look.md, "Dialogs" and "Buttons and fields"): the `.modal-box` form
//! dialog, the `#dlg` message dialog, labels, notes, radios, checkboxes,
//! selects, fields, buttons and the `.pv` preview tables. Spacing is explicit
//! (item spacing 0): each part adds the CSS margins it has.

use super::Cx;
use crate::grid::semibold;
use crate::theme::{alpha, Palette};
use eframe::egui::{self, pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui};
use egui::text::{LayoutJob, TextFormat, TextWrapping};

pub fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::fonts::UI_MEDIUM.into()))
}

/// Numbers as the app shows them: `1 000 000`.
pub fn n(k: usize) -> String {
    crate::grid::fmt_int(k)
}

/// `3 rows`, `1 row`.
pub fn plural(k: usize, w: &str) -> String {
    format!("{} {w}{}", n(k), if k == 1 { "" } else { "s" })
}

// --- Rich text: a stat line's bold numbers and warnings --------------------

/// Marks for `rich`: the text between two of the same is bold (`txt_hi`, ui 600),
/// a warning (`warn`), muted (`txt_faint`), or new / old (`ok` / struck `danger`).
pub const B: char = '\u{1}';
pub const W: char = '\u{2}';
pub const M: char = '\u{3}';

pub fn b(s: impl std::fmt::Display) -> String {
    format!("{B}{s}{B}")
}
pub fn warn(s: impl std::fmt::Display) -> String {
    format!("{W}{s}{W}")
}
pub fn muted(s: impl std::fmt::Display) -> String {
    format!("{M}{s}{M}")
}

/// Text with the marks above, wrapped at `width`, in ui `size` and colour `base`.
pub fn rich(p: &Palette, text: &str, size: f32, base: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let (mut bold, mut w, mut m) = (false, false, false);
    let mut run = String::new();
    let flush = |job: &mut LayoutJob, run: &mut String, bold: bool, w: bool, m: bool| {
        if run.is_empty() { return; }
        let color = if w { p.warn } else if m { p.txt_faint } else if bold { p.txt_hi } else { base };
        let font = if bold { semibold(size) } else { ui_font(size) };
        job.append(run, 0.0, TextFormat { font_id: font, color, line_height: Some((size * 1.5).round()), ..Default::default() });
        run.clear();
    };
    for ch in text.chars() {
        match ch {
            B => { flush(&mut job, &mut run, bold, w, m); bold = !bold; }
            W => { flush(&mut job, &mut run, bold, w, m); w = !w; }
            M => { flush(&mut job, &mut run, bold, w, m); m = !m; }
            c => run.push(c),
        }
    }
    flush(&mut job, &mut run, bold, w, m);
    job.wrap = TextWrapping { max_width: width, ..Default::default() };
    job
}

/// A paragraph of rich text, full width.
pub fn para(ui: &mut Ui, p: &Palette, text: &str, size: f32, base: Color32) {
    let job = rich(p, text, size, base, ui.available_width());
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, base);
}

/// `.fld-label`: ui 12 `txt_mut`, `top` px above, 5 below.
pub fn label(ui: &mut Ui, p: &Palette, text: &str, top: f32) {
    ui.add_space(top);
    para(ui, p, text, 12.0, p.txt_mut);
    ui.add_space(5.0);
}

/// `.stat-line`: ui 12 `txt_mut`, line 1.6, 10 above; bold parts `txt_hi`.
pub fn stat(ui: &mut Ui, p: &Palette, text: &str) {
    stat_at(ui, p, text, 10.0);
}

/// A stat line `top` px below what precedes it.
pub fn stat_at(ui: &mut Ui, p: &Palette, text: &str, top: f32) -> egui::Response {
    ui.add_space(top);
    let mut job = rich(p, text, 12.0, p.txt_mut, ui.available_width());
    for s in &mut job.sections { s.format.line_height = Some(19.0); }
    let g = ui.painter().layout_job(job);
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, p.txt_mut);
    resp
}

// --- Dialog frames ----------------------------------------------------------

/// The backdrop: the web blurs what is behind; without blur, darker (look.md).
pub fn backdrop(p: &Palette) -> Color32 {
    alpha(p.bg_deep, 0.6)
}

/// A form dialog (`.modal-box`): `width` of content, radius 8, padding 20,
/// the title in ui 600 16 with 14 below, at most 80 % of the window tall
/// (the content scrolls). Returns the content's result and whether Escape or
/// a click beside it asks to close.
pub fn modal<R>(cx: &mut Cx, id: &str, width: f32, title: &str, add: impl FnOnce(&mut Ui, &mut Cx) -> R) -> (R, bool) {
    let p = cx.p.clone();
    let ctx = cx.ctx.clone();
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - 40.0 - 42.0);
    let frame = crate::widgets::dialog_frame(&p, egui::Margin::same(20));
    let r = crate::widgets::modal(&ctx, egui::Id::unique(("ftool", id)), &p, frame, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        crate::widgets::dialog_title(ui, &p, title);
        let max_h = screen.height() * 0.8 - 40.0 - 34.0;
        // min_scrolled_height: the area's room is guessed from its last size
        // (anchored at the centre); the box is as tall as its content up to max_h.
        egui::ScrollArea::vertical().id_salt(("ftool-scroll", id)).max_height(max_h).min_scrolled_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(width);
            add(ui, cx)
        }).inner
    });
    let close = r.should_close();
    (r.inner, close)
}

/// A tall form dialog (Anonymise, Review): a fixed height, the content laid out by the caller.
pub fn modal_tall<R>(cx: &mut Cx, id: &str, width: f32, height: f32, title: &str, add: impl FnOnce(&mut Ui, &mut Cx) -> R) -> (R, bool) {
    let p = cx.p.clone();
    let ctx = cx.ctx.clone();
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - 40.0 - 42.0);
    let height = height.min(screen.height() - 40.0 - 42.0);
    let frame = crate::widgets::dialog_frame(&p, egui::Margin::same(20));
    let r = crate::widgets::modal(&ctx, egui::Id::unique(("ftool", id)), &p, frame, |ui| {
        ui.set_width(width);
        ui.set_height(height);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        crate::widgets::dialog_title(ui, &p, title);
        add(ui, cx)
    });
    let close = r.should_close();
    (r.inner, close)
}

/// What a message dialog's buttons are.
pub struct Ask<'a> {
    pub main: &'a str,
    pub sub: &'a str,
    /// Extra choices, outline, left of Cancel.
    pub choices: &'a [&'a str],
    /// None: an alert (OK only).
    pub cancel: Option<&'a str>,
    pub ok: &'a str,
    pub danger: bool,
}

/// What was answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Ok,
    Cancel,
    Choice(usize),
}

/// The message dialog (`#dlg`): min(440, window − 32) wide, padding 22 22 18,
/// the message ui 600 14.5, the detail ui 13 `txt_mut` 8 below, buttons 20
/// below: the choices, Cancel, then OK (primary or danger). Enter is OK,
/// Escape or a click beside it Cancel (OK for an alert).
pub fn ask(cx: &mut Cx, id: &str, a: &Ask) -> Option<Answer> {
    ask_with(cx, id, a, 440.0, |_, _| None)
}

/// The same with something between the detail and the buttons (the sheet list).
pub fn ask_with(cx: &mut Cx, id: &str, a: &Ask, width: f32, mid: impl FnOnce(&mut Ui, &Palette) -> Option<Answer>) -> Option<Answer> {
    let p = cx.p.clone();
    let ctx = cx.ctx.clone();
    let w = width.min(ctx.content_rect().width() - 32.0) - 44.0;
    let frame = crate::widgets::dialog_frame(&p, egui::Margin { left: 22, right: 22, top: 22, bottom: 18 });
    let mut ans = None;
    let r = crate::widgets::modal(&ctx, egui::Id::unique(("fask", id)), &p, frame, |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let mut job = rich(&p, a.main, 14.5, p.txt_hi, w);
        for s in &mut job.sections { s.format.font_id = semibold(14.5); s.format.line_height = Some(21.0); }
        let g = ui.painter().layout_job(job);
        let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
        ui.painter().galley(r.min, g, p.txt_hi);
        if !a.sub.is_empty() {
            ui.add_space(8.0);
            let mut job = rich(&p, a.sub, 13.0, p.txt_mut, w);
            for s in &mut job.sections { s.format.line_height = Some(19.5); }
            let g = ui.painter().layout_job(job);
            let (r, _) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
            ui.painter().galley(r.min, g, p.txt_mut);
        }
        if let Some(x) = mid(ui, &p) { ans = Some(x); }
        ui.add_space(20.0);
        let mut btns: Vec<Btn> = a.choices.iter().map(|c| Btn::outline(c)).collect();
        if let Some(c) = a.cancel { btns.push(Btn::outline(c)); }
        btns.push(if a.danger { Btn::danger(a.ok) } else { Btn::primary(a.ok) });
        match actions(ui, &p, &btns) {
            Some(k) if k == btns.len() - 1 => ans = Some(Answer::Ok),
            Some(k) if a.cancel.is_some() && k == btns.len() - 2 => ans = Some(Answer::Cancel),
            Some(k) => ans = Some(Answer::Choice(k)),
            None => {}
        }
    });
    if ans.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) { ans = Some(Answer::Ok); }
    if ans.is_none() && r.should_close() { ans = Some(if a.cancel.is_some() { Answer::Cancel } else { Answer::Ok }); }
    ans
}

/// An alert waiting to be read: tools keep one and show it until OK.
pub struct Alert {
    pub main: String,
    pub sub: String,
}

impl Alert {
    pub fn new(msg: impl Into<String>) -> Alert {
        let msg = msg.into();
        match msg.split_once("\n\n") {
            Some((a, b)) => Alert { main: a.into(), sub: b.into() },
            None => Alert { main: msg, sub: String::new() },
        }
    }

    /// True once dismissed.
    pub fn show(&self, cx: &mut Cx, id: &str) -> bool {
        ask(cx, id, &Ask { main: &self.main, sub: &self.sub, choices: &[], cancel: None, ok: "OK", danger: false }).is_some()
    }
}

// --- Buttons ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Outline,
    Primary,
    Danger,
}

pub struct Btn<'a> {
    pub text: &'a str,
    pub kind: Kind,
    pub enabled: bool,
    pub tip: &'a str,
}

impl<'a> Btn<'a> {
    pub fn outline(text: &'a str) -> Self {
        Btn { text, kind: Kind::Outline, enabled: true, tip: "" }
    }
    pub fn primary(text: &'a str) -> Self {
        Btn { text, kind: Kind::Primary, enabled: true, tip: "" }
    }
    pub fn danger(text: &'a str) -> Self {
        Btn { text, kind: Kind::Danger, enabled: true, tip: "" }
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn tip(mut self, t: &'a str) -> Self {
        self.tip = t;
        self
    }
}

/// `.btn`: ui 500 13, padding 6 × 14, 30 tall, radius 6; disabled at .45 (`widgets::button`).
pub fn button(ui: &mut Ui, p: &Palette, b: &Btn) -> Response {
    let kind = match b.kind {
        Kind::Primary => crate::widgets::Kind::Primary,
        Kind::Danger => crate::widgets::Kind::Danger,
        Kind::Outline => crate::widgets::Kind::Outline,
    };
    let resp = crate::widgets::button(ui, p, kind, None, b.text, b.enabled, 30.0, 14.0, 13.0);
    if !b.tip.is_empty() { resp.on_hover_text(b.tip) } else { resp }
}

/// `.modal-actions`: buttons right-aligned, 8 apart; the index of the one clicked.
pub fn actions(ui: &mut Ui, p: &Palette, btns: &[Btn]) -> Option<usize> {
    // Lines of buttons, filled from the right: a dialog too narrow for all of
    // them (Compare's four) takes two lines rather than overflowing its box.
    let w = ui.available_width();
    let widths: Vec<f32> = btns.iter().map(|b| ui.painter().layout_no_wrap(b.text.to_string(), medium(13.0), Color32::WHITE).size().x.round() + 28.0).collect();
    let mut lines: Vec<std::ops::Range<usize>> = Vec::new();
    let mut end = btns.len();
    while end > 0 {
        let mut start = end - 1;
        let mut used = widths[start];
        while start > 0 && used + 8.0 + widths[start - 1] <= w {
            start -= 1;
            used += 8.0 + widths[start];
        }
        lines.push(start..end);
        end = start;
    }
    lines.reverse();
    // Screenshot / test hook `submit`: the last button (OK, Save…) clicked once it is enabled.
    let mut hit = (btns.last().is_some_and(|b| b.enabled) && crate::shot::take("submit").is_some()).then(|| btns.len() - 1);
    for (i, line) in lines.into_iter().enumerate() {
        if i > 0 { ui.add_space(8.0); }
        ui.allocate_ui_with_layout(vec2(w, 30.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for k in line.rev() {
                if button(ui, p, &btns[k]).clicked() { hit = Some(k); }
            }
        });
    }
    hit
}

/// Buttons left-aligned, 10 apart (`.row2`), with something right-aligned after them.
pub fn row_buttons(ui: &mut Ui, p: &Palette, btns: &[Btn], right: impl FnOnce(&mut Ui)) -> Option<usize> {
    let mut hit = None;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 30.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (k, b) in btns.iter().enumerate() {
            if button(ui, p, b).clicked() { hit = Some(k); }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
    });
    hit
}

// --- Radios and checkboxes ----------------------------------------------------

fn mark_row(ui: &mut Ui, p: &Palette, on: bool, text: &str, key: &str, round: bool, full: bool, enabled: bool) -> Response {
    let _ = full;
    crate::widgets::tick_row(ui, p, on, round, text, p.txt_hi, 8.0, 20.0, key, enabled)
}

/// A radio and its label (`.col-label`): true when clicked.
pub fn radio(ui: &mut Ui, p: &Palette, on: bool, text: &str) -> bool {
    mark_row(ui, p, on, text, "", true, false, true).clicked()
}

/// A full-width radio with a key at its right (`#save-fmt`: `;`, `.xlsx`).
pub fn radio_key(ui: &mut Ui, p: &Palette, on: bool, text: &str, key: &str) -> bool {
    mark_row(ui, p, on, text, key, true, true, true).clicked()
}

/// A checkbox and its label: true when it changed.
pub fn check(ui: &mut Ui, p: &Palette, on: &mut bool, text: &str) -> bool {
    check_enabled(ui, p, on, text, true)
}

pub fn check_enabled(ui: &mut Ui, p: &Palette, on: &mut bool, text: &str, enabled: bool) -> bool {
    if mark_row(ui, p, *on, text, "", false, false, enabled).clicked() {
        *on = !*on;
        return true;
    }
    false
}

/// `.opts`: radios or checkboxes in a wrapping row, 18 apart (4 between lines), 10 above.
pub fn opts<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(18.0, 4.0);
        add(ui)
    }).inner
}

// --- Fields -----------------------------------------------------------------

/// A text field (`.bs-input`): `width` × 31, padding 6 × 12, ui 14 `txt_hi`
/// (mono 13 when `mono`), placeholder `txt_sub`.
pub fn field(ui: &mut Ui, p: &Palette, id: egui::Id, text: &mut String, width: f32, mono: bool, hint: &str) -> Response {
    let (r, _) = ui.allocate_exact_size(vec2(width, 31.0), Sense::hover());
    let font = if mono { FontId::monospace(13.0) } else { ui_font(14.0) };
    crate::widgets::text_field(ui, p, id, r, text, hint, font, 12.0, false)
}

/// A select (`.bs-select`): the field look, the value in ui `size`, a chevron 16 × 12
/// 12 px from the right; its options in a menu. True when the choice changed.
pub fn select(ui: &mut Ui, p: &Palette, id: impl std::hash::Hash, width: f32, height: f32, size: f32, sel: &mut usize, options: &[String]) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let id = ui.scope_id().with(("form-select", { use std::hash::Hasher; let mut h = std::hash::DefaultHasher::new(); id.hash(&mut h); h.finish() }));
    crate::widgets::select(ui, p, id, r, &resp, sel, options, size)
}

/// A label and a control on one line (`.row2`, 10 apart).
pub fn row2<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 31.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        add(ui)
    }).inner
}

/// Plain text on a line, ui `size`, coloured.
pub fn text(ui: &mut Ui, s: &str, size: f32, c: Color32) -> Response {
    let g = ui.painter().layout_no_wrap(s.to_string(), ui_font(size), c);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x, 20.0), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    ui.painter().galley(pos2(r.min.x, ((r.center().y - g.size().y / 2.0) * ppp).round() / ppp), g, c);
    resp
}

// --- Preview tables (`.pv`) -----------------------------------------------------

/// How a piece of a cell is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sty {
    Txt,
    Sub,
    /// `.muted` / `.empty`: `txt_faint`, italic when `Empty`.
    Faint,
    Empty,
    Warn,
    /// Review / compare: old values struck in `danger`, new ones `ok`.
    Old,
    New,
    Add,
    Del,
    Chg,
    Bold,
}

#[derive(Clone, Default)]
pub struct PvCell {
    pub parts: Vec<(String, Sty)>,
    /// Columns it spans.
    pub span: usize,
}

impl PvCell {
    pub fn new(s: impl Into<String>, sty: Sty) -> PvCell {
        PvCell { parts: vec![(s.into(), sty)], span: 1 }
    }
    pub fn txt(s: impl Into<String>) -> PvCell {
        PvCell::new(s, Sty::Txt)
    }
    pub fn and(mut self, s: impl Into<String>, sty: Sty) -> PvCell {
        self.parts.push((s.into(), sty));
        self
    }
    pub fn span(mut self, k: usize) -> PvCell {
        self.span = k;
        self
    }
    /// A value as review shows it: "empty" in italic when there is none, cut at `max` characters.
    pub fn value(v: &str, sty: Sty, max: usize) -> PvCell {
        PvCell { parts: vec![value_part(v, sty, max)], span: 1 }
    }
}

pub fn value_part(v: &str, sty: Sty, max: usize) -> (String, Sty) {
    if v.is_empty() { return ("empty".into(), Sty::Empty); }
    let one: String = v.chars().map(|c| if c == '\n' || c == '\r' { '↵' } else { c }).collect();
    if one.chars().count() > max { (one.chars().take(max).collect::<String>() + "…", sty) } else { (one, sty) }
}

#[derive(Clone, Default)]
pub struct PvRow {
    pub cells: Vec<PvCell>,
    /// A section head (`tr.sec th`): ui 600 `txt_mut`, padding 6 × 8.
    pub sec: bool,
    /// Clickable (`tr.go`).
    pub go: bool,
}

impl PvRow {
    pub fn new(cells: Vec<PvCell>) -> PvRow {
        PvRow { cells, sec: false, go: false }
    }
    pub fn sec(cells: Vec<PvCell>) -> PvRow {
        PvRow { cells, sec: true, go: false }
    }
    pub fn go(mut self) -> PvRow {
        self.go = true;
        self
    }
}

fn sty_format(p: &Palette, sty: Sty, sec: bool) -> TextFormat {
    let font = if sec || sty == Sty::Bold { semibold(12.0) } else { ui_font(12.0) };
    let color = match sty {
        Sty::Txt => p.txt,
        Sty::Bold => p.txt_hi,
        Sty::Sub => p.txt_sub,
        Sty::Faint | Sty::Empty => p.txt_faint,
        Sty::Warn | Sty::Chg => p.warn,
        Sty::Old | Sty::Del => p.danger,
        Sty::New | Sty::Add => p.ok,
    };
    let color = if sec { p.txt_mut } else { color };
    TextFormat {
        font_id: font,
        color,
        italics: sty == Sty::Empty,
        strikethrough: if sty == Sty::Old { Stroke::new(1.0, alpha(p.danger, 0.5)) } else { Stroke::NONE },
        ..Default::default()
    }
}

fn cell_job(p: &Palette, c: &PvCell, sec: bool, width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (s, sty) in &c.parts {
        job.append(s, 0.0, sty_format(p, *sty, sec));
    }
    job.wrap = TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job
}

/// A preview table: a bordered box (radius 6, 10 above), the head row sticky,
/// rows 24 tall (padding 4 × 8, ui 12, 1 px `brd` rules), `max_h` tall at most
/// (scrolls). Column widths from their content, `max_w` at most; the last one
/// takes the room left. Returns the index of the row clicked.
pub fn pv(ui: &mut Ui, p: &Palette, id: &str, head: &[PvCell], rows: &[PvRow], max_h: f32, max_w: f32) -> Option<usize> {
    ui.add_space(10.0);
    let total_w = ui.available_width();
    let ncols = head.len().max(rows.iter().map(|r| r.cells.iter().map(|c| c.span.max(1)).sum::<usize>()).max().unwrap_or(0)).max(1);
    // Natural widths, from the cells that span one column.
    let mut widths = vec![30.0f32; ncols];
    let measure = |c: &PvCell, sec: bool| -> f32 {
        let job = cell_job(p, c, sec, f32::INFINITY);
        ui.fonts_mut(|f| f.layout_job(job)).size().x + 17.0
    };
    for (k, c) in head.iter().enumerate() { widths[k] = widths[k].max(measure(c, true).min(max_w)); }
    for r in rows.iter().take(400) {
        let mut k = 0;
        for c in &r.cells {
            if c.span.max(1) == 1 && k < ncols { widths[k] = widths[k].max(measure(c, r.sec).min(max_w)); }
            k += c.span.max(1);
        }
    }
    let sum: f32 = widths.iter().sum();
    if sum < total_w - 2.0 {
        *widths.last_mut().unwrap() += total_w - 2.0 - sum;
    } else {
        // Too wide: the widest columns give way first.
        let mut over = sum - (total_w - 2.0);
        while over > 0.5 {
            let (i, w) = widths.iter().copied().enumerate().fold((0, 0.0), |a, (i, w)| if w > a.1 { (i, w) } else { a });
            let second = widths.iter().copied().filter(|&x| x < w).fold(40.0, f32::max);
            let cut = (w - second).max(1.0).min(over);
            widths[i] -= cut;
            over -= cut;
            if widths[i] <= 40.0 { break; }
        }
    }
    let row_h = 24.0;
    let head_h = if head.is_empty() { 0.0 } else { row_h };
    let body_h = (rows.len() as f32 * row_h).min(max_h - head_h - 2.0).max(0.0);
    let box_h = head_h + body_h + 2.0;
    let (bx, _) = ui.allocate_exact_size(vec2(total_w, box_h), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let draw_row = |pt: &egui::Painter, y: f32, cells: &[PvCell], sec: bool, head: bool| {
        let mut x = bx.min.x + 1.0;
        let mut k = 0;
        for c in cells {
            let span = c.span.max(1);
            let w: f32 = widths[k.min(ncols - 1)..(k + span).min(ncols)].iter().sum();
            let cr = Rect::from_min_size(pos2(x, y), vec2(w, row_h));
            let padx = if head { 4.0 } else { 8.0 };
            let g = pt.layout_job(cell_job(p, c, sec || head, w - padx * 2.0));
            pt.with_clip_rect(cr.intersect(pt.clip_rect())).galley(pos2(snap(cr.min.x + padx), snap(cr.center().y - g.size().y / 2.0)), g, p.txt);
            k += span;
            x += w;
            if k < ncols { pt.vline(snap(x) - 0.5, cr.y_range(), Stroke::new(1.0, p.brd)); }
        }
        // Cells missing at the end still get their rules.
        while k < ncols {
            x += widths[k];
            k += 1;
            if k < ncols { pt.vline(snap(x) - 0.5, y..=y + row_h, Stroke::new(1.0, p.brd)); }
        }
        pt.hline(bx.min.x + 1.0..=bx.max.x - 1.0, snap(y + row_h) - 0.5, Stroke::new(1.0, p.brd));
    };
    let mut hit = None;
    let body = Rect::from_min_size(pos2(bx.min.x, bx.min.y + 1.0 + head_h), vec2(total_w, body_h));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("pv", id)));
    egui::ScrollArea::vertical().id_salt(("pv-scroll", id)).max_height(body_h).auto_shrink([false, false]).show_viewport(&mut child, |ui, vp| {
        ui.set_height(rows.len() as f32 * row_h);
        let top = ui.min_rect().min.y;
        let first = (vp.min.y / row_h).floor().max(0.0) as usize;
        let last = ((vp.max.y / row_h).ceil() as usize).min(rows.len());
        for (i, r) in rows.iter().enumerate().take(last).skip(first) {
            let y = top + i as f32 * row_h;
            let rr = Rect::from_min_size(pos2(bx.min.x + 1.0, y), vec2(total_w - 2.0, row_h));
            if r.go {
                let resp = ui.interact(rr, ui.scope_id().with(("pvrow", i)), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rr, 0.0, alpha(p.hi, 0.06));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() { hit = Some(i); }
            }
            draw_row(ui.painter(), y, &r.cells, r.sec, false);
        }
    });
    if !head.is_empty() {
        let pt = ui.painter();
        let hr = Rect::from_min_size(pos2(bx.min.x + 1.0, bx.min.y + 1.0), vec2(total_w - 2.0, head_h));
        pt.rect_filled(hr, CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, p.surf);
        draw_row(pt, hr.min.y, head, true, true);
    }
    ui.painter().rect_stroke(bx, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
    hit
}

// --- Files ------------------------------------------------------------------

pub fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The system's save dialog, on its own thread (it blocks): the path chosen, or None.
pub fn pick_save(ctx: &egui::Context, name: &str, dir: Option<std::path::PathBuf>) -> crate::job::Job<Option<std::path::PathBuf>> {
    let name = name.to_string();
    crate::job::Job::spawn(ctx, move |_, _| {
        let mut d = rfd::FileDialog::new().set_file_name(name);
        if let Some(dir) = dir { d = d.set_directory(dir); }
        d.save_file()
    })
}

/// The system's dialog to pick files to open.
pub fn pick_files(ctx: &egui::Context, dir: Option<std::path::PathBuf>) -> crate::job::Job<Vec<std::path::PathBuf>> {
    crate::job::Job::spawn(ctx, move |_, _| {
        let mut d = rfd::FileDialog::new().add_filter("CSV and text", &["csv", "tsv", "txt", "tab", "psv", "dat", "log"]).add_filter("All files", &["*"]);
        if let Some(dir) = dir { d = d.set_directory(dir); }
        d.pick_files().unwrap_or_default()
    })
}

/// The system's dialog to pick a folder.
pub fn pick_folder(ctx: &egui::Context, dir: Option<std::path::PathBuf>) -> crate::job::Job<Option<std::path::PathBuf>> {
    crate::job::Job::spawn(ctx, move |_, _| {
        let mut d = rfd::FileDialog::new();
        if let Some(dir) = dir { d = d.set_directory(dir); }
        d.pick_folder()
    })
}
