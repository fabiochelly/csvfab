//! Convert formats: one column of the rows shown rewritten into one format —
//! dates, numbers or phone numbers — with a guess of what the column holds,
//! a preview, the count of values not recognised, and "Show unrecognised"
//! (the view restricted to their rows).

use super::edit_kit::{self as form, Btn, Live, Snap, Td, Th};
use super::{has_table, Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::edit::convert_values::{self as cv, convert_done, convert_note, Convert, Converter, DateOut, NumOut, PhoneOut};
use csvfab_engine::edit::{self, Change, Scan};
use csvfab_engine::RowId;
use eframe::egui::{vec2, Id, Sense, Ui};

pub fn commands() -> Vec<Command> {
    vec![Command { name: "Convert formats…", group: "Columns", keys: None, enabled: has_table, run: |cx| cx.out.push(Cmd::Tool(Box::new(ConvertTool::new(cx.tab, None)))) }]
}

const DORDER: [&str; 2] = ["day first — 15/06/2024", "month first — 06/15/2024"];
const DOUT: [(&str, DateOut); 5] = [("2024-06-15 (ISO)", DateOut::Iso), ("15/06/2024", DateOut::Dmy), ("06/15/2024", DateOut::Mdy), ("15.06.2024", DateOut::DmyDot), ("15-06-2024", DateOut::DmyDash)];
const NDEC: [&str; 2] = ["comma — 1 234,50", "point — 1,234.50"];
const NOUT: [(&str, NumOut); 4] = [("1234.5", NumOut::Point), ("1234,5", NumOut::Comma), ("1 234,5", NumOut::Fr), ("1,234.5", NumOut::En)];
const NFIX: [&str; 6] = ["as they are", "0", "1", "2", "3", "4"];
const POUT: [(&str, PhoneOut); 3] = [("+33612345678 (E.164)", PhoneOut::E164), ("+33 6 12 34 56 78", PhoneOut::Intl), ("06 12 34 56 78", PhoneOut::Nat)];

pub struct ConvertTool {
    col: usize,
    /// 0 dates, 1 numbers, 2 phone numbers.
    kind: usize,
    dorder: usize,
    dout: usize,
    ndec: usize,
    nout: usize,
    nfix: usize,
    cc: String,
    pout: usize,
    /// The line under "Read as".
    dnote: String,
    /// The column the guess was made for.
    guessed: Option<usize>,
    scan: Live<(usize, Convert, u64, usize), Scan>,
    apply: Option<Job<Option<Vec<Change>>>>,
    unrec: Option<Job<Option<Vec<RowId>>>>,
}

impl ConvertTool {
    pub fn new(tab: &Tab, col: Option<usize>) -> ConvertTool {
        ConvertTool {
            col: col.unwrap_or_else(|| form::cursor_col(tab)),
            kind: 0,
            dorder: 0,
            dout: 0,
            ndec: 0,
            nout: 0,
            nfix: 0,
            cc: "33".into(),
            pout: 0,
            dnote: String::new(),
            guessed: None,
            scan: Live::default(),
            apply: None,
            unrec: None,
        }
    }

    fn convert(&self) -> Convert {
        match self.kind {
            0 => Convert::Date { month_first: self.dorder == 1, out: DOUT[self.dout].1 },
            1 => Convert::Number { dec: if self.ndec == 0 { ',' } else { '.' }, out: NOUT[self.nout].1, decimals: self.nfix.checked_sub(1) },
            _ => Convert::Phone { cc: self.cc.clone(), out: POUT[self.pout].1 },
        }
    }

    /// A new column: what it seems to hold, and how it is written (from its first values shown).
    fn guess(&mut self, tab: &Tab, title: &str) {
        let Some(st) = tab.st() else { return };
        let g = cv::guess(st, &form::first_rows(tab, 50_000), self.col, title, &self.cc);
        self.kind = match g.kind { cv::Kind::Date => 0, cv::Kind::Number => 1, cv::Kind::Phone => 2 };
        self.dorder = g.month_first as usize;
        self.ndec = (g.dec == '.') as usize;
        self.dnote = g.note;
        self.guessed = Some(self.col);
    }
}

/// `.cv-grid`: labels (ui 13 `txt_hi`) in a column as wide as the widest, fields after, 8 × 12 apart.
fn grid_row(ui: &mut Ui, p: &crate::theme::Palette, lw: f32, label: &str, field: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 31.0), eframe::egui::Layout::left_to_right(eframe::egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let g = ui.painter().layout_no_wrap(label.to_string(), form::ui_font(13.0), p.txt_hi);
        let (r, _) = ui.allocate_exact_size(vec2(lw, 31.0), Sense::hover());
        ui.painter().galley(eframe::egui::pos2(r.min.x, (r.center().y - g.size().y / 2.0).round()), g, p.txt_hi);
        field(ui);
    });
}

