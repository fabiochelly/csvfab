//! Duplicates (31-duplicates.js): rows are duplicates when ANY of several keys
//! match, each key one or more fields that must ALL match once cleaned. The
//! dialog builds the keys, counts the groups in the background as it changes,
//! and then marks them (the groups side by side, as a restricted view) or
//! deletes every row of a group but its first (or last), as one undo step.
//!
//! Cross-checks: a field may also be several columns read alike — a person's
//! name from first / last / full name columns in any order, any phone column
//! (as E.164, the country from the row's), any e-mail column. The dialog
//! detects these columns and proposes ready-made checks ("Same person: name +
//! phone OR name + e-mail"…), each with its count, computed in the background
//! before one is picked. Every group found gets a confidence and its reasons
//! (identical rows, same e-mail, same name + city, but different names…),
//! shown in the preview and on the marked rows; groups can be listed least
//! sure first.
//!
//! Marks live in `tab.marks.dups` (`DupMarks`): the grid draws each group's
//! edge, its first row's top line and the cells that differ within it, and
//! the bar over a hovered marked row (`bar`) deletes it, merges its group
//! into it (`apply`) or tells how sure the group is. After such an edit the
//! groups are counted again in the background (`Refresh`), as the web app did
//! at each applyFilters().

// The grid's half (bar, merge_hint, apply, chip…) is wired by the grid and the status bar.
#![allow(dead_code)]

use super::analysis_kit::{self as kit, Btn};
use super::{Cmd, Command, Cx, Tool};
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::{Restrict, Tab};
use crate::theme::{alpha, Palette};
use csvfab_engine::anonymize::{self, Mode};
use csvfab_engine::dupes::{self as eng, Clean, Dupes, Names, Order, Part, Roles, Rule};
use csvfab_engine::{RowId, State};
use eframe::egui::{self, pos2, vec2, Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The cleanings a field can go through: (key, label).
const FNS: [(&str, &str); 8] = [
    ("loose", "Ignore case and spaces"),
    ("exact", "Exactly as written"),
    ("slug", "Slug: no case, accents, symbols"),
    ("digits", "Digits only"),
    ("phone", "Phone number"),
    ("email", "E-mail"),
    ("sound", "Sounds like"),
    ("fx", "Formula on v…"),
];
/// "Sounds like": how close the spellings must be once the sounds match.
const TOL: [(f64, &str); 3] = [(0.9, "Very close"), (0.8, "Close"), (0.7, "Loose")];
const SOUND: usize = 6;
const FX: usize = 7;

/// A field's cross-check: several columns read alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossKind {
    Name,
    Phone,
    Email,
}

impl CrossKind {
    const ALL: [CrossKind; 3] = [CrossKind::Name, CrossKind::Phone, CrossKind::Email];

    /// As the column select lists it, above the columns.
    fn label(self) -> &'static str {
        match self {
            CrossKind::Name => "Person's name (any order)",
            CrossKind::Phone => "Any phone column",
            CrossKind::Email => "Any e-mail column",
        }
    }
}

/// A cross-check field: its columns by NAME, and how they are read.
#[derive(Clone, Debug, PartialEq)]
pub struct Cross {
    pub kind: CrossKind,
    pub cols: Vec<String>,
    /// Name: also the names that sound alike, or with a first name's initial.
    pub sound: bool,
    pub initials: bool,
    /// E-mail: +tags and gmail dots ignored.
    pub canon: bool,
    /// Phone: the calling code of national numbers, unless the row's country column says otherwise.
    pub cc: u16,
    pub country: Option<String>,
}

const NAME_MODES: [&str; 4] = ["Same words, any order", "Or sounding alike", "Or an initial (J. Dupont)", "Or sounding alike, or an initial"];
const MAIL_MODES: [&str; 2] = ["Ignore case", "Ignore case, +tags, gmail dots"];
/// The countries a phone cross-check reads national numbers in.
const CCS: [(u16, &str); 12] = [(33, "France"), (32, "Belgium"), (41, "Switzerland"), (352, "Luxembourg"), (1, "US, Canada"), (44, "United Kingdom"), (49, "Germany"), (34, "Spain"), (39, "Italy"), (351, "Portugal"), (31, "Netherlands"), (212, "Morocco")];

impl Cross {
    /// The mode select's options and the current one.
    fn modes(&self) -> (Vec<String>, usize) {
        match self.kind {
            CrossKind::Name => (NAME_MODES.iter().map(|s| s.to_string()).collect(), self.sound as usize + 2 * self.initials as usize),
            CrossKind::Email => (MAIL_MODES.iter().map(|s| s.to_string()).collect(), self.canon as usize),
            CrossKind::Phone => {
                let mut o: Vec<(u16, String)> = CCS.iter().map(|&(cc, n)| (cc, format!("Numbers of {n} (+{cc})"))).collect();
                if !o.iter().any(|x| x.0 == self.cc) { o.push((self.cc, format!("Numbers of +{}", self.cc))) }
                let sel = o.iter().position(|x| x.0 == self.cc).unwrap_or(0);
                (o.into_iter().map(|x| x.1).collect(), sel)
            }
        }
    }

    fn set_mode(&mut self, k: usize) {
        match self.kind {
            CrossKind::Name => { self.sound = k & 1 != 0; self.initials = k & 2 != 0; }
            CrossKind::Email => self.canon = k == 1,
            CrossKind::Phone => self.cc = CCS.get(k).map_or(self.cc, |x| x.0),
        }
    }

    /// In words: "name (or sounding alike)", "any phone (+33)".
    fn text(&self) -> String {
        match self.kind {
            CrossKind::Name => format!("name{}", match (self.sound, self.initials) { (false, false) => "", (true, false) => " (or sounding alike)", (false, true) => " (or an initial)", _ => " (or sounding alike, or an initial)" }),
            CrossKind::Phone => format!("any phone (+{})", self.cc),
            CrossKind::Email => format!("any e-mail{}", if self.canon { " (+tags ignored)" } else { "" }),
        }
    }
}

/// A field of the dialog: its column by NAME (marks survive columns being moved), its cleaning;
/// or a cross-check over several columns.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDraft {
    pub col: String,
    pub func: usize,
    pub tol: f64,
    pub expr: String,
    pub cross: Option<Cross>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyDraft {
    pub fields: Vec<FieldDraft>,
}

/// The duplicate groups marked on a tab, and how they were found.
pub struct DupMarks {
    pub keys: Vec<KeyDraft>,
    pub rules: Vec<Rule>,
    /// The name columns the groups were cross-checked against.
    pub names: Vec<String>,
    pub dupes: Arc<Dupes>,
    /// The groups' order in the duplicates view.
    pub order: Order,
    /// Per group, the columns that differ (computed when the group is drawn).
    diff: Mutex<HashMap<u32, Arc<Vec<usize>>>>,
}

pub const VIEW_LABEL: &str = "Duplicates only";

#[cfg(test)]
impl KeyDraft {
    /// One column compared exactly (tests).
    pub fn column(name: &str) -> KeyDraft {
        KeyDraft { fields: vec![FieldDraft { col: name.into(), func: FNS.iter().position(|f| f.0 == "exact").unwrap(), tol: 0.0, expr: String::new(), cross: None }] }
    }
}

#[cfg(test)]
impl DupMarks {
    pub fn new(keys: Vec<KeyDraft>, rules: Vec<Rule>, d: Dupes) -> DupMarks {
        DupMarks { keys, rules, names: Vec::new(), dupes: Arc::new(d), order: Order::File, diff: Default::default() }
    }
}

impl DupMarks {
    #[inline]
    pub fn group(&self, id: RowId) -> Option<u32> {
        self.dupes.group(id)
    }

    /// The columns whose values differ within group g (or that one row fills
    /// and another leaves empty): every cell of such a column is drawn
    /// `danger_subtle` / `diff_txt`, the empty ones included.
    pub fn diff(&self, st: &State, g: u32) -> Arc<Vec<usize>> {
        let mut m = self.diff.lock().unwrap();
        if let Some(d) = m.get(&g) { return d.clone(); }
        if m.len() > 2000 { m.clear(); }
        let d = Arc::new(self.dupes.diff_cols_rules(st, &self.rules, g));
        m.insert(g, d.clone());
        d
    }

    /// The spec in words: `Email OR Last name + First name (slug)`.
    pub fn spec_text(&self) -> String {
        spec_text(&self.keys)
    }

    /// Why group g's rows are duplicates, and how sure: "97 % — same e-mail · same phone".
    pub fn why(&self, g: u32) -> String {
        format!("{} % sure — {}", self.dupes.confidence(g), self.dupes.reason(g))
    }
}

/// What the bar of a marked row (or a differing cell's arrows) asks.
#[derive(Clone, Copy, Debug)]
pub enum DupAct {
    Delete(RowId),
    /// Keep this row, fill its empty cells from the others of its group shown, delete them.
    Merge(RowId),
    /// A differing cell's value copied into the same column of a sister row.
    Copy { from: RowId, to: RowId, col: usize },
}

