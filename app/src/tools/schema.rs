//! Schemas (ui/js/41-schema.js): a file's rules kept beside it as
//! `<stem>.schema.json` in Table Schema, so other tools check the same file.
//! - "Create a schema…": every row read in a `Job` (`schema::infer`), then the
//!   draft editor — one line per column: its type, its rules (ticked or not,
//!   their values typed in place), "+ rule" filled in from the values; a rule
//!   added or edited is counted at once on the whole file, in the background.
//! - "Validate against a schema…": the schema beside the file (its own or its
//!   family's, `find_beside`) or one picked by hand, checked in a `Job`; the
//!   report lists each kind of error with its count and examples, a click
//!   shows its rows (restricted, cells at fault marked with the reason).

use super::analysis_kit as kit;
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::grid::semibold;
use crate::job::Job;
use crate::tab::Marks;
use crate::theme::{alpha, Palette};
use csvfab_engine::schema::{self as sc, Check, Draft, Field, Inferred, Rule, Validation};
use csvfab_engine::State;
use eframe::egui::{self, pos2, vec2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Ui};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "Create a schema…", group: "File", keys: None, enabled: has_table, run: |cx| { if let Some(t) = Create::new(cx) { cx.out.push(Cmd::Tool(Box::new(t))); } } },
        Command { name: "Validate against a schema…", group: "File", keys: None, enabled: has_table, run: |cx| { if let Some(t) = Validate::new(cx) { cx.out.push(Cmd::Tool(Box::new(t))); } } },
    ]
}

// ---- The dialog's frame and parts ---------------------------------------------

/// `#modal-schema`: a `.modal-box` min(980, window − 40) wide and 90 % of the window tall.
fn window<R>(cx: &mut Cx, id: &str, add: impl FnOnce(&mut Ui, &mut Cx, f32) -> R) -> (R, bool) {
    let p = cx.p;
    let frame = egui::Frame::new().fill(p.surf).stroke(Stroke::new(1.0, p.brd)).corner_radius(8).inner_margin(20)
        .shadow(egui::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(128) });
    let ctx = cx.ctx.clone();
    let screen = ctx.content_rect();
    let (w, h) = ((980f32.min(screen.width() - 40.0) - 40.0).max(0.0), (screen.height() * 0.9 - 40.0).max(0.0));
    let r = crate::widgets::modal(&ctx, egui::Id::unique(("schema-dialog", id)), p, frame, |ui| {
        ui.set_width(w);
        ui.set_height(h);
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        add(ui, cx, h)
    });
    let close = r.should_close();
    (r.inner, !close)
}

/// A `.stat-line`: ui 12 `txt_mut`, line 1.6; `**…**` in ui 600 `txt_hi`.
fn stat_line(ui: &mut Ui, job: egui::text::LayoutJob) {
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.min.x, kit::snap(ui, r.min.y)), g, Color32::WHITE);
}

/// Text in parts of their own colour, wrapped to `width`, ui 12, line 1.6.
fn parts_job(parts: &[(&str, Color32, bool)], width: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    for (t, c, bold) in parts {
        // `**…**` inside a part: ui 600 in `txt_hi` (`.stat-line b`) — the colour of the part's first char is kept otherwise.
        for (k, s) in t.split("**").enumerate() {
            if s.is_empty() { continue; }
            let b = *bold || k % 2 == 1;
            job.append(s, 0.0, egui::TextFormat {
                font_id: if b { semibold(12.0) } else { kit::ui(12.0) },
                color: *c,
                line_height: Some(19.0),
                valign: egui::Align::Center,
                ..Default::default()
            });
        }
    }
    job
}

/// The table's box (`.pv-wrap.pf-wrap`): 1 px `brd`, radius 6, the head row
/// fixed on `surf`, the body scrolling under it. `body` gets the columns' x offsets and widths.
fn table_box(ui: &mut Ui, p: &Palette, id: &str, height: f32, head: &[(&str, f32, bool)], body: impl FnOnce(&mut Ui, &[(f32, f32)])) {
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(120.0)), Sense::hover());
    ui.painter().rect_stroke(outer, 6.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
    let inner = outer.shrink(1.0);
    let cols = col_layout(head.iter().map(|h| h.1).collect::<Vec<_>>().as_slice(), inner.width());
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(("schema-table", id)));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    if !head.is_empty() {
        let hr = Rect::from_min_size(inner.min, vec2(inner.width(), 25.0));
        let pt = child.painter().clone();
        pt.rect_filled(hr, egui::CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, p.surf);
        for ((t, _, right), (x, w)) in head.iter().zip(&cols) {
            let cr = Rect::from_min_size(pos2(inner.min.x + x, hr.min.y), vec2(*w, 25.0));
            kit::cell_text(&child, &pt, cr, t, semibold(12.0), p.txt_hi, *right);
            rule_v(&child, &pt, cr.max.x, cr.y_range(), p);
        }
        rule_h(&child, &pt, hr.x_range(), hr.max.y, p);
        child.allocate_exact_size(vec2(inner.width(), 25.0), Sense::hover());
    }
    egui::ScrollArea::vertical().id_salt(("schema-scroll", id)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.set_width(inner.width());
        body(ui, &cols);
    });
}

/// Column widths: positive = fixed px, negative = share of the width, 0 = what is left (split evenly).
fn col_layout(spec: &[f32], total: f32) -> Vec<(f32, f32)> {
    let fixed: f32 = spec.iter().map(|&w| if w > 0.0 { w } else if w < 0.0 { -w * total } else { 0.0 }).sum();
    let rest = spec.iter().filter(|&&w| w == 0.0).count().max(1) as f32;
    let mut x = 0.0;
    spec.iter().map(|&w| {
        let w = if w > 0.0 { w } else if w < 0.0 { (-w * total).round() } else { ((total - fixed) / rest).max(40.0).floor() };
        let r = (x, w);
        x += w;
        r
    }).collect()
}

fn rule_v(ui: &Ui, pt: &egui::Painter, x: f32, y: egui::Rangef, p: &Palette) {
    pt.vline(kit::snap(ui, x) - 0.5, y, Stroke::new(1.0, p.brd));
}

fn rule_h(ui: &Ui, pt: &egui::Painter, x: egui::Rangef, y: f32, p: &Palette) {
    pt.hline(x, kit::snap(ui, y) - 0.5, Stroke::new(1.0, p.brd));
}

