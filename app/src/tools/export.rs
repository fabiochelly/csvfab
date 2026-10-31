//! Save as, export and extract (14-save-export.js): the save dialog — a file
//! name, a format (CSV with its delimiter and encoding; as a copy, an Excel workbook, a
//! SQLite database, a Parquet file, JSON, JSON Lines, TSV or any delimiter, an HTML or
//! Markdown table), only the rows shown — written in a job; Extract (the
//! rows shown to a new file beside the source, opened as a tab); Discard (the
//! edits dropped, the file read again).
//!
//! A new CSV name is a save-as: the tab moves onto the new file and the
//! source keeps its last saved content. A copy is a second artefact and never
//! takes the tab over.
//!
//! What is an export and what is a save decides what is written. An export
//! (every copy above, a CSV of the rows shown, Extract)
//! leaves out the hidden columns, and writes the rows shown, in the view's
//! order, unless "Only the rows shown" is unticked. A save or a save-as writes
//! the table whole: the tab goes on from that file, so anything left out
//! would be lost.

use super::form::{self, Alert, Answer, Ask, Btn};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::convert::text_write::{self, TextFormat};
use csvfab_engine::convert::{self, Format};
use csvfab_engine::save::{self, SaveError, SaveOptions, Switch};
use csvfab_engine::table::RowMask;
use csvfab_engine::{Enc, RowId, State};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "Save as Excel workbook", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Xlsx)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Save as SQLite database", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Sqlite)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Save as Parquet file", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Parquet)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Export as JSON", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Json)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Export as JSON Lines", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::JsonLines)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Export as TSV", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Delim(b'\t'))); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Export as an HTML table", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Html)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Export as a Markdown table", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Markdown)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Extract the rows shown to a new file", group: "Save", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(extract())) },
        Command { name: "Discard all edits and reload from disk", group: "Save", keys: None, enabled: |t| t.dirty() && t.path.is_some(), run: |cx| cx.out.push(Cmd::Tool(discard())) },
    ]
}

/// The save dialog for the tab (what `App::save_as` opens).
pub fn save_as(tab: &Tab) -> Box<dyn Tool> {
    Box::new(SaveAs::new(tab, None))
}

/// Extract: the rows shown, to a new file beside the source, opened as a tab.
pub fn extract() -> Box<dyn Tool> {
    Box::new(Extract { stage: XStage::Start })
}

/// Discard: asks, then drops the edits and reads the file again.
pub fn discard() -> Box<dyn Tool> {
    Box::new(Discard { summary: None, job: None })
}

// --- Formats ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fmt {
    /// Raw text, lines as they are (only for a raw text tab).
    Text,
    Csv(u8),
    Xlsx,
    Sqlite,
    Parquet,
    /// A delimited copy (TSV, any delimiter): an export, where `Csv` is the table saved.
    Delim(u8),
    Json,
    JsonLines,
    Html,
    Markdown,
}

impl Fmt {
    /// What a file of this format ends in, when it has its own extension.
    fn ext(self) -> Option<&'static str> {
        Some(match self {
            Fmt::Xlsx => ".xlsx",
            Fmt::Sqlite => ".sqlite",
            Fmt::Parquet => ".parquet",
            Fmt::Delim(b'\t') => ".tsv",
            Fmt::Json | Fmt::JsonLines | Fmt::Html | Fmt::Markdown => self.text()?.ext(),
            _ => return None,
        })
    }

    fn text(self) -> Option<TextFormat> {
        Some(match self { Fmt::Json => TextFormat::Json, Fmt::JsonLines => TextFormat::JsonLines, Fmt::Html => TextFormat::Html, Fmt::Markdown => TextFormat::Markdown, _ => return None })
    }

    /// Written through `save`, with an encoding.
    fn delimited(self) -> bool {
        matches!(self, Fmt::Csv(_) | Fmt::Text | Fmt::Delim(_))
    }

    /// What the file is, in the note and the messages.
    fn noun(self) -> &'static str {
        match self {
            Fmt::Xlsx => "workbook",
            Fmt::Sqlite => "database",
            Fmt::Parquet => "Parquet file",
            Fmt::Delim(b'\t') => "TSV file",
            Fmt::Delim(_) => "delimited file",
            Fmt::Json => "JSON file",
            Fmt::JsonLines => "JSON Lines file",
            Fmt::Html => "HTML table",
            Fmt::Markdown => "Markdown table",
            _ => "file",
        }
    }
}

const DELIMS: [(u8, &str, &str); 4] = [(b';', "CSV — semicolon", ";"), (b',', "CSV — comma", ","), (b'\t', "CSV — tab", "\\t"), (b'|', "CSV — pipe", "|")];

fn enc_name(e: Enc) -> &'static str {
    e.label()
}

/// The one ASCII character typed as a delimiter (`\t` for a tab): never a quote or a line break.
fn custom_delim(s: &str) -> Option<u8> {
    let s = if s == "\\t" { "\t" } else { s };
    (s.len() == 1 && s.is_ascii()).then(|| s.as_bytes()[0]).filter(|&d| d != b'"' && d != b'\n' && d != b'\r' && d != 0)
}

/// The name without its table extension.
fn stem_of(name: &str) -> &str {
    csvfab_engine::split::stem_of(name)
}

/// The tab's CSV extension (`.csv`, `.tsv`, `.txt`), else `.csv`.
fn csv_ext(name: &str) -> &str {
    csvfab_engine::split::csv_ext(name)
}