fn spec_text(keys: &[KeyDraft]) -> String {
    let ks: Vec<String> = keys.iter().map(|k| k.fields.iter().filter(|f| !f.col.is_empty() || f.cross.is_some()).map(|f| {
        if let Some(c) = &f.cross { return c.text(); }
        let how = if f.func == 0 { String::new() } else if f.func == FX { format!(" ({})", f.expr) } else { format!(" ({})", FNS[f.func].0) };
        format!("{}{how}", f.col)
    }).collect::<Vec<_>>().join(" + ")).filter(|s| !s.is_empty()).collect();
    if ks.is_empty() { "the whole row".into() } else { ks.join(" OR ") }
}

/// The keys against the tab's columns, as rules: Err(message) for a formula
/// that does not compile, Ok(None) when a column is gone.
pub(crate) fn compile(headers: &[String], keys: &[KeyDraft]) -> Result<Option<Vec<Rule>>, String> {
    let col = |name: &str| headers.iter().position(|h| h == name);
    let mut out = Vec::new();
    for k in keys {
        let mut parts = Vec::new();
        for f in &k.fields {
            if let Some(c) = &f.cross {
                if c.cols.is_empty() { continue }
                let Some(cols) = c.cols.iter().map(|n| col(n)).collect::<Option<Vec<usize>>>() else { return Ok(None) };
                let country = match &c.country { Some(n) => match col(n) { Some(i) => Some(i), None => return Ok(None) }, None => None };
                parts.push(match c.kind {
                    CrossKind::Name => Part::Name(Names { cols, sound: c.sound.then_some(0.8), initials: c.initials }),
                    CrossKind::Phone => Part::Any { cols, clean: Clean::E164 { cc: c.cc }, country },
                    CrossKind::Email => Part::Any { cols, clean: Clean::Mail { canon: c.canon }, country: None },
                });
                continue;
            }
            if f.col.is_empty() { continue }
            let Some(c) = col(&f.col) else { return Ok(None) };
            let clean = match FNS[f.func].0 {
                "exact" => Clean::Exact,
                "slug" => Clean::Slug,
                "digits" => Clean::Digits,
                "phone" => Clean::Phone,
                "email" => Clean::Email,
                "sound" => Clean::Sound { tol: f.tol },
                "fx" if !f.expr.trim().is_empty() => {
                    let e = csvfab_engine::formula::compile_value(&f.expr).map_err(|e| format!("{}: {}", f.col, e.msg))?;
                    Clean::Custom(Arc::new(move |v: &str, out: &mut String| { if let Ok(s) = e.eval_value(v) { out.push_str(&s); } }))
                }
                _ => Clean::Loose,
            };
            parts.push(Part::Field(eng::Field { col: c, clean }));
        }
        if !parts.is_empty() {
            let label = eng::auto_label(&parts, headers);
            out.push(Rule::new(parts, label));
        }
    }
    Ok(Some(out))
}

/// The dialog's keys of engine rules (a suggestion's).
fn drafts_of(rules: &[Rule], headers: &[String]) -> Vec<KeyDraft> {
    let name = |c: usize| headers.get(c).cloned().unwrap_or_default();
    let names = |cs: &[usize]| cs.iter().map(|&c| name(c)).collect::<Vec<_>>();
    let cross = |kind, cols: &[usize]| Cross { kind, cols: names(cols), sound: false, initials: false, canon: false, cc: 33, country: None };
    rules.iter().map(|r| KeyDraft {
        fields: r.parts.iter().map(|p| {
            let x = match p {
                Part::Name(n) => Cross { sound: n.sound.is_some(), initials: n.initials, ..cross(CrossKind::Name, &n.cols) },
                Part::Any { cols, clean: Clean::E164 { cc }, country } => Cross { cc: *cc, country: country.map(name), ..cross(CrossKind::Phone, cols) },
                Part::Any { cols, clean, .. } => Cross { canon: matches!(clean, Clean::Mail { canon: true }), ..cross(CrossKind::Email, cols) },
                Part::Field(f) => {
                    let (func, tol) = match f.clean {
                        Clean::Exact => (1, 0.8),
                        Clean::Slug => (2, 0.8),
                        Clean::Digits => (3, 0.8),
                        Clean::Phone | Clean::E164 { .. } => (4, 0.8),
                        Clean::Email | Clean::Mail { .. } => (5, 0.8),
                        Clean::Sound { tol } => (SOUND, tol),
                        _ => (0, 0.8),
                    };
                    return FieldDraft { col: name(f.col), func, tol, expr: String::new(), cross: None };
                }
            };
            FieldDraft { col: String::new(), func: 0, tol: 0.8, expr: String::new(), cross: Some(x) }
        }).collect(),
    }).collect()
}

/// A new field on column c: its cleaning from what the column holds.
fn new_field(st: Option<&State>, c: Option<usize>) -> FieldDraft {
    let (col, func) = match (st, c) {
        (Some(st), Some(c)) => {
            let f = match anonymize::guess(st, c) {
                Mode::Email => 5,
                Mode::Phone => 4,
                Mode::Postal => 3,
                Mode::First | Mode::Last | Mode::Full | Mode::Company | Mode::Address => 2,
                _ => 0,
            };
            (st.headers().get(c).cloned().unwrap_or_default(), f)
        }
        _ => (String::new(), 0),
    };
    FieldDraft { col, func, tol: 0.8, expr: String::new(), cross: None }
}

/// A new cross-check field, on the columns found for it.
fn new_cross(kind: CrossKind, roles: Option<&Roles>, headers: &[String]) -> FieldDraft {
    let name = |c: usize| headers.get(c).cloned().unwrap_or_default();
    let (cols, cc, country) = match roles {
        Some(r) => {
            let cols = match kind { CrossKind::Name => r.names(), CrossKind::Phone => r.phones.clone(), CrossKind::Email => r.emails.clone() };
            (cols.into_iter().map(name).collect(), r.cc, if kind == CrossKind::Phone { r.country.map(name) } else { None })
        }
        None => (Vec::new(), 33, None),
    };
    FieldDraft { col: String::new(), func: 0, tol: 0.8, expr: String::new(), cross: Some(Cross { kind, cols, sound: false, initials: false, canon: false, cc, country }) }
}

/// Keys from the columns' titles and values: last name + first name (+ postal code), e-mail, phone.
fn suggest(st: &State) -> Vec<KeyDraft> {
    let kinds = anonymize::guess_all(st);
    let of = |m: Mode| kinds.iter().position(|&k| k == m);
    let mut keys = Vec::new();
    let name: Vec<usize> = match (of(Mode::Last), of(Mode::First), of(Mode::Full)) {
        (Some(l), Some(f), _) => vec![l, f],
        (_, _, Some(full)) => vec![full],
        (Some(l), _, _) => vec![l],
        _ => vec![],
    };
    if !name.is_empty() {
        let cols = name.into_iter().chain(of(Mode::Postal));
        keys.push(KeyDraft { fields: cols.map(|c| new_field(Some(st), Some(c))).collect() });
    }
    for (c, k) in kinds.iter().enumerate() {
        if matches!(k, Mode::Email | Mode::Phone) && !shared(st, c) { keys.push(KeyDraft { fields: vec![new_field(Some(st), Some(c))] }); }
    }
    keys
}

/// Whether one value fills most of a column's first rows (`Created_by`: the
/// same address on every row): such a column would make the whole file one group.
fn shared(st: &State, col: usize) -> bool {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut tmp = Vec::new();
    let mut filled = 0;
    for p in 0..st.len().min(2000) {
        st.with_cell(st.data_id(p), col, &mut tmp, |v| {
            let v = v.trim();
            if !v.is_empty() { filled += 1; *counts.entry(v.to_lowercase()).or_default() += 1; }
        });
    }
    counts.values().max().is_some_and(|&m| m * 4 > filled && m > 2)
}

thread_local! {
    /// The dialog's state between openings, per tab.
    static DRAFTS: RefCell<HashMap<u64, (Vec<KeyDraft>, bool, Order)>> = RefCell::default();
    /// The suggestions of a tab and their counts, while its rows are the same.
    static SMART: RefCell<HashMap<u64, (Fingerprint, Arc<Mutex<Smart>>)>> = RefCell::default();
}

/// Every data row, in file order: duplicates are looked for among all of them, filtered or not.
fn all_ids(st: &State) -> Vec<RowId> {
    (0..st.len()).map(|p| st.data_id(p)).collect()
}

/// What tells a table's rows unchanged (suggestion counts are kept while it holds).
type Fingerprint = (usize, usize, usize, usize);

fn fingerprint(st: &State) -> Fingerprint {
    (st.len(), st.ncols, Arc::as_ptr(&st.cols) as usize, st.order.as_ref().map_or(0, |o| Arc::as_ptr(o) as usize))
}

/// A ready-made cross-check, as the dialog lists it.
#[derive(Clone)]
struct SmartItem {
    title: String,
    note: String,
    review: bool,
    keys: Vec<KeyDraft>,
    /// Rows, groups, groups sure (≥ 90 %), once counted.
    count: Option<(usize, usize, usize)>,
}

/// The columns' roles and the suggestions, filled in the background.
#[derive(Default)]
struct Smart {
    roles: Option<Roles>,
    items: Vec<SmartItem>,
    done: bool,
}

