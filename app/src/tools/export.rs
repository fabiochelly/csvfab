//! Save as, export and extract (14-save-export.js): the save dialog — a file
//! name, a format (CSV with its delimiter and encoding, an Excel workbook, a
//! SQLite database), only the rows shown — written in a job; Extract (the
//! rows shown to a new file beside the source, opened as a tab); Discard (the
//! edits dropped, the file read again).
//!
//! A new CSV name is a save-as: the tab moves onto the new file and the
//! source keeps its last saved content. A workbook or a database is a second
//! artefact and never takes the tab over.

use super::form::{self, Alert, Answer, Ask, Btn};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::convert::{self, Format};
use csvfab_engine::save::{self, SaveError, SaveOptions};
use csvfab_engine::{Enc, RowId, State};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "Save as Excel workbook", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Xlsx)); cx.out.push(Cmd::Tool(Box::new(t))) } },
        Command { name: "Save as SQLite database", group: "Save", keys: None, enabled: has_table, run: |cx| { let t = SaveAs::new(cx.tab, Some(Fmt::Sqlite)); cx.out.push(Cmd::Tool(Box::new(t))) } },
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
}

const DELIMS: [(u8, &str, &str); 4] = [(b';', "CSV — semicolon", ";"), (b',', "CSV — comma", ","), (b'\t', "CSV — tab", "\\t"), (b'|', "CSV — pipe", "|")];
const ENCS: [(Enc, &str); 6] = [
    (Enc::Utf8, "UTF-8"),
    (Enc::Windows1252, "Windows-1252"),
    (Enc::Latin1, "ISO-8859-1 (Latin-1)"),
    (Enc::Latin9, "ISO-8859-15 (Latin-9, with €)"),
    (Enc::MacRoman, "Mac Roman (Excel for Mac)"),
    (Enc::Utf16Le, "UTF-16"),
];

fn enc_name(e: Enc) -> &'static str {
    e.label()
}