/// `foo-3.csv` → `foo-4.csv`, `v007.csv` → `v008.csv` (padding kept), `foo.csv` → `foo-2.csv`.
pub fn next_name(name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(k) if k > 0 => (&name[..k], &name[k..]),
        _ => (name, ".csv"),
    };
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 { return format!("{stem}-2{ext}"); }
    let (head, num) = stem.split_at(stem.len() - digits);
    // Decimal increment of any length (the web used BigInt).
    let mut d: Vec<u8> = num.bytes().collect();
    let mut i = d.len();
    loop {
        if i == 0 { d.insert(0, b'1'); break; }
        i -= 1;
        if d[i] == b'9' { d[i] = b'0'; } else { d[i] += 1; break; }
    }
    format!("{head}{}{ext}", String::from_utf8(d).unwrap())
}

/// What an encoding cannot write of a table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lossy {
    /// Cells holding at least one such character.
    pub cells: usize,
    /// How many there are.
    pub chars: usize,
    /// The first few, each once.
    pub first: Vec<char>,
}

impl Lossy {
    const FIRST: usize = 5;

    fn add(&mut self, v: &str, enc: Enc) {
        let chars = self.chars;
        csvfab_engine::enc::each_unencodable(enc, v, |c| {
            self.chars += 1;
            if self.first.len() < Self::FIRST && !self.first.contains(&c) { self.first.push(c); }
        });
        self.cells += (self.chars > chars) as usize;
    }

    fn merge(&mut self, o: Lossy) {
        self.cells += o.cells;
        self.chars += o.chars;
        for c in o.first {
            if self.first.len() < Self::FIRST && !self.first.contains(&c) { self.first.push(c); }
        }
    }

    /// The question's first line: how many, in how many cells, naming the first few.
    pub fn say(&self, enc: Enc) -> String {
        let named = self.first.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
        let more = if self.chars > self.first.len() { ", and others" } else { "" };
        format!("{} cannot write {} in {}: {named}{more}.", enc.label(), form::plural(self.chars, "character"), form::plural(self.cells, "cell"))
    }
}

/// What `enc` cannot write of the rows (headers included). None if cancelled. In the file's own
/// encoding only the edited cells are looked at (a cell as read holds none); in another, every
/// cell, each row read in one pass, the rows in parallel.
pub(crate) fn unencodable(st: &State, enc: Enc, rows: Option<&[RowId]>, cancel: &AtomicBool) -> Option<Lossy> {
    use rayon::prelude::*;
    if st.base.enc == enc {
        // The rows written: these (and the titles), or every row of the table.
        let head = (st.header && st.all_len() > 0).then(|| st.all_id(0));
        let keep = match (rows, &st.order) {
            (Some(r), _) => Some(RowMask::new(r.iter().copied().chain(head))),
            (None, Some(o)) => Some(RowMask::new(o.iter().copied())),
            (None, None) => None,
        };
        let parts: Vec<Lossy> = (0..st.ncols).into_par_iter().map(|c| {
            let mut ids: Vec<RowId> = st.cols[c].iter().flat_map(|l| l.ids().iter().copied()).filter(|&id| keep.as_ref().map_or((id as usize) < st.base.n, |k| k.contains(id))).collect();
            ids.par_sort_unstable();
            ids.dedup();
            let mut l = Lossy::default();
            for id in ids {
                if let Some(v) = st.edited(id, c) { l.add(v, enc); }
            }
            l
        }).collect();
        if cancel.load(Ordering::Relaxed) { return None; }
        return Some(parts.into_iter().fold(Lossy::default(), |mut a, b| { a.merge(b); a }));
    }
    let mut all = Lossy::default();
    for h in st.headers() { all.add(&h, enc); }
    let every: Vec<RowId>;
    let ids: &[RowId] = match rows {
        Some(r) => r,
        None => { every = (0..st.len()).map(|p| st.data_id(p)).collect(); &every }
    };
    let parts: Vec<Lossy> = ids.par_chunks(8192).map(|chunk| {
        let mut l = Lossy::default();
        if cancel.load(Ordering::Relaxed) { return l; }
        for &id in chunk {
            st.visit(id, |_, v| l.add(v, enc));
        }
        l
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    for p in parts { all.merge(p); }
    Some(all)
}

/// The encoding of a file written: a select, its list grouped and searchable.
fn enc_select(ui: &mut egui::Ui, p: &crate::theme::Palette, enc: Enc, current: Enc) -> Option<Enc> {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 31.0), egui::Sense::click());
    let id = resp.id.with("menu");
    let open = egui::Popup::is_id_open(ui.ctx(), id);
    crate::widgets::field_frame(ui, p, resp.id, r, open, false, 6.0);
    let text = if enc == current { format!("{} (current)", enc.label()) } else { enc.label().to_string() };
    ui.painter().text(egui::pos2(r.min.x + 12.0, r.center().y), egui::Align2::LEFT_CENTER, text, form::ui_font(14.0), p.txt_hi);
    let chev = egui::Rect::from_center_size(egui::pos2(r.max.x - 20.0, r.center().y), egui::vec2(16.0, 12.0));
    crate::icons::paint(ui.painter(), crate::icons::Icon::SelectChevron, chev, p.txt_dim);
    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
    let mut pick = None;
    egui::Popup::menu(&resp).id(id).frame(crate::widgets::menu_frame(p)).width(r.width()).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        crate::widgets::width(ui, r.width() - 12.0);
        pick = crate::widgets::enc_list(ui, p, Some(enc));
        if pick.is_some() { ui.close(); }
    });
    pick
}