/// Looks at the columns, then counts each suggestion, one after the other.
fn smart_job(ctx: &egui::Context, st: State, shared: Arc<Mutex<Smart>>) -> Job<()> {
    let ctx2 = ctx.clone();
    Job::spawn(ctx, move |cancel, _| {
        let roles = eng::roles(&st);
        let h = st.headers();
        let sugg = eng::suggest(&roles, &h);
        let names = roles.names();
        {
            let mut s = shared.lock().unwrap();
            s.items = sugg.iter().map(|x| SmartItem { title: x.title.clone(), note: x.note.clone(), review: x.review, keys: drafts_of(&x.rules, &h), count: None }).collect();
            s.roles = Some(roles);
        }
        ctx2.request_repaint();
        let ids = all_ids(&st);
        for (k, x) in sugg.iter().enumerate() {
            let Some(d) = eng::find_rules(&st, &ids, &x.rules, &names, cancel, None) else { return };
            shared.lock().unwrap().items[k].count = Some((d.rows(), d.groups(), d.count_scored(90, 101)));
            ctx2.request_repaint();
        }
        shared.lock().unwrap().done = true;
    })
}

/// The count of the dialog's spec, run in the background.
struct Count {
    keys: Vec<KeyDraft>,
    job: Job<Option<Dupes>>,
    units: usize,
}

/// A count done: the keys it is for, the groups, the rules and name columns it ran.
#[derive(Clone)]
struct Found {
    keys: Vec<KeyDraft>,
    dupes: Arc<Dupes>,
    rules: Vec<Rule>,
    names: Vec<String>,
}

/// The preview's lines: (group, row; None: "+n more rows"), for one count and order.
struct Preview {
    of: (usize, Order),
    lines: Vec<(u32, Option<RowId>)>,
}

/// Groups and rows a preview shows at most.
const PREVIEW_GROUPS: usize = 150;
const PREVIEW_ROWS: usize = 4;

struct Dialog {
    keys: Vec<KeyDraft>,
    keep_last: bool,
    order: Order,
    /// When the keys last changed: counted 150 ms later.
    changed: Option<Instant>,
    count: Option<Count>,
    /// The last count and the keys it is for.
    result: Option<Found>,
    error: Option<String>,
    rows: usize,
    smart: Arc<Mutex<Smart>>,
    smart_job: Option<Job<()>>,
    opened: Instant,
    preview: Option<Preview>,
    /// Screenshot hook `dupes-smart:N`: suggestion N taken once listed.
    pending_smart: Option<usize>,
}

impl Dialog {
    fn new(cx: &mut Cx) -> Dialog {
        let tab = &*cx.tab;
        let st = tab.st();
        let headers = st.map(|s| s.headers()).unwrap_or_default();
        let ok = |ks: &[KeyDraft]| ks.iter().all(|k| k.fields.iter().all(|f| match &f.cross {
            Some(c) => c.cols.iter().chain(&c.country).all(|n| headers.contains(n)),
            None => f.col.is_empty() || headers.contains(&f.col),
        }));
        let saved = DRAFTS.with(|d| d.borrow().get(&tab.uid).cloned());
        let keys = match (&tab.marks.dups, &saved) {
            (Some(m), _) if ok(&m.keys) => m.keys.clone(),
            (_, Some((k, _, _))) if ok(k) => k.clone(),
            _ => vec![KeyDraft { fields: vec![new_field(None, None)] }],
        };
        let (keep_last, mut order) = saved.map_or((false, Order::LeastSure), |x| (x.1, x.2));
        if let Some(m) = &tab.marks.dups { order = m.order; }
        let keys = match (crate::shot::take("dupes-suggest"), st) { (Some(_), Some(st)) => suggest(st), _ => keys };
        // The suggestions: kept while the rows are the same, else looked for anew.
        let (smart, smart_job) = match st {
            Some(st) => {
                let fp = fingerprint(st);
                let kept = SMART.with(|m| m.borrow().get(&tab.uid).filter(|x| x.0 == fp).map(|x| x.1.clone()));
                match kept {
                    Some(s) => (s, None),
                    None => {
                        let s = Arc::new(Mutex::new(Smart::default()));
                        SMART.with(|m| m.borrow_mut().insert(tab.uid, (fp, s.clone())));
                        let job = smart_job(cx.ctx, st.clone(), s.clone());
                        (s, Some(job))
                    }
                }
            }
            None => (Arc::new(Mutex::new(Smart::default())), None),
        };
        Dialog {
            keys, keep_last, order, changed: Some(Instant::now() - Duration::from_secs(1)), count: None, result: None, error: None,
            rows: st.map_or(0, |s| s.len()), smart, smart_job, opened: Instant::now(), preview: None,
            pending_smart: crate::shot::take("dupes-smart").and_then(|s| s.parse().ok()),
        }
    }

    /// The name columns groups are cross-checked against, once the columns are known.
    fn name_cols(&self, headers: &[String]) -> Option<Vec<String>> {
        let s = self.smart.lock().unwrap();
        s.roles.as_ref().map(|r| r.names().into_iter().filter_map(|c| headers.get(c).cloned()).collect())
    }

    /// Starts counting the current keys (the previous count, if any, is dropped: cancelled).
    fn recount(&mut self, cx: &Cx) {
        let Some(st) = cx.tab.st() else { return };
        let headers = st.headers();
        // The name columns first (a moment): the groups are cross-checked against them.
        let names = match self.name_cols(&headers) {
            Some(n) => n,
            None if self.opened.elapsed() < Duration::from_secs(3) => { cx.ctx.request_repaint_after(Duration::from_millis(30)); return; }
            None => Vec::new(),
        };
        self.changed = None;
        match compile(&headers, &self.keys) {
            Err(e) => { self.error = Some(e); self.count = None; }
            Ok(None) => { self.error = Some("A column of the keys is gone.".into()); self.count = None; }
            Ok(Some(rules)) => {
                self.error = None;
                if self.result.as_ref().is_some_and(|r| r.keys == self.keys) { self.count = None; return; }
                let st = st.clone();
                let units = st.len() * eng::passes(&rules);
                let cols: Vec<usize> = names.iter().filter_map(|n| headers.iter().position(|h| h == n)).collect();
                let job = Job::spawn(cx.ctx, move |cancel, progress| eng::find_rules(&st, &all_ids(&st), &rules, &cols, cancel, Some(progress)));
                self.count = Some(Count { keys: self.keys.clone(), job, units });
            }
        }
    }

    /// The result for the keys as they are now, if counted.
    fn current(&self) -> Option<&Found> {
        self.result.as_ref().filter(|r| r.keys == self.keys && self.error.is_none())
    }

