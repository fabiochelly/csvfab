//! The function picker (ƒx): every helper of formulas, the operators and the
//! columns of the tab, searched and sorted by family; for the entry chosen its
//! signature, what it does and its examples — each run on the selected row
//! (else the first shown) with a real column of the right kind in place of
//! {N} {D} {T} {T2}, so the result shown is the one the formula will give.
//! Inserting puts the entry at the caret of the box it was opened from (the
//! computed column's formula, or the search box, which then turns to
//! Expression) and selects its first argument, to be typed over.
//!
//! The picker is a modal over everything: Escape and the arrows are its own
//! while it is open (Escape must not close the computed column below).

use super::{Command, Cx, Tool};
use crate::icons::{self, Icon};
use crate::tab::Tab;
use crate::theme::{alpha, Metrics, Palette};
use csvfab_engine::formula::{self, doc::{self, Entry, PickerItem}};
use csvfab_engine::view::Kind;
use csvfab_engine::RowId;
use eframe::egui::{self, pos2, text::LayoutJob, vec2, Align2, Color32, FontFamily, FontId, Id, Key, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui};

/// What the user picked: an entry (inserted as `doc::insertion` says) or an example's text.
pub enum Pick {
    Entry(Entry),
    Text(String),
}

pub enum Out {
    Close,
    Insert(Pick),
}

pub struct Picker {
    q: String,
    fam: usize,
    act: usize,
    items: Vec<PickerItem>,
    stale: bool,
    /// Scroll the list to the entry chosen (after a key, not a hover).
    reveal: bool,
    detail: Option<Detail>,
}

/// The detail pane of one entry on one row, computed once (its examples run formulas).
struct Detail {
    key: (String, Option<RowId>, u64),
    head: String,
    alt: Option<String>,
    kind: Option<Kind>,
    fam: String,
    desc: Vec<(String, bool)>,
    examples: Vec<(String, String, Option<Result<String, String>>)>,
    row_note: Option<String>,
}

pub fn ui_font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(crate::fonts::MONO_BOLD.into()))
}

impl Picker {
    /// Opened on the entry named `name`, if any (the autocomplete's "more about").
    pub fn new(name: Option<&str>, tab: &Tab) -> Picker {
        let mut p = Picker { q: String::new(), fam: 0, act: 0, items: Vec::new(), stale: true, reveal: true, detail: None };
        p.refresh(tab);
        if let Some(n) = name {
            if let Some(k) = p.items.iter().position(|x| matches!(&x.entry, Entry::Doc(d) if d.name == n)) {
                p.act = k;
            }
        }
        p
    }

    /// Opened with a search typed (the screenshot hooks).
    pub fn set_query(&mut self, q: &str, tab: &Tab) {
        self.q = q.to_string();
        self.act = 0;
        self.refresh(tab);
    }

    fn refresh(&mut self, tab: &Tab) {
        let headers = tab.st().map(|s| s.headers()).unwrap_or_default();
        self.items = doc::picker(&self.q, doc::TABS[self.fam], &headers);
        self.act = self.act.min(self.items.len().saturating_sub(1));
        self.stale = false;
    }

