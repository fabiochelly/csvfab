//! Split into files (42-split-combine.js): the rows shown, one file per value
//! of a column (per agency, per month…), written in a new folder beside the
//! source — a folder of its own, so no file already there is overwritten.
//! The values are counted in a job, again whenever the column or the match
//! changes; the files are written in another. The source is never touched.

use super::form::{self, b, warn, Alert, Answer, Ask, Btn, PvCell, PvRow, Sty};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use csvfab_engine::split::{self, Group, Match, SPLIT_ASK, SPLIT_MAX};
use csvfab_engine::{RowId, State};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Split into files…", group: "Transform the file", keys: None, enabled: has_table, run: |cx| { if let Some(t) = SplitFiles::new(cx, None) { cx.out.push(Cmd::Tool(Box::new(t))) } } }]
}

/// Split into files by column `col` (the palette's "Split into files by: …").
pub fn open_by(cx: &mut Cx, col: usize) {
    if let Some(t) = SplitFiles::new(cx, Some(col)) { cx.out.push(Cmd::Tool(Box::new(t))) }
}

enum Stage {
    Form,
    Picking(Job<Option<PathBuf>>),
    Confirm(PathBuf),
    Writing(Job<split::Split>, String, usize),
    Failed(Alert),
}

struct SplitFiles {
    st: State,
    rows: Arc<Vec<RowId>>,
    filtered: bool,
    col: usize,
    how: Match,
    counting: Option<Job<Option<Vec<Group>>>>,
    groups: Option<Arc<Vec<Group>>>,
    stage: Stage,
}

impl SplitFiles {
    fn new(cx: &mut Cx, col: Option<usize>) -> Option<SplitFiles> {
        let st = cx.tab.st()?.clone();
        if st.ncols == 0 { return None; }
        let rows = cx.tab.shown_ids();
        // A column of categories: the first with 2 to 50 values (unless one is asked for).
        let col = col.filter(|&c| c < st.ncols).unwrap_or_else(|| split::guess_column(&st, &rows));
        let mut s = SplitFiles { st, rows, filtered: cx.tab.filtered(), col, how: Match::Exact, counting: None, groups: None, stage: Stage::Form };
        s.count(cx);
        Some(s)
    }

    fn count(&mut self, cx: &mut Cx) {
        let (st, rows, col, how, name) = (self.st.clone(), self.rows.clone(), self.col, self.how, cx.tab.name.clone());
        self.groups = None;
        self.counting = Some(Job::spawn(cx.ctx, move |cancel, progress| split::groups(&st, &rows, col, how, &name, cancel, Some(progress))));
    }

    fn folder(&self, cx: &Cx) -> String {
        split::folder_name(&cx.tab.name, &self.st.headers()[self.col], self.col)
    }

    fn write(&mut self, parent: PathBuf, cx: &mut Cx) {
        let Some(groups) = self.groups.clone() else { return };
        let (st, folder) = (self.st.clone(), self.folder(cx));
        let total = self.rows.len();
        let job = Job::spawn(cx.ctx, move |cancel, progress| split::write(&st, &groups, &parent, &folder, cancel, Some(progress)));
        self.stage = Stage::Writing(job, self.st.headers()[self.col].clone(), total);
    }

