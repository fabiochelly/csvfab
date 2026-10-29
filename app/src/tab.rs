//! One open file: its table, the rows shown, and the background jobs that
//! read it and compute the view. The UI thread never waits on a job: it
//! polls their channels each frame and is woken by `request_repaint`.

use crate::job::Job;
use csvfab_engine::base::OpenOptions;
use csvfab_engine::look::{self, Pills, Rich};
use csvfab_engine::view::{self, FilterSpec, Kind, SortKey};
use csvfab_engine::{Base, Enc, RowId, State, Table};
use eframe::egui;
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
}

impl Ready {
    /// On a background thread: `header` is the table's when it is read again.
    pub fn new(base: Base, header: Option<bool>) -> Ready {
        let table = Table::new(Arc::new(base), header);
        let (kinds, rich, pills) = looks(&table.st);
        Ready { table, kinds, rich, pills }
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
    /// `head`: send the first records first (nothing is shown yet).
    fn spawn(path: PathBuf, opts: OpenOptions, from: Option<Arc<Base>>, header: Option<bool>, head: bool, ctx: &egui::Context) -> Loader {
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
    pub find_col: Option<usize>,
    pub look: crate::grid::ColLook,
    pub totals: crate::totals_ui::Choices,
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
    pub totals: crate::totals_ui::State,
    pub find: Find,
    pub ghost: Option<Arc<Ghost>>,
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
        self.after_edit(true, ctx);
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
            totals: Default::default(),
            find: Find::default(),
            ghost: None,
        }
    }