    /// Draws the picker; returns what the user did, if anything.
    pub fn show(&mut self, ctx: &egui::Context, tab: &Tab, p: &Palette, m: &Metrics) -> Option<Out> {
        let _ = m;
        let mut out = None;
        // Its keys first, before the search field sees them.
        let n = self.items.len();
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                out = Some(Out::Close);
            }
            if n > 0 && i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                self.act = (self.act + 1) % n;
                self.reveal = true;
            }
            if n > 0 && i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.act = (self.act + n - 1) % n;
                self.reveal = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Enter) {
                if let Some(x) = self.items.get(self.act) {
                    out = Some(Out::Insert(Pick::Entry(x.entry.clone())));
                }
            }
            let k = doc::TABS.len();
            if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                self.fam = (self.fam + k - 1) % k;
                (self.act, self.stale, self.reveal) = (0, true, true);
            } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                self.fam = (self.fam + 1) % k;
                (self.act, self.stale, self.reveal) = (0, true, true);
            }
        });
        if self.stale {
            self.refresh(tab);
        }
        let screen = ctx.content_rect();
        let (w, h) = ((screen.width() - 32.0).min(900.0), (screen.height() - 64.0).min(580.0));
        let frame = egui::Frame::new()
            .fill(p.surf)
            .stroke(Stroke::new(1.0, p.brd_2))
            .corner_radius(14)
            .inner_margin(0)
            .shadow(egui::Shadow { offset: [0, 28], blur: 70, spread: 0, color: Color32::from_black_alpha(153) });
        let area = egui::Modal::default_area(Id::unique("fx-picker")).anchor(Align2::CENTER_TOP, vec2(0.0, (screen.height() * 0.07).round()));
        let modal = egui::Modal::new(Id::unique("fx-picker")).area(area).frame(frame).backdrop_color(alpha(p.bg_deep, 0.55)).show(ctx, |ui| {
            ui.set_min_size(vec2(w, h));
            ui.set_max_size(vec2(w, h));
            let r = ui.max_rect();
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            // Search row: ƒx logo, the field; a rule under it.
            let row = Rect::from_min_size(r.min, vec2(w, 50.0));
            icons::paint(ui.painter(), Icon::Fx, Rect::from_min_size(pos2(row.min.x + 16.0, row.center().y - 11.0), vec2(25.0, 22.0)), p.accent_2);
            let field = Rect::from_min_max(pos2(row.min.x + 16.0 + 25.0 + 10.0, row.min.y), pos2(row.max.x - 16.0, row.max.y));
            let te = egui::TextEdit::singleline(&mut self.q)
                .id(Id::unique("fxp-in"))
                .font(ui_font(15.0))
                .text_color(p.txt_hi)
                .frame(egui::Frame::NONE)
                .event_filter(egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
                .hint_text(egui::RichText::new("Search a function, a column, an operator…").font(ui_font(15.0)).color(p.txt_dim));
            let resp = crate::widgets::centered_edit(ui, field, te);
            if !resp.has_focus() {
                resp.request_focus();
            }
            if resp.changed() {
                (self.act, self.stale, self.reveal) = (0, true, true);
            }
            ui.painter().hline(r.x_range(), row.max.y + 0.5, Stroke::new(1.0, p.brd));
            // Families.
            let tabs_r = Rect::from_min_size(pos2(r.min.x, row.max.y + 1.0), vec2(w, 8.0 + 22.0 + 8.0));
            let mut x = tabs_r.min.x + 12.0;
            for (k, name) in doc::TABS.iter().enumerate() {
                let g = ui.painter().layout_no_wrap(name.to_string(), ui_font(12.0), p.txt_mut);
                let tr = Rect::from_min_size(pos2(x, tabs_r.min.y + 8.0), vec2(g.size().x + 22.0, 22.0));
                let t = ui.interact(tr, Id::unique(("fxp-tab", k)), Sense::click());
                let on = k == self.fam;
                if on {
                    ui.painter().rect_filled(tr, 12.0, alpha(p.accent, 0.16));
                } else if t.hovered() {
                    ui.painter().rect_filled(tr, 12.0, alpha(p.gray, 0.15));
                }
                let c = if on || t.hovered() { p.txt_hi } else { p.txt_mut };
                ui.painter().galley(pos2(tr.min.x + 11.0, (tr.center().y - g.size().y / 2.0).round()), g, c);
                if t.clicked() {
                    self.fam = k;
                    (self.act, self.stale, self.reveal) = (0, true, true);
                }
                x = tr.max.x + 4.0;
            }
            ui.painter().hline(r.x_range(), tabs_r.max.y + 0.5, Stroke::new(1.0, p.brd));
            // Footer.
            let foot = Rect::from_min_max(pos2(r.min.x, r.max.y - 33.0), r.max);
            ui.painter().hline(r.x_range(), foot.min.y + 0.5, Stroke::new(1.0, p.brd));
            let mut x = foot.min.x + 16.0;
            for (keys, what) in [(&["↑", "↓"][..], "choose"), (&["Tab"][..], "next family"), (&["Enter"][..], "insert"), (&["Esc"][..], "close")] {
                for k in keys {
                    x = kbd(ui, p, pos2(x, foot.center().y + 0.5), k) + 3.0;
                }
                let g = ui.painter().layout_no_wrap(what.to_string(), ui_font(11.5), p.txt_dim);
                let gw = g.size().x;
                ui.painter().galley(pos2(x + 1.5, (foot.center().y + 0.5 - g.size().y / 2.0).round()), g, p.txt_dim);
                x += gw + 1.5 + 18.0;
            }
            // Body: the list, the detail.
            let body = Rect::from_min_max(pos2(r.min.x, tabs_r.max.y + 1.0), pos2(r.max.x, foot.min.y));
            if self.stale {
                self.refresh(tab);
            }
            let lw = (w * 0.38).round();
            let list_r = Rect::from_min_max(body.min, pos2(body.min.x + lw, body.max.y));
            ui.painter().vline(list_r.max.x - 0.5, body.y_range(), Stroke::new(1.0, p.brd));
            let mut list = ui.new_child(egui::UiBuilder::new().max_rect(list_r.shrink2(vec2(0.0, 0.0))).id_salt("fxp-list"));
            if let Some(o) = self.list(&mut list, tab, p) {
                out = Some(o);
            }
            let det_r = Rect::from_min_max(pos2(list_r.max.x, body.min.y), body.max);
            let mut det = ui.new_child(egui::UiBuilder::new().max_rect(det_r).id_salt("fxp-det"));
            if let Some(o) = self.detail_pane(&mut det, tab, p) {
                out = Some(o);
            }
        });
        if modal.backdrop_response.clicked() {
            out = Some(Out::Close);
        }
        out
    }

    fn list(&mut self, ui: &mut Ui, tab: &Tab, p: &Palette) -> Option<Out> {
        let mut out = None;
        let headers = tab.st().map(|s| s.headers()).unwrap_or_default();
        let show_fam = self.fam == 0 || !self.q.trim().is_empty();
        egui::ScrollArea::vertical().id_salt("fxp-list-scroll").auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(6).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                if self.items.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::hover());
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, "Nothing matches.", ui_font(13.0), p.txt_dim);
                }
                for (i, x) in self.items.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                    if !ui.is_rect_visible(r) && !(self.reveal && i == self.act) {
                        continue;
                    }
                    let act = i == self.act;
                    let pt = ui.painter();
                    if act {
                        pt.rect_filled(r, 8.0, alpha(p.accent, 0.14));
                        pt.rect_filled(Rect::from_min_max(pos2(r.min.x, r.min.y + 7.0), pos2(r.min.x + 3.0, r.max.y - 7.0)), 2.0, p.prim);
                        if self.reveal {
                            ui.scroll_to_rect(r, None);
                        }
                    }
                    let fg = if act { p.txt_hi } else { p.txt };
                    let mut lx = r.min.x + 12.0;
                    let (label, fam, font, bold) = match &x.entry {
                        Entry::Column(c) => {
                            let kind = tab.kinds.get(*c).copied().unwrap_or(Kind::Text);
                            lx += type_icon(pt, p, pos2(lx, r.center().y), kind) + 7.0;
                            (headers.get(*c).cloned().unwrap_or_default(), "Columns", ui_font(13.5), crate::grid::semibold(13.5))
                        }
                        Entry::Doc(d) => (d.name.to_string(), d.fam, mono(13.0), mono_bold(13.0)),
                    };
                    let mut right = r.max.x - 12.0;
                    if show_fam {
                        right = cg_tag(pt, p, pos2(right, r.center().y), fam) - 12.0;
                    }
                    let mut job = marked(&label, &x.hits, font, bold, fg, p.accent_2);
                    job.wrap = egui::text::TextWrapping { max_width: (right - lx).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                    let g = pt.layout_job(job);
                    pt.galley(pos2(lx, (r.center().y - g.size().y / 2.0).round()), g, fg);
                    if resp.double_clicked() && act {
                        out = Some(Out::Insert(Pick::Entry(x.entry.clone())));
                    } else if resp.clicked() {
                        self.act = i;
                    }
                }
            });
        });
        self.reveal = false;
        out
    }

    fn detail_pane(&mut self, ui: &mut Ui, tab: &Tab, p: &Palette) -> Option<Out> {
        let Some(x) = self.items.get(self.act) else { return None };
        let row = example_row(tab);
        let name = match &x.entry { Entry::Doc(d) => d.name.to_string(), Entry::Column(c) => format!("{{{c}}}") };
        let key = (name, row.map(|r| r.0), tab.generation);
        let entry = x.entry.clone();
        if self.detail.as_ref().is_none_or(|d| d.key != key) {
            self.detail = Some(detail(tab, &entry, row, key));
        }
        let d = self.detail.as_ref().unwrap();
        let mut out = None;
        egui::ScrollArea::vertical().id_salt(("fxp-det-scroll", self.act, self.fam)).auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 18 }).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let w = ui.available_width();
                // Signature, family tag.
                let (sr, _) = ui.allocate_exact_size(vec2(w, 0.0), Sense::hover());
                let mut x0 = sr.min.x;
                let tag_w = {
                    let g = ui.painter().layout_no_wrap(d.fam.clone(), ui_font(11.0), p.txt_dim);
                    g.size().x + 14.0
                };
                if let Some(k) = d.kind {
                    x0 += type_icon(ui.painter(), p, pos2(x0, sr.min.y + 11.0), k) + 10.0;
                }
                let job = LayoutJob { wrap: egui::text::TextWrapping { max_width: w - (x0 - sr.min.x) - tag_w - 10.0, ..Default::default() }, ..LayoutJob::simple(d.head.clone(), mono_bold(14.5), p.txt_hi, f32::INFINITY) };
                let g = ui.painter().layout_job(job);
                let gh = g.size().y.max(20.0);
                let (hr, _) = ui.allocate_exact_size(vec2(w, gh), Sense::hover());
                ui.painter().galley(pos2(x0, hr.min.y + (gh - g.size().y) / 2.0), g, p.txt_hi);
                cg_tag(ui.painter(), p, pos2(hr.max.x, hr.min.y + 10.0), &d.fam);
                if let Some(alt) = &d.alt {
                    ui.add_space(4.0);
                    let mut job = LayoutJob::default();
                    job.append("also ", 0.0, egui::TextFormat::simple(ui_font(12.0), p.txt_dim));
                    job.append(alt, 0.0, egui::TextFormat::simple(mono(12.0), p.txt_dim));
                    job.wrap.max_width = w;
                    ui.label(job);
                }
                ui.add_space(10.0);
                let mut job = LayoutJob::default();
                for (t, code) in &d.desc {
                    let mut f = egui::TextFormat::simple(if *code { mono(13.0) } else { ui_font(13.0) }, if *code { p.txt_hi } else { p.txt_mut });
                    f.line_height = Some(13.0 * 1.55);
                    job.append(t, 0.0, f);
                }
                job.wrap.max_width = w;
                ui.label(job);
                ui.add_space(16.0);
                for (src, note, res) in &d.examples {
                    if let Some(o) = example_box(ui, p, w, src, note, res.as_ref()) {
                        out = Some(o);
                    }
                    ui.add_space(8.0);
                }
                if let Some(n) = &d.row_note {
                    ui.add_space(4.0);
                    let mut job = LayoutJob::simple(n.clone(), ui_font(11.5), p.txt_dim, w);
                    job.sections[0].format.line_height = Some(11.5 * 1.5);
                    ui.label(job);
                }
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if button(ui, p, "Insert", true, true).clicked() {
                        out = Some(Out::Insert(Pick::Entry(entry.clone())));
                    }
                });
            });
        });
        out
    }
}