/// One table row of cells laid out by `cell(ui, column)` (padding 4 × 8, top
/// aligned), as tall as its tallest cell, with its rules. Returns the row's rect.
fn table_row(ui: &mut Ui, p: &Palette, cols: &[(f32, f32)], mut cell: impl FnMut(&mut Ui, usize)) -> Rect {
    let top = ui.cursor().min;
    let mut h: f32 = 0.0;
    for (c, (x, w)) in cols.iter().enumerate() {
        let r = Rect::from_min_size(pos2(top.x + x + 8.0, top.y + 4.0), vec2(w - 16.0, 4000.0));
        let mut cu = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
        cu.spacing_mut().item_spacing = vec2(0.0, 0.0);
        cell(&mut cu, c);
        h = h.max(cu.min_rect().height());
    }
    let total = cols.last().map_or(0.0, |(x, w)| x + w);
    let (row, _) = ui.allocate_exact_size(vec2(total, (h + 8.0 + 1.0).max(25.0)), Sense::hover());
    let pt = ui.painter();
    for (x, w) in cols { rule_v(ui, pt, top.x + x + w, row.y_range(), p); }
    rule_h(ui, pt, row.x_range(), row.max.y, p);
    row
}

/// A one-line text in a cell, ellipsed.
fn line(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> egui::Response {
    let w = ui.available_width();
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: w.max(4.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = ui.painter().layout_job(job);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x, 16.0), Sense::hover());
    ui.painter().galley(pos2(r.min.x, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, color);
    resp
}

/// A wrapped text in a cell.
fn wrapped(ui: &mut Ui, job: egui::text::LayoutJob) {
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y.max(16.0)), Sense::hover());
    ui.painter().galley(pos2(r.min.x, kit::snap(ui, r.min.y)), g, Color32::WHITE);
}

// ---- Rules as the dialog shows them ---------------------------------------------

/// How a rule's value is edited.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Value,
    Length,
    Pattern,
    List,
    Format,
}

fn kind_of(r: Rule) -> Option<Kind> {
    match r {
        Rule::Minimum | Rule::Maximum => Some(Kind::Value),
        Rule::MinLength | Rule::MaxLength => Some(Kind::Length),
        Rule::Pattern => Some(Kind::Pattern),
        Rule::Enum => Some(Kind::List),
        Rule::Format => Some(Kind::Format),
        _ => None,
    }
}

/// A rule's value as the dialog shows it (JS `scRuleText`).
fn rule_text(f: &Field, r: Rule) -> String {
    let k = f.constraints.as_ref();
    // A string as itself (a list's quoting only for a list); a macro: the JSON value's type is not named here.
    macro_rules! one { ($v:expr) => { match $v { None => String::new(), Some(v) => v.as_str().map_or_else(|| sc::list_text(std::slice::from_ref(v)), str::to_string) } } }
    match r {
        Rule::Format => f.format.clone().unwrap_or_default(),
        Rule::Enum => k.and_then(|k| k.en.as_deref()).map_or(String::new(), sc::list_text),
        Rule::Minimum => one!(k.and_then(|k| k.minimum.as_ref())),
        Rule::Maximum => one!(k.and_then(|k| k.maximum.as_ref())),
        Rule::MinLength => k.and_then(|k| k.min_length).map_or(String::new(), |x| x.to_string()),
        Rule::MaxLength => k.and_then(|k| k.max_length).map_or(String::new(), |x| x.to_string()),
        Rule::Pattern => k.and_then(|k| k.pattern.clone()).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Under the type: format, decimal comma, thousands, yes/no words (JS `extra`).
fn extra(f: &Field) -> String {
    let mut v = Vec::new();
    if let Some(fm) = f.format.as_deref().filter(|fm| f.ty() != "string" && *fm != "default") { v.push(format!("format {fm}")); }
    if f.decimal_char.as_deref() == Some(",") { v.push("decimal comma".to_string()); }
    if f.group_char.is_some() { v.push("spaces between thousands".to_string()); }
    if let Some(t) = &f.true_values { v.push(format!("{} = true", t.join("/"))); }
    v.join(" · ")
}

/// What is said beside a rule once counted.
#[derive(Clone, Debug, PartialEq)]
enum Count {
    Rows(usize),
    Note(&'static str),
}

/// (text, colour, tooltip) of a count (JS `count`).
fn count_label(c: &Count, p: &Palette, name: &str) -> (String, Color32, String) {
    match c {
        Count::Note(s) => (s.to_string(), p.txt_faint, String::new()),
        Count::Rows(0) => ("✓".into(), p.ok_txt, "Every row of this file follows it".into()),
        Count::Rows(n) => (format!("{} break it", kit::plural(*n, "row", "rows")), p.warn, format!("Rows of {name} that break this rule")),
    }
}

/// An editor's width in characters (JS `size`), + 2 as the web's `ch` width.
fn editor_chars(kind: Kind, val: &str) -> usize {
    let n = val.chars().count();
    2 + match kind {
        Kind::Length => 3,
        Kind::List => 36,
        Kind::Pattern => (n + 2).clamp(14, 40),
        _ => (n + 2).clamp(6, 14),
    }
}

/// The "+ rule" menu's entries for field i: (rule, arg, label, detail).
fn add_menu(d: &Draft, i: usize) -> Vec<(Rule, String, String, String)> {
    let can = d.addable(i);
    let h = &d.hints[i];
    let ok = |r: Rule| can.contains(&r);
    let mut out = Vec::new();
    if ok(Rule::Required) { out.push((Rule::Required, String::new(), "Required".into(), format!("filled in {} % of the rows", js_num((h.filled * 1000.0).round() / 10.0)))); }
    if ok(Rule::Unique) { out.push((Rule::Unique, String::new(), "Unique".into(), "no value twice".into())); }
    if ok(Rule::Minimum) { out.push((Rule::Minimum, String::new(), "Minimum".into(), h.min.as_ref().map_or(String::new(), |m| format!("smallest seen: {m}")))); }
    if ok(Rule::Maximum) { out.push((Rule::Maximum, String::new(), "Maximum".into(), h.max.as_ref().map_or(String::new(), |m| format!("largest seen: {m}")))); }
    if ok(Rule::MinLength) || ok(Rule::MaxLength) {
        out.push((Rule::MinLength, String::new(), "Length".into(), h.lmin.map_or(String::new(), |a| format!("from {a} to {} characters seen", h.lmax.unwrap_or(a)))));
    }
    if ok(Rule::Pattern) {
        for (pat, l) in h.patterns.iter().take(3) {
            let short = if pat.chars().count() > 48 { format!("{}…", pat.chars().take(47).collect::<String>()) } else { pat.clone() };
            out.push((Rule::Pattern, pat.clone(), "Pattern".into(), format!("{l}: {short}")));
        }
        out.push((Rule::Pattern, String::new(), "Pattern…".into(), "type your own, or pick a common one".into()));
    }
    if ok(Rule::Enum) { if let Some(v) = &h.values { out.push((Rule::Enum, String::new(), "List of values".into(), format!("the {} seen", kit::n(v.len())))); } }
    if ok(Rule::Format) { for (k, l) in sc::FORMATS { out.push((Rule::Format, k.to_string(), format!("Format: {l}"), String::new())); } }
    for c in Check::ALL {
        if ok(Rule::Check(c)) { let t = c.texts(); out.push((Rule::Check(c), String::new(), t[1].into(), t[2].into())); }
    }
    out
}

/// JS `String(x)` of a share: 97.5, 100.
fn js_num(x: f64) -> String {
    if x.fract() == 0.0 { format!("{}", x as i64) } else { format!("{x}") }
}

/// The dashed capsule of "+ rule" (`.sc-add`).
fn dashed_capsule(pt: &egui::Painter, r: Rect, c: Color32) {
    let rad = r.height() / 2.0;
    let mut pts = Vec::new();
    let arc = |cx: f32, a0: f32, pts: &mut Vec<egui::Pos2>| for k in 0..=8 {
        let a = a0 + std::f32::consts::PI * k as f32 / 8.0;
        pts.push(pos2(cx + rad * a.cos(), r.center().y + rad * a.sin()));
    };
    arc(r.max.x - rad, -std::f32::consts::FRAC_PI_2, &mut pts);
    arc(r.min.x + rad, std::f32::consts::FRAC_PI_2, &mut pts);
    pts.push(pts[0]);
    pt.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0, c), 3.0, 2.0));
}