// --- Save as --------------------------------------------------------------------

/// What a click on Save decided, written once the questions are answered.
struct Plan {
    fmt: Fmt,
    dest: PathBuf,
    in_place: bool,
    /// Only these rows (the rows shown): a new file, opened as a tab.
    rows: Option<Arc<Vec<RowId>>>,
    exists: bool,
    enc: Enc,
    bom: bool,
}

enum Done {
    Workbook(convert::Exported),
    /// The file's stamp once written, the sample of what was written, the rows' new ids.
    Csv(csvfab_engine::Stamp, Option<u64>, Option<std::sync::Arc<Vec<u32>>>),
}

enum Stage {
    Form,
    Picking(Job<Option<PathBuf>>, Plan),
    ConfirmOverwrite(Plan),
    Checking(Job<Option<Lossy>>, Plan),
    ConfirmLossy(Lossy, Plan),
    Writing { job: Job<Result<Done, (bool, String)>>, plan: Plan, version: u64, total: usize },
    /// The file changed on disk since it was read: overwrite?
    Changed(Plan),
    Failed(Alert),
}

struct SaveAs {
    uid: u64,
    name: String,
    fmt: Fmt,
    enc: Enc,
    bom: bool,
    /// "Only the rows shown" as ticked by hand. None: on for an export, off for a save-as.
    only_shown: Option<bool>,
    /// What is typed as "Other delimiter".
    custom: String,
    stage: Stage,
    focus: bool,
    /// The "Writing…" message was hidden.
    quiet: bool,
}

impl SaveAs {
    fn new(tab: &Tab, preset: Option<Fmt>) -> SaveAs {
        let (cur_fmt, enc, bom) = match tab.st() {
            Some(st) => (if st.base.dialect.lines { Fmt::Text } else { Fmt::Csv(st.base.dialect.delim) }, st.base.enc, st.base.bom),
            None => (Fmt::Csv(b';'), Enc::Utf8, false),
        };
        let fmt = preset.unwrap_or(cur_fmt);
        let name = match fmt.ext() {
            Some(e) => format!("{}{e}", stem_of(&tab.name)),
            None => tab.name.clone(),
        };
        let name = if name.is_empty() { "table.csv".into() } else { name };
        SaveAs { uid: tab.uid, name, fmt, enc, bom, only_shown: None, custom: String::new(), stage: Stage::Form, focus: true, quiet: false }
    }

    /// A second artefact, not the table saved.
    fn exports(fmt: Fmt) -> bool {
        !matches!(fmt, Fmt::Csv(_) | Fmt::Text)
    }

    fn only(&self) -> bool {
        self.only_shown.unwrap_or(Self::exports(self.fmt))
    }

    fn cur_fmt(st: &State) -> Fmt {
        if st.base.dialect.lines { Fmt::Text } else { Fmt::Csv(st.base.dialect.delim) }
    }

    /// The note under the choices (updateSaveNote).
    fn note(&self, tab: &Tab, st: &State) -> String {
        let name = self.name.trim();
        let where_ = if tab.path.is_some() { "beside the source" } else { "where you choose" };
        let hides = !tab.grid.hidden.is_empty();
        if Self::exports(self.fmt) {
            let what = self.fmt.noun();
            let rows = if self.subset(tab) { format!("the {} rows shown as a {what}", form::n(tab.shown())) } else { format!("the {what}") };
            return format!("Writes {rows} {where_}. {} is left as it is{}.{}", tab.name, if tab.dirty() { " and keeps its pending edits" } else { "" }, if hides { " Hidden columns are left out." } else { "" });
        }
        if self.subset(tab) {
            return format!("Writes the {} rows shown to a new file {where_} and opens it. {} is left as it is.{}", form::n(tab.shown()), tab.name, if hides { " Hidden columns are left out." } else { "" });
        }
        let kept = if hides { " Hidden columns are kept." } else { "" };
        let cur = Self::cur_fmt(st);
        let mut changes = Vec::new();
        if self.fmt != cur {
            changes.push(if self.fmt == Fmt::Text { "as text" } else if cur == Fmt::Text { "as a one-column CSV" } else { "re-delimited" }.to_string());
        }
        if self.enc != st.base.enc { changes.push(format!("re-encoded as {}", enc_name(self.enc))); }
        if name == tab.name {
            if tab.path.is_none() {
                "This tab is a read-only copy: change the name to write a new file.".into()
            } else {
                format!("Overwrites {}{} — a timestamped .bak copy is kept.{}", tab.name, if changes.is_empty() { String::new() } else { format!(", {}", changes.join(", ")) }, kept)
            }
        } else {
            format!("Writes a new file {where_} and switches this tab to it. {} stays as it was last saved.{kept}", tab.name)
        }
    }

    fn subset(&self, tab: &Tab) -> bool {
        self.only() && tab.filtered()
    }