/// An example: its formula, then "= result" and what it shows; a click inserts it.
fn example_box(ui: &mut Ui, p: &Palette, w: f32, src: &str, note: &str, res: Option<&Result<String, String>>) -> Option<Out> {
    let inner = w - 22.0 - 2.0;
    let code = ui.painter().layout(src.to_string(), mono(12.5), p.txt_hi, inner);
    let h = 8.0 + code.size().y + 4.0 + 16.0 + 8.0 + 2.0;
    let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let hot = resp.hovered();
    ui.painter().rect(r, 8.0, if hot { alpha(p.accent, 0.08) } else { Color32::TRANSPARENT }, Stroke::new(1.0, if hot { alpha(p.accent, 0.5) } else { p.brd }), StrokeKind::Inside);
    let x0 = r.min.x + 12.0;
    let y1 = r.min.y + 9.0 + code.size().y + 4.0;
    ui.painter().galley(pos2(x0, r.min.y + 9.0), code, p.txt_hi);
    let cy = y1 + 8.0;
    let mut x = x0;
    let note_g = ui.painter().layout_no_wrap(note.to_string(), ui_font(12.0), p.txt_dim);
    let note_w = note_g.size().x;
    ui.painter().galley(pos2(r.max.x - 12.0 - note_w, (cy - note_g.size().y / 2.0).round()), note_g, p.txt_dim);
    if let Some(res) = res {
        let g = ui.painter().layout_no_wrap("=".into(), mono(12.0), p.txt_dim);
        let gw = g.size().x;
        ui.painter().galley(pos2(x, (cy - g.size().y / 2.0).round()), g, p.txt_dim);
        x += gw + 6.0;
        let (text, font, c) = match res {
            Ok(v) if v.is_empty() => ("empty".to_string(), FontId::new(12.0, FontFamily::Proportional), p.txt_dim),
            Ok(v) => (v.clone(), mono(12.0), p.ok_txt),
            Err(e) => (e.clone(), mono(12.0), p.danger_txt),
        };
        let mut job = LayoutJob::simple_singleline(text, font, c);
        job.wrap = egui::text::TextWrapping { max_width: (r.max.x - 12.0 - note_w - 10.0 - x).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        if matches!(res, Ok(v) if v.is_empty()) {
            job.sections[0].format.italics = true;
        }
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(x, (cy - g.size().y / 2.0).round()), g, c);
    }
    resp.clicked().then(|| Out::Insert(Pick::Text(src.to_string())))
}