// ---- Create ----------------------------------------------------------------------

struct Create {
    name: String,
    path: Option<PathBuf>,
    st: State,
    infer: Option<Job<Option<Inferred>>>,
    draft: Option<Draft>,
    counts: HashMap<(usize, Rule), Count>,
    counting: HashMap<(usize, Rule), Job<Option<usize>>>,
    /// Texts of the editors being typed in, and those that do not read (shown red).
    texts: HashMap<(usize, Rule), String>,
    bad: HashSet<(usize, Rule)>,
    focus: Option<(usize, Rule)>,
    /// Asking before a schema already beside the file is replaced.
    replace: Option<PathBuf>,
    saving: Option<Job<Option<PathBuf>>>,
    error: Option<String>,
    /// The file written: the dialog closes with a toast.
    written: Option<String>,
    shot_menu: bool,
}

impl Create {
    fn new(cx: &mut Cx) -> Option<Create> {
        let st = cx.tab.st()?.clone();
        let s2 = st.clone();
        let infer = Job::spawn(cx.ctx, move |cancel, progress| sc::infer(&s2, cancel, Some(progress)));
        Some(Create {
            name: cx.tab.name.clone(), path: cx.tab.path.clone(), st, infer: Some(infer), draft: None,
            counts: HashMap::new(), counting: HashMap::new(), texts: HashMap::new(), bad: HashSet::new(), focus: None,
            replace: None, saving: None, error: None, written: None, shot_menu: crate::shot::take("schema-menu").is_some(),
        })
    }

    /// Counts the rows breaking rule `r` of field i, in the background (JS `schemaRecount`).
    fn recount(&mut self, ctx: &egui::Context, i: usize, r: Rule) {
        let Some(d) = &self.draft else { return };
        if r == Rule::Pattern && rule_text(&d.schema.fields[i], r).is_empty() {
            self.counts.remove(&(i, r));
            return;
        }
        let (d, st) = (d.clone(), self.st.clone());
        self.counts.insert((i, r), Count::Note("counting…"));
        self.counting.insert((i, r), Job::spawn(ctx, move |cancel, _| d.count_breaking(&st, i, r, cancel)));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(r) = self.infer.as_mut().and_then(|j| j.poll()) {
            self.infer = None;
            match r {
                Some(inf) => {
                    self.draft = Some(Draft::new(inf));
                    // Screenshot hook: a few rules added, to show their editors and counts.
                    if crate::shot::take("schema-add").is_some() {
                        let n = self.draft.as_ref().unwrap().schema.fields.len();
                        let pat = self.draft.as_ref().unwrap().hints.get(6).and_then(|h| h.patterns.first()).map(|x| x.0.clone()).unwrap_or_default();
                        for a in [Act::Add(2, Rule::MinLength, String::new()), Act::Add(6, Rule::Pattern, pat), Act::Add(4, Rule::Check(Check::Capitalised), String::new())] {
                            if n > 6 { self.act(ctx, a); }
                        }
                    }
                }
                None => self.error = Some("Reading stopped.".into()),
            }
        }
        let mut done = Vec::new();
        for (k, j) in &mut self.counting {
            if let Some(r) = j.poll() { done.push((*k, r)); }
        }
        for (k, r) in done {
            self.counting.remove(&k);
            match r { Some(n) => { self.counts.insert(k, Count::Rows(n)); } None => { self.counts.remove(&k); } }
        }
        if let Some(r) = self.saving.as_mut().and_then(|j| j.poll()) {
            self.saving = None;
            if let Some(p) = r { self.write(&p, true, ctx); }
        }
    }

    /// Writes the schema to `path` (asking first when one is there). True once written.
    fn write(&mut self, path: &PathBuf, replace: bool, _ctx: &egui::Context) -> bool {
        let Some(d) = &self.draft else { return false };
        if !replace && path.exists() { self.replace = Some(path.clone()); return false; }
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::write(path, sc::to_json(&d.to_schema())) {
            Ok(()) => { self.error = None; self.written = Some(file); true }
            Err(e) => { self.error = Some(format!("Could not write {file}: {e}")); false }
        }
    }
}

