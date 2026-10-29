//! Look up values from another tab (VLOOKUP): this tab's rows are matched to
//! another table's on a key column, and the columns picked there are copied
//! in, right after the key. The other table is an open tab — the app lists
//! them with `publish` — or a file chosen here.

use super::edit_kit::{self as form, Btn, Live, Snap, Td, Th};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::edit::lookup::{self as lk, Index, LookupPlan, Match, Shown, Stats};
use csvfab_engine::State;
use eframe::egui::{vec2, Align, Id, Layout};
use std::cell::RefCell;
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Look up from another tab…", group: "Columns", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(LookupTool::new(cx.tab, None)))) }]
}

thread_local! {
    static TABS: RefCell<Vec<(u64, String, State)>> = const { RefCell::new(Vec::new()) };
}

/// The open tabs with a table (uid, name, snapshot), for the source list: the
/// app calls it once a frame, before drawing the tools (a few Arc clones a tab).
pub fn publish(tabs: &[Tab]) {
    // The rows each filtered tab shows (an Arc each: nothing copied), for Combine.
    SHOWN.with_borrow_mut(|m| {
        m.clear();
        m.extend(tabs.iter().filter(|t| t.filtered()).filter_map(|t| Some((t.uid, t.view.clone()?))));
    });
    TABS.with_borrow_mut(|v| {
        // Only when a table changed: the snapshot replaced may hold the last reference to
        // a closed or reloaded file (its mapping, its index), freed off the UI thread.
        let same = |(uid, name, st): &(u64, String, State), t: &Tab| t.uid == *uid && t.name == *name && t.st().is_some_and(|s| same_state(s, st));
        if v.len() == tabs.iter().filter(|t| t.st().is_some()).count() && v.iter().zip(tabs.iter().filter(|t| t.st().is_some())).all(|(e, t)| same(e, t)) {
            return;
        }
        let old = std::mem::take(v);
        v.extend(tabs.iter().filter_map(|t| Some((t.uid, t.name.clone(), t.st()?.clone()))));
        crate::app::drop_later(old);
    });
}

/// The open tabs with a table, as last published: (uid, name, snapshot).
pub fn open_tabs() -> Vec<(u64, String, State)> {
    TABS.with_borrow(|v| v.clone())
}

thread_local! {
    static SHOWN: std::cell::RefCell<std::collections::HashMap<u64, std::sync::Arc<Vec<csvfab_engine::RowId>>>> = Default::default();
}

/// The rows tab `uid` shows, when a filter or a sort narrows or orders them.
pub fn shown_rows(uid: u64) -> Option<std::sync::Arc<Vec<csvfab_engine::RowId>>> {
    SHOWN.with_borrow(|m| m.get(&uid).cloned())
}

/// The same snapshot of a table (the same parts, not equal contents).
fn same_state(a: &State, b: &State) -> bool {
    fn ptr<T>(x: &Option<std::sync::Arc<T>>) -> usize { x.as_ref().map_or(0, |a| std::sync::Arc::as_ptr(a) as usize) }
    std::sync::Arc::ptr_eq(&a.base, &b.base) && std::sync::Arc::ptr_eq(&a.cols, &b.cols) && ptr(&a.order) == ptr(&b.order) && ptr(&a.cmap) == ptr(&b.cmap) && a.ncols == b.ncols && a.header == b.header
}

struct Src {
    id: u64,
    name: String,
    st: State,
}

const MODES: [(Match, &str); 3] = [(Match::Exact, "Exact keys"), (Match::Loose, "Ignoring case and surrounding spaces"), (Match::Slug, "Slugified")];

pub struct LookupTool {
    key: usize,
    user_key: bool,
    /// Files chosen with "Choose a file…" (ids from u64::MAX down).
    files: Vec<Src>,
    /// The source picked, by id.
    src: Option<u64>,
    skey: usize,
    mode: Match,
    cols: Vec<usize>,
    idx: Live<(u64, usize, Match), Arc<Index>>,
    stats: Live<(u64, usize, Match, usize, u64), Stats>,
    picking: Option<Job<Option<Result<(String, State), String>>>>,
    error: Option<String>,
    apply: Option<Job<Option<LookupPlan>>>,
}

impl LookupTool {
    pub fn new(_tab: &Tab, col: Option<usize>) -> LookupTool {
        LookupTool {
            key: col.unwrap_or(0),
            user_key: col.is_some(),
            files: Vec::new(),
            src: None,
            skey: 0,
            mode: Match::Loose,
            cols: Vec::new(),
            idx: Live::default(),
            stats: Live::default(),
            picking: None,
            error: None,
            apply: None,
        }
    }

