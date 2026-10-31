//! Combine files (42-split-combine.js): this tab, the other open tabs and the files added, one
//! after the other in one new file, columns matched by their title (in any
//! order; case, accents and punctuation ignored by default), whatever each
//! file's delimiter or encoding. The first (this tab) leads: its columns come
//! first, its delimiter and line breaks are the result's. Each file added is
//! read in a job; the result is written in another, then opened as a tab.
//! Nothing read is touched.

use super::form::{self, b, muted, warn, Alert, Btn, PvCell, PvRow, Sty};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use csvfab_engine::combine::{self, OutCol, Source};
use csvfab_engine::save::Switch;
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Combine files…", group: "Transform the file", keys: None, enabled: has_table, run: |cx| { if let Some(t) = Combine::new(cx) { cx.out.push(Cmd::Tool(Box::new(t))) } } }]
}

enum Read {
    Reading(Job<std::io::Result<Source>>),
    Ok(Source),
    Err(String),
}

struct Entry {
    name: String,
    on: bool,
    read: Read,
}

enum Stage {
    Form,
    Picking(Job<Option<PathBuf>>),
    Writing(Job<std::io::Result<Option<Switch>>>, PathBuf, usize, usize),
    Failed(Alert),
}

/// (loose titles, common columns only, a source column) as last chosen.
static LAST: std::sync::Mutex<Option<(bool, bool, bool)>> = std::sync::Mutex::new(None);

struct Combine {
    /// This tab first, then the files added.
    entries: Vec<Entry>,
    adding: Option<Job<Vec<PathBuf>>>,
    loose: bool,
    common: bool,
    source_col: bool,
    stage: Stage,
}

fn delim_name(d: u8) -> String {
    match d {
        b'\t' => "tab".into(),
        b';' => "semicolon".into(),
        b',' => "comma".into(),
        b'|' => "pipe".into(),
        d => format!("“{}”", d as char),
    }
}

impl Combine {
    fn new(cx: &mut Cx) -> Option<Combine> {
        let st = cx.tab.export_st()?;
        // A filtered tab gives its rows shown, a tab hiding columns the others.
        let rows = cx.tab.filtered().then(|| cx.tab.shown_ids());
        let first = Entry { name: cx.tab.name.clone(), on: true, read: Read::Ok(Source { name: cx.tab.name.clone(), st, rows }) };
        // The other open tabs, ticked (as 42-split-combine.js: "open tabs into one new file").
        let uid = cx.tab.uid;
        let mut entries = vec![first];
        // Each with its rows shown when it is filtered, as this one.
        entries.extend(super::lookup::open_tabs().into_iter().filter(|t| t.0 != uid && combine::combinable(&t.2)).map(|(id, name, st)| Entry { name: name.clone(), on: true, read: Read::Ok(Source { name, st: super::lookup::without_hidden(id, st), rows: super::lookup::shown_rows(id) }) }));
        // The options chosen last (as the web's kept them).
        let (loose, common, source_col) = LAST.lock().ok().and_then(|l| *l).unwrap_or((true, false, false));
        Some(Combine { entries, adding: None, loose, common, source_col, stage: Stage::Form })
    }

    /// The sources ticked and read, this tab first.
    fn sources(&self) -> Vec<Source> {
        self.entries.iter().filter(|e| e.on).filter_map(|e| if let Read::Ok(s) = &e.read { combine::combinable(&s.st).then(|| s.clone()) } else { None }).collect()
    }

    fn poll(&mut self, cx: &mut Cx) {
        // Screenshot / test hook `cbfile:<path>`: that file added, as if picked.
        if let Some(f) = crate::shot::take("cbfile") {
            let f = PathBuf::from(f);
            let ctx = cx.ctx.clone();
            self.adding = Some(Job::spawn(&ctx, move |_, _| vec![f]));
        }
        if let Some(files) = self.adding.as_mut().and_then(|j| j.poll()) {
            self.adding = None;
            for f in files {
                // Opened as tabs too, as the web's "Open files…" (behind this one).
                cx.out.push(Cmd::OpenBehind(f.clone()));
                let name = form::file_name(&f);
                let job = Job::spawn(cx.ctx, move |_, _| Source::open(&f));
                self.entries.push(Entry { name, on: true, read: Read::Reading(job) });
            }
        }
        for e in &mut self.entries {
            if let Read::Reading(j) = &mut e.read {
                if let Some(r) = j.poll() {
                    e.read = match r {
                        Ok(s) if !combine::combinable(&s.st) => Read::Err("raw text: no columns to match".into()),
                        Ok(s) => Read::Ok(s),
                        Err(_) => Read::Err("cannot be read".into()),
                    };
                }
            }
        }
    }