/// A button (`.btn`, or `.btn-outline` when not `primary`): ui 500 13, padding 6 × 14, 30 tall, radius 6; disabled at .45.
pub fn button(ui: &mut Ui, p: &Palette, text: &str, primary: bool, enabled: bool) -> egui::Response {
    let f = FontId::new(13.0, FontFamily::Name(crate::fonts::UI_MEDIUM.into()));
    let g = ui.painter().layout_no_wrap(text.to_string(), f, Color32::WHITE);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 28.0, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    let hot = enabled && resp.hovered();
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.45) };
    let (fill, stroke, fg) = if primary {
        (if hot { p.prim_hover } else { p.prim }, Stroke::NONE, p.on_prim)
    } else {
        (if hot { p.surf_2 } else { Color32::TRANSPARENT }, Stroke::new(1.0, if hot { p.brd_2 } else { p.brd }), p.txt)
    };
    ui.painter().rect(r, 6.0, fade(fill), Stroke::new(stroke.width, fade(stroke.color)), StrokeKind::Inside);
    ui.painter().galley(pos2((r.center().x - g.size().x / 2.0).round(), (r.center().y - g.size().y / 2.0).round()), g, fade(fg));
    resp
}

/// A key cap (`kbd`): mono 11, padding 1 × 5, radius 4; returns its right edge.
pub fn kbd(ui: &Ui, p: &Palette, left_center: egui::Pos2, k: &str) -> f32 {
    let g = ui.painter().layout_no_wrap(k.to_string(), mono(11.0), p.txt_mut);
    let r = Rect::from_min_size(pos2(left_center.x, (left_center.y - 9.0).round()), vec2(g.size().x + 12.0, 18.0));
    ui.painter().rect(r, 4.0, alpha(p.hi, 0.06), Stroke::new(1.0, alpha(p.hi, 0.2)), StrokeKind::Inside);
    ui.painter().galley(pos2(r.min.x + 6.0, (r.center().y - g.size().y / 2.0).round()), g, p.txt_mut);
    r.max.x
}

