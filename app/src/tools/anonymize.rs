//! Anonymise (25-anonymize.js): a treatment per column, guessed from its title
//! and values and changeable, with an example (the column's first value and
//! what it becomes). The same value always gives the same replacement, so
//! duplicates stay duplicates and files still join. The cells are rewritten
//! in a job, then applied as one undo step; nothing is written until Save.

use super::form::{self, b, Btn};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use csvfab_engine::anonymize::{self, Anonymized, Mode};
use csvfab_engine::{RowId, State};
use eframe::egui::{self, pos2, vec2, Rect, Sense, Stroke};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Anonymise…", group: "Transform the file", keys: None, enabled: has_table, run: |cx| { if let Some(t) = Anon::new(cx) { cx.out.push(Cmd::Tool(Box::new(t))) } } }]
}

/// Each column's guess and its first value (None: the column is empty).
type Guess = (Vec<Mode>, Vec<Option<String>>);

struct Anon {
    st: State,
    modes: Vec<Mode>,
    first: Vec<Option<String>>,
    guessing: Option<Job<Guess>>,
    shown_only: bool,
    running: Option<(Job<Option<Anonymized>>, usize)>,
}

fn guess(st: &State) -> Guess {
    let modes = anonymize::guess_all(st);
    let first = (0..st.ncols).map(|c| anonymize::example(st, c, Mode::Keep).map(|e| e.0)).collect();
    (modes, first)
}

impl Anon {
    fn new(cx: &mut Cx) -> Option<Anon> {
        let st = cx.tab.st()?.clone();
        let s2 = st.clone();
        let guessing = Some(Job::spawn(cx.ctx, move |_, _| guess(&s2)));
        Some(Anon { modes: vec![Mode::Keep; st.ncols], first: vec![None; st.ncols], st, guessing, shown_only: false, running: None })
    }

    fn rows(&self, cx: &Cx) -> Arc<Vec<RowId>> {
        if self.shown_only { cx.tab.shown_ids() } else { Arc::new((0..self.st.len()).map(|p| self.st.data_id(p)).collect()) }
    }
}

/// `s` cut at 34 characters, as the web's sample.
fn cut(s: &str) -> String {
    if s.chars().count() > 34 { s.chars().take(33).collect::<String>() + "…" } else { s.to_string() }
}