    fn key_box(&mut self, ui: &mut Ui, cx: &Cx, ki: usize, edit: &mut Option<Edit>) {
        let p = cx.p;
        let headers = cx.tab.st().map(|s| s.headers()).unwrap_or_default();
        let nkeys = self.keys.len();
        let error = self.error.clone();
        let key = &mut self.keys[ki];
        kit::boxed(ui, p, |ui| {
            // Head: "Key 1  every field must match  ×".
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(egui::RichText::new(format!("Key {}", ki + 1)).font(crate::grid::semibold(12.5)).color(p.txt_hi));
                ui.label(egui::RichText::new(if key.fields.len() > 1 { "every field must match" } else { "this field must match" }).font(kit::ui(12.5)).color(p.txt_dim));
                if nkeys > 1 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if kit::x_button(ui, p).on_hover_text("Remove this key").clicked() { *edit = Some(Edit::DropKey(ki)); }
                    });
                }
            });
            ui.add_space(6.0);
            // The column select: the cross-checks first, then the columns.
            let mut cols = vec!["Column…".to_string()];
            cols.extend(CrossKind::ALL.iter().map(|k| k.label().to_string()));
            cols.extend(headers.iter().cloned());
            let fns: Vec<String> = FNS.iter().map(|f| f.1.to_string()).collect();
            let tols: Vec<String> = TOL.iter().map(|t| t.1.to_string()).collect();
            let nx = CrossKind::ALL.len();
            for (fi, f) in key.fields.iter_mut().enumerate() {
                let id = ui.scope_id().with(("dk", ki, fi));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let w = ui.available_width() - 24.0 - 6.0;
                    let extra = f.cross.is_none() && (f.func == SOUND || f.func == FX);
                    // Flex 1.2 : 1 (: 1 for the tolerance or the formula).
                    let unit = (w - if extra { 12.0 } else { 6.0 }) / if extra { 3.2 } else { 2.2 };
                    let mut sel = match &f.cross {
                        Some(c) => 1 + CrossKind::ALL.iter().position(|k| *k == c.kind).unwrap_or(0),
                        None => headers.iter().position(|h| *h == f.col).map_or(0, |c| c + 1 + nx),
                    };
                    if kit::select(ui, p, id.with("col"), vec2(unit * 1.2, 28.0), &mut sel, &cols) {
                        *edit = Some(if (1..=nx).contains(&sel) { Edit::Cross(ki, fi, CrossKind::ALL[sel - 1]) } else { Edit::Col(ki, fi, sel.checked_sub(1 + nx)) });
                    }
                    if let Some(c) = &mut f.cross {
                        let (modes, mut m) = c.modes();
                        if kit::select(ui, p, id.with("mode"), vec2(unit, 28.0), &mut m, &modes) { c.set_mode(m); *edit = Some(Edit::Touch); }
                    } else {
                        let mut func = f.func;
                        if kit::select(ui, p, id.with("fn"), vec2(unit, 28.0), &mut func, &fns) { f.func = func; *edit = Some(Edit::Touch); }
                        if f.func == SOUND {
                            let mut t = TOL.iter().position(|x| x.0 == f.tol).unwrap_or(1);
                            if kit::select(ui, p, id.with("tol"), vec2(unit, 28.0), &mut t, &tols) { f.tol = TOL[t].0; *edit = Some(Edit::Touch); }
                        } else if f.func == FX {
                            let bad = error.as_deref().is_some_and(|e| e.starts_with(&format!("{}:", f.col)));
                            let r = kit::text_field(ui, p, id.with("expr"), vec2(unit, 28.0), &mut f.expr, "firstNumber(v)", kit::mono(12.5), bad)
                                .on_hover_text("v is the cell's text, e.g. left(slug(v), 5)");
                            if r.changed() { *edit = Some(Edit::Touch); }
                        }
                    }
                    if kit::x_button(ui, p).on_hover_text("Remove this field").clicked() { *edit = Some(Edit::DropField(ki, fi)); }
                });
                if let Some(c) = &mut f.cross {
                    ui.add_space(5.0);
                    cross_columns(ui, p, id, c, &headers, edit);
                }
                ui.add_space(5.0);
            }
            if kit::link(ui, p, "+ Field", 12.5).clicked() { *edit = Some(Edit::AddField(ki)); }
        });
    }

    /// The ready-made cross-checks: one line each, its count on the right; a click takes it.
    fn smart_list(&mut self, ui: &mut Ui, p: &Palette, edit: &mut Option<Edit>) {
        let s = self.smart.lock().unwrap();
        if s.roles.is_none() {
            kit::progress(ui, p, None, "Looking at the columns…");
            return;
        }
        if s.items.is_empty() {
            kit::note_colored(ui, p, "No name, e-mail or phone column recognised: build the keys below.", p.txt_dim, p.txt_hi, 12.0);
            return;
        }
        kit::label(ui, p, "Suggested cross-checks");
        let w = ui.available_width();
        for (k, it) in s.items.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
            let on = it.keys == self.keys;
            let pt = ui.painter();
            if on {
                pt.rect_filled(r, 5.0, alpha(p.prim, 0.14));
                pt.rect_filled(Rect::from_min_size(r.min + vec2(0.0, 5.0), vec2(2.0, r.height() - 10.0)), 1.0, p.prim);
            } else if resp.hovered() {
                pt.rect_filled(r, 5.0, alpha(p.hi, 0.06));
            }
            let (count, ccolor) = match it.count {
                None => ("counting…".to_string(), p.txt_dim),
                Some((_, 0, _)) => ("no duplicate".to_string(), p.txt_dim),
                Some((rows, groups, sure)) => (format!("{} rows · {} groups{}", kit::n(rows), kit::n(groups), if it.review || sure == groups || sure == 0 { String::new() } else { format!(" · {} sure", kit::n(sure)) }), p.txt_mut),
            };
            let cg = pt.layout_no_wrap(count, kit::ui(12.0), ccolor);
            let tw = (w - 20.0 - cg.size().x - 16.0).max(40.0);
            let mut job = egui::text::LayoutJob::simple_singleline(it.title.clone(), kit::medium(12.5), if it.review { p.txt } else { p.txt_hi });
            job.wrap = egui::text::TextWrapping { max_width: tw, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let tg = pt.layout_job(job);
            pt.galley(pos2(r.min.x + 10.0, kit::snap(ui, r.center().y - tg.size().y / 2.0)), tg, p.txt_hi);
            pt.galley(pos2(r.max.x - 10.0 - cg.size().x, kit::snap(ui, r.center().y - cg.size().y / 2.0)), cg, ccolor);
            if resp.on_hover_text(&it.note).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { *edit = Some(Edit::Smart(k)); }
        }
    }

    /// The groups found, least sure first (or as chosen): how sure, why, the rows.
    fn preview(&mut self, ui: &mut Ui, cx: &Cx) {
        let p = cx.p;
        let Some(f) = self.current().cloned() else { return };
        let Some(st) = cx.tab.st() else { return };
        let d = &f.dupes;
        if d.groups() == 0 { return }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(egui::RichText::new("Groups").font(kit::ui(12.0)).color(p.txt_mut));
            let (sure, likely, review) = (d.count_scored(90, 101), d.count_scored(70, 90), d.count_scored(0, 70));
            let mut job = egui::text::LayoutJob::default();
            for (k, (n, what, c)) in [(sure, "sure", p.ok_txt), (likely, "likely", p.accent_2), (review, "to review", p.warn)].into_iter().enumerate() {
                if n == 0 { continue }
                if k > 0 && !job.text.is_empty() { job.append(" · ", 0.0, egui::TextFormat::simple(kit::ui(12.0), p.txt_dim)); }
                job.append(&format!("{} {what}", kit::n(n)), 0.0, egui::TextFormat::simple(kit::ui(12.0), c));
            }
            ui.label(job);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let opts: Vec<String> = ["Least sure first", "Surest first", "In file order"].iter().map(|s| s.to_string()).collect();
                let mut k = match self.order { Order::LeastSure => 0, Order::SurestFirst => 1, Order::File => 2 };
                if kit::select(ui, p, ui.scope_id().with("dk-order"), vec2(170.0, 24.0), &mut k, &opts) {
                    self.order = [Order::LeastSure, Order::SurestFirst, Order::File][k];
                }
            });
        });
        ui.add_space(6.0);
        let of = (Arc::as_ptr(d) as usize, self.order);
        if self.preview.as_ref().is_none_or(|x| x.of != of) {
            let mut lines = Vec::new();
            for g in d.order(self.order).into_iter().take(PREVIEW_GROUPS) {
                let m = d.members(g);
                lines.extend(m.iter().take(PREVIEW_ROWS).map(|&id| (g, Some(id))));
                if m.len() > PREVIEW_ROWS { lines.push((g, None)) }
            }
            self.preview = Some(Preview { of, lines });
        }
        let lines = &self.preview.as_ref().unwrap().lines;
        // The columns the keys compare (the first ones for the whole row).
        let headers = st.headers();
        let mut show: Vec<usize> = Vec::new();
        for r in &f.rules {
            for part in &r.parts {
                let cs: Vec<usize> = match part { Part::Field(x) => vec![x.col], Part::Any { cols, .. } => cols.clone(), Part::Name(n) => n.cols.clone() };
                for c in cs { if !show.contains(&c) { show.push(c) } }
            }
        }
        if show.is_empty() { show = (0..st.ncols.min(4)).collect() }
        show.truncate(5);
        let w = ui.available_width();
        let rest = ((w - 52.0 - 250.0 - 58.0) / show.len().max(1) as f32).max(110.0);
        let mut cols = vec![("Sure".to_string(), 52.0, true), ("Why".to_string(), 250.0, false), ("Row".to_string(), 58.0, true)];
        cols.extend(show.iter().map(|&c| (headers.get(c).cloned().unwrap_or_default(), rest, false)));
        let mut tmp = Vec::new();
        let mut cell = |r: usize, c: usize| -> (String, Color32) {
            let (g, id) = lines[r];
            let first = r == 0 || lines[r - 1].0 != g;
            match (c, id) {
                (0, _) if first => { let s = d.confidence(g); (format!("{s} %"), tier_color(p, s)) }
                (1, _) if first => (d.reason(g), p.txt),
                (1, None) => (format!("+{} more rows", kit::n(d.members(g).len() - PREVIEW_ROWS)), p.txt_dim),
                (2, Some(id)) => (kit::n(file_number(st, id)), p.txt_dim),
                (c, Some(id)) if c >= 3 => (st.with_cell(id, show[c - 3], &mut tmp, |v| v.to_string()), if first { p.txt_hi } else { p.txt }),
                _ => (String::new(), p.txt),
            }
        };
        kit::Table { id: ui.scope_id().with("dk-preview"), cols, rows: lines.len(), max_h: 230.0, row_h: 24.0, cell: &mut cell }.show(ui, p);
    }
}

/// A confidence's colour: sure (90 % and more), likely (70), to review.
fn tier_color(p: &Palette, s: u8) -> Color32 {
    if s >= 90 { p.ok_txt } else if s >= 70 { p.accent_2 } else { p.warn }
}