/// A family tag (`.cg`), its right edge at `right_center`; returns its left edge.
pub fn cg_tag(pt: &egui::Painter, p: &Palette, right_center: egui::Pos2, text: &str) -> f32 {
    let g = pt.layout_no_wrap(text.to_string(), ui_font(11.0), p.txt_dim);
    let r = Rect::from_min_size(pos2(right_center.x - g.size().x - 14.0, (right_center.y - 9.0).round()), vec2(g.size().x + 14.0, 18.0));
    pt.rect_filled(r, 9.0, alpha(p.gray, 0.15));
    pt.galley(pos2(r.min.x + 7.0, (r.center().y - g.size().y / 2.0).round()), g, p.txt_dim);
    r.min.x
}

/// A column's kind (`.ty`): # / calendar / Aa on a tint, 16 tall, radius 4; returns its width.
pub fn type_icon(pt: &egui::Painter, p: &Palette, left_center: egui::Pos2, kind: Kind) -> f32 {
    let f = mono_bold(10.0);
    let (text, fg, bg) = match kind {
        Kind::Number => ("#", p.cyan_txt, alpha(p.cyan, 0.13)),
        Kind::Date => ("", p.violet_txt, alpha(p.violet, 0.13)),
        Kind::Text => ("Aa", p.txt_dim, alpha(p.gray, 0.16)),
    };
    let w = if text.is_empty() { 17.0 } else { (pt.layout_no_wrap(text.into(), f.clone(), fg).size().x + 6.0).max(16.0) };
    let r = Rect::from_min_size(pos2(left_center.x, (left_center.y - 8.0).round()), vec2(w, 16.0));
    pt.rect_filled(r, 4.0, bg);
    if text.is_empty() {
        icons::paint(pt, Icon::Calendar, Rect::from_center_size(r.center(), vec2(11.0, 11.0)), fg);
    } else {
        pt.text(r.center(), Align2::CENTER_CENTER, text, f, fg);
    }
    w
}

