//! The application: open tabs, the tools open on them, saving, undo, the
//! actions the grid asks for, and the dialogs of the shell itself.

use crate::config::Config;
use crate::grid::{self, Action};
use crate::job::Job;
use crate::tab::Tab;
use crate::theme::{self, Metrics, Theme};
use crate::tools::{self, Cmd, Cx, Tool};
use csvfab_engine::base::OpenOptions;
use csvfab_engine::save::{self, SaveError, SaveOptions};
use csvfab_engine::RowId;
use eframe::egui;
use egui::{Key, KeyboardShortcut, Modifiers, RichText};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, OnceLock};

/// The screen scale (native pixels per point) the window last ran at, from the settings:
/// `None` for the screenshot renderer (no window), `Some(0)` for a window whose scale is
/// not known yet. Set by `crate::early`.
static EXPECTED_SCALE: OnceLock<u32> = OnceLock::new();

pub fn expect_scale(scale: Option<f32>) {
    let _ = EXPECTED_SCALE.set(scale.map_or(0, f32::to_bits));
}

/// A Wayland window's first frame is drawn before the compositor has said which screen
/// it is on: at scale 1, thrown away as soon as the scale arrives — the text laid out, the
/// icons rasterized, the textures uploaded for nothing, and the half-size layout flashing
/// on a HiDPI screen. When the window last ran at another scale, that frame is left empty
/// (the theme's background) and the real one follows as soon as the scale is known.
pub fn provisional(ctx: &egui::Context) -> bool {
    EXPECTED_SCALE.get().is_some_and(|&e| e != 0 && ctx.cumulative_frame_nr() == 0 && ctx.input(|i| i.viewport().native_pixels_per_point).is_some_and(|s| s.to_bits() != e))
}

static FPS: OnceLock<bool> = OnceLock::new();
static SLOW: OnceLock<Option<f64>> = OnceLock::new();

/// Drops `x` on a background thread: the last reference to a closed or reloaded file
/// (unmapping hundreds of MB, freeing its index) costs the UI thread nothing.
pub fn drop_later<T: Send + 'static>(x: T) {
    static TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<Box<dyn Send>>>> = std::sync::Mutex::new(None);
    let mut tx = TX.lock().unwrap_or_else(|e| e.into_inner());
    let tx = tx.get_or_insert_with(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn Send>>();
        let _ = std::thread::Builder::new().name("csvfab-drop".into()).spawn(move || for x in rx { drop(x) });
        tx
    });
    let _ = tx.send(Box::new(x));
}

/// This thread's time on a CPU so far, ns (`/proc/thread-self/schedstat`): what a frame
/// cost, whatever else the machine runs.
fn cpu_ns() -> u64 {
    std::fs::read_to_string("/proc/thread-self/schedstat").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(0)
}

/// (minor, major) page faults of the process so far (a slow frame reading the file from disk).
fn faults() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let f: Vec<&str> = s.rsplit_once(')').map_or("", |x| x.1).split_whitespace().collect();
    let n = |i: usize| f.get(i).and_then(|x| x.parse().ok()).unwrap_or(0);
    (n(7), n(9))
}

struct Laps {
    /// CPU ns at the frame's start and at the last lap.
    start: u64,
    at: u64,
    wall: Option<std::time::Instant>,
    faults: (u64, u64),
    parts: Vec<(&'static str, f64)>,
}

thread_local! {
    static LAPS: std::cell::RefCell<Laps> = const { std::cell::RefCell::new(Laps { start: 0, at: 0, wall: None, faults: (0, 0), parts: Vec::new() }) };
}

/// `CSVFAB_SLOW=<ms>`: a frame that kept the UI thread on the CPU longer than that prints
/// its parts on stderr (where the time went). `lap` closes the part named `what`.
pub fn lap(what: &'static str) {
    if SLOW.get().copied().flatten().is_none() { return; }
    LAPS.with_borrow_mut(|l| {
        let now = cpu_ns();
        l.parts.push((what, (now - l.at) as f64 / 1e6));
        l.at = now;
    });
}

fn laps_start() {
    if SLOW.get_or_init(|| std::env::var("CSVFAB_SLOW").ok().and_then(|s| s.parse().ok())).is_none() { return; }
    LAPS.with_borrow_mut(|l| {
        (l.start, l.wall, l.faults) = (cpu_ns(), Some(std::time::Instant::now()), faults());
        l.at = l.start;
        l.parts.clear();
    });
}

fn laps_end() {
    let Some(limit) = SLOW.get().copied().flatten() else { return };
    LAPS.with_borrow(|l| {
        let ms = (cpu_ns() - l.start) as f64 / 1e6;
        if ms < limit { return; }
        let wall = l.wall.map_or(0.0, |w| w.elapsed().as_secs_f64() * 1e3);
        let f = faults();
        let parts: Vec<String> = l.parts.iter().filter(|x| x.1 >= 0.05).map(|(w, t)| format!("{w} {t:.2}")).collect();
        eprintln!("slow frame {ms:.1} ms on the CPU ({wall:.1} wall; {} minor, {} major page faults): {}", f.0 - l.faults.0, f.1 - l.faults.1, parts.join(", "));
    });
}
static SHOT: OnceLock<bool> = OnceLock::new();

/// An environment switch, read once.
fn env_set(cell: &OnceLock<bool>, name: &str) -> bool {
    *cell.get_or_init(|| std::env::var_os(name).is_some())
}

pub enum Dialog {
    GoTo(String),
    Rename { col: usize, text: String },
    /// A column inserted at `at` once named (as the web's prompt).
    InsertColumn { at: usize, text: String },
    /// Close tab `usize` although it has unsaved changes.
    CloseDirty(usize),
    QuitDirty,
    Message(String),
    ReplaceChanged(usize),
    /// Copies of unsaved edits left by a csvfab that stopped without asking.
    Recover(Vec<crate::recovery::Left>),
    /// Another program changed tab `tab`'s file, cut it short or removed it, seen while open.
    OnDisk { tab: usize, what: OnDisk },
    /// Saving tab `tab` in its own single-byte encoding would write `n` cells' characters
    /// (the first `c`) as "?"; `force`: overwrite a file changed on disk.
    Lossy { tab: usize, n: usize, c: char, force: bool },
    /// Read tab `tab` again in another encoding or with another delimiter, losing its edits.
    Reread { tab: usize, enc: crate::tab::Choice<csvfab_engine::Enc>, delim: crate::tab::Choice<u8> },
    /// A deletion asked about first: its question, its button, then the action (taken on OK).
    Confirm { title: String, sub: String, ok: String, then: Option<Action> },
    Shortcuts,
    About,
    WhatsNew,
}

/// What another program did to an open file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnDisk {
    Changed,
    /// Truncated under the map: the bytes past its new end read as zeros (mapguard).
    Cut,
    Gone,
}

/// What each edit told about did, by (tab, version): Undo / Redo say it again, Review
/// changes lists it.
/// The files open in tabs: (tab, path, name), noted before commands and tools run, for one
/// writing a file to refuse a file another tab holds (that tab would no longer match it).
static OPEN: std::sync::Mutex<Vec<(u64, PathBuf, String)>> = std::sync::Mutex::new(Vec::new());

/// The name of the tab other than `uid` that holds `path`, if one does.
pub fn open_elsewhere(path: &std::path::Path, uid: u64) -> Option<String> {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    OPEN.lock().ok()?.iter().find(|(u, p, _)| *u != uid && *p == canon).map(|x| x.2.clone())
}

pub mod edit_what {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static WHAT: Mutex<Option<HashMap<(u64, u64), String>>> = Mutex::new(None);

    pub fn note(key: (u64, u64), what: String) {
        if let Ok(mut m) = WHAT.lock() { m.get_or_insert_with(HashMap::new).insert(key, what); }
    }

    pub fn get(key: (u64, u64)) -> Option<String> {
        WHAT.lock().ok()?.as_ref()?.get(&key).cloned()
    }
}

struct Saving {
    tab: u64,
    version: u64,
    job: Job<Result<Saved, SaveFail>>,
    path: PathBuf,
    same_file: bool,
    force: bool,
}

impl<X> crate::job::Failed for Result<X, SaveFail> {
    fn failed(msg: &str) -> Self { Err(SaveFail::Save(SaveError::Io(std::io::Error::other(msg.to_string())))) }
}

/// What a save wrote: the file's stamp and sample (`base::file_sample`), the file as read
/// again (when the save could tell its index), the rows' new ids (`tab::renumbering`).
type Saved = (csvfab_engine::Stamp, Option<u64>, Option<crate::tab::Ready>, Option<Arc<Vec<u32>>>);

/// The stamps a tab's file is known to have from us: (its stamp as read, the one we wrote or
/// found again on our content, the sample of that content).
type Written = (Option<csvfab_engine::Stamp>, csvfab_engine::Stamp, Option<u64>);

/// Why a save wrote nothing.
enum SaveFail {
    Save(SaveError),
    /// Edits hold characters the file's single-byte encoding cannot write: (cells, the first).
    Lossy(usize, char),
}

pub struct Toast {
    pub text: String,
    pub until: f64,
    /// Its own: the animations follow it while its time is pushed back (hovered).
    pub id: u64,
    /// The edit it tells about, still undoable from it: (tab, table version).
    pub undo: Option<(u64, u64)>,
}