/// A cross-check's columns as chips (× takes one out), a select to add one,
/// and for phones the country column.
fn cross_columns(ui: &mut Ui, p: &Palette, id: egui::Id, c: &mut Cross, headers: &[String], edit: &mut Option<Edit>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        ui.add_space(4.0);
        let mut drop = None;
        for (k, name) in c.cols.iter().enumerate() {
            if col_chip(ui, p, name) { drop = Some(k) }
        }
        if let Some(k) = drop { c.cols.remove(k); *edit = Some(Edit::Touch); }
        if c.cols.is_empty() {
            ui.label(egui::RichText::new("no column yet").font(kit::ui(12.0)).color(p.warn));
        }
        let mut add = vec!["+ Column…".to_string()];
        let left: Vec<&String> = headers.iter().filter(|h| !c.cols.contains(h)).collect();
        add.extend(left.iter().map(|h| h.to_string()));
        let mut k = 0;
        if kit::select(ui, p, id.with("add"), vec2(130.0, 24.0), &mut k, &add) && k > 0 {
            c.cols.push(left[k - 1].clone());
            *edit = Some(Edit::Touch);
        }
        if c.kind == CrossKind::Phone {
            let mut opts = vec!["Country column: none".to_string()];
            opts.extend(headers.iter().map(|h| format!("Country: {h}")));
            let mut k = c.country.as_ref().and_then(|n| headers.iter().position(|h| h == n)).map_or(0, |i| i + 1);
            if kit::select(ui, p, id.with("country"), vec2(170.0, 24.0), &mut k, &opts) {
                c.country = k.checked_sub(1).map(|i| headers[i].clone());
                *edit = Some(Edit::Touch);
            }
        }
    });
}

/// A column chip (ui 12 `txt_hi` on `alpha(gray, .12)`, radius 11) and its ×: true when the × is clicked.
fn col_chip(ui: &mut Ui, p: &Palette, text: &str) -> bool {
    let g = ui.painter().layout_no_wrap(text.to_string(), kit::ui(12.0), p.txt_hi);
    let (r, _) = ui.allocate_exact_size(vec2(10.0 + g.size().x + 4.0 + 18.0, 24.0), Sense::hover());
    let pt = ui.painter();
    pt.rect(r, 12.0, alpha(p.gray, 0.12), Stroke::new(1.0, p.brd), StrokeKind::Inside);
    pt.galley(pos2(r.min.x + 10.0, kit::snap(ui, r.center().y - g.size().y / 2.0)), g, p.txt_hi);
    let xr = Rect::from_center_size(pos2(r.max.x - 12.0, r.center().y), vec2(18.0, 18.0));
    let resp = ui.interact(xr, ui.scope_id().with(("chip-x", text)), Sense::click()).on_hover_text("Leave this column out");
    let hot = resp.hovered();
    if hot { pt.rect_filled(xr, 9.0, alpha(p.danger, 0.2)); }
    icons::cross(pt, Rect::from_center_size(xr.center(), vec2(8.0, 8.0)), 1.4, if hot { p.danger_txt } else { p.txt_dim });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

enum Edit {
    Touch,
    AddField(usize),
    DropField(usize, usize),
    DropKey(usize),
    Col(usize, usize, Option<usize>),
    Cross(usize, usize, CrossKind),
    AddKey,
    Suggest,
    Smart(usize),
}

impl Tool for Dialog {
    fn show(&mut self, cx: &mut Cx) -> bool {
        if cx.tab.table.is_none() { return false; }
        if let Some(j) = &mut self.smart_job { if j.poll().is_some() { self.smart_job = None; } }
        // A count done: kept for the keys it was made for.
        if let Some(c) = &mut self.count {
            if let Some(r) = c.job.poll() {
                let c = self.count.take().unwrap();
                if let Some(d) = r {
                    let headers = cx.tab.st().map(|s| s.headers()).unwrap_or_default();
                    let rules = compile(&headers, &c.keys).ok().flatten().unwrap_or_default();
                    let names = self.name_cols(&headers).unwrap_or_default();
                    self.result = Some(Found { keys: c.keys, dupes: Arc::new(d), rules, names });
                }
            }
        }
        if self.changed.is_some_and(|t| t.elapsed() >= Duration::from_millis(150)) {
            self.recount(cx);
        } else if self.changed.is_some() {
            cx.ctx.request_repaint_after(Duration::from_millis(150));
        }
        // Screenshot hooks: `dupes-smart:N` takes suggestion N once listed, `dupes-order:file|sure|least`.
        if let Some(k) = self.pending_smart.filter(|&k| k < self.smart.lock().unwrap().items.len()) {
            self.pending_smart = None;
            self.edit(Edit::Smart(k), cx);
        }
        if let Some(o) = crate::shot::take("dupes-order") {
            self.order = match o.as_str() { "file" => Order::File, "sure" => Order::SurestFirst, _ => Order::LeastSure };
        }
        let p = cx.p;
        let mut edit = None;
        let mut go: Option<bool> = None; // Some(true): delete, Some(false): mark
        let mut cancel = false;
        let (_, open) = kit::modal(cx, "dedupe", 760.0, |ui, cx| {
            kit::title(ui, p, "Duplicates");
            kit::note_colored(ui, p, "Rows are duplicates when **any** key matches; within a key, **every** field must match once cleaned. A key whose fields are not all filled is ignored for that row. Groups chain: rows sharing an e-mail with one and a phone with another are one group. No field: the whole row. A **cross-check** reads several columns alike (a name in any order, any phone or e-mail column); each group says how sure it is.", p.txt_mut, p.txt_hi, 12.0);
            ui.add_space(10.0);
            self.smart_list(ui, p, &mut edit);
            ui.add_space(10.0);
            let maxh = (ui.ctx().content_rect().height() * 0.3).min(300.0);
            egui::ScrollArea::vertical().id_salt("dk-keys").max_height(maxh).auto_shrink([false, true]).show(ui, |ui| {
                for ki in 0..self.keys.len() {
                    if ki > 0 {
                        ui.add_space(4.0);
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 15.0), Sense::hover());
                        ui.painter().text(r.center(), Align2::CENTER_CENTER, "OR", crate::grid::semibold(11.0), p.accent_2);
                        ui.add_space(4.0);
                    }
                    self.key_box(ui, cx, ki, &mut edit);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if kit::button(ui, p, "+ Key (or)", Btn::Outline, true).clicked() { edit = Some(Edit::AddKey); }
                if kit::button(ui, p, "Suggest keys", Btn::Outline, true).clicked() { edit = Some(Edit::Suggest); }
                ui.add_space(12.0);
                ui.label(egui::RichText::new("Delete keeps, in each group:").font(kit::ui(12.0)).color(p.txt_mut));
                if kit::radio(ui, p, !self.keep_last, "Its first row").clicked() { self.keep_last = false; }
                if kit::radio(ui, p, self.keep_last, "Its last row").clicked() { self.keep_last = true; }
            });
            ui.add_space(12.0);
            let total = kit::n(self.rows);
            let mut ready = false;
            if let Some(e) = &self.error {
                kit::note_colored(ui, p, e, p.warn, p.warn, 12.0);
            } else if let Some(f) = self.current() {
                let d = &f.dupes;
                let (dup, groups) = (d.removable(), d.groups());
                let text = if dup > 0 {
                    let per = if f.rules.len() > 1 {
                        format!(" — {}", d.hits.iter().enumerate().map(|(i, h)| format!("key {}: {} rows", i + 1, kit::n(*h))).collect::<Vec<_>>().join(" · "))
                    } else { String::new() };
                    format!("**{}** rows in **{}** groups{per}. Delete keeps one row per group: **{}** deleted (all {total} rows are checked, filtered or not).", kit::n(d.rows()), kit::n(groups), kit::n(dup))
                } else {
                    format!("No duplicate among the {total} rows{}.", if f.rules.is_empty() { " (compared on the whole row)" } else { "" })
                };
                kit::note(ui, p, &text);
                ready = dup > 0;
            } else if let Some(c) = &self.count {
                let done = c.job.progress.load(Ordering::Relaxed) as f32 / c.units.max(1) as f32;
                kit::progress(ui, p, Some(done), &format!("Looking for duplicates among the {total} rows…"));
            } else {
                kit::progress(ui, p, None, "Looking for duplicates…");
            }
            self.preview(ui, cx);
            kit::actions(ui, 16.0, |ui| {
                if kit::button(ui, p, "Delete duplicates", Btn::Primary, ready).clicked() { go = Some(true); }
                if kit::button(ui, p, "Mark them", Btn::Outline, ready).on_hover_text("Group the duplicates, differences in red, how sure on each; nothing is deleted").clicked() { go = Some(false); }
                if kit::button(ui, p, "Cancel", Btn::Outline, true).clicked() { cancel = true; }
            });
        });
        if let Some(e) = edit { self.edit(e, cx); }
        DRAFTS.with(|d| d.borrow_mut().insert(cx.tab.uid, (self.keys.clone(), self.keep_last, self.order)));
        if go.is_none() && self.current().is_some_and(|r| r.dupes.groups() > 0) && crate::shot::take("dupes-mark").is_some() { go = Some(false); }
        if let Some(delete) = go {
            let Some(f) = self.current().cloned() else { return true };
            if delete {
                remove(cx, &f.dupes, self.keep_last);
            } else {
                mark(cx, f, self.order);
            }
            return false;
        }
        open && !cancel
    }
}