impl Tool for Create {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let ctx = cx.ctx.clone();
        self.poll(&ctx);
        if let Some(f) = self.written.take() {
            cx.toast(format!("{} | Schema written: {f} — check other files with \"Validate against a schema\".", self.name));
            return false;
        }
        let p = cx.p;
        let mut save = false;
        let mut cancel = false;
        let (_, open) = window(cx, "create", |ui, _cx, h| {
            kit::title(ui, p, "Create a schema");
            let stem = sc::stem(&self.name).to_string();
            stat_line(ui, parts_job(&[(&format!("The rules {} already follows, to be written beside it as {stem}.schema.json. Add your own with “+ rule”.", self.name), p.txt_mut, false)], ui.available_width()));
            ui.add_space(10.0);
            let rest = h - (ui.cursor().min.y - ui.max_rect().min.y) - (10.0 + 19.0 + 16.0 + 30.0);
            let total = self.st.len();
            match self.draft.is_some() {
                false => {
                    let done = self.infer.as_ref().map_or(0, |j| j.progress.load(Ordering::Relaxed));
                    let msg = match &self.error { Some(e) => e.clone(), None => format!("Reading every row… {} %", (done as f64 * 100.0 / total.max(1) as f64).min(100.0).round()) };
                    table_box(ui, p, "create", rest, &[], |ui, _| {
                        let r = table_row(ui, p, &[(0.0, ui.available_width())], |ui, _| { line(ui, &msg, kit::ui(12.0), p.txt_faint); });
                        let _ = r;
                    });
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
                true => self.draft_table(ui, p, rest),
            }
            ui.add_space(10.0);
            let note = match (&self.error, &self.draft) {
                (Some(e), Some(_)) => vec![(e.as_str().to_string(), p.warn)],
                (_, Some(d)) => {
                    let n: usize = d.on.iter().map(Vec::len).sum::<usize>() + usize::from(d.pk && d.schema.primary_key.is_some());
                    vec![(format!("{} columns, {} rules ticked. Untick a rule the next files may break; change a type that was guessed too narrowly.", kit::n(d.schema.fields.len()), kit::n(n)), p.txt_mut)]
                }
                _ => vec![(String::new(), p.txt_mut)],
            };
            let parts: Vec<(&str, Color32, bool)> = note.iter().map(|(s, c)| (s.as_str(), *c, false)).collect();
            stat_line(ui, parts_job(&parts, ui.available_width()));
            kit::actions(ui, 16.0, |ui| {
                save = kit::button(ui, p, "Write the schema", kit::Btn::Primary, self.draft.is_some() && self.saving.is_none()).clicked();
                cancel = kit::button(ui, p, "Cancel", kit::Btn::Outline, true).clicked();
            });
        });
        // Screenshot hook: write the schema once the draft is there.
        if self.draft.is_some() && !save { save = crate::shot::take("schema-write").is_some(); }
        if save {
            match self.path.clone() {
                Some(csv) => { let to = sc::path_beside(&csv); self.write(&to, false, &ctx); }
                None => {
                    // A tab with no file: where to write it is asked (off the UI thread).
                    let name = format!("{}.schema.json", sc::stem(&self.name));
                    self.saving = Some(Job::spawn(&ctx, move |_, _| rfd::FileDialog::new().set_file_name(name).add_filter("Table Schema", &["json"]).save_file()));
                }
            }
        }
        if let Some(path) = self.replace.clone() {
            let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            match confirm(cx, &format!("Replace {file}?"), "A schema with this name is already beside the file.", "Replace") {
                Some(true) => { self.replace = None; self.write(&path, true, &ctx); }
                Some(false) => self.replace = None,
                None => {}
            }
        }
        if let Some(f) = self.written.take() {
            cx.toast(format!("{} | Schema written: {f} — check other files with \"Validate against a schema\".", self.name));
            return false;
        }
        open && !cancel
    }
}

/// A message dialog over the schema dialog: Some(true) on OK, Some(false) on Cancel.
fn confirm(cx: &mut Cx, msg: &str, detail: &str, ok: &str) -> Option<bool> {
    let p = cx.p;
    let mut out = None;
    let r = crate::widgets::modal(cx.ctx, egui::Id::unique("schema-confirm"), p, crate::theme::dialog_frame(p, cx.m), |ui| {
        ui.set_width(396.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        ui.label(egui::RichText::new(msg).font(semibold(14.5)).color(p.txt_hi));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(detail).font(kit::ui(13.0)).color(p.txt_mut));
        kit::actions(ui, 20.0, |ui| {
            if kit::button(ui, p, ok, kit::Btn::Primary, true).clicked() { out = Some(true); }
            if kit::button(ui, p, "Cancel", kit::Btn::Outline, true).clicked() { out = Some(false); }
        });
    });
    if r.should_close() { out = Some(false); }
    out
}

impl Create {
    /// The draft editor: Column | Type | Rules (JS `renderSchemaDraft`).
    fn draft_table(&mut self, ui: &mut Ui, p: &Palette, height: f32) {
        let ctx = ui.ctx().clone();
        let mut acts: Vec<Act> = Vec::new();
        let n = self.draft.as_ref().map_or(0, |d| d.schema.fields.len());
        table_box(ui, p, "create", height, &[("Column", -0.22, false), ("Type", 130.0, false), ("Rules", 0.0, false)], |ui, cols| {
            for i in 0..n {
                table_row(ui, p, cols, |ui, c| match c {
                    0 => { let d = self.draft.as_ref().unwrap(); line(ui, &d.schema.fields[i].name, kit::ui(12.0), p.txt); }
                    1 => self.type_cell(ui, p, i, &mut acts),
                    _ => self.rules_cell(ui, p, i, &mut acts),
                });
            }
        });
        for a in acts { self.act(&ctx, a); }
    }

    fn type_cell(&mut self, ui: &mut Ui, p: &Palette, i: usize, acts: &mut Vec<Act>) {
        let d = self.draft.as_ref().unwrap();
        let f = &d.schema.fields[i];
        let types: Vec<String> = sc::TYPES.iter().map(|s| s.to_string()).collect();
        let mut sel = sc::TYPES.iter().position(|t| *t == f.ty()).unwrap_or(0);
        if kit::select(ui, p, egui::Id::unique(("sc-type", i)), vec2(ui.available_width().min(112.0), 22.0), &mut sel, &types) {
            acts.push(Act::Type(i, sc::TYPES[sel]));
        }
        let ex = extra(f);
        if !ex.is_empty() {
            ui.add_space(3.0);
            let mut job = egui::text::LayoutJob::single_section(ex, egui::TextFormat { font_id: kit::ui(11.0), color: p.txt_faint, ..Default::default() });
            job.wrap.max_width = ui.available_width();
            wrapped(ui, job);
        }
    }