impl Tool for Anon {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if let Some((m, f)) = self.guessing.as_mut().and_then(|j| j.poll()) {
            self.guessing = None;
            self.modes = m;
            self.first = f;
        }
        if let Some((job, k)) = &mut self.running {
            match job.poll() {
                Some(Some(a)) => {
                    let names: Vec<String> = self.modes.iter().enumerate().filter(|(_, m)| **m != Mode::Keep).map(|(c, _)| self.st.headers()[c].clone()).collect();
                    let name = cx.tab.name.clone();
                    if a.changes.is_empty() {
                        cx.toast(format!("{name} | Nothing to anonymise in these columns."));
                    } else {
                        let cells = a.changes.len();
                        cx.tab.apply_changes(&a.changes, cx.ctx);
                        let kept = if a.kept > 0 { format!(" · {} values not recognised and kept", form::n(a.kept)) } else { String::new() };
                        cx.toast(format!("{name} | {} cells anonymised in {}{kept} — not written yet, use Save (or Save as… to keep the original).", form::n(cells), names.join(", ")));
                    }
                    return false;
                }
                Some(None) => return false,
                None => {
                    let done = job.progress.load(Ordering::Relaxed);
                    cx.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    if job.elapsed().as_millis() < 300 { return true; }
                    let a = form::Ask { main: "Anonymising…", sub: &format!("{} %", (done as f32 / (*k).max(1) as f32 * 100.0).min(100.0) as u32), choices: &[], cancel: None, ok: "Stop", danger: false };
                    if form::ask(cx, "an-busy", &a).is_some() { job.cancel(); }
                    cx.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    return true;
                }
            }
        }
        let p = cx.p.clone();
        let heads = self.st.headers();
        let labels: Vec<String> = Mode::ALL.iter().map(|m| m.label().to_string()).collect();
        let total = if self.shown_only { cx.tab.shown() } else { self.st.len() };
        let filtered = cx.tab.filtered();
        let screen = cx.ctx.content_rect();
        let mut go = false;
        let mut cancel = false;
        let mut reguess = false;
        let guessing = self.guessing.is_some();
        let (_, close) = form::modal_tall(cx, "anon", 820.0_f32.min(screen.width() - 40.0) - 42.0, screen.height() * 0.94 - 42.0, "Anonymise", |ui, _| {
            form::label(ui, &p, "A treatment per column, guessed from its title and values — change it where the guess is wrong. The same value always gives the same replacement, so duplicates stay duplicates and files still join.", 0.0);
            // The list takes the room left by the parts under it.
            let list_h = (ui.available_height() - 10.0 - 8.0 - 30.0 - 10.0 - 40.0 - 16.0 - 30.0).max(160.0);
            ui.add_space(5.0);
            let w = ui.available_width();
            let (bx, _) = ui.allocate_exact_size(vec2(w, list_h), Sense::hover());
            ui.painter().rect_stroke(bx, 6.0, Stroke::new(1.0, p.brd), egui::StrokeKind::Inside);
            let name_w = heads.iter().map(|h| ui.painter().layout_no_wrap(h.clone(), form::medium(12.0), p.txt_hi).size().x + 17.0).fold(140.0, f32::max).min(340.0);
            let sel_w = 190.0;
            let row_h = 32.0;
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bx.shrink(1.0)));
            egui::ScrollArea::vertical().id_salt("an-list").max_height(list_h - 2.0).auto_shrink([false, false]).show(&mut child, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for (c, h) in heads.iter().enumerate() {
                    let (r, _) = ui.allocate_exact_size(vec2(w - 2.0, row_h), Sense::hover());
                    let on = self.modes[c] != Mode::Keep;
                    let pt = ui.painter().clone();
                    let y = r.center().y;
                    let rule = |x: f32| pt.vline(x.round() - 0.5, r.y_range(), Stroke::new(1.0, p.brd));
                    let mut job = egui::text::LayoutJob::simple_singleline(h.clone(), form::medium(12.0), if on { p.txt_hi } else { p.txt_dim });
                    job.wrap = egui::text::TextWrapping { max_width: name_w - 16.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                    let g = pt.layout_job(job);
                    pt.galley(pos2(r.min.x + 8.0, (y - g.size().y / 2.0).round()), g, p.txt_hi);
                    rule(r.min.x + name_w);
                    let sr = Rect::from_min_size(pos2(r.min.x + name_w + 8.0, y - 11.5), vec2(sel_w - 16.0, 23.0));
                    let mut k = Mode::ALL.iter().position(|m| *m == self.modes[c]).unwrap_or(0);
                    let mut sub = ui.new_child(egui::UiBuilder::new().max_rect(sr));
                    if form::select(&mut sub, &p, ("an", c), sr.width(), 23.0, 12.0, &mut k, &labels) { self.modes[c] = Mode::ALL[k]; }
                    rule(r.min.x + name_w + sel_w);
                    if on {
                        let x0 = r.min.x + name_w + sel_w + 8.0;
                        let mut job = egui::text::LayoutJob::default();
                        let f = |c| egui::text::TextFormat { font_id: form::ui_font(12.0), color: c, ..Default::default() };
                        let fi = |c| egui::text::TextFormat { font_id: form::ui_font(12.0), color: c, italics: true, ..Default::default() };
                        match &self.first[c] {
                            None if guessing => job.append("…", 0.0, f(p.txt_faint)),
                            None => job.append("empty column", 0.0, fi(p.txt_faint)),
                            Some(v) => {
                                job.append(&cut(v), 0.0, f(p.txt_mut));
                                job.append(" → ", 0.0, f(p.txt));
                                match self.modes[c].apply(v) {
                                    None => job.append("not recognised, kept", 0.0, fi(p.txt_faint)),
                                    Some(nv) if nv.is_empty() => job.append("empty", 0.0, fi(p.txt_faint)),
                                    Some(nv) => job.append(&cut(&nv), 0.0, f(p.txt)),
                                }
                            }
                        }
                        job.wrap = egui::text::TextWrapping { max_width: r.max.x - x0 - 8.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                        let g = pt.layout_job(job);
                        pt.galley(pos2(x0, (y - g.size().y / 2.0).round()), g, p.txt);
                    }
                    pt.hline(r.x_range(), r.max.y.round() - 0.5, Stroke::new(1.0, p.brd));
                }
            });
            ui.add_space(8.0);
            let mut shown = self.shown_only;
            let hit = form::row_buttons(ui, &p, &[Btn::outline("Guess again"), Btn::outline("Keep all").tip("Every column kept as is")], |ui| {
                let en = filtered || shown;
                form::check_enabled(ui, &p, &mut shown, "Only the rows shown", en);
            });
            self.shown_only = shown;
            match hit {
                Some(0) => reguess = true,
                Some(_) => self.modes.iter_mut().for_each(|m| *m = Mode::Keep),
                None => {}
            }
            let n = self.modes.iter().filter(|m| **m != Mode::Keep).count();
            form::stat(ui, &p, &if guessing {
                "Guessing what each column holds…".to_string()
            } else if n > 0 {
                format!("{} column{} over {} rows. Empty cells stay empty; a value the treatment does not recognise is kept.", b(form::n(n)), if n == 1 { "" } else { "s" }, b(form::n(total)))
            } else {
                "No column selected: pick a treatment for the columns that identify people.".to_string()
            });
            let rest = ui.available_height() - 30.0;
            ui.add_space(rest.max(16.0));
            match form::actions(ui, &p, &[Btn::outline("Cancel"), Btn::primary("Anonymise").enabled(n > 0)]) {
                Some(0) => cancel = true,
                Some(_) => go = true,
                None => {}
            }
        });
        if reguess && self.guessing.is_none() {
            let st = self.st.clone();
            self.guessing = Some(Job::spawn(cx.ctx, move |_, _| guess(&st)));
        }
        if go {
            let rows = self.rows(cx);
            let (st, modes, k) = (self.st.clone(), self.modes.clone(), rows.len());
            self.running = Some((Job::spawn(cx.ctx, move |cancel, progress| anonymize::anonymize(&st, &rows, &modes, cancel, Some(progress))), k));
            return true;
        }
        !(cancel || close)
    }
}