    /// Save clicked: the plan, or a message.
    fn submit(&mut self, cx: &mut Cx) {
        let mut name = self.name.trim().to_string();
        if name.is_empty() || name.contains(['/', '\\']) {
            self.stage = Stage::Failed(Alert::new("Enter a file name, without any folder."));
            return;
        }
        let tab = &*cx.tab;
        let Some(st) = tab.st() else { return };
        let lower = name.to_ascii_lowercase();
        match self.fmt {
            Fmt::Xlsx if !lower.ends_with(".xlsx") => name.push_str(".xlsx"),
            Fmt::Sqlite if ![".sqlite", ".sqlite3", ".db", ".db3"].iter().any(|e| lower.ends_with(e)) => name.push_str(".sqlite"),
            Fmt::Parquet if ![".parquet", ".parq", ".pqt"].iter().any(|e| lower.ends_with(e)) => name.push_str(".parquet"),
            Fmt::Json if !lower.ends_with(".json") => name.push_str(".json"),
            Fmt::JsonLines if ![".jsonl", ".ndjson"].iter().any(|e| lower.ends_with(e)) => name.push_str(".jsonl"),
            Fmt::Html if ![".html", ".htm"].iter().any(|e| lower.ends_with(e)) => name.push_str(".html"),
            Fmt::Markdown if ![".md", ".markdown"].iter().any(|e| lower.ends_with(e)) => name.push_str(".md"),
            Fmt::Csv(_) | Fmt::Text | Fmt::Delim(_) if Path::new(&name).extension().is_none() => name.push_str(if self.fmt == Fmt::Delim(b'\t') { ".tsv" } else { ".csv" }),
            _ => {}
        }
        // A copy never replaces the file the tab stands on.
        if Self::exports(self.fmt) && self.fmt.delimited() && name == tab.name {
            self.stage = Stage::Failed(Alert::new("A copy needs another file name."));
            return;
        }
        let rows = self.subset(tab).then(|| tab.shown_ids());
        let csv = matches!(self.fmt, Fmt::Csv(_) | Fmt::Text);
        let in_place = csv && rows.is_none() && name == tab.name && tab.path.is_some();
        if in_place && self.fmt == Self::cur_fmt(st) && self.enc == st.base.enc && self.bom == st.base.bom && !tab.dirty() {
            cx.toast(format!("{} | Nothing to save.", tab.name));
            self.stage = Stage::Failed(Alert::new(""));
            return;
        }
        let plan = Plan { fmt: self.fmt, dest: PathBuf::new(), in_place, rows, exists: false, enc: self.enc, bom: self.bom };
        match tab.path.as_ref().and_then(|p| p.parent()) {
            Some(dir) => {
                let dest = if in_place { tab.path.clone().unwrap() } else { dir.join(&name) };
                self.planned(Plan { dest, ..plan }, cx);
            }
            None => self.stage = Stage::Picking(form::pick_save(cx.ctx, &name, None), plan),
        }
    }

    /// The destination is known: the questions, then the write.
    fn planned(&mut self, mut plan: Plan, cx: &mut Cx) {
        // Over a file another tab holds: refused, as Save as (that tab would no longer match it).
        if let Some(o) = crate::app::open_elsewhere(&plan.dest, cx.tab.uid).filter(|_| !plan.in_place) {
            self.stage = Stage::Failed(Alert::new(&format!("{o} is open in another tab — close it first.")));
            return;
        }
        plan.exists = !plan.in_place && plan.dest.exists();
        if plan.exists {
            self.stage = Stage::ConfirmOverwrite(plan);
            return;
        }
        self.check(plan, cx);
    }

    /// An encoding short of Unicode: what would be lost is said first.
    fn check(&mut self, plan: Plan, cx: &mut Cx) {
        let Some(st) = cx.tab.st().cloned() else { return };
        if plan.fmt.delimited() && plan.enc.limited() && (plan.enc != st.base.enc || cx.tab.dirty()) {
            let (enc, rows) = (plan.enc, plan.rows.clone());
            let job = Job::spawn(cx.ctx, move |cancel, _| unencodable(&st, enc, rows.as_deref().map(|v| v.as_slice()), cancel));
            self.stage = Stage::Checking(job, plan);
        } else {
            self.write(plan, false, cx);
        }
    }

    fn write(&mut self, plan: Plan, force: bool, cx: &mut Cx) {
        let tab = &mut *cx.tab;
        let Some(table) = &tab.table else { return };
        let version = table.version();
        // Only an export leaves columns out: a save-as is what the tab goes on from.
        let st = if Self::exports(plan.fmt) || plan.rows.is_some() {
            let Some(st) = tab.export_st() else {
                self.stage = Stage::Failed(Alert::new("Every column is hidden: nothing to write."));
                return;
            };
            st
        } else {
            table.st.clone()
        };
        let total = plan.rows.as_ref().map_or(st.len(), |r| r.len());
        let (fmt, dest, rows) = (plan.fmt, plan.dest.clone(), plan.rows.clone());
        let job = match fmt {
            Fmt::Xlsx | Fmt::Sqlite | Fmt::Parquet => {
                let f = match fmt { Fmt::Xlsx => Format::Xlsx, Fmt::Sqlite => Format::Sqlite, _ => Format::Parquet };
                let sheet = convert::export_sheet_name(&form::file_name(&dest));
                Job::spawn(cx.ctx, move |cancel, progress| {
                    convert::export(f, &st, rows.as_deref().map(|v| v.as_slice()), &dest, &sheet, Some(progress), cancel).map(Done::Workbook).map_err(|e| (false, e))
                })
            }
            Fmt::Json | Fmt::JsonLines | Fmt::Html | Fmt::Markdown => {
                let f = fmt.text().unwrap();
                let name = convert::export_sheet_name(&form::file_name(&dest));
                Job::spawn(cx.ctx, move |cancel, progress| {
                    text_write::write(f, &st, rows.as_deref().map(|v| v.as_slice()), &dest, &name, Some(progress), cancel).map(Done::Workbook).map_err(|e| (false, e))
                })
            }
            // A copy: nothing to keep in step with, the tab stays on its file.
            Fmt::Delim(d) => {
                let opts = SaveOptions { enc: Some(plan.enc), delim: Some(d), bom: Some(plan.bom), backup: true, rows: rows.as_deref().cloned() };
                let n = total as u64;
                Job::spawn(cx.ctx, move |_, _| match save::save(&st, &dest, &opts, None, &mut false) {
                    Ok(_) => Ok(Done::Workbook(convert::Exported { rows: n + st.header as u64, cols: st.ncols, ..Default::default() })),
                    Err(e) => Err((false, e.to_string())),
                })
            }
            Fmt::Csv(_) | Fmt::Text => {
                let cur = Self::cur_fmt(&st);
                let delim = match fmt { Fmt::Csv(d) if fmt != cur => Some(d), Fmt::Text if cur != Fmt::Text => Some(0), _ => None };
                let opts = SaveOptions { enc: Some(plan.enc), delim, bom: Some(plan.bom), backup: true, rows: rows.as_deref().cloned() };
                let expect = if plan.in_place && !force { st.base.stamp.map(Into::into) } else { None };
                // Overwriting another program's version (`force`): a .bak of theirs first. The
                // tab counts its .bak made once the save is done.
                let mut backed = plan.in_place && !force && tab.backed_up;
                Job::spawn(cx.ctx, move |_, _| match save::save(&st, &dest, &opts, expect, &mut backed) {
                    Ok(stamp) => Ok(Done::Csv(stamp, csvfab_engine::base::file_sample(&dest, stamp.len), crate::tab::renumbering(&st))),
                    Err(SaveError::Changed) => Err((true, String::new())),
                    Err(e) => Err((false, e.to_string())),
                })
            }
        };
        self.stage = Stage::Writing { job, plan, version, total };
    }