    fn rules_cell(&mut self, ui: &mut Ui, p: &Palette, i: usize, acts: &mut Vec<Act>) {
        let (rules, pk) = {
            let d = self.draft.as_ref().unwrap();
            let f = &d.schema.fields[i];
            (d.rules(i), d.schema.primary_key.as_ref().and_then(|v| v.as_str()) == Some(f.name.as_str()))
        };
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for r in rules { self.rule_chip(ui, p, i, r, acts); ui.add_space(12.0); }
            if pk {
                let on = self.draft.as_ref().unwrap().pk;
                if check_label(ui, p, on, "primary key").clicked() { acts.push(Act::Pk(!on)); }
                ui.add_space(12.0);
            }
            self.add_chip(ui, p, i, acts);
        });
    }

    /// One rule (`.sc-rule`): its checkbox and label, its editor, its count.
    fn rule_chip(&mut self, ui: &mut Ui, p: &Palette, i: usize, r: Rule, acts: &mut Vec<Act>) {
        let d = self.draft.as_ref().unwrap();
        let f = &d.schema.fields[i];
        let on = d.on[i].contains(&r);
        let key = (i, r);
        let kind = kind_of(r);
        let label = r.label();
        let val = self.texts.get(&key).cloned().unwrap_or_else(|| rule_text(f, r));
        let cw = ui.fonts_mut(|fs| fs.glyph_width(&kit::mono(12.0), '0'));
        let ew = match kind {
            None => 0.0,
            Some(Kind::Format) => 12.0 * cw + 20.0,
            Some(k) => editor_chars(k, &val) as f32 * cw + 14.0,
        } + if kind == Some(Kind::Pattern) { 18.0 } else { 0.0 };
        let count = self.counts.get(&key).map(|c| count_label(c, p, &self.name));
        let lw = ui.painter().layout_no_wrap(label.to_string(), kit::ui(12.0), p.txt).size().x;
        let cnt_w = count.as_ref().map_or(0.0, |c| 4.0 + ui.painter().layout_no_wrap(c.0.clone(), kit::ui(11.0), p.txt).size().x);
        let w = 16.0 + 4.0 + lw + if kind.is_some() { 4.0 + ew } else { 0.0 } + cnt_w + 4.0;
        ui.allocate_ui_with_layout(vec2(w, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
            ui.set_min_height(24.0);
            if check_label(ui, p, on, label).clicked() { acts.push(Act::Toggle(i, r, !on)); }
            match kind {
                None => {}
                Some(Kind::Format) => {
                    let opts: Vec<String> = sc::FORMATS.iter().map(|f| f.1.to_string()).collect();
                    let mut sel = sc::FORMATS.iter().position(|x| Some(x.0) == f.format.as_deref()).unwrap_or(0);
                    if kit::select(ui, p, egui::Id::unique(("sc-fmt", i)), vec2(ew, 20.0), &mut sel, &opts) {
                        acts.push(Act::Set(i, r, sc::FORMATS[sel].0.to_string()));
                    }
                }
                Some(k) => {
                    let id = egui::Id::unique(("sc-val", i, format!("{r:?}")));
                    let mut text = val.clone();
                    let fw = ew - if k == Kind::Pattern { 18.0 } else { 0.0 };
                    let resp = kit::text_field(ui, p, id, vec2(fw, 20.0), &mut text, "", kit::mono(12.0), self.bad.contains(&key));
                    if self.focus == Some(key) { resp.request_focus(); self.focus = None; }
                    let resp = if k == Kind::List { resp.on_hover_text(&text) } else { resp };
                    if resp.has_focus() || self.bad.contains(&key) || text != rule_text(f, r) { self.texts.insert(key, text.clone()); } else { self.texts.remove(&key); }
                    if resp.lost_focus() && (text != rule_text(f, r) || self.bad.contains(&key)) { acts.push(Act::Set(i, r, text)); }
                    if k == Kind::Pattern { pattern_picks(ui, p, i, &d.hints[i].patterns, acts); }
                }
            }
            if let Some((t, c, tip)) = count {
                let resp = ui.label(egui::RichText::new(t).font(kit::ui(11.0)).color(c));
                if !tip.is_empty() { resp.on_hover_text(tip); }
            }
        });
    }

    /// "+ rule" (`.sc-add`) and its menu.
    fn add_chip(&mut self, ui: &mut Ui, p: &Palette, i: usize, acts: &mut Vec<Act>) {
        let g = ui.painter().layout_no_wrap("+ rule".into(), kit::ui(11.0), p.txt_mut);
        let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 14.0 + 2.0, 24.0), Sense::click());
        let chip = Rect::from_min_size(pos2(r.min.x + 2.0, kit::snap(ui, r.center().y - 8.5)), vec2(g.size().x + 14.0, 17.0));
        let hot = resp.hovered();
        dashed_capsule(ui.painter(), chip, if hot { p.prim } else { p.brd_2 });
        ui.painter().galley_with_override_text_color(pos2(chip.min.x + 7.0, kit::snap(ui, chip.center().y - g.size().y / 2.0)), g, if hot { p.txt_hi } else { p.txt_mut });
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if self.shot_menu && i == 0 { self.shot_menu = false; egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&resp)); }
        let d = self.draft.as_ref().unwrap();
        let name = d.schema.fields[i].name.clone();
        let entries = add_menu(d, i);
        egui::Popup::menu(&resp).frame(crate::theme::menu_frame(p, &crate::theme::Metrics::default())).show(|ui| {
            crate::widgets::width(ui, 300.0);
            crate::widgets::head(ui, p, &name);
            if entries.is_empty() {
                crate::widgets::Item::new("Every rule of this type is there").enabled(false).show(ui, p);
            }
            for (rule, arg, label, sub) in &entries {
                if sub_item(ui, p, label, sub).clicked() {
                    acts.push(Act::Add(i, *rule, arg.clone()));
                    ui.close();
                }
            }
        });
    }

    fn act(&mut self, ctx: &egui::Context, a: Act) {
        let Some(d) = &mut self.draft else { return };
        match a {
            Act::Type(i, ty) => {
                d.set_type(i, ty);
                self.texts.retain(|k, _| k.0 != i);
                self.bad.retain(|k| k.0 != i);
                // The others are counted again: the column's reading has changed.
                let on = d.on[i].clone();
                self.counts.retain(|k, _| k.0 != i);
                for r in on { self.recount(ctx, i, r); }
            }
            Act::Toggle(i, r, on) => d.toggle(i, r, on),
            Act::Pk(on) => d.pk = on,
            Act::Add(i, r, arg) => {
                d.add_rule(i, r, &arg);
                let rs = if r == Rule::MinLength { vec![Rule::MinLength, Rule::MaxLength] } else { vec![r] };
                if r == Rule::Pattern && arg.is_empty() { self.focus = Some((i, r)); }
                for r in rs { self.recount(ctx, i, r); }
            }
            Act::Set(i, r, text) => {
                let key = (i, r);
                if d.set_value(i, r, &text).is_ok() {
                    self.bad.remove(&key);
                    self.texts.remove(&key);
                    self.counting.remove(&key);
                    self.recount(ctx, i, r);
                } else {
                    self.bad.insert(key);
                    self.texts.insert(key, text);
                    self.counting.remove(&key);
                    self.counts.insert(key, Count::Note("not valid"));
                }
            }
        }
    }
}

