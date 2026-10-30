//! Split a column into several, merge several into one: every data row is
//! rewritten (filtered or not), the titles follow; each a single undo step.

use super::edit_kit::{self as form, Btn, Live, Snap, Td, Th};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::edit::columns::{self as cols, MergePlan, SplitPlan, SplitStats, Splitter, MERGE_CHIPS, SPLIT_CHIPS};
use eframe::egui::{vec2, Align, Id, Layout, RichText, Sense};
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "Split column…", group: "Columns", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(SplitTool::new(cx.tab, None)))) },
        Command { name: "Merge columns…", group: "Columns", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(MergeTool::new(cx.tab.grid.cursor.map(|c| c.col))))) },
    ]
}

/// A line in `warn` (`.stat-line .warn`).
fn warn(ui: &mut eframe::egui::Ui, p: &crate::theme::Palette, text: &str) {
    ui.label(RichText::new(text).font(form::ui_font(12.0)).color(p.warn));
}

/* ---- Split ---------------------------------------------------------- */

type SepKey = (String, bool, bool, bool);

pub struct SplitTool {
    col: usize,
    sep: String,
    regex: bool,
    collapse: bool,
    trim: bool,
    keep: bool,
    n_text: String,
    user_n: bool,
    /// The new columns' titles, and whether each is still the automatic one.
    names: Vec<(String, bool)>,
    sp: Option<(SepKey, Result<Option<Arc<Splitter>>, String>)>,
    /// The parts' counts and the first values holding a separator.
    ana: Live<(usize, SepKey, u64), (SplitStats, Vec<String>)>,
    /// The analysis the column count was proposed from.
    proposed: Option<(usize, SepKey, u64)>,
    apply: Option<Job<Option<SplitPlan>>>,
    focused: bool,
}

impl SplitTool {
    pub fn new(tab: &Tab, col: Option<usize>) -> SplitTool {
        SplitTool {
            col: col.unwrap_or_else(|| form::cursor_col(tab)),
            sep: String::new(),
            regex: false,
            collapse: true,
            trim: true,
            keep: false,
            n_text: "2".into(),
            user_n: false,
            names: Vec::new(),
            sp: None,
            ana: Live::default(),
            proposed: None,
            apply: None,
            focused: false,
        }
    }

    pub fn with_sep(mut self, sep: &str) -> Self {
        self.sep = sep.into();
        self
    }

    fn splitter(&mut self) -> Result<Option<Arc<Splitter>>, String> {
        let k = (self.sep.clone(), self.regex, self.collapse, self.trim);
        if self.sp.as_ref().is_none_or(|s| s.0 != k) {
            let s = Splitter::new(&self.sep, self.regex, self.collapse, self.trim).map(|o| o.map(Arc::new));
            self.sp = Some((k, s));
        }
        self.sp.as_ref().unwrap().1.clone()
    }
}