impl Dialog {
    fn edit(&mut self, e: Edit, cx: &mut Cx) {
        let st = cx.tab.st();
        let headers = st.map(|s| s.headers()).unwrap_or_default();
        match e {
            Edit::Touch => {}
            Edit::AddField(k) => self.keys[k].fields.push(new_field(None, None)),
            Edit::DropField(k, f) => {
                self.keys[k].fields.remove(f);
                if self.keys[k].fields.is_empty() && self.keys.len() > 1 { self.keys.remove(k); }
            }
            Edit::DropKey(k) => { self.keys.remove(k); }
            Edit::Col(k, f, c) => {
                let old = self.keys[k].fields[f].clone();
                let mut n = new_field(st, c);
                if old.func == FX && old.cross.is_none() { n.func = FX; n.expr = old.expr; }
                self.keys[k].fields[f] = n;
            }
            Edit::Cross(k, f, kind) => {
                let roles = self.smart.lock().unwrap().roles.clone();
                self.keys[k].fields[f] = new_cross(kind, roles.as_ref(), &headers);
            }
            Edit::AddKey => self.keys.push(KeyDraft { fields: vec![new_field(None, None)] }),
            Edit::Suggest => {
                let keys = st.map(suggest).unwrap_or_default();
                if keys.is_empty() { cx.toast("No name, e-mail or phone column recognised: build the keys by hand."); return; }
                self.keys = keys;
            }
            Edit::Smart(i) => {
                let Some(keys) = self.smart.lock().unwrap().items.get(i).map(|x| x.keys.clone()) else { return };
                self.keys = keys;
            }
        }
        self.changed = Some(Instant::now());
    }
}

/// Deletes every row of a group but its first (or last): one undo step.
fn remove(cx: &mut Cx, d: &Dupes, keep_last: bool) {
    let del = d.to_delete(keep_last);
    let n = del.len();
    if n == 0 { return; }
    let Some(t) = &mut cx.tab.table else { return };
    t.delete_rows(&del);
    drop_from_view(cx.tab, &del);
    cx.tab.marks_mut_dups(None);
    cx.tab.after_edit(true, cx.ctx);
    cx.toast(format!("{} duplicate rows deleted — not written yet, use Save.", kit::n(n)));
}

/// Rows deleted leave the rows a tool restricted the view to.
fn drop_from_view(tab: &mut Tab, del: &std::collections::HashSet<RowId>) {
    if let Some(r) = &mut tab.restrict {
        Arc::make_mut(&mut r.ids).retain(|id| !del.contains(id));
    }
}

/// The groups marked and shown side by side, in `order`.
fn mark(cx: &mut Cx, f: Found, order: Order) {
    let d = f.dupes;
    if d.groups() == 0 {
        cx.tab.marks_mut_dups(None);
        cx.toast("No duplicate rows.");
        return;
    }
    let (rows, groups, review) = (d.rows(), d.groups(), d.count_scored(0, 70));
    let ids = d.side_by_side(order);
    cx.tab.marks_mut_dups(Some(Arc::new(DupMarks { keys: f.keys, rules: f.rules, names: f.names, dupes: d, order, diff: Default::default() })));
    cx.tab.restrict_to(VIEW_LABEL, ids, cx.ctx);
    let first = match order { Order::LeastSure if review > 0 => format!(", the {} to review first", kit::n(review)), Order::LeastSure => ", least sure first".into(), Order::SurestFirst => ", surest first".into(), Order::File => String::new() };
    cx.toast(format!("{} rows in {} duplicate groups, side by side{first} — hover a row to see how sure, delete it or merge it with the next; differing values in red.", kit::n(rows), kit::n(groups)));
}

/// Whether the duplicates view is on.
pub fn in_view(tab: &Tab) -> bool {
    tab.marks.dups.is_some() && tab.restrict.as_ref().is_some_and(|r| r.label == VIEW_LABEL)
}

/// The status bar's chip (`12 duplicates · 5 groups ×`): its text, whether
/// the view is on (fill `alpha(violet, .3)`, text `txt_strong`; off:
/// `violet_txt` on `alpha(violet, .08)`, border `alpha(violet, .45)`), its tooltip.
pub fn chip(tab: &Tab) -> Option<(String, bool, String)> {
    let m = tab.marks.dups.as_ref()?;
    let on = in_view(tab);
    let g = m.dupes.groups();
    let d = &m.dupes;
    let (sure, likely, review) = (d.count_scored(90, 101), d.count_scored(70, 90), d.count_scored(0, 70));
    let tiers: Vec<String> = [(sure, "sure"), (likely, "likely"), (review, "to review")].iter().filter(|x| x.0 > 0).map(|(n, w)| format!("{} {w}", kit::n(*n))).collect();
    Some((
        format!("{} duplicates · {} group{}", kit::n(m.dupes.rows()), kit::n(g), if g == 1 { "" } else { "s" }),
        on,
        format!("On {} — {}{}", m.spec_text(), tiers.join(" · "), if on { " · click: show all" } else { " · click: show only these" }),
    ))
}

/// The chip clicked: the duplicates view on or off.
pub fn toggle_view(tab: &mut Tab, ctx: &egui::Context) {
    let Some(m) = tab.marks.dups.clone() else { return };
    if in_view(tab) { tab.restrict = None; tab.refresh(ctx); } else { tab.restrict_to(VIEW_LABEL, m.dupes.side_by_side(m.order), ctx); }
}

/// The duplicates view's groups in another order (the view shown again if on).
pub fn reorder(tab: &mut Tab, ctx: &egui::Context, order: Order) {
    let Some(m) = tab.marks.dups.clone() else { return };
    if m.order == order { return }
    let view = in_view(tab);
    tab.marks_mut_dups(Some(Arc::new(DupMarks { keys: m.keys.clone(), rules: m.rules.clone(), names: m.names.clone(), dupes: m.dupes.clone(), order, diff: Default::default() })));
    DRAFTS.with(|d| if let Some(x) = d.borrow_mut().get_mut(&tab.uid) { x.2 = order });
    if view { tab.restrict_to(VIEW_LABEL, m.dupes.side_by_side(order), ctx); }
}

/// The chip's ×: the marks gone (and their view).
pub fn clear(tab: &mut Tab, ctx: &egui::Context) {
    if in_view(tab) { tab.restrict = None; tab.refresh(ctx); }
    tab.marks_mut_dups(None);
}

/// A hovered marked row's bar, right of the row number (`#dup-bar`): a
/// delete button 24 × 24 and, when the group has other rows shown, "Merge".
/// `left`: the gutter's right edge + 4; `row`: the row's rect. While the
/// pointer is on Merge, `merge_hint` names the row kept: the grid then draws
/// `Icon::MergeKeep` (16 px `txt_strong`) in place of that row's number and
/// `Icon::MergeDrop` (16 px `diff_txt`) in place of the others' of its group.
pub fn bar(ui: &mut Ui, p: &Palette, tab: &Tab, id: RowId, left: f32, row: Rect) -> Option<DupAct> {
    let m = tab.marks.dups.as_ref()?;
    let g = m.group(id)?;
    let others = others_shown(tab, m, g, id).len();
    let font = kit::ui(12.0);
    let y = row.center().y;
    let mut act = None;
    let shadow = |pt: &egui::Painter, r: Rect| pt.add(egui::Shadow { offset: [0, 2], blur: 6, spread: 0, color: egui::Color32::from_black_alpha(77) }.as_shape(r, 6.0));
    let painter = ui.painter().clone().with_layer_id(egui::LayerId::new(egui::Order::Foreground, ui.scope_id().with("dup-bar")));
    // Delete.
    let dr = Rect::from_min_size(pos2(left, y - 12.0), vec2(24.0, 24.0));
    let dresp = ui.interact(dr, ui.scope_id().with(("dup-del", id)), Sense::click()).on_hover_text("Delete this row");
    let hot = dresp.hovered();
    shadow(&painter, dr);
    painter.rect(dr, 6.0, if hot { p.danger } else { p.surf_3 }, Stroke::new(1.0, if hot { p.danger } else { p.brd_2 }), StrokeKind::Inside);
    icons::paint(&painter, Icon::DupDelete, Rect::from_center_size(dr.center(), vec2(13.0, 13.0)), if hot { p.on_danger } else { p.txt_hi });
    if dresp.clicked() { act = Some(DupAct::Delete(id)); }
    // Merge.
    let mut hint = false;
    let mut x = dr.max.x;
    if others > 0 {
        let g = painter.layout_no_wrap("Merge".into(), font, p.txt_hi);
        let mr = Rect::from_min_size(pos2(dr.max.x + 4.0, y - 12.0), vec2(8.0 + 13.0 + 5.0 + g.size().x + 8.0, 24.0));
        let mresp = ui.interact(mr, ui.scope_id().with(("dup-merge", id)), Sense::click());
        let hot = mresp.hovered();
        hint = hot;
        shadow(&painter, mr);
        painter.rect(mr, 6.0, if hot { p.prim } else { p.surf_3 }, Stroke::new(1.0, if hot { p.prim } else { p.brd_2 }), StrokeKind::Inside);
        let fg = if hot { p.on_prim } else { p.txt_hi };
        icons::paint(&painter, Icon::DupMerge, Rect::from_min_size(pos2(mr.min.x + 8.0, y - 6.5), vec2(13.0, 13.0)), fg);
        painter.galley_with_override_text_color(pos2(mr.min.x + 26.0, kit::snap(ui, y - g.size().y / 2.0)), g, fg);
        if hot {
            let tip = if others == 1 { "Keep this row, fill its empty cells from the other, delete the other".to_string() } else { format!("Keep this row, fill its empty cells from the {others} others, delete them") };
            egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.scope_id().with("dup-tip"), egui::PopupAnchor::Position(pos2(mr.min.x, row.max.y + 10.0)))
                .gap(0.0).show(|ui| { ui.label(egui::RichText::new(tip).font(kit::ui(12.0)).color(p.txt_hi)); });
        }
        if mresp.clicked() { act = Some(DupAct::Merge(id)); }
        x = mr.max.x;
    }
    // How sure the group is: a pill, its reasons in the tooltip.
    let s = m.dupes.confidence(g);
    let c = tier_color(p, s);
    let tg = painter.layout_no_wrap(format!("{s} %"), crate::grid::semibold(12.0), c);
    let sr = Rect::from_min_size(pos2(x + 4.0, y - 12.0), vec2(tg.size().x + 16.0, 24.0));
    let sresp = ui.interact(sr, ui.scope_id().with(("dup-sure", id)), Sense::hover());
    shadow(&painter, sr);
    painter.rect(sr, 6.0, if sresp.hovered() { p.surf_2 } else { p.surf_3 }, Stroke::new(1.0, alpha(c, 0.6)), StrokeKind::Inside);
    painter.galley(pos2(sr.min.x + 8.0, kit::snap(ui, y - tg.size().y / 2.0)), tg, c);
    if sresp.hovered() {
        let n = m.dupes.members(g).len();
        let tip = format!("{}\nGroup of {} rows", m.why(g), kit::n(n));
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.scope_id().with("dup-sure-tip"), egui::PopupAnchor::Position(pos2(sr.min.x, row.max.y + 10.0)))
            .gap(0.0).show(|ui| { ui.label(egui::RichText::new(tip).font(kit::ui(12.0)).color(p.txt_hi)); });
    }
    ui.ctx().data_mut(|d| if hint { d.insert_temp(egui::Id::unique("dup-merge-hint"), (id, g)); } else { d.remove::<(RowId, u32)>(egui::Id::unique("dup-merge-hint")); });
    act
}