    fn form(&mut self, cx: &mut Cx) -> bool {
        if let Some(r) = self.counting.as_mut().and_then(|j| j.poll()) {
            self.counting = None;
            self.groups = r.map(Arc::new);
        }
        let p = cx.p.clone();
        let heads = self.st.headers();
        let name = cx.tab.name.clone();
        let where_ = if cx.tab.path.is_some() {
            format!("In a new folder beside {name}, one file per value — the rows shown{}, with the title line.", if self.filtered { " (filtered)" } else { "" })
        } else {
            "In a new folder of the folder you pick, one file per value — the rows shown, with the title line.".to_string()
        };
        let mut changed = false;
        let mut go = false;
        let mut cancel = false;
        let count_share = self.counting.as_ref().map(|j| j.progress.load(Ordering::Relaxed) as f32 / self.rows.len().max(1) as f32);
        let (_, close) = form::modal(cx, "splitf", 560.0, "Split into files", |ui, _| {
            form::label(ui, &p, &where_, 0.0);
            ui.add_space(3.0);
            form::row2(ui, |ui| {
                form::text(ui, "One file per value of", 12.0, p.txt_mut);
                let w = ui.available_width();
                changed |= form::select(ui, &p, "sf-col", w, 31.0, 14.0, &mut self.col, &heads);
            });
            form::opts(ui, |ui| {
                for (m, l) in [(Match::Exact, "Exact values"), (Match::Loose, "Ignoring case and surrounding spaces"), (Match::Slug, "Slugified")] {
                    if form::radio(ui, &p, self.how == m, l) && self.how != m { self.how = m; changed = true; }
                }
            });
            let groups = self.groups.clone();
            match (&groups, count_share) {
                (_, Some(s)) => form::stat(ui, &p, &format!("Counting the values…{}", if self.rows.len() > 50_000 { format!(" {} %", (s * 100.0).min(100.0) as u32) } else { String::new() })),
                (Some(g), None) => {
                    let n = g.len();
                    form::stat(ui, &p, &if n > SPLIT_MAX {
                        warn(format!("{} different values: more than {} files. Pick a column of categories (a city, a status, a month), not of identifiers.", b(form::n(n)), form::n(SPLIT_MAX)))
                    } else {
                        format!("{} file{} from {} {}rows", b(form::n(n)), if n == 1 { "" } else { "s" }, form::n(self.rows.len()), if self.filtered { "filtered " } else { "" })
                    });
                    let mut rows: Vec<PvRow> = g.iter().take(10).map(|g| PvRow::new(vec![
                        if g.value.is_empty() { PvCell::new("empty", Sty::Faint) } else { PvCell::txt(&g.value) },
                        PvCell::new(form::n(g.rows.len()), Sty::Sub),
                        PvCell::txt(&g.name),
                    ])).collect();
                    if n > 10 { rows.push(PvRow::new(vec![PvCell::new(format!("…and {} more", form::n(n - 10)), Sty::Faint).span(3)])); }
                    form::pv(ui, &p, "sf-pv", &[PvCell::txt("Value"), PvCell::txt("Rows"), PvCell::txt("File")], &rows, 220.0, 220.0);
                }
                (None, None) => form::stat(ui, &p, ""),
            }
            ui.add_space(16.0);
            let ok = groups.as_ref().is_some_and(|g| !g.is_empty() && g.len() <= SPLIT_MAX);
            match form::actions(ui, &p, &[Btn::outline("Cancel"), Btn::primary("Write the files").enabled(ok)]) {
                Some(0) => cancel = true,
                Some(_) => go = true,
                None => {}
            }
        });
        if changed { self.count(cx); }
        if go {
            match cx.tab.path.as_ref().and_then(|p| p.parent()) {
                Some(dir) => {
                    let dir = dir.to_path_buf();
                    if self.groups.as_ref().is_some_and(|g| g.len() > SPLIT_ASK) { self.stage = Stage::Confirm(dir) } else { self.write(dir, cx) }
                }
                None => self.stage = Stage::Picking(form::pick_folder(cx.ctx, None)),
            }
        }
        !(cancel || close)
    }
}

impl Tool for SplitFiles {
    fn show(&mut self, cx: &mut Cx) -> bool {
        match std::mem::replace(&mut self.stage, Stage::Form) {
            Stage::Form => return self.form(cx),
            Stage::Picking(mut j) => match j.poll() {
                Some(Some(dir)) => {
                    if self.groups.as_ref().is_some_and(|g| g.len() > SPLIT_ASK) { self.stage = Stage::Confirm(dir) } else { self.write(dir, cx) }
                }
                Some(None) => {}
                None => self.stage = Stage::Picking(j),
            },
            Stage::Confirm(dir) => {
                let n = self.groups.as_ref().map_or(0, |g| g.len());
                let a = Ask {
                    main: &format!("Write {} files?", form::n(n)),
                    sub: &format!("One per value of “{}”, in the folder “{}”.", self.st.headers()[self.col], self.folder(cx)),
                    choices: &[],
                    cancel: Some("Cancel"),
                    ok: &format!("Write {} files", form::n(n)),
                    danger: false,
                };
                match form::ask(cx, "sf-confirm", &a) {
                    Some(Answer::Ok) => self.write(dir, cx),
                    Some(_) => {}
                    None => self.stage = Stage::Confirm(dir),
                }
            }
            Stage::Writing(mut j, title, total) => match j.poll() {
                Some(r) => {
                    let n = self.groups.as_ref().map_or(0, |g| g.len());
                    let name = cx.tab.name.clone();
                    let folder = r.folder.as_ref().map(|f| form::file_name(f)).unwrap_or_default();
                    match r.error {
                        Some(e) if e.kind() == std::io::ErrorKind::Interrupted => return false,
                        Some(e) => {
                            let at = if folder.is_empty() { String::new() } else { format!(" in “{folder}”") };
                            self.stage = Stage::Failed(Alert::new(format!("The split stopped: {e}\n\n{} of {} files were written{at}. {name} itself is untouched.", form::n(r.written), form::n(n))));
                        }
                        None => {
                            cx.toast(format!("{name} | {} files written in the folder “{folder}”, one per value of {title}.", form::n(r.written)));
                            return false;
                        }
                    }
                }
                None => {
                    let done = j.progress.load(Ordering::Relaxed);
                    let a = Ask { main: "Writing the files…", sub: &format!("{} %", (done as f32 / total.max(1) as f32 * 100.0).min(100.0) as u32), choices: &[], cancel: None, ok: "Stop", danger: false };
                    if j.elapsed().as_millis() > 300 && form::ask(cx, "sf-busy", &a).is_some() { j.cancel(); }
                    cx.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    self.stage = Stage::Writing(j, title, total);
                }
            },
            Stage::Failed(a) => {
                if a.show(cx, "sf-failed") { return false; }
                self.stage = Stage::Failed(a);
            }
        }
        !matches!(self.stage, Stage::Form)
    }
}
