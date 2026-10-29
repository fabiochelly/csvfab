//! Remove noise words: a list of words or phrases ("SARL", "Monsieur", "et
//! Fils"…) taken out of one column or of every column, in the rows shown —
//! whole words, case and accents ignored by default, the gaps tidied.

use super::edit_kit::{self as form, Btn, Live, Snap, Td, Th};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::tab::Tab;
use csvfab_engine::edit::not_saved;
use csvfab_engine::noise::{self, Noise, Opts, PRESETS};
use eframe::egui::Id;
use std::cell::RefCell;
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Remove noise words…", group: "Transform the file", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(NoiseTool::new(cx.tab)))) }]
}

thread_local! {
    /// The list typed last, for the next opening; kept on disk too (`noise-words` in the
    /// state folder, as the web kept it in localStorage), read once.
    static DRAFT: RefCell<Option<String>> = const { RefCell::new(None) };
}

thread_local! {
    /// The column and options chosen last (0: all columns), for the next opening.
    static LAST: std::cell::Cell<Option<(usize, Opts)>> = const { std::cell::Cell::new(None) };
}

fn draft_path() -> std::path::PathBuf {
    crate::config::dir().join("noise-words")
}

fn draft() -> String {
    DRAFT.with_borrow_mut(|d| d.get_or_insert_with(|| std::fs::read_to_string(draft_path()).unwrap_or_default()).clone())
}

/// Kept when it changed (written on disk then, not every frame).
fn keep_draft(words: &str) {
    DRAFT.with_borrow_mut(|d| {
        if d.as_deref() == Some(words) { return; }
        *d = Some(words.to_string());
        if std::env::var_os("CSVFAB_SHOT").is_none() {
            let _ = std::fs::create_dir_all(crate::config::dir());
            let _ = std::fs::write(draft_path(), words);
        }
    });
}

pub struct NoiseTool {
    words: String,
    /// 0: all columns, k: column k − 1.
    from: usize,
    o: Opts,
    focused: bool,
    scan: Live<(Vec<String>, Option<usize>, (bool, bool, bool, bool), u64, usize), (Arc<Noise>, noise::Scan)>,
}

impl NoiseTool {
    /// One column selected: that one, else every column.
    pub fn new(tab: &Tab) -> NoiseTool {
        let one = tab.grid.range().filter(|(_, c)| c.start() == c.end()).map(|(_, c)| *c.start() + 1);
        // Else the column and the options of last time (this session), as the web's dialog.
        let (from, o) = LAST.get().map_or((0, Opts::default()), |(f, o)| (if f <= tab.st().map_or(0, |s| s.ncols) { f } else { 0 }, o));
        NoiseTool { words: draft(), from: one.unwrap_or(from), o, focused: false, scan: Live::default() }
    }
}

impl NoiseTool {
    pub fn words(mut self, w: &str) -> Self {
        self.words = w.into();
        self
    }
}

