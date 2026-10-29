//! Compare with another tab or file (20-compare.js): two versions of one export
//! (Monday's and today's), rows matched on a key column, the other columns
//! paired by name. The other file is read in a job, the comparison runs in
//! another (again at each change of key or match). Results: marks (the
//! differing cells tinted, the other file's value on hover, only those rows
//! shown), a status column, or the rows only the other file has, appended.

use super::form::{self, b, plural, warn, Alert, Btn, PvCell, PvRow, Sty};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Marks;
use csvfab_engine::base::{Base, OpenOptions};
use csvfab_engine::compare::{self, Compare};
use csvfab_engine::group::Match;
use csvfab_engine::{RowId, State, Table};
use std::path::PathBuf;
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Compare with another tab or file…", group: "Rows", keys: None, enabled: has_table, run: |cx| { let t = Cmp::new(cx); cx.out.push(Cmd::Tool(Box::new(t))) } }]
}

struct Other {
    name: String,
    st: State,
    /// The open tab it is (None: a file read here).
    tab: Option<u64>,
}

struct Cmp {
    key: usize,
    skey: usize,
    user_key: bool,
    how: Match,
    picking: Option<Job<Option<PathBuf>>>,
    reading: Option<(String, Job<std::io::Result<Base>>)>,
    other: Option<Other>,
    run: Option<Job<Option<Compare>>>,
    res: Option<Arc<Compare>>,
    alert: Option<Alert>,
}

impl Cmp {
    fn new(cx: &mut Cx) -> Cmp {
        let dir = cx.tab.path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf());
        let key = cx.tab.grid.cursor.map_or(0, |c| c.col);
        let mut c = Cmp { key, skey: 0, user_key: false, how: Match::Loose, picking: None, reading: None, other: None, run: None, res: None, alert: None };
        // Screenshot hook `cmpfile:<path>`: that file instead of the picker. Else another open
        // tab when there is one (the most recent), the file picker when not.
        let uid = cx.tab.uid;
        match crate::shot::take("cmpfile") {
            Some(path) => c.picking = Some(Job::spawn(cx.ctx, move |_, _| Some(PathBuf::from(path)))),
            None => match super::lookup::open_tabs().into_iter().rev().find(|t| t.0 != uid) {
                Some((id, name, st)) => c.set_other(cx, name, st, Some(id)),
                None => c.picking = Some(pick(cx, dir)),
            },
        }
        c
    }

    fn set_other(&mut self, cx: &mut Cx, name: String, st: State, tab: Option<u64>) {
        let here = cx.tab.st().map(|s| s.headers()).unwrap_or_default();
        let (k, sk) = compare::guess_keys(&here, &st.headers(), self.key.min(here.len().saturating_sub(1)));
        if !self.user_key { self.key = k; }
        self.skey = sk;
        self.other = Some(Other { name, st, tab });
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Cx) {
        let (Some(here), Some(o)) = (cx.tab.st().cloned(), &self.other) else { return };
        let there = o.st.clone();
        let (k, sk, how) = (self.key, self.skey, self.how);
        self.res = None;
        self.run = Some(Job::spawn(cx.ctx, move |cancel, progress| compare::compare(&here, &there, k, sk, how, cancel, Some(progress))));
    }

    fn poll(&mut self, cx: &mut Cx) -> bool {
        if let Some(r) = self.picking.as_mut().and_then(|j| j.poll()) {
            self.picking = None;
            match r {
                Some(path) => {
                    let name = form::file_name(&path);
                    self.reading = Some((name, Job::spawn(cx.ctx, move |_, progress| Base::open(&path, OpenOptions::default(), Some(progress)))));
                }
                // Nothing picked: nothing to compare with.
                None if self.other.is_none() => return false,
                None => {}
            }
        }
        if let Some(r) = self.reading.as_mut().and_then(|j| j.1.poll()) {
            let (name, _) = self.reading.take().unwrap();
            match r {
                Ok(base) => {
                    let st = Table::new(Arc::new(base), None).st;
                    self.set_other(cx, name, st, None);
                }
                Err(e) => self.alert = Some(Alert::new(format!("Cannot read {name}.\n\n{e}"))),
            }
        }
        if let Some(r) = self.run.as_mut().and_then(|j| j.poll()) {
            self.run = None;
            self.res = r.map(Arc::new);
        }
        true
    }

    /// Changed rows and rows only here, as marks; the tip gives the other file's value.
    fn mark(&self, cx: &mut Cx) {
        let (Some(res), Some(o)) = (&self.res, &self.other) else { return };
        let Some(st) = cx.tab.st().cloned() else { return };
        let mut m = Marks::default();
        m.rows.extend(res.only_here.iter().copied());
        for c in &res.changed {
            m.rows.insert(c.id);
            for (col, v) in &c.cells { m.cells.insert((c.id, *col), format!("In {}: {}", o.name, if v.is_empty() { "(empty)" } else { v })); }
        }
        let ids: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).filter(|id| m.rows.contains(id)).collect();
        cx.tab.marks = Arc::new(m);
        cx.tab.restrict_to(format!("differences with {}", o.name), ids, cx.ctx);
        cx.toast(format!("{} | {} and {} only here, marked — hover a tinted cell to see the value in {}.", cx.tab.name, plural(res.changed.len(), "changed row"), plural(res.only_here.len(), "row"), o.name));
    }

    /// One column at the end: "changed: A, B", "same", "only here" (empty without a key).
    fn status_column(&self, cx: &mut Cx) {
        let (Some(res), Some(o)) = (&self.res, &self.other) else { return };
        let Some(t) = &mut cx.tab.table else { return };
        let status = res.status_column(&t.st);
        let n = t.st.ncols;
        let name = format!("vs {}", o.name);
        let changes: Vec<(RowId, usize, String)> = status.into_iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(p, s)| (t.st.data_id(p), n, s)).collect();
        t.batch(|t| {
            t.insert_column(n, &name);
            t.set_cells(&changes);
        });
        cx.tab.after_edit(true, cx.ctx);
        cx.toast(format!("{} | column \"{name}\" added — not written yet, use Save.", cx.tab.name));
    }

    /// The rows only the other file has, their columns matched by name.
    fn append(&self, cx: &mut Cx) {
        let (Some(res), Some(o)) = (&self.res, &self.other) else { return };
        let Some(t) = &mut cx.tab.table else { return };
        let rows = res.rows_to_append(&o.st, t.st.ncols);
        let k = rows.len();
        t.batch(|t| {
            let ids = t.insert_rows(t.st.len(), k);
            let changes: Vec<(RowId, usize, String)> = ids.iter().zip(rows).flat_map(|(&id, r)| r.into_iter().enumerate().filter(|(_, v)| !v.is_empty()).map(move |(c, v)| (id, c, v))).collect();
            t.set_cells(&changes);
        });
        cx.tab.after_edit(true, cx.ctx);
        let left = if res.only_there_cols.is_empty() { String::new() } else { format!(" (its columns {} have no match here and were left out)", res.only_there_cols.join(", ")) };
        cx.toast(format!("{} | {} only in {} appended at the end{left} — not written yet, use Save.", cx.tab.name, plural(k, "row"), o.name));
    }
}