enum Act {
    Type(usize, &'static str),
    Toggle(usize, Rule, bool),
    Pk(bool),
    Add(usize, Rule, String),
    Set(usize, Rule, String),
}

/// A checkbox and its label (`.sc-rule label`): ui 12, `txt_hi` when on, `txt_dim` off.
fn check_label(ui: &mut Ui, p: &Palette, on: bool, text: &str) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_string(), kit::ui(12.0), if on { p.txt_hi } else { p.txt_dim });
    let (r, resp) = ui.allocate_exact_size(vec2(16.0 + 4.0 + g.size().x, 22.0), Sense::click());
    let b = Rect::from_min_size(pos2(r.min.x, kit::snap(ui, r.center().y - 8.0)), vec2(16.0, 16.0));
    kit::tick_box(ui.painter(), p, b, on, false, resp.hovered(), true);
    ui.painter().galley(pos2(b.max.x + 4.0, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, p.txt_hi);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The pattern field's list (the web's datalist): the shapes read from the values, then common patterns.
fn pattern_picks(ui: &mut Ui, p: &Palette, i: usize, hints: &[(String, String)], acts: &mut Vec<Act>) {
    let (r, resp) = ui.allocate_exact_size(vec2(14.0, 20.0), Sense::click());
    kit::chevron(ui.painter(), Rect::from_center_size(r.center(), vec2(12.0, 9.0)), if resp.hovered() { p.txt_hi } else { p.txt_dim });
    let resp = resp.on_hover_text("Patterns read from the values, and common ones");
    egui::Popup::menu(&resp).frame(crate::theme::menu_frame(p, &crate::theme::Metrics::default())).show(|ui| {
        crate::widgets::width(ui, 360.0);
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            let presets = sc::PATTERN_PRESETS.iter().map(|(a, b)| (a.to_string(), b.to_string()));
            for (pat, l) in hints.iter().cloned().chain(presets) {
                if sub_item(ui, p, &l, &pat).clicked() { acts.push(Act::Set(i, Rule::Pattern, pat)); ui.close(); }
            }
        });
    });
}

/// A menu item with a detail under its label (`.dd-item` + `.sc-sub`: mono 11 `txt_mut`, wrapped).
fn sub_item(ui: &mut Ui, p: &Palette, label: &str, sub: &str) -> egui::Response {
    let w = ui.available_width();
    let g = ui.painter().layout_no_wrap(label.to_string(), kit::ui(13.0), p.txt_hi);
    let sg = (!sub.is_empty()).then(|| {
        let mut job = egui::text::LayoutJob::single_section(sub.to_string(), egui::TextFormat { font_id: kit::mono(11.0), color: p.txt_mut, ..Default::default() });
        job.wrap.max_width = w - 20.0;
        ui.painter().layout_job(job)
    });
    let h = 14.0 + g.size().y + sg.as_ref().map_or(0.0, |s| 1.0 + s.size().y);
    let (r, resp) = ui.allocate_exact_size(vec2(w, h.max(30.0)), Sense::click());
    if resp.hovered() { ui.painter().rect_filled(r, 5.0, p.surf_2); }
    let y = if sg.is_some() { r.min.y + 7.0 } else { kit::snap(ui, r.center().y - g.size().y / 2.0) };
    let gh = g.size().y;
    ui.painter().galley(pos2(r.min.x + 10.0, y), g, p.txt_hi);
    if let Some(s) = sg { ui.painter().galley(pos2(r.min.x + 10.0, y + gh + 1.0), s, p.txt_mut); }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---- Validate --------------------------------------------------------------------

/// The summary line above the table.
enum Sum {
    Plain(String),
    Warn(String),
    Report,
}

struct Validate {
    name: String,
    path: Option<PathBuf>,
    st: State,
    sum: Sum,
    schema_name: String,
    picking: Option<Job<Option<PathBuf>>>,
    job: Option<Job<Option<Result<Validation, String>>>>,
    report: Option<Arc<Validation>>,
}

impl Validate {
    fn new(cx: &mut Cx) -> Option<Validate> {
        let st = cx.tab.st()?.clone();
        let mut v = Validate { name: cx.tab.name.clone(), path: cx.tab.path.clone(), st, sum: Sum::Plain("Looking for a schema beside the file…".into()),
            schema_name: String::new(), picking: None, job: None, report: None };
        match v.path.as_deref().and_then(sc::find_beside) {
            Some((path, text)) => v.run(cx.ctx, &path, &text),
            None => v.sum = Sum::Plain(format!("No **{}.schema.json** beside the file. Choose a schema, or create one from a file that is right with \"Create a schema\".", sc::stem(&v.name))),
        }
        Some(v)
    }

    /// Checks the file against the schema's text (JS `schemaRun`).
    fn run(&mut self, ctx: &egui::Context, path: &std::path::Path, text: &str) {
        let name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        self.report = None;
        match sc::parse(text) {
            Err(e) => { self.sum = Sum::Warn(format!("{name}: {e}.")); self.job = None; }
            Ok(schema) => {
                self.sum = Sum::Plain(format!("Checking {} against {name}…", self.name));
                let st = self.st.clone();
                self.job = Some(Job::spawn(ctx, move |cancel, progress| sc::validate(&st, &schema, cancel, Some(progress))));
            }
        }
        self.schema_name = name;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            match r {
                Some(Ok(v)) => { self.report = Some(Arc::new(v)); self.sum = Sum::Report; }
                Some(Err(e)) => self.sum = Sum::Warn(format!("{}: {e}.", self.schema_name)),
                None => {}
            }
        }
        if let Some(r) = self.picking.as_mut().and_then(|j| j.poll()) {
            self.picking = None;
            if let Some(path) = r {
                match std::fs::read_to_string(&path) {
                    Ok(text) => self.run(ctx, &path, &text),
                    Err(e) => self.sum = Sum::Warn(format!("{}: {e}.", path.display())),
                }
            }
        }
    }

    /// The rows of one kind of error (or of all), shown alone and marked (JS `schemaShow`).
    fn show_rows(&self, cx: &mut Cx, item: Option<usize>) {
        let Some(v) = &self.report else { return };
        let (rows, label) = match item {
            None => (v.rows.clone(), "rows breaking the schema".to_string()),
            Some(i) => {
                let it = &v.items[i];
                (it.rows.clone(), if it.field.is_empty() { it.label.clone() } else { format!("{}: {}", it.field, it.label) })
            }
        };
        cx.tab.marks = Arc::new(Marks { dups: cx.tab.marks.dups.clone(), ..marks_of(v, &rows) });
        let n = rows.len();
        cx.tab.restrict_to(label.clone(), rows, cx.ctx);
        cx.toast(format!("{} | {} {label} — the chip shows all rows again.", self.name, kit::n(n)));
    }
}