/// A label with its matched characters (char indexes) in bold `hit`.
pub fn marked(label: &str, hits: &[usize], font: FontId, bold: FontId, c: Color32, hit: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut buf = String::new();
    let mut on = false;
    let flush = |job: &mut LayoutJob, buf: &mut String, on: bool| {
        if !buf.is_empty() {
            job.append(buf, 0.0, egui::TextFormat::simple(if on { bold.clone() } else { font.clone() }, if on { hit } else { c }));
            buf.clear();
        }
    };
    for (i, ch) in label.chars().enumerate() {
        let h = hits.contains(&i);
        if h != on {
            flush(&mut job, &mut buf, on);
            on = h;
        }
        buf.push(ch);
    }
    flush(&mut job, &mut buf, on);
    job
}

/// The row the examples run on: the selected one, else the first shown; and whether it is selected.
fn example_row(tab: &Tab) -> Option<(RowId, bool)> {
    let n = tab.shown();
    if tab.table.is_none() || n == 0 {
        return None;
    }
    match tab.grid.cursor.filter(|c| c.row < n) {
        Some(c) => Some((tab.shown_id(c.row), true)),
        None => Some((tab.shown_id(0), false)),
    }
}

fn selected_col(tab: &Tab) -> Option<usize> {
    tab.grid.cursor.map(|c| c.col)
}

/// The column a value of `kind` most likely comes from (see `doc::likely_column`).
fn likely(tab: &Tab, kind: Kind, pick: Option<doc::Pick>) -> Option<usize> {
    let st = tab.st()?;
    doc::likely_column(st, &tab.kinds, kind, pick, selected_col(tab), &|c| tab.grid.hidden.contains(&c))
}

/// {N} {D} {T} {T2} → columns of the file of that kind; the web app's stand-ins without a file.
pub fn example(tab: &Tab, src: &str, pick: Option<doc::Pick>) -> String {
    let Some(st) = tab.st() else { return doc::fill_example_default(src) };
    let headers = st.headers();
    if headers.is_empty() {
        return doc::fill_example_default(src);
    }
    let (n, d, x) = (likely(tab, Kind::Number, None), likely(tab, Kind::Date, None), likely(tab, Kind::Text, pick));
    // Columns the grid shows, as the web's (a hidden one makes an example nobody sees).
    let shown = |c: &usize| !tab.grid.hidden.contains(c);
    let x2 = (0..tab.kinds.len()).filter(shown).find(|&c| tab.kinds[c] == Kind::Text && Some(c) != x);
    let first = (0..headers.len()).find(shown).unwrap_or(0);
    let name = |c: Option<usize>, fb: usize| headers.get(c.unwrap_or(fb)).cloned().unwrap_or_default();
    doc::fill_example(src, &name(n, first), &name(d, first), &name(x, first), &name(x2, x.unwrap_or(first)))
}