fn pick(cx: &Cx, dir: Option<PathBuf>) -> Job<Option<PathBuf>> {
    let ctx = cx.ctx.clone();
    Job::spawn(&ctx, move |_, _| {
        let mut d = rfd::FileDialog::new().set_title("Compare with…").add_filter("CSV and text", &["csv", "tsv", "txt", "tab", "psv", "dat"]).add_filter("All files", &["*"]);
        if let Some(dir) = dir { d = d.set_directory(dir); }
        d.pick_file()
    })
}

impl Tool for Cmp {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if !self.poll(cx) { return false; }
        if let Some(a) = &self.alert {
            if a.show(cx, "cmp-alert") {
                self.alert = None;
                if self.other.is_none() { return false; }
            }
            return true;
        }
        if self.other.is_none() && self.reading.is_none() { return true; }   // the picker is up
        let Some(here) = cx.tab.st().cloned() else { return false };
        let p = cx.p.clone();
        let heads = here.headers();
        let oname = self.other.as_ref().map_or_else(|| self.reading.as_ref().map(|r| r.0.clone()).unwrap_or_default(), |o| o.name.clone());
        let oheads = self.other.as_ref().map(|o| o.st.headers()).unwrap_or_default();
        let (mut changed, mut repick) = (false, false);
        let mut act = None;
        // The other side: an open tab, the file read here, or another file.
        let uid = cx.tab.uid;
        let tabs: Vec<(u64, String, State)> = super::lookup::open_tabs().into_iter().filter(|t| t.0 != uid).collect();
        let cur_tab = self.other.as_ref().and_then(|o| o.tab);
        let mut srcs: Vec<String> = tabs.iter().map(|t| t.1.clone()).collect();
        let file_slot = (cur_tab.is_none() && !oname.is_empty()).then(|| { srcs.push(oname.clone()); srcs.len() - 1 });
        srcs.push("Another file…".into());
        let mut src_k = cur_tab.and_then(|id| tabs.iter().position(|t| t.0 == id)).or(file_slot).unwrap_or(0);
        let mut src_pick = None;
        let (_, close) = form::modal(cx, "compare", 560.0, "Compare with another tab or file", |ui, _| {
            // Three columns: the key here, the other file, its key.
            let w = (ui.available_width() - 20.0) / 3.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.vertical(|ui| {
                    ui.set_width(w);
                    form::label(ui, &p, "Key in this tab", 0.0);
                    if form::select(ui, &p, "cmp-key", w, 31.0, 14.0, &mut self.key, &heads) { self.user_key = true; changed = true; }
                });
                ui.vertical(|ui| {
                    ui.set_width(w);
                    form::label(ui, &p, "Compare with", 0.0);
                    if form::select(ui, &p, "cmp-src", w, 31.0, 14.0, &mut src_k, &srcs) {
                        if src_k + 1 == srcs.len() { repick = true; } else if src_k < tabs.len() { src_pick = Some(src_k); }
                    }
                });
                ui.vertical(|ui| {
                    ui.set_width(w);
                    form::label(ui, &p, "…on its column", 0.0);
                    if form::select(ui, &p, "cmp-skey", w, 31.0, 14.0, &mut self.skey, &oheads) { changed = true; }
                });
            });
            form::opts(ui, |ui| {
                for (m, l) in [(Match::Exact, "Exact"), (Match::Loose, "Ignoring case and surrounding spaces"), (Match::Slug, "Slugified")] {
                    if form::radio(ui, &p, self.how == m, l) && self.how != m { self.how = m; changed = true; }
                }
            });
            let res = self.res.clone();
            match &res {
                None => form::stat(ui, &p, &format!("Reading {oname}…")),
                Some(r) => {
                    let mut s = [
                        format!("{} changed", b(form::n(r.changed.len()))),
                        format!("{} identical", b(form::n(r.same))),
                        format!("{} only here", b(form::n(r.only_here.len()))),
                        format!("{} only in {oname}", b(form::n(r.only_there.len()))),
                    ].join(" · ");
                    s += &format!("\nCompared on {} common column{}", r.pairs.len(), if r.pairs.len() == 1 { "" } else { "s" });
                    if !r.only_here_cols.is_empty() { s += &format!(" · {} only here", r.only_here_cols.len()); }
                    if !r.only_there_cols.is_empty() { s += &format!(" · {} only in {oname}", r.only_there_cols.len()); }
                    if r.no_key > 0 { s += &format!(" · {}", warn(format!("{} without a key", plural(r.no_key, "row")))); }
                    if r.dup_there > 0 { s += &format!(" · {}", warn(format!("{} in {oname}", plural(r.dup_there, "repeated key")))); }
                    if r.pairs.is_empty() && r.only_here.is_empty() && r.only_there.is_empty() { s += &format!("\n{}", warn("No column in common besides the key.")); }
                    form::stat(ui, &p, &s);
                    let mut rows = Vec::new();
                    for c in r.changed.iter().take(60) {
                        let mut d = PvCell::default();
                        for (k, (col, other)) in c.cells.iter().enumerate() {
                            if k > 0 { d.parts.push((" · ".into(), Sty::Txt)); }
                            d.parts.push((format!("{}: ", heads.get(*col).map_or("", |s| s.as_str())), Sty::Txt));
                            d.parts.push(form::value_part(&here.cell(c.id, *col), Sty::New, 60));
                            d.parts.push((" / ".into(), Sty::Faint));
                            d.parts.push(form::value_part(other, Sty::Old, 60));
                        }
                        rows.push(PvRow::new(vec![PvCell::new("changed", Sty::Chg), PvCell::value(&here.cell(c.id, r.key), Sty::Txt, 60), d]));
                    }
                    for &id in r.only_here.iter().take(20) { rows.push(PvRow::new(vec![PvCell::new("only here", Sty::Add), PvCell::value(&here.cell(id, r.key), Sty::Txt, 60), PvCell::txt("")])); }
                    if let Some(o) = &self.other {
                        for &id in r.only_there.iter().take(20) { rows.push(PvRow::new(vec![PvCell::new("only there", Sty::Del), PvCell::value(&o.st.cell(id, r.other_key), Sty::Txt, 60), PvCell::txt("")])); }
                    }
                    form::pv(ui, &p, "cmp-pv", &[PvCell::txt(""), PvCell::txt(heads.get(r.key).cloned().unwrap_or_default()), PvCell::txt("Differences")], &rows, 220.0, 260.0);
                }
            }
            ui.add_space(16.0);
            let (go, col, add) = match &res {
                Some(r) => (!r.changed.is_empty() || !r.only_here.is_empty(), true, !r.only_there.is_empty()),
                None => (false, false, false),
            };
            let add_label = match &res { Some(r) if !r.only_there.is_empty() => format!("Append the {} only there", plural(r.only_there.len(), "row")), _ => "Append the rows only there".into() };
            act = form::actions(ui, &p, &[
                Btn::outline("Cancel"),
                Btn::outline(&add_label).enabled(add),
                Btn::outline("Add a status column").enabled(col).tip("changed / same / only here"),
                Btn::primary("Mark the differences").enabled(go).tip("Tint the differing cells, show only those rows"),
            ]);
        });
        if let Some(i) = src_pick {
            let (id, name, st) = tabs[i].clone();
            if cur_tab != Some(id) { self.set_other(cx, name, st, Some(id)); }
        }
        if repick && self.picking.is_none() {
            let dir = cx.tab.path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf());
            self.picking = Some(pick(cx, dir));
        }
        if changed { self.refresh(cx); }
        match act {
            Some(0) => return false,
            Some(1) => { self.append(cx); return false; }
            Some(2) => { self.status_column(cx); return false; }
            Some(3) => { self.mark(cx); return false; }
            _ => {}
        }
        !close
    }
}
