//! One open file: its table, the rows shown, and the background jobs that
//! read it and compute the view. The UI thread never waits on a job: it
//! polls their channels each frame and is woken by `request_repaint`.

use crate::job::Job;
use csvfab_engine::base::OpenOptions;
use csvfab_engine::look::{self, Pills, Rich};
use csvfab_engine::view::{self, FilterSpec, Kind, SortKey};
use csvfab_engine::{Base, Enc, RowId, State, Table};
use eframe::egui;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A base read, made into a table with its columns analysed (kinds, reading
/// aids): all of it on the reading thread, none on the UI's.
pub struct Ready {
    table: Table,
    kinds: Vec<Kind>,
    rich: Vec<Option<Rich>>,
    pills: Vec<Option<Pills>>,
    /// A save's file, its rows where the table had them (record k is row k): the rows shown
    /// stay what they were.
    same_rows: bool,
}

impl Ready {
    /// On a background thread: `header` is the table's when it is read again.
    pub fn new(base: Base, header: Option<bool>) -> Ready {
        Ready::of(Table::new(Arc::new(base), header))
    }

    fn of(table: Table) -> Ready {
        let (kinds, rich, pills) = looks(&table.st);
        Ready { table, kinds, rich, pills, same_rows: false }
    }

    /// The file a save of `st` wrote (`save::save_indexed`), on a background thread. When every
    /// row kept its place, the column caches of the columns no edit touched carry over: the
    /// filters, sorts and charts on them are not read from the file again.
    pub fn saved(st: &State, base: Base) -> Ready {
        let same_rows = st.order.is_none() && base.n == st.base.n;
        let r = Ready { same_rows, ..Ready::new(base, Some(st.header)) };
        if same_rows { csvfab_engine::colcache::carry(st, &r.table.st.base); }
        r
    }
}

/// Column kinds and the grid's reading aids, from the first rows.
fn looks(st: &State) -> (Vec<Kind>, Vec<Option<Rich>>, Vec<Option<Pills>>) {
    let kinds = view::detect_kinds(st);
    let rich = look::rich_cols(st, &kinds);
    let pills = look::pills_from_rows(st, &kinds, &rich);
    (kinds, rich, pills)
}

/// What a file read sends, in this order: the first records (a big file
/// shown for the first time), then the whole file — or why it could not be read.
enum Load {
    Head(Box<Ready>),
    Full(Box<Ready>),
    Failed(String),
}

/// A file being read on its own thread: the first rows within milliseconds,
/// the whole index after.
struct Loader {
    rx: Receiver<Load>,
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    total: u64,
    started: Instant,
}

impl Loader {
    /// Reads `path`. `from`: the bytes already held (a new encoding or delimiter:
    /// no re-read when possible); `header`: the table's, when it is read again;
    /// `head`: send the first records first (nothing is shown yet); `journal`: edits to
    /// replay over the file read (crash recovery: `csvfab_engine::journal`).
    fn spawn(path: PathBuf, opts: OpenOptions, from: Option<Arc<Base>>, header: Option<bool>, head: bool, journal: Option<Vec<u8>>, ctx: &egui::Context) -> Loader {
        let (tx, rx) = channel();
        let progress = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (p, c, ctx) = (progress.clone(), cancel.clone(), ctx.clone());
        std::thread::Builder::new().name("csvfab-read".into()).spawn(move || {
            let send = |m: Load| {
                let _ = tx.send(m);
                ctx.request_repaint();
            };
            let read = || -> Result<Load, String> {
                if let Some(j) = journal {
                    let base = Arc::new(Base::open(&path, opts, Some(&p)).map_err(|e| e.to_string())?);
                    let t = csvfab_engine::journal::replay(base, &j).map_err(|e| format!("The unsaved edits could not be restored: {e}."))?;
                    return Ok(Load::Full(Box::new(Ready::of(t))));
                }
                if let Some(b) = from.and_then(|b| b.reopen(opts, Some(&p))) {
                    return Ok(Load::Full(Box::new(Ready::new(b, header))));
                }
                if !head {
                    return Ok(Load::Full(Box::new(Ready::new(Base::open(&path, opts, Some(&p)).map_err(|e| e.to_string())?, header))));
                }
                let first = Base::open_head(&path, opts).map_err(|e| e.to_string())?;
                if !first.partial {
                    return Ok(Load::Full(Box::new(Ready::new(first, header))));
                }
                let first = Ready::new(first, header);
                let base = first.table.st.base.clone();
                let header = Some(first.table.st.header);
                send(Load::Head(Box::new(first)));
                if c.load(Ordering::Relaxed) { return Err(String::new()); }
                Ok(Load::Full(Box::new(Ready::new(base.complete(opts, Some(&p)), header))))
            };
            match crate::job::guarded(read).unwrap_or_else(|m| Err(format!("internal error: {m}"))) {
                Ok(m) => send(m),
                Err(e) if !e.is_empty() => send(Load::Failed(e)),
                Err(_) => {}
            }
        }).expect("thread");
        Loader { rx, progress, cancel, total, started: Instant::now() }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Rows a tool asked to show (an Insights card, the duplicates, a pivot
/// cell): the filters then apply within them. Shown as a chip.
#[derive(Clone)]
pub struct Restrict {
    pub label: String,
    /// In this order (a tool may group rows side by side).
    pub ids: Arc<Vec<RowId>>,
}

/// Rows and cells a tool marked (review, compare, Insights): drawn in the grid.
#[derive(Clone, Default)]
pub struct Marks {
    pub rows: HashSet<RowId>,
    /// (row, column) → what was expected, shown on hover.
    pub cells: std::collections::HashMap<(RowId, usize), String>,
    /// Duplicate groups (tools::dupes): each group's edge, first-row line, differing cells.
    pub dups: Option<Arc<crate::tools::dupes::DupMarks>>,
}

/// The find & replace bar's state (Ctrl+F): it finds without filtering; the grid
/// highlights `text` in the cells shown, the bar steps through the matches.
#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub text: String,
    pub replace: String,
    pub regex: bool,
    pub match_case: bool,
    /// Only this column (None: every column).
    pub col: Option<usize>,
    /// Matching cells, in view order: (position in the view, column).
    pub matches: Arc<Vec<(usize, usize)>>,
    /// Index in `matches` of the current one.
    pub current: Option<usize>,
    /// Asks the bar to take the keyboard focus.
    pub focus: bool,
}

/// Values offered by fill by example: drawn in grey in the empty cells of `col`
/// until Tab takes them or Escape drops them.
pub struct Ghost {
    pub col: usize,
    pub values: std::collections::HashMap<RowId, String>,
    /// The rule, spelled out.
    pub rule: String,
    /// When offered: the ghosts come in as a wave down the rows (`grid::GHOST_IN`).
    pub born: std::time::Instant,
}

/// What the rows shown were computed from: a filter that only narrows them
/// (one more letter typed, one more column filter) starts from them, and a
/// new sort of the same rows skips the filter.
struct ViewMemo {
    spec: FilterSpec,
    expr: Option<String>,
    restrict: Option<Arc<Vec<RowId>>>,
    sort: Vec<SortKey>,
    kinds: Vec<Kind>,
    state: [usize; 6],
    /// The rows shown, once the job is done.
    view: Option<Arc<Vec<RowId>>>,
    /// The same rows before the sort (in file or restrict order), set by the job; None: as `view`.
    filtered: Arc<std::sync::Mutex<Option<Arc<Vec<RowId>>>>>,
}

/// The filters of a tab, compiled (`Tab::query`).
pub struct Query {
    /// Inverted: the value filters, applied as they are, and the text tests, whose rows go.
    /// Otherwise everything is in `texts` (the engine inverts a search alone).
    pub values: view::Filter,
    pub texts: view::Filter,
    pub expr: Option<csvfab_engine::formula::Expr>,
    expr_text: Option<String>,
    pub whole: bool,
    /// The rows a tool asked to show, the filters applying within them.
    pub within: Option<Arc<Vec<RowId>>>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.texts.is_empty() && self.expr.is_none() && self.within.is_none()
    }

    /// The rows passing, in file order (or `within`'s), unsorted; None if cancelled. `errors`:
    /// where the expression's errors are told (count, first message).
    pub fn run(&self, st: &State, errors: Option<&std::sync::Mutex<Option<(usize, String)>>>, cancel: &AtomicBool) -> Option<Vec<RowId>> {
        let all = || -> Vec<RowId> { (0..st.len()).map(|p| st.data_id(p)).collect() };
        let over = |f: &view::Filter, rows: Option<&[RowId]>| -> Option<Vec<RowId>> {
            match rows {
                Some(r) => view::filter_ids(st, f, r, cancel),
                None if f.is_empty() => Some(all()),
                None => view::filter(st, f, cancel),
            }
        };
        // The universe: the rows a tool showed, those of the value filters (inverted only).
        let universe: Option<Arc<Vec<RowId>>> = if self.values.is_empty() { self.within.clone() } else { Some(Arc::new(over(&self.values, self.within.as_deref().map(|v| v.as_slice()))?)) };
        let within = match &self.expr {
            Some(e) => {
                let o = csvfab_engine::formula::filter(st, e, universe.as_deref().map(|v| v.as_slice()), cancel)?;
                if let (Some(m), true) = (errors, o.errors > 0) { *m.lock().unwrap() = Some((o.errors, o.first_error.unwrap_or_default())); }
                Some(Arc::new(o.out))
            }
            None => universe.clone(),
        };
        let ids = over(&self.texts, within.as_deref().map(|v| v.as_slice()))?;
        if !self.whole { return Some(ids); }
        // The rows that fail the test, in their order.
        let pass = csvfab_engine::table::RowMask::new(ids.iter().copied());
        Some(match &universe {
            Some(u) => u.iter().copied().filter(|&id| !pass.contains(id)).collect(),
            None => (0..st.len()).map(|p| st.data_id(p)).filter(|&id| !pass.contains(id)).collect(),
        })
    }
}

/// A table's identity: rows and cells change only with a new value of one of these.
fn state_key(st: &State) -> [usize; 6] {
    let ptr = |p: Option<*const u8>| p.map_or(0, |p| p as usize);
    [
        Arc::as_ptr(&st.base) as usize,
        Arc::as_ptr(&st.cols) as *const u8 as usize,
        ptr(st.order.as_ref().map(|o| Arc::as_ptr(o) as *const u8)),
        ptr(st.cmap.as_ref().map(|m| Arc::as_ptr(m) as *const u8)),
        st.ncols,
        st.header as usize,
    ]
}

/// A selection's numbers are for: (first row, last row, first column, last column, version, view).
type SumKey = (usize, usize, usize, usize, u64, u64);

/// A reading option picked: None, unchanged; Some(None), detected ("Auto"); Some(Some(x)), x.
pub type Choice<T> = Option<Option<T>>;

/// The view's settings tied to column positions: the column filters, widths, sort, values
/// hidden from a panel, find's column, the grid's hidden / frozen columns, scales and bars,
/// the totals picked. A structural edit carries them over (`edit_kit::remap_columns`);
/// kept per table version, Undo and Redo bring them back with the columns.
#[derive(Clone)]
pub struct ColView {
    pub filters: Vec<String>,
    pub widths: Vec<f32>,
    pub sort: Vec<SortKey>,
    pub hidden_vals: Vec<(usize, Arc<HashSet<String>>)>,
    pub picks: Vec<view::ValuePick>,
    pub find_col: Option<usize>,
    pub look: crate::grid::ColLook,
    pub totals: crate::totals_ui::Choices,
    /// The rest of the view, kept by an edit that clears it ("Delete hidden rows"): the search
    /// (text, expression mode) and the rows a tool showed. None: left as it is.
    pub rest: Option<(String, bool, Option<Restrict>)>,
}

pub struct Tab {
    /// Stable identity (tools hold on to their tab by it).
    pub uid: u64,
    /// The column settings at each table version a structural edit left (`ColView`).
    pub col_views: std::collections::HashMap<u64, ColView>,
    /// Read again with another encoding or delimiter: the columns may be others, so what
    /// was tied to them (filters, sort, widths…) goes once it is read (as the web's `reread`).
    reread: bool,
    pub path: Option<PathBuf>,
    pub name: String,
    pub table: Option<Table>,
    loading: Option<Loader>,
    pub error: Option<String>,
    pub open_opts: OpenOptions,
    /// The rows shown, in display order; None: every data row in file order.
    pub view: Option<Arc<Vec<RowId>>>,
    pub kinds: Vec<Kind>,
    /// Line 1 as chosen before the file was opened (None: detected).
    pub want_header: Option<bool>,
    /// What the marked rows are ("changed rows"): the status bar's chip says it.
    pub marks_label: String,
    /// The name typed for the column being inserted (the insert dialog → the action).
    pub new_col_name: Option<String>,
    /// Garbled accents ("Ã©" for "é") in the table, for the status bar's chip: the answer
    /// for (bytes, version), and the look under way.
    moji: Option<((usize, u64), bool)>,
    /// The selection's numbers (sum, count) for the status bar, worked out in the background
    /// up to a million cells: the answer for its key, and the job under way.
    sel_sum: Option<(SumKey, Option<(f64, usize)>)>,
    sel_job: Option<(SumKey, crate::job::Job<Option<(f64, usize)>>)>,
    moji_job: Option<((usize, u64), crate::job::Job<bool>)>,
    pub rich: Vec<Option<Rich>>,
    pub pills: Vec<Option<Pills>>,
    pub filter: FilterSpec,
    /// Per column filter texts, as typed (the spec is rebuilt from them).
    pub col_filters: Vec<String>,
    pub sort: Vec<SortKey>,
    pub restrict: Option<Restrict>,
    /// The search box holds a formula (`num({Amount}) > 1000`), not text.
    pub expr_mode: bool,
    pub marks: Arc<Marks>,
    pub filter_error: Option<String>,
    /// The column whose filter box holds the error (None: the search box).
    pub filter_error_col: Option<usize>,
    /// The expression filter's rows that raised an error (hidden), with the first message:
    /// set by the view job, said in the status bar.
    pub expr_errors: Arc<std::sync::Mutex<Option<(usize, String)>>>,
    job: Option<Job<Option<Vec<RowId>>>>,
    /// What `view` was computed from, and what the running job computes (view.rs's filter path).
    memo: Option<ViewMemo>,
    pending: Option<ViewMemo>,
    pub view_time: Option<Duration>,
    pub backed_up: bool,
    pub grid: crate::grid::GridState,
    pub show_filters: bool,
    pub show_charts: bool,
    pub show_totals: bool,
    /// The app's band settings were given to this tab (on its first frame).
    pub bands_set: bool,
    /// Bumped whenever the rows shown or their content change: views computed
    /// from them (charts, totals) know they are stale.
    pub generation: u64,
    pub spark: crate::spark_ui::State,
    /// The column inspector's figures, while it is on.
    pub inspector: Option<Box<crate::inspector::Inspector>>,
    pub totals: crate::totals_ui::State,
    pub find: Find,
    pub ghost: Option<Arc<Ghost>>,
    /// The duplicate marks' count after the table changed (`tools::dupes::start_refresh`): the
    /// job under way, the table the marks were counted on, when the next count starts (a
    /// moment after the last change), and the marks held back while the rows were renumbered.
    pub dups_job: Option<crate::tools::dupes::Refresh>,
    pub dups_seen: (usize, u64),
    dups_due: Option<Instant>,
    pub dups_held: Option<Arc<crate::tools::dupes::DupMarks>>,
    /// Set by a save that wrote the rows in another order (inserted, deleted, moved): each old
    /// row id's record in the file written (`u32::MAX`: not written). The next table installed
    /// is that file: the rows a tool showed or marked follow their new ids.
    pub renumber: Option<Arc<Vec<u32>>>,
    /// The column titles, for (`state_key`, version): read once, not at every frame's call.
    heads: std::sync::Mutex<Option<(([usize; 6], u64), Arc<Vec<String>>)>>,
    /// Every data row's id in file order, for (`state_key`): made once for the tools that take
    /// the rows shown of an unfiltered tab, not at each opening.
    all_ids: std::sync::Mutex<Option<([usize; 6], Arc<Vec<RowId>>)>>,
}

/// After a save of `st`: each row id's record in the file written, when the rows are no longer
/// where they were (None: every row kept its id). On the save's thread.
pub fn renumbering(st: &State) -> Option<Arc<Vec<u32>>> {
    let order = st.order.as_ref()?;
    let max = order.iter().copied().max().map_or(0, |m| m as usize + 1);
    let mut map = vec![u32::MAX; max];
    for (k, &id) in order.iter().enumerate() { map[id as usize] = k as u32; }
    Some(Arc::new(map))
}

fn next_uid() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

impl Tab {
    pub fn open(path: PathBuf, opts: OpenOptions, ctx: &egui::Context) -> Tab {
        Tab::open_with(path, opts, None, ctx)
    }

    /// `open`, line 1 as titles or data (`header`; None: detected).
    pub fn open_with(path: PathBuf, opts: OpenOptions, header: Option<bool>, ctx: &egui::Context) -> Tab {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let mut t = Tab::empty(name, Some(path), opts);
        t.want_header = header;
        t.reload(ctx);
        t
    }

    /// The file at `path` with the unsaved edits a journal kept (crash recovery), read as the
    /// journal says: unsaved, Save writes them over it.
    pub fn recover(path: PathBuf, opts: OpenOptions, journal: Vec<u8>, ctx: &egui::Context) -> Tab {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let mut t = Tab::empty(name, Some(path.clone()), opts);
        t.loading = Some(Loader::spawn(path, opts, None, None, false, Some(journal), ctx));
        t
    }

    /// A tab whose table is built in memory (a group-by result): no file until saved.
    pub fn memory(name: String, bytes: Vec<u8>, ctx: &egui::Context, m: &crate::theme::Metrics) -> Tab {
        let mut t = Tab::empty(name, None, OpenOptions::default());
        let base = Base::from_bytes(bytes, OpenOptions::default());
        t.table = Some(Table::new(Arc::new(base), None));
        t.analyse();
        let n = t.table.as_ref().unwrap().st.ncols;
        t.col_filters.resize(n, String::new());
        t.grid.reset_widths(&t.table.as_ref().unwrap().st, ctx, m);
        t
    }

    /// Renames column `col`, as one undo step. A file without a title line gets one, above
    /// its first row (the other columns named "Column N"): its data stays data.
    pub fn rename_column(&mut self, col: usize, text: String, ctx: &egui::Context) {
        let Some(table) = &mut self.table else { return };
        table.batch(|table| {
            if table.st.header {
                let hid = table.st.all_id(0);
                table.set_cells(&[(hid, col, text)]);
            } else {
                let names = table.st.headers();
                let id = table.insert_rows(0, 1)[0];
                let cells: Vec<_> = names.into_iter().enumerate().map(|(c, n)| (id, c, if c == col { text.clone() } else { n })).collect();
                table.set_cells(&cells);
                table.set_header_edit(true);
            }
        });
        // A title changed: the rows and the columns' kinds are the same (no analysis, no
        // filtering again), only what is drawn from the titles.
        self.after_edit(false, ctx);
    }

    /// A tab with nothing in it (for tools bound to no file).
    pub fn blank() -> Tab {
        Tab::empty(String::new(), None, OpenOptions::default())
    }

    fn empty(name: String, path: Option<PathBuf>, opts: OpenOptions) -> Tab {
        Tab {
            uid: next_uid(),
            path,
            name,
            table: None,
            loading: None,
            error: None,
            open_opts: opts,
            view: None,
            kinds: Vec::new(),
            new_col_name: None,
            col_views: Default::default(),
            reread: false,
            marks_label: String::new(),
            want_header: None,
            moji: None,
            sel_sum: None,
            sel_job: None,
            moji_job: None,
            rich: Vec::new(),
            pills: Vec::new(),
            filter: FilterSpec::default(),
            col_filters: Vec::new(),
            sort: Vec::new(),
            restrict: None,
            expr_mode: false,
            marks: Arc::default(),
            filter_error: None,
            filter_error_col: None,
            expr_errors: Default::default(),
            job: None,
            memo: None,
            pending: None,
            view_time: None,
            backed_up: false,
            grid: Default::default(),
            show_filters: true,
            show_charts: true,
            show_totals: false,
            bands_set: false,
            generation: 0,
            spark: Default::default(),
            inspector: None,
            totals: Default::default(),
            find: Find::default(),
            ghost: None,
            dups_job: None,
            dups_seen: (0, 0),
            dups_due: None,
            dups_held: None,
            renumber: None,
            heads: Default::default(),
            all_ids: Default::default(),
        }
    }

    /// Reads the file (again) in the background: on open, after a save, or with
    /// another encoding or delimiter. Filters and sort are kept. A big file
    /// shows its first rows at once, the whole index following.
    pub fn reload(&mut self, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else { return };
        let header = self.table.as_ref().map(|t| t.st.header).or(self.want_header);
        self.loading = Some(Loader::spawn(path, self.open_opts, None, header, self.table.is_none(), None, ctx));
    }

    /// While reading: the share done.
    pub fn load_progress(&self) -> Option<f32> {
        self.loading.as_ref().map(|l| l.progress.load(Ordering::Relaxed) as f32 / l.total.max(1) as f32)
    }

    /// Whether the rows shown are only the file's first ones, the rest being indexed.
    pub fn partial(&self) -> bool {
        self.table.as_ref().is_some_and(|t| t.st.base.partial)
    }

    /// Called every frame: picks up finished jobs.
    pub fn poll(&mut self, ctx: &egui::Context, m: &crate::theme::Metrics) {
        let mut latest = None;
        if let Some(l) = &self.loading {
            while let Ok(msg) = l.rx.try_recv() {
                // The whole base supersedes a head arrived in the same frame.
                if !(matches!(msg, Load::Head(_)) && matches!(latest, Some(Load::Full(_)))) { latest = Some(msg); }
            }
        }
        match latest {
            Some(Load::Failed(e)) => {
                self.loading = None;
                self.error = Some(e);
            }
            Some(Load::Head(r)) => self.install(*r, ctx, m),
            Some(Load::Full(r)) => {
                if let Some(l) = &self.loading {
                    crate::profile::mark(&format!("file read ({:.1} ms in the engine, {:.1} ms since the read started)", r.table.st.base.load_time.as_secs_f64() * 1e3, l.started.elapsed().as_secs_f64() * 1e3));
                }
                self.loading = None;
                self.install(*r, ctx, m);
            }
            None => {}
        }
        // The duplicate marks follow every change of the table, counted a moment after the last.
        if self.marks.dups.is_some() || self.dups_held.is_some() {
            let key = self.table_key();
            if key != self.dups_seen {
                self.dups_seen = key;
                let wait = if self.dups_held.is_some() { 0 } else { 300 };
                self.dups_due = Some(Instant::now() + Duration::from_millis(wait));
                ctx.request_repaint_after(Duration::from_millis(wait));
            }
            if self.dups_due.is_some_and(|d| Instant::now() >= d) && !self.busy() {
                self.dups_due = None;
                crate::tools::dupes::start_refresh(self, ctx);
            } else if let Some(d) = self.dups_due {
                ctx.request_repaint_after(d.saturating_duration_since(Instant::now()));
            }
        }
        if self.dups_job.is_some() { crate::tools::dupes::poll_refresh(self, ctx); }
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            let job = self.job.take().unwrap();
            if let Some(ids) = r {
                self.view = Some(Arc::new(ids));
                self.view_time = Some(job.started.elapsed());
                self.generation += 1;
                self.grid.clamp(self.shown());
            }
        }
    }

    /// Shows a file read: a new table, or the whole file under the table its
    /// first records made (edits made meanwhile kept).
    fn install(&mut self, r: Ready, ctx: &egui::Context, m: &crate::theme::Metrics) {
        let keep_view = r.same_rows && self.job.is_none();
        let base = &r.table.st.base;
        if base.partial { crate::profile::mark("first rows read"); }
        let first = self.table.is_none();
        let grows = self.table.as_ref().is_some_and(|t| t.st.base.partial && !base.partial && t.st.base.same_bytes(base));
        match &mut self.table {
            // Edited while the rest was being read: the edits move onto the whole file.
            Some(t) if grows && (t.can_undo() || t.can_redo()) => {
                t.rebase(base.clone());
                if t.st.ncols == r.table.st.ncols && t.st.cmap.is_none() {
                    (self.kinds, self.rich, self.pills) = (r.kinds, r.rich, r.pills);
                } else {
                    self.analyse();
                }
            }
            _ => {
                // The table read before (a reload): freed off the UI thread.
                crate::app::drop_later(self.table.replace(r.table));
                if let Some(map) = self.renumber.take() { self.renumber_ids(&map); }
                (self.kinds, self.rich, self.pills) = (r.kinds, r.rich, r.pills);
                // A new table counts its versions from 0 again: the old ones' settings would mismatch.
                self.col_views.clear();
            }
        }
        self.error = None;
        self.generation += 1;
        let n = self.table.as_ref().unwrap().st.ncols;
        let reread = std::mem::take(&mut self.reread);
        if reread {
            self.col_filters.clear();
            self.filter.hidden.clear();
            self.filter.picks.clear();
            self.spark.picks.clear();
            self.sort.clear();
            self.restrict = None;
            self.find.col = None;
            self.grid.set_col_look(Default::default(), n);
            self.totals.set_choices(Default::default());
            self.col_views.clear();
        }
        self.col_filters.resize(n, String::new());
        if first || reread || self.grid.widths.len() != n {
            if grows { self.grid.fit_count(n); } else { self.grid.reset_widths(&self.table.as_ref().unwrap().st, ctx, m); }
        }
        if keep_view {
            // A save of the rows shown, each where it was: the same rows, the same cells. The
            // view stands; only what it was computed on is now the new table.
            if let Some(mut p) = self.pending.take() {
                p.view = self.view.clone();
                if p.view.is_some() { self.memo = Some(p); }
            }
            if let Some(memo) = &mut self.memo { memo.state = state_key(&self.table.as_ref().unwrap().st); }
            self.generation += 1;
            return;
        }
        self.refresh(ctx);
    }

    /// The column titles (`State::headers`), kept until the table changes.
    pub fn heads(&self) -> Arc<Vec<String>> {
        let Some(t) = &self.table else { return Arc::default() };
        let key = (state_key(&t.st), t.version());
        let mut h = self.heads.lock().unwrap();
        if let Some((k, v)) = &*h { if *k == key { return v.clone(); } }
        let v = Arc::new(t.st.headers());
        *h = Some((key, v.clone()));
        v
    }

    /// The table's identity for what is counted on it: its file and its version.
    pub fn table_key(&self) -> (usize, u64) {
        self.table.as_ref().map_or((0, 0), |t| (Arc::as_ptr(&t.st.base) as usize, t.version()))
    }

    /// The rows a tool showed or marked, on their ids in the file a save just wrote (`map`:
    /// old id → new). The duplicate marks are held back until counted again on the new ids.
    fn renumber_ids(&mut self, map: &[u32]) {
        let to = |id: RowId| map.get(id as usize).copied().filter(|&n| n != u32::MAX);
        if let Some(r) = &mut self.restrict {
            r.ids = Arc::new(r.ids.iter().filter_map(|&id| to(id)).collect());
        }
        let m = Arc::make_mut(&mut self.marks);
        m.rows = m.rows.iter().filter_map(|&id| to(id)).collect();
        m.cells = std::mem::take(&mut m.cells).into_iter().filter_map(|((id, c), v)| Some(((to(id)?, c), v))).collect();
        if let Some(d) = m.dups.take() { self.dups_held = Some(d); }
        self.ghost = None;
        self.find.matches = Arc::default();
        self.find.current = None;
    }

    /// Column kinds and the grid's reading aids, from the first rows.
    fn analyse(&mut self) {
        let Some(t) = &self.table else { return };
        (self.kinds, self.rich, self.pills) = looks(&t.st);
    }

    /// Whether the table shows garbled accents (the status bar's chip): looked for in the
    /// background, again after each edit (a repair makes the chip go). As read, the file's
    /// bytes (`file_mojibake`); edited, its cells (the first 200 000 rows).
    pub fn garbled(&mut self, ctx: &egui::Context) -> bool {
        let Some(t) = &self.table else { return false };
        let key = (Arc::as_ptr(&t.st.base) as usize, t.version());
        if let Some((k, v)) = self.moji { if k == key { return v; } }
        if let Some((k, j)) = &mut self.moji_job {
            if let Some(v) = j.poll() {
                self.moji = Some((*k, v));
                self.moji_job = None;
                if self.moji.is_some_and(|m| m.0 == key) { return v; }
            } else if *k == key {
                return self.moji.is_some_and(|m| m.1);
            }
        }
        let (st, dirty) = (t.st.clone(), t.dirty());
        self.moji_job = Some((key, crate::job::Job::spawn(ctx, move |cancel, _| {
            if !dirty { return csvfab_engine::kinds::file_mojibake(&st); }
            let mut row = Vec::new();
            for pos in 0..st.len().min(200_000) {
                if pos % 4096 == 0 && cancel.load(Ordering::Relaxed) { return false; }
                st.row(st.data_id(pos), &mut row);
                if row.iter().any(|c| csvfab_engine::kinds::has_mojibake(c)) { return true; }
            }
            false
        })));
        // Until it answers: what was known before.
        self.moji.is_some_and(|m| m.1)
    }

    /// The sum and count of the numbers in the selection (None: none yet, or past a
    /// million cells). Answered at once when known, else in a moment.
    pub fn selection_numbers(&mut self, ctx: &egui::Context) -> Option<(f64, usize)> {
        let (rows, cols) = self.grid.range()?;
        let t = self.table.as_ref()?;
        let key: SumKey = (*rows.start(), *rows.end(), *cols.start(), *cols.end(), t.version(), self.generation);
        let cells = (rows.end() - rows.start() + 1) * (cols.end() - cols.start() + 1);
        if cells > 1_000_000 { return None; }
        if let Some((k, v)) = &self.sel_sum { if *k == key { return *v; } }
        if let Some((k, j)) = &mut self.sel_job {
            if let Some(v) = j.poll() {
                let k = *k;
                self.sel_job = None;
                self.sel_sum = Some((k, v));
                if k == key { return v; }
            } else if *k == key {
                return None;
            }
        }
        // A drag growing the selection: counted once it stops (not a job per frame).
        if ctx.input(|i| i.pointer.primary_down()) { return None; }
        let (st, view) = (t.st.clone(), self.view.clone());
        // The columns shown only (as the web's), the rows' ids read in the job.
        let cols: Vec<usize> = cols.filter(|c| !self.grid.hidden.contains(c)).collect();
        self.sel_job = Some((key, crate::job::Job::spawn(ctx, move |cancel, _| {
            let shown = view.as_ref().map_or(st.len(), |v| v.len());
            let (mut sum, mut n, mut tmp) = (0.0, 0usize, Vec::new());
            for (k, pos) in rows.take_while(|&p| p < shown).enumerate() {
                if k % 4096 == 0 && cancel.load(Ordering::Relaxed) { return None; }
                let id = view.as_ref().map_or_else(|| st.data_id(pos), |v| v[pos]);
                for &c in &cols {
                    st.with_cell(id, c, &mut tmp, |s| if let Some(x) = csvfab_engine::stats::num_cell(s) { sum += x; n += 1; });
                }
            }
            (n > 0).then_some((sum, n))
        })));
        None
    }

    pub fn st(&self) -> Option<&State> {
        self.table.as_ref().map(|t| &t.st)
    }

    pub fn busy(&self) -> bool {
        self.loading.is_some() || self.job.is_some()
    }

    pub fn dirty(&self) -> bool {
        self.table.as_ref().is_some_and(|t| t.dirty())
    }

    /// Rows shown.
    pub fn shown(&self) -> usize {
        match (&self.view, &self.table) {
            (Some(v), _) => v.len(),
            (None, Some(t)) => t.st.len(),
            _ => 0,
        }
    }

    #[inline]
    pub fn shown_id(&self, pos: usize) -> RowId {
        match &self.view {
            Some(v) => v[pos],
            None => self.table.as_ref().unwrap().st.data_id(pos),
        }
    }

    /// Where row `id` is shown (None: not shown): no copy of the rows, found from its id when the
    /// view is the file in its order.
    pub fn position_of(&self, id: RowId) -> Option<usize> {
        match (&self.view, self.st()) {
            (Some(v), _) => v.iter().position(|&x| x == id),
            (None, Some(st)) => match &st.order {
                None => (id as usize).checked_sub(st.header as usize).filter(|&p| p < st.len()),
                Some(o) => o.iter().skip(st.header as usize).position(|&x| x == id),
            },
            _ => None,
        }
    }

    /// Every row shown, in order (a copy when the view is the whole file).
    pub fn shown_ids(&self) -> Arc<Vec<RowId>> {
        match (&self.view, &self.table) {
            (Some(v), _) => v.clone(),
            (None, Some(t)) => {
                let key = state_key(&t.st);
                let mut a = self.all_ids.lock().unwrap();
                if let Some((k, v)) = &*a { if *k == key { return v.clone(); } }
                let v: Arc<Vec<RowId>> = Arc::new((0..t.st.len()).into_par_iter().with_min_len(1 << 16).map(|p| t.st.data_id(p)).collect());
                *a = Some((key, v.clone()));
                v
            }
            _ => Arc::default(),
        }
    }

    /// Whether filters or a tool's rows narrow the view (a sort aside).
    pub fn narrowed(&self) -> bool {
        !self.filter.is_empty() || self.col_filters.iter().any(|s| !s.is_empty()) || self.restrict.is_some()
    }

    /// Whether filters, a sort or a tool's rows narrow or reorder the view.
    /// The table as exports write it: without the hidden columns. None: every column is hidden.
    /// Never for a Save or a Save as: the file keeps its hidden columns.
    pub fn export_st(&self) -> Option<csvfab_engine::State> {
        let st = self.st()?;
        let hidden = &self.grid.hidden;
        if hidden.is_empty() { return Some(st.clone()); }
        Some(st.without_columns(|c| hidden.contains(&c))).filter(|s| s.ncols > 0)
    }

    pub fn filtered(&self) -> bool {
        !self.filter.is_empty() || self.col_filters.iter().any(|s| !s.is_empty()) || !self.sort.is_empty() || self.restrict.is_some()
    }

    /// Hides (or shows again) values of a column, as the column panel and a cell's menu do: the
    /// column's value filter becomes this one (a chart's pick on it goes).
    pub fn hide_values(&mut self, col: usize, values: impl Into<Arc<HashSet<String>>>, ctx: &egui::Context) {
        let values = values.into();
        self.filter.hidden.retain(|(c, _)| *c != col);
        self.filter.picks.retain(|p| p.col != col);
        if !values.is_empty() { self.filter.hidden.push((col, values)); }
        self.grid.scroll = 0.0;
        self.refresh(ctx);
    }

    /// Keeps only the rows passing `keep` in its column (a chart's part, a cell's value): the
    /// column's value filter becomes this one, the others stay. None: the column's goes.
    pub fn keep_values(&mut self, col: usize, keep: Option<view::Keeps>, ctx: &egui::Context) {
        self.filter.hidden.retain(|(c, _)| *c != col);
        self.filter.picks.retain(|p| p.col != col);
        if let Some(keep) = keep { self.filter.picks.push(view::ValuePick { col, keep }); }
        self.grid.scroll = 0.0;
        self.refresh(ctx);
    }

    /// Whether value filters (values hidden, kept) are set on column `col`.
    pub fn has_values(&self, col: usize) -> bool {
        self.filter.hidden.iter().any(|(c, h)| *c == col && !h.is_empty()) || self.filter.picks.iter().any(|p| p.col == col)
    }

    /// Recomputes the rows shown from the filters and sort, in the background.
    /// A job still running is cancelled: typing in a filter never queues work.
    /// A filter that narrows the previous one filters only the rows shown (in
    /// their order: a sort unchanged needs no sorting again); a new sort of the
    /// same filter sorts the rows kept, without filtering again.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        // A job done but not picked up yet: its rows are the latest.
        if let Some(Some(ids)) = self.job.as_mut().and_then(|j| j.poll()) {
            self.view = Some(Arc::new(ids));
            self.job = None;
            self.grid.clamp(self.shown());
        }
        if let Some(j) = self.job.take() {
            j.cancel();
            self.pending = None;
        } else if let Some(mut p) = self.pending.take() {
            p.view = self.view.clone();
            if p.view.is_some() { self.memo = Some(p); }
        }
        if self.table.is_none() { return; }
        self.filter.cols = self.col_filters.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(c, s)| (c, s.clone())).collect();
        self.generation += 1;
        *self.expr_errors.lock().unwrap() = None;
        let q = match self.query(None) {
            Ok(q) => { (self.filter_error, self.filter_error_col) = (None, None); q }
            // The rows shown stay those of the last filter that compiled; the status bar says why.
            Err((e, col)) => { (self.filter_error, self.filter_error_col) = (Some(e), col); return; }
        };
        let t = self.table.as_ref().unwrap();
        if q.is_empty() && self.sort.is_empty() {
            self.view = None;
            self.memo = None;
            self.view_time = None;
            self.grid.clamp(self.shown());
            return;
        }
        let st = t.st.clone();
        // The column caches these read, built in the background for the next keystroke.
        q.values.warm(&st, &self.kinds);
        q.texts.warm(&st, &self.kinds);
        view::warm_sort(&st, &self.sort, &self.kinds);
        let (sort, kinds) = (self.sort.clone(), self.kinds.clone());
        let mut spec = self.filter.clone();
        if self.expr_mode { spec.global.clear(); }
        let memo = ViewMemo {
            spec: spec.clone(),
            expr: q.expr_text.clone(),
            restrict: q.within.clone(),
            sort: sort.clone(),
            kinds: kinds.clone(),
            state: state_key(&st),
            view: None,
            filtered: Default::default(),
        };
        // Where to start from: the rows shown (already in this sort's order), or
        // the same rows unsorted, when the new filter keeps no row they lack.
        let start = self.memo.as_ref().filter(|m| {
            !q.whole
                && m.view.is_some()
                && m.view.as_ref().map(Arc::as_ptr) == self.view.as_ref().map(Arc::as_ptr)
                && m.expr == memo.expr
                && m.restrict.as_ref().map(Arc::as_ptr) == q.within.as_ref().map(Arc::as_ptr)
                && m.state == memo.state
                && spec.refines(&m.spec)
        });
        let start = start.and_then(|m| {
            let filtered = m.filtered.lock().unwrap().clone().or_else(|| m.sort.is_empty().then(|| m.view.clone().unwrap()));
            if m.sort == sort && m.kinds == kinds {
                Some((m.view.clone().unwrap(), true, filtered, m.spec == spec))
            } else {
                filtered.map(|fl| (fl, false, None, m.spec == spec))
            }
        });
        let slot = memo.filtered.clone();
        let keep_filtered = !sort.is_empty() && !q.is_empty();
        let errors = self.expr_errors.clone();
        // The duplicates view sorted: its groups kept together, ordered by their first row.
        let groups = if sort.is_empty() || !crate::tools::dupes::in_view(self) { None } else { self.marks.dups.as_ref().map(|d| d.dupes.clone()) };
        self.pending = Some(memo);
        self.job = Some(Job::spawn(ctx, move |cancel, _| {
            let (mut ids, sorted) = match start {
                Some((rows, sorted, prev_filtered, same)) => {
                    let ids = if same { rows.to_vec() } else { view::filter_ids(&st, &q.texts, &rows, cancel)? };
                    if sorted && keep_filtered {
                        // The unsorted rows kept, for a later sort: the previous ones still shown.
                        if let Some(pf) = prev_filtered {
                            let mut keep = vec![false; ids.iter().max().map_or(0, |&m| m as usize + 1)];
                            for &id in &ids { keep[id as usize] = true; }
                            *slot.lock().unwrap() = Some(Arc::new(pf.iter().copied().filter(|&id| keep.get(id as usize) == Some(&true)).collect()));
                        }
                    }
                    (ids, sorted)
                }
                None => (q.run(&st, Some(&errors), cancel)?, false),
            };
            if !sorted {
                if keep_filtered { *slot.lock().unwrap() = Some(Arc::new(ids.clone())); }
                if !view::sort(&st, &mut ids, &sort, &kinds, cancel) { return None; }
                if let Some(d) = &groups { d.keep_together(&mut ids); }
            }
            Some(ids)
        }));
    }

    /// The filters as they stand, ready to run (`Query::run`): `skip`, a column whose value
    /// filters are left out (its panel counts what the others let through). Err: one does not
    /// compile (the reason).
    pub fn query(&self, skip: Option<usize>) -> Result<Query, (String, Option<usize>)> {
        let t = self.table.as_ref().ok_or_else(|| (String::new(), None))?;
        let mut spec = self.filter.clone();
        spec.cols = self.col_filters.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(c, s)| (c, s.clone())).collect();
        if let Some(c) = skip {
            spec.hidden.retain(|h| h.0 != c);
            spec.picks.retain(|p| p.col != c);
        }
        // In formula mode the search box is an expression, applied before the other filters.
        let expr = if self.expr_mode && !spec.global.trim().is_empty() {
            match csvfab_engine::formula::compile(&spec.global, &t.st.headers()) {
                Ok(mut e) => { e.set_decimal_comma(csvfab_engine::formula::decimal_comma(&t.st)); Some(e) }
                Err(e) => return Err((format!("expression: {}", e.msg), None)),
            }
        } else { None };
        let expr_text = expr.as_ref().map(|_| spec.global.clone());
        if self.expr_mode { spec.global.clear(); }
        // Invert flips the text tests, as the web's: the search, the column filters, the
        // expression — never the value filters (ticked by hand). The search alone, the engine
        // inverts it itself (the value filters apply as they are).
        let whole = spec.invert && (!spec.cols.is_empty() || expr.is_some());
        let compile = |s: &FilterSpec| view::Filter::compile(s).map_err(|e| {
            // Said with the column it is in.
            let bad = spec.cols.iter().find(|(_, t)| view::Filter::compile(&FilterSpec { cols: vec![(0, t.clone())], regex: spec.regex, match_case: spec.match_case, ..Default::default() }).is_err());
            match bad {
                Some((c, _)) => (format!("column \"{}\": {e}", t.st.headers().get(*c).map_or("", |h| h.as_str())), Some(*c)),
                None if spec.regex => (format!("regex: {e}"), None),
                None => (e, None),
            }
        });
        let (values, texts) = if whole {
            (compile(&spec.values())?, compile(&FilterSpec { invert: false, ..spec.texts() })?)
        } else {
            (compile(&FilterSpec::default())?, compile(&spec)?)
        };
        Ok(Query { values, texts, expr, expr_text, whole, within: self.restrict.as_ref().map(|r| r.ids.clone()) })
    }

    /// Shows only these rows (in this order), filters applying within them.    /// Shows only these rows (in this order), filters applying within them.
    pub fn restrict_to(&mut self, label: impl Into<String>, ids: Vec<RowId>, ctx: &egui::Context) {
        let label = label.into();
        // The rows a tool marked, shown alone: the marks' chip is named after them.
        if !self.marks.rows.is_empty() { self.marks_label = label.clone(); }
        self.restrict = Some(Restrict { label, ids: Arc::new(ids) });
        self.grid.scroll = 0.0;
        self.refresh(ctx);
    }

    /// Applies cell changes as one undo step; the rows stay where they are.
    pub fn apply_changes(&mut self, changes: &[(RowId, usize, String)], ctx: &egui::Context) {
        let Some(t) = &mut self.table else { return };
        let grow = changes.iter().any(|c| c.1 >= t.st.ncols);
        // A few cells typed under the totals row: told to it, not counted again (a view's rows
        // are looked through for each, hence so few).
        let few = if self.view.is_some() { 4 } else { 64 };
        let before = (self.show_totals && !grow && changes.len() <= few).then(|| t.st.clone());
        t.set_cells(changes);
        if let Some(b) = before { crate::totals_ui::edited(self, &b, changes); }
        self.generation += 1;
        self.after_edit(grow, ctx);
    }

    /// Puts in cells made ready beforehand (`Patch`, often in a job) as one undo step; the rows
    /// stay where they are.
    pub fn apply_patch(&mut self, p: csvfab_engine::table::Patch, ctx: &egui::Context) {
        let Some(t) = &mut self.table else { return };
        let grow = p.columns().any(|c| c >= t.st.ncols);
        t.apply(p);
        self.generation += 1;
        self.after_edit(grow, ctx);
    }

    pub fn col_view(&self) -> ColView {
        ColView {
            filters: self.col_filters.clone(),
            widths: self.grid.widths.clone(),
            sort: self.sort.clone(),
            hidden_vals: self.filter.hidden.clone(),
            picks: self.filter.picks.clone(),
            find_col: self.find.col,
            look: self.grid.col_look(),
            totals: self.totals.choices(),
            rest: None,
        }
    }

    /// Puts column settings back (made for the table's present columns).
    pub fn set_col_view(&mut self, v: ColView) {
        let n = v.filters.len();
        self.col_filters = v.filters;
        self.grid.widths = v.widths;
        self.sort = v.sort;
        self.filter.hidden = v.hidden_vals;
        self.filter.picks = v.picks;
        self.find.col = v.find_col;
        self.grid.set_col_look(v.look, n);
        self.totals.set_choices(v.totals);
        if let Some((global, expr, restrict)) = v.rest {
            (self.filter.global, self.expr_mode, self.restrict) = (global, expr, restrict);
        }
    }

    /// Keeps the column settings of table version `version`, for Undo / Redo to come back to.
    pub fn keep_col_view(&mut self, version: u64, v: ColView) {
        if self.col_views.len() >= 256 {
            if let Some(&old) = self.col_views.keys().min() { self.col_views.remove(&old); }
        }
        self.col_views.insert(version, v);
    }

    /// After an undo/redo or a structural edit: the view is recomputed; the
    /// rows of a cell edit simply stay where they are.
    pub fn after_edit(&mut self, structure: bool, ctx: &egui::Context) {
        self.generation += 1;
        if structure {
            if let Some(t) = &self.table {
                let n = t.st.ncols;
                self.col_filters.resize(n, String::new());
                self.grid.fit_count(n);
            }
            self.analyse();
            let n = self.table.as_ref().map_or(0, |t| t.st.ncols);
            self.sort.retain(|k| k.col < n);
            self.refresh(ctx);
        }
    }

    /// After columns were inserted, deleted or moved (`map`: an old column's new place): the
    /// kinds and reading aids follow their columns — only a new column is looked at — and
    /// the view is recomputed, but for a move: the same rows in the same order.
    pub fn after_columns(&mut self, old_n: usize, map: &dyn Fn(usize) -> Option<usize>, ctx: &egui::Context) {
        self.generation += 1;
        let Some(st) = self.table.as_ref().map(|t| t.st.clone()) else { return };
        let n = st.ncols;
        self.col_filters.resize(n, String::new());
        self.grid.fit_count(n);
        let (mut kinds, mut rich, mut pills) = (vec![None; n], vec![None; n], vec![None; n]);
        for c in 0..old_n.min(self.kinds.len()) {
            if let Some(k) = map(c).filter(|&k| k < n) {
                kinds[k] = Some(self.kinds[c]);
                rich[k] = Some(self.rich.get(c).cloned().flatten());
                pills[k] = Some(self.pills.get(c).cloned().flatten());
            }
        }
        let new: Vec<usize> = (0..n).filter(|&k| kinds[k].is_none()).collect();
        let moved = new.is_empty() && n == old_n;
        let found = view::detect_kinds_of(&st, &new);
        for (&k, f) in new.iter().zip(found) { kinds[k] = Some(f); }
        self.kinds = kinds.into_iter().map(|k| k.unwrap_or(Kind::Text)).collect();
        for (&k, r) in new.iter().zip(look::rich_cols_of(&st, &self.kinds, &new)) { rich[k] = Some(r); }
        self.rich = rich.into_iter().map(Option::flatten).collect();
        for (&k, p) in new.iter().zip(look::pills_of(&st, &self.kinds, &self.rich, &new)) { pills[k] = Some(p); }
        self.pills = pills.into_iter().map(Option::flatten).collect();
        self.sort.retain(|k| k.col < n);
        if moved && self.job.is_none() {
            // The filters follow their columns (`filter.cols` from the texts moved with them);
            // the rows they keep are the same, in the same order.
            self.filter.cols = self.col_filters.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(c, s)| (c, s.clone())).collect();
            self.memo = None;
            return;
        }
        self.refresh(ctx);
    }

    /// `enc`, `delim`: None leaves it as it is, Some(None) goes back to detecting it.
    pub fn reopen_with(&mut self, enc: Choice<Enc>, delim: Choice<u8>, ctx: &egui::Context) {
        if let Some(e) = enc { self.open_opts.enc = e; }
        if let Some(d) = delim { self.open_opts.delim = d; }
        let Some(path) = self.path.clone() else { return };
        // The bytes held are read again with the new options: no trip to the disk.
        let from = self.table.as_ref().map(|t| t.st.base.clone());
        // Line 1 as it was chosen; as it was detected (Auto), detected again in the new reading
        // when the delimiter changes (as the web's: raw text's line 1 may be titles as a CSV).
        let header = self.table.as_ref().map(|t| t.st.header).filter(|&h| delim.is_none() || self.table.as_ref().is_some_and(|t| h != csvfab_engine::table::detected_header(&t.st.base)));
        self.reread = true;
        self.loading = Some(Loader::spawn(path, self.open_opts, from, header, false, None, ctx));
    }

    /// After a save that already knows the new file's index
    /// (`save::save_indexed`, its base made `Ready` in the save job): shown as
    /// if read again, without reading it.
    pub fn adopt(&mut self, r: Ready, ctx: &egui::Context, m: &crate::theme::Metrics) {
        self.loading = None;
        self.install(r, ctx, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Waits for the view job, as `poll` picks it up.
    fn settle(t: &mut Tab) {
        while let Some(j) = t.job.as_mut() {
            if let Some(r) = j.poll() {
                t.job = None;
                if let Some(ids) = r { t.view = Some(Arc::new(ids)); }
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn expected(t: &Tab) -> Vec<RowId> {
        let st = &t.table.as_ref().unwrap().st;
        let no = AtomicBool::new(false);
        let mut spec = t.filter.clone();
        spec.cols = t.col_filters.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(c, s)| (c, s.clone())).collect();
        // Invert flips the text tests once column filters join the search; the values hidden
        // apply as they are.
        let whole = spec.invert && !spec.cols.is_empty();
        let all: Vec<RowId> = match &t.restrict { Some(r) => r.ids.to_vec(), None => (0..st.len()).map(|p| st.data_id(p)).collect() };
        let mut ids = if whole {
            let base = view::filter_ids(st, &view::Filter::compile(&spec.values()).unwrap(), &all, &no).unwrap();
            let pass: std::collections::HashSet<RowId> = view::filter_ids(st, &view::Filter::compile(&FilterSpec { invert: false, ..spec.texts() }).unwrap(), &base, &no).unwrap().into_iter().collect();
            base.into_iter().filter(|id| !pass.contains(id)).collect()
        } else {
            view::filter_ids(st, &view::Filter::compile(&spec).unwrap(), &all, &no).unwrap()
        };
        view::sort(st, &mut ids, &t.sort, &t.kinds, &no);
        ids
    }

    /// Invert flips the search and the column filters, never the values hidden or kept (as the
    /// web's); a value filter compares the exact text.
    #[test]
    fn invert_leaves_value_filters_alone() {
        let ctx = egui::Context::default();
        let mut t = Tab::blank();
        t.table = Some(Table::new(Arc::new(Base::from_bytes(b"name,city\na,Lyon\nb,Paris\nc,Lyon \nd,Nice\ne,Paris\n".to_vec(), OpenOptions::default())), None));
        t.analyse();
        t.col_filters = vec![String::new(); 2];
        let names = |t: &mut Tab| -> String {
            t.refresh(&ctx);
            settle(t);
            let st = t.st().unwrap().clone();
            (0..t.shown()).map(|p| st.cell(t.shown_id(p), 0)).collect()
        };
        t.filter.hidden = vec![(1, Arc::new(["Lyon".to_string()].into_iter().collect()))];
        assert_eq!(names(&mut t), "bcde", "\"Lyon \" is not \"Lyon\"");
        t.filter.invert = true;
        assert_eq!(names(&mut t), "bcde", "nothing to flip");
        t.col_filters[1] = "paris".into();
        assert_eq!(names(&mut t), "cd");
        t.filter.invert = false;
        t.filter.hidden.clear();
        t.keep_values(1, Some(view::Keeps::Values(Arc::new(["Paris".to_string(), "Nice".to_string()].into_iter().collect()))), &ctx);
        t.col_filters[1].clear();
        t.filter.global = "a".into();
        t.filter.invert = true;
        assert_eq!(names(&mut t), "d", "the search flipped within the values kept");
    }

    /// Typing, sorting, flipping, hiding, editing: every view as computed from scratch.
    #[test]
    fn refinements_match_full_runs() {
        let ctx = egui::Context::default();
        let mut csv = String::from("name,city,amount\n");
        let cities = ["Lyon", "Paris", "lyonnais", "Ly", "Villeurbanne", "", "Émile-Lyon"];
        for i in 0..5000 {
            csv.push_str(&format!("n{},{},{}\n", i % 97, cities[(i * 3 + i / 11) % cities.len()], (i * 37) % 1000));
        }
        let mut t = Tab::blank();
        t.table = Some(Table::new(Arc::new(Base::from_bytes(csv.into_bytes(), OpenOptions::default())), None));
        t.analyse();
        t.col_filters = vec![String::new(); 3];
        let check = |t: &mut Tab| {
            t.refresh(&ctx);
            settle(t);
            let got: Vec<RowId> = match &t.view { Some(v) => v.to_vec(), None => (0..t.st().unwrap().len()).map(|p| t.st().unwrap().data_id(p)).collect() };
            assert_eq!(got, expected(t), "{:?} {:?} {:?}", t.filter.global, t.col_filters, t.sort);
        };
        for g in ["l", "ly", "lyo", "lyon", "lyo", "lyonn"] {
            t.filter.global = g.into();
            check(&mut t);
        }
        t.filter.global = "l".into();
        check(&mut t);
        t.sort = vec![SortKey { col: 2, desc: false }];
        check(&mut t);
        t.sort[0].desc = true;
        check(&mut t);
        t.col_filters[1] = "l".into();
        check(&mut t);
        t.col_filters[1] = "ly".into();
        check(&mut t);
        t.col_filters[1] = "lyo".into();
        check(&mut t);
        t.sort = vec![SortKey { col: 0, desc: false }, SortKey { col: 2, desc: true }];
        check(&mut t);
        t.filter.hidden = vec![(1, Arc::new(["Lyon".to_string()].into_iter().collect()))];
        check(&mut t);
        t.filter.invert = true;
        check(&mut t);
        t.filter.global = "ly".into();
        check(&mut t);
        // An edit: the rows shown are no longer a superset.
        t.filter.global = "l".into();
        t.filter.invert = false;
        t.filter.hidden.clear();
        check(&mut t);
        let id = t.shown_id(0);
        t.table.as_mut().unwrap().set_cells(&[(id, 1, "zzz".into())]);
        t.filter.global = "ly".into();
        check(&mut t);
        t.restrict = Some(Restrict { label: "x".into(), ids: Arc::new((0..3000).rev().collect()) });
        check(&mut t);
        t.filter.global = "lyo".into();
        check(&mut t);
        t.sort.clear();
        check(&mut t);
        t.filter.global = "lyon".into();
        check(&mut t);
        // Typing faster than the jobs: each cancelled midway.
        t.restrict = None;
        for g in ["p", "pa", "par", "pari", "paris"] {
            t.filter.global = g.into();
            t.refresh(&ctx);
        }
        settle(&mut t);
        let got = t.view.as_ref().unwrap().to_vec();
        assert_eq!(got, expected(&t));
    }
}