impl Tool for SplitTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let heads = snap.st.headers();
        self.col = self.col.min(heads.len().saturating_sub(1));
        let title = heads.get(self.col).cloned().unwrap_or_default();
        let sp = self.splitter();
        let key = (self.col, (self.sep.clone(), self.regex, self.collapse, false), cx.tab.generation);
        if let Ok(Some(s)) = &sp {
            if self.ana.stale(&key) {
                let (s, col, st) = (s.clone(), self.col, snap.clone());
                self.ana.run(cx.ctx, key.clone(), move |c| {
                    let all = st.all();
                    let stats = cols::analyse(&st.st, &all, col, &s, c)?;
                    let sample = cols::split_preview(&st.st, &all, col, &s, 2, 6).into_iter().map(|x| x.0).collect();
                    Some((stats, sample))
                });
            }
        }
        self.ana.poll();
        let fresh = matches!(sp, Ok(Some(_))) && self.ana.fresh(&key).is_some();
        if fresh && !self.user_n && self.proposed.as_ref() != Some(&key) {
            self.proposed = Some(key.clone());
            self.n_text = self.ana.val.as_ref().map_or(2, |a| a.0.proposed()).to_string();
        }
        if let Some(r) = self.apply.as_mut().and_then(|j| j.poll()) {
            self.apply = None;
            if let (Some(plan), Some(t)) = (r, cx.tab.table.as_mut()) {
                let (old_n, col, keep, n) = (t.st.ncols, self.col, self.keep, cols::split_n(self.n_text.trim().parse().ok()));
                let msg = cols::apply_split(t, plan);
                form::remap_columns(cx.tab, old_n, |c| Some(split_map(col, keep, n, c)), cx.ctx);
                cx.toast(msg);
            }
            return false;
        }
        let typed: Option<i64> = self.n_text.trim().parse().ok();
        let n = cols::split_n(typed);
        while self.names.len() < n {
            self.names.push((String::new(), true));
        }
        for (i, (v, auto)) in self.names.iter_mut().enumerate() {
            if *auto {
                *v = format!("{title} {}", i + 1);
            }
        }
        let (p, m) = (cx.p, cx.m);
        let (mut go, mut cancel) = (false, false);
        let mut col = self.col;
        let mut edits: Vec<String> = self.names.iter().take(n).map(|x| x.0.clone()).collect();
        let ana = self.ana.val.as_ref().filter(|_| matches!(sp, Ok(Some(_))));
        let open = form::modal(cx.ctx, p, m, "split", 560.0, "Split a column", |ui| {
            form::label(ui, p, "Column", 0.0);
            form::select(ui, p, m, Id::unique("split-col"), &mut col, &heads, f32::INFINITY);
            form::label(ui, p, "Separator — a character, a string, or a regular expression", 12.0);
            let w = ui.available_width();
            ui.allocate_ui_with_layout(vec2(w, 31.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let rw = ui.painter().layout_no_wrap("Regex".into(), form::ui_font(13.0), p.txt_hi).size().x + 24.0;
                let r = form::field(ui, p, Id::unique("split-sep"), &mut self.sep, "e.g.  ;  or  -  or  \\t  for a tab", ui.available_width() - rw - 10.0, matches!(sp, Err(_)));
                if !self.focused {
                    self.focused = true;
                    r.request_focus();
                }
                form::checkbox(ui, p, &mut self.regex, "Regex");
            });
            let labels: Vec<String> = SPLIT_CHIPS.iter().map(|c| c.1.to_string()).collect();
            let tips: Vec<&str> = SPLIT_CHIPS.iter().map(|c| if c.2 { "Regex: any run of spaces" } else { "" }).collect();
            if let Some(k) = form::chips(ui, p, &labels, &tips) {
                self.sep = SPLIT_CHIPS[k].0.to_string();
                self.regex = SPLIT_CHIPS[k].2;
            }
            form::opts(ui, 10.0, |ui| {
                form::checkbox(ui, p, &mut self.collapse, "Consecutive separators count as one");
                form::checkbox(ui, p, &mut self.trim, "Trim the parts");
                form::checkbox(ui, p, &mut self.keep, "Keep the original column");
            });
            ui.add_space(10.0);
            let (stats, note) = match (&sp, ana) {
                (Err(e), _) => (Err(e.clone()), String::new()),
                (Ok(None), _) => (Ok(cols::TYPE_SEPARATOR.to_string()), String::new()),
                (Ok(Some(_)), None) => (Ok("Counting…".into()), String::new()),
                (Ok(Some(_)), Some((s, _))) if s.filled == 0 => (Err("This column is empty.".into()), String::new()),
                (Ok(Some(_)), Some((s, _))) => {
                    let (a, b) = cols::split_notes(s, typed);
                    (Ok(a), b)
                }
            };
            match stats {
                Ok(t) => form::stat(ui, p, &t, p.txt_hi),
                Err(t) => warn(ui, p, &t),
            }
            ui.add_space(10.0);
            ui.allocate_ui_with_layout(vec2(w, 31.0), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(RichText::new("Columns to create").font(form::ui_font(13.0)).color(p.txt_hi));
                if form::field(ui, p, Id::unique("split-n"), &mut self.n_text, "", 70.0, false).changed() {
                    self.user_n = true;
                }
                if !note.is_empty() {
                    warn(ui, p, &note);
                }
            });
            if let (Ok(Some(s)), Some((_, sample))) = (&sp, ana) {
                let mut head = Vec::new();
                if self.keep {
                    head.push(Th::Text(title.clone()));
                }
                head.extend((0..n).map(Th::Edit));
                let rows: Vec<Vec<Td>> = sample.iter().map(|v| {
                    let mut r = Vec::new();
                    if self.keep {
                        r.push(Td::Src(v.clone()));
                    }
                    r.extend(s.parts(v, n).into_iter().map(|x| if x.is_empty() { Td::Empty("empty".into()) } else { Td::Text(x) }));
                    r
                }).collect();
                form::table(ui, p, Id::unique("split-pv"), &head, &rows, &mut edits);
            }
            ui.add_space(16.0);
            form::actions(ui, |ui| {
                let ready = fresh && ana.is_some_and(|a| a.0.ready()) && self.apply.is_none();
                go = form::button(ui, p, Btn::Prim, "Split", ready).clicked();
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
        });
        if col != self.col {
            self.col = col;
            self.user_n = false;
        }
        for (i, e) in edits.into_iter().enumerate() {
            if e != self.names[i].0 {
                self.names[i] = (e, false);
            }
        }
        if go {
            if let Ok(Some(s)) = sp {
                let typed_names: Vec<Option<String>> = self.names.iter().take(n).map(|(v, auto)| (!auto).then(|| v.clone())).collect();
                let names = cols::split_names(&title, &typed_names, n);
                let (col, keep) = (self.col, self.keep);
                self.apply = Some(Job::spawn(cx.ctx, move |c, _| cols::split_plan(&snap.st, &snap.all(), col, &s, n, keep, names, c)));
            }
        }
        (open && !cancel) || self.apply.is_some()
    }
}