    fn write(&mut self, dest: PathBuf, cx: &mut Cx) {
        let sources = self.sources();
        let cols = combine::columns(&sources, self.loose, self.common);
        let f = combine::format(&sources);
        let total: usize = sources.iter().map(|s| s.len()).sum();
        let (n, d, source_col) = (sources.len(), dest.clone(), self.source_col);
        let job = Job::spawn(cx.ctx, move |cancel, progress| {
            let sw = combine::switch(&sources, &f);
            combine::write(&sources, &cols, source_col, &f, &d, cancel, Some(progress)).map(|_| sw)
        });
        self.stage = Stage::Writing(job, dest, total, n);
    }

    fn form(&mut self, cx: &mut Cx) -> bool {
        self.poll(cx);
        let p = cx.p.clone();
        let sources = self.sources();
        let cols: Vec<OutCol> = if sources.len() >= 2 { combine::columns(&sources, self.loose, self.common) } else { Vec::new() };
        let mut go = false;
        let mut cancel = false;
        let mut add = false;
        let ctx = cx.ctx.clone();
        let (_, close) = form::modal(cx, "combine", 640.0, "Combine files", |ui, _| {
            form::label(ui, &p, "This tab, the other open tabs and the files added, one after the other in one new file. Columns are matched by their title, in any order; a file without a column leaves it empty. Each file keeps its own delimiter and encoding until then.", 0.0);
            // The files: a tick, the name, what it gives.
            ui.add_space(5.0);
            egui::Frame::new().stroke(egui::Stroke::new(1.0, p.brd)).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
                let lead = self.entries.iter().position(|e| e.on);
                for (k, e) in self.entries.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.set_min_height(26.0);
                        let label = e.name.clone();
                        // Each can be left out, this tab too (as the web's); the first ticked leads.
                        form::check(ui, &p, &mut e.on, &label);
                        if Some(k) == lead { form::text(ui, "(first: its layout leads)", 12.0, p.txt_faint); }
                        if e.on {
                            let what = match &e.read {
                                Read::Reading(_) => muted("reading…"),
                                Read::Err(m) => warn(m),
                                Read::Ok(s) => {
                                    let n = s.len();
                                    format!("{} row{}{} · {} columns · {} · {}", form::n(n), if n == 1 { "" } else { "s" }, if s.rows.is_some() { " shown (filtered)" } else { "" }, form::n(s.st.ncols), delim_name(s.st.base.dialect.delim), s.st.base.enc.label())
                                }
                            };
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let job = form::rich(&p, &what, 12.0, p.txt, f32::INFINITY);
                                ui.label(job);
                            });
                        }
                    });
                }
            });
            ui.add_space(8.0);
            if form::row_buttons(ui, &p, &[Btn::outline("Add files…")], |_| {}).is_some() { add = true; }
            form::opts(ui, |ui| {
                if form::radio(ui, &p, self.loose, "Titles matched ignoring case, accents and punctuation") { self.loose = true; }
                if form::radio(ui, &p, !self.loose, "Titles matched exactly") { self.loose = false; }
            });
            form::opts(ui, |ui| {
                form::check(ui, &p, &mut self.common, "Only the columns every file has");
                form::check(ui, &p, &mut self.source_col, "Add a column with each row's file name");
            });
            if sources.len() < 2 {
                form::stat(ui, &p, "Add at least one file to combine with this tab (Add files…): its columns are matched with this tab's by their title.");
            } else {
                let f = combine::format(&sources);
                let rows: usize = sources.iter().map(|s| s.len()).sum();
                form::stat(ui, &p, &format!(
                    "{} rows from {} files → {} columns, written as {} · {}{}",
                    b(form::n(rows)), form::n(sources.len()), b(form::n(cols.len() + self.source_col as usize)), delim_name(f.delim), f.enc.label(),
                    if !sources[0].st.header { format!(" · {}", muted("no title line in the first file: columns matched by position")) } else { String::new() }
                ));
                let heads: Vec<Vec<String>> = sources.iter().map(|s| s.st.headers()).collect();
                let mut rows: Vec<PvRow> = cols.iter().map(|o| {
                    let miss: Vec<&str> = sources.iter().zip(&o.from).filter(|(_, f)| f.is_none()).map(|(s, _)| s.name.as_str()).collect();
                    let mut names: Vec<&str> = o.from.iter().enumerate().filter_map(|(k, f)| f.map(|c| heads[k][c].as_str())).filter(|h| *h != o.name).collect();
                    names.dedup();
                    let mut c1 = PvCell::txt(&o.name);
                    if !names.is_empty() { c1 = c1.and(format!(" = {}", names.join(", ")), Sty::Faint); }
                    let c2 = if miss.is_empty() {
                        PvCell::txt(format!("all {}", form::n(sources.len())))
                    } else {
                        PvCell::new(format!("{} of {}", form::n(sources.len() - miss.len()), form::n(sources.len())), Sty::Warn).and(format!(" — empty for {}", miss.join(", ")), Sty::Faint)
                    };
                    PvRow::new(vec![c1, c2])
                }).collect();
                if self.source_col { rows.push(PvRow::new(vec![PvCell::new("source file", Sty::Empty), PvCell::new("the name of the file each row comes from", Sty::Faint)])); }
                form::pv(ui, &p, "cb-pv", &[PvCell::txt("Column"), PvCell::txt("Found in")], &rows, 220.0, 320.0);
            }
            ui.add_space(16.0);
            match form::actions(ui, &p, &[Btn::outline("Cancel"), Btn::primary("Write the combined file").enabled(sources.len() >= 2 && !cols.is_empty())]) {
                Some(0) => cancel = true,
                Some(_) => go = true,
                None => {}
            }
        });
        if add && self.adding.is_none() {
            let dir = cx.tab.path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf());
            self.adding = Some(form::pick_files(&ctx, dir));
        }
        if go {
            match &cx.tab.path {
                Some(path) => match combine::dest_path(path) {
                    Some(d) => self.write(d, cx),
                    None => self.stage = Stage::Failed(Alert::new("No free file name found beside the first file.")),
                },
                None => {
                    let name = &cx.tab.name;
                    let suggested = format!("{} - combined{}", csvfab_engine::split::stem_of(name), csvfab_engine::split::csv_ext(name));
                    self.stage = Stage::Picking(form::pick_save(cx.ctx, &suggested, None));
                }
            }
        }
        !(cancel || close)
    }
}