/// Marks for these rows: the rows, and their cells at fault with the reason.
fn marks_of(v: &Validation, rows: &[csvfab_engine::RowId]) -> Marks {
    let set: HashSet<_> = rows.iter().copied().collect();
    let cells = v.cells.iter().filter(|c| set.contains(&c.0)).map(|(r, c, why)| ((*r, *c), why.clone())).collect();
    Marks { rows: set, cells, ..Default::default() }
}

/// The report's summary as parts (JS `renderSchemaReport`).
fn report_parts(v: &Validation, p: &Palette) -> Vec<(String, Color32)> {
    let bad = v.rows.len();
    if bad == 0 && v.structure.is_empty() {
        return vec![(format!("All {} rows follow the schema.", kit::n(v.checked)), p.ok_txt)];
    }
    let cols = v.items.iter().filter(|i| !i.field.is_empty()).map(|i| &i.field).collect::<HashSet<_>>().len();
    let mut s = if bad > 0 { format!("**{}** of {} rows break the schema", kit::n(bad), kit::n(v.checked)) } else { "The rows follow the schema".into() };
    if cols > 0 { s += &format!(", in {}", kit::plural(cols, "column", "columns")); }
    let k = v.structure.len();
    if k > 0 { s += &format!(" — {} of structure", kit::plural(k, "problem", "problems")); }
    vec![(s + ".", p.warn)]
}

impl Tool for Validate {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let ctx = cx.ctx.clone();
        self.poll(&ctx);
        let p = cx.p;
        let mut pick = false;
        let mut show: Option<Option<usize>> = None;
        let mut close = false;
        let (_, open) = window(cx, "validate", |ui, _cx, h| {
            kit::title(ui, p, "Validate with a schema");
            let w = ui.available_width();
            let job = match &self.sum {
                Sum::Plain(s) => parts_job(&[(s, p.txt_mut, false)], w),
                Sum::Warn(s) => parts_job(&[(s, p.warn, false)], w),
                Sum::Report => {
                    let v = self.report.as_ref().unwrap();
                    let mut parts: Vec<(String, Color32)> = report_parts(v, p);
                    parts.push((format!(" {} · {}", self.name, self.schema_name), p.txt_faint));
                    let parts: Vec<(&str, Color32, bool)> = parts.iter().map(|(s, c)| (s.as_str(), *c, false)).collect();
                    let mut j = parts_job(&parts, w);
                    // `.stat-line b` stays `txt_hi` inside the warning.
                    for s in &mut j.sections { if s.format.font_id == semibold(12.0) { s.format.color = p.txt_hi; } }
                    j
                }
            };
            stat_line(ui, job);
            ui.add_space(10.0);
            let rest = h - (ui.cursor().min.y - ui.max_rect().min.y) - (10.0 + 19.0 + 16.0 + 30.0);
            if let Some(j) = &self.job {
                let done = j.progress.load(Ordering::Relaxed) as f64 / self.st.len().max(1) as f64;
                let msg = format!("Checking… {} %", (done * 100.0).min(100.0).round());
                table_box(ui, p, "validate", rest, &[], |ui, _| { table_row(ui, p, &[(0.0, ui.available_width())], |ui, _| { line(ui, &msg, kit::ui(12.0), p.txt_faint); }); });
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            } else if let Some(v) = self.report.clone() {
                show = report_table(ui, p, &v, rest).map(Some);
            } else {
                table_box(ui, p, "validate", rest, &[], |_, _| {});
            }
            ui.add_space(10.0);
            let note = if self.report.as_ref().is_some_and(|v| !v.items.is_empty()) { "Click a line to show its rows; the cells at fault are tinted, their tooltip says why." } else { "" };
            stat_line(ui, parts_job(&[(note, p.txt_mut, false)], w));
            kit::actions(ui, 16.0, |ui| {
                close = kit::button(ui, p, "Close", kit::Btn::Primary, true).clicked();
                let any = self.report.as_ref().is_some_and(|v| !v.rows.is_empty());
                if kit::button(ui, p, "Show every row at fault", kit::Btn::Outline, any).clicked() { show = Some(None); }
                pick = kit::button(ui, p, "Choose a schema…", kit::Btn::Outline, self.picking.is_none()).clicked();
            });
        });
        if pick {
            let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
            self.picking = Some(Job::spawn(&ctx, move |_, _| {
                let mut d = rfd::FileDialog::new().add_filter("Table Schema", &["json"]).add_filter("All files", &["*"]);
                if let Some(dir) = dir { d = d.set_directory(dir); }
                d.pick_file()
            }));
        }
        if let Some(item) = show {
            self.show_rows(cx, item);
            return false;
        }
        open && !close
    }
}