/// Column `c`'s index after column `col` is split into `n` (`keep`: it stays before them;
/// else the first part takes its place).
pub fn split_map(col: usize, keep: bool, n: usize, c: usize) -> usize {
    if c <= col { c } else { c + n - !keep as usize }
}

/* ---- Merge ---------------------------------------------------------- */

/// Column `c`'s index after the columns `order` are merged: the new one after the
/// last of them when they stay (`keep`), else in place of the first, the others gone.
pub fn merge_map(order: &[usize], keep: bool, c: usize) -> Option<usize> {
    if keep {
        Some(form::after_insert(order.iter().max().map_or(0, |m| m + 1), 1, c))
    } else {
        let at = order.iter().min().copied().unwrap_or(0);
        let gone: Vec<usize> = order.iter().copied().filter(|&g| g != at).collect();
        form::after_delete(&gone, c)
    }
}

pub struct MergeTool {
    order: Vec<usize>,
    sep: String,
    skip: bool,
    keep: bool,
    name: String,
    user_name: bool,
    apply: Option<Job<Option<Result<MergePlan, &'static str>>>>,
}

impl MergeTool {
    pub fn new(col: Option<usize>) -> MergeTool {
        MergeTool { order: col.into_iter().collect(), sep: " ".into(), skip: true, keep: true, name: String::new(), user_name: false, apply: None }
    }
}