impl Toast {
    pub fn new(text: String, until: f64) -> Toast {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Toast { text, until, id: N.fetch_add(1, std::sync::atomic::Ordering::Relaxed), undo: None }
    }
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub theme: &'static Theme,
    pub m: Metrics,
    pub dialog: Option<Dialog>,
    picking: Option<Job<Vec<PathBuf>>>,
    saving: Option<Saving>,
    /// A big selection being copied (its text built off the UI thread).
    copying: Option<Job<Option<csvfab_engine::edit::clipboard::Copied>>>,
    pub card: crate::rowcard::Card,
    pub font_picker: Option<crate::fontpick::Picker>,
    pub toasts: Vec<Toast>,
    /// The active tab and its table's version at the end of the last frame: a toast
    /// following an edit made since offers to undo it.
    seen: (u64, u64),
    /// Per tab (uid): the file's stamp before a save of ours and the one it wrote: while
    /// the table still stands on that older read, the file on disk is expected to be ours.
    written: std::collections::HashMap<u64, Written>,
    /// A look at whether a file whose stamp changed still holds our content (`Stamp::holds`):
    /// (tab, the stamp found, the job: the sample if it does).
    disk_job: Option<(u64, csvfab_engine::Stamp, Job<Option<u64>>)>,
    /// Per tab: the change on disk already told about (not asked again for the same).
    told: std::collections::HashMap<u64, Option<csvfab_engine::Stamp>>,
    /// When the files were last looked at, and the cuts counted then (mapguard).
    disk_at: f64,
    cuts_seen: usize,
    /// Unsaved edits kept on disk against a crash; the copies of a run that crashed, looked
    /// for once at start.
    pub recovery: crate::recovery::Recovery,
    pub recover_checked: bool,
    quit_ok: bool,
    pub focus_search: bool,
    pub config: Config,
    incoming: Option<Receiver<Vec<PathBuf>>>,
    /// Tools open, with the uid of the tab they work on.
    pub tools: Vec<(u64, Box<dyn Tool>)>,
    pub palette: Option<crate::chrome::Launcher>,
    pending: Vec<Cmd>,
    /// A row to go to once the view is computed again (its filters just cleared): (tab, id).
    pending_goto: Option<(u64, RowId)>,
    /// A text field's right-click menu, and the events it sends to the field next frame.
    pub field_menu: Option<crate::fieldmenu::FieldMenu>,
    /// How the next file opened is read, chosen with no file open (the web's reading
    /// defaults): its delimiter and encoding, None detected.
    pub next_opts: OpenOptions,
    pub next_header: Option<bool>,
    pub inject: Vec<egui::Event>,
    /// A field had the keyboard last frame: an Escape now was its own (egui drops the focus
    /// before the frame runs), not the grid's.
    focus_last: bool,
    /// A line for the status bar, after the file's name: (tab, text, when).
    pub status: Option<(u64, String, f64)>,
    /// Commands to run after the grid's actions (they need the whole app).
    deferred: Vec<Box<dyn FnOnce(&mut Cx)>>,
    /// The window title last sent.
    title: String,
    /// The recent files' sizes (None: missing) for the welcome screen, looked up off the UI
    /// thread when the list changes or the window comes back to the front: (the list, sizes).
    recent_meta: Option<(Vec<PathBuf>, Vec<Option<u64>>)>,
    recent_job: Option<(Vec<PathBuf>, Job<Vec<Option<u64>>>)>,
}