    /// The system's file dialog, then the file read — off the UI thread.
    fn pick(&mut self, ctx: &eframe::egui::Context) {
        self.picking = Some(Job::spawn(ctx, |_, _| {
            let path = rfd::FileDialog::new().add_filter("CSV and text", &["csv", "tsv", "txt", "tab", "psv", "dat"]).add_filter("All files", &["*"]).pick_file()?;
            Some(csvfab_engine::combine::Source::open(&path).map(|s| (s.name, s.st)).map_err(|e| format!("Cannot read {}: {e}", path.display())))
        }));
    }

    /// A new source: the key columns guessed (a title found on both sides), the ticks dropped.
    fn set_source(&mut self, id: u64, heads: &[String], src_heads: &[String]) {
        self.src = Some(id);
        let (k, sk) = lk::pick_keys(heads, src_heads, self.key, self.user_key);
        self.key = k;
        self.skey = sk;
        self.cols.clear();
    }
}

impl Tool for LookupTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let heads = snap.st.headers();
        let uid = cx.tab.uid;
        let mut srcs: Vec<Src> = TABS.with_borrow(|v| v.iter().filter(|t| t.0 != uid).map(|t| Src { id: t.0, name: t.1.clone(), st: t.2.clone() }).collect());
        srcs.extend(self.files.iter().map(|f| Src { id: f.id, name: f.name.clone(), st: f.st.clone() }));
        let (p, m) = (cx.p, cx.m);
        if let Some(r) = self.picking.as_mut().and_then(|j| j.poll()) {
            self.picking = None;
            match r {
                Some(Ok((name, st))) => {
                    let id = u64::MAX - self.files.len() as u64;
                    let sh = st.headers();
                    self.files.push(Src { id, name: name.clone(), st: st.clone() });
                    srcs.push(Src { id, name, st });
                    self.set_source(id, &heads, &sh);
                }
                Some(Err(e)) => self.error = Some(e),
                None => {}
            }
        }
        if let Some(r) = self.apply.as_mut().and_then(|j| j.poll()) {
            self.apply = None;
            if let (Some(plan), Some(t)) = (r, cx.tab.table.as_mut()) {
                let (old_n, key, n) = (t.st.ncols, self.key, plan.names.len());
                let name = srcs.iter().find(|s| Some(s.id) == self.src).map_or(String::new(), |s| s.name.clone());
                let msg = lk::apply(t, plan, &name);
                form::remap_columns(cx.tab, old_n, |c| Some(form::after_insert(key + 1, n, c)), cx.ctx);
                cx.toast(msg);
            }
            return false;
        }
        if srcs.is_empty() {
            if self.picking.is_some() {
                return true;
            }
            // No other tab: say so, and offer a file instead.
            return match form::message(cx.ctx, p, m, "lookup-none", self.error.as_deref().unwrap_or(lk::NO_OTHER_FILE), "Choose a file…", Some("Cancel"), false) {
                Some(0) => {
                    self.error = None;
                    self.pick(cx.ctx);
                    true
                }
                Some(_) => false,
                None => true,
            };
        }
        if self.src.is_none_or(|id| !srcs.iter().any(|s| s.id == id)) {
            let (id, sh) = (srcs[0].id, srcs[0].st.headers());
            self.set_source(id, &heads, &sh);
        }
        let si = srcs.iter().position(|s| Some(s.id) == self.src).unwrap_or(0);
        let src = &srcs[si];
        let src_heads = src.st.headers();
        self.key = self.key.min(heads.len().saturating_sub(1));
        self.skey = self.skey.min(src_heads.len().saturating_sub(1));
        self.cols.retain(|&c| c != self.skey && c < src_heads.len());
        // The source's index, then how this tab's rows fare against it.
        let ik = (src.id, self.skey, self.mode);
        if self.idx.stale(&ik) {
            let (st, sk, mode) = (src.st.clone(), self.skey, self.mode);
            self.idx.run(cx.ctx, ik.clone(), move |c| Index::build(&st, &csvfab_engine::edit::all_rows(&st), sk, mode, c).map(Arc::new));
        }
        self.idx.poll();
        let idx = self.idx.fresh(&ik).cloned();
        let sk = (src.id, self.skey, self.mode, self.key, cx.tab.generation);
        if let Some(ix) = &idx {
            if self.stats.stale(&sk) {
                let (s, ix, key) = (snap.clone(), ix.clone(), self.key);
                self.stats.run(cx.ctx, sk.clone(), move |c| lk::stats(&s.st, &s.all(), key, &ix, c));
            }
        }
        self.stats.poll();
        let stats = self.stats.fresh(&sk).copied();

        let names = lk::new_names(&heads, &src_heads, &self.cols, &src.name);
        let (mut go, mut cancel, mut choose) = (false, false, false);
        let mut src_pick = si;
        let mut opts_src: Vec<String> = srcs.iter().map(|s| s.name.clone()).collect();
        opts_src.push("Choose a file…".into());
        let open = form::modal(cx.ctx, p, m, "lookup", 560.0, "Look up values in another tab", |ui| {
            let w = ui.available_width();
            let cw = ((w - 20.0) / 3.0).floor();
            ui.allocate_ui_with_layout(vec2(w, 48.0), Layout::left_to_right(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.vertical(|ui| {
                    ui.set_width(cw);
                    form::label(ui, p, "Key in this tab", 0.0);
                    if form::select(ui, p, m, Id::unique("lk-key"), &mut self.key, &heads, cw) {
                        self.user_key = true;
                    }
                });
                ui.vertical(|ui| {
                    ui.set_width(cw);
                    form::label(ui, p, "Look up in the tab", 0.0);
                    form::select(ui, p, m, Id::unique("lk-src"), &mut src_pick, &opts_src, cw);
                });
                ui.vertical(|ui| {
                    ui.set_width(cw);
                    form::label(ui, p, "…on its column", 0.0);
                    form::select(ui, p, m, Id::unique("lk-skey"), &mut self.skey, &src_heads, cw);
                });
            });
            form::opts(ui, 10.0, |ui| {
                for (mode, label) in MODES {
                    if form::radio(ui, p, self.mode == mode, label) {
                        self.mode = mode;
                    }
                }
            });
            form::label(ui, p, "Columns to bring over — added right after the key, in the order ticked", 12.0);
            let items: Vec<(usize, String)> = src_heads.iter().cloned().enumerate().filter(|(c, _)| *c != self.skey).collect();
            form::ordered_list(ui, p, Id::unique("lk-cols"), &items, &mut self.cols);
            ui.add_space(10.0);
            let note = match (&self.error, stats) {
                (Some(e), _) => e.clone(),
                (None, Some(s)) => lk::stats_note(&s, &src.name, idx.as_ref().map_or(0, |i| i.dups)),
                (None, None) => format!("Reading {}…", src.name),
            };
            form::stat(ui, p, &note, p.txt_hi);
            match &idx {
                Some(ix) if !self.cols.is_empty() => {
                    let ids: Vec<_> = (0..snap.st.len().min(8)).map(|k| snap.st.data_id(k)).collect();
                    let mut head = vec![Th::Text(heads.get(self.key).cloned().unwrap_or_default())];
                    head.extend(names.iter().map(|n| Th::New(n.clone())));
                    let rows: Vec<Vec<Td>> = lk::preview(&snap.st, &ids, self.key, &src.st, ix, &self.cols, 8)
                        .into_iter()
                        .map(|(k, cells)| std::iter::once(Td::Src(k)).chain(cells.into_iter().map(|c| match c { Shown::Value(v) => Td::Text(v), o => Td::Empty(o.text().into()) })).collect())
                        .collect();
                    form::table(ui, p, Id::unique("lk-pv"), &head, &rows, &mut []);
                }
                _ => form::table(ui, p, Id::unique("lk-pv"), &[], &[vec![Td::Empty(lk::PICK_COLUMNS.into())]], &mut []),
            }
            ui.add_space(16.0);
            form::actions(ui, |ui| {
                let can = !self.cols.is_empty() && stats.is_some_and(|s| s.hit > 0) && self.apply.is_none();
                go = form::button(ui, p, Btn::Prim, "Add the columns", can).clicked();
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
        });
        if src_pick != si {
            if src_pick >= srcs.len() {
                choose = true;
            } else {
                let sh = srcs[src_pick].st.headers();
                self.set_source(srcs[src_pick].id, &heads, &sh);
            }
        }
        if choose {
            self.pick(cx.ctx);
        }
        if go {
            if let Some(ix) = idx {
                let (s, src_st, key, cols) = (snap.clone(), src.st.clone(), self.key, self.cols.clone());
                self.apply = Some(Job::spawn(cx.ctx, move |c, _| lk::plan(&s.st, &s.all(), key, &src_st, &ix, &cols, names, c)));
            }
        }
        (open && !cancel) || self.apply.is_some()
    }
}