/// This tab and `file` combined at once into `dest` (a test cannot press the form's button).
#[cfg(test)]
pub fn written_to(cx: &mut Cx, file: &std::path::Path, dest: PathBuf) -> Option<Box<dyn Tool>> {
    let mut c = Combine::new(cx)?;
    c.entries.push(Entry { name: form::file_name(file), on: true, read: Read::Ok(Source::open(file).ok()?) });
    c.write(dest, cx);
    Some(Box::new(c))
}

impl Tool for Combine {
    fn show(&mut self, cx: &mut Cx) -> bool {
        match std::mem::replace(&mut self.stage, Stage::Form) {
            Stage::Form => return self.form(cx),
            Stage::Picking(mut j) => match j.poll() {
                Some(Some(d)) => self.write(d, cx),
                Some(None) => {}
                None => self.stage = Stage::Picking(j),
            },
            Stage::Writing(mut j, dest, total, n) => match j.poll() {
                Some(Ok(sw)) => {
                    cx.toast(format!("{} | {} rows of {} files combined into {}.", cx.tab.name, form::n(total), form::n(n), form::file_name(&dest)));
                    if let Some(sw) = sw { cx.toast(format!("{} | {}", form::file_name(&dest), sw.says())); }
                    cx.out.push(Cmd::Open(dest));
                    return false;
                }
                Some(Err(e)) if e.kind() == std::io::ErrorKind::Interrupted => return false,
                Some(Err(e)) => self.stage = Stage::Failed(Alert::new(format!("Could not write the combined file:\n{e}\n\nThe files combined are untouched."))),
                None => {
                    let done = j.progress.load(Ordering::Relaxed);
                    let a = form::Ask { main: &format!("Combining {} files…", form::n(n)), sub: &format!("{} %", (done as f32 / total.max(1) as f32 * 100.0).min(100.0) as u32), choices: &[], cancel: None, ok: "Stop", danger: false };
                    if j.elapsed().as_millis() > 300 && form::ask(cx, "cb-busy", &a).is_some() { j.cancel(); }
                    cx.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    self.stage = Stage::Writing(j, dest, total, n);
                }
            },
            Stage::Failed(a) => {
                if a.show(cx, "cb-failed") { return false; }
                self.stage = Stage::Failed(a);
            }
        }
        !matches!(self.stage, Stage::Form)
    }
}

impl Drop for Combine {
    fn drop(&mut self) {
        if let Ok(mut l) = LAST.lock() { *l = Some((self.loose, self.common, self.source_col)); }
    }
}