    /// Reads the file (again) in the background: on open, after a save, or with
    /// another encoding or delimiter. Filters and sort are kept. A big file
    /// shows its first rows at once, the whole index following.
    pub fn reload(&mut self, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else { return };
        let header = self.table.as_ref().map(|t| t.st.header).or(self.want_header);
        self.loading = Some(Loader::spawn(path, self.open_opts, None, header, self.table.is_none(), ctx));
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
        self.refresh(ctx);
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
        let st = t.st.clone();
        let ids: Vec<RowId> = rows.clone().take_while(|&p| p < self.shown()).map(|p| self.shown_id(p)).collect();
        self.sel_job = Some((key, crate::job::Job::spawn(ctx, move |cancel, _| {
            let (mut sum, mut n, mut tmp) = (0.0, 0usize, Vec::new());
            for (k, id) in ids.into_iter().enumerate() {
                if k % 4096 == 0 && cancel.load(Ordering::Relaxed) { return None; }
                for c in cols.clone() {
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

    /// Every row shown, in order (a copy when the view is the whole file).
    pub fn shown_ids(&self) -> Arc<Vec<RowId>> {
        match (&self.view, &self.table) {
            (Some(v), _) => v.clone(),
            (None, Some(t)) => Arc::new((0..t.st.len()).map(|p| t.st.data_id(p)).collect()),
            _ => Arc::default(),
        }
    }

    /// Whether filters, a sort or a tool's rows narrow or reorder the view.
    pub fn filtered(&self) -> bool {
        !self.filter.is_empty() || self.col_filters.iter().any(|s| !s.is_empty()) || !self.sort.is_empty() || self.restrict.is_some()
    }

    /// Hides (or shows again) one value of a column, as the column panel and the charts do.
    pub fn hide_values(&mut self, col: usize, values: std::collections::HashSet<String>, ctx: &egui::Context) {
        self.filter.hidden.retain(|(c, _)| *c != col);
        if !values.is_empty() { self.filter.hidden.push((col, Arc::new(values))); }
        self.grid.scroll = 0.0;
        self.refresh(ctx);
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
        let Some(t) = &self.table else { return };
        self.filter.cols = self.col_filters.iter().enumerate().filter(|(_, s)| !s.is_empty()).map(|(c, s)| (c, s.clone())).collect();
        self.generation += 1;
        // In formula mode the search box is an expression, applied before the other filters.
        let mut spec = self.filter.clone();
        let expr = if self.expr_mode && !spec.global.trim().is_empty() {
            match csvfab_engine::formula::compile(&spec.global, &t.st.headers()) {
                Ok(mut e) => { e.set_decimal_comma(csvfab_engine::formula::decimal_comma(&t.st)); Some(e) }
                Err(e) => { self.filter_error = Some(e.msg); return; }
            }
        } else { None };
        let expr_text = expr.as_ref().map(|_| spec.global.clone());
        if self.expr_mode { spec.global.clear(); }
        // Invert flips the whole test, as the web's: column filters, values hidden and the
        // expression with the search. With the search alone the engine inverts it itself.
        let whole = spec.invert && (!spec.cols.is_empty() || spec.hidden.iter().any(|(_, h)| !h.is_empty()) || expr.is_some());
        let fspec = if whole { FilterSpec { invert: false, ..spec.clone() } } else { spec.clone() };
        *self.expr_errors.lock().unwrap() = None;
        let f = match view::Filter::compile(&fspec) {
            Ok(f) => { self.filter_error = None; f }
            Err(e) => { self.filter_error = Some(e); return; }
        };
        if f.is_empty() && expr.is_none() && self.sort.is_empty() && self.restrict.is_none() {
            self.view = None;
            self.memo = None;
            self.view_time = None;
            self.grid.clamp(self.shown());
            return;
        }
        let st = t.st.clone();
        // The column caches these read, built in the background for the next keystroke.
        f.warm(&st, &self.kinds);
        view::warm_sort(&st, &self.sort, &self.kinds);
        let (sort, kinds) = (self.sort.clone(), self.kinds.clone());
        let within = self.restrict.as_ref().map(|r| r.ids.clone());
        let memo = ViewMemo {
            spec: spec.clone(),
            expr: expr_text,
            restrict: within.clone(),
            sort: sort.clone(),
            kinds: kinds.clone(),
            state: state_key(&st),
            view: None,
            filtered: Default::default(),
        };
        // Where to start from: the rows shown (already in this sort's order), or
        // the same rows unsorted, when the new filter keeps no row they lack.
        let start = self.memo.as_ref().filter(|m| {
            !whole
                && m.view.is_some()
                && m.view.as_ref().map(Arc::as_ptr) == self.view.as_ref().map(Arc::as_ptr)
                && m.expr == memo.expr
                && m.restrict.as_ref().map(Arc::as_ptr) == within.as_ref().map(Arc::as_ptr)
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
        let keep_filtered = !sort.is_empty() && (!f.is_empty() || expr.is_some() || within.is_some());
        let errors = self.expr_errors.clone();
        self.pending = Some(memo);
        self.job = Some(Job::spawn(ctx, move |cancel, _| {
            let (mut ids, sorted) = match start {
                Some((rows, sorted, prev_filtered, same)) => {
                    let ids = if same { rows.to_vec() } else { view::filter_ids(&st, &f, &rows, cancel)? };
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
                None => {
                    let universe = within.clone();
                    let within = match &expr {
                        Some(e) => {
                            let o = csvfab_engine::formula::filter(&st, e, within.as_deref().map(|v| v.as_slice()), cancel)?;
                            if o.errors > 0 { *errors.lock().unwrap() = Some((o.errors, o.first_error.unwrap_or_default())); }
                            Some(Arc::new(o.out))
                        }
                        None => within,
                    };
                    let mut ids = match &within {
                        None => view::filter(&st, &f, cancel)?,
                        Some(ids) => view::filter_ids(&st, &f, ids, cancel)?,
                    };
                    if whole {
                        // The rows that fail the test, in their order.
                        let pass = csvfab_engine::table::RowMask::new(ids.iter().copied());
                        ids = match &universe {
                            Some(u) => u.iter().copied().filter(|&id| !pass.contains(id)).collect(),
                            None => (0..st.len()).map(|p| st.data_id(p)).filter(|&id| !pass.contains(id)).collect(),
                        };
                    }
                    (ids, false)
                }
            };
            if !sorted {
                if keep_filtered { *slot.lock().unwrap() = Some(Arc::new(ids.clone())); }
                if !view::sort(&st, &mut ids, &sort, &kinds, cancel) { return None; }
            }
            Some(ids)
        }));
    }

    /// Shows only these rows (in this order), filters applying within them.
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
        t.set_cells(changes);
        self.generation += 1;
        self.after_edit(grow, ctx);
    }

    pub fn col_view(&self) -> ColView {
        ColView {
            filters: self.col_filters.clone(),
            widths: self.grid.widths.clone(),
            sort: self.sort.clone(),
            hidden_vals: self.filter.hidden.clone(),
            find_col: self.find.col,
            look: self.grid.col_look(),
            totals: self.totals.choices(),
        }
    }

    /// Puts column settings back (made for the table's present columns).
    pub fn set_col_view(&mut self, v: ColView) {
        let n = v.filters.len();
        self.col_filters = v.filters;
        self.grid.widths = v.widths;
        self.sort = v.sort;
        self.filter.hidden = v.hidden_vals;
        self.find.col = v.find_col;
        self.grid.set_col_look(v.look, n);
        self.totals.set_choices(v.totals);
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

    /// `enc`, `delim`: None leaves it as it is, Some(None) goes back to detecting it.
    pub fn reopen_with(&mut self, enc: Choice<Enc>, delim: Choice<u8>, ctx: &egui::Context) {
        if let Some(e) = enc { self.open_opts.enc = e; }
        if let Some(d) = delim { self.open_opts.delim = d; }
        let Some(path) = self.path.clone() else { return };
        // The bytes held are read again with the new options: no trip to the disk.
        let from = self.table.as_ref().map(|t| t.st.base.clone());
        let header = self.table.as_ref().map(|t| t.st.header);
        self.reread = true;
        self.loading = Some(Loader::spawn(path, self.open_opts, from, header, false, ctx));
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
        // Invert flips the whole test once column filters or hidden values join the search.
        let whole = spec.invert && (!spec.cols.is_empty() || spec.hidden.iter().any(|(_, h)| !h.is_empty()));
        if whole { spec.invert = false; }
        let f = view::Filter::compile(&spec).unwrap();
        let mut ids = match &t.restrict {
            Some(r) => view::filter_ids(st, &f, &r.ids, &no).unwrap(),
            None => view::filter(st, &f, &no).unwrap(),
        };
        if whole {
            let pass: std::collections::HashSet<RowId> = ids.into_iter().collect();
            let all: Vec<RowId> = match &t.restrict { Some(r) => r.ids.to_vec(), None => (0..st.len()).map(|p| st.data_id(p)).collect() };
            ids = all.into_iter().filter(|id| !pass.contains(id)).collect();
        }
        view::sort(st, &mut ids, &t.sort, &t.kinds, &no);
        ids
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