impl Tool for ConvertTool {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let Some(snap) = Snap::of(cx.tab) else { return false };
        let heads = snap.st.headers();
        self.col = self.col.min(heads.len().saturating_sub(1));
        let title = heads.get(self.col).cloned().unwrap_or_default();
        if self.guessed != Some(self.col) {
            self.guess(cx.tab, &title);
        }
        let key = (self.col, self.convert(), cx.tab.generation, cx.tab.shown());
        if self.scan.stale(&key) {
            let (col, conv) = (self.col, Converter::new(self.convert()));
            let s = snap.clone();
            self.scan.run(cx.ctx, key.clone(), move |c| edit::scan(&s.st, &s.rows(), &[col], &conv, false, c));
        }
        self.scan.poll();
        if let Some(r) = self.apply.as_mut().and_then(|j| j.poll()) {
            self.apply = None;
            if let Some(ch) = r {
                let bad = self.scan.val.as_ref().map_or(0, |s| s.bad);
                cx.tab.apply_changes(&ch, cx.ctx);
                cx.toast(convert_done(ch.len(), &title, bad));
            }
            return false;
        }
        if let Some(r) = self.unrec.as_mut().and_then(|j| j.poll()) {
            self.unrec = None;
            if let Some(ids) = r {
                let n = ids.len();
                cx.tab.restrict_to(format!("\"{title}\" not recognised"), ids, cx.ctx);
                cx.toast(format!("{} rows whose \"{title}\" was not recognised — clear the chip to see every row.", form::n(n)));
            }
            return false;
        }
        let (p, m) = (cx.p, cx.m);
        let (shown, filtered) = (cx.tab.shown(), form::filtered(cx.tab));
        let fresh = self.scan.fresh(&key).is_some();
        let (mut go, mut cancel, mut show_bad) = (false, false, false);
        let mut col = self.col;
        let open = form::modal(cx.ctx, p, m, "convert", 560.0, "Convert formats", |ui| {
            form::label(ui, p, "Column", 0.0);
            form::select(ui, p, m, Id::unique("cv-col"), &mut col, &heads, f32::INFINITY);
            form::label(ui, p, "Its values are", 12.0);
            form::opts(ui, 0.0, |ui| {
                for (k, t) in ["Dates", "Numbers", "Phone numbers"].iter().enumerate() {
                    if form::radio(ui, p, self.kind == k, t) {
                        self.kind = k;
                    }
                }
            });
            ui.add_space(12.0);
            let labels: &[&str] = match self.kind { 0 => &["Read as", "Write as"], 1 => &["Decimal separator", "Write as", "Decimals"], _ => &["Country code", "Write as"] };
            let lw = labels.iter().map(|l| ui.painter().layout_no_wrap(l.to_string(), form::ui_font(13.0), p.txt_hi).size().x).fold(0.0, f32::max).ceil();
            let opts = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            match self.kind {
                0 => {
                    grid_row(ui, p, lw, "Read as", |ui| { form::select(ui, p, m, Id::unique("cv-dorder"), &mut self.dorder, &opts(&DORDER), f32::INFINITY); });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(lw + 12.0);
                        ui.label(eframe::egui::RichText::new(&self.dnote).font(form::ui_font(12.0)).color(p.txt_mut));
                    });
                    ui.add_space(8.0);
                    grid_row(ui, p, lw, "Write as", |ui| { form::select(ui, p, m, Id::unique("cv-dout"), &mut self.dout, &DOUT.map(|x| x.0.to_string()), f32::INFINITY); });
                }
                1 => {
                    grid_row(ui, p, lw, "Decimal separator", |ui| { form::select(ui, p, m, Id::unique("cv-ndec"), &mut self.ndec, &opts(&NDEC), f32::INFINITY); });
                    ui.add_space(8.0);
                    grid_row(ui, p, lw, "Write as", |ui| { form::select(ui, p, m, Id::unique("cv-nout"), &mut self.nout, &NOUT.map(|x| x.0.to_string()), f32::INFINITY); });
                    ui.add_space(8.0);
                    grid_row(ui, p, lw, "Decimals", |ui| { form::select(ui, p, m, Id::unique("cv-nfix"), &mut self.nfix, &opts(&NFIX), f32::INFINITY); });
                }
                _ => {
                    grid_row(ui, p, lw, "Country code", |ui| { form::field(ui, p, Id::unique("cv-cc"), &mut self.cc, "", f32::INFINITY, false).on_hover_text("For numbers starting with 0"); });
                    ui.add_space(8.0);
                    grid_row(ui, p, lw, "Write as", |ui| { form::select(ui, p, m, Id::unique("cv-pout"), &mut self.pout, &POUT.map(|x| x.0.to_string()), f32::INFINITY); });
                }
            }
            ui.add_space(10.0);
            let s = self.scan.val.as_ref();
            form::stat(ui, p, &s.map_or_else(|| "Counting…".into(), |s| convert_note(s, shown, filtered)), p.txt_hi);
            let rows: Vec<Vec<Td>> = match s {
                Some(s) if s.changed > 0 => s.examples.iter().map(|e| vec![Td::Src(e.old.clone()), Td::Text(e.new.clone())]).collect(),
                _ => vec![vec![Td::Empty(cv::NOTHING_TO_CONVERT.into())]],
            };
            let head = if s.is_some_and(|s| s.changed > 0) { vec![Th::Text(title.clone()), Th::New("converted".into())] } else { vec![] };
            form::table(ui, p, Id::unique("cv-pv"), &head, &rows, &mut []);
            ui.add_space(16.0);
            let busy = self.apply.is_some() || self.unrec.is_some();
            form::actions(ui, |ui| {
                go = form::button(ui, p, Btn::Prim, "Convert", fresh && s.is_some_and(|s| s.changed > 0) && !busy).clicked();
                if s.is_some_and(|s| s.bad > 0) {
                    show_bad = form::button(ui, p, Btn::Outline, "Show unrecognised", fresh && !busy).clicked();
                }
                cancel = form::button(ui, p, Btn::Outline, "Cancel", true).clicked();
            });
        });
        self.col = col;
        if go || show_bad {
            let (c, conv) = (self.col, Converter::new(self.convert()));
            self.apply = None;
            if go {
                self.apply = Some(Job::spawn(cx.ctx, move |k, _| edit::changes(&snap.st, &snap.rows(), &[c], &conv, k)));
            } else {
                self.unrec = Some(Job::spawn(cx.ctx, move |k, _| cv::unrecognised(&snap.st, &snap.rows(), c, &conv, k)));
            }
        }
        (open && !cancel) || self.apply.is_some() || self.unrec.is_some()
    }
}