/// The report (`.pv.rv.sc`): the structure, then one line per kind of error; returns the line clicked.
fn report_table(ui: &mut Ui, p: &Palette, v: &Validation, height: f32) -> Option<usize> {
    let mut hit = None;
    table_box(ui, p, "validate", height, &[], |ui, _| {
        let w = ui.available_width();
        let full = [(0.0, w)];
        let cols = col_layout(&[0.0, 0.0, 0.0, 0.0], w);
        let sec = |ui: &mut Ui, t: &str| { table_row(ui, p, &full, |ui, _| { line(ui, t, semibold(12.0), p.txt_mut); }); };
        if !v.structure.is_empty() {
            sec(ui, "Structure");
            for s in &v.structure { table_row(ui, p, &full, |ui, _| { line(ui, s, kit::ui(12.0), p.warn); }); }
        }
        if v.items.is_empty() { return; }
        table_row(ui, p, &cols, |ui, c| {
            let (t, right) = [("Column", false), ("Problem", false), ("Rows", true), ("For example", false)][c];
            if right { ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| { line(ui, t, semibold(12.0), p.txt_mut); }); } else { line(ui, t, semibold(12.0), p.txt_mut); }
        });
        for (i, it) in v.items.iter().enumerate() {
            let top = ui.cursor().min;
            let r = table_row(ui, p, &cols, |ui, c| match c {
                0 => { line(ui, if it.field.is_empty() { "(row)" } else { &it.field }, kit::ui(12.0), p.txt); }
                1 => { line(ui, &it.label, kit::ui(12.0), p.txt); }
                2 => { ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| { line(ui, &kit::n(it.count), kit::ui(12.0), p.txt_sub); }); }
                _ => examples(ui, p, &it.examples),
            });
            let resp = ui.interact(Rect::from_min_max(top, r.max), egui::Id::unique(("sc-item", i)), Sense::click());
            if resp.hovered() { ui.painter().rect_filled(Rect::from_min_max(top, pos2(r.max.x, r.max.y - 1.0)), 0.0, alpha(p.hi, 0.06)); }
            if resp.clicked() { hit = Some(i); }
        }
    });
    hit
}

/// The examples as code (mono 11), "(empty)" for an empty one, wrapped.
fn examples(ui: &mut Ui, p: &Palette, ex: &[String]) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    for (k, v) in ex.iter().enumerate() {
        if k > 0 { job.append(" ", 0.0, egui::TextFormat { font_id: kit::ui(12.0), color: p.txt, ..Default::default() }); }
        let shown = if v.is_empty() { "(empty)" } else { v.as_str() };
        job.append(shown, 0.0, egui::TextFormat { font_id: kit::mono(11.0), color: p.txt, line_height: Some(16.0), valign: egui::Align::Center, ..Default::default() });
    }
    wrapped(ui, job);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(json: &str) -> Field {
        sc::parse(&format!("{{\"fields\": [{json}]}}")).unwrap().fields.remove(0)
    }

    #[test]
    fn rule_texts() {
        let f = field(r#"{"name": "a", "type": "integer", "constraints": {"minimum": 3, "maximum": 1200.5, "enum": ["x", "y, z"], "minLength": 2, "pattern": "\\d{5}"}}"#);
        assert_eq!(rule_text(&f, Rule::Minimum), "3");
        assert_eq!(rule_text(&f, Rule::Maximum), "1200.5");
        assert_eq!(rule_text(&f, Rule::Enum), "x, \"y, z\"");
        assert_eq!(rule_text(&f, Rule::MinLength), "2");
        assert_eq!(rule_text(&f, Rule::MaxLength), "");
        assert_eq!(rule_text(&f, Rule::Pattern), r"\d{5}");
        let d = field(r#"{"name": "d", "type": "date", "constraints": {"minimum": "01/02/2020"}}"#);
        assert_eq!(rule_text(&d, Rule::Minimum), "01/02/2020");
    }

    #[test]
    fn extras() {
        let f = field(r#"{"name": "a", "type": "number", "decimalChar": ",", "groupChar": " "}"#);
        assert_eq!(extra(&f), "decimal comma · spaces between thousands");
        let f = field(r#"{"name": "a", "type": "date", "format": "%d/%m/%Y"}"#);
        assert_eq!(extra(&f), "format %d/%m/%Y");
        let f = field(r#"{"name": "a", "type": "boolean", "trueValues": ["oui"], "falseValues": ["non"]}"#);
        assert_eq!(extra(&f), "oui = true");
        assert_eq!(extra(&field(r#"{"name": "a", "type": "string", "format": "email"}"#)), "");
    }

    #[test]
    fn sizes_and_numbers() {
        assert_eq!(editor_chars(Kind::Length, "12"), 5);
        assert_eq!(editor_chars(Kind::Value, "3"), 8);
        assert_eq!(editor_chars(Kind::Pattern, ""), 16);
        assert_eq!(editor_chars(Kind::Pattern, &"x".repeat(80)), 42);
        assert_eq!(js_num(97.5), "97.5");
        assert_eq!(js_num(100.0), "100");
        let c = col_layout(&[-0.22, 130.0, 0.0], 1000.0);
        assert_eq!(c, vec![(0.0, 220.0), (220.0, 130.0), (350.0, 650.0)]);
    }

    #[test]
    fn menu_and_marks() {
        let base = Arc::new(csvfab_engine::Base::from_bytes(b"id,name,cp\n1,Ann,75001\n2,Bob,75002\n3,Cy,7500\n".to_vec(), Default::default()));
        let t = csvfab_engine::Table::new(base, None);
        let inf = sc::infer(&t.st, &Default::default(), None).unwrap();
        let d = Draft::new(inf);
        let names: Vec<String> = add_menu(&d, 1).iter().map(|e| e.2.clone()).collect();
        assert!(names.contains(&"Pattern…".to_string()));
        assert!(names.contains(&"Format: e-mail".to_string()));
        assert!(names.iter().any(|n| n == "Length"));
        // A validation's marks keep only the cells of the rows shown.
        let schema = sc::parse(r#"{"fields": [{"name": "id", "type": "integer"}, {"name": "name", "type": "string"}, {"name": "cp", "type": "string", "constraints": {"pattern": "\\d{5}"}}]}"#).unwrap();
        let v = sc::validate(&t.st, &schema, &Default::default(), None).unwrap().unwrap();
        assert_eq!(v.items.len(), 1);
        let m = marks_of(&v, &v.items[0].rows);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.cells.len(), 1);
        assert!(m.cells.keys().all(|k| k.1 == 2));
    }
}