impl Tool for NoiseTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let heads = snap.st.headers();
        let words = noise::words(&self.words);
        let col = self.from.checked_sub(1).filter(|&c| c < heads.len());
        let o = self.o;
        let key = (words.clone(), col, (o.caseless, o.accents, o.whole, o.tidy), cx.tab.generation, cx.tab.shown());
        if self.scan.stale(&key) {
            match Noise::new(&words, o) {
                Some(nz) => {
                    let nz = Arc::new(nz);
                    self.scan.run(cx.ctx, key.clone(), move |c| noise::scan(&snap.st, &snap.rows(), col, &nz, c).map(|s| (nz, s)));
                }
                None => self.scan.forget(),
            }
        }
        self.scan.poll();
        let (p, m) = (cx.p, cx.m);
        let shown = cx.tab.shown();
        let filtered = form::filtered(cx.tab);
        let fresh = self.scan.fresh(&key).is_some();
        let (mut go, mut cancel) = (false, false);
        let mut opts = vec!["All columns".to_string()];
        opts.extend(heads.iter().cloned());
        let open = form::modal(cx.ctx, p, m, "noise", 560.0, "Remove noise words", |ui| {
            form::label(ui, p, "Words or phrases to remove — one per line, or separated by commas", 0.0);
            let r = form::text_area(ui, p, Id::unique("nz-words"), &mut self.words, "e.g.  SARL, Monsieur, et Fils", 4);
            if !self.focused {
                self.focused = true;
                r.request_focus();
            }
            let labels: Vec<String> = PRESETS.iter().map(|(l, _)| format!("+ {l}")).collect();
            let tips: Vec<&str> = PRESETS.iter().map(|(_, w)| *w).collect();
            if let Some(k) = form::chips(ui, p, &labels, &tips) {
                let cur = self.words.trim_end().to_string();
                self.words = if cur.is_empty() { PRESETS[k].1.to_string() } else { format!("{cur}\n{}", PRESETS[k].1) };
            }
            form::label(ui, p, "From", 12.0);
            form::select(ui, p, m, Id::unique("nz-col"), &mut self.from, &opts, f32::INFINITY);
            form::opts(ui, 10.0, |ui| {
                form::checkbox(ui, p, &mut self.o.whole, "Whole words only — \"SA\" leaves \"SARLAT\" alone");
                form::checkbox(ui, p, &mut self.o.caseless, "Ignore case");
                form::checkbox(ui, p, &mut self.o.accents, "Ignore accents — \"societe\" removes \"Société\"");
                form::checkbox(ui, p, &mut self.o.tidy, "Tidy the gaps left behind")
                    .on_hover_text("Doubled spaces, a comma or dash left dangling, empty brackets — only where a word was removed");
            });
            ui.add_space(10.0);
            let n = words.len();
            let rows = format!("{} {}rows", form::n(shown), if filtered { "filtered " } else { "" });
            let res = self.scan.val.as_ref().filter(|_| n > 0);
            let stat = match res {
                _ if n == 0 => "Type the words to remove, or add a list below.".to_string(),
                Some((_, s)) if s.cells > 0 => format!(
                    "{} cell{} will change, in {} row{} — {} word{}, in the {rows}.",
                    form::n(s.cells), if s.cells == 1 { "" } else { "s" }, form::n(s.rows), if s.rows == 1 { "" } else { "s" }, form::n(n), if n == 1 { "" } else { "s" }
                ),
                _ => format!("No cell would change — {} word{}, in the {rows}.", form::n(n), if n == 1 { "" } else { "s" }),
            };
            form::stat(ui, p, &stat, p.txt_hi);
            let all = col.is_none();
            let (head, body): (Vec<Th>, Vec<Vec<Td>>) = match res {
                Some((nz, s)) if !s.examples.is_empty() => {
                    let mut head = Vec::new();
                    if all {
                        head.push(Th::Text("column".into()));
                    }
                    head.push(Th::Text(if all { "before".into() } else { heads.get(col.unwrap_or(0)).cloned().unwrap_or_default() }));
                    head.push(Th::New("after".into()));
                    let body = s.examples.iter().map(|(c, old, new)| {
                        let mut r = Vec::new();
                        if all {
                            r.push(Td::Src(heads.get(*c).cloned().unwrap_or_default()));
                        }
                        r.push(Td::Marked(old.clone(), nz.find(old)));
                        r.push(if new.is_empty() { Td::Empty("(empty)".into()) } else { Td::Text(new.clone()) });
                        r
                    }).collect();
                    (head, body)
                }
                _ => (vec![], vec![vec![Td::Empty(if n > 0 { "Nothing to remove with these settings." } else { "The cells that change will show here." }.into())]]),
            };
            form::table(ui, p, Id::unique("nz-pv"), &head, &body, &mut []);
            ui.add_space(16.0);
            form::actions(ui, |ui| {
                go = form::button(ui, p, Btn::Prim, "Remove", fresh && res.is_some_and(|(_, s)| s.cells > 0)).clicked();
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
        });
        keep_draft(&self.words);
        LAST.set(Some((self.from, self.o)));
        if go {
            if let Some((_, s)) = self.scan.val.take() {
                let where_ = col.and_then(|c| heads.get(c).cloned()).unwrap_or_else(|| "every column".into());
                cx.tab.apply_changes(&s.changes, cx.ctx);
                cx.toast(not_saved(&format!("Noise words removed from {} cell{} in {where_}", form::n(s.cells), if s.cells == 1 { "" } else { "s" })));
            }
            return false;
        }
        open && !cancel
    }
}