    /// Written: the tab follows a save-as, a toast says what was done.
    fn done(&mut self, d: Done, plan: &Plan, version: u64, total: usize, cx: &mut Cx) {
        let file = form::file_name(&plan.dest);
        let tab = &mut *cx.tab;
        match d {
            Done::Workbook(j) => {
                let what = plan.fmt.noun();
                let mut what = what.to_string();
                if let Some(c) = what.get_mut(0..1) { c.make_ascii_uppercase(); }
                let limits = if j.truncated_rows || j.truncated_cols { " · truncated to Excel's limits" } else { "" };
                // A cell holds 32 767 characters at most in a workbook: the rest is not written.
                let cut = match j.truncated_cells {
                    0 => String::new(),
                    1 => " · 1 cell cut to 32 767 characters (Excel's limit)".into(),
                    k => format!(" · {} cells cut to 32 767 characters (Excel's limit)", form::n(k as usize)),
                };
                cx.out.push(Cmd::Toast(format!("{} | {what} written: {file} — {} rows{limits}{cut}.", tab.name, form::n(j.rows as usize))));
            }
            Done::Csv(..) if plan.rows.is_some() => {
                cx.out.push(Cmd::Toast(format!("{} | {} rows exported to {file}.", tab.name, form::n(total))));
                cx.out.push(Cmd::Open(plan.dest.clone()));
            }
            Done::Csv(stamp, sample, renumber) => {
                let from = tab.name.clone();
                // The file is ours now: no "changed on disk" for it while the table stands on its read.
                if plan.in_place { cx.out.push(Cmd::Wrote(tab.uid, tab.st().and_then(|st| st.base.stamp), stamp, sample)); }
                if let Some(t) = &mut tab.table { t.mark_saved_at(version); }
                // The file uses that delimiter and encoding now: the next read must agree.
                let st = tab.st().cloned();
                if let Some(st) = st {
                    let cur = Self::cur_fmt(&st);
                    if plan.fmt != cur {
                        tab.open_opts.delim = Some(match plan.fmt { Fmt::Csv(d) => d, _ => 0 });
                    }
                    if plan.enc != st.base.enc { tab.open_opts.enc = Some(plan.enc); }
                }
                tab.path = Some(plan.dest.clone());
                tab.name = file.clone();
                tab.backed_up = true;
                // Read again: the rows a tool showed or marked follow their new ids.
                if !tab.dirty() || !plan.in_place { tab.renumber = renumber; tab.reload(cx.ctx); }
                let rows = form::n(total);
                cx.out.push(Cmd::Toast(if plan.in_place {
                    format!("{file} | Saved in place — {rows} rows written.")
                } else {
                    format!("{file} | Saved as a new file — {rows} rows written, {from} left as it was.")
                }));
            }
        }
    }