/// While the pointer is on a Merge button: the row kept and its group.
pub fn merge_hint(ctx: &egui::Context) -> Option<(RowId, u32)> {
    ctx.data(|d| d.get_temp(egui::Id::unique("dup-merge-hint")))
}

/// The other rows of group g that are shown (all of them in the duplicates view).
fn others_shown(tab: &Tab, m: &DupMarks, g: u32, id: RowId) -> Vec<RowId> {
    let members = m.dupes.members(g);
    let shown: Box<dyn Fn(RowId) -> bool> = match &tab.view {
        // The view may be millions of rows: a set of it only when the group is not all in sight.
        Some(v) if !in_view(tab) => {
            let set: std::collections::HashSet<RowId> = members.iter().copied().collect();
            let found: std::collections::HashSet<RowId> = v.iter().copied().filter(|x| set.contains(x)).collect();
            Box::new(move |x| found.contains(&x))
        }
        _ => Box::new(|_| true),
    };
    // In view order (the order of the view; the group's file order otherwise).
    let mut out: Vec<RowId> = members.iter().copied().filter(|&x| x != id && shown(x)).collect();
    if let Some(v) = &tab.view {
        if out.len() > 1 {
            let pos: HashMap<RowId, usize> = v.iter().enumerate().filter(|(_, x)| out.contains(x)).map(|(k, &x)| (x, k)).collect();
            out.sort_by_key(|x| pos.get(x).copied().unwrap_or(usize::MAX));
        }
    }
    out
}

/// Carries out a bar's or a cell arrow's request (the app routes the grid's
/// action here): one undo step each; the groups are then counted again.
pub fn apply(cx: &mut Cx, act: DupAct) {
    let Some(m) = cx.tab.marks.dups.clone() else { return };
    if cx.tab.table.is_none() { return; }
    let msg = match act {
        DupAct::Delete(id) => {
            let del: std::collections::HashSet<RowId> = [id].into();
            cx.tab.table.as_mut().unwrap().delete_rows(&del);
            drop_from_view(cx.tab, &del);
            None
        }
        DupAct::Merge(id) => {
            let Some(g) = m.group(id) else { return };
            let others = others_shown(cx.tab, &m, g, id);
            if others.is_empty() { return; }
            let st = cx.tab.st().unwrap();
            let merged = eng::merge(st, id, &others);
            let changes: Vec<(RowId, usize, String)> = if merged.filled > 0 { merged.row.into_iter().enumerate().map(|(c, v)| (id, c, v)).collect() } else { Vec::new() };
            let del: std::collections::HashSet<RowId> = others.iter().copied().collect();
            let t = cx.tab.table.as_mut().unwrap();
            t.batch(|t| { t.set_cells(&changes); t.delete_rows(&del); });
            drop_from_view(cx.tab, &del);
            let n = others.len() + 1;
            let row = cx.tab.st().map_or(0, |s| file_number(s, id));
            Some(format!("{n} rows merged into row {}: {} empty cell{} filled, {} differing value{} kept from it — not written yet, use Save.",
                kit::n(row), kit::n(merged.filled), if merged.filled == 1 { "" } else { "s" }, kit::n(merged.kept), if merged.kept == 1 { "" } else { "s" }))
        }
        DupAct::Copy { from, to, col } => {
            let st = cx.tab.st().unwrap();
            let (v, old) = (st.cell(from, col), st.cell(to, col));
            if v == old { return; }
            cx.tab.table.as_mut().unwrap().set_cells(&[(to, col, v.clone())]);
            Some(format!("\"{v}\" copied into row {}, column {} — not written yet, use Save.", kit::n(cx.tab.st().map_or(0, |s| file_number(s, to))), cx.tab.st().map(|s| s.headers()).and_then(|h| h.get(col).cloned()).unwrap_or_default()))
        }
    };
    // The rows stay where they are on screen: the view is recomputed in place.
    let scroll = cx.tab.grid.scroll;
    cx.tab.after_edit(true, cx.ctx);
    cx.tab.grid.scroll = scroll;
    if let Some(msg) = msg { cx.toast(msg); }
}

/// A row's number as the gutter shows it: its record's place in the file.
fn file_number(st: &State, id: RowId) -> usize {
    (id as usize + 1).saturating_sub(st.header as usize)
}

/// The marked groups counted again (`Tab::poll` starts it a moment after the table changed: an
/// edit, an undo, a tool, a save that renumbered the rows): one job per tab, a newer one
/// cancelling the one under way, so an older count never lands over a newer one.
pub struct Refresh {
    job: Job<Option<(Dupes, Vec<Rule>)>>,
    marks: Arc<DupMarks>,
    /// The table the count is of (`Tab::table_key`).
    key: (usize, u64),
}

/// Starts the count for the marks shown (or held back while the rows were renumbered).
pub fn start_refresh(tab: &mut Tab, ctx: &egui::Context) {
    let Some(m) = tab.marks.dups.clone().or_else(|| tab.dups_held.clone()) else { return };
    let Some(st) = tab.st().cloned() else { return };
    let headers = st.headers();
    match compile(&headers, &m.keys) {
        Ok(Some(rules)) => {
            let names: Vec<usize> = m.names.iter().filter_map(|n| headers.iter().position(|h| h == n)).collect();
            let key = tab.table_key();
            // The one under way is dropped, hence cancelled.
            tab.dups_job = Some(Refresh { job: Job::spawn(ctx, move |cancel, _| eng::find_rules(&st, &all_ids(&st), &rules, &names, cancel, None).map(|d| (d, rules))), marks: m, key });
        }
        // A compared column is gone: the marks go.
        _ => {
            tab.dups_held = None;
            clear(tab, ctx);
        }
    }
}

/// Takes the count once done: the marks and the duplicates view follow.
pub fn poll_refresh(tab: &mut Tab, ctx: &egui::Context) {
    let Some(r) = tab.dups_job.as_mut().and_then(|j| j.job.poll()) else { return };
    let Refresh { marks: m, key, .. } = tab.dups_job.take().unwrap();
    // Marks changed meanwhile (cleared, or set anew): this count is not theirs.
    let held = tab.dups_held.take();
    let current = tab.marks.dups.as_ref().or(held.as_ref());
    if !current.is_some_and(|c| Arc::ptr_eq(c, &m)) { return; }
    let Some((d, rules)) = r else { return };
    let view = in_view(tab) || (held.is_some() && tab.restrict.as_ref().is_some_and(|r| r.label == VIEW_LABEL));
    if d.groups() == 0 {
        if view { tab.restrict = None; tab.refresh(ctx); }
        tab.marks_mut_dups(None);
        return;
    }
    let ids = d.side_by_side(m.order);
    tab.marks_mut_dups(Some(Arc::new(DupMarks { keys: m.keys.clone(), rules, names: m.names.clone(), dupes: Arc::new(d), order: m.order, diff: Default::default() })));
    // Counted on that table: changed since, it is counted again.
    tab.dups_seen = key;
    if view {
        tab.restrict = Some(Restrict { label: VIEW_LABEL.into(), ids: Arc::new(ids) });
        let scroll = tab.grid.scroll;
        tab.refresh(ctx);
        tab.grid.scroll = scroll;
    }
}