impl App {
    /// Everything on the calling thread (the screenshot renderer); a window starts
    /// through `crate::early` instead, which does the same work beside the window's creation.
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<PathBuf>) -> App {
        let config = Config::load();
        Self::style(&cc.egui_ctx, config.theme.as_deref(), config.mono_font.as_deref());
        Self::build(&cc.egui_ctx, config, files)
    }

    /// Fonts, icons and the theme into `ctx`: any thread, before the first frame.
    pub fn style(ctx: &egui::Context, theme: Option<&str>, mono_font: Option<&str>) {
        crate::fonts::install(ctx, mono_font);
        crate::icons::install(ctx);
        theme::pointing_hand(ctx);
        theme::apply(ctx, theme::by_id(theme.unwrap_or(theme::DEFAULT_THEME)), &Metrics::default());
    }

    /// The app on `files` (their reading starts now): needs no window.
    pub fn build(ctx: &egui::Context, config: Config, files: Vec<PathBuf>) -> App {
        let theme = theme::by_id(config.theme.as_deref().unwrap_or(theme::DEFAULT_THEME));
        theme::set_look(config.look.unwrap_or(theme::ALL));
        let m = Metrics::default();
        let c = ctx.clone();
        // Tests (off screen) never take over the socket of the user's running window.
        let incoming = if cfg!(test) || std::env::var_os("CSVFAB_SHOT").is_some() { None } else { crate::instance::listen(move || c.request_repaint()) };
        let mut app = App {
            tabs: Vec::new(),
            active: 0,
            theme,
            m,
            dialog: None,
            picking: None,
            saving: None,
            copying: None,
            card: Default::default(),
            font_picker: None,
            toasts: Vec::new(),
            seen: (0, 0),
            written: Default::default(),
            disk_job: None,
            told: Default::default(),
            disk_at: 0.0,
            cuts_seen: 0,
            recovery: crate::recovery::Recovery::new(),
            recover_checked: false,
            quit_ok: false,
            focus_search: false,
            config,
            incoming,
            tools: Vec::new(),
            palette: None,
            pending: Vec::new(),
            pending_goto: None,
            focus_last: false,
            field_menu: None,
            next_opts: OpenOptions::default(),
            next_header: None,
            inject: Vec::new(),
            status: None,
            deferred: Vec::new(),
            title: String::new(),
            recent_meta: None,
            recent_job: None,
        };
        for f in files {
            app.open(f, ctx);
        }
        app
    }

    /// The recent files with their size (None: no longer there), as last looked up; a look in
    /// the background when the list changed or the window came back to the front.
    pub fn recent_files(&mut self, ctx: &egui::Context) -> Vec<(PathBuf, Option<u64>)> {
        if let Some(r) = self.recent_job.as_mut().and_then(|j| j.1.poll()) {
            let (paths, _) = self.recent_job.take().unwrap();
            self.recent_meta = Some((paths, r));
        }
        let back = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true))));
        let stale = self.recent_meta.as_ref().is_none_or(|m| m.0 != self.config.recent) || back;
        if stale && self.recent_job.as_ref().is_none_or(|j| j.0 != self.config.recent || back) {
            let paths = self.config.recent.clone();
            let p2 = paths.clone();
            self.recent_job = Some((paths, Job::spawn(ctx, move |_, _| p2.iter().map(|p| std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len())).collect())));
        }
        match &self.recent_meta {
            Some((paths, sizes)) if *paths == self.config.recent => paths.iter().cloned().zip(sizes.iter().copied()).collect(),
            // Until it answers: the list, sizes unknown (drawn as present).
            _ => self.config.recent.iter().map(|p| (p.clone(), Some(u64::MAX))).collect(),
        }
    }

    pub fn set_theme(&mut self, id: &str, ctx: &egui::Context) {
        self.theme = theme::by_id(id);
        theme::apply(ctx, self.theme, &self.m);
        self.config.theme = Some(self.theme.id.to_string());
        self.config.save();
    }

    pub fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        // A folder (dropped, or named on the command line): its tables, each in a tab (not its sub-folders).
        if path.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&path).into_iter().flatten().flatten().map(|e| e.path())
                .filter(|p| p.is_file() && (csvfab_engine::convert::import_format(p).is_some()
                    || p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["csv", "tsv", "txt", "tab", "psv", "dat", "log"].contains(&e.to_ascii_lowercase().as_str()))))
                .collect();
            files.sort();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match files.len() {
                0 => self.toast(format!("No table in the folder {name}: CSV, TSV, text, Excel, JSON or Parquet files"), ctx),
                n if n > 40 => self.toast(format!("The folder {name} holds {n} tables: open them from the file dialog instead (Ctrl+O)"), ctx),
                _ => for f in files { self.open(f, ctx); },
            }
            return;
        }
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_ref() == Some(&path)) {
            self.active = i;
            return;
        }
        self.config.add_recent(&path);
        self.config.save();
        // A workbook, a database, JSON or Parquet: converted to a CSV beside it first (tools::import).
        if csvfab_engine::convert::import_format(&path).is_some() {
            if let Some(t) = crate::tools::import::open(path) { self.tools.push((0, t)); }
            return;
        }
        // Not a table by its name (a log, code, settings, no extension): raw text, as in the
        // web — commas in a .log or a .py must not cut its lines into columns. Text and .dat
        // files are tables as often as not: read by their content.
        let table = path.extension().and_then(|e| e.to_str()).is_some_and(|e| ["csv", "tsv", "tab", "psv", "txt", "dat"].contains(&e.to_ascii_lowercase().as_str()));
        let opts = if table { self.next_opts } else { OpenOptions { delim: Some(0), ..self.next_opts } };
        self.tabs.push(Tab::open_with(path, opts, if table { self.next_header } else { None }, ctx));
        self.active = self.tabs.len() - 1;
    }

    pub fn pick_files(&mut self, ctx: &egui::Context) {
        if self.picking.is_some() { return; }
        // The system dialog blocks its thread: never the UI's.
        self.picking = Some(Job::spawn(ctx, |_, _| {
            rfd::FileDialog::new()
                .add_filter("Tables: CSV, text, Excel, JSON, SQLite, Parquet", &["csv", "tsv", "txt", "tab", "psv", "dat", "xlsx", "xlsm", "json", "jsonl", "ndjson", "sqlite", "sqlite3", "db", "db3", "parquet", "parq", "pqt"])
                .add_filter("CSV and text", &["csv", "tsv", "txt", "tab", "psv", "dat", "log"])
                .add_filter("Excel", &["xlsx", "xlsm"])
                .add_filter("JSON", &["json", "jsonl", "ndjson"])
                .add_filter("SQLite", &["sqlite", "sqlite3", "db", "db3"])
                .add_filter("Parquet", &["parquet", "parq", "pqt"])
                .add_filter("All files", &["*"])
                .pick_files()
                .unwrap_or_default()
        }));
    }

    pub fn tab(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn toast(&mut self, msg: impl Into<String>, ctx: &egui::Context) {
        let mut t = Toast::new(msg.into(), ctx.input(|i| i.time) + 5.0);
        let now = self.version_now();
        if now.0 == self.seen.0 && now.1 != self.seen.1 && self.tabs.get(self.active).and_then(|t| t.table.as_ref()).is_some_and(|t| t.can_undo()) {
            t.undo = Some(now);
            // "Row 3 moved to 7 — not written yet…": what the edit did, for Undo's line.
            let what = t.text.split(" — ").next().unwrap_or(&t.text).trim_end_matches('.').to_string();
            edit_what::note(now, what);
        }
        self.toasts.push(t);
        if self.toasts.len() > 4 { self.toasts.remove(0); }
    }

    pub fn saving(&self) -> bool {
        self.saving.is_some()
    }

    pub fn save(&mut self, as_path: Option<PathBuf>, force: bool, ctx: &egui::Context) {
        // Save in place with nothing pending: nothing to write (as the web's).
        if as_path.is_none() && !force && self.tabs.get(self.active).is_some_and(|t| t.path.is_some() && t.table.is_some() && !t.dirty()) {
            if let Some(uid) = self.tabs.get(self.active).map(|t| t.uid) { self.status = Some((uid, "Nothing to save.".into(), ctx.input(|i| i.time))); }
            return;
        }
        self.save_checked(as_path, force, false, ctx);
    }

    /// `save`; `lossy_ok`: characters the encoding cannot hold were already accepted as "?".
    fn save_checked(&mut self, as_path: Option<PathBuf>, force: bool, lossy_ok: bool, ctx: &egui::Context) {
        if self.saving.is_some() { return; }
        // Over a file another tab holds: refused (that tab would no longer match its file).
        if let Some(p) = &as_path {
            self.note_open();
            if let Some(o) = self.tabs.get(self.active).and_then(|t| open_elsewhere(p, t.uid)) {
                self.dialog = Some(Dialog::Message(format!("{o} is open in another tab — close it first.")));
                return;
            }
        }
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if tab.partial() {
            self.status = Some((tab.uid, "The file is still loading: save once it is read whole.".into(), ctx.input(|i| i.time)));
            return;
        }
        let Some(table) = &tab.table else { return };
        let Some(path) = as_path.or_else(|| tab.path.clone()) else {
            self.save_as(ctx);
            return;
        };
        let st = table.st.clone();
        let version = table.version();
        let same_file = Some(&path) == tab.path.as_ref();
        let expect = if force || !same_file { None } else { Self::expected(&self.written, tab) };
        let mut backed = if same_file { tab.backed_up } else { true };
        // A single-byte encoding holds ~250 characters: an edit beyond them is said before
        // it is written as "?" (rows untouched since reading need no look).
        let check = !lossy_ok && tab.dirty() && tools::export::single_byte(st.base.enc);
        let p2 = path.clone();
        let job = Job::spawn(ctx, move |cancel, _| {
            if check {
                if let Some((n, c)) = tools::export::unencodable(&st, st.base.enc, None, cancel).filter(|x| x.0 > 0) {
                    return Err(SaveFail::Lossy(n, c));
                }
            }
            // The saved file's index comes from the save itself: nothing is read again.
            save::save_indexed(&st, &p2, &SaveOptions::default(), expect, &mut backed)
                .map(|(stamp, b)| (stamp, csvfab_engine::base::file_sample(&p2, stamp.len), b.map(|b| crate::tab::Ready::saved(&st, b)), crate::tab::renumbering(&st)))
                .map_err(SaveFail::Save)
        });
        self.saving = Some(Saving { tab: tab.uid, version, job, path, same_file, force });
    }

    /// The stamp the tab's file should have on disk: the one it was read with, or the one
    /// our last save gave it while the table still stands on that read.
    fn expected(written: &std::collections::HashMap<u64, Written>, tab: &Tab) -> Option<csvfab_engine::Expect> {
        let read = tab.st()?.base.stamp;
        match written.get(&tab.uid) {
            Some((before, ours, sample)) if *before == read => Some(csvfab_engine::Expect { stamp: *ours, sample: *sample }),
            _ => read.map(Into::into),
        }
    }

    /// Another program changed an open file, cut it short or removed it: told once per
    /// change, with Reload. Looked at when the window comes back to the front, and at most
    /// every 2 s while it is used — never by waking an idle window.
    fn check_disk(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let back = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::WindowFocused(true))));
        let used = ctx.input(|i| !i.events.is_empty() || i.pointer.any_pressed());
        let cuts = csvfab_engine::mapguard::cuts();
        if !(back || cuts != self.cuts_seen || (used && now - self.disk_at > 2.0)) { return; }
        // A file cut short is told even over "changed on disk" (it was that change, seen first).
        let open = !matches!(self.dialog, None | Some(Dialog::OnDisk { what: OnDisk::Changed, .. }));
        if open || self.saving.is_some() { return; }
        self.disk_at = now;
        self.cuts_seen = cuts;
        for (i, t) in self.tabs.iter().enumerate() {
            // A followed file cut short (a log rotated by truncation) is read again by the tail.
            if t.st().is_some_and(|st| st.base.cut_short()) && tools::tail::state(t.uid).is_none() && !matches!(self.told.get(&t.uid), Some(None)) {
                self.told.insert(t.uid, None);
                self.dialog = Some(Dialog::OnDisk { tab: i, what: OnDisk::Cut });
                return;
            }
        }
        if self.dialog.is_some() { return; }
        let i = self.active;
        let Some(t) = self.tabs.get(i) else { return };
        let Some(path) = &t.path else { return };
        if t.table.is_none() || t.busy() || tools::tail::state(t.uid).is_some() { return; }
        let Some(want) = Self::expected(&self.written, t) else { return };
        let on_disk = csvfab_engine::Stamp::of(path);
        if on_disk == Some(want.stamp) {
            self.told.remove(&t.uid);
            return;
        }
        if self.told.get(&t.uid).is_some_and(|s| *s == on_disk) { return; }
        // Another stamp, the same size: a sync client rewriting our own save, perhaps — the
        // content says (read in the background, as the web app's fingerprint).
        if let Some(now) = on_disk.filter(|n| n.len == want.stamp.len) {
            if self.disk_job.as_ref().is_some_and(|(u, s, _)| *u == t.uid && *s == now) { return; }
            let (base, path) = (t.st().unwrap().base.clone(), path.clone());
            self.disk_job = Some((t.uid, now, Job::spawn(ctx, move |_, _| {
                let sample = want.sample.or_else(|| (base.stamp == Some(want.stamp)).then(|| base.sample()).flatten())?;
                want.stamp.holds(&now, &path, || Some(sample)).then_some(sample)
            })));
            return;
        }
        self.told.insert(t.uid, on_disk);
        self.dialog = Some(Dialog::OnDisk { tab: i, what: if on_disk.is_none() { OnDisk::Gone } else { OnDisk::Changed } });
    }

    /// The answer of `disk_job`: our content under another stamp is taken as ours (expected from
    /// now on); another content is told, as any change.
    fn poll_disk_job(&mut self) {
        let Some(r) = self.disk_job.as_mut().and_then(|j| j.2.poll()) else { return };
        let (uid, now, _) = self.disk_job.take().unwrap();
        let Some(i) = self.tabs.iter().position(|t| t.uid == uid) else { return };
        let read = self.tabs[i].st().and_then(|st| st.base.stamp);
        match r {
            Some(sample) => { self.written.insert(uid, (read, now, Some(sample))); }
            None if self.dialog.is_none() => {
                self.told.insert(uid, Some(now));
                self.dialog = Some(Dialog::OnDisk { tab: i, what: OnDisk::Changed });
            }
            None => {}
        }
    }

    /// Reads tab `i`'s file again, its edits dropped (filters and sort stay).
    pub fn reload_tab(&mut self, i: usize, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get_mut(i) else { return };
        tab.table = None;
        tab.marks = Default::default();
        tab.reload(ctx);
        self.written.remove(&tab.uid);
        self.told.remove(&tab.uid);
    }

    fn poll_save(&mut self, ctx: &egui::Context) {
        let Some(r) = self.saving.as_mut().and_then(|s| s.job.poll()) else { return };
        let s = self.saving.take().unwrap();
        let i = self.tabs.iter().position(|t| t.uid == s.tab);
        match r {
            Ok((stamp, sample, ready, renumber)) => {
                let mut msg = format!("Saved {}", s.path.display());
                if let Some(tab) = i.map(|i| &mut self.tabs[i]) {
                    // As the web's: in place, how many rows, and the backup the first save keeps.
                    if s.same_file {
                        let rows = tab.st().map_or(0, |st| st.len());
                        let bak = if tab.backed_up { "" } else { " · the original kept as a dated .bak beside it" };
                        msg = format!("Saved in place — {} rows written{bak}", grid::fmt_int(rows));
                    }
                    if let Some(t) = &mut tab.table { t.mark_saved_at(s.version); }
                    // The file is ours now, even while the table stands on the read before
                    // (edits made during the save keep it): no "changed on disk" for it.
                    self.written.insert(tab.uid, (tab.st().and_then(|st| st.base.stamp), stamp, sample));
                    self.told.remove(&tab.uid);
                    // Its .bak is made: later saves this session overwrite without another.
                    if s.same_file { tab.backed_up = true; }
                    let renamed = tab.path.as_ref() != Some(&s.path);
                    tab.path = Some(s.path.clone());
                    tab.name = s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    // The file on disk is the new reference: read it again (filters and sort stay);
                    // the rows a tool showed or marked follow their new ids.
                    if !tab.dirty() || renamed { tab.renumber = renumber; }
                    match ready {
                        Some(r) if !tab.dirty() => tab.adopt(r, ctx, &self.m),
                        _ if !tab.dirty() || renamed => tab.reload(ctx),
                        _ => {}
                    }
                }
                self.config.add_recent(&s.path);
                self.config.save();
                self.toast(msg, ctx);
            }
            Err(SaveFail::Save(SaveError::Changed)) => self.dialog = Some(Dialog::ReplaceChanged(i.unwrap_or(self.active))),
            Err(SaveFail::Save(e)) => self.dialog = Some(Dialog::Message(format!("Could not write \"{}\": {e}. The original file is untouched.", s.path.display()))),
            Err(SaveFail::Lossy(n, c)) => self.dialog = Some(Dialog::Lossy { tab: i.unwrap_or(self.active), n, c, force: s.force }),
        }
    }

    /// The save dialog (name, format, encoding, the rows shown only): tools::export.
    pub fn save_as(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        if tab.table.is_none() { return; }
        if tab.partial() {
            self.status = Some((tab.uid, "The file is still loading: save once it is read whole.".into(), ctx.input(|i| i.time)));
            return;
        }
        let tool = tools::export::save_as(tab);
        self.open_tool(tool);
    }

    pub fn close_tab(&mut self, i: usize, force: bool) {
        if i >= self.tabs.len() { return; }
        if !force && self.tabs[i].dirty() {
            self.dialog = Some(Dialog::CloseDirty(i));
            return;
        }
        let uid = self.tabs[i].uid;
        self.tools.retain(|(t, _)| *t != uid);
        self.recovery.forget(uid);
        // A big file's mapping and index are freed off the UI thread.
        let table = self.tabs.remove(i).table;
        drop_later(table);
        if self.active >= self.tabs.len() { self.active = self.tabs.len().saturating_sub(1); }
    }

    /// The filter row, the column charts and the totals row are shown or hidden for every
    /// tab at once and remembered, as in the web: a new tab takes the settings; a switch
    /// turned in one tab (status bar, palette, menu) turns in all and is saved.
    fn sync_bands(&mut self, ctx: &egui::Context) {
        let c = &self.config;
        let want = (c.show_filters.unwrap_or(true), c.show_charts.unwrap_or(true), c.show_totals.unwrap_or(true));
        for t in self.tabs.iter_mut().filter(|t| !t.bands_set) {
            (t.show_filters, t.show_charts, t.show_totals, t.bands_set) = (want.0, want.1, want.2, true);
        }
        let Some(now) = self.tabs.get(self.active).map(|t| (t.show_filters, t.show_charts, t.show_totals)) else { return };
        if now == want { return; }
        for t in &mut self.tabs { (t.show_filters, t.show_charts, t.show_totals) = now; }
        // The filter row hidden: its filters go with it (in every tab), as in the web, so no
        // filter hides rows out of sight.
        if want.0 && !now.0 {
            let mut n = 0;
            for t in &mut self.tabs {
                let k = t.col_filters.iter().filter(|f| !f.is_empty()).count();
                if k > 0 {
                    t.col_filters.iter_mut().for_each(String::clear);
                    t.refresh(ctx);
                    n += k;
                }
            }
            if n > 0 { self.toast(format!("Filter row hidden — its {} filter{} cleared", grid::fmt_int(n), if n == 1 { "" } else { "s" }), ctx); }
        }
        if want.2 != now.2 { self.toast(if now.2 { "Totals row shown — click a total to change it." } else { "Totals row hidden." }, ctx); }
        (self.config.show_filters, self.config.show_charts, self.config.show_totals) = (Some(now.0), Some(now.1), Some(now.2));
        if !env_set(&SHOT, "CSVFAB_SHOT") { self.config.save(); }
    }

    pub fn open_tool(&mut self, tool: Box<dyn Tool>) {
        let Some(uid) = self.tabs.get(self.active).map(|t| t.uid) else { return };
        if let Some(k) = tool.single() { self.tools.retain(|(u, t)| *u != uid || t.single() != Some(k)); }
        self.tools.push((uid, tool));
    }

    /// Runs a command of the menu or the palette on the active tab.
    pub fn run_command(&mut self, run: fn(&mut Cx), ctx: &egui::Context) {
        self.run_with(Box::new(run), ctx);
    }

    /// Notes the files the tabs hold (`open_elsewhere`).
    fn note_open(&self) {
        if let Ok(mut o) = OPEN.lock() {
            o.clear();
            o.extend(self.tabs.iter().filter_map(|t| Some((t.uid, t.path.clone()?, t.name.clone()))));
        }
    }

    fn run_with(&mut self, run: Box<dyn FnOnce(&mut Cx)>, ctx: &egui::Context) {
        self.note_open();
        let (p, m) = (&self.theme.p, &self.m);
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let mut out = Vec::new();
        run(&mut Cx { ctx, tab, p, m, out: &mut out });
        self.pending.extend(out);
    }

    pub fn apply(&mut self, actions: Vec<Action>, ctx: &egui::Context) {
        let m = self.m.clone();
        let mut deferred = Vec::new();
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        for a in actions {
            let Some(table) = &mut tab.table else { return };
            // Deletions are asked about first (several rows, any column), as in the web.
            let (a, sure) = match a { Action::Sure(a) => (*a, true), a => (a, false) };
            let a = match a {
                Action::DeleteRows(ids) if !sure && ids.len() > 1 => {
                    let n = grid::fmt_int(ids.len());
                    self.dialog = Some(Dialog::Confirm { title: format!("Delete the {n} selected rows?"), sub: "Ctrl+Z brings them back.".into(), ok: format!("Delete {n} rows"), then: Some(Action::DeleteRows(ids)) });
                    return;
                }
                Action::DeleteColumns(mut cols) if !sure => {
                    // A selection spanning several columns, this one among them: they all go.
                    if let (&[c], Some((_, cs))) = (cols.as_slice(), tab.grid.range()) {
                        if cs.contains(&c) && cs.end() > cs.start() {
                            cols = cs.filter(|k| !tab.grid.hidden.contains(k)).collect();
                        }
                    }
                    let heads = table.st.headers();
                    let names: Vec<String> = cols.iter().take(8).map(|&c| format!("\"{}\"", heads.get(c).map_or("", |s| s.as_str()))).collect();
                    let more = if cols.len() > 8 { format!(" and {} more", cols.len() - 8) } else { String::new() };
                    let (title, ok) = if cols.len() == 1 { (format!("Delete column {} permanently?", names[0]), "Delete column".to_string()) } else { (format!("Delete the {} selected columns?", cols.len()), format!("Delete {} columns", cols.len())) };
                    let sub = if cols.len() == 1 { "Its values go with it; Ctrl+Z brings it back.".into() } else { format!("{}{more}. Ctrl+Z brings them back.", names.join(", ")) };
                    self.dialog = Some(Dialog::Confirm { title, sub, ok, then: Some(Action::DeleteColumns(cols)) });
                    return;
                }
                a => a,
            };
            match a {
                Action::SetCells(ch) => {
                    // One cell typed: no toast (one per keystroke would be noise); a block says what it did.
                    if ch.len() > 1 {
                        let n = grid::fmt_int(ch.len());
                        self.pending.push(Cmd::Toast(if ch.iter().all(|c| c.2.is_empty()) { format!("{n} cells cleared") } else { format!("{n} cells changed") }));
                    }
                    if let Some(c) = crate::tools::fill_example::typed(tab, &ch, ctx) { self.pending.push(c); }
                    tab.apply_changes(&ch, ctx);
                }
                Action::Patch { patch, pasted } => {
                    let n = patch.cells();
                    let fmt = csvfab_engine::edit::fmt_int;
                    // As the changes it stands for would say it (a paste's line, a block's count).
                    let msg = if pasted {
                        Some(if n == 0 { "Nothing changed.".to_string() } else { csvfab_engine::edit::not_saved(&format!("{} cells pasted", fmt(n))) })
                    } else {
                        (n > 1).then(|| format!("{} cells {}", grid::fmt_int(n), if patch.blank() { "cleared" } else { "changed" }))
                    };
                    if let Some(m) = msg { self.pending.push(Cmd::Toast(m)); }
                    if !patch.is_empty() {
                        tab.apply_patch(patch, ctx);
                    }
                }
                Action::Sort(s) => {
                    // Said as the web's: by what, which way, read as what.
                    let heads = table.st.headers();
                    let kind = |c: usize| match tab.kinds.get(c) { Some(csvfab_engine::view::Kind::Number) => "numbers", Some(csvfab_engine::view::Kind::Date) => "dates", _ => "text" };
                    let keys: Vec<String> = s.iter().map(|k| format!("{} {} ({})", heads.get(k.col).map_or("", |h| h.as_str()), if k.desc { "↓" } else { "↑" }, kind(k.col))).collect();
                    let line = if keys.is_empty() { "Sort removed: the rows in the file's order.".to_string() } else { format!("Sorted by {} — the view only: Save keeps the file's order (Rows › Write the sorted order to the file).", keys.join(", then ")) };
                    self.status = Some((tab.uid, line, ctx.input(|i| i.time)));
                    tab.sort = s;
                    tab.refresh(ctx);
                }
                Action::MoveRow { id, target, after } => {
                    let before = tab.table.as_ref().map(|t| t.version());
                    let view = tab.col_view();
                    let moved = tab.table.as_mut().and_then(|t| t.move_row(id, target, after));
                    if let Some((from, to)) = moved {
                        // A sort would put it back: the order is now the file's; Undo brings it back.
                        let sorted = !tab.sort.is_empty();
                        if let Some(v) = before.filter(|_| sorted) { tab.keep_col_view(v, view); }
                        tab.sort.clear();
                        if sorted {
                            let after = tab.col_view();
                            if let Some(v) = tab.table.as_ref().map(|t| t.version()) { tab.keep_col_view(v, after); }
                        }
                        // The same rows and columns: the kinds stand; the view follows the new
                        // order (filtered or sorted), else it is the file's order already.
                        tab.after_edit(false, ctx);
                        if sorted || tab.narrowed() { tab.refresh(ctx); }
                        // The row selected where it landed (under a filter its place is the view's to say).
                        if !tab.filtered() {
                            let last = tab.table.as_ref().map_or(0, |t| t.st.ncols.saturating_sub(1));
                            tab.grid.anchor = Some(grid::Cell { row: to, col: 0 });
                            tab.grid.cursor = Some(grid::Cell { row: to, col: last });
                        }
                        self.pending.push(Cmd::Toast(format!("Row {} moved to {}{} — not written yet, use Save.", grid::fmt_int(from + 1), grid::fmt_int(to + 1), if sorted { " (the sort is off)" } else { "" })));
                    }
                }
                Action::FiltersChanged => tab.refresh(ctx),
                Action::InsertRows { before, count } => {
                    self.pending.push(Cmd::Toast(if count == 1 { "Row inserted".into() } else { format!("{} rows inserted", grid::fmt_int(count)) }));
                    // Before the row shown at `before`, in file order.
                    let len = table.st.len();
                    // Unfiltered and unsorted, the row shown at `before` is the file's: no look for it.
                    let pos = if tab.view.is_none() { before.min(len) } else if before < tab.shown() { file_pos(tab, tab.shown_id(before)) } else { len };
                    let ids = tab.table.as_mut().unwrap().insert_rows(pos, count);
                    if let Some(v) = &mut tab.view {
                        *v = Arc::new(csvfab_engine::table::splice_ids(v, before, &ids));
                    }
                    tab.generation += 1;
                }
                Action::DeleteRows(ids) => {
                    self.pending.push(Cmd::Toast(if ids.len() == 1 { "Row deleted".into() } else { format!("{} rows deleted", grid::fmt_int(ids.len())) }));
                    let mask = csvfab_engine::table::RowMask::new(ids.iter().copied());
                    if !ids.is_empty() { table.delete_mask(&mask); }
                    // The view without them: one pass, a bit per row (a view of millions).
                    if let Some(v) = &mut tab.view { *v = Arc::new(v.iter().copied().filter(|&id| !mask.contains(id)).collect()); }
                    tab.grid.anchor = tab.grid.cursor;
                    tab.grid.clamp(tab.shown());
                    tab.generation += 1;
                }
                Action::InsertColumn(at) if !sure => {
                    self.dialog = Some(Dialog::InsertColumn { at, text: "New_Column".into() });
                    return;
                }
                Action::InsertColumn(at) => {
                    let name = tab.new_col_name.take().unwrap_or_else(|| format!("Column {}", at + 1));
                    self.pending.push(Cmd::Toast(format!("Column \"{name}\" inserted")));
                    let old_n = table.st.ncols;
                    table.insert_column(at, &name);
                    tools::edit_kit::remap_columns(tab, old_n, |c| Some(tools::edit_kit::after_insert(at, 1, c)), ctx);
                }
                Action::DeleteColumns(cols) => {
                    let heads = table.st.headers();
                    self.pending.push(Cmd::Toast(match cols.as_slice() {
                        [c] => format!("Column \"{}\" deleted", heads.get(*c).map_or("", |s| s.as_str())),
                        _ => format!("{} columns deleted", cols.len()),
                    }));
                    let old_n = table.st.ncols;
                    table.delete_columns(&cols);
                    tools::edit_kit::remap_columns(tab, old_n, |c| tools::edit_kit::after_delete(&cols, c), ctx);
                }
                Action::GoToRow { id, clear } => {
                    if clear {
                        tools::rows::clear_filters(tab);
                        tab.refresh(ctx);
                        self.pending_goto = Some((tab.uid, id));
                    } else {
                        Self::select_id(tab, id);
                    }
                }
                Action::RenameColumn(col) => {
                    let text = table.st.headers().get(col).cloned().unwrap_or_default();
                    self.dialog = Some(Dialog::Rename { col, text });
                    return;
                }
                // A value filter on the exact text, as the web's (not the column box: Invert
                // leaves it alone, case and accents count).
                Action::FilterValue { col, value, keep } => {
                    let head = table.st.headers().get(col).cloned().unwrap_or_default();
                    let shown = if value.is_empty() { "(empty)".to_string() } else { value.clone() };
                    if keep {
                        tab.keep_values(col, Some(csvfab_engine::view::Keeps::Values(Arc::new([value].into_iter().collect()))), ctx);
                    } else {
                        let mut set: std::collections::HashSet<String> = tab.filter.hidden.iter().find(|(hc, _)| *hc == col).map(|(_, s)| (**s).clone()).unwrap_or_default();
                        set.insert(value);
                        tab.hide_values(col, set, ctx);
                    }
                    let line = format!("{head}: {} “{shown}” — the column's panel or Clear all filters brings them back.", if keep { "only" } else { "without" });
                    self.status = Some((tab.uid, line, ctx.input(|i| i.time)));
                }
                Action::Copy => match copy_selection(tab, ctx) {
                    Ok(c) => self.pending.push(Cmd::Toast(copied(&c))),
                    Err(job) => self.copying = job,
                },
                Action::Paste(text) => {
                    if let Some(msg) = tools::rows::paste(tab, &text, ctx) { self.pending.push(Cmd::Toast(msg)); }
                }
                Action::RequestPaste => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
                Action::RowCard => self.card.toggle(),
                Action::RowCardRaw => self.card.open_raw(),
                Action::RenameTo(col, text) => {
                    let old = table.st.headers().get(col).cloned().unwrap_or_default();
                    self.pending.push(Cmd::Toast(format!("Column \"{old}\" renamed to \"{text}\"")));
                    tab.rename_column(col, text, ctx);
                }
                Action::RunCommand(name) => {
                    if let Some(c) = tools::commands().into_iter().find(|c| c.name == name && (c.enabled)(tab)) {
                        deferred.push(Box::new(c.run) as Box<dyn FnOnce(&mut Cx)>);
                    }
                }
                Action::ToggleFilters => tab.show_filters = !tab.show_filters,
                Action::ToggleCharts => tab.show_charts = !tab.show_charts,
                Action::ToggleTotals => tab.show_totals = !tab.show_totals,
                // Grid agent stubs, for the lead to route: column tools (split, merge, convert), the row
                // diagnosis (tools::diagnose::open), duplicates (tools::dupes::apply), a tool to open.
                Action::Diagnose(id) => self.pending.push(Cmd::Tool(tools::diagnose::open(tab, id))),
                Action::Dup(act) => deferred.push(Box::new(move |cx: &mut Cx| tools::dupes::apply(cx, act))),
                Action::Tool(t) => self.pending.push(Cmd::Tool(t)),
                // The column tools act on the column under the cursor: put it there, then run the tool's command.
                Action::SplitColumn(c) | Action::MergeColumns(c) | Action::ConvertColumn(c) => {
                    let prefix = match a { Action::SplitColumn(_) => "Split", Action::MergeColumns(_) => "Merge", _ => "Convert" };
                    let row = tab.grid.cursor.map_or(tab.grid.top, |x| x.row);
                    tab.grid.select(grid::Cell { row, col: c }, false);
                    match column_command(prefix) {
                        Some(run) => deferred.push(Box::new(run)),
                        None => self.pending.push(Cmd::Toast(format!("{prefix}: not available yet"))),
                    }
                }
                // Unwrapped above; a Sure inside a Sure is never made.
                Action::Sure(_) => {}
                Action::FitWidth(c) => {
                    let st = table.st.clone();
                    grid::fit_column(tab, &st, ctx, c, &m);
                }
            }
        }
        self.deferred.extend(deferred);
    }

    /// Ctrl+G: row `n` as the gutter numbers it — its place in the file, whatever the
    /// filters and the sort. Hidden by the filters: asks to clear them; past the end or
    /// deleted: says so.
    pub fn go_to_number(&mut self, n: usize, ctx: &egui::Context) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let Some(st) = t.st().cloned() else { return };
        let id = (n + st.header as usize).checked_sub(1).map_or(RowId::MAX, |i| i.min(RowId::MAX as usize) as RowId);
        if n > 0 && Self::select_id(t, id) { return; }
        // A row added here numbers past the file's records (as the gutter shows it).
        let exists = n > 0 && match &st.order { None => (id as usize) < st.base.n, Some(o) => o.contains(&id) };
        if !exists {
            let msg = if n > 0 && (id as usize) < st.base.n { format!("Row {} was deleted", grid::fmt_int(n)) } else { format!("No row {}: the file has {} rows", grid::fmt_int(n), grid::fmt_int(st.len())) };
            self.toast(msg, ctx);
        } else {
            self.dialog = Some(Dialog::Confirm {
                title: format!("Row {} is hidden by the filters", grid::fmt_int(n)),
                sub: "Clear the filters to show it?".into(),
                ok: "Clear the filters".into(),
                then: Some(Action::GoToRow { id, clear: true }),
            });
        }
    }

    /// Puts the cursor on row `id` if it is shown, scrolled into the middle.
    fn select_id(t: &mut Tab, id: RowId) -> bool {
        let Some(row) = t.position_of(id) else { return false };
        t.grid.select(grid::Cell { row, col: t.grid.cursor.map_or(0, |c| c.col) }, false);
        t.grid.set_top(row.saturating_sub(t.grid.page / 2));
        true
    }

    /// The active tab and its table's version.
    fn version_now(&self) -> (u64, u64) {
        self.tabs.get(self.active).map_or((0, 0), |t| (t.uid, t.table.as_ref().map_or(0, |x| x.version())))
    }

    /// A toast's Undo: only while that edit is still the last of its tab (else it would
    /// undo another).
    pub fn undo_from_toast(&mut self, (uid, version): (u64, u64), ctx: &egui::Context) {
        if let Some(i) = self.tabs.iter().position(|t| t.uid == uid && t.table.as_ref().is_some_and(|x| x.version() == version && x.can_undo())) {
            self.active = i;
            self.undo(false, ctx);
        } else {
            self.toast("That edit is no longer the last one: use Ctrl+Z to step back to it.", ctx);
        }
    }

    pub fn undo(&mut self, redo: bool, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let uid = tab.uid;
        let Some(t) = &mut tab.table else { return };
        let before = t.version();
        let r = if redo { t.redo() } else { t.undo() };
        // As the web's status line: what was undone and how many are left.
        let lower = |s: &str| { let mut c = s.chars(); c.next().map_or(String::new(), |f| f.to_lowercase().collect::<String>() + c.as_str()) };
        let line = match (&r, redo) {
            (None, false) => "Nothing to undo.".to_string(),
            (None, true) => "Nothing to redo.".to_string(),
            (Some(_), false) => {
                let what = edit_what::get((uid, before)).map_or("the last edit".to_string(), |w| lower(&w));
                let left = if t.dirty() { format!(" — {} left", grid::fmt_int(t.undo_depth())) } else { " — back to the saved file".into() };
                format!("Undone: {what}{left}.")
            }
            (Some(_), true) => {
                let what = edit_what::get((uid, t.version())).map_or("the edit".to_string(), |w| lower(&w));
                let more = if t.redo_depth() > 0 { format!(" — {} more to redo", grid::fmt_int(t.redo_depth())) } else { String::new() };
                format!("Redone: {what}{more}.")
            }
        };
        self.status = Some((uid, line, now));
        let tab = &mut self.tabs[self.active];
        if let Some(touched) = r {
            let structure = matches!(touched, csvfab_engine::table::Touched::Structure);
            // Columns added, removed or moved: their settings as they were at this version.
            let ncols = tab.st().map_or(0, |s| s.ncols);
            let v = tab.table.as_ref().map_or(0, |t| t.version());
            if let Some(cv) = tab.col_views.get(&v).filter(|cv| structure && cv.filters.len() == ncols).cloned() { tab.set_col_view(cv); }
            tab.after_edit(structure, ctx);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if sc(Modifiers::COMMAND, Key::O) { self.pick_files(ctx); }
        if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) { self.save_as(ctx); } else if sc(Modifiers::COMMAND, Key::S) { self.save(None, false, ctx); }
        if sc(Modifiers::COMMAND, Key::W) { self.close_tab(self.active, false); }
        if sc(Modifiers::COMMAND, Key::Q) { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        // In a text field, Ctrl+Z / Ctrl+Y are the field's own (its typing), not the table's.
        let in_field = ctx.memory(|m| m.focused()).is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if !in_field {
            if sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) || sc(Modifiers::COMMAND, Key::Y) { self.undo(true, ctx); } else if sc(Modifiers::COMMAND, Key::Z) { self.undo(false, ctx); }
        }
        if sc(Modifiers::COMMAND, Key::G) { self.dialog = Some(Dialog::GoTo(String::new())); }
        if sc(Modifiers::COMMAND, Key::P) || sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::P) { self.palette = Some(Default::default()); }
        if sc(Modifiers::NONE, Key::F1) { self.dialog = Some(Dialog::Shortcuts); }
        // Outside any field, menu or edit: "?" the shortcuts (no cell selected, where it
        // would start typing one), Escape closes the find bar, then drops the selection.
        let free = ctx.memory(|m| m.focused().is_none()) && !egui::Popup::is_any_open(ctx);
        if let Some(t) = self.tabs.get_mut(self.active).filter(|t| free && t.grid.editing.is_none()) {
            if t.grid.cursor.is_none() && ctx.input_mut(|i| {
                let k = i.events.iter().position(|e| matches!(e, egui::Event::Text(s) if s == "?"));
                k.map(|k| i.events.remove(k)).is_some()
            }) {
                self.dialog = Some(Dialog::Shortcuts);
            } else if !self.focus_last && ctx.input(|i| i.key_pressed(Key::Escape)) {
                if t.find.open { t.find.open = false; } else { t.grid.cursor = None; t.grid.anchor = None; }
            }
        } else if self.tabs.is_empty() && free && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(s) if s == "?"))) {
            self.dialog = Some(Dialog::Shortcuts);
        }
        if sc(Modifiers::ALT, Key::ArrowRight) && !self.tabs.is_empty() { self.active = (self.active + 1) % self.tabs.len(); }
        if sc(Modifiers::ALT, Key::ArrowLeft) && !self.tabs.is_empty() { self.active = (self.active + self.tabs.len() - 1) % self.tabs.len(); }
        // Alt+1…9: that tab.
        for (i, k) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
            if i < self.tabs.len() && sc(Modifiers::ALT, k) { self.active = i; }
        }
        // Tool shortcuts (Ctrl+H…), from the registry: their keys read once.
        static KEYS: OnceLock<Vec<(Modifiers, Key, &'static tools::Command)>> = OnceLock::new();
        let keys = KEYS.get_or_init(|| tools::commands().iter().filter_map(|c| c.keys.and_then(parse_keys).map(|k| (k.0, k.1, c))).collect());
        for &(m, k, c) in keys {
            if self.tabs.get(self.active).is_some_and(|t| (c.enabled)(t)) && sc(m, k) {
                self.run_command(c.run, ctx);
            }
        }
    }

    fn carry_out(&mut self, ctx: &egui::Context) {
        // A JSON file the import chose to open as raw text.
        for p in tools::import::take_text_opens() {
            self.tabs.push(Tab::open(p, OpenOptions { delim: Some(0), ..Default::default() }, ctx));
            self.active = self.tabs.len() - 1;
        }
        // Screenshot hook: `cmd:<command name>` runs a command once the file is shown.
        if std::env::var_os("CSVFAB_SHOT").is_some() && self.tools.is_empty() && self.tabs.get(self.active).is_some_and(|t| t.table.is_some() && !t.busy()) {
            if let Some(name) = crate::shot::take("cmd") {
                if let Some(c) = tools::commands().into_iter().find(|c| c.name == name) { self.run_command(c.run, ctx); }
                if name == "Save as…" { self.save_as(ctx); }
            }
        }
        for cmd in std::mem::take(&mut self.pending) {
            match cmd {
                Cmd::Open(p) => self.open(p, ctx),
                Cmd::OpenBehind(p) => {
                    let keep = self.active;
                    self.open(p, ctx);
                    self.active = keep.min(self.tabs.len().saturating_sub(1));
                }
                Cmd::OpenBytes { name, bytes } => {
                    self.tabs.push(Tab::memory(name, bytes, ctx, &self.m));
                    self.active = self.tabs.len() - 1;
                }
                Cmd::Toast(t) => self.toast(t, ctx),
                Cmd::Wrote(uid, read, ours, sample) => {
                    self.written.insert(uid, (read, ours, sample));
                    self.told.remove(&uid);
                }
                Cmd::Tool(t) => self.open_tool(t),
            }
        }
    }

    fn show_tools(&mut self, ctx: &egui::Context) {
        self.note_open();
        let (p, m) = (&self.theme.p, &self.m);
        let mut out = Vec::new();
        let mut tools = std::mem::take(&mut self.tools);
        let mut scratch: Option<Tab> = None;
        tools.retain_mut(|(uid, tool)| {
            // uid 0: a tool bound to no tab (an import before its tab exists) gets a blank one.
            let tab = if *uid == 0 {
                scratch.get_or_insert_with(|| Tab::blank())
            } else {
                match self.tabs.iter_mut().find(|t| t.uid == *uid) { Some(t) => t, None => return false }
            };
            tool.show(&mut Cx { ctx, tab, p, m, out: &mut out })
        });
        tools.append(&mut self.tools);
        self.tools = tools;
        self.pending.extend(out);
    }

    /// One frame. False: left empty, its scale was not the window's yet (see `expect_scale`).
    pub fn frame(&mut self, ui: &mut egui::Ui) -> bool {
        laps_start();
        let r = self.frame_inner(ui);
        laps_end();
        r
    }

    fn frame_inner(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        // CSVFAB_FPS=1: frames drawn per second, on stderr (an idle window should draw none).
        if env_set(&FPS, "CSVFAB_FPS") {
            use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
            static N: AtomicU64 = AtomicU64::new(0);
            static T: AtomicU64 = AtomicU64::new(0);
            let now = ctx.input(|i| i.time) as u64;
            N.fetch_add(1, Relaxed);
            if T.swap(now, Relaxed) != now { eprintln!("fps {} — repaint asked by {:?}", N.swap(0, Relaxed), ctx.repaint_causes()); }
        }
        if provisional(&ctx) {
            ctx.request_repaint();
            return false;
        }
        let native = ctx.input(|i| i.viewport().native_pixels_per_point);
        if native.is_some() && native != self.config.scale && EXPECTED_SCALE.get().is_some() {
            self.config.scale = native;
            self.config.save();
        }
        // Files dropped on the window, picked in the dialog, or handed by a second launch.
        self.sync_bands(&ctx);
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| { let p = f.path(); (!p.as_os_str().is_empty()).then(|| p.to_path_buf()) }).collect());
        for p in dropped { self.open(p, &ctx); }
        crate::fieldmenu::detect(self, &ctx);
        if let Some(files) = self.picking.as_mut().and_then(|j| j.poll()) {
            self.picking = None;
            for f in files { self.open(f, &ctx); }
        }
        let handed: Vec<Vec<PathBuf>> = self.incoming.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for batch in handed {
            crate::profile::mark("paths handed over");
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            for p in batch { self.open(p, &ctx); }
        }
        lap("input");
        let m = self.m.clone();
        for t in &mut self.tabs { t.poll(&ctx, &m); }
        lap("tab poll");
        self.poll_save(&ctx);
        self.poll_disk_job();
        self.check_disk(&ctx);
        tools::import::NEXT_DELIM.store(self.next_opts.delim.filter(|&d| d != 0).unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        if let Some(n) = self.tabs.get_mut(self.active).and_then(|t| t.grid.notice.take()) { self.toast(n, &ctx); }
        if let Some((uid, id)) = self.pending_goto {
            match self.tabs.iter_mut().find(|t| t.uid == uid) {
                Some(t) if t.busy() => {}
                Some(t) => { Self::select_id(t, id); self.pending_goto = None; }
                None => self.pending_goto = None,
            }
        }
        if let Some(r) = self.copying.as_mut().and_then(|j| j.poll()) {
            self.copying = None;
            if let Some(mut c) = r {
                ctx.copy_text(std::mem::take(&mut c.text));
                self.toast(copied(&c), &ctx);
            }
        }
        if self.dialog.is_none() && self.palette.is_none() { self.shortcuts(&ctx); }
        if env_set(&SHOT, "CSVFAB_SHOT") && self.tabs.get(self.active).is_some_and(|t| t.table.is_some()) {
            self.run_command(crate::tools::analysis_kit::shots, &ctx);
        }

        // Quit with unsaved changes: ask first.
        if ctx.input(|i| i.viewport().close_requested()) {
            if !self.quit_ok && self.tabs.iter().any(|t| t.dirty()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.dialog = Some(Dialog::QuitDirty);
            } else {
                // A quit asked for: the edits were saved or let go, their copies too.
                self.recovery.clear();
            }
        }
        let title = match self.tabs.get(self.active) {
            Some(t) => format!("{}{} — csvfab", if t.dirty() { "● " } else { "" }, t.name),
            None => "csvfab".into(),
        };
        // The window's maximized state, remembered for the next start (written when it changes).
        // Not in the first second: the compositor has not told the window its state yet.
        if std::env::var_os("CSVFAB_SHOT").is_none() && ctx.input(|i| i.time) > 1.5 {
            if let Some(m) = ctx.input(|i| i.viewport().maximized) {
                if self.config.maximized != Some(m) {
                    self.config.maximized = Some(m);
                    self.config.save();
                }
                // And its size when not maximized (as the browser window kept it), once the
                // pointer is up (not at every step of a resize).
                let size = ctx.input(|i| i.viewport().inner_rect).map(|r| (r.width().round(), r.height().round()));
                if let Some(s) = size.filter(|s| !m && s.0 >= 400.0 && s.1 >= 300.0 && self.config.size != Some(*s) && !ctx.input(|i| i.pointer.any_down())) {
                    self.config.size = Some(s);
                    self.config.save();
                }
            }
        }
        // Only when it changes: every viewport command asks egui for another frame.
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        lap("shortcuts");
        crate::chrome::tab_bar(self, ui);
        lap("tab bar");
        crate::chrome::toolbar(self, ui);
        lap("toolbar");
        crate::tools::find::bar(self, ui);
        lap("find bar");
        crate::chrome::status_bar(self, ui);
        lap("status bar");
        let mut actions = Vec::new();
        let p = &self.theme.p;
        // The row card, docked at the right of the grid (before the central panel, which takes the rest).
        if env_set(&SHOT, "CSVFAB_SHOT") {
            if crate::shot::take("card").is_some() { self.card.open = true; }
            if crate::shot::take("about").is_some() { self.dialog = Some(Dialog::About); }
            if crate::shot::take("news").is_some() { self.dialog = Some(Dialog::WhatsNew); }
            if let Some(w) = crate::shot::take("ondisk") {
                let what = match w.as_str() { "cut" => OnDisk::Cut, "gone" => OnDisk::Gone, _ => OnDisk::Changed };
                self.dialog = Some(Dialog::OnDisk { tab: self.active, what });
            }
            if crate::shot::take("replace").is_some() { self.dialog = Some(Dialog::ReplaceChanged(self.active)); }
            if crate::shot::take("recover").is_some() {
                let when = std::time::SystemTime::now() - std::time::Duration::from_secs(1500);
                let l = |n: &str, o: &str| crate::recovery::Left { data: PathBuf::new(), name: n.into(), original: Some(PathBuf::from(o)), when };
                self.dialog = Some(Dialog::Recover(vec![l("Companies.csv", "/home/you/Documents/Companies.csv"), l("Contacts.csv", "/home/you/Documents/Contacts.csv")]));
            }
            if crate::shot::take("font").is_some() { self.font_picker = Some(crate::fontpick::Picker::new(&ctx)); }
        }
        let card_shown = self.card.open && self.tabs.get(self.active).is_some_and(|t| t.table.is_some());
        if card_shown {
            let tab = &mut self.tabs[self.active];
            actions = crate::rowcard::panel(&mut self.card, ui, tab, p, &self.m, &mut self.pending);
        } else {
            crate::rowcard::closed(&ctx);
        }
        // One central panel: a second one in the same frame would get no room.
        if self.tabs.is_empty() {
            egui::CentralPanel::no_frame().show(ui, |ui| crate::chrome::welcome(self, ui));
        } else {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                let Some(tab) = self.tabs.get_mut(self.active) else { return };
                if tab.table.is_none() {
                    if let Some(e) = &tab.error {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new(format!("Could not open {}: {e}", tab.name)).color(p.danger_txt)));
                    } else {
                        let f = tab.load_progress().unwrap_or(0.0);
                        ui.add_space(ui.available_height() / 3.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(format!("Reading {}…", tab.name)).size(15.0).color(p.txt_hi));
                            ui.add(egui::ProgressBar::new(f.min(1.0)).desired_width(360.0));
                        });
                    }
                    return;
                }
                crate::tools::fill_example::keys(tab, &ctx);
                actions.extend(grid::show(ui, tab, p, &m));
            });
        }
        lap("central panel (grid rest)");
        self.apply(actions, &ctx);
        // What the grid said this frame (an edit's toast carries its Undo).
        if let Some(n) = self.tabs.get_mut(self.active).and_then(|t| t.grid.notice.take()) { self.toast(n, &ctx); }
        for run in std::mem::take(&mut self.deferred) { self.run_with(run, &ctx); }
        lap("actions");
        self.show_tools(&ctx);
        lap("tools");
        crate::chrome::palette(self, &ctx);
        self.dialogs(&ctx);
        crate::fontpick::show(self, &ctx);
        crate::fieldmenu::show(self, &ctx);
        crate::chrome::drop_overlay(self, &ctx);
        crate::chrome::toasts(self, &ctx);
        lap("palette, dialogs, toasts");
        self.carry_out(&ctx);
        self.seen = self.version_now();
        self.focus_last = ctx.memory(|m| m.focused().is_some());
        // A job that panicked was caught (job.rs): it stopped as on a failure; say so.
        for m in crate::job::take_failures() {
            self.toast(format!("Something went wrong in a background task and it stopped ({m}). Your file and edits are intact."), &ctx);
        }
        self.recovery.tick(&self.tabs, &ctx);
        if !self.recover_checked && self.dialog.is_none() {
            self.recover_checked = true;
            let left = self.recovery.leftovers();
            if !left.is_empty() { self.dialog = Some(Dialog::Recover(left)); }
        }
        lap("carry out");
        if self.tabs.iter().any(|t| t.busy()) || self.saving.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        true
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialog else { return };
        let (p, m) = (&self.theme.p, &self.m);
        let mut close = false;
        let mut after: Option<Box<dyn FnOnce(&mut App)>> = None;
        let modal = crate::widgets::modal(ctx, egui::Id::unique("dialog"), p, theme::dialog_frame(p, m), |ui| {
            ui.set_width(396.0);
            let head = |ui: &mut egui::Ui, t: &str| { ui.label(RichText::new(t).font(grid::semibold(14.5)).color(p.txt_hi)); ui.add_space(8.0); };
            let buttons = |ui: &mut egui::Ui, ok: &str, danger: bool| -> (bool, bool) {
                ui.add_space(20.0);
                let mut r = (false, false);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let fill = if danger { p.danger } else { p.prim };
                    let fg = if danger { p.on_danger } else { p.on_prim };
                    r.0 = ui.add(egui::Button::new(RichText::new(ok).color(fg)).fill(fill)).clicked();
                    r.1 = ui.button("Cancel").clicked();
                });
                r
            };
            // Up to three: (`ok` in colour, unless empty; `extra`; `cancel`) clicked.
            let buttons3 = |ui: &mut egui::Ui, ok: &str, danger: bool, extra: Option<&str>, cancel: &str| -> (bool, bool, bool) {
                ui.add_space(20.0);
                let mut r = (false, false, false);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !ok.is_empty() {
                        let (fill, fg) = if danger { (p.danger, p.on_danger) } else { (p.prim, p.on_prim) };
                        r.0 = ui.add(egui::Button::new(RichText::new(ok).color(fg)).fill(fill)).clicked();
                    }
                    if let Some(x) = extra { r.1 = ui.button(x).clicked(); }
                    r.2 = ui.button(cancel).clicked();
                });
                r
            };
            match d {
                Dialog::GoTo(text) => {
                    head(ui, "Go to row");
                    let records = self.tabs.get(self.active).and_then(|t| t.st()).map_or(0, |st| st.len());
                    let hint = format!("Its number in the file: 1 to {}", grid::fmt_int(records));
                    ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(f32::INFINITY)).request_focus();
                    let (ok, cancel) = buttons(ui, "Go", false);
                    if ok || ui.input(|i| i.key_pressed(Key::Enter)) {
                        let ctx = ctx.clone();
                        // Digits grouped as people write them: "1 234", "1,234", "1.234".
                        match text.trim().replace(['\u{202f}', ' ', '\u{a0}', ',', '.', '\''], "").parse::<usize>() {
                            Ok(n) => after = Some(Box::new(move |app: &mut App| app.go_to_number(n, &ctx))),
                            Err(_) if text.trim().is_empty() => {}
                            Err(_) => {
                                let msg = format!("“{}” is not a row number: type its number in the file, 1 to {}.", text.trim(), grid::fmt_int(records));
                                after = Some(Box::new(move |app: &mut App| app.toast(msg, &ctx)));
                            }
                        }
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::InsertColumn { at, text } => {
                    head(ui, "Insert a column");
                    ui.add(egui::TextEdit::singleline(text).hint_text("Its name").desired_width(f32::INFINITY)).request_focus();
                    let (ok, cancel) = buttons(ui, "Insert", false);
                    if (ok || ui.input(|i| i.key_pressed(Key::Enter))) && !text.trim().is_empty() {
                        let (at, name, ctx) = (*at, text.trim().to_string(), ctx.clone());
                        after = Some(Box::new(move |a: &mut App| {
                            if let Some(t) = a.tab() { t.new_col_name = Some(name); }
                            a.apply(vec![Action::Sure(Box::new(Action::InsertColumn(at)))], &ctx);
                        }));
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::Rename { col, text } => {
                    head(ui, "Rename the column");
                    let titled = self.tabs.get(self.active).and_then(|t| t.st()).is_none_or(|st| st.header);
                    if !titled {
                        // Line 1 is data here: renaming must not turn it into titles.
                        ui.label(RichText::new("This file has no title line: one is added above the first row, the other columns named Column 1, Column 2…").color(p.txt_mut));
                        ui.add_space(6.0);
                    }
                    ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY)).request_focus();
                    let (ok, cancel) = buttons(ui, "Rename", false);
                    if (ok || ui.input(|i| i.key_pressed(Key::Enter))) && !text.trim().is_empty() {
                        let (col, text) = (*col, text.clone());
                        let ctx2 = ctx.clone();
                        after = Some(Box::new(move |app: &mut App| {
                            if let Some(t) = app.tab() { t.rename_column(col, text, &ctx2); }
                        }));
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::CloseDirty(i) => {
                    head(ui, "Close without saving?");
                    // As the web's: the file and how many edits go.
                    let (name, n) = self.tabs.get(*i).map_or((String::new(), 0), |t| (t.name.clone(), t.table.as_ref().map_or(0, |x| x.pending_edits())));
                    ui.label(RichText::new(format!("\"{name}\" has {} unsaved edit{}. Closing drops {}.", grid::fmt_int(n), if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "them" })).color(p.txt_mut));
                    let (ok, cancel) = buttons(ui, "Close without saving", true);
                    if ok { let i = *i; after = Some(Box::new(move |a: &mut App| a.close_tab(i, true))); close = true; }
                    close |= cancel;
                }
                Dialog::Recover(left) => {
                    head(ui, if left.len() == 1 { "Unsaved edits found" } else { "Unsaved edits found in several files" });
                    ui.label(RichText::new("csvfab stopped before these edits were saved. Restore them as new tabs (Save as… then writes them where you want), or discard them.").color(p.txt_mut));
                    ui.add_space(10.0);
                    for l in left.iter() {
                        let ago = l.when.elapsed().map_or(String::new(), |d| match d.as_secs() {
                            s if s < 3600 => format!("{} min ago", s / 60),
                            s if s < 86400 => format!("{} h ago", s / 3600),
                            s => format!("{} d ago", s / 86400),
                        });
                        ui.label(RichText::new(format!("{} · {ago}", l.name)).color(p.txt_hi));
                        if let Some(o) = &l.original {
                            ui.label(RichText::new(o.display().to_string()).font(egui::FontId::proportional(11.5)).color(p.txt_dim));
                        }
                        ui.add_space(4.0);
                    }
                    let (restore, discard, later) = buttons3(ui, "Restore", false, Some("Discard"), "Later");
                    if restore || discard {
                        let (left, ctx) = (left.clone(), ctx.clone());
                        after = Some(Box::new(move |a: &mut App| {
                            for l in &left {
                                if restore {
                                    let Ok(bytes) = std::fs::read(&l.data) else { continue };
                                    match crate::recovery::restore(l, bytes, &ctx, &a.m) {
                                        Ok(t) => {
                                            a.tabs.push(t);
                                            a.active = a.tabs.len() - 1;
                                        }
                                        Err(e) => a.toast(format!("{} | {e}", l.name), &ctx),
                                    }
                                }
                                crate::recovery::remove(l);
                            }
                        }));
                        close = true;
                    }
                    close |= later;
                }
                Dialog::QuitDirty => {
                    head(ui, "Quit without saving?");
                    let dirty: Vec<&Tab> = self.tabs.iter().filter(|t| t.dirty()).collect();
                    let text = match dirty.as_slice() {
                        [t] => { let n = t.table.as_ref().map_or(0, |x| x.pending_edits()); format!("\"{}\" has {} unsaved edit{}. Quitting drops {}.", t.name, grid::fmt_int(n), if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "them" }) }
                        d => format!("{} files have unsaved edits ({}). Quitting drops them.", d.len(), d.iter().map(|t| t.name.as_str()).take(4).collect::<Vec<_>>().join(", ") + if d.len() > 4 { "…" } else { "" }),
                    };
                    ui.label(RichText::new(text).color(p.txt_mut));
                    let (ok, cancel) = buttons(ui, "Quit without saving", true);
                    if ok {
                        after = Some(Box::new(|a: &mut App| a.quit_ok = true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::ReplaceChanged(i) => {
                    head(ui, "The file changed on disk");
                    ui.label(RichText::new("Another program changed this file since it was opened. Overwrite it with your version (theirs is kept as a dated .bak beside it), or reload it (your edits are dropped)?").color(p.txt_mut));
                    let (ok, reload, cancel) = buttons3(ui, "Overwrite", true, Some("Reload"), "Cancel");
                    if ok {
                        let (i, ctx) = (*i, ctx.clone());
                        // Their version is kept too: a new dated .bak before it is overwritten.
                        after = Some(Box::new(move |a: &mut App| {
                            a.active = i;
                            if let Some(t) = a.tabs.get_mut(i) { t.backed_up = false; }
                            a.save(None, true, &ctx);
                        }));
                        close = true;
                    }
                    if reload {
                        let (i, ctx) = (*i, ctx.clone());
                        after = Some(Box::new(move |a: &mut App| a.reload_tab(i, &ctx)));
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::OnDisk { tab, what } => {
                    let name = self.tabs.get(*tab).map_or(String::new(), |t| t.name.clone());
                    let dirty = self.tabs.get(*tab).is_some_and(|t| t.dirty());
                    let (title, text, ok, keep) = match what {
                        OnDisk::Changed => (
                            format!("{name} changed on disk"),
                            if dirty { "Another program changed this file while you were editing it. Reload it to see its new version (your edits are dropped), or keep yours: saving will ask before overwriting." } else { "Another program changed this file. Reload it to see its new version." },
                            "Reload",
                            if dirty { "Keep mine" } else { "Keep this view" },
                        ),
                        OnDisk::Cut => (
                            format!("{name} was cut short by another program"),
                            "Part of what was read is no longer in the file: it shows as blank here and cannot be written back. Reload to see the file as it is now. Review changes… lists your edits before they go.",
                            "Reload",
                            "Later",
                        ),
                        OnDisk::Gone => (
                            format!("{name} was deleted or moved"),
                            "Another program removed this file. Its rows stay here: Save writes it again where it was.",
                            "",
                            "OK",
                        ),
                    };
                    head(ui, &title);
                    ui.label(RichText::new(text).color(p.txt_mut));
                    let (reload, _, keep_hit) = buttons3(ui, ok, dirty, None, keep);
                    if reload {
                        let (i, ctx) = (*tab, ctx.clone());
                        after = Some(Box::new(move |a: &mut App| a.reload_tab(i, &ctx)));
                        close = true;
                    }
                    close |= keep_hit;
                }
                Dialog::Lossy { tab, n, c, force } => {
                    let enc = self.tabs.get(*tab).and_then(|t| t.st()).map_or("This encoding", |st| st.base.enc.label());
                    head(ui, &format!("{} cells contain characters {enc} cannot represent (e.g. \"{c}\")", grid::fmt_int(*n)));
                    ui.label(RichText::new("They will be written as \"?\". Save as… keeps them, in UTF-8.").color(p.txt_mut));
                    let (ok, cancel) = buttons(ui, "Write them as \"?\"", true);
                    if ok {
                        let (i, force, ctx) = (*tab, *force, ctx.clone());
                        after = Some(Box::new(move |a: &mut App| { a.active = i; a.save_checked(None, force, true, &ctx); }));
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::Reread { tab, enc, delim } => {
                    let how = match (*enc, *delim) {
                        (Some(Some(e)), _) => format!("in {}", e.label()),
                        (Some(None), _) => "with its encoding detected".into(),
                        (_, Some(Some(0))) => "as raw text".into(),
                        (_, Some(Some(b'\t'))) => "with tabs as the delimiter".into(),
                        (_, Some(Some(d))) => format!("with \"{}\" as the delimiter", d as char),
                        (_, Some(None)) => "with its delimiter detected".into(),
                        _ => String::new(),
                    };
                    head(ui, &format!("Read the file again {how}?"));
                    ui.label(RichText::new("The file is read from disk again: the edits not saved yet are lost.").color(p.txt_mut));
                    let (ok, cancel) = buttons(ui, "Discard the edits and read again", true);
                    if ok {
                        let (i, enc, delim, ctx) = (*tab, *enc, *delim, ctx.clone());
                        after = Some(Box::new(move |a: &mut App| {
                            if let Some(t) = a.tabs.get_mut(i) { t.reopen_with(enc, delim, &ctx); }
                        }));
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::Confirm { title, sub, ok, then } => {
                    head(ui, title.as_str());
                    ui.label(RichText::new(sub.as_str()).color(p.txt_mut));
                    let (yes, cancel) = buttons(ui, ok.as_str(), true);
                    // Enter confirms, as the web's dialogs.
                    if yes || ui.input(|i| i.key_pressed(Key::Enter)) {
                        if let Some(act) = then.take() {
                            let ctx = ctx.clone();
                            after = Some(Box::new(move |a: &mut App| a.apply(vec![Action::Sure(Box::new(act))], &ctx)));
                        }
                        close = true;
                    }
                    close |= cancel;
                }
                Dialog::Message(msg) => {
                    head(ui, msg.as_str());
                    ui.add_space(12.0);
                    if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) { close = true; }
                }
                Dialog::Shortcuts => {
                    head(ui, "Keyboard shortcuts");
                    ui.set_width(560.0);
                    egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                        egui::Grid::new("keys").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                            for (k, what) in crate::chrome::SHORTCUTS {
                                ui.label(RichText::new(*k).monospace().color(p.accent_2));
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                        ui.add_space(12.0);
                        ui.label(RichText::new("Columns, with the mouse").font(grid::semibold(13.0)).color(p.txt_hi));
                        ui.add_space(6.0);
                        egui::Grid::new("mouse").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                            for (k, what) in crate::chrome::MOUSE {
                                ui.label(RichText::new(*k).color(p.accent_2));
                                ui.label(*what);
                                ui.end_row();
                            }
                        });
                    });
                    ui.add_space(12.0);
                    if ui.button("Close").clicked() { close = true; }
                }
                Dialog::About => match crate::about::about(ui, p) {
                    Some(crate::about::Pick::News) => after = Some(Box::new(|app: &mut App| app.dialog = Some(Dialog::WhatsNew))),
                    Some(_) => close = true,
                    None => {}
                },
                Dialog::WhatsNew => {
                    ui.set_width(520.0);
                    match crate::about::news(ui, p) {
                        Some(crate::about::Pick::About) => after = Some(Box::new(|app: &mut App| app.dialog = Some(Dialog::About))),
                        Some(_) => close = true,
                        None => {}
                    }
                }
            }
        });
        if modal.should_close() { close = true; }
        if close {
            self.dialog = None;
            if let Some(f) = after { f(self); }
        }
    }
}

/// The tool command of the Columns group whose name starts with `prefix` ("Split", "Merge", "Convert").
fn column_command(prefix: &str) -> Option<fn(&mut Cx)> {
    tools::commands().into_iter().find(|c| c.name.starts_with(prefix) && c.group == "Columns").map(|c| c.run)
}

/// "Ctrl+Shift+H" → the shortcut.
fn parse_keys(s: &str) -> Option<(Modifiers, Key)> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part {
            "Ctrl" | "Cmd" => m |= Modifiers::COMMAND,
            "Shift" => m |= Modifiers::SHIFT,
            "Alt" => m |= Modifiers::ALT,
            k => key = Key::from_name(k),
        }
    }
    Some((m, key?))
}

/// A row's position in file order (data rows).
fn file_pos(tab: &Tab, id: RowId) -> usize {
    let st = &tab.table.as_ref().unwrap().st;
    match &st.order {
        None => (id as usize).saturating_sub(st.header as usize),
        Some(o) => o.iter().position(|&x| x == id).unwrap_or(o.len()).saturating_sub(st.header as usize),
    }
}

/// The selection as tab-separated values, as Excel and Google Sheets paste them. A big
/// one (whole columns of a big file) is built in a job: the text goes to the clipboard
/// when it is ready, the window never waits for it.
/// The copy's toast: the engine's rows and columns (a cell holding a line break is one cell).
fn copied(c: &csvfab_engine::edit::clipboard::Copied) -> String {
    if c.rows * c.cols == 1 { "Cell copied.".to_string() } else { format!("{} × {} cells copied.", grid::fmt_int(c.rows), grid::fmt_int(c.cols)) }
}

/// Ok: copied at once (a small selection); Err: being built in a job (None: nothing to copy).
fn copy_selection(tab: &Tab, ctx: &egui::Context) -> Result<csvfab_engine::edit::clipboard::Copied, Option<Job<Option<csvfab_engine::edit::clipboard::Copied>>>> {
    let (Some(t), Some((rows, cols))) = (&tab.table, tab.grid.range()) else { return Err(None) };
    let vis = tab.grid.shown_cols(t.st.ncols);
    let sel = csvfab_engine::edit::series::Rect { r0: *rows.start(), r1: *rows.end(), c0: *cols.start(), c1: *cols.end() };
    let (st, view) = (t.st.clone(), tab.view.clone());
    let copy = move |cancel: &std::sync::atomic::AtomicBool| {
        // The rows of the selection only (not a copy of every id shown).
        let ids: Vec<RowId> = (sel.r0..=sel.r1).map(|p| view.as_ref().map_or_else(|| st.data_id(p), |v| v[p])).collect();
        let sel = csvfab_engine::edit::series::Rect { r0: 0, r1: sel.r1 - sel.r0, ..sel };
        csvfab_engine::edit::clipboard::copy(&st, &ids, &vis, sel, cancel)
    };
    if (sel.r1 - sel.r0 + 1) * (sel.c1 - sel.c0 + 1) <= 50_000 {
        let Some(mut c) = copy(&std::sync::atomic::AtomicBool::new(false)) else { return Err(None) };
        ctx.copy_text(std::mem::take(&mut c.text));
        return Ok(c);
    }
    Err(Some(Job::spawn(ctx, move |cancel, _| copy(cancel))))
}