    fn form(&mut self, cx: &mut Cx) -> bool {
        let tab_name = cx.tab.name.clone();
        let Some(st) = cx.tab.st().cloned() else { return false };
        let note = self.note(cx.tab, &st);
        let filtered = cx.tab.filtered();
        let shown = cx.tab.shown();
        let cur = Self::cur_fmt(&st);
        let mut submit = false;
        let mut cancel = false;
        let p = cx.p.clone();
        let id = egui::Id::unique(("save-name", self.uid));
        let (_, close) = form::modal(cx, "save", 380.0, "Save", |ui, _cx| {
            form::label(ui, &p, "File name", 0.0);
            let resp = form::field(ui, &p, id, &mut self.name, ui.available_width(), true, "");
            if self.focus {
                self.focus = false;
                resp.request_focus();
                // The stem selected, the extension kept: typing renames.
                let k = stem_of(&self.name).chars().count();
                let mut s = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
                s.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(k))));
                s.store(ui.ctx(), id);
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { submit = true; }
            ui.add_space(14.0);
            form::label(ui, &p, "Format", 0.0);
            let mut pick = None;
            let mut choices: Vec<(Fmt, String, &str)> = Vec::new();
            if cur == Fmt::Text { choices.push((Fmt::Text, "Text file — lines as they are (current)".into(), "¶")); }
            for (d, l, k) in DELIMS {
                choices.push((Fmt::Csv(d), if Fmt::Csv(d) == cur { format!("{l} (current)") } else { l.to_string() }, k));
            }
            for (f, l, k) in &choices {
                if form::radio_key(ui, &p, self.fmt == *f, l, k) { pick = Some(*f); }
                ui.add_space(6.0);
            }
            form::label(ui, &p, "Or a copy, without hidden columns", 8.0);
            const COPIES: [(Fmt, &str, &str); 8] = [
                (Fmt::Xlsx, "Excel", ".xlsx"), (Fmt::Json, "JSON", ".json"),
                (Fmt::Sqlite, "SQLite", ".sqlite"), (Fmt::JsonLines, "JSON Lines", ".jsonl"),
                (Fmt::Parquet, "Parquet", ".parquet"), (Fmt::Html, "HTML table", ".html"),
                (Fmt::Delim(b'\t'), "TSV", ".tsv"), (Fmt::Markdown, "Markdown table", ".md"),
            ];
            let half = (ui.available_width() - 16.0) / 2.0;
            for pair in COPIES.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    for (f, l, k) in pair {
                        ui.allocate_ui(egui::vec2(half, 20.0), |ui| {
                            ui.set_width(half);
                            if form::radio_key(ui, &p, self.fmt == *f, l, k) { pick = Some(*f); }
                        });
                    }
                });
                ui.add_space(6.0);
            }
            let other = matches!(self.fmt, Fmt::Delim(d) if d != b'\t');
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.allocate_ui(egui::vec2(half, 20.0), |ui| {
                    ui.set_width(half);
                    if form::radio(ui, &p, other, "Other delimiter") { pick = Some(Fmt::Delim(custom_delim(&self.custom).unwrap_or(b'~'))); }
                });
                if other {
                    let was = self.custom.clone();
                    let r = form::field(ui, &p, id.with("custom"), &mut self.custom, 56.0, true, "~");
                    if self.custom != was || r.lost_focus() {
                        let d = custom_delim(&self.custom).unwrap_or(b'~');
                        self.fmt = Fmt::Delim(d);
                    }
                }
            });
            ui.add_space(6.0);
            if let Some(f) = pick {
                // The extension follows the format; the stem typed is kept.
                let stem = stem_of(self.name.trim()).to_string();
                let ext = Path::new(&self.name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
                match f.ext() {
                    Some(e) => self.name = format!("{stem}{e}"),
                    None if matches!(ext.as_deref(), Some("xlsx" | "sqlite" | "parquet" | "json" | "jsonl" | "html" | "md" | "tsv")) => self.name = format!("{stem}{}", csv_ext(&tab_name)),
                    None => {}
                }
                self.fmt = f;
            }
            if self.fmt.delimited() {
                form::label(ui, &p, "Encoding", 6.0);
                if let Some(e) = enc_select(ui, &p, self.enc, st.base.enc) {
                    self.enc = e;
                    // Written as read: the BOM if the file had one; UTF-16 and UTF-32 always with one.
                    self.bom = if e == st.base.enc { st.base.bom } else { e.wide() };
                }
                ui.add_space(6.0);
                if self.enc == Enc::Utf8 {
                    ui.add_space(4.0);
                    form::check(ui, &p, &mut self.bom, "With a BOM — Excel then reads the accents right");
                    ui.add_space(6.0);
                }
            }
            if filtered {
                ui.add_space(6.0);
                let mut only = self.only();
                if form::check(ui, &p, &mut only, &format!("Only the rows shown ({} of {})", form::n(shown), form::n(st.len()))) { self.only_shown = Some(only); }
                ui.add_space(6.0);
            }
            ui.add_space(4.0);
            let job = form::rich(&p, &note, 12.0, p.txt_mut, ui.available_width());
            let g = ui.painter().layout_job(job);
            let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), g.size().y.max(35.0)), egui::Sense::hover());
            ui.painter().galley(r.min, g, p.txt_mut);
            ui.add_space(16.0);
            let blocked = self.subset_same_name(&tab_name);
            match form::actions(ui, &p, &[Btn::outline("Cancel"), Btn::primary("Save").enabled(!blocked)]) {
                Some(0) => cancel = true,
                Some(_) => submit = true,
                None => {}
            }
        });
        if submit && !self.subset_same_name(&tab_name) { self.submit(cx); }
        !(cancel || close)
    }

    /// The rows shown never overwrite the file itself.
    fn subset_same_name(&self, tab_name: &str) -> bool {
        self.only() && self.name.trim() == tab_name && self.fmt.delimited()
    }
}

