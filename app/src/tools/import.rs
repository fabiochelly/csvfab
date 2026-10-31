//! Opening an Excel or OpenDocument workbook, a SQLite database, a JSON or a Parquet
//! file (27-import.js): it is first written as a CSV beside it — choosing the sheet
//! or table when several hold data, and for JSON whether to flatten it or read
//! it as raw text — then that CSV opens. A compressed file goes the same way, the
//! file it holds written beside it (asking which, for a zip of several), and what
//! comes out opens as any file does. The conversion runs in a job, cancellable.
//! See engine/src/convert.

use super::form::{self, Alert, Answer, Ask};
use super::{Cmd, Cx, Tool};
use crate::job::Job;
use csvfab_engine::convert::{self, Format, Imported, Sheet};
use eframe::egui::{self, pos2, vec2, Rect, Sense, Stroke, StrokeKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// The delimiter of the CSV written: the one chosen for the next file (the status bar with
/// no file open, as the web's reading defaults), else `;` (a `;` file gets decimal commas,
/// the French spreadsheet convention). The app sets it each frame.
pub static NEXT_DELIM: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn delim() -> u8 {
    match NEXT_DELIM.load(Ordering::Relaxed) { 0 => b';', d => d }
}

enum Stage {
    /// JSON: flatten, or open as text?
    AskJson,
    /// A workbook or a database: its sheets are listed first.
    Start,
    Listing(Job<Result<Vec<Sheet>, String>>),
    Pick { sheets: Vec<Sheet>, act: usize },
    /// The CSV is already there: open it, or convert again over it?
    Exists { sheet: Option<String>, dest: PathBuf },
    Converting { job: Job<Result<Imported, String>>, dest: PathBuf },
    Failed(Alert),
    /// Nothing more to do (its CSV is open already).
    Done,
}

struct Import {
    path: PathBuf,
    format: Format,
    name: String,
    stage: Stage,
    /// The source's folder, when it cannot be written: the CSV goes to `convert::fallback_dir`.
    elsewhere: Option<PathBuf>,
}

/// Paths the user chose to open as raw text (a JSON file read line by line):
/// the app opens them with `OpenOptions { delim: Some(0), .. }`, bypassing the
/// import. See `take_text_opens`.
static TEXT_OPENS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The files to open as raw text, asked since the last call (the app drains it each frame).
pub fn take_text_opens() -> Vec<PathBuf> {
    TEXT_OPENS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

/// The tool that converts `path` (a workbook, a database, JSON, Parquet, a compressed file), None if it is none of them.
pub fn open(path: PathBuf) -> Option<Box<dyn Tool>> {
    let format = convert::import_format(&path)?;
    let name = file_name(&path);
    let stage = if format == Format::Json { Stage::AskJson } else { Stage::Start };
    Some(Box::new(Import { path, format, name, stage, elsewhere: None }))
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Import {
    /// Sheets listed: pick one when several hold data, else go on.
    fn listed(&mut self, sheets: Vec<Sheet>, cx: &mut Cx) {
        if matches!(self.format, Format::Sqlite | Format::Zip) && sheets.is_empty() {
            self.stage = Stage::Failed(Alert::new(format!("\"{}\" has no table.", self.name)));
            return;
        }
        let offered = convert::offered(&sheets);
        if offered.len() > 1 {
            self.stage = Stage::Pick { sheets, act: 0 };
            return;
        }
        // A database's CSV is always named after its table, a zip's file is its own.
        let sheet = matches!(self.format, Format::Sqlite | Format::Zip).then(|| offered.first().map_or(sheets[0].name.clone(), |s| s.name.clone()));
        self.go(sheet, cx);
    }

    /// The sheet is known: convert, unless its CSV is already there.
    fn go(&mut self, sheet: Option<String>, cx: &mut Cx) {
        let mut dest = convert::dest_for(self.format, &self.path, sheet.as_deref());
        // A folder that cannot be written (a read-only mount, an archive's view): the CSV goes
        // to the documents folder, and the toast says so.
        let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
        if !convert::dir_writable(&dir) {
            if let (Some(f), Some(n)) = (convert::fallback_dir(), dest.file_name()) {
                dest = f.join(n);
                self.elsewhere = Some(dir);
            }
        }
        // Its CSV is open in a tab: that tab, as it is (converting again would write under it).
        let canon = std::fs::canonicalize(&dest).unwrap_or_else(|_| dest.clone());
        if super::lookup::open_tabs().iter().any(|(_, _, st)| st.base.path.as_deref() == Some(canon.as_path())) {
            cx.out.push(Cmd::Open(dest));
            self.stage = Stage::Done;
        } else if dest.exists() {
            self.stage = Stage::Exists { sheet, dest };
        } else {
            self.convert(sheet, dest, false, cx);
        }
    }

    fn convert(&mut self, sheet: Option<String>, dest: PathBuf, backup: bool, cx: &mut Cx) {
        let (f, src, d) = (self.format, self.path.clone(), dest.clone());
        let job = Job::spawn(cx.ctx, move |cancel, progress| convert::import(f, &src, sheet.as_deref(), delim(), &d, backup, Some(progress), cancel));
        self.stage = Stage::Converting { job, dest };
    }
}

impl Tool for Import {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let name = self.name.clone();
        match &mut self.stage {
            Stage::AskJson => {
                let a = Ask {
                    main: &format!("Open \"{name}\" as a table or as text?"),
                    sub: "Flatten to CSV turns its records into rows and columns, written to a CSV beside it. Raw text opens the file itself, one line per row, every space kept.",
                    choices: &["Raw text"],
                    cancel: Some("Cancel"),
                    ok: "Flatten to CSV",
                    danger: false,
                };
                match form::ask(cx, "import-json", &a) {
                    Some(Answer::Ok) => self.go(None, cx),
                    Some(Answer::Choice(_)) => {
                        if let Ok(mut v) = TEXT_OPENS.lock() { v.push(self.path.clone()); }
                        cx.ctx.request_repaint();
                        return false;
                    }
                    Some(Answer::Cancel) => return false,
                    None => {}
                }
            }
            // One table: nothing to list, nothing to ask.
            Stage::Start if matches!(self.format, Format::Parquet | Format::Packed) => self.go(None, cx),
            Stage::Start => {
                let (path, f) = (self.path.clone(), self.format);
                self.stage = Stage::Listing(Job::spawn(cx.ctx, move |_, _| match f {
                    Format::Sqlite => convert::sqlite::tables(&path),
                    Format::Book => convert::book_read::sheets(&path),
                    Format::Zip => convert::packed::members(&path),
                    _ => convert::xlsx_read::sheets(&path),
                }));
            }
            Stage::Listing(job) => {
                if let Some(r) = job.poll() {
                    match r {
                        Ok(s) => self.listed(s, cx),
                        Err(e) => self.stage = Stage::Failed(Alert::new(format!("Could not read \"{name}\".\n\n{e}"))),
                    }
                }
            }
            Stage::Pick { sheets, act } => match pick(cx, &name, self.format, sheets, act) {
                Some(Some(s)) => self.go(Some(s), cx),
                Some(None) => return false,
                None => {}
            },
            Stage::Exists { sheet, dest } => {
                let csv = file_name(dest);
                let a = Ask {
                    main: &match &self.elsewhere {
                        None => format!("\"{csv}\" already exists beside {name}."),
                        Some(_) => format!("\"{csv}\" already exists in {}.", dest.parent().map_or(String::new(), |d| d.display().to_string())),
                    },
                    sub: &format!("Open it as it is, or convert {name} again and overwrite it (a .bak copy is kept)?"),
                    choices: &["Open the existing CSV"],
                    cancel: Some("Cancel"),
                    ok: "Convert again",
                    danger: true,
                };
                match form::ask(cx, "import-exists", &a) {
                    Some(Answer::Ok) => {
                        let (s, d) = (sheet.take(), dest.clone());
                        self.convert(s, d, true, cx);
                    }
                    Some(Answer::Choice(_)) => {
                        cx.out.push(Cmd::Open(dest.clone()));
                        return false;
                    }
                    Some(Answer::Cancel) => return false,
                    None => {}
                }
            }
            Stage::Converting { job, dest } => {
                if let Some(r) = job.poll() {
                    match r {
                        Ok(j) => {
                            let mut note = convert::import_note(&name, &file_name(dest), self.format, &j);
                            if let Some(ro) = &self.elsewhere {
                                note += &format!(" Its folder ({}) cannot be written: the CSV is in {}.", ro.display(), dest.parent().map_or(String::new(), |d| d.display().to_string()));
                            }
                            cx.toast(note);
                            cx.out.push(Cmd::Open(dest.clone()));
                            return false;
                        }
                        Err(e) if e == convert::CANCELLED => return false,
                        Err(e) => self.stage = Stage::Failed(Alert::new(format!("Could not convert \"{name}\".\n\n{e}"))),
                    }
                } else {
                    // Said only when the conversion takes a while.
                    if job.elapsed().as_millis() > 400 || crate::shot::take("import-busy").is_some() {
                        let done = job.progress.load(Ordering::Relaxed);
                        let what = match self.format { Format::Json => "records read", Format::Packed | Format::Zip => "MB unpacked", _ => "rows read" };
                        let a = Ask { main: &format!("Converting {name}…"), sub: &format!("{} {what}.", form::n(done)), choices: &[], cancel: None, ok: "Cancel", danger: false };
                        if form::ask(cx, "import-busy", &a).is_some() { job.cancel(); }
                    }
                    cx.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
            Stage::Failed(a) => {
                if a.show(cx, "import-failed") { return false; }
            }
            Stage::Done => return false,
        }
        true
    }
}

/// The sheet (or table) picker (`#dlg.sheet-dlg`): the sheets with data, their
/// size when the file gives it; ↑ ↓ and Enter, a click, a double-click.
/// Some(Some(name)): picked; Some(None): cancelled; None: still open.
fn pick(cx: &mut Cx, file: &str, f: Format, sheets: &[Sheet], act: &mut usize) -> Option<Option<String>> {
    let noun = f.noun();
    let list: Vec<&Sheet> = convert::offered(sheets);
    let empty = sheets.iter().filter(|s| !s.filled).count();
    let hidden = sheets.iter().filter(|s| s.filled && s.hidden).count();
    let pl = |k: usize, w: &str| format!("{k} {w}{noun}{}", if k > 1 { "s" } else { "" });
    let left: Vec<String> = [(empty, "empty "), (hidden, "hidden ")].iter().filter(|x| x.0 > 0).map(|&(k, w)| pl(k, w)).collect();
    let sub = if f == Format::Zip { "Pick the one to open: it is written beside the zip.".to_string() } else { format!("Pick the one to open: it becomes a CSV of its own, named after the {noun}.{}", if left.is_empty() { String::new() } else { format!(" Not listed: {}.", left.join(" and ")) }) };
    let main = if f == Format::Zip { format!("\"{file}\" holds {} tables", list.len()) } else { format!("\"{file}\" has {} {noun}s with data", list.len()) };
    let (up, down) = cx.ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)));
    if up { *act = (*act + list.len() - 1) % list.len(); }
    if down { *act = (*act + 1) % list.len(); }
    let mut dbl = None;
    let a = Ask { main: &main, sub: &sub, choices: &[], cancel: Some("Cancel"), ok: "Open", danger: false };
    let r = form::ask_with(cx, "import-pick", &a, 480.0, |ui, p| {
        ui.add_space(14.0);
        let h = (list.len() as f32 * 31.0 + 10.0).min(320.0);
        let (bx, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        ui.painter().rect_stroke(bx, 8.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
        let inner = bx.shrink(5.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        egui::ScrollArea::vertical().id_salt("sheets").max_height(inner.height()).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, s) in list.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(inner.width(), 31.0), Sense::click());
                let on = i == *act;
                let pt = ui.painter();
                if on { pt.rect_filled(r, 6.0, crate::theme::alpha(p.accent, 0.16)); } else if resp.hovered() { pt.rect_filled(r, 6.0, crate::theme::alpha(p.gray, 0.14)); }
                if on && (up || down) { resp.scroll_to_me(None); }
                let y = r.center().y;
                let dim = s.dim.map(|(rows, cols)| format!("{}{} row{} × {} col{}", if s.approx { "≈ " } else { "" }, form::n(rows as usize), if rows == 1 { "" } else { "s" }, form::n(cols as usize), if cols == 1 { "" } else { "s" })).unwrap_or_default();
                let dg = pt.layout_no_wrap(dim, form::ui_font(12.0), p.txt_dim);
                let mut right = r.max.x - 10.0 - dg.size().x;
                pt.galley(pos2(right, (y - dg.size().y / 2.0).round()), dg, p.txt_dim);
                if s.view {
                    let vg = pt.layout_no_wrap("view".into(), form::ui_font(11.0), p.txt_mut);
                    let vr = Rect::from_min_size(pos2(right - 12.0 - 10.0 - vg.size().x - 12.0, y - 8.0), vec2(vg.size().x + 12.0, 16.0));
                    pt.rect_stroke(vr, 4.0, Stroke::new(1.0, p.brd_2), StrokeKind::Inside);
                    pt.galley(pos2(vr.min.x + 6.0, (y - vg.size().y / 2.0).round()), vg, p.txt_mut);
                    right = vr.min.x;
                }
                let mut job = egui::text::LayoutJob::simple_singleline(s.name.clone(), form::medium(13.5), if on { p.txt_hi } else { p.txt });
                job.wrap = egui::text::TextWrapping { max_width: (right - r.min.x - 10.0 - 12.0).max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = pt.layout_job(job);
                pt.galley(pos2(r.min.x + 10.0, (y - g.size().y / 2.0).round()), g, p.txt);
                if resp.clicked() { *act = i; }
                if resp.double_clicked() { dbl = Some(i); }
            }
        });
        None
    });
    if let Some(i) = dbl { return Some(Some(list[i].name.clone())); }
    match r {
        Some(Answer::Ok) => Some(Some(list[*act].name.clone())),
        Some(_) => Some(None),
        None => None,
    }
}