/// The column put in an entry's first argument on insert ("{City}"), if any.
pub fn arg_column(tab: &Tab, e: &Entry) -> Option<String> {
    let Entry::Doc(d) = e else { return None };
    let c = likely(tab, doc::arg_kind(d)?, d.pick)?;
    tab.st().and_then(|st| st.headers().get(c).map(|h| format!("{{{h}}}")))
}

/// What a formula gives on a row: its value, or why not.
pub fn try_formula(tab: &Tab, id: RowId, src: &str) -> Result<String, String> {
    let st = tab.st().ok_or_else(|| "no file".to_string())?;
    let mut e = formula::compile(src, &st.headers()).map_err(|e| if e.msg.is_empty() { "empty".into() } else { e.msg })?;
    e.set_decimal_comma(formula::decimal_comma(st));
    e.eval_row(st, id)
}

fn detail(tab: &Tab, entry: &Entry, row: Option<(RowId, bool)>, key: (String, Option<RowId>, u64)) -> Detail {
    let headers = tab.st().map(|s| s.headers()).unwrap_or_default();
    let res = |src: &str| row.map(|(id, _)| try_formula(tab, id, src));
    let row_note = row.map(|(id, sel)| {
        let rec = tab.st().map_or(id as usize, |st| (id as usize + 1).saturating_sub(st.header as usize));
        format!("Results on row {}{}. Click an example to insert it.", crate::grid::fmt_int(rec), if sel { ", the one selected" } else { ", the first shown — select a cell to try another" })
    });
    match entry {
        Entry::Column(c) => {
            let s = format!("{{{}}}", headers.get(*c).cloned().unwrap_or_default());
            let kind = tab.kinds.get(*c).copied().unwrap_or(Kind::Text);
            let ex = match kind {
                Kind::Number => vec![format!("{s}.num() > 0"), format!("{s}.num().round()")],
                Kind::Date => vec![format!("{s}.year()"), format!("{s}.days(today())")],
                Kind::Text => vec![s.clone(), format!("{s}.upper()"), format!("{s}.empty()")],
            };
            Detail {
                key,
                head: s.clone(),
                alt: None,
                kind: Some(kind),
                fam: "Columns".into(),
                desc: vec![
                    ("A column: in a formula, ".into(), false),
                    (s.clone(), true),
                    (" is the row's cell as text. Read it with ".into(), false),
                    ("num()".into(), true),
                    (" or ".into(), false),
                    ("date()".into(), true),
                    (" to compute, or use its string methods: ".into(), false),
                    (format!("{s}.startsWith(\"A\")"), true),
                    (".".into(), false),
                ],
                examples: ex.into_iter().map(|src| { let r = res(&src); (src, String::new(), r) }).collect(),
                row_note,
            }
        }
        Entry::Doc(d) => {
            let ms = if d.fam != "Columns" { doc::method_sig(d) } else { None };
            let head = match &ms {
                Some(ms) => format!("{}{ms}", arg_column(tab, entry).unwrap_or_else(|| "{Column}".into())),
                None => d.sig.trim().to_string(),
            };
            Detail {
                key,
                head,
                alt: ms.as_ref().map(|_| d.sig.to_string()),
                kind: None,
                fam: d.fam.to_string(),
                desc: vec![(d.desc.to_string(), false)],
                examples: d.ex.iter().map(|(src, note)| { let f = example(tab, src, d.pick); let r = res(&f); (f, note.to_string(), r) }).collect(),
                row_note,
            }
        }
    }
}

// ---- Inserting into a text box -------------------------------------------

pub fn char_to_byte(s: &str, c: usize) -> usize {
    s.char_indices().nth(c).map_or(s.len(), |x| x.0)
}