impl Tool for SaveAs {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if cx.tab.table.is_none() { return false; }
        match std::mem::replace(&mut self.stage, Stage::Form) {
            Stage::Form => return self.form(cx),
            Stage::Picking(mut job, plan) => match job.poll() {
                Some(Some(dest)) => self.planned(Plan { dest, in_place: false, ..plan }, cx),
                Some(None) => {}
                None => self.stage = Stage::Picking(job, plan),
            },
            Stage::ConfirmOverwrite(plan) => {
                let a = Ask {
                    main: &format!("\"{}\" already exists. Overwrite it?", form::file_name(&plan.dest)),
                    sub: if plan.fmt.delimited() { "A timestamped .bak copy of it is kept." } else { "" },
                    choices: &[],
                    cancel: Some("Cancel"),
                    ok: "Overwrite",
                    danger: true,
                };
                match form::ask(cx, "save-exists", &a) {
                    Some(Answer::Ok) => self.check(plan, cx),
                    Some(_) => {}
                    None => self.stage = Stage::ConfirmOverwrite(plan),
                }
            }
            Stage::Checking(mut job, plan) => match job.poll() {
                Some(Some(l)) if l.chars == 0 => self.write(plan, false, cx),
                Some(Some(l)) => self.stage = Stage::ConfirmLossy(l, plan),
                Some(None) => {}
                None => self.stage = Stage::Checking(job, plan),
            },
            Stage::ConfirmLossy(l, plan) => {
                let a = Ask {
                    main: &l.say(plan.enc),
                    sub: "UTF-8 holds them all. Replaced, each is written as \"?\" and is lost.",
                    choices: &["Save as UTF-8"],
                    cancel: Some("Cancel"),
                    ok: "Replace with \"?\"",
                    danger: true,
                };
                match form::ask(cx, "save-lossy", &a) {
                    Some(Answer::Ok) => self.write(plan, false, cx),
                    Some(Answer::Choice(_)) => self.write(Plan { enc: Enc::Utf8, bom: true, ..plan }, false, cx),
                    Some(_) => {}
                    None => self.stage = Stage::ConfirmLossy(l, plan),
                }
            }
            Stage::Writing { mut job, plan, version, total } => match job.poll() {
                Some(Ok(d)) => {
                    self.done(d, &plan, version, total, cx);
                    return false;
                }
                Some(Err((true, _))) => self.stage = Stage::Changed(plan),
                Some(Err((_, e))) if e == convert::CANCELLED => {}
                Some(Err((_, e))) => {
                    let what = if Self::exports(plan.fmt) && !plan.fmt.delimited() { format!("Could not build the {}:", plan.fmt.noun()) } else { format!("Could not write \"{}\":", plan.dest.display()) };
                    // Written beside, then renamed: a failed save leaves the file as it was.
                    let safe = if plan.in_place { "\n\nThe original file is untouched." } else { "" };
                    self.stage = Stage::Failed(Alert::new(format!("{what}\n\n{e}{safe}")));
                }
                None => {
                    // A big file takes a moment: say so, and let it be stopped.
                    if job.elapsed().as_millis() > 300 && !self.quiet {
                        let book = Self::exports(plan.fmt) && !plan.fmt.delimited();
                        let k = job.progress.load(Ordering::Relaxed);
                        let sub = if book { format!("{} %", (k as f32 / (2 * total.max(1)) as f32 * 100.0).min(100.0) as u32) } else { format!("{} rows.", form::n(total)) };
                        // A workbook can be stopped; a CSV save is atomic and short: only hidden.
                        let a = Ask { main: &format!("Writing {}…", form::file_name(&plan.dest)), sub: &sub, choices: &[], cancel: None, ok: if book { "Stop" } else { "Hide" }, danger: false };
                        if form::ask(cx, "save-busy", &a).is_some() {
                            if book { job.cancel(); } else { self.quiet = true; }
                        }
                    }
                    cx.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    self.stage = Stage::Writing { job, plan, version, total };
                }
            },
            Stage::Changed(plan) => {
                let a = Ask {
                    main: "The file changed on disk",
                    sub: "Another program changed this file since it was opened. Overwrite it anyway?",
                    choices: &[],
                    cancel: Some("Cancel"),
                    ok: "Overwrite",
                    danger: true,
                };
                match form::ask(cx, "save-changed", &a) {
                    Some(Answer::Ok) => self.write(plan, true, cx),
                    Some(_) => {}
                    None => self.stage = Stage::Changed(plan),
                }
            }
            Stage::Failed(a) => {
                if a.main.is_empty() { return false; }
                if a.show(cx, "save-failed") {
                    // Back to the form: the name can be corrected.
                    self.stage = Stage::Form;
                    return true;
                }
                self.stage = Stage::Failed(a);
            }
        }
        // A stage that ended without a next one (a question answered no, a picker
        // dismissed) leaves `Form` behind: the dialog is done.
        !matches!(self.stage, Stage::Form)
    }
}

/// The save dialog on a CSV of the rows shown, under `name` (a test cannot tick the box).
#[cfg(test)]
pub fn shown_csv(tab: &Tab, name: &str) -> Box<dyn Tool> {
    Box::new(SaveAs { name: name.to_string(), only_shown: Some(true), ..SaveAs::new(tab, None) })
}

// --- Extract --------------------------------------------------------------------

enum XStage {
    Start,
    Picking(Job<Option<PathBuf>>, Arc<Vec<RowId>>),
    Writing(Job<Result<Option<Switch>, String>>, PathBuf, usize),
    Failed(Alert),
}

struct Extract {
    stage: XStage,
}

impl Extract {
    fn write(&mut self, dest: PathBuf, rows: Arc<Vec<RowId>>, cx: &mut Cx) {
        let Some(st) = cx.tab.export_st() else {
            self.stage = XStage::Failed(Alert::new("Every column is hidden: nothing to write."));
            return;
        };
        let n = rows.len();
        let d = dest.clone();
        let job = Job::spawn(cx.ctx, move |_, _| {
            let sw = save::new_file_switch(&st);
            let opts = SaveOptions { rows: Some((*rows).clone()), enc: sw.map(|_| Enc::Utf8), backup: false, ..Default::default() };
            save::save(&st, &d, &opts, None, &mut true).map(|_| sw).map_err(|e| e.to_string())
        });
        self.stage = XStage::Writing(job, dest, n);
    }
}