impl Tab {
    /// Sets (or clears) the duplicate marks, keeping the other marks: counted on the table as
    /// it is now (an edit after this counts them again).
    pub fn marks_mut_dups(&mut self, d: Option<Arc<DupMarks>>) {
        if d.is_none() { self.dups_job = None; self.dups_held = None; }
        self.dups_seen = self.table_key();
        Arc::make_mut(&mut self.marks).dups = d;
    }
}

fn has_dups(t: &Tab) -> bool {
    t.table.is_some() && t.marks.dups.is_some()
}

pub fn commands() -> Vec<Command> {
    let open = |cx: &mut Cx| { let d = Dialog::new(cx); cx.out.push(Cmd::Tool(Box::new(d))); };
    vec![
        Command { name: "Remove duplicates…", group: "Rows", keys: None, enabled: super::has_table, run: open },
        Command { name: "Mark duplicates…", group: "Rows", keys: None, enabled: super::has_table, run: open },
        Command { name: "Show only the duplicate rows", group: "Rows", keys: None, enabled: |t| has_dups(t) && !in_view(t), run: |cx| toggle_view(cx.tab, cx.ctx) },
        Command { name: "Show all rows again (leave the duplicates view)", group: "Rows", keys: None, enabled: |t| has_dups(t) && in_view(t), run: |cx| toggle_view(cx.tab, cx.ctx) },
        Command { name: "Remove the duplicate marks", group: "Rows", keys: None, enabled: has_dups, run: |cx| clear(cx.tab, cx.ctx) },
        Command { name: "Duplicate groups: least sure first", group: "Rows", keys: None, enabled: |t| t.marks.dups.as_ref().is_some_and(|m| m.order != Order::LeastSure), run: |cx| reorder(cx.tab, cx.ctx, Order::LeastSure) },
        Command { name: "Duplicate groups: in file order", group: "Rows", keys: None, enabled: |t| t.marks.dups.as_ref().is_some_and(|m| m.order != Order::File), run: |cx| reorder(cx.tab, cx.ctx, Order::File) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use csvfab_engine::{Base, OpenOptions, Table};
    use std::sync::atomic::AtomicBool;

    fn table(csv: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(csv.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn field(col: &str, func: usize) -> FieldDraft {
        FieldDraft { col: col.into(), func, tol: 0.8, expr: String::new(), cross: None }
    }

    fn find(t: &Table, rules: &[Rule]) -> Dupes {
        eng::find_rules(&t.st, &all_ids(&t.st), rules, &[], &AtomicBool::new(false), None).unwrap()
    }

    #[test]
    fn compile_and_find() {
        let t = table("Last,First,Email\nDupont,Ana,ana@x.io\nDUPONT, ana ,ANA@X.IO\nMartin,Léo,\nmartin,leo,\n");
        let h = t.st.headers();
        // E-mail OR last + first (slug): rows 1–2 by e-mail, 3–4 by name.
        let keys = vec![KeyDraft { fields: vec![field("Email", 5)] }, KeyDraft { fields: vec![field("Last", 2), field("First", 2)] }];
        let rules = compile(&h, &keys).unwrap().unwrap();
        let d = find(&t, &rules);
        assert_eq!((d.groups(), d.rows(), d.removable()), (2, 4, 2));
        assert_eq!(d.side_by_side(Order::File).len(), 4);
        assert_eq!(spec_text(&keys), "Email (email) OR Last (slug) + First (slug)");
        assert_eq!(d.reason(0), "same Email · same Last + First");
        assert_eq!(d.reason(1), "same Last + First");
        // A column gone, a formula that does not compile.
        assert!(compile(&h, &[KeyDraft { fields: vec![field("Phone", 0)] }]).unwrap().is_none());
        let bad = FieldDraft { expr: "left(v,".into(), ..field("Last", FX) };
        assert!(compile(&h, &[KeyDraft { fields: vec![bad] }]).unwrap_err().starts_with("Last:"));
        let fx = FieldDraft { expr: "left(slug(v), 3)".into(), ..field("Last", FX) };
        let rules = compile(&h, &[KeyDraft { fields: vec![fx] }]).unwrap().unwrap();
        assert_eq!(find(&t, &rules).groups(), 2);
    }

    #[test]
    fn cross_fields() {
        let t = table("Prénom,Nom,Nom complet,Tél,Mobile,Pays\nJean,Dupont,,01 02 03 04 05,,France\n,,DUPONT Jean,,+33 1 02 03 04 05,\nAna,Lopez,,,,\nLopez,Ana,,,,\n");
        let h = t.st.headers();
        let cross = |kind, cols: &[&str]| FieldDraft { cross: Some(Cross { kind, cols: cols.iter().map(|s| s.to_string()).collect(), sound: false, initials: false, canon: false, cc: 33, country: (kind == CrossKind::Phone).then(|| "Pays".to_string()) }), ..field("", 0) };
        let keys = vec![KeyDraft { fields: vec![cross(CrossKind::Name, &["Prénom", "Nom", "Nom complet"]), cross(CrossKind::Phone, &["Tél", "Mobile"])] }];
        let rules = compile(&h, &keys).unwrap().unwrap();
        assert_eq!(rules[0].label, "same name + phone");
        let d = find(&t, &rules);
        assert_eq!(d.members(0).len(), 2);
        assert_eq!(d.groups(), 1, "Ana Lopez has no phone");
        assert_eq!(spec_text(&keys), "name + any phone (+33)");
        // Round trip through the engine's rules.
        assert_eq!(drafts_of(&rules, &h), keys);
        // A cross column gone.
        let gone = vec![KeyDraft { fields: vec![cross(CrossKind::Email, &["Email"])] }];
        assert!(compile(&h, &gone).unwrap().is_none());
        // Modes.
        let mut c = keys[0].fields[0].cross.clone().unwrap();
        c.set_mode(3);
        assert!(c.sound && c.initials);
        assert_eq!(c.modes().1, 3);
        let mut c = keys[0].fields[1].cross.clone().unwrap();
        c.set_mode(4);
        assert_eq!((c.cc, c.modes().1), (1, 4));
    }

    #[test]
    fn smart_suggestions() {
        let t = table("Prénom,Nom,Email,Téléphone,Ville\nJean,Dupont,jd@x.fr,0612345678,Lyon\nJean,DUPONT,JD@x.fr,,Lyon\nAna,Lopez,a@x.fr,0712345678,Paris\nLopez,Ana,,07 12 34 56 78,Paris\nLéo,Petit,l@x.fr,,Nice\n");
        let st = &t.st;
        let roles = eng::roles(st);
        let h = st.headers();
        let sugg = eng::suggest(&roles, &h);
        let titles: Vec<&str> = sugg.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles[0], "Same person: name + phone OR name + e-mail");
        assert!(titles.contains(&"Same phone number") && titles.contains(&"Same name + Ville"), "{titles:?}");
        let keys = drafts_of(&sugg[0].rules, &h);
        assert_eq!(spec_text(&keys), "name + any phone (+33) OR name + any e-mail");
        let rules = compile(&h, &keys).unwrap().unwrap();
        let d = find(&t, &rules);
        assert_eq!((d.groups(), d.rows()), (2, 4));
    }

    #[test]
    fn suggested_keys() {
        let t = table("Nom,Prénom,Code postal,Email,Téléphone\nDupont,Ana,75011,ana@x.io,0612345678\nMartin,Léo,69001,leo@y.fr,0698765432\n");
        let keys = suggest(&t.st);
        let cols: Vec<Vec<&str>> = keys.iter().map(|k| k.fields.iter().map(|f| f.col.as_str()).collect()).collect();
        assert!(cols.iter().any(|k| k.contains(&"Email")), "{cols:?}");
        assert!(keys.iter().flat_map(|k| &k.fields).any(|f| f.col == "Email" && f.func == 5));
        // An address on every row is not a person's.
        let t = table("Email,Created_by\na@x.io,admin@x.io\nb@x.io,admin@x.io\nc@x.io,admin@x.io\nd@x.io,admin@x.io\n");
        let keys = suggest(&t.st);
        assert!(keys.iter().flat_map(|k| &k.fields).all(|f| f.col != "Created_by"), "{keys:?}");
    }

    #[test]
    fn row_numbers() {
        let mut t = table("A\n1\n2\n3\n");
        let (first, second) = (t.st.data_id(0), t.st.data_id(1));
        assert_eq!(file_number(&t.st, second), 2);
        t.delete_rows(&[first].into());
        // The gutter keeps a record's number.
        assert_eq!(file_number(&t.st, t.st.data_id(0)), 2);
    }
}