impl Tool for MergeTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let heads = snap.st.headers();
        self.order.retain(|&c| c < heads.len());
        if let Some(r) = self.apply.as_mut().and_then(|j| j.poll()) {
            self.apply = None;
            match (r, cx.tab.table.as_mut()) {
                (Some(Ok(plan)), Some(t)) => {
                    let old_n = t.st.ncols;
                    let (keep, order) = (self.keep, self.order.clone());
                    let msg = cols::apply_merge(t, plan);
                    // The merged column is a new one: the first original's filter, sort and look
                    // do not carry onto it (as the web's).
                    let first = order.iter().min().copied();
                    form::remap_columns(cx.tab, old_n, |c| if !keep && Some(c) == first { None } else { merge_map(&order, keep, c) }, cx.ctx);
                    cx.toast(msg);
                }
                (Some(Err(e)), _) => cx.toast(e),
                _ => {}
            }
            return false;
        }
        if !self.user_name {
            self.name = cols::merge_name(&heads, &self.order);
        }
        let (p, m) = (cx.p, cx.m);
        let (mut go, mut cancel) = (false, false);
        let open = form::modal(cx.ctx, p, m, "merge", 560.0, "Merge columns", |ui| {
            form::label(ui, p, "Columns — tick them in the order they are joined", 0.0);
            let items: Vec<(usize, String)> = heads.iter().cloned().enumerate().collect();
            form::ordered_list(ui, p, Id::unique("merge-cols"), &items, &mut self.order);
            form::label(ui, p, "Separator", 12.0);
            form::field(ui, p, Id::unique("merge-sep"), &mut self.sep, "", f32::INFINITY, false);
            let labels: Vec<String> = MERGE_CHIPS.iter().map(|c| c.1.to_string()).collect();
            if let Some(k) = form::chips(ui, p, &labels, &[]) {
                self.sep = MERGE_CHIPS[k].0.to_string();
            }
            form::opts(ui, 10.0, |ui| {
                form::checkbox(ui, p, &mut self.skip, "Skip empty values");
                form::checkbox(ui, p, &mut self.keep, "Keep the original columns");
            });
            form::label(ui, p, "Name of the new column", 12.0);
            if form::field(ui, p, Id::unique("merge-name"), &mut self.name, "", f32::INFINITY, false).changed() {
                self.user_name = true;
            }
            if self.order.len() < 2 {
                form::table(ui, p, Id::unique("merge-pv"), &[], &[vec![Td::Empty(cols::PICK_TWO.into())]], &mut []);
            } else {
                let ids: Vec<_> = (0..snap.st.len().min(6)).map(|k| snap.st.data_id(k)).collect();
                let mut head: Vec<Th> = self.order.iter().map(|&c| Th::Text(heads[c].clone())).collect();
                head.push(Th::New(if self.name.is_empty() { "merged".into() } else { self.name.clone() }));
                let rows: Vec<Vec<Td>> = cols::merge_preview(&snap.st, &ids, &self.order, &self.sep, self.skip, 6)
                    .into_iter()
                    .map(|(vals, merged)| vals.into_iter().map(Td::Src).chain([Td::Text(merged)]).collect())
                    .collect();
                form::table(ui, p, Id::unique("merge-pv"), &head, &rows, &mut []);
            }
            ui.add_space(16.0);
            form::actions(ui, |ui| {
                go = form::button(ui, p, Btn::Prim, "Merge", self.order.len() >= 2 && self.apply.is_none()).clicked();
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
            let _ = ui.allocate_exact_size(vec2(0.0, 0.0), Sense::hover());
        });
        if go {
            let (order, sep, skip, keep, name) = (self.order.clone(), self.sep.clone(), self.skip, self.keep, self.name.clone());
            self.apply = Some(Job::spawn(cx.ctx, move |c, _| cols::merge_plan(&snap.st, &snap.all(), &order, &sep, skip, keep, &name, c)));
        }
        (open && !cancel) || self.apply.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn a_merge_keeps_the_tabs_column_state_in_place() {
        let ctx = eframe::egui::Context::default();
        // The first pass uploads the font atlas: hand its texture changes over, as a renderer would.
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let m = crate::theme::Metrics::default();
        let mut tab = Tab::memory("t".into(), b"a,b,c,d\n1,2,3,4\n".to_vec(), &ctx, &m);
        tab.col_filters = vec!["".into(), "".into(), "".into(), "4".into()];
        tab.sort = vec![csvfab_engine::view::SortKey { col: 3, desc: true }];
        tab.grid.widths = vec![10.0, 20.0, 30.0, 40.0];
        let st = tab.st().unwrap().clone();
        let plan = cols::merge_plan(&st, &csvfab_engine::edit::all_rows(&st), &[1, 2], "-", true, false, "", &AtomicBool::new(false)).unwrap().unwrap();
        let t = tab.table.as_mut().unwrap();
        assert_eq!(cols::apply_merge(t, plan), "2 columns merged into \"b c\" — not written yet, use Save.");
        form::remap_columns(&mut tab, 4, |c| merge_map(&[1, 2], false, c), &ctx);
        assert_eq!(tab.st().unwrap().headers(), ["a", "b c", "d"]);
        assert_eq!(tab.col_filters, ["", "", "4"]);
        assert_eq!(tab.sort[0].col, 2);
        assert_eq!(tab.grid.widths, [10.0, 20.0, 40.0]);
    }

    #[test]
    fn columns_follow_a_split() {
        // a b c, b split in 3: a b1 b2 b3 c (or a b b1 b2 b3 c when kept).
        assert_eq!((0..3).map(|c| split_map(1, false, 3, c)).collect::<Vec<_>>(), [0, 1, 4]);
        assert_eq!((0..3).map(|c| split_map(1, true, 3, c)).collect::<Vec<_>>(), [0, 1, 5]);
    }

    #[test]
    fn columns_follow_a_merge() {
        // a b c d, d and b merged: kept → a b c d m; dropped → a m c.
        assert_eq!((0..4).map(|c| merge_map(&[3, 1], true, c)).collect::<Vec<_>>(), [Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!((0..4).map(|c| merge_map(&[1, 2], true, c)).collect::<Vec<_>>(), [Some(0), Some(1), Some(2), Some(4)]);
        assert_eq!((0..4).map(|c| merge_map(&[3, 1], false, c)).collect::<Vec<_>>(), [Some(0), Some(1), Some(2), None]);
    }
}