impl Tool for Extract {
    fn show(&mut self, cx: &mut Cx) -> bool {
        match std::mem::replace(&mut self.stage, XStage::Start) {
            XStage::Start => {
                if cx.tab.table.is_none() { return false; }
                let rows = cx.tab.shown_ids();
                if rows.is_empty() {
                    self.stage = XStage::Failed(Alert::new("No row to copy."));
                    return true;
                }
                match cx.tab.path.clone() {
                    Some(path) => {
                        // Beside the source, the first free name of the nextName() sequence.
                        let mut name = cx.tab.name.clone();
                        let dest = (0..1000).find_map(|_| { name = next_name(&name); let d = path.with_file_name(&name); (!d.exists()).then_some(d) });
                        match dest {
                            Some(d) => self.write(d, rows, cx),
                            None => self.stage = XStage::Failed(Alert::new("No free file name found beside the source.")),
                        }
                    }
                    None => self.stage = XStage::Picking(form::pick_save(cx.ctx, &next_name(&cx.tab.name), None), rows),
                }
            }
            XStage::Picking(mut job, rows) => match job.poll() {
                Some(Some(d)) => self.write(d, rows, cx),
                Some(None) => return false,
                None => self.stage = XStage::Picking(job, rows),
            },
            XStage::Writing(mut job, dest, n) => match job.poll() {
                Some(Ok(sw)) => {
                    cx.toast(format!("{} | {} rows exported to {}.", cx.tab.name, form::n(n), form::file_name(&dest)));
                    if let Some(sw) = sw { cx.toast(format!("{} | {}", form::file_name(&dest), sw.says())); }
                    cx.out.push(Cmd::Open(dest));
                    return false;
                }
                Some(Err(e)) => self.stage = XStage::Failed(Alert::new(format!("Could not write {}.\n\n{e}", form::file_name(&dest)))),
                None => self.stage = XStage::Writing(job, dest, n),
            },
            XStage::Failed(a) => {
                if a.show(cx, "extract-failed") { return false; }
                self.stage = XStage::Failed(a);
            }
        }
        true
    }
}

// --- Discard --------------------------------------------------------------------

struct Discard {
    summary: Option<String>,
    job: Option<Job<Option<String>>>,
}

impl Tool for Discard {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if !cx.tab.dirty() { return false; }
        // What would be lost, counted in the background (the summary of Review changes).
        if self.summary.is_none() && self.job.is_none() {
            let st = cx.tab.st().cloned().unwrap();
            self.job = Some(Job::spawn(cx.ctx, move |cancel, _| csvfab_engine::review::changes(&st, cancel).map(|c| c.summary())));
        }
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            // The engine's counts, grouped as the app shows numbers (59 347).
            self.summary = Some(r.unwrap_or_default().split(' ').map(|w| w.parse::<usize>().map_or_else(|_| w.to_string(), form::n)).collect::<Vec<_>>().join(" "));
        }
        let sub = match &self.summary {
            Some(s) if !s.is_empty() => format!("{s} in {}.", cx.tab.name),
            _ => format!("The edits of {} are dropped and the file is read again.", cx.tab.name),
        };
        let a = Ask { main: "Discard the edits?", sub: &sub, choices: &[], cancel: Some("Cancel"), ok: "Discard", danger: false };
        match form::ask(cx, "discard", &a) {
            Some(Answer::Ok) => {
                let tab = &mut *cx.tab;
                tab.table = None;
                tab.marks = Default::default();
                tab.reload(cx.ctx);
                false
            }
            Some(_) => false,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_names() {
        assert_eq!(next_name("foo.csv"), "foo-2.csv");
        assert_eq!(next_name("foo-3.csv"), "foo-4.csv");
        assert_eq!(next_name("v007.csv"), "v008.csv");
        assert_eq!(next_name("v099.tsv"), "v100.tsv");
        assert_eq!(next_name("x99"), "x100.csv");
        assert_eq!(next_name("a99999999999999999999999.csv"), "a100000000000000000000000.csv");
    }

    #[test]
    fn counts_unencodable() {
        let t = csvfab_engine::Table::new(Arc::new(csvfab_engine::Base::from_bytes("a,b\né,€\n✓,Ā\n".as_bytes().to_vec(), Default::default())), None);
        let no = AtomicBool::new(false);
        let l = unencodable(&t.st, Enc::Windows1252, None, &no).unwrap();
        assert_eq!((l.cells, l.chars, l.first), (2, 2, vec!['✓', 'Ā']));
        assert_eq!(unencodable(&t.st, Enc::Utf8, None, &no), Some(Lossy::default()));
        let t = csvfab_engine::Table::new(Arc::new(csvfab_engine::Base::from_bytes("a,b\n한글,東\n한,x\n".as_bytes().to_vec(), Default::default())), None);
        let l = unencodable(&t.st, Enc::ShiftJis, None, &no).unwrap();
        assert_eq!((l.cells, l.chars, &l.first), (2, 3, &vec!['한', '글']));
        assert_eq!(l.say(Enc::ShiftJis), "Shift_JIS cannot write 3 characters in 2 cells: \"한\", \"글\", and others.");
    }
}