pub(crate) fn single_byte(e: Enc) -> bool {
    matches!(e, Enc::Windows1252 | Enc::Latin1 | Enc::Latin9 | Enc::MacRoman)
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

/// Cells (headers included) holding a character `enc` cannot write, and the
/// first such character. Rows read in that very encoding hold none. None if cancelled.
pub(crate) fn unencodable(st: &State, enc: Enc, rows: Option<&[RowId]>, cancel: &AtomicBool) -> Option<(usize, char)> {
    let same = st.base.enc == enc;
    let mut n = 0;
    let mut first = None;
    let mut test = |v: &str| {
        if let Some(c) = csvfab_engine::enc::unencodable(enc, v) {
            n += 1;
            first.get_or_insert(c);
        }
    };
    for h in st.headers() { test(&h); }
    let all: Vec<RowId>;
    let ids: &[RowId] = match rows {
        Some(r) => r,
        None => { all = (0..st.len()).map(|p| st.data_id(p)).collect(); &all }
    };
    let mut tmp = Vec::new();
    for (k, &id) in ids.iter().enumerate() {
        if k % 8192 == 0 && cancel.load(Ordering::Relaxed) { return None; }
        if same && st.pristine(id) { continue; }
        for c in 0..st.ncols { st.with_cell(id, c, &mut tmp, |v| test(v)); }
    }
    Some((n, first.unwrap_or('?')))
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
    Csv,
}

enum Stage {
    Form,
    Picking(Job<Option<PathBuf>>, Plan),
    ConfirmOverwrite(Plan),
    Checking(Job<Option<(usize, char)>>, Plan),
    ConfirmLossy(usize, char, Plan),
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
    only_shown: bool,
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
        let name = match fmt {
            Fmt::Xlsx => convert::export_name(&tab.name, Format::Xlsx),
            Fmt::Sqlite => convert::export_name(&tab.name, Format::Sqlite),
            _ => tab.name.clone(),
        };
        let name = if name.is_empty() { "table.csv".into() } else { name };
        SaveAs { uid: tab.uid, name, fmt, enc, bom, only_shown: false, stage: Stage::Form, focus: true, quiet: false }
    }

    fn cur_fmt(st: &State) -> Fmt {
        if st.base.dialect.lines { Fmt::Text } else { Fmt::Csv(st.base.dialect.delim) }
    }

    /// The note under the choices (updateSaveNote).
    fn note(&self, tab: &Tab, st: &State) -> String {
        let name = self.name.trim();
        let where_ = if tab.path.is_some() { "beside the source" } else { "where you choose" };
        if let Fmt::Xlsx | Fmt::Sqlite = self.fmt {
            let what = if self.fmt == Fmt::Xlsx { "workbook" } else { "database" };
            return format!("Writes the {what} {where_}. {} is left as it is{}.", tab.name, if tab.dirty() { " and keeps its pending edits" } else { "" });
        }
        if self.subset(tab) {
            return format!("Writes the {} rows shown to a new file {where_} and opens it. {} is left as it is.", form::n(tab.shown()), tab.name);
        }
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
                format!("Overwrites {}{} — a timestamped .bak copy is kept.", tab.name, if changes.is_empty() { String::new() } else { format!(", {}", changes.join(", ")) })
            }
        } else {
            format!("Writes a new file {where_} and switches this tab to it. {} stays as it was last saved.", tab.name)
        }
    }

    fn subset(&self, tab: &Tab) -> bool {
        self.only_shown && tab.filtered()
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
            Fmt::Csv(_) | Fmt::Text if Path::new(&name).extension().is_none() => name.push_str(".csv"),
            _ => {}
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
        plan.exists = !plan.in_place && plan.dest.exists();
        if plan.exists {
            self.stage = Stage::ConfirmOverwrite(plan);
            return;
        }
        self.check(plan, cx);
    }

    /// A single-byte encoding holds ~250 characters: what would be lost is said first.
    fn check(&mut self, plan: Plan, cx: &mut Cx) {
        let Some(st) = cx.tab.st().cloned() else { return };
        let csv = matches!(plan.fmt, Fmt::Csv(_) | Fmt::Text);
        if csv && single_byte(plan.enc) && (plan.enc != st.base.enc || cx.tab.dirty()) {
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
        let st = table.st.clone();
        let version = table.version();
        let total = plan.rows.as_ref().map_or(st.len(), |r| r.len());
        let (fmt, dest, rows) = (plan.fmt, plan.dest.clone(), plan.rows.clone());
        let job = match fmt {
            Fmt::Xlsx | Fmt::Sqlite => {
                let f = if fmt == Fmt::Xlsx { Format::Xlsx } else { Format::Sqlite };
                let sheet = convert::export_sheet_name(&form::file_name(&dest));
                Job::spawn(cx.ctx, move |cancel, progress| {
                    convert::export(f, &st, rows.as_deref().map(|v| v.as_slice()), &dest, &sheet, Some(progress), cancel).map(Done::Workbook).map_err(|e| (false, e))
                })
            }
            Fmt::Csv(_) | Fmt::Text => {
                let cur = Self::cur_fmt(&st);
                let delim = match fmt { Fmt::Csv(d) if fmt != cur => Some(d), Fmt::Text if cur != Fmt::Text => Some(0), _ => None };
                let opts = SaveOptions { enc: Some(plan.enc), delim, bom: Some(plan.bom), backup: true, rows: rows.as_deref().cloned() };
                let expect = if plan.in_place && !force { st.base.stamp } else { None };
                let mut backed = if plan.in_place { tab.backed_up } else { false };
                if plan.in_place { tab.backed_up = true; }
                Job::spawn(cx.ctx, move |_, _| match save::save(&st, &dest, &opts, expect, &mut backed) {
                    Ok(_) => Ok(Done::Csv),
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
                let what = if plan.fmt == Fmt::Xlsx { "Workbook" } else { "Database" };
                let limits = if j.truncated_rows || j.truncated_cols { " · truncated to Excel's limits" } else { "" };
                cx.out.push(Cmd::Toast(format!("{} | {what} written: {file} — {} rows{limits}.", tab.name, form::n(j.rows as usize))));
            }
            Done::Csv if plan.rows.is_some() => {
                cx.out.push(Cmd::Toast(format!("{} | {} rows exported to {file}.", tab.name, form::n(total))));
                cx.out.push(Cmd::Open(plan.dest.clone()));
            }
            Done::Csv => {
                let from = tab.name.clone();
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
                if !tab.dirty() || !plan.in_place { tab.reload(cx.ctx); }
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
            choices.push((Fmt::Xlsx, "Excel workbook".into(), ".xlsx"));
            choices.push((Fmt::Sqlite, "SQLite database — one table, typed columns".into(), ".sqlite"));
            for (f, l, k) in &choices {
                if form::radio_key(ui, &p, self.fmt == *f, l, k) { pick = Some(*f); }
                ui.add_space(6.0);
            }
            if let Some(f) = pick {
                // The extension follows the format; the stem typed is kept.
                let stem = stem_of(self.name.trim()).to_string();
                let ext = Path::new(&self.name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
                match f {
                    Fmt::Xlsx => self.name = format!("{stem}.xlsx"),
                    Fmt::Sqlite => self.name = format!("{stem}.sqlite"),
                    _ if matches!(ext.as_deref(), Some("xlsx" | "sqlite")) => self.name = format!("{stem}{}", csv_ext(&tab_name)),
                    _ => {}
                }
                self.fmt = f;
            }
            if matches!(self.fmt, Fmt::Csv(_) | Fmt::Text) {
                form::label(ui, &p, "Encoding", 6.0);
                let mut encs: Vec<(Enc, &str)> = ENCS.to_vec();
                if !encs.iter().any(|e| e.0 == st.base.enc) { encs.push((st.base.enc, st.base.enc.label())); }
                for (e, l) in encs {
                    let label = if e == st.base.enc { format!("{l} (current)") } else { l.to_string() };
                    if form::radio(ui, &p, self.enc == e, &label) {
                        self.enc = e;
                        // Written as read: the BOM if the file had one; UTF-16 always with one.
                        self.bom = if e == st.base.enc { st.base.bom } else { e.is_utf16() };
                    }
                    ui.add_space(6.0);
                }
                if self.enc == Enc::Utf8 {
                    ui.add_space(4.0);
                    form::check(ui, &p, &mut self.bom, "With a BOM — Excel then reads the accents right");
                    ui.add_space(6.0);
                }
            }
            if filtered {
                ui.add_space(6.0);
                form::check(ui, &p, &mut self.only_shown, &format!("Only the rows shown ({} of {})", form::n(shown), form::n(st.len())));
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
        self.only_shown && self.name.trim() == tab_name && matches!(self.fmt, Fmt::Csv(_) | Fmt::Text)
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
                    sub: if matches!(plan.fmt, Fmt::Csv(_) | Fmt::Text) { "A timestamped .bak copy of it is kept." } else { "" },
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
                Some(Some((0, _))) => self.write(plan, false, cx),
                Some(Some((n, c))) => self.stage = Stage::ConfirmLossy(n, c, plan),
                Some(None) => {}
                None => self.stage = Stage::Checking(job, plan),
            },
            Stage::ConfirmLossy(n, c, plan) => {
                let a = Ask {
                    main: &format!("{} cells contain characters {} cannot represent (e.g. \"{c}\").", form::n(n), enc_name(plan.enc)),
                    sub: "They will be written as \"?\".",
                    choices: &[],
                    cancel: Some("Cancel"),
                    ok: "Write them as \"?\"",
                    danger: false,
                };
                match form::ask(cx, "save-lossy", &a) {
                    Some(Answer::Ok) => self.write(plan, false, cx),
                    Some(_) => {}
                    None => self.stage = Stage::ConfirmLossy(n, c, plan),
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
                    let what = match plan.fmt { Fmt::Xlsx => "Could not build the workbook:", Fmt::Sqlite => "Could not build the database:", _ => "Could not save:" };
                    self.stage = Stage::Failed(Alert::new(format!("{what}\n\n{e}")));
                }
                None => {
                    // A big file takes a moment: say so, and let it be stopped.
                    if job.elapsed().as_millis() > 300 && !self.quiet {
                        let book = matches!(plan.fmt, Fmt::Xlsx | Fmt::Sqlite);
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

// --- Extract --------------------------------------------------------------------

enum XStage {
    Start,
    Picking(Job<Option<PathBuf>>, Arc<Vec<RowId>>),
    Writing(Job<Result<(), String>>, PathBuf, usize),
    Failed(Alert),
}

struct Extract {
    stage: XStage,
}

impl Extract {
    fn write(&mut self, dest: PathBuf, rows: Arc<Vec<RowId>>, cx: &mut Cx) {
        let Some(st) = cx.tab.st().cloned() else { return };
        let n = rows.len();
        let d = dest.clone();
        let job = Job::spawn(cx.ctx, move |_, _| {
            let opts = SaveOptions { rows: Some((*rows).clone()), backup: false, ..Default::default() };
            save::save(&st, &d, &opts, None, &mut true).map(|_| ()).map_err(|e| e.to_string())
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
                Some(Ok(())) => {
                    cx.toast(format!("{} | {} rows exported to {}.", cx.tab.name, form::n(n), form::file_name(&dest)));
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
        assert_eq!(unencodable(&t.st, Enc::Windows1252, None, &no), Some((2, '✓')));
        assert_eq!(unencodable(&t.st, Enc::Utf8, None, &no), Some((0, '?')));
    }
}