pub fn byte_to_char(s: &str, b: usize) -> usize {
    s[..b.min(s.len())].chars().count()
}

/// Puts a pick into `text` over the selection `sel` (char indexes); returns the
/// selection after it (chars): the argument to type over, else the caret after.
pub fn insert(text: &mut String, sel: (usize, usize), pick: &Pick, tab: &Tab) -> (usize, usize) {
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let (ba, bb) = (char_to_byte(text, a), char_to_byte(text, b));
    let (ins, rel) = match pick {
        Pick::Text(s) => (s.clone(), None),
        Pick::Entry(e) => {
            let headers = tab.st().map(|s| s.headers()).unwrap_or_default();
            doc::insertion(e, &headers, &text[..ba], arg_column(tab, e).as_deref())
        }
    };
    text.replace_range(ba..bb, &ins);
    match rel {
        Some((x, y)) => (a + byte_to_char(&ins, x), a + byte_to_char(&ins, y)),
        None => {
            let e = a + ins.chars().count();
            (e, e)
        }
    }
}

/// Selects chars a..b of the text box `id` (and gives it the focus).
pub fn select(ctx: &egui::Context, id: Id, sel: (usize, usize)) {
    let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    s.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(sel.0), egui::text::CCursor::new(sel.1))));
    s.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// The selection of the text box `id` (chars), or the end of `text`.
pub fn selection(ctx: &egui::Context, id: Id, text: &str) -> (usize, usize) {
    let n = text.chars().count();
    egui::TextEdit::load_state(ctx, id)
        .and_then(|s| s.cursor.char_range())
        .map_or((n, n), |r| (usize::from(r.secondary.index).min(n), usize::from(r.primary.index).min(n)))
}

// ---- The picker on the search box ------------------------------------------

/// The search box's id (chrome.rs).
pub fn search_id() -> Id {
    Id::unique("global-search")
}

struct SearchPicker {
    picker: Picker,
    sel: Option<(usize, usize)>,
}

impl Tool for SearchPicker {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let sel = *self.sel.get_or_insert_with(|| selection(cx.ctx, search_id(), &cx.tab.filter.global));
        match self.picker.show(cx.ctx, cx.tab, cx.p, cx.m) {
            None => true,
            Some(Out::Close) => {
                cx.ctx.memory_mut(|m| m.request_focus(search_id()));
                false
            }
            Some(Out::Insert(pick)) => {
                let tab = &mut *cx.tab;
                let mut text = std::mem::take(&mut tab.filter.global);
                let s = insert(&mut text, sel, &pick, tab);
                tab.filter.global = text;
                tab.expr_mode = true;
                tab.grid.scroll = 0.0;
                tab.refresh(cx.ctx);
                select(cx.ctx, search_id(), s);
                false
            }
        }
    }
}

/// The ƒx button left of the search box (and the palette): the picker, inserting into the search.
pub fn open_search(cx: &mut Cx) {
    let mut picker = Picker::new(None, cx.tab);
    if let Some(q) = crate::shot::take("fxq") {
        picker.set_query(&q, cx.tab);
    }
    cx.out.push(super::Cmd::Tool(Box::new(SearchPicker { picker, sel: None })));
}

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Insert a function (ƒx)", group: "View", keys: None, enabled: super::has_table, run: open_search }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_over_selection() {
        let tab = Tab::blank();
        let mut s = String::from("é + x");
        let r = insert(&mut s, (4, 5), &Pick::Text("{Amount}".into()), &tab);
        assert_eq!(s, "é + {Amount}");
        assert_eq!(r, (12, 12));
        let mut s = String::new();
        let left = Entry::Doc(doc::doc_of("left").unwrap());
        let r = insert(&mut s, (0, 0), &Pick::Entry(left), &tab);
        // No file: {Column} in front, selected to be typed over.
        assert_eq!(&s[..8], "{Column}");
        assert_eq!(r, (0, 8));
    }

    #[test]
    fn marks_hits() {
        let j = marked("abc", &[1], mono(13.0), mono_bold(13.0), Color32::WHITE, Color32::RED);
        assert_eq!(j.sections.len(), 3);
        assert_eq!(j.text, "abc");
    }
}
