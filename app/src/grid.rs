//! The grid: drawn by hand, row by row, only what is on screen. Scrolling is
//! kept as f64 pixels (`GridState::scroll`): 20 million rows of 32 px is 640
//! million pixels, past what an f32 holds exactly (egui's ScrollArea would
//! drift); the first row and its offset are derived from it each frame, the
//! offset on a physical pixel. The look follows docs/look.md (the web app's,
//! measured): stripes by record, category dots, rich values (yes / no, colours, links,
//! JSON), colour scales and data bars, marks, search and find highlights,
//! fill-by-example ghosts, groups of a sorted view, frozen columns.
//!
//! Nothing here grows with the file on the UI thread: every pass over the rows
//! (charts, totals, colour scales, fills, strip marks) is a `Job`, and until it
//! ends the previous picture stays — never a frame drawn differently and back.

use crate::colpanel;
use crate::fonts;
use crate::icons::{self, Icon};
use crate::job::Job;
use crate::tab::{Ghost, Tab};
use crate::theme::{alpha, mix, Metrics, Palette};
use csvfab_engine::edit::series;
use csvfab_engine::look::{self, DataBars, Heat, Pills, Rich};
use csvfab_engine::sparkline::{Keep, Spark};
use csvfab_engine::text::fold_into;
use csvfab_engine::table::Patch;
use csvfab_engine::view::{Kind, SortKey};
use csvfab_engine::{RowId, State};
use eframe::egui;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontFamily, FontId, Id, Key, Modifiers, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

pub struct Editing {
    pub at: Cell,
    pub id: RowId,
    pub text: String,
    /// Typing on a cell replaces its text; Enter/F2 keep it with the caret at the end.
    pub fresh: bool,
    /// Enter: the whole text selected, as the web's (F2, a double-click: the caret at the end).
    pub select: bool,
}

/// A colour scale or data bars being computed for a column (over every row, as the JS).
enum LookRes {
    Heat(Option<Heat>),
    Bars(Option<DataBars>),
}

/// A strip picture computed in the background: per pixel row of the vertical strip.
struct StripPic<K> {
    key: K,
    data: Vec<u32>,
}

/// What the search highlights were made from (`Hl`), to make them again only when it changes.
#[derive(PartialEq)]
struct HlKey {
    global: String,
    cols: Vec<String>,
    regex: bool,
    case: bool,
    off: bool,
}

#[derive(Default)]
pub struct GridState {
    /// The search's and the column filters' highlights (a regex compiled once, not every frame).
    hl: Option<(HlKey, Arc<(Hl, Vec<Hl>)>, u64)>,
    /// A regex find's pattern, by its text and case.
    find_re: Option<((String, bool), Option<Arc<csvfab_engine::re::Re>>)>,
    /// The navigator held down, and whether the pointer has reached the magnified map
    /// since (the view then follows the pointer on it).
    nav_hold: Option<bool>,
    /// Where the view is scrolled to, in pixels from the first row: f64 holds
    /// the pixel exactly far past a billion rows, so the wheel, the strip and the
    /// keys all move smoothly by the pixel, never by the row.
    pub scroll: f64,
    /// Derived from `scroll` each frame: the first row on screen and how much of it is above the body.
    pub top: usize,
    off: f32,
    row_h: f32,
    body_h: f32,
    pub xoff: f32,
    pub widths: Vec<f32>,
    /// Columns hidden (the columns manager): skipped by the layout, the drawing, the keys and
    /// the copy; a view setting, kept across filters. The status bar shows how many.
    pub hidden: std::collections::BTreeSet<usize>,
    /// The columns manager while its popup is open.
    pub manager: Option<colpanel::Manager>,
    /// Scroll to the cursor's column next frame (the manager's names).
    pub reveal_col: bool,
    /// Asked from the palette: this column's panel (opened once its title is drawn), the
    /// columns manager, a series down the selection (Ctrl+D).
    pub open_panel: Option<usize>,
    pub open_manager: bool,
    pub want_fill_down: bool,
    /// A word for the user from the grid (why something was refused): the app toasts it.
    pub notice: Option<String>,
    pub cursor: Option<Cell>,
    pub anchor: Option<Cell>,
    /// The column in hand: the last title clicked or the cursor's. Faintly tinted; the
    /// inspector follows it.
    pub focus_col: Option<usize>,
    /// The cursor's column last frame: a move of it brings `focus_col` along.
    cursor_col: Option<usize>,
    pub editing: Option<Editing>,
    selecting: bool,
    /// The drag selecting started on a row number: whole rows.
    selecting_rows: bool,
    /// A row dragged by its number (07-columns-structure.js "Move rows"): its id, and where
    /// it would land — the row shown at that position, and whether after it.
    row_drag: Option<(RowId, Option<(usize, bool)>)>,
    /// A title being renamed in place: (column, the text typed, focused yet).
    pub renaming: Option<(usize, String, bool)>,
    /// The sort before a title's first click: a double click (rename) puts it back.
    pub pre_click_sort: Option<(usize, Vec<SortKey>)>,
    resizing: Option<(usize, f32, f32)>,
    thumb_grab: Option<f32>,
    hthumb_grab: Option<f32>,
    pub menu_at: Option<Cell>,
    /// Rows that fit on screen (last frame).
    pub page: usize,
    pub focus_filter: Option<usize>,
    /// Columns frozen at the left (0: none), counted from the first.
    pub frozen: usize,
    /// Colour scales and data bars, by column (the column panel's toggles).
    pub heat: HashMap<usize, Heat>,
    pub bars: HashMap<usize, DataBars>,
    look_jobs: Vec<(Job<Option<LookRes>>, usize)>,
    /// The column panel while its popup is open.
    pub panel: Option<colpanel::Panel>,
    /// The row a column panel's "go to" looks for (the row shown, its column), in the background.
    pub goto_job: Option<(Job<Option<(usize, usize)>>, usize)>,
    /// A fill (handle, Ctrl+D, Ctrl+Enter) computing its cells.
    fill_job: Option<Job<Option<Vec<(RowId, usize, String)>>>>,
    /// A big selection cleared or given one value (Delete, Enter on a block, a value pasted),
    /// its patch built in the background; true: told as a paste.
    patch_job: Option<(Job<Option<Patch>>, bool)>,
    /// The fill handle being dragged: the selection, and the cell under the pointer.
    fill_drag: Option<(series::Rect, Option<Cell>)>,
    /// A title being dragged to another place.
    col_drag: Option<usize>,
    /// The cell under the pointer since when, and whether its value shows cut (the peek).
    peek: Option<(Cell, Instant)>,
    /// Whether the sorted view has groups: (view, length, generation, column) → yes / no.
    groups: Option<((usize, usize, u64, usize), bool)>,
    /// The row ⋮ menu: the row and the button it opened from.
    row_menu: Option<(RowId, usize, Rect)>,
    /// The width the columns get (for the frozen columns' limit).
    view_w: f32,
    ncols_seen: usize,
    /// The sort column's strip labels kind: (column, base) → kind.
    sort_kind: Option<((usize, usize), Kind)>,
    marks_pic: Option<StripPic<(usize, usize, u64, usize, u32)>>,
    marks_job: Option<(Job<Option<Vec<u8>>>, (usize, usize, u64, usize, u32))>,
    glow_pic: Option<StripPic<(usize, String, usize, u32)>>,
    glow_job: Option<(Job<Option<Vec<u32>>>, (usize, String, usize, u32))>,
    find_pic: Option<StripPic<(usize, usize, u32)>>,
    /// With CSVFAB_GRID_TIMING: the frame's time, then its parts (input, rows, over, header, charts, totals, strips).
    timing: (Duration, u32, [Duration; 7]),
    /// The galleys of the cells on screen, kept from frame to frame.
    cells: Cells,
    /// Cells just edited, since when (they flash).
    flash: FxMap<(RowId, u32), Instant>,
    /// The rows drawn last frame (y from the body's top, as drawn), of which view, at which
    /// scroll; the sort that view had: what a motion starts from.
    drawn: Vec<(RowId, f32)>,
    drawn_key: (usize, usize),
    drawn_scroll: f64,
    view_sort: Vec<SortKey>,
    motion: Option<motion::Motion>,
    fling: Fling,
    wheel: Wheel,
    /// A column easing to its fitted width (double-click on its edge): (column, from, to, since).
    width_to: Option<(usize, f32, f32, Instant)>,
}

/// What the grid asks the app to do (the app owns the table, its undo and dialogs).
pub enum Action {
    SetCells(Vec<(RowId, usize, String)>),
    /// Cells made ready as a patch (a selection cleared or filled); `pasted`: told as a paste.
    Patch { patch: Patch, pasted: bool },
    Sort(Vec<SortKey>),
    FiltersChanged,
    InsertRows { before: usize, count: usize },
    /// A row dragged by its number, dropped next to `target` (after it when `after`).
    MoveRow { id: RowId, target: RowId, after: bool },
    DeleteRows(Vec<RowId>),
    InsertColumn(usize),
    DeleteColumns(Vec<usize>),
    /// An action already confirmed (a deletion asked about first): run as is.
    Sure(Box<Action>),
    RenameColumn(usize),
    /// A column renamed in its title (the in-place field).
    RenameTo(usize, String),
    /// The row card, open on the row as it sits in the file.
    RowCardRaw,
    /// A command of the registry, by name (a menu's way to a tool).
    RunCommand(&'static str),
    /// Go to the row of id `id`; `clear`: the filters hid it, clear them first.
    GoToRow { id: RowId, clear: bool },
    /// Keep (or hide) the rows holding this value in this column.
    FilterValue { col: usize, value: String, keep: bool },
    Copy,
    Paste(String),
    /// From a menu: ask the system for the clipboard, which comes back as a paste event.
    RequestPaste,
    RowCard,
    ToggleFilters,
    ToggleCharts,
    ToggleTotals,
    ToggleInspector,
    FitWidth(usize),
    /// The column panel's Split / Merge / Convert buttons and the title's menu: open
    /// that tool on this column (the edit-tools agent's dialogs).
    SplitColumn(usize),
    MergeColumns(usize),
    ConvertColumn(usize),
    /// "Why does this row look wrong?" (row ⋮ and cell menus): `tools::diagnose::open(tab, id)`.
    Diagnose(RowId),
    /// A duplicate row's bar (delete, merge) or a differing cell's arrows: `tools::dupes::apply`.
    Dup(crate::tools::dupes::DupAct),
    /// A tool to open (the column panel's Group by / Pivot table by this column).
    Tool(Box<dyn crate::tools::Tool>),
}

fn mono(m: &Metrics) -> FontId {
    FontId::monospace(m.font_grid)
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(fonts::UI_SEMIBOLD.into()))
}

pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(fonts::MONO_BOLD.into()))
}

/// Width of one character of the grid font.
fn char_w(ctx: &egui::Context, m: &Metrics) -> f32 {
    ctx.fonts_mut(|f| f.glyph_width(&mono(m), '0'))
}

impl GridState {
    /// No edit of the grid's own under way (a fill computing).
    pub fn idle(&self) -> bool {
        self.fill_job.is_none() && self.patch_job.is_none()
    }

    /// Auto widths, as the web app: the widest of the first 80 rows (mono advance × chars)
    /// + 21, clamped to 60…480.
    pub fn reset_widths(&mut self, st: &State, ctx: &egui::Context, m: &Metrics) {
        self.widths.clear();
        self.widths.resize(st.ncols, 120.0);
        let cw = char_w(ctx, m);
        let heads = st.headers();
        let n = st.len().min(80);
        let mut tmp = Vec::new();
        for (c, w) in self.widths.iter_mut().enumerate() {
            let mut chars = 0usize;
            for p in 0..n {
                st.with_cell(st.data_id(p), c, &mut tmp, |s| chars = chars.max(columns_of(s).min(200)));
            }
            // The title whole: its measured width, the kind badge before it, its number after
            // it, the ▾'s room (a column is never narrower than its name).
            let title = heads.get(c).map_or(0.0, |h| ctx.fonts_mut(|f| f.layout_no_wrap(h.clone(), semibold(m.font_header), Color32::WHITE).size().x).max(28.0) + m.cell_pad_x + BADGE_W + 7.0 + num_w(c) + m.col_title_pad_r + 8.0);
            *w = (chars as f32 * cw + 21.0).max(title.min(m.col_auto_max)).clamp(m.col_min_w, m.col_auto_max);
        }
        if st.base.dialect.lines && st.ncols > 0 { self.widths[0] = self.text_col_width(st, ctx, m); }
    }

    /// A text file's one column (the web's textColWidth): the longest line among the first
    /// 5 000 and the shown ones (tabs as 4), at least the view's width — a wide column
    /// scrolls sideways as an editor does — at most 30 000 px.
    fn text_col_width(&self, st: &State, ctx: &egui::Context, m: &Metrics) -> f32 {
        let (len, cw) = (st.len(), char_w(ctx, m));
        let (mut n, mut tmp) = (0usize, Vec::new());
        for p in (0..len.min(5000)).chain(self.top.min(len)..(self.top + 200).min(len)) {
            st.with_cell(st.data_id(p), 0, &mut tmp, |s| if s.len() > n { n = n.max(crate::syntax::columns(s)) });
        }
        // The view before the first frame: the window less the row numbers and the scrollbar.
        let view = if self.view_w > 0.0 { self.view_w } else { ctx.content_rect().width() - m.gutter_w - m.strip_w };
        view.max((n as f32 * cw + 40.0).min(30000.0))
    }

    pub fn fit_count(&mut self, n: usize) {
        self.widths.resize(n, 120.0);
        // Fewer columns (a deletion, an insertion undone): nothing left pointing past the last.
        if n == 0 {
            (self.cursor, self.anchor) = (None, None);
        } else {
            for c in [&mut self.cursor, &mut self.anchor].into_iter().flatten() { c.col = c.col.min(n - 1); }
        }
        if self.editing.as_ref().is_some_and(|e| e.at.col >= n) { self.editing = None; }
        if self.menu_at.is_some_and(|c| c.col >= n) { self.menu_at = None; }
        if self.peek.is_some_and(|(c, _)| c.col >= n) { self.peek = None; }
        if self.col_drag.is_some_and(|c| c >= n) { self.col_drag = None; }
        if self.focus_filter.is_some_and(|c| c >= n) { self.focus_filter = None; }
        if self.focus_col.is_some_and(|c| c >= n) { self.focus_col = None; }
        self.frozen = self.frozen.min(n);
        self.hidden.retain(|&c| c < n);
        self.heat.retain(|&c, _| c < n);
        self.bars.retain(|&c, _| c < n);
    }

    /// Goes to column `c` (a hidden one shown again, as the web's `goToColumn`), the cursor on
    /// it in `row`, scrolled into view.
    pub fn go_to_col(&mut self, row: usize, c: usize) {
        self.hidden.retain(|&k| k != c);
        self.select(Cell { row, col: c }, false);
        self.reveal_col = true;
    }

    /// Scrolls so that `row` is the first one shown.
    pub fn set_top(&mut self, row: usize) {
        self.scroll = row as f64 * self.rh() as f64;
    }

    fn rh(&self) -> f32 {
        if self.row_h > 0.0 { self.row_h } else { Metrics::default().row_h }
    }

    /// Column `c`'s width as laid out: 0 when hidden.
    #[inline]
    pub fn w(&self, c: usize) -> f32 {
        if self.hidden.contains(&c) { 0.0 } else { self.widths.get(c).copied().unwrap_or(0.0) }
    }

    /// The widths as laid out (hidden columns 0).
    fn laid(&self) -> Vec<f32> {
        (0..self.widths.len()).map(|c| self.w(c)).collect()
    }

    /// The columns shown, in order (what a copy takes).
    pub fn shown_cols(&self, n: usize) -> Vec<usize> {
        (0..n).filter(|c| !self.hidden.contains(c)).collect()
    }

    fn max_scroll(&self, rows: usize) -> f64 {
        (rows as f64 * self.rh() as f64 - self.body_h as f64).max(0.0)
    }

    /// Clamps the scroll and derives the first row and its offset, the offset
    /// on a physical pixel so text and rules stay crisp while scrolling.
    fn sync(&mut self, rows: usize, ppp: f32) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(rows));
        let rh = self.rh() as f64;
        self.top = (self.scroll / rh).floor() as usize;
        let off = (self.scroll - self.top as f64 * rh) as f32;
        self.off = (off * ppp).round() / ppp;
    }

    pub fn clamp(&mut self, rows: usize) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(rows));
        self.top = self.top.min(rows.saturating_sub(1));
        for c in [&mut self.cursor, &mut self.anchor].into_iter().flatten() {
            c.row = c.row.min(rows.saturating_sub(1));
        }
    }

    /// The selected range: (rows, cols), inclusive.
    pub fn range(&self) -> Option<(std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>)> {
        let c = self.cursor?;
        let a = self.anchor.unwrap_or(c);
        Some((a.row.min(c.row)..=a.row.max(c.row), a.col.min(c.col)..=a.col.max(c.col)))
    }

    fn block(&self) -> Option<series::Rect> {
        self.range().map(|(r, c)| series::Rect { r0: *r.start(), r1: *r.end(), c0: *c.start(), c1: *c.end() })
    }

    pub fn select(&mut self, c: Cell, extend: bool) {
        if !extend || self.anchor.is_none() {
            self.anchor = Some(c);
        }
        self.cursor = Some(c);
    }

    /// Scrolls just enough to show the cursor (rows, and columns right of the frozen ones).
    fn reveal(&mut self, rows: usize, view_w: f32) {
        let Some(c) = self.cursor else { return };
        let rh = self.rh() as f64;
        let (y0, y1) = (c.row as f64 * rh, (c.row + 1) as f64 * rh);
        if y0 < self.scroll {
            self.scroll = y0;
        } else if y1 > self.scroll + self.body_h as f64 {
            self.scroll = y1 - self.body_h as f64;
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(rows));
        let f = self.frozen.min(self.widths.len());
        if c.col < self.widths.len() && c.col >= f {
            let x0: f32 = (f..c.col).map(|k| self.w(k)).sum();
            let x1 = x0 + self.w(c.col);
            if x0 < self.xoff { self.xoff = x0; } else if x1 > self.xoff + view_w { self.xoff = x1 - view_w; }
        }
    }

    /// What is tied to column positions here: hidden and frozen columns, colour scales, data bars.
    pub fn col_look(&self) -> ColLook {
        ColLook { hidden: self.hidden.clone(), frozen: self.frozen, heat: self.heat.clone(), bars: self.bars.clone() }
    }

    /// Puts a look back, made for `ncols` columns: the reset a new column count brings is skipped.
    pub fn set_col_look(&mut self, l: ColLook, ncols: usize) {
        self.hidden = l.hidden;
        self.frozen = l.frozen;
        self.heat = l.heat;
        self.bars = l.bars;
        self.panel = None;
        self.look_jobs.clear();
        self.ncols_seen = ncols;
    }

    /// Forgets what was tied to column positions (columns added or removed).
    fn reset_columns(&mut self) {
        self.heat.clear();
        self.bars.clear();
        self.frozen = 0;
        self.hidden.clear();
        self.panel = None;
        self.look_jobs.clear();
    }
}

/// The grid's settings tied to column positions (`GridState::col_look`).
#[derive(Clone, Default)]
pub struct ColLook {
    pub hidden: std::collections::BTreeSet<usize>,
    pub frozen: usize,
    pub heat: HashMap<usize, Heat>,
    pub bars: HashMap<usize, DataBars>,
}

/// The column shown next to `c` toward `dir` (±1), else `c` (hidden columns stepped over).
fn step_col(hidden: &std::collections::BTreeSet<usize>, c: usize, dir: i64, n: usize) -> usize {
    let mut k = c as i64 + dir;
    while k >= 0 && (k as usize) < n {
        if !hidden.contains(&(k as usize)) { return k as usize; }
        k += dir;
    }
    c
}

/// French-style thousands: 1 234 567, with narrow no-break spaces.
pub fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 * 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 { out.push('\u{202f}'); }
        out.push(ch);
    }
    out
}

/// One line of a cell: line breaks and tabs shown as symbols, long text cut.
pub fn display(s: &str) -> std::borrow::Cow<'_, str> {
    let s = if s.len() > 400 { &s[..s.floor_char_boundary(400)] } else { s };
    if s.bytes().any(|b| b == b'\n' || b == b'\r' || b == b'\t') {
        s.replace("\r\n", "↵").replace(['\n', '\r'], "↵").replace('\t', "→").into()
    } else {
        s.into()
    }
}

pub fn is_link(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://") || (s.contains('@') && !s.contains(' ') && s.contains('.') && s.len() < 200)
}

/// A search's marks in a cell: its words (folded, or as typed with Match case), or its regex.
#[derive(Default)]
struct Hl {
    words: Vec<String>,
    case: bool,
    re: Option<csvfab_engine::re::Re>,
}

impl Hl {
    fn new(text: &str, regex: bool, case: bool) -> Hl {
        if text.trim().is_empty() { return Hl::default(); }
        if regex {
            return Hl { re: csvfab_engine::re::Re::new(text, !case, 1 << 20).ok(), ..Default::default() };
        }
        let words = csvfab_engine::view::terms(text).into_iter().map(|w| if case { w } else { csvfab_engine::text::fold(&w) }).filter(|w| !w.is_empty()).collect();
        Hl { words, case, re: None }
    }

    fn is_empty(&self) -> bool {
        self.words.is_empty() && self.re.is_none()
    }

    /// Its ranges in `s`, into `out` (cleared); `tmp` a buffer.
    fn hits(&self, s: &str, out: &mut Vec<(usize, usize)>, tmp: &mut Vec<(usize, usize)>) {
        out.clear();
        if let Some(re) = &self.re {
            spans(re, s, out);
            return;
        }
        for w in &self.words {
            if self.case {
                out.extend(s.match_indices(w.as_str()).take(16).map(|(a, t)| (a, a + t.len())));
            } else {
                matches(s, w, tmp);
                out.extend_from_slice(tmp);
            }
        }
    }
}

/// A regex's first 16 non-empty matches in `s`, appended to `out`.
fn spans(re: &csvfab_engine::re::Re, s: &str, out: &mut Vec<(usize, usize)>) {
    let mut k = 0;
    re.for_each_match(s, |a, b| if b > a && k < 16 { out.push((a, b)); k += 1; });
}

/// Byte ranges of `s` where the folded `needle` matches (accents and case ignored).
fn matches(s: &str, needle: &str, out: &mut Vec<(usize, usize)>) {
    out.clear();
    if needle.is_empty() { return; }
    let mut folded = String::with_capacity(s.len());
    let mut origin: Vec<usize> = Vec::with_capacity(s.len() + 1);
    let mut one = String::new();
    for (i, ch) in s.char_indices() {
        one.clear();
        fold_into(ch.encode_utf8(&mut [0; 4]), &mut one);
        for _ in 0..one.len() { origin.push(i); }
        folded.push_str(&one);
    }
    origin.push(s.len());
    let mut from = 0;
    while let Some(k) = folded[from..].find(needle) {
        let a = from + k;
        let b = a + needle.len();
        let oa = origin[a];
        let ob = origin[b.min(origin.len() - 1)];
        let ob = if ob <= oa { s[oa..].chars().next().map_or(s.len(), |c| oa + c.len_utf8()) } else { ob };
        out.push((oa, ob));
        from = b.max(a + 1);
        if out.len() > 16 { break; }
    }
}

/// A value as the peek shows it: JSON indented when it is an object or a list, at most 4 000 characters.
fn peek_text(v: &str) -> String {
    let t = v.trim();
    let mut s = if (t.starts_with('{') || t.starts_with('[')) && t.len() < 200_000 && look::json_object(t) { pretty_json(t) } else { v.to_string() };
    if s.chars().count() > 4000 {
        s = s.chars().take(4000).collect::<String>() + "…";
    }
    s
}

/// JSON re-indented by two spaces (as `JSON.stringify(j, null, 2)`), strings kept as written.
fn pretty_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    let nl = |out: &mut String, d: usize| { out.push('\n'); for _ in 0..d { out.push_str("  "); } };
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if in_str {
            out.push(ch);
            if esc { esc = false } else if ch == '\\' { esc = true } else if ch == '"' { in_str = false }
            i += 1;
            continue;
        }
        match ch {
            '"' => { in_str = true; out.push(ch); }
            '{' | '[' => {
                // An empty object or list stays on its line.
                let close = if ch == '{' { '}' } else { ']' };
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() { j += 1; }
                if j < chars.len() && chars[j] == close { out.push(ch); out.push(close); i = j + 1; continue; }
                out.push(ch);
                depth += 1;
                nl(&mut out, depth);
            }
            '}' | ']' => { depth = depth.saturating_sub(1); nl(&mut out, depth); out.push(ch); }
            ',' => { out.push(','); nl(&mut out, depth); }
            ':' => out.push_str(": "),
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// How long the pointer rests before a tip shows (egui's own tooltips: the same, theme.rs).
pub const TIP_DELAY: f64 = 0.3;

/// A tooltip box (`theme::tooltip_frame`, ui 11.5 / 1.45 `tip_fg`) under `anchor`, centred on
/// it, above when there is no room, kept 6 px inside the window. Asked for every frame the
/// pointer is on its element; shown after `TIP_DELAY` — at once while one was already
/// showing the frame before (sliding along a chart, from one button to the next).
pub fn tip(ctx: &egui::Context, p: &Palette, m: &Metrics, anchor: Rect, text: &str) {
    let id = Id::unique("csvfab-tip-rest");
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    let (last, since) = ctx.data(|d| d.get_temp::<(u64, f64)>(id)).unwrap_or((0, now));
    let since = if last + 1 >= pass && last != 0 { since } else { now };
    ctx.data_mut(|d| d.insert_temp(id, (pass, since)));
    let wait = TIP_DELAY - (now - since);
    if wait > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        return;
    }
    tip_box(ctx, p, m, anchor, text, FontId::proportional(11.5), 11.5 * 1.45, 420.0, vec2(8.0, 4.0), false);
}

#[allow(clippy::too_many_arguments)]
fn tip_box(ctx: &egui::Context, p: &Palette, m: &Metrics, anchor: Rect, text: &str, font: FontId, line_h: f32, max_w: f32, pad: Vec2, left: bool) {
    // A menu or a column's panel is open: nothing under it, nor beside it in the grid, shows
    // its tooltip (egui's rule for its own); a tooltip of the open panel's own still does.
    if egui::Popup::is_any_open(ctx) && ctx.layer_id_at(anchor.center()).is_none_or(|l| l.order == egui::Order::Background) {
        return;
    }
    let fmt = TextFormat { font_id: font, color: p.tip_fg, line_height: Some(line_h), ..Default::default() };
    let mut job = LayoutJob::single_section(text.to_owned(), fmt);
    job.wrap = TextWrapping { max_width: max_w - 2.0 * pad.x, break_anywhere: false, ..Default::default() };
    let g = ctx.fonts_mut(|f| f.layout_job(job));
    let size = g.size() + 2.0 * pad;
    let screen = ctx.content_rect();
    let ppp = ctx.pixels_per_point();
    let x = if left { anchor.min.x } else { anchor.center().x - size.x / 2.0 }.clamp(screen.min.x + 6.0, (screen.max.x - 6.0 - size.x).max(screen.min.x + 6.0));
    let below = anchor.max.y + if left { 6.0 } else { 10.0 };
    let y = if below + size.y <= screen.max.y - 6.0 { below } else { (anchor.min.y - 10.0 - size.y).max(screen.min.y + 6.0) };
    let r = Rect::from_min_size(pos2((x * ppp).round() / ppp, (y * ppp).round() / ppp), size);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("csvfab-tip")));
    let frame = crate::theme::tooltip_frame(p, m);
    painter.add(frame.paint(r.shrink2(pad)));
    painter.galley(r.min + pad, g, p.tip_fg);
}

/// Moves a column (the panel's ‹ ›, a title dragged), as one undo step; the view's settings
/// tied to column positions follow it.
pub fn move_column(tab: &mut Tab, from: usize, to: usize, ctx: &egui::Context) {
    if tab.grid.focus_col == Some(from) { tab.grid.focus_col = Some(to); }
    let Some(t) = &mut tab.table else { return };
    let n = t.st.ncols;
    if from >= n || to >= n || from == to { return; }
    t.move_column(from, to);
    let mut order: Vec<usize> = (0..n).collect();
    let f = order.remove(from);
    order.insert(to, f);
    let new = |c: usize| order.iter().position(|&x| x == c);
    crate::tools::edit_kit::remap_columns(tab, n, new, ctx);
    let name = tab.heads().get(to).cloned().unwrap_or_default();
    tab.grid.notice = Some(format!("Column \"{name}\" moved — not written yet, use Save."));
}

/// Freezes every column up to `col` — or none, when they already are. Refused when they
/// would take most of the grid: nothing would be left to scroll.
pub fn freeze_to(tab: &mut Tab, col: usize) {
    let g = &mut tab.grid;
    if g.frozen == col + 1 { g.frozen = 0; g.notice = Some("Columns unfrozen.".into()); return; }
    // "The first column": the first shown (a hidden column 0 is not the one in view).
    let ncols = g.widths.len();
    let col = if col == 0 { (0..ncols).find(|c| !g.hidden.contains(c)).unwrap_or(0) } else { col };
    let w: f32 = (0..=col).map(|c| g.w(c)).sum();
    if g.view_w > 0.0 && w > g.view_w * 0.8 {
        g.notice = Some("Not frozen: those columns would fill most of the window. Narrow them, or freeze fewer.".into());
        return;
    }
    g.frozen = col + 1;
    g.xoff = 0.0;
    let name = tab.heads().get(col).cloned().unwrap_or_default();
    tab.grid.notice = Some(if col == 0 || tab.grid.frozen == 1 { format!("Column \"{name}\" frozen: it stays in view when scrolling sideways.") } else { format!("Columns frozen up to \"{name}\": they stay in view when scrolling sideways.") });
}

/// The column panel's data bars (`heat` false) or colour scale: on (computed over every row,
/// in the background) or off.
pub fn toggle_look(tab: &mut Tab, col: usize, heat: bool, ctx: &egui::Context) {
    let map_has = if heat { tab.grid.heat.remove(&col).is_some() } else { tab.grid.bars.remove(&col).is_some() };
    if map_has { return; }
    let Some(st) = tab.st().cloned() else { return };
    tab.grid.look_jobs.push((Job::spawn(ctx, move |cancel, _| {
        let ids: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
        Some(if heat { LookRes::Heat(look::heat(&st, &ids, col, cancel)?) } else { LookRes::Bars(look::data_bars(&st, &ids, col, cancel)?) })
    }), col));
}

/// The kind badge's width before a title (an 18 px capsule round a 12 px glyph, at most 26).
const BADGE_W: f32 = 26.0;

/// The room of a column's number after its title (5 + its digits in semibold 9.5).
fn num_w(c: usize) -> f32 {
    5.0 + (c.max(1).ilog10() + 1) as f32 * 5.5
}

/// Column `c` fitted to its title and the values on screen, eased there (the title menu's
/// "Fit the width", as a double-click on the column's edge).
pub fn fit_column(tab: &mut Tab, st: &State, ctx: &egui::Context, c: usize, m: &Metrics) {
    let Some(&from) = tab.grid.widths.get(c) else { return };
    let to = fit_width(ctx, st, tab, c, m);
    tab.grid.width_to = Some((c, from, to, Instant::now()));
    tab.grid.notice = Some(format!("Column \"{}\" fitted to its content.", tab.heads().get(c).map_or("", |s| s.as_str())));
    ctx.request_repaint();
}

/// The columns' titles as shown: a text file's one column is titled by its language
/// ("Python", "Log", "Text"), not "Column 1".
pub fn titles(st: &State, tab: &Tab) -> Arc<Vec<String>> {
    let mut h = tab.heads();
    if st.base.dialect.lines {
        if let Some(t) = Arc::make_mut(&mut h).first_mut() { *t = crate::syntax::lang_of(&tab.name).label.to_owned(); }
    }
    h
}

/// A column's width fitted to its title and the values on screen.
pub fn fit_width(ctx: &egui::Context, st: &State, tab: &Tab, c: usize, m: &Metrics) -> f32 {
    let title = titles(st, tab).get(c).cloned().unwrap_or_default();
    let mut w = ctx.fonts_mut(|f| f.layout_no_wrap(title, semibold(m.font_header), Color32::WHITE).size().x) + 20.0 + BADGE_W + num_w(c) + 22.0;
    let cw = char_w(ctx, m);
    let n = tab.shown();
    let extra = if tab.pills.get(c).is_some_and(|x| x.is_some()) { m.dot_d + 6.0 } else if matches!(tab.rich.get(c), Some(Some(Rich::Color))) { 20.0 } else { 0.0 };
    // Every row shown, as the web's (past 200 000, an even sample of them), counted on every
    // core: a long value far down the view counts too.
    use rayon::prelude::*;
    let take = n.min(200_000);
    let view = tab.view.as_deref();
    let chars = (0..take).into_par_iter().with_min_len(4096).map_init(Vec::new, |tmp, k| {
        let pos = if take == n { k } else { k * n / take };
        let id = view.map_or_else(|| st.data_id(pos), |v| v[pos]);
        st.with_cell(id, c, tmp, |s| columns_of(&display(s)))
    }).max().unwrap_or(0);
    w = w.max(chars as f32 * cw + 2.0 * m.cell_pad_x + 1.0 + extra);
    w.clamp(m.col_min_w, m.col_fit_max)
}

/// The width of a text in monospace cells: CJK and emoji take two.
fn columns_of(s: &str) -> usize {
    if s.is_ascii() { return s.len(); }
    s.chars().map(|c| 1 + matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF | 0x20000..=0x3FFFD) as usize).sum()
}

/// Fill by example's ghosts coming in (as the web's `ffin`): the delay from one row to the
/// next, how long the wave runs at most, each ghost's fade, in seconds.
pub const GHOST_IN: (f32, f32, f32) = (0.016, 1.6, 0.45);

/// What a find highlights: its text (folded unless it matches case), its column, its matches.
struct FindLook {
    needle: String,
    case: bool,
    regex: bool,
    col: Option<usize>,
    /// The regex of a regex find (None: not valid).
    re: Option<Arc<csvfab_engine::re::Re>>,
    matches: Arc<Vec<(usize, usize)>>,
    current: Option<(usize, usize)>,
}

/// Everything a cell's drawing reads, gathered once a frame.
struct Look<'a> {
    st: &'a State,
    p: &'a Palette,
    m: &'a Metrics,
    kinds: &'a [Kind],
    rich: &'a [Option<Rich>],
    pills: &'a [Option<Pills>],
    marks: &'a crate::tab::Marks,
    dups: Option<&'a crate::tools::dupes::DupMarks>,
    heat: &'a HashMap<usize, Heat>,
    bars: &'a HashMap<usize, DataBars>,
    lit: Option<(usize, Keep<'a>)>,
    hl: &'a (Hl, Vec<Hl>),
    /// The highlights' identity (the search's, the column filters', the find's): a cell whose
    /// text and this are the last frame's has the same marks, not looked for again.
    hl_gen: u64,
    find: Option<FindLook>,
    ghost: Option<&'a Ghost>,
    /// Seconds since the ghosts were offered (infinite: no motion).
    ghost_t: f32,
    editing: Option<Cell>,
    range: Option<(std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>)>,
    multi: bool,
    fill_target: Option<series::Rect>,
    /// The column in hand and its faint tint.
    tint: Option<(usize, Color32)>,
    /// Number cells: the unit (€, $, %) dimmed.
    units: bool,
    font: FontId,
    font_bold: FontId,
    pill_font: FontId,
    /// The grid font's advance.
    cw: f32,
    ppp: f32,
    ctrl: bool,
    /// A raw-text file's grammar (None at the RAW colour level): its line coloured, plain
    /// otherwise — no pills, rich values nor right-aligned numbers (the web's text cells).
    syn: Option<crate::syntax::Grammar>,
}

/// Plain rectangles gathered into one mesh: a row's fills (and the column rules) are one
/// shape instead of hundreds. Each is clipped here to its pane: a mesh takes its painter's
/// clip only as a whole.
#[derive(Default)]
struct Quads(egui::Mesh);

impl Quads {
    #[inline]
    fn add(&mut self, r: Rect, clip: Rect, c: Color32) {
        let r = r.intersect(clip);
        if r.width() > 0.0 && r.height() > 0.0 && c != Color32::TRANSPARENT {
            self.0.add_colored_rect(r, c);
        }
    }

    /// Paints what was gathered (nothing when empty).
    fn flush(&mut self, pt: &egui::Painter, at: Option<egui::layers::ShapeIdx>) {
        let mesh = std::mem::take(&mut self.0);
        match at {
            Some(i) => pt.set(i, mesh),
            None if !mesh.is_empty() => { pt.add(mesh); }
            None => {}
        }
    }
}

/// A small fast hasher (FxHash): cell keys and the look of a cell, many a frame.
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl Fx {
    #[inline]
    fn mix(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, b: &[u8]) {
        let mut it = b.chunks_exact(8);
        for c in &mut it { self.mix(u64::from_le_bytes(c.try_into().unwrap())); }
        let r = it.remainder();
        let mut w = [0u8; 8];
        w[..r.len()].copy_from_slice(r);
        self.mix(u64::from_le_bytes(w) ^ (r.len() as u64) << 59);
    }
    fn write_u32(&mut self, v: u32) {
        self.mix(v as u64)
    }
    fn write_u64(&mut self, v: u64) {
        self.mix(v)
    }
    fn write_usize(&mut self, v: usize) {
        self.mix(v as u64)
    }
}

type FxMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;

/// What a cell's text is drawn with, besides the text: a galley is reused while it is the same.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TextLook {
    color: Color32,
    font: u8,
    max_w: u32,
    marks: u64,
}

/// A cell's galley kept between frames.
struct CellG {
    /// The value's first bytes and length, hashed: tells an edit (the flash).
    src: u64,
    look: TextLook,
    galley: Option<Arc<egui::Galley>>,
    cut: bool,
    seen: u32,
    /// `Look::hl_gen` its marks (`look.marks`) were found with.
    hl: u64,
}

/// The galleys of the cells on screen. Laying text out (shaping) is most of a frame's
/// work; a cell whose text and look did not change since the last frame is not laid out
/// again — scrolling by a few pixels lays out only the row coming in.
#[derive(Default)]
struct Cells {
    map: FxMap<(RowId, u32), CellG>,
    frame: u32,
    /// egui's own galley of a fixed text, last frame: a new one means egui dropped its
    /// galleys (font atlas rebuilt, zoom): ours point into the old atlas too.
    sentinel: Option<Arc<egui::Galley>>,
    /// The table's edit layers, last frame: when they change, a cell on screen whose text
    /// changed under it was just edited (it flashes).
    edits: (usize, usize, usize, usize),
    edited: bool,
}

impl Cells {
    /// Once a frame, before any cell: drops what egui invalidated, notes an edit.
    fn begin(&mut self, ctx: &egui::Context, st: &State) {
        self.frame = self.frame.wrapping_add(1);
        let s = ctx.fonts_mut(|f| f.layout_no_wrap("0".into(), FontId::monospace(13.0), Color32::WHITE));
        if self.sentinel.as_ref().is_none_or(|o| !Arc::ptr_eq(o, &s)) {
            self.map.clear();
        }
        self.sentinel = Some(s);
        // The same file and columns, other edit layers (a file read again is no edit).
        let e = (Arc::as_ptr(&st.cols) as usize, st.cmap.as_ref().map_or(0, |m| Arc::as_ptr(m) as usize), st.ncols, Arc::as_ptr(&st.base) as usize);
        self.edited = self.edits.0 != 0 && self.edits.0 != e.0 && (self.edits.1, self.edits.2, self.edits.3) == (e.1, e.2, e.3);
        self.edits = e;
    }

    /// After the frame's cells: forgets the cells no longer on screen.
    fn end(&mut self) {
        let f = self.frame;
        self.map.retain(|_, e| e.seen == f);
    }
}

/// The source key of a value for the flash: its length and first 256 bytes.
fn src_key(s: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = Fx(s.len() as u64);
    h.write(&s.as_bytes()[..s.len().min(256)]);
    h.finish()
}

/// What a frame's cells need besides the look: the galley cache, the flashes.
struct CellCx<'a> {
    cells: &'a mut Cells,
    /// Cells edited under the eyes this frame (they start flashing).
    flashed: &'a mut Vec<(RowId, usize)>,
    flashes: &'a FxMap<(RowId, u32), f32>,
    tmp: Vec<u8>,
    hits: Vec<(usize, usize)>,
    hits2: Vec<(usize, usize)>,
    /// The row being drawn, read once for all its cells.
    row: RowFields,
}

/// One row's values for the columns on screen, read in one pass over its record: a cell
/// read alone scans its record from its start, and column 150 of a wide file is kilobytes in.
#[derive(Default)]
struct RowFields {
    /// Whether it holds the row (no: the cells are read one by one).
    ok: bool,
    id: RowId,
    text: String,
    /// (column, start, end) in `text`.
    at: Vec<(usize, u32, u32)>,
}

impl RowFields {
    /// Reads row `id`'s `want`ed fields: (column, field in the record), sorted by field.
    fn load(&mut self, st: &State, id: RowId, want: &[(usize, usize)], tmp: &mut Vec<u8>) {
        self.text.clear();
        self.at.clear();
        self.id = id;
        let b = &st.base;
        self.ok = !b.dialect.lines && (id as usize) < b.n && want.len() > 1;
        if !self.ok { return; }
        let last = want[want.len() - 1].1;
        let mut k = 0;
        let (text, at) = (&mut self.text, &mut self.at);
        csvfab_engine::scan::visit_fields(b.record(id as usize), b.dialect.delim, tmp, |i, v| {
            while k < want.len() && want[k].1 < i { k += 1; }
            while k < want.len() && want[k].1 == i {
                let a = text.len() as u32;
                text.push_str(&b.text(v));
                at.push((want[k].0, a, text.len() as u32));
                k += 1;
            }
            i < last
        });
    }

    /// Cell `c` of the row loaded: its edit, else its field ("" past the record's end).
    #[inline]
    fn get<'a>(&'a self, st: &'a State, id: RowId, c: usize) -> Option<&'a str> {
        if !self.ok || self.id != id { return None; }
        if let Some(s) = st.edited(id, c) { return Some(s); }
        Some(self.at.iter().find(|x| x.0 == c).map_or("", |&(_, a, b)| &self.text[a as usize..b as usize]))
    }
}

impl Look<'_> {
    fn snap(&self, v: f32) -> f32 {
        (v * self.ppp).round() / self.ppp
    }

    /// Draws cell (`pos`, `c`) of record `id` at `x` (width `w`) on the row at `ry` whose fill is
    /// `row_fill`, its fills into `q` (clipped to `clip`, its pane), its text with `pt`;
    /// `frozen`: opaque (drawn over the columns scrolled under it). Returns whether its value
    /// shows cut (an ellipsis, a line break), for the peek.
    #[allow(clippy::too_many_arguments)]
    fn cell(&self, pt: &egui::Painter, q: &mut Quads, clip: Rect, cx: &mut CellCx, pos: usize, id: RowId, c: usize, x: f32, w: f32, ry: f32, even: bool, row_fill: Color32, frozen: bool, differs: bool) -> bool {
        let (p, m) = (self.p, self.m);
        let inner = Rect::from_min_size(pos2(x, ry), vec2(w - 1.0, m.row_h - 1.0));
        let selected = self.range.as_ref().is_some_and(|(r, cs)| r.contains(&pos) && cs.contains(&c)) && self.multi;
        let marked = self.marks.cells.contains_key(&(id, c));
        let key = (id, c as u32);
        let mut cut = false;
        let CellCx { cells, flashed, flashes, tmp, hits, hits2, row } = cx;
        let mut body = |s: &str| {
            let syn = self.syn.filter(|_| c == 0);
            let kind = if syn.is_some() { Kind::Text } else { self.kinds.get(c).copied().unwrap_or(Kind::Text) };
            // An edit seen happening: the cell's value changed under it since the last frame.
            let src = src_key(s);
            let old = cells.map.get_mut(&key);
            if cells.edited && old.as_ref().is_some_and(|e| e.src != src) { flashed.push((id, c)); }
            let entry = match old {
                Some(e) => { e.seen = cells.frame; if e.src != src { e.galley = None; e.src = src; } e }
                None => cells.map.entry(key).or_insert(CellG { src, look: TextLook { color: Color32::TRANSPARENT, font: 0, max_w: 0, marks: 0 }, galley: None, cut: false, seen: cells.frame, hl: 0 }),
            };
            // The fills: frozen cells opaque, then the colour scale, the marks, the selection.
            let heat = self.heat.get(&c).and_then(|h| h.cell(s.trim()));
            let base = match heat {
                Some((neg, pc)) => Some(mix(if neg { p.danger } else { p.accent }, row_fill, pc as f32 / 100.0)),
                None if frozen => Some(row_fill),
                None => None,
            };
            if frozen && selected {
                q.add(inner, clip, if even { p.sel_frz_alt } else { p.sel_frz_bg });
            } else if frozen && marked {
                q.add(inner, clip, mix(p.warn_rgb, row_fill, 0.16));
            } else {
                if let Some(b) = base { q.add(inner, clip, b); }
                if marked { q.add(inner, clip, p.mark_fill); }
                if selected { q.add(inner, clip, p.sel_fill); }
            }
            if let Some((_, t)) = self.tint.filter(|t| t.0 == c) { q.add(inner, clip, t); }
            // A value that differs within its duplicate group.
            if differs { q.add(inner, clip, p.danger_subtle); }
            if self.fill_target.is_some_and(|t| (t.r0..=t.r1).contains(&pos) && (t.c0..=t.c1).contains(&c)) {
                q.add(inner, clip, p.fill_preview);
            }
            // Just edited: a flash fading out.
            if let Some(&a) = flashes.get(&key) { q.add(inner, clip, p.flash_fill.gamma_multiply(a)); }
            if self.editing == Some(Cell { row: pos, col: c }) { return; }
            if s.is_empty() {
                // Fill by example: the value it would write, in grey.
                if let Some(g) = self.ghost.filter(|g| g.col == c) {
                    if let Some(v) = g.values.get(&id) {
                        // The wave: each row 16 ms after the one above, sliding 8 px in as it shows.
                        let k = ((ry - clip.min.y) / m.row_h).max(0.0);
                        let t = ((self.ghost_t - k * GHOST_IN.0) / GHOST_IN.2).clamp(0.0, 1.0);
                        let a = 1.0 - (1.0 - t).powi(3);
                        let col = p.txt_dim.gamma_multiply(a);
                        let mut f = TextFormat::simple(self.font.clone(), col);
                        f.italics = true;
                        let mut job = LayoutJob::single_section(display(v).into_owned(), f);
                        job.wrap = TextWrapping { max_width: (w - 2.0 * m.cell_pad_x - 1.0).max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                        let g = pt.layout_job(job);
                        let gx = x + m.cell_pad_x - 8.0 * (1.0 - a);
                        pt.with_clip_rect(inner.intersect(clip)).galley(pos2(self.snap(gx), self.snap(ry + (m.row_h - 1.0 - g.size().y) / 2.0)), g, col);
                    }
                }
                return;
            }
            let rich = if syn.is_some() { None } else { self.rich.get(c).and_then(|r| r.as_ref()) };
            let cy = ry + (m.row_h - 1.0) / 2.0;
            // Yes / no: a disc with a check, or a ring; no text.
            if let Some(on) = rich.and_then(|r| r.checked(s)) {
                let ctr = pos2(self.snap(x + (w - 1.0) / 2.0), self.snap(cy));
                let clip = pt.with_clip_rect(inner.intersect(clip));
                if on {
                    clip.circle_filled(ctr, 7.5, p.prim);
                    let s2 = Stroke::new(2.0, p.on_prim);
                    clip.line_segment([ctr + vec2(-3.5, 0.2), ctr + vec2(-1.0, 2.7)], s2);
                    clip.line_segment([ctr + vec2(-1.0, 2.7), ctr + vec2(3.6, -2.4)], s2);
                } else {
                    clip.circle_stroke(ctr, 7.0, Stroke::new(1.0, p.txt_faint));
                }
                return;
            }
            let lit = self.lit.as_ref().is_some_and(|(lc, k)| *lc == c && k.test(s));
            let pill = if syn.is_some() { None } else { self.pills.get(c).and_then(|x| x.as_ref()).and_then(|pl| pl.slot(s)) };
            let swatch = matches!(rich, Some(Rich::Color)) && rich.unwrap().holds(s);
            let link = matches!(rich, Some(Rich::Url | Rich::Email)) && rich.unwrap().holds(s);
            let lead = if swatch { 30.0 } else if pill.is_some() { m.cell_pad_x + m.dot_d + 6.0 } else { m.cell_pad_x };
            let max_w = (w - 1.0 - lead - m.cell_pad_x).max(1.0);
            // A text line: its tabs as spaces (as an editor shows them), long lines kept.
            let full = match syn {
                Some(_) => { let (t, long) = crate::syntax::shown(s); cut |= long; t }
                None => { let t = display(s); if t.len() != s.len() || s.contains('\n') { cut = true; } t }
            };
            // Only what can show is laid out: a grid glyph is `cw` wide (a fallback glyph at
            // least half that), a category's at least 2.5 — a long value costs a short one's.
            let keep = (max_w / if pill.is_some() { 2.5 } else { self.cw * 0.5 }) as usize + 2;
            let text: &str = match full.char_indices().nth(keep) {
                Some((b, _)) => { cut = true; &full[..b] }
                None => &full,
            };
            let (mut color, mut font) = match (kind, pill) {
                (_, Some(_)) => (p.txt, 2u8),
                (Kind::Number, _) if crate::theme::type_colors() => (p.num_txt, 0),
                (Kind::Date, _) if crate::theme::type_colors() => (p.date_txt, 0),
                (Kind::Text, _) if link => (p.accent_2, 0),
                (Kind::Text, _) if matches!(rich, Some(Rich::Json)) => (p.txt_mut, 0),
                _ => (p.txt, 0),
            };
            if differs { color = p.diff_txt; }
            if lit {
                color = p.txt_strong;
                if pill.is_none() { font = 1; }
            }
            // A text line coloured (unless lit or differing: their colour says more). Its galley
            // is kept as any cell's; the theme's syntax colours are in its key.
            let colored = syn.filter(|_| !lit && !differs);
            if colored.is_some() { font |= 0x80; }
            // Search highlights (the global search and this column's filter), then a find's: not
            // looked for again when the text, the look and the searches are the last frame's.
            let (hl_global, hl_col) = (&self.hl.0, &self.hl.1);
            hits.clear();
            let probe = TextLook { color, font, max_w: max_w.to_bits(), marks: entry.look.marks };
            let mut reuse = entry.hl == self.hl_gen && entry.look == probe && entry.galley.as_ref().is_some_and(|g| g.job.text == text);
            let mut look = probe;
            if !reuse {
                let mut marks_key = 0u64;
                if !hl_global.is_empty() || !hl_col[c].is_empty() || self.find.is_some() {
                    hl_global.hits(text, hits, hits2);
                    let mine = std::mem::take(hits);
                    hl_col[c].hits(text, hits, hits2);
                    hits.extend(mine);
                    let n = hits.len();
                    if let Some(fl) = &self.find {
                        if fl.col.is_none_or(|fc| fc == c) && fl.matches.binary_search(&(pos, c)).is_ok() {
                            if fl.regex {
                                if let Some(re) = &fl.re { spans(re, text, hits); }
                            } else if fl.case {
                                hits.extend(text.match_indices(fl.needle.as_str()).take(16).map(|(a, t)| (a, a + t.len())));
                            } else {
                                matches(text, &fl.needle, hits2);
                                hits.extend_from_slice(hits2);
                            }
                        }
                    }
                    // Search marks sort before a find's at the same place: (a, b, find) as the old tuple order.
                    let mut h = Fx(0);
                    for (k, &(a, b)) in hits.iter().enumerate() { h.mix(((a as u64) << 33) ^ ((b as u64) << 1) ^ (k >= n) as u64); }
                    marks_key = h.0 | 1;
                    // Tag the find's hits by the high bit of the end (cleared below).
                    for hm in hits[n..].iter_mut() { hm.1 |= 1 << 63; }
                }
                if colored.is_some() { marks_key ^= (u32::from_le_bytes(p.sy_kw.to_array()) as u64) << 32 | u32::from_le_bytes(p.sy_str.to_array()) as u64; }
                look = TextLook { color, font, max_w: max_w.to_bits(), marks: marks_key };
                // (A right-to-left line is laid out in another order: its text tells nothing, the
                // value's hash, `src`, does.)
                reuse = entry.look == look && entry.galley.as_ref().is_some_and(|g| g.job.text == text || (entry.src == src && crate::bidi::has_rtl(text)));
            }
            entry.hl = self.hl_gen;
            let marks_key = look.marks;
            let f = match font & 0x7f { 1 => &self.font_bold, 2 => &self.pill_font, _ => &self.font };
            if !reuse {
                let plain = TextFormat::simple(f.clone(), color);
                let mut job = LayoutJob::default();
                // A number's unit dimmed, marks or not.
                let unit = if self.units && kind == Kind::Number && pill.is_none() { unit_of(text) } else { None };
                let dim = TextFormat { color: p.txt_dim, ..plain.clone() };
                let put = |job: &mut LayoutJob, a: usize, b: usize| match unit {
                    Some((ua, ub)) if a < ub && b > ua => {
                        if a < ua { job.append(&text[a..ua], 0.0, plain.clone()); }
                        job.append(&text[a.max(ua)..b.min(ub)], 0.0, dim.clone());
                        if b > ub { job.append(&text[ub..b], 0.0, plain.clone()); }
                    }
                    _ => job.append(&text[a..b], 0.0, plain.clone()),
                };
                if let Some(g) = colored {
                    let mut marks: Vec<(usize, usize, bool)> = hits.iter().map(|&(a, b)| (a, b & !(1 << 63), b >> 63 == 1)).collect();
                    crate::syntax::append(&mut job, text, g, &mut marks, &plain, &self.font_bold, p);
                } else if marks_key == 0 && crate::bidi::has_rtl(text) {
                    crate::bidi::append(&mut job, text, &plain);
                } else if marks_key == 0 {
                    put(&mut job, 0, text.len());
                } else {
                    let mut marks: Vec<(usize, usize, bool)> = hits.iter().map(|&(a, b)| (a, b & !(1 << 63), b >> 63 == 1)).collect();
                    marks.sort_unstable();
                    let mut at = 0;
                    for &(a, b, fd) in &marks {
                        if a < at || b > text.len() { continue; }
                        if a > at { put(&mut job, at, a); }
                        job.append(&text[a..b], 0.0, TextFormat { background: if fd { p.find_bg } else { p.match_bg }, color: p.mark_txt, ..plain.clone() });
                        at = b;
                    }
                    if at < text.len() { put(&mut job, at, text.len()); }
                }
                job.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = pt.layout_job(job);
                entry.cut = cut || g.elided;
                entry.galley = Some(g);
                entry.look = look;
            }
            cut |= entry.cut;
            let g = entry.galley.clone().unwrap();
            let gw = g.size().x;
            let ty = self.snap(cy - g.size().y / 2.0);
            let tx = self.snap(if kind == Kind::Number && pill.is_none() { x + w - 1.0 - m.cell_pad_x - gw } else { x + lead });
            if let Some(slot) = pill {
                // A category: its colour in a dot, the text calm; lit by a chart, the dot grows.
                let ctr = pos2(x + m.cell_pad_x + m.dot_d / 2.0, self.snap(cy) + 0.5 / self.ppp);
                pt.with_clip_rect(inner.intersect(clip)).circle_filled(ctr, m.dot_d / 2.0 + if lit { 1.0 } else { 0.0 }, p.pill_colors(slot).0);
                pt.galley(pos2(tx, ty), g, color);
            } else {
                if swatch {
                    let ctr = pos2(x + 17.0, self.snap(cy) + 0.5 / self.ppp);
                    pt.circle_filled(ctr, 6.25, alpha(p.hi, 0.35));
                    pt.circle_filled(ctr, 5.5, parse_hex(s.trim()).unwrap_or(p.txt_faint));
                }
                pt.galley(pos2(tx, ty), g, color);
                // Links underlined while Ctrl is held (3 px under the text).
                if link && self.ctrl {
                    let uy = self.snap(ty + g_baseline(f, self.ppp) + 3.0) - 0.5 / self.ppp;
                    pt.hline(tx..=tx + gw, uy, Stroke::new(1.0, color));
                }
            }
            // Data bars: a 3 px rule at the cell's foot, from zero.
            if let Some(b) = self.bars.get(&c) {
                if let Some(v) = csvfab_engine::stats::num_cell(s.trim()) {
                    let (a, bw, neg) = b.bar(v);
                    let cw = w - 1.0 - 2.0 * m.cell_pad_x;
                    let x0 = x + m.cell_pad_x + cw * a as f32 / 100.0;
                    let r = Rect::from_min_size(pos2(x0, ry + m.row_h - 1.0 - 4.0 - 3.0), vec2((cw * bw as f32 / 100.0).max(1.0), 3.0));
                    pt.rect_filled(r.intersect(inner), 0.0, if neg { p.danger } else { p.prim });
                }
            }
            if let Some(fl) = &self.find {
                if fl.current == Some((pos, c)) {
                    pt.rect_stroke(inner, 0.0, Stroke::new(2.0, p.ok), StrokeKind::Inside);
                }
            }
        };
        match row.get(self.st, id, c) {
            Some(s) => body(s),
            None => st_cell(self.st, id, c, tmp, body),
        }
        cut
    }
}

/// Where a number as written holds its unit (`12 €`, `$3`, `-€3`, `45 %`): its bytes, spaces aside.
fn unit_of(s: &str) -> Option<(usize, usize)> {
    const UNITS: &[char] = &['€', '$', '%', '£', '¥'];
    let (first, last) = (s.find(|c: char| c.is_ascii_digit())?, s.rfind(|c: char| c.is_ascii_digit())?);
    let tail = s[last + 1..].trim_start();
    if tail.len() == tail.chars().next().map_or(0, char::len_utf8) && tail.starts_with(UNITS) {
        return Some((s.len() - tail.len(), s.len()));
    }
    let head = s[..first].trim_end();
    let a = head.find(UNITS)?;
    (head[a..].chars().count() == 1).then_some((a, head.len()))
}

/// The text's baseline below its top, near enough (the font's ascent).
fn g_baseline(f: &FontId, _ppp: f32) -> f32 {
    f.size * 0.95
}

#[inline]
fn st_cell<R>(st: &State, id: RowId, c: usize, tmp: &mut Vec<u8>, f: impl FnOnce(&str) -> R) -> R {
    st.with_cell(id, c, tmp, f)
}

/// `CSVFAB_SHOT=…,motion:MS`: motions and flashes held at MS ms, to look at them off screen.
fn hold() -> Option<f32> {
    static HOLD: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *HOLD.get_or_init(|| crate::shot::take("motion").and_then(|v| v.parse::<f32>().ok()).map(|v| v / 1000.0))
}

/// The cells just edited: their flash's strength (1 → 0 over 1.2 s). Frames are
/// asked for only while one runs.
fn flash_alphas(flash: &mut FxMap<(RowId, u32), Instant>, ctx: &egui::Context) -> FxMap<(RowId, u32), f32> {
    const DUR: f32 = 1.2;
    if flash.is_empty() { return FxMap::default(); }
    let age = |t: &Instant| hold().unwrap_or_else(|| t.elapsed().as_secs_f32());
    flash.retain(|_, t| age(t) < DUR);
    if !flash.is_empty() { ctx.request_repaint(); }
    // As the web's `cellflash`: full for the first quarter, then easing out.
    flash.iter().map(|(k, t)| (*k, 1.0 - motion::bezier(0.0, 0.0, 0.58, 1.0, (age(t) / DUR - 0.25) / 0.75))).collect()
}

/// The rows' motion after a sort or a filter, as the web app's (50-sort-motion.js,
/// 52-motion-links.js): only drawn, for a few hundred ms — clicks and keys already see the
/// new view. A sort: the rows on screen go toward their new places and the new ones come
/// from theirs (beyond the edge they were past, compressed: farther comes faster), over a
/// faint copy of the old screen. A filter: the rows still there glide to close the gaps,
/// the new ones rise into place, the rows gone fade where they were.
mod motion {
    use super::*;

    pub const SORT: f32 = 0.28;
    pub const FILTER: f32 = 0.2;
    pub const GHOST: f32 = SORT + 0.1;

    /// cubic-bezier(x1, y1, x2, y2) at `t`, as CSS (x solved by bisection: a few dozen rows a frame).
    pub fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let b = |a: f32, b: f32, s: f32| 3.0 * a * s * (1.0 - s) * (1.0 - s) + 3.0 * b * s * s * (1.0 - s) + s * s * s;
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..18 {
            let s = (lo + hi) / 2.0;
            if b(x1, x2, s) < t { lo = s } else { hi = s }
        }
        b(y1, y2, (lo + hi) / 2.0)
    }

    pub fn ease_in(t: f32) -> f32 {
        bezier(0.42, 0.0, 1.0, 1.0, t)
    }

    /// Where a row of the new view starts from (y from the body's top).
    pub struct From {
        y: f32,
        dur: f32,
        rise: bool,
    }

    impl From {
        /// Its y and opacity at `t` s, its place being `to`.
        pub fn at(&self, to: f32, t: f32, sort: bool) -> (f32, f32) {
            let k = t / self.dur;
            if sort {
                // Arrivals come in fast and settle.
                (self.y + (to - self.y) * bezier(0.12, 0.7, 0.25, 1.0, k), 1.0)
            } else {
                let e = bezier(0.2, 0.8, 0.3, 1.0, k);
                (self.y + (to - self.y) * e, if self.rise { 0.35 + 0.65 * e } else { 1.0 })
            }
        }
    }

    /// A row of the old view leaving the screen.
    pub struct Gone {
        pub id: RowId,
        y0: f32,
        y1: f32,
        dur: f32,
    }

    impl Gone {
        pub fn at(&self, t: f32, sort: bool) -> (f32, f32) {
            let k = t / self.dur;
            if k >= 1.0 { return (self.y1, 0.0); }
            if sort {
                // Departures start slowly: they stay while the others cross them.
                (self.y0 + (self.y1 - self.y0) * bezier(0.6, 0.0, 0.8, 0.4, k), 1.0)
            } else {
                (self.y0, 1.0 - bezier(0.0, 0.0, 0.58, 1.0, k))
            }
        }
    }

    pub struct Motion {
        pub start: Instant,
        pub sort: bool,
        /// The scroll it was made for: a scroll ends it (the rows jump to their places).
        scroll: f64,
        pub from: FxMap<RowId, From>,
        pub gone: Vec<Gone>,
        /// The old screen (a sort), faint and fading under everything.
        pub ghost: Vec<(RowId, f32)>,
        pub dur: f32,
    }

    /// Starts a motion when the rows shown changed since the last frame (a view arrived), from
    /// where the rows were drawn then — a motion running is taken over from where it is.
    pub fn start(tab: &mut Tab, st: &State, ctx: &egui::Context, body: Rect, end: usize) {
        let rows = tab.shown();
        let key = (tab.view.as_ref().map_or(0, |v| Arc::as_ptr(v) as usize), rows);
        let top = tab.grid.top;
        let shown: Vec<RowId> = if key != tab.grid.drawn_key { (top..end).map(|p| tab.shown_id(p)).collect() } else { Vec::new() };
        let g = &mut tab.grid;
        if g.motion.as_ref().is_some_and(|m| (m.scroll - g.scroll).abs() > 0.5) { g.motion = None; }
        if key == g.drawn_key { return; }
        let (old_key, old_scroll) = (g.drawn_key, g.drawn_scroll);
        g.drawn_key = key;
        let sort = tab.sort != g.view_sort;
        g.view_sort = tab.sort.clone();
        g.motion = None;
        // Reduced motion (no animation time) skips it — but a shot holding one (`motion:MS`) shows it.
        if g.drawn.is_empty() || rows == 0 || (ctx.global_style().animation_time <= 0.0 && hold().is_none()) { return; }
        let (h, rh) = (body.height(), g.rh());
        let header = st.header as RowId;
        // In file order (no view) a row's place is its id: the old places of the new rows, the
        // new places of the old ones, are known without a search.
        let place = |view_none: bool, id: RowId, scroll: f64| (view_none && st.order.is_none() && id >= header).then(|| ((id - header) as f64 * rh as f64 - scroll) as f32);
        let dur = |d: f32| SORT + (d.abs() / h * 0.09).min(0.12);
        let far = |d: f32| rh * (0.5 + 1.3 * (1.0 + d.max(0.0) / rh).log2());
        let screen = |y: Option<f32>, k: usize| match y {
            None => h + far(rh * (4 + k) as f32),
            Some(y) if y >= h => h + far(y - h),
            Some(y) if y < 0.0 => -rh - far(-y),
            Some(y) => y,
        };
        let before: FxMap<RowId, f32> = g.drawn.iter().copied().collect();
        let mut on: FxMap<RowId, ()> = FxMap::default();
        let mut from = FxMap::default();
        let mut longest: f32 = 0.0;
        let mut k = 0;
        for (pos, &id) in (top..end).zip(&shown) {
            let y = (pos as f64 * rh as f64 - g.scroll) as f32;
            on.insert(id, ());
            let f = match before.get(&id) {
                Some(&y0) if (y0 - y).abs() < 1.0 => continue,
                Some(&y0) => From { y: y0, dur: if sort { dur(y0 - y) } else { FILTER }, rise: false },
                None if sort => {
                    let y0 = screen(place(old_key.0 == 0, id, old_scroll), k);
                    k += 1;
                    From { y: y0, dur: dur(y0 - y), rise: false }
                }
                None => From { y: y + 10.0, dur: FILTER, rise: true },
            };
            longest = longest.max(f.dur);
            from.insert(id, f);
        }
        let gone: Vec<Gone> = g.drawn.iter().filter(|x| !on.contains_key(&x.0)).enumerate().map(|(n, &(id, y0))| {
            if sort {
                let y1 = screen(place(key.0 == 0, id, g.scroll), n);
                Gone { id, y0, y1, dur: dur(y1 - y0) }
            } else {
                Gone { id, y0, y1: y0, dur: FILTER * 0.8 }
            }
        }).collect();
        if from.is_empty() && gone.is_empty() { return; }
        let longest = gone.iter().map(|x| x.dur).fold(longest, f32::max).max(if sort { GHOST } else { 0.0 });
        let ghost = if sort { g.drawn.clone() } else { Vec::new() };
        g.motion = Some(Motion { start: Instant::now(), sort, scroll: g.scroll, from, gone, ghost, dur: longest });
        ctx.request_repaint();
    }
}

/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let d = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok().map(|v| if n == 1 { v * 17 } else { v });
    Some(match h.len() {
        3 | 4 => Color32::from_rgba_unmultiplied(d(0, 1)?, d(1, 1)?, d(2, 1)?, if h.len() == 4 { d(3, 1)? } else { 255 }),
        6 | 8 => Color32::from_rgba_unmultiplied(d(0, 2)?, d(2, 2)?, d(4, 2)?, if h.len() == 8 { d(6, 2)? } else { 255 }),
        _ => return None,
    })
}

/// The rows shown, in a job (the whole file's ids built there, not on the UI thread).
fn ids_of(st: &State, view: &Option<Arc<Vec<RowId>>>) -> Arc<Vec<RowId>> {
    match view {
        Some(v) => v.clone(),
        None => Arc::new((0..st.len()).map(|p| st.data_id(p)).collect()),
    }
}

struct Band {
    head: Rect,
    charts: Option<Rect>,
    filt: Option<Rect>,
}

/// Picks up the grid's own jobs: colour scales, fills.
fn poll_jobs(tab: &mut Tab, actions: &mut Vec<Action>) {
    let shown = tab.shown();
    let g = &mut tab.grid;
    let mut k = 0;
    while k < g.look_jobs.len() {
        let Some(r) = g.look_jobs[k].0.poll() else { k += 1; continue };
        let col = g.look_jobs.remove(k).1;
        match r {
            Some(LookRes::Heat(Some(h))) => { g.heat.insert(col, h); }
            Some(LookRes::Bars(Some(b))) => { g.bars.insert(col, b); }
            _ => {}
        }
    }
    if let Some(j) = &mut g.fill_job {
        if let Some(r) = j.poll() {
            g.fill_job = None;
            if let Some(ch) = r.filter(|c| !c.is_empty()) { actions.push(Action::SetCells(ch)); }
        }
    }
    if let Some(r) = g.goto_job.as_mut().and_then(|j| j.0.poll()) {
        g.goto_job = None;
        if let Some((row, col)) = r.filter(|r| r.0 < shown) {
            g.select(Cell { row, col }, false);
            g.set_top(row.saturating_sub(g.page / 2));
        } else {
            g.notice = Some("No row shown holds that value.".into());
        }
    }
    if let Some((j, pasted)) = &mut g.patch_job {
        if let Some(r) = j.poll() {
            let pasted = *pasted;
            g.patch_job = None;
            if let Some(patch) = r.filter(|p| pasted || !p.is_empty()) { actions.push(Action::Patch { patch, pasted }); }
        }
    }
}

/// Selections up to this many cells are written at once; bigger ones in a job.
const AT_ONCE: usize = 50_000;

/// `value` in every cell of the selection (its columns shown) that does not hold it already
/// (Delete, Enter on a block, one value pasted): a small selection at once (the action
/// returned), a big one in the background — never a change per cell (a whole file cleared
/// is one empty text per column over the rows, 4 bytes a cell).
pub fn fill_selection(tab: &mut Tab, value: String, pasted: bool, ctx: &egui::Context) -> Option<Action> {
    let (rows, cols) = tab.grid.range()?;
    let st = tab.st()?.clone();
    let shown = tab.shown();
    let cols: Vec<usize> = cols.filter(|c| *c < st.ncols && !tab.grid.hidden.contains(c)).collect();
    if shown == 0 || cols.is_empty() { return None; }
    let (r0, r1) = (*rows.start(), (*rows.end()).min(shown - 1));
    let view = tab.view.clone();
    let big = (r1 + 1).saturating_sub(r0) * cols.len() > AT_ONCE;
    let work = move |cancel: &std::sync::atomic::AtomicBool| {
        // The rows of the selection only, made here (in the job for a big one).
        let ids: Vec<RowId> = (r0..=r1).map(|p| view.as_ref().map_or_else(|| st.data_id(p), |v| v[p])).collect();
        Patch::fill(&st, &ids, &cols, &value, cancel)
    };
    if !big {
        return work(&std::sync::atomic::AtomicBool::new(false)).map(|patch| Action::Patch { patch, pasted });
    }
    // A newer one replaces it (the old one cancelled as it is dropped).
    tab.grid.patch_job = Some((Job::spawn(ctx, move |cancel, _| work(cancel)), pasted));
    None
}

/// Draws the grid and handles its input. Returns what the app must apply.
pub fn show(ui: &mut Ui, tab: &mut Tab, p: &Palette, m: &Metrics) -> Vec<Action> {
    let t0 = Instant::now();
    let mut tl = t0;
    let mut actions = Vec::new();
    let Some(table) = &tab.table else { return actions };
    let st = table.st.clone();
    let rows = tab.shown();
    let ncols = st.ncols;
    // Also a selection past the last column (whatever put it there): the fill handle and the
    // keys index the columns' widths with it.
    if tab.grid.widths.len() != ncols || tab.grid.cursor.into_iter().chain(tab.grid.anchor).any(|c| c.col >= ncols) {
        tab.grid.fit_count(ncols);
    }
    if tab.grid.ncols_seen != ncols {
        if tab.grid.ncols_seen != 0 { tab.grid.reset_columns(); tab.totals.reset_columns(); }
        tab.grid.ncols_seen = ncols;
    }
    let ctx = ui.ctx().clone();
    poll_jobs(tab, &mut actions);
    // A column fitting its width eases there in 160 ms.
    if let Some((c, a, b, t0)) = tab.grid.width_to {
        let k = t0.elapsed().as_secs_f32() / 0.16;
        if c >= ncols || k >= 1.0 {
            if c < ncols { tab.grid.widths[c] = b; }
            tab.grid.width_to = None;
        } else {
            tab.grid.widths[c] = a + (b - a) * motion::bezier(0.2, 0.8, 0.3, 1.0, k);
            ctx.request_repaint();
        }
    }
    if tab.show_charts { crate::spark_ui::poll(tab, &ctx); }
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, Sense::hover());
    let painter = ui.painter_at(full);
    painter.rect_filled(full, 0.0, p.bg);

    let sw = m.strip_w;
    let gut = m.gutter_w;
    let mut y = full.min.y;
    let head = Rect::from_min_size(pos2(full.min.x, y), vec2(full.width() - sw, m.header_h));
    y = head.max.y;
    let charts = tab.show_charts.then(|| { let r = Rect::from_min_size(pos2(full.min.x, y), vec2(head.width(), m.spk_h)); y = r.max.y; r });
    let filt = tab.show_filters.then(|| { let r = Rect::from_min_size(pos2(full.min.x, y), vec2(head.width(), m.filter_h)); y = r.max.y; r });
    let band = Band { head, charts, filt };
    let foot = if tab.show_totals && rows > 0 { m.totals_h } else { 0.0 };
    // The totals row sits right under the last row when every row fits.
    let body_max = full.max.y - sw - foot;
    let body_max = body_max.min(y + rows as f32 * m.row_h).max(y + 1.0).min(full.max.y - sw - foot);
    let body = Rect::from_min_max(pos2(full.min.x, y), pos2(full.max.x - sw, if foot > 0.0 { body_max } else { full.max.y - sw }));
    let cells_x0 = full.min.x + gut;
    let frozen = tab.grid.frozen.min(ncols);
    let widths = tab.grid.laid();
    let frz_w: f32 = widths[..frozen].iter().sum();
    let scroll_x0 = { let q = ctx.pixels_per_point(); ((cells_x0 + frz_w) * q).round() / q };
    let total_w: f32 = widths[frozen..].iter().sum();
    let view_w = (body.max.x - scroll_x0).max(1.0);
    tab.grid.view_w = body.max.x - cells_x0;
    tab.grid.xoff = tab.grid.xoff.clamp(0.0, (total_w - view_w).max(0.0));
    let page = ((body.height() / m.row_h).floor() as usize).max(1);
    tab.grid.page = page;
    // Another density: the same row stays on top; what moved was drawn at the old height.
    if tab.grid.row_h > 0.0 && tab.grid.row_h != m.row_h {
        tab.grid.scroll *= (m.row_h / tab.grid.row_h) as f64;
        tab.grid.motion = None;
    }
    tab.grid.row_h = m.row_h;
    tab.grid.body_h = (full.max.y - sw - foot - y).max(1.0);
    let ppp = ctx.pixels_per_point();
    let ctrl = ui.input(|i| i.modifiers.command);
    if std::mem::take(&mut tab.grid.reveal_col) { tab.grid.reveal(rows, view_w); }

    // ---- Input on the body: wheel, clicks, drags, keys.
    let body_id = ui.scope_id().with("grid-body");
    let resp = ui.interact(body, body_id, Sense::click_and_drag());
    let last_col = (0..ncols).rev().find(|c| !tab.grid.hidden.contains(c)).unwrap_or(ncols.saturating_sub(1));
    let col_at = |x: f32, xoff: f32| -> usize {
        let x = x - cells_x0;
        if x < frz_w {
            let mut acc = 0.0;
            for (c, w) in widths[..frozen].iter().enumerate() { acc += w; if x < acc { return c; } }
            return 0;
        }
        let x = x - frz_w + xoff;
        let mut acc = 0.0;
        for (k, w) in widths[frozen..].iter().enumerate() { acc += w; if x < acc { return frozen + k; } }
        last_col
    };
    let hit = |pt: Pos2, scroll: f64, xoff: f32, selecting: bool| -> Option<Cell> {
        if rows == 0 || (!body.contains(pt) && !selecting) { return None; }
        let y = scroll + (pt.y - body.min.y) as f64;
        let row = if y < 0.0 { 0 } else { (y / m.row_h as f64) as usize };
        Some(Cell { row: row.min(rows - 1), col: col_at(pt.x.max(cells_x0), xoff) })
    };
    let max_x = (total_w - view_w).max(0.0);
    wheel(ui, &mut tab.grid, ui.rect_contains_pointer(full), max_x, rows, m.row_h);
    fling(ui, &mut tab.grid, ui.rect_contains_pointer(full), max_x, rows);
    shot_hooks(tab, &ctx, &resp, ncols);
    tab.grid.sync(rows, ppp);
    // The column in hand follows the cursor (not a whole row's, which stands on the last column).
    let cursor_col = tab.grid.cursor.map(|c| c.col);
    if cursor_col != tab.grid.cursor_col {
        tab.grid.cursor_col = cursor_col;
        let rows_only = tab.grid.range().is_some_and(|(_, cs)| *cs.start() == 0 && cs.end() + 1 >= ncols && ncols > 1);
        if !rows_only { tab.grid.focus_col = cursor_col.or(tab.grid.focus_col); }
    }
    // The fill handle: the selection's bottom-right corner.
    let handle_rect = |g: &GridState| -> Option<Rect> {
        let (r, cs) = g.range()?;
        let (hr, hc) = (*r.end(), *cs.end());
        let y = body.min.y + (hr as f32 - g.top as f32) * m.row_h - g.off + m.row_h - 1.0;
        let x = if hc < frozen { cells_x0 + widths[..hc + 1].iter().sum::<f32>() } else { scroll_x0 - g.xoff + widths[frozen..hc + 1].iter().sum::<f32>() } - 1.0;
        Some(Rect::from_center_size(pos2(x - 3.5, y - 3.5), vec2(7.0, 7.0)))
    };
    let mut gutter_menu = false;
    let in_handle = |g: &GridState, q: Option<Pos2>| g.editing.is_none() && q.zip(handle_rect(g)).is_some_and(|(q, h)| h.expand(3.0).contains(q));
    let on_handle = in_handle(&tab.grid, ui.input(|i| i.pointer.hover_pos()).filter(|_| ui.rect_contains_pointer(full)));
    if on_handle || tab.grid.fill_drag.is_some() { ctx.set_cursor_icon(CursorIcon::Crosshair); }
    // Where the button went down, not where the pointer is: a drag starts only once it has
    // moved a few points, by then often out of the handle's small zone.
    if resp.drag_started() && in_handle(&tab.grid, ui.input(|i| i.pointer.press_origin())) {
        tab.grid.fill_drag = tab.grid.block().map(|b| (b, None));
    } else if resp.drag_started() || resp.clicked() || resp.secondary_clicked() {
        // A drag selects from the cell pressed, not the one the pointer reached meanwhile.
        let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()) } else { None };
        if let Some(pt) = origin.or(resp.interact_pointer_pos()) {
            let shift = ui.input(|i| i.modifiers.shift);
            if let Some(c) = hit(pt, tab.grid.scroll, tab.grid.xoff, false) {
                commit_edit(tab, &st, &mut actions);
                if resp.secondary_clicked() {
                    if pt.x < cells_x0 {
                        // A row's number: the row's menu, as its ⋮.
                        let gr = Rect::from_min_size(pos2(body.min.x, pt.y), vec2(gut, 1.0));
                        tab.grid.row_menu = Some((tab.shown_id(c.row), c.row, gr));
                        gutter_menu = true;
                        egui::Popup::open_id(&ctx, row_menu_id());
                    } else {
                        let inside = tab.grid.range().is_some_and(|(r, cs)| r.contains(&c.row) && cs.contains(&c.col));
                        if !inside { tab.grid.select(c, false); }
                        tab.grid.menu_at = Some(c);
                    }
                } else if pt.x < cells_x0 {
                    // A row number: whole rows. Dragged from a row already selected whole, it
                    // moves (as in Excel); from any other, the drag selects rows.
                    let held = tab.grid.range().is_some_and(|(r, cs)| r.contains(&c.row) && *cs.start() == 0 && *cs.end() + 1 >= ncols);
                    if resp.drag_started() && !shift && held {
                        tab.grid.row_drag = Some((tab.shown_id(c.row), None));
                    } else if resp.drag_started() {
                        tab.grid.selecting = true;
                        tab.grid.selecting_rows = true;
                    }
                    let a = Cell { row: c.row, col: 0 };
                    if !shift || tab.grid.anchor.is_none_or(|x| x.col != 0) { tab.grid.anchor = Some(a); }
                    tab.grid.cursor = Some(Cell { row: c.row, col: ncols.saturating_sub(1) });
                } else {
                    tab.grid.select(c, shift);
                    tab.grid.selecting = resp.drag_started();
                    // Ctrl+click on a link opens it, on an address writes to it.
                    if resp.clicked() && ctrl {
                        let s = st.cell(tab.shown_id(c.row), c.col);
                        let s = s.trim();
                        if is_link(s) {
                            let url = if s.contains('@') && !s.contains("://") { format!("mailto:{s}") } else { s.to_string() };
                            tab.grid.notice = Some(format!("Opened {url}"));
                            ctx.open_url(egui::OpenUrl::new_tab(url));
                        }
                    }
                }
                resp.request_focus();
            }
        }
    }
    let drag_pos = ui.input(|i| i.pointer.interact_pos());
    if resp.dragged() && tab.grid.row_drag.is_some() {
        if let Some(pt) = drag_pos {
            if pt.y > body.max.y - 24.0 { tab.grid.scroll += ((pt.y - body.max.y + 24.0) as f64).min(200.0) * 0.4 + 2.0; }
            if pt.y < body.min.y + 24.0 { tab.grid.scroll -= ((body.min.y + 24.0 - pt.y) as f64).min(200.0) * 0.4 + 2.0; }
            tab.grid.sync(rows, ppp);
            let q = pt.min(body.max - vec2(1.0, 1.0)).max(body.min);
            let target = hit(q, tab.grid.scroll, tab.grid.xoff, true).map(|c| {
                let top = body.min.y + (c.row as f64 * m.row_h as f64 - tab.grid.scroll) as f32;
                (c.row, q.y >= top + m.row_h / 2.0)
            });
            if let Some(d) = &mut tab.grid.row_drag { d.1 = target; }
            // The line where the row would land, across the rows.
            if let Some((r, after)) = target {
                let y = body.min.y + ((r + after as usize) as f64 * m.row_h as f64 - tab.grid.scroll) as f32;
                let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("row-drop"))).with_clip_rect(body.expand2(vec2(0.0, 2.0)));
                fg.rect_filled(Rect::from_min_max(pos2(body.min.x, y - 1.0), pos2(body.max.x, y + 1.0)), 1.0, p.prim);
                fg.circle_filled(pos2(body.min.x + 4.0, y), 4.0, p.prim);
            }
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            ctx.request_repaint();
        }
    }
    if resp.drag_stopped() {
        if let Some((id, Some((r, after)))) = tab.grid.row_drag.take() {
            if r < rows { actions.push(Action::MoveRow { id, target: tab.shown_id(r), after }); }
        }
    }
    if resp.dragged() && (tab.grid.selecting || tab.grid.fill_drag.is_some()) {
        if let Some(pt) = drag_pos {
            // Dragging past the edges scrolls, faster the further out.
            if pt.y > body.max.y { tab.grid.scroll += ((pt.y - body.max.y) as f64).min(200.0) * 0.5 + 4.0; }
            if pt.y < body.min.y { tab.grid.scroll -= ((body.min.y - pt.y) as f64).min(200.0) * 0.5 + 4.0; }
            tab.grid.sync(rows, ppp);
            let c = hit(pt.min(body.max - vec2(1.0, 1.0)).max(body.min), tab.grid.scroll, tab.grid.xoff, true);
            if let Some((_, at)) = &mut tab.grid.fill_drag {
                *at = c;
            } else if let Some(c) = c {
                tab.grid.cursor = Some(if tab.grid.selecting_rows { Cell { row: c.row, col: ncols.saturating_sub(1) } } else { c });
            }
            ctx.request_repaint();
        }
    }
    // The columns shown: hidden ones are stepped over by fills, as by paste and Delete.
    let all_cols: Vec<usize> = (0..ncols).filter(|c| !tab.grid.hidden.contains(c)).collect();
    let fill_target = tab.grid.fill_drag.and_then(|(sel, at)| at.and_then(|c| series::fill_target(sel, c.row, c.col, &all_cols)));
    if resp.drag_stopped() {
        tab.grid.selecting = false;
        tab.grid.selecting_rows = false;
        if let Some((sel, _)) = tab.grid.fill_drag.take() {
            if let Some((dir, target)) = fill_target {
                let (st2, view) = (st.clone(), tab.view.clone());
                let vis = all_cols.clone();
                tab.grid.fill_job = Some(Job::spawn(&ctx, move |cancel, _| series::fill_handle(&st2, &ids_of(&st2, &view), &vis, sel, dir, target, ctrl, cancel)));
                // The selection grows over what was filled.
                let (a, c) = match dir {
                    series::Dir::Down => (Cell { row: sel.r0, col: sel.c0 }, Cell { row: target.r1, col: sel.c1 }),
                    series::Dir::Up => (Cell { row: sel.r1, col: sel.c0 }, Cell { row: target.r0, col: sel.c1 }),
                    series::Dir::Right => (Cell { row: sel.r0, col: sel.c0 }, Cell { row: sel.r1, col: target.c1 }),
                    series::Dir::Left => (Cell { row: sel.r0, col: sel.c1 }, Cell { row: sel.r1, col: target.c0 }),
                };
                tab.grid.anchor = Some(a);
                tab.grid.cursor = Some(c);
            }
        }
    }
    if resp.double_clicked() && !on_handle {
        if let Some(c) = resp.interact_pointer_pos().filter(|q| q.x >= cells_x0).and_then(|pt| hit(pt, tab.grid.scroll, tab.grid.xoff, false)) {
            let id = tab.shown_id(c.row);
            tab.grid.editing = Some(Editing { at: c, id, text: st.cell(id, c.col), fresh: false, select: false });
        }
    }
    if resp.has_focus() && tab.grid.editing.is_none() {
        ui.memory_mut(|mem| mem.set_focus_lock_filter(body_id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        keys(ui, tab, &st, rows, ncols, page, view_w, &mut actions);
        tab.grid.sync(rows, ppp);
    }
    let off = tab.grid.off;
    let row_y = |pos: usize, top: usize| body.min.y + (pos as f32 - top as f32) * m.row_h - off;

    // Visible columns (after the keys: they may scroll): those scrolled, then the frozen ones.
    let xoff = tab.grid.xoff;
    // Each column's edges on physical pixels: rules, fills and texts crisp, nothing blurred
    // by a fractional width (auto widths are character counts × the font's advance).
    let snap = |v: f32| (v * ppp).round() / ppp;
    let mut vis_scroll = Vec::new();
    {
        let mut x = scroll_x0 - xoff;
        for c in frozen..ncols {
            let w = widths[c];
            if w > 0.0 && x + w > scroll_x0 && x < body.max.x { vis_scroll.push((c, snap(x), snap(x + w) - snap(x))); }
            x += w;
            if x > body.max.x { break; }
        }
    }
    let mut vis_frz = Vec::new();
    {
        let mut x = cells_x0;
        for c in (0..frozen).filter(|&c| widths[c] > 0.0) { vis_frz.push((c, snap(x), snap(x + widths[c]) - snap(x))); x += widths[c]; }
    }
    let vis_all: Vec<(usize, f32, f32)> = vis_scroll.iter().chain(vis_frz.iter()).copied().collect();
    let scroll_clip = Rect::from_min_max(pos2(scroll_x0, body.min.y), body.max);
    let frz_clip = Rect::from_min_max(pos2(cells_x0, body.min.y), pos2(scroll_x0, body.max.y));

    // Groups of a sorted view: a line where the first sort key's value changes.
    let group_col = tab.sort.first().map(|k| k.col).filter(|&c| c < ncols).filter(|&c| {
        let key = (tab.view.as_ref().map_or(0, |v| Arc::as_ptr(v) as usize), rows, tab.generation, c);
        match tab.grid.groups {
            Some((k, on)) if k == key => on,
            _ => {
                let on = tab.view.as_ref().is_some_and(|v| look::has_groups(&st, v, c));
                tab.grid.groups = Some((key, on));
                on
            }
        }
    });

    lap(tab, 0, &mut tl);
    // ---- Paint the body.
    let pb = painter.with_clip_rect(body);
    let pscroll = painter.with_clip_rect(scroll_clip);
    let pfrz = painter.with_clip_rect(frz_clip);
    let hover_pos = ui.input(|i| i.pointer.hover_pos()).filter(|pt| body.contains(*pt) && ui.rect_contains_pointer(body));
    let hover_row = hover_pos.map(|pt| ((tab.grid.scroll + (pt.y - body.min.y) as f64) / m.row_h as f64) as usize).filter(|&r| r < rows);
    let hover_cell = hover_pos.filter(|q| q.x >= cells_x0).and_then(|q| hit(q, tab.grid.scroll, xoff, false));
    // The pointer: a cell's cross over the data (as the web's `cursor: cell`), a hand on the row numbers.
    if let Some(q) = hover_pos.filter(|_| !on_handle && tab.grid.fill_drag.is_none() && tab.grid.col_drag.is_none()) {
        ctx.set_cursor_icon(if q.x >= cells_x0 { CursorIcon::Cell } else { CursorIcon::PointingHand });
    }
    let end = (tab.grid.top + page + 2).min(rows);
    let cur_row = tab.grid.cursor.map(|c| c.row);
    let spark_base: Option<Arc<Spark>> = tab.spark.base.clone();
    let mut hover_cut = false;
    // The rows' motion when the view changed (a sort, a filter), started before anything is drawn.
    motion::start(tab, &st, &ctx, body, end);
    let mo = tab.grid.motion.take();
    let mo_t = mo.as_ref().map(|x| hold().unwrap_or_else(|| x.start.elapsed().as_secs_f32()));
    // The cells' galleys kept from the last frame, the flashes of the cells just edited.
    let mut cells = std::mem::take(&mut tab.grid.cells);
    cells.begin(&ctx, &st);
    let flash_a = flash_alphas(&mut tab.grid.flash, &ctx);
    let mut flashed = Vec::new();
    let mut drawn = std::mem::take(&mut tab.grid.drawn);
    drawn.clear();
    // What the search and each column's filter matched, marked in the cells: each word, or
    // the regex (nothing when inverted: the rows shown are those without it). Made again only
    // when they change.
    let hl = {
        let (rx, case) = (tab.filter.regex, tab.filter.match_case);
        let off = tab.filter.invert || tab.expr_mode;
        let fresh = |k: &HlKey| k.global == tab.filter.global && k.cols.len() == ncols && k.cols.iter().zip(&tab.col_filters).all(|(a, b)| a == b) && k.regex == rx && k.case == case && k.off == off;
        match &tab.grid.hl {
            Some((k, h, n)) if fresh(k) => (h.clone(), *n),
            _ => {
                let global = if off { Hl::default() } else { Hl::new(&tab.filter.global, rx, case) };
                let cols: Vec<Hl> = (0..ncols)
                    .map(|c| {
                        let f = tab.col_filters.get(c).map_or("", |s| s.as_str());
                        if tab.filter.invert || (!rx && f.starts_with(['=', '!', '<', '>'])) { Hl::default() } else { Hl::new(f, rx, case) }
                    })
                    .collect();
                let h = Arc::new((global, cols));
                let key = HlKey { global: tab.filter.global.clone(), cols: (0..ncols).map(|c| tab.col_filters.get(c).cloned().unwrap_or_default()).collect(), regex: rx, case, off };
                // Numbered: a cell's marks are told apart from those of the searches before.
                let n = tab.grid.hl.as_ref().map_or(1, |x| x.2 + 1);
                tab.grid.hl = Some((key, h.clone(), n));
                (h, n)
            }
        }
    };
    let (hl, hl_n) = hl;
    // A regex find's pattern, compiled once per change of it.
    let find_re = if tab.find.open && tab.find.regex && !tab.find.text.is_empty() {
        let key = (tab.find.text.clone(), tab.find.match_case);
        match &tab.grid.find_re {
            Some((k, re)) if *k == key => re.clone(),
            _ => {
                let re = csvfab_engine::re::Re::new(&key.0, !key.1, 1 << 20).ok().map(Arc::new);
                tab.grid.find_re = Some((key, re.clone()));
                re
            }
        }
    } else {
        None
    };
    {
        let tabr: &Tab = tab;
        let fold = csvfab_engine::text::fold;
        let find = (tabr.find.open && !tabr.find.text.is_empty()).then(|| FindLook {
            needle: if tabr.find.match_case { tabr.find.text.clone() } else { fold(&tabr.find.text) },
            case: tabr.find.match_case,
            regex: tabr.find.regex,
            col: tabr.find.col,
            re: find_re.clone(),
            matches: tabr.find.matches.clone(),
            current: tabr.find.current.and_then(|i| tabr.find.matches.get(i).copied()),
        });
        let range = tabr.grid.range();
        let multi = range.as_ref().is_some_and(|(r, cs)| r.start() != r.end() || cs.start() != cs.end());
        // Raw values: none of what is drawn from them beyond the text.
        let raw = crate::theme::raw();
        let (no_heat, no_bars) = (HashMap::new(), HashMap::new());
        let lk = Look {
            st: &st,
            p,
            m,
            kinds: &tabr.kinds,
            rich: if raw { &[] } else { &tabr.rich },
            pills: if raw { &[] } else { &tabr.pills },
            marks: &tabr.marks,
            dups: tabr.marks.dups.as_deref(),
            heat: if raw { &no_heat } else { &tabr.grid.heat },
            bars: if raw { &no_bars } else { &tabr.grid.bars },
            lit: tabr.spark.lit.and_then(|(c, pk)| spark_base.as_ref().filter(|s| c < s.cols.len()).map(|s| (c, s.keep(c, pk)))),
            hl: &hl,
            hl_gen: {
                // Doubled: the `| 1` below must not make two searches in a row one number.
                let mut h = Fx(hl_n << 1);
                if let Some(f) = &find {
                    h.mix(Arc::as_ptr(&f.matches) as u64 ^ (f.matches.len() as u64) << 44);
                    h.mix(f.needle.len() as u64 ^ (f.case as u64) << 40 ^ (f.regex as u64) << 41 ^ (f.col.map_or(u64::MAX, |c| c as u64)) << 8);
                    for b in f.needle.bytes() { h.mix(b as u64); }
                }
                h.0 | 1
            },
            find,
            ghost: tabr.ghost.as_deref(),
            ghost_t: match &tabr.ghost {
                Some(g) if ctx.global_style().animation_time > 0.0 => {
                    let t = g.born.elapsed().as_secs_f32();
                    if t < GHOST_IN.1 { ctx.request_repaint(); }
                    t
                }
                _ => f32::INFINITY,
            },
            editing: tabr.grid.editing.as_ref().map(|e| e.at),
            range,
            multi,
            fill_target: fill_target.map(|x| x.1),
            tint: tabr.grid.focus_col.map(|c| (c, alpha(p.accent, 0.07))),
            units: !raw,
            font: mono(m),
            font_bold: mono_bold(m.font_grid),
            pill_font: FontId::proportional(12.0),
            cw: char_w(&ctx, m),
            ppp,
            ctrl,
            syn: (st.base.dialect.lines && !raw).then(|| crate::syntax::lang_of(&tabr.name).g),
        };
        let num_font = FontId::monospace(12.0);
        let mut cx = CellCx { cells: &mut cells, flashed: &mut flashed, flashes: &flash_a, tmp: Vec::new(), hits: Vec::new(), hits2: Vec::new(), row: RowFields::default() };
        // The fields the columns on screen read, in record order.
        let mut want: Vec<(usize, usize)> = vis_scroll.iter().chain(vis_frz.iter()).filter_map(|v| match &st.cmap { None => Some((v.0, v.0)), Some(m) => m.get(v.0).filter(|&&f| f >= 0).map(|&f| (v.0, f as usize)) }).collect();
        want.sort_unstable_by_key(|w| w.1);
        let mut rtmp = Vec::new();
        let mut gtmp = Vec::new();
        let mut prev_group: Option<String> = None;
        let dup_view = crate::tools::dupes::in_view(tabr);
        let merge = crate::tools::dupes::merge_hint(&ctx);
        let gw = gut - m.gutter_rule_w;
        // The hover glow's strength: it fades in over 100 ms on each cell (the peek's timer).
        let glow = match (hover_cell, tabr.grid.peek) {
            (Some(h), Some((c, t))) if c == h && !resp.dragged() => (t.elapsed().as_secs_f32() / 0.1).min(1.0),
            _ => 0.0,
        };
        if hover_cell.is_some() && glow < 1.0 { ctx.request_repaint(); }
        let multi_sel = lk.multi;
        // One row: its fills and rules as one mesh under its texts. `pos` is None for a row
        // of the old view still moving away; `op` its opacity.
        let mut draw_row = |cx: &mut CellCx, pos: Option<usize>, id: RowId, ry: f32, op: f32| -> bool {
            let mut hover_cut = false;
            let (mut pb, mut pscroll, mut pfrz) = (pb.clone(), pscroll.clone(), pfrz.clone());
            if op < 1.0 { for p in [&mut pb, &mut pscroll, &mut pfrz] { p.multiply_opacity(op); } }
            let at = pb.add(egui::Shape::Noop);
            let mut q = Quads::default();
            let ps = pos.unwrap_or(usize::MAX);
            let rrect = Rect::from_min_size(pos2(body.min.x, ry), vec2(body.width(), m.row_h));
            let rec = (id as usize + 1).saturating_sub(st.header as usize);
            let base_row = (id as usize) < st.base.n;
            let even = if base_row { rec % 2 == 0 } else { ps % 2 == 1 };
            let fill = p.row_fill(even, pos.is_some() && hover_row == pos, pos.is_some() && cur_row == pos);
            q.add(rrect, body, fill);
            let dg = lk.dups.and_then(|d| d.group(id));
            let diff = dg.map(|g| lk.dups.unwrap().diff(&st, g));
            let diffs: &[usize] = diff.as_deref().map_or(&[], |v| v.as_slice());
            cx.row.load(&st, id, &want, &mut rtmp);
            for (list, pt, clip, frz) in [(&vis_scroll, &pscroll, scroll_clip, false), (&vis_frz, &pfrz, frz_clip, true)] {
                for &(c, x, w) in list.iter() {
                    let cut = lk.cell(pt, &mut q, clip, cx, ps, id, c, x, w, ry, even, fill, frz, diffs.contains(&c));
                    if pos.is_some() && hover_cell == Some(Cell { row: ps, col: c }) { hover_cut = cut; }
                }
            }
            // The cell under the pointer glows softly, fading in (not the cursor's: it has its frame).
            if let Some(hc) = hover_cell.filter(|h| pos == Some(h.row) && glow > 0.0 && tabr.grid.cursor != Some(*h) && !multi_sel) {
                if let Some((&(_, x, w), clip)) = vis_scroll.iter().find(|v| v.0 == hc.col).map(|v| (v, scroll_clip)).or_else(|| vis_frz.iter().find(|v| v.0 == hc.col).map(|v| (v, frz_clip))) {
                    let r = Rect::from_min_size(pos2(x, ry), vec2(w - 1.0, m.row_h - 1.0));
                    q.add(r, clip, alpha(p.accent, 0.07 * glow));
                    let ring = alpha(p.accent, 0.30 * glow);
                    let t = 1.0;
                    for e in [Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + t)), Rect::from_min_max(pos2(r.min.x, r.max.y - t), r.max), Rect::from_min_max(pos2(r.min.x, r.min.y + t), pos2(r.min.x + t, r.max.y - t)), Rect::from_min_max(pos2(r.max.x - t, r.min.y + t), pos2(r.max.x, r.max.y - t))] {
                        q.add(e, clip, ring);
                    }
                }
            }
            q.add(Rect::from_min_max(pos2(body.min.x, ry + m.row_h - 1.0), pos2(body.max.x, ry + m.row_h)), body, p.brd);
            // Row number, and its marks.
            let gr = Rect::from_min_size(pos2(body.min.x, ry), vec2(gw, m.row_h - 1.0));
            let selrow = pos.is_some() && lk.range.as_ref().is_some_and(|(r, cs)| r.contains(&ps) && *cs.start() == 0 && cs.end() + 1 == ncols);
            q.add(gr, body, if selrow { alpha(p.prim, 0.25) } else if pos.is_some() && hover_row == pos { p.idx_hover } else { p.surf_2 });
            let irregular = base_row && st.base.odd.binary_search_by_key(&id, |&(r, _)| r).is_ok();
            if irregular { q.add(Rect::from_min_size(gr.min, vec2(3.0, m.row_h - 1.0)), body, p.warn); }
            if tabr.marks.rows.contains(&id) { q.add(Rect::from_min_size(pos2(gr.max.x - 3.0, gr.min.y), vec2(3.0, m.row_h - 1.0)), body, p.warn); }
            if let Some(g) = dg {
                // Duplicate groups: a bar at the gutter's right edge, every other group in another colour.
                q.add(Rect::from_min_size(pos2(gr.max.x - 3.0, gr.min.y), vec2(3.0, m.row_h - 1.0)), body, if g % 2 == 0 { p.violet_txt } else { p.cyan_txt });
                if dup_view && ps > 0 && ps != usize::MAX && lk.dups.unwrap().group(tabr.shown_id(ps - 1)) != Some(g) {
                    q.add(Rect::from_min_size(pos2(body.min.x, ry), vec2(body.width(), 2.0)), body, alpha(p.violet, 0.5));
                }
            }
            q.flush(&pb, Some(at));
            match merge.filter(|h| dg == Some(h.1)) {
                // A merge being pointed at: ✓ on the row kept, ✕ on the others of its group.
                Some((keep, _)) => {
                    let (icon, c) = if keep == id { (Icon::MergeKeep, p.txt_strong) } else { (Icon::MergeDrop, p.diff_txt) };
                    icons::paint(&pb, icon, Rect::from_center_size(pos2(gr.center().x, ry + (m.row_h - 1.0) / 2.0), vec2(16.0, 16.0)), c);
                }
                None => {
                    // The number's galley is kept as the cells' (key: the column past the last).
                    let num_c = if selrow { p.accent_2 } else { p.txt_dim };
                    let look = TextLook { color: num_c, font: 9, max_w: rec as u32, marks: base_row as u64 };
                    let e = cx.cells.map.entry((id, u32::MAX)).or_insert(CellG { src: 0, look, galley: None, cut: false, seen: 0, hl: 0 });
                    e.seen = cx.cells.frame;
                    if e.look != look || e.galley.is_none() {
                        e.look = look;
                        e.galley = Some(pb.layout_no_wrap(if base_row { fmt_int(rec) } else { "+".into() }, num_font.clone(), num_c));
                    }
                    let ng = e.galley.clone().unwrap();
                    pb.galley(pos2(lk.snap(gr.center().x - ng.size().x / 2.0), lk.snap(ry + (m.row_h - 1.0 - ng.size().y) / 2.0)), ng, num_c);
                }
            }
            hover_cut
        };
        // A motion: the old screen faint behind, the rows gone moving off (or fading), then the
        // rows of the new view, those in place first.
        if let (Some(mo), Some(t)) = (&mo, mo_t) {
            for &(id, y) in &mo.ghost {
                let a = 0.5 * (1.0 - motion::ease_in(t / motion::GHOST));
                if a > 0.01 { draw_row(&mut cx, None, id, lk.snap(body.min.y + y), a); }
            }
            for g in &mo.gone {
                let (y, a) = g.at(t, mo.sort);
                if a > 0.01 && y > -m.row_h && y < body.height() { draw_row(&mut cx, None, g.id, lk.snap(body.min.y + y), a); }
            }
        }
        let mut later: Vec<(usize, RowId, f32, f32)> = Vec::new();
        for pos in tabr.grid.top..end {
            let ry = row_y(pos, tabr.grid.top);
            let id = tabr.shown_id(pos);
            let at = mo.as_ref().zip(mo_t).and_then(|(mo, t)| mo.from.get(&id).map(|f| f.at(ry - body.min.y, t, mo.sort)));
            let (y, a) = match at {
                Some((y, a)) => { later.push((pos, id, lk.snap(body.min.y + y), a)); continue; }
                None => (ry, 1.0),
            };
            if draw_row(&mut cx, Some(pos), id, y, a) { hover_cut = true; }
            drawn.push((id, y - body.min.y));
        }
        for &(pos, id, y, a) in &later {
            if y > body.min.y - m.row_h && y < body.max.y && draw_row(&mut cx, Some(pos), id, y, a) { hover_cut = true; }
            drawn.push((id, y - body.min.y));
        }
        // Groups of a sorted view: a line at the top of each row whose sort value differs from
        // the row above's (once the rows are in place).
        if let Some(gc) = group_col.filter(|_| mo.is_none()) {
            for pos in tabr.grid.top..end {
                let id = tabr.shown_id(pos);
                let v = st.with_cell(id, gc, &mut gtmp, |s| s.trim().to_string());
                let prev = match prev_group.take() {
                    Some(x) => Some(x),
                    None if pos > 0 => Some(st.with_cell(tabr.shown_id(pos - 1), gc, &mut gtmp, |s| s.trim().to_string())),
                    None => None,
                };
                if prev.is_some_and(|x| x != v) {
                    pb.hline(cells_x0..=body.max.x, row_y(pos, tabr.grid.top) + 0.5, Stroke::new(1.0, p.group_line));
                }
                prev_group = Some(v);
            }
        }
        // The columns' rules, top to bottom of the rows, as one mesh.
        let bottom = if mo.is_some() { body.max.y } else { row_y(end, tabr.grid.top).min(body.max.y) };
        let mut q = Quads::default();
        for (list, clip, frz) in [(&vis_scroll, scroll_clip, false), (&vis_frz, frz_clip, true)] {
            for (k, &(_, x, w)) in list.iter().enumerate() {
                let last_frozen = frz && k + 1 == list.len();
                q.add(Rect::from_min_max(pos2(x + w - 1.0, body.min.y), pos2(x + w, bottom)), clip, if last_frozen { p.frz_rule } else { p.brd });
            }
        }
        q.flush(&pb, None);
    }
    cells.end();
    tab.grid.cells = cells;
    tab.grid.drawn = drawn;
    tab.grid.drawn_scroll = tab.grid.scroll;
    let now = Instant::now();
    for (id, c) in flashed { tab.grid.flash.insert((id, c as u32), now); }
    if let (Some(mo), Some(t)) = (mo, mo_t) {
        if t < mo.dur { tab.grid.motion = Some(mo); ctx.request_repaint(); }
    }
    // The cursor cell's frame, the fill handle, the fill's target.
    let cell_rect = |c: Cell, g: &GridState| -> Option<(Rect, bool)> {
        if c.row < g.top || c.row >= end { return None; }
        let &(_, x, w) = vis_all.iter().find(|v| v.0 == c.col)?;
        Some((Rect::from_min_size(pos2(x, row_y(c.row, g.top)), vec2(w - 1.0, m.row_h - 1.0)), c.col < frozen))
    };
    let clip_of = |frz: bool| if frz { &pfrz } else { &pscroll };
    if let Some(c) = tab.grid.cursor {
        if let Some((r, frz)) = cell_rect(c, &tab.grid) {
            clip_of(frz).rect_stroke(r, 0.0, Stroke::new(2.0, p.cursor), StrokeKind::Inside);
        }
        if tab.grid.fill_drag.is_none() {
            if let (Some(h), Some((_, cs))) = (handle_rect(&tab.grid), tab.grid.range()) {
                pb.with_clip_rect(if *cs.end() < frozen { frz_clip } else { scroll_clip }.expand2(vec2(0.0, 4.0))).rect(h, 1.0, p.prim, Stroke::new(1.0, p.bg), StrokeKind::Outside);
            }
        }
    }
    if let Some((_, t)) = fill_target {
        // The target's frame, over the visible part.
        let a = Cell { row: t.r0.max(tab.grid.top), col: t.c0 };
        let b = Cell { row: t.r1.min(end.saturating_sub(1)), col: t.c1 };
        if let (Some((ra, fa)), Some((rb, _))) = (cell_rect(a, &tab.grid), cell_rect(b, &tab.grid)) {
            clip_of(fa).rect_stroke(ra.union(rb), 0.0, Stroke::new(1.0, p.fill_preview_brd), StrokeKind::Inside);
        }
    }
    if rows == 0 {
        let msg = if st.is_empty() { "Empty file" } else { "No row matches the filters" };
        pb.text(body.center_top() + vec2(0.0, 40.0), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), p.txt_mut);
    }
    // Gutter rule, on every band.
    painter.rect_filled(Rect::from_min_max(pos2(cells_x0 - m.gutter_rule_w, full.min.y), pos2(cells_x0, body.max.y + foot)), 0.0, p.prim);

    lap(tab, 1, &mut tl);
    // ---- Over the rows: the ⋮ of the hovered row, the group tag, a mark's tooltip, the peek.
    row_button(ui, tab, &painter, body, hover_row, &row_y, p, m);
    if let Some(gc) = group_col {
        group_tag(tab, &st, &painter, body, &vis_all, gc, frozen, scroll_x0, p);
    }
    dup_hover(ui, tab, &st, &painter, body, cells_x0, hover_row, hover_cell, &cell_rect, &row_y, p, m, &mut actions);
    if let Some(c) = hover_cell.filter(|_| !resp.dragged()) {
        let id = tab.shown_id(c.row);
        if let Some(r) = cell_rect(c, &tab.grid).map(|x| x.0) {
            if let Some(exp) = tab.marks.cells.get(&(id, c.col)) {
                tip(&ctx, p, m, r, exp);
            } else {
                peek(tab, &st, &ctx, c, r, hover_cut, p, m);
            }
        }
    } else {
        tab.grid.peek = None;
    }

    // ---- Cell editor, header bands, totals, scroll strips, menus.
    editor(ui, tab, &st, &painter, body, &vis_all, end, rows, ncols, view_w, p, m, &resp, &mut actions, off);
    lap(tab, 2, &mut tl);
    header(ui, tab, &st, &painter, &band, &vis_scroll, &vis_frz, cells_x0, scroll_x0, full, p, m, &mut actions);
    lap(tab, 3, &mut tl);
    if let Some(r) = band.charts {
        crate::spark_ui::frame_start(tab);
        crate::spark_ui::band(ui, tab, &painter, r, Rect::from_min_max(pos2(scroll_x0, r.min.y), r.max), &vis_scroll, p, m);
        if frozen > 0 {
            crate::spark_ui::band(ui, tab, &painter, r, Rect::from_min_max(pos2(cells_x0, r.min.y), pos2(scroll_x0, r.max.y)), &vis_frz, p, m);
        }
        crate::spark_ui::frame_end(tab);
    } else {
        tab.spark.lit = None;
    }
    lap(tab, 4, &mut tl);
    if foot > 0.0 {
        let r = Rect::from_min_max(pos2(full.min.x, body.max.y), pos2(body.max.x, body.max.y + foot));
        crate::totals_ui::row(ui, tab, &painter, r, p, m, &mut actions);
        crate::totals_ui::cells(ui, tab, &painter, r, Rect::from_min_max(pos2(scroll_x0, r.min.y), r.max), &vis_scroll, p, m, false);
        if frozen > 0 {
            crate::totals_ui::cells(ui, tab, &painter, r, Rect::from_min_max(pos2(cells_x0, r.min.y), pos2(scroll_x0, r.max.y)), &vis_frz, p, m, true);
        }
        painter.rect_filled(Rect::from_min_max(pos2(cells_x0 - m.gutter_rule_w, r.min.y + 1.0), pos2(cells_x0, r.max.y)), 0.0, p.prim);
    }
    lap(tab, 5, &mut tl);
    strips(ui, tab, &st, &painter, full, rows, page, total_w, view_w, cells_x0, frz_w, p, m);
    if let Some(c) = tab.grid.menu_at.filter(|_| !gutter_menu) {
        let heads = titles(&st, tab);
        resp.context_menu(|ui| cell_menu(ui, tab, &st, c, &heads, p, &mut actions));
    }
    row_menu(ui, tab, &st, ncols, p, m, &mut actions);
    lap(tab, 6, &mut tl);
    timing(tab, t0);
    actions
}

/// The wheel, our own way (egui's smoothing gives each notch a long tail the hand feels):
/// a touchpad's pixels apply at once; a notch is 3 rows, eased over ~55 ms toward its target
/// (the next frame already moves a quarter of the way), faster and faster when notches come
/// in quick succession (×1.6 each within 60 ms, up to ×8; back to ×1 after 200 ms), so a
/// flick crosses thousands of rows. Shift: sideways.
#[derive(Default)]
struct Wheel {
    /// What the notches still have to move: (x, y) pixels.
    left: (f32, f64),
    last_notch: f64,
    speed: f32,
    last: f64,
}

/// The notches' ease time constant (s).
const WHEEL_TAU: f32 = 0.055;

fn wheel(ui: &Ui, g: &mut GridState, over: bool, max_x: f32, rows: usize, row_h: f32) {
    use egui::{Event, MouseWheelUnit};
    let (now, dt, mut px, notches) = ui.input(|i| {
        let (mut px, mut notches) = (Vec2::ZERO, Vec2::ZERO);
        for e in &i.events {
            if let Event::MouseWheel { unit, delta, modifiers, .. } = e {
                // Ctrl+wheel zooms (egui's), it does not scroll.
                if modifiers.command { continue; }
                // Shift turns a vertical wheel sideways.
                let d = if modifiers.shift && delta.x == 0.0 { vec2(delta.y, 0.0) } else { *delta };
                match unit {
                    MouseWheelUnit::Point => px += d,
                    MouseWheelUnit::Line => notches += d,
                    MouseWheelUnit::Page => px += d * i.content_rect().height(),
                }
            }
        }
        (i.time, i.stable_dt.clamp(1.0 / 500.0, 0.05), px, notches)
    });
    let w = &mut g.wheel;
    if !over { px = Vec2::ZERO; }
    if over && notches != Vec2::ZERO {
        let gap = now - w.last_notch;
        w.speed = if gap < 0.06 { (w.speed * 1.6).clamp(1.0, 8.0) } else if gap > 0.2 { 1.0 } else { w.speed.max(1.0) };
        w.last_notch = now;
        let step = 3.0 * row_h * w.speed;
        w.left.0 += notches.x * step;
        w.left.1 += (notches.y * step) as f64;
        // Reversing the wheel drops what was left the other way.
        if w.left.1.signum() != notches.y.signum() as f64 && notches.y != 0.0 { w.left.1 = (notches.y * step) as f64; }
    }
    // The touchpad: as it comes.
    g.scroll -= px.y as f64;
    g.xoff = (g.xoff - px.x).clamp(0.0, max_x);
    let w = &mut g.wheel;
    if w.left != (0.0, 0.0) {
        // Since the last frame (a notch arriving on an idle grid moves at once, by a frame's share).
        let dt = if now - w.last > 0.1 { dt } else { ((now - w.last) as f32).max(1.0 / 500.0) };
        let k = 1.0 - (-dt / WHEEL_TAU).exp();
        let (mut sx, mut sy) = (w.left.0 * k, w.left.1 * k as f64);
        if w.left.0.abs() < 0.5 { sx = w.left.0; }
        if w.left.1.abs() < 0.5 { sy = w.left.1; }
        w.left.0 -= sx;
        w.left.1 -= sy;
        g.scroll -= sy;
        g.xoff = (g.xoff - sx).clamp(0.0, max_x);
        // At an end, what is left goes nowhere.
        if g.scroll <= 0.0 || g.scroll >= g.max_scroll(rows) { g.wheel.left.1 = 0.0; }
        if g.xoff <= 0.0 || g.xoff >= max_x { g.wheel.left.0 = 0.0; }
        if g.wheel.left != (0.0, 0.0) { ui.ctx().request_repaint(); }
    }
    g.wheel.last = now;
}

/// A touchpad's scroll going on after the fingers lift, slowing down: Wayland leaves kinetic
/// scrolling to the app (the compositor sends the lift, `TouchPhase::End`), and egui does not
/// do it. Any other input stops it at once.
#[derive(Default)]
struct Fling {
    /// The touchpad's last motions: (frame time, delta that frame).
    recent: Vec<(f64, Vec2)>,
    v: Vec2,
    last: f64,
}

/// Time constant of the slowdown (s): a fling of 3 000 pt/s glides about 1 000 pt.
const FLING_TAU: f32 = 0.33;

fn fling(ui: &Ui, g: &mut GridState, over: bool, max_x: f32, rows: usize) {
    use egui::{Event, MouseWheelUnit, TouchPhase};
    let (now, frame_dt, sum, lifted, stop) = ui.input(|i| {
        let (mut sum, mut lifted, mut stop) = (Vec2::ZERO, false, i.pointer.any_pressed() || i.events.iter().any(|e| matches!(e, Event::Key { pressed: true, .. })));
        for e in &i.events {
            if let Event::MouseWheel { unit, delta, phase, .. } = e {
                match (unit, phase) {
                    (MouseWheelUnit::Point, TouchPhase::Move) => { sum += *delta; stop = true; }
                    (MouseWheelUnit::Point, TouchPhase::End) => lifted = true,
                    _ => stop = true,
                }
            }
        }
        (i.time, i.unstable_dt.clamp(1.0 / 240.0, 1.0 / 30.0), sum, lifted, stop)
    });
    let max_y = g.max_scroll(rows);
    let f = &mut g.fling;
    if stop { f.v = Vec2::ZERO; }
    if sum != Vec2::ZERO && over { f.recent.push((now, sum)); }
    f.recent.retain(|r| r.0 >= now - 0.1);
    if lifted && over {
        // The speed of the last 100 ms, if the fingers were still moving when they lifted.
        if f.recent.last().is_some_and(|r| now - r.0 < 0.05) {
            let span = (f.recent.last().unwrap().0 - f.recent[0].0) as f32 + frame_dt;
            let v = f.recent.iter().fold(Vec2::ZERO, |a, r| a + r.1) / span;
            if v.length() > 100.0 { f.v = v.clamp(Vec2::splat(-9000.0), Vec2::splat(9000.0)); }
        }
        f.recent.clear();
    }
    if f.v != Vec2::ZERO {
        let dt = ((now - f.last) as f32).clamp(0.0, 0.05);
        g.scroll -= (f.v.y * dt) as f64;
        g.xoff = (g.xoff - f.v.x * dt).clamp(0.0, max_x);
        f.v *= (-dt / FLING_TAU).exp();
        // Stopped by the ends, or slow enough to stop.
        if g.scroll <= 0.0 || g.scroll >= max_y { f.v.y = 0.0; }
        if g.xoff <= 0.0 || g.xoff >= max_x { f.v.x = 0.0; }
        if f.v.length() < 20.0 { f.v = Vec2::ZERO; } else { ui.ctx().request_repaint(); }
    }
    f.last = now;
}

/// The screenshot hooks of the grid's states (`CSVFAB_SHOT`, see shot.rs).
fn shot_hooks(tab: &mut Tab, ctx: &egui::Context, resp: &egui::Response, ncols: usize) {
    use crate::shot::take;
    if let Some(px) = take("scroll") { tab.grid.scroll = px.parse().unwrap_or(0.0); }
    if take("cell").is_some() {
        tab.grid.select(Cell { row: 2, col: 3 }, false);
        tab.grid.menu_at = Some(Cell { row: 2, col: 3 });
        egui::Popup::open_id(ctx, egui::Popup::default_response_id(resp));
    }
    // A sort or a filter once a frame is drawn: the rows' motion has a picture to start from.
    let drawn = !tab.grid.drawn.is_empty();
    if let Some(v) = drawn.then(|| take("sort")).flatten() {
        // `sort:3` ascending, `sort:-3` descending.
        let desc = v.starts_with('-');
        if let Ok(c) = v.trim_start_matches('-').parse::<usize>() {
            if c < ncols { tab.sort = vec![SortKey { col: c, desc }]; tab.refresh(ctx); }
        }
    }
    if take("edit").is_some() {
        let c = Cell { row: 2, col: 1 };
        tab.grid.select(c, false);
        let id = tab.shown_id(2);
        tab.grid.editing = Some(Editing { at: c, id, text: tab.st().map(|s| s.cell(id, 1)).unwrap_or_default(), fresh: false, select: false });
    }
    if let Some(v) = drawn.then(|| take("filter")).flatten() {
        // `filter:3=text`: a column filter.
        if let Some((c, t)) = v.split_once('=') {
            if let Ok(c) = c.parse::<usize>() {
                if c < tab.col_filters.len() { tab.col_filters[c] = t.to_string(); tab.refresh(ctx); }
            }
        }
    }
    if let Some(v) = take("select") {
        // `select:r0.c0-r1.c1`
        let cell = |s: &str| s.split_once('.').and_then(|(r, c)| Some(Cell { row: r.parse().ok()?, col: c.parse().ok()? }));
        if let Some((a, b)) = v.split_once('-').and_then(|(a, b)| Some((cell(a)?, cell(b)?))) {
            tab.grid.anchor = Some(a);
            tab.grid.cursor = Some(b);
        }
    }
    if let Some(v) = take("freeze") { if let Ok(c) = v.parse::<usize>() { tab.grid.frozen = c; } }
    if take("totals").is_some() { tab.show_totals = true; }
    // `hide:1.3`: columns 1 and 3 hidden.
    if let Some(v) = take("hide") { tab.grid.hidden.extend(v.split('.').filter_map(|x| x.parse::<usize>().ok()).filter(|&c| c < ncols)); }
    // `flash`: cells (rows 2–3, columns 1–2) as if just edited.
    if take("flash").is_some() {
        for pos in 2..4.min(tab.shown()) {
            let id = tab.shown_id(pos);
            for c in 1..3 { tab.grid.flash.insert((id, c), Instant::now()); }
        }
    }
    if let Some(v) = take("heat") { if let Ok(c) = v.parse::<usize>() { toggle_look(tab, c, true, ctx); } }
    if let Some(v) = take("bars") { if let Ok(c) = v.parse::<usize>() { toggle_look(tab, c, false, ctx); } }
    if let Some(v) = take("xoff") { tab.grid.xoff = v.parse().unwrap_or(0.0); }
    // `autoscroll:PX`: scrolls PX a frame for ever (with CSVFAB_GRID_TIMING, to time the grid).
    static AUTO: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let auto = *AUTO.get_or_init(|| take("autoscroll").and_then(|v| v.parse().ok()).unwrap_or(0.0));
    if auto != 0.0 {
        tab.grid.scroll += auto;
        if tab.grid.scroll >= tab.grid.max_scroll(tab.shown()) { tab.grid.scroll = 0.0; }
        ctx.request_repaint();
    }
}

fn timing_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CSVFAB_GRID_TIMING").is_some())
}

/// With `CSVFAB_GRID_TIMING`: the time since the last lap, into part `k`.
fn lap(tab: &mut Tab, k: usize, at: &mut Instant) {
    crate::app::lap(["grid: input", "grid: rows", "grid: over", "grid: header", "grid: charts", "grid: totals", "grid: strips"][k]);
    if !timing_on() { return; }
    let now = Instant::now();
    tab.grid.timing.2[k] += now - *at;
    *at = now;
}

/// With `CSVFAB_GRID_TIMING` set: the grid's average frame time and its parts, every 120 frames, on stderr.
fn timing(tab: &mut Tab, t0: Instant) {
    if !timing_on() { return; }
    let t = &mut tab.grid.timing;
    t.0 += t0.elapsed();
    t.1 += 1;
    if t.1 == 120 {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / 120.0;
        let p = t.2.map(ms);
        eprintln!("grid: {:.3} ms a frame (120 frames) — input {:.3}, rows {:.3}, over {:.3}, header {:.3}, charts {:.3}, totals {:.3}, strips {:.3}", ms(t.0), p[0], p[1], p[2], p[3], p[4], p[5], p[6]);
        *t = Default::default();
    }
}

/// The peek: a value cut short, whole, after resting 450 ms on its cell.
#[allow(clippy::too_many_arguments)]
fn peek(tab: &mut Tab, st: &State, ctx: &egui::Context, c: Cell, r: Rect, cut: bool, p: &Palette, m: &Metrics) {
    let moved = ctx.input(|i| i.pointer.any_down() || i.smooth_scroll_delta != Vec2::ZERO || !i.keys_down.is_empty());
    match tab.grid.peek {
        Some((pc, _)) if pc == c && !moved => {}
        _ => { tab.grid.peek = Some((c, Instant::now())); }
    }
    if !cut || tab.grid.editing.is_some() { return; }
    let since = tab.grid.peek.map_or(Duration::ZERO, |x| x.1.elapsed());
    let wait = Duration::from_millis(450);
    if since < wait {
        ctx.request_repaint_after(wait - since);
        return;
    }
    let v = st.cell(tab.shown_id(c.row), c.col);
    if v.is_empty() { return; }
    let screen = ctx.content_rect();
    let max_w = 560.0f32.min(screen.width() - 24.0);
    tip_box(ctx, p, m, r, &peek_text(&v), FontId::monospace(12.5), 12.5 * 1.5, max_w, vec2(10.0, 7.0), true);
}

/// A hovered duplicate row: its bar (delete, merge); on a differing cell, ▲ / ▼ to copy its
/// value into the sister row above or below.
#[allow(clippy::too_many_arguments)]
fn dup_hover(ui: &mut Ui, tab: &mut Tab, st: &State, painter: &egui::Painter, body: Rect, cells_x0: f32, hover_row: Option<usize>, hover_cell: Option<Cell>, cell_rect: &dyn Fn(Cell, &GridState) -> Option<(Rect, bool)>, row_y: &dyn Fn(usize, usize) -> f32, p: &Palette, m: &Metrics, actions: &mut Vec<Action>) {
    use crate::tools::dupes::{self, DupAct};
    let ctx = ui.ctx().clone();
    let Some(dm) = tab.marks.dups.clone() else { return };
    let group = |pos: usize| (pos < tab.shown()).then(|| dm.group(tab.shown_id(pos))).flatten();
    let Some((pos, g)) = hover_row.and_then(|r| group(r).map(|g| (r, g))) else {
        ctx.data_mut(|d| d.remove::<(RowId, u32)>(Id::unique("dup-merge-hint")));
        return;
    };
    let id = tab.shown_id(pos);
    let ry = row_y(pos, tab.grid.top);
    let row = Rect::from_min_max(pos2(body.min.x, ry), pos2(body.max.x, ry + m.row_h));
    // The whole group framed in dashes, as far as it is in view (the web's dupGroupBox).
    {
        let (top, page) = (tab.grid.top, tab.grid.page.max(1));
        let mut a = pos;
        while a > top && group(a - 1) == Some(g) { a -= 1; }
        let mut z = pos;
        while z + 1 < top + page && group(z + 1) == Some(g) { z += 1; }
        if z > a {
            let fr = Rect::from_min_max(pos2(cells_x0 + 1.0, row_y(a, top) + 1.0), pos2(body.max.x - 2.0, row_y(z, top) + m.row_h - 1.0));
            let stroke = Stroke::new(1.5, alpha(p.warn, 0.85));
            for (s, e) in [(fr.left_top(), fr.right_top()), (fr.right_top(), fr.right_bottom()), (fr.right_bottom(), fr.left_bottom()), (fr.left_bottom(), fr.left_top())] {
                let len = (e - s).length();
                let dir = (e - s) / len.max(1.0);
                let mut t = 0.0;
                while t < len {
                    painter.line_segment([s + dir * t, s + dir * (t + 6.0).min(len)], stroke);
                    t += 10.0;
                }
            }
        }
    }
    if let Some(a) = dupes::bar(ui, p, tab, id, cells_x0 + 4.0, row) { actions.push(Action::Dup(a)); }
    let Some(c) = hover_cell.filter(|c| c.col != 0 && dm.diff(st, g).contains(&c.col)) else { return };
    let (up, down) = (pos > 0 && group(pos - 1) == Some(g), group(pos + 1) == Some(g));
    if !up && !down { return; }
    let Some((cr, _)) = cell_rect(c, &tab.grid) else { return };
    let n = up as usize + down as usize;
    let size = vec2(4.0 + 20.0 * n as f32 + (n - 1) as f32, 24.0);
    let bx = Rect::from_min_size(pos2(cr.max.x - 3.0 - size.x, (cr.center().y - size.y / 2.0).round()), size);
    let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("dup-cell")));
    fg.add(egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(102) }.as_shape(bx, 6.0));
    fg.rect(bx, 6.0, p.surf_3, Stroke::new(1.0, p.brd_2), StrokeKind::Inside);
    let mut x = bx.min.x + 2.0;
    for (on, dir) in [(up, -1i64), (down, 1)] {
        if !on { continue; }
        let br = Rect::from_min_size(pos2(x, bx.min.y + 2.0), vec2(20.0, 20.0));
        x += 21.0;
        let r = ui.interact(br, ui.scope_id().with(("dup-arrow", dir)), Sense::click());
        if r.hovered() {
            fg.rect_filled(br, 4.0, alpha(p.accent, 0.25));
            let target = Cell { row: (pos as i64 + dir) as usize, col: c.col };
            if let Some((tr, _)) = cell_rect(target, &tab.grid) {
                painter.with_clip_rect(body).rect_stroke(tr, 0.0, Stroke::new(2.0, alpha(p.accent, 0.7)), StrokeKind::Inside);
            }
            tip(&ctx, p, m, br, if dir < 0 { "Copy up" } else { "Copy down" });
        }
        icons::paint(&fg, if dir < 0 { Icon::ChevronUpBold } else { Icon::ChevronDownBold }, Rect::from_center_size(br.center(), vec2(14.0, 14.0)), p.txt_hi);
        if r.clicked() {
            actions.push(Action::Dup(DupAct::Copy { from: id, to: tab.shown_id((pos as i64 + dir) as usize), col: c.col }));
        }
    }
}

fn row_menu_id() -> Id {
    Id::unique("csvfab-row-menu")
}

/// The ⋮ of the hovered row: 18 × 22 at the gutter's right, 4 px in; opens the row's menu.
#[allow(clippy::too_many_arguments)]
fn row_button(ui: &mut Ui, tab: &mut Tab, painter: &egui::Painter, body: Rect, hover_row: Option<usize>, row_y: &dyn Fn(usize, usize) -> f32, p: &Palette, m: &Metrics) {
    let ctx = ui.ctx().clone();
    let open = egui::Popup::is_id_open(&ctx, row_menu_id());
    let pos = match (hover_row, tab.grid.row_menu) {
        (_, Some((_, pos, _))) if open => Some(pos),
        (Some(r), _) => Some(r),
        _ => None,
    };
    let Some(pos) = pos.filter(|&r| r >= tab.grid.top && r < tab.shown()) else { return };
    let gr_max = body.min.x + m.gutter_w - m.gutter_rule_w;
    let ry = row_y(pos, tab.grid.top);
    let br = Rect::from_center_size(pos2(gr_max - 4.0 - 9.0, ry + (m.row_h - 1.0) / 2.0), vec2(18.0, 22.0));
    if !body.contains(br.center()) { return; }
    let resp = ui.interact(br, ui.scope_id().with("row-btn"), Sense::click());
    let this_open = open && tab.grid.row_menu.is_some_and(|x| x.1 == pos);
    let hot = resp.hovered() || this_open;
    let pb = painter.with_clip_rect(body);
    if hot { pb.rect_filled(br, 4.0, p.surf_3); }
    icons::paint(&pb, Icon::Kebab, Rect::from_center_size(br.center(), vec2(4.0, 13.5)), if hot { p.txt_strong } else { p.txt_mut });
    if resp.clicked() {
        if this_open {
            egui::Popup::close_id(&ctx, row_menu_id());
        } else {
            tab.grid.row_menu = Some((tab.shown_id(pos), pos, br));
            egui::Popup::open_id(&ctx, row_menu_id());
        }
    }
}

/// The row's menu (its ⋮, or a right click on its number).
fn row_menu(ui: &mut Ui, tab: &mut Tab, st: &State, ncols: usize, p: &Palette, m: &Metrics, actions: &mut Vec<Action>) {
    use crate::widgets::{head, sep, width, Item};
    let Some((id, pos, anchor)) = tab.grid.row_menu else { return };
    let ctx = ui.ctx().clone();
    let popup = egui::Popup::new(row_menu_id(), ctx.clone(), anchor, ui.layer_id())
        .open_memory(None)
        .kind(egui::PopupKind::Menu)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .align(egui::RectAlign::RIGHT_START)
        .gap(4.0)
        .frame(crate::theme::menu_frame(p, m))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let shown = popup.show(|ui| {
        width(ui, 230.0);
        if pos >= tab.shown() || tab.shown_id(pos) != id { ui.close(); return; }
        let rec = (id as usize + 1).saturating_sub(st.header as usize);
        head(ui, p, &format!("Row {}", fmt_int(rec)));
        if Item::new("Insert a row above").icon(Icon::RowAbove).show(ui, p).clicked() { actions.push(Action::InsertRows { before: pos, count: 1 }); ui.close(); }
        if Item::new("Insert a row below").icon(Icon::RowBelow).show(ui, p).clicked() { actions.push(Action::InsertRows { before: pos + 1, count: 1 }); ui.close(); }
        if Item::new("Duplicate below").icon(Icon::Copy).show(ui, p).clicked() { duplicate_row(tab, pos, &ctx); ui.close(); }
        sep(ui, p);
        if Item::new("Row card — every field as a form").icon(Icon::Card).show(ui, p).clicked() {
            tab.grid.select(Cell { row: pos, col: tab.grid.cursor.map_or(0, |c| c.col) }, false);
            actions.push(Action::RowCard);
            ui.close();
        }
        if Item::new("Raw line — as it sits in the file").icon(Icon::FileText).show(ui, p).clicked() {
            tab.grid.select(Cell { row: pos, col: tab.grid.cursor.map_or(0, |c| c.col) }, false);
            actions.push(Action::RowCardRaw);
            ui.close();
        }
        if Item::new("Why does this row look wrong?").icon(Icon::Code).show(ui, p).clicked() { actions.push(Action::Diagnose(id)); ui.close(); }
        sep(ui, p);
        let many = tab.grid.range().filter(|(r, _)| r.contains(&pos) && r.end() > r.start()).map(|(r, _)| r);
        match many {
            Some(r) => {
                if Item::new(&format!("Delete the {} selected rows", fmt_int(r.end() - r.start() + 1))).icon(Icon::Trash).danger().show(ui, p).clicked() {
                    actions.push(Action::DeleteRows(r.map(|q| tab.shown_id(q)).collect()));
                    ui.close();
                }
            }
            None => {
                if Item::new("Delete the row").icon(Icon::Trash).danger().show(ui, p).clicked() { actions.push(Action::DeleteRows(vec![id])); ui.close(); }
            }
        }
        let _ = ncols;
    });
    if shown.is_none() { tab.grid.row_menu = None; }
}

/// A copy of the row at `pos` right under it (in the file and in the view), as one undo step.
pub fn duplicate_row(tab: &mut Tab, pos: usize, ctx: &egui::Context) {
    let id = tab.shown_id(pos);
    let Some(t) = &mut tab.table else { return };
    let st = t.st.clone();
    // Unfiltered and unsorted, the row shown at `pos` is the file's.
    let fpos = if tab.view.is_none() { Some(pos) } else {
        match &st.order {
            None => (id as usize).checked_sub(st.header as usize),
            Some(o) => o.iter().skip(st.header as usize).position(|&x| x == id),
        }
    };
    let Some(fpos) = fpos.filter(|&p| p < st.len()) else { return };
    let mut row = Vec::new();
    st.row(id, &mut row);
    let mut new_id = None;
    t.batch(|t| {
        let ids = t.insert_rows(fpos + 1, 1);
        if let Some(&n) = ids.first() {
            let ch: Vec<(RowId, usize, String)> = row.iter().enumerate().take(st.ncols).filter(|(_, v)| !v.is_empty()).map(|(c, v)| (n, c, v.clone())).collect();
            t.set_cells(&ch);
            new_id = Some(n);
        }
    });
    if let (Some(n), Some(v)) = (new_id, &mut tab.view) {
        *v = Arc::new(csvfab_engine::table::splice_ids(v, pos + 1, &[n]));
    }
    tab.after_edit(false, ctx);
    // Said, with Undo (as the web's "Row duplicated").
    tab.grid.notice = Some(format!("Row {} duplicated — not written yet, use Save.", fmt_int(fpos + 1)));
}

/// The group at the top of the view, floating 4 px under the header over the sort column.
#[allow(clippy::too_many_arguments)]
fn group_tag(tab: &Tab, st: &State, painter: &egui::Painter, body: Rect, vis: &[(usize, f32, f32)], gc: usize, frozen: usize, scroll_x0: f32, p: &Palette) {
    if tab.grid.scroll < tab.grid.rh() as f64 { return; }
    let Some(&(_, x, w)) = vis.iter().find(|v| v.0 == gc) else { return };
    let left = if gc < frozen { x } else { x.max(scroll_x0) } + 6.0;
    let right = x + w - 6.0;
    if right - left < 40.0 { return; }
    let top = (tab.grid.top + (tab.grid.off > 0.0) as usize).min(tab.shown().saturating_sub(1));
    let v = st.cell(tab.shown_id(top), gc);
    let v = v.trim();
    let text = if v.is_empty() { "(empty)".to_string() } else { display(v).into_owned() };
    let mut job = LayoutJob::simple_singleline(text, semibold(12.0), p.txt_strong);
    job.wrap = TextWrapping { max_width: right - left - 18.0 - 2.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = painter.layout_job(job);
    let r = Rect::from_min_size(pos2(left, body.min.y + 4.0), vec2(g.size().x + 18.0 + 2.0, 18.0 + 2.0 + 2.0));
    let pt = painter.with_clip_rect(body);
    pt.rect(r, 10.0, mix(p.accent, p.bg, 0.22), Stroke::new(1.0, alpha(p.accent, 0.45)), StrokeKind::Inside);
    let ppp = pt.ctx().pixels_per_point();
    pt.galley(pos2(r.min.x + 10.0, ((r.center().y - g.size().y / 2.0) * ppp).round() / ppp), g, p.txt_strong);
}

#[allow(clippy::too_many_arguments)]
fn editor(ui: &mut Ui, tab: &mut Tab, st: &State, painter: &egui::Painter, body: Rect, vis_cols: &[(usize, f32, f32)], end: usize, rows: usize, ncols: usize, view_w: f32, p: &Palette, m: &Metrics, resp: &egui::Response, actions: &mut Vec<Action>, off: f32) {
    let range = tab.grid.range();
    let Some(e) = &mut tab.grid.editing else { return };
    if e.at.row < tab.grid.top || e.at.row >= end { return; }
    let Some(&(_, x, w)) = vis_cols.iter().find(|v| v.0 == e.at.col) else { return };
    let y = body.min.y + (e.at.row - tab.grid.top) as f32 * m.row_h - off;
    // The box grows with its text: rightwards as far as the grid's edge, and a row's height
    // per line (Shift+Enter) down to the grid's foot, as the web's textarea.
    let tw = painter.layout_no_wrap(e.text.clone(), mono(m), p.txt_strong).size().x + 2.0 * m.cell_pad_x + 12.0;
    let lines = e.text.split('\n').count();
    let line_h = ui.fonts_mut(|f| f.row_height(&mono(m)));
    let h = (m.row_h + (lines - 1) as f32 * line_h).min((body.max.y - y).max(m.row_h));
    let r = Rect::from_min_size(pos2(x - 1.0, y - 1.0), vec2(tw.min(body.max.x - x).max(w) + 2.0, h + 1.0));

    let te_id = Id::unique("cell-editor");
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layer_id(egui::LayerId::new(egui::Order::Foreground, Id::unique("cell-editor-layer"))));
    // Shadow, box and text on one layer, in that order: on separate layers egui may
    // stack the box over the text.
    {
        let fg = child.painter().clone().with_clip_rect(ui.ctx().content_rect());
        fg.rect_filled(r.translate(vec2(0.0, 6.0)).expand(4.0), 8.0, Color32::from_black_alpha(60));
        fg.rect(r, 3.0, p.bg, Stroke::new(2.0, p.prim), StrokeKind::Inside);
    }
    // The text exactly where the cell drew it: the cell's padding, centred on the row.
    // Several lines: from the top, the first on its row as the cell drew it.
    let layout = if lines > 1 { egui::Layout::top_down(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Center) };
    let mut inner = child.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(r.min.x + 1.0 + m.cell_pad_x, r.min.y), pos2(r.max.x - m.cell_pad_x, r.max.y - 1.0))).layout(layout));
    inner.spacing_mut().interact_size.y = 0.0;
    if lines > 1 { inner.add_space(((m.row_h - line_h) / 2.0).max(0.0)); }
    // Enter writes the value; Shift+Enter is a line break in it.
    // A text file: Tab types a tab (code is indented with it), as the web's editor.
    let raw = st.base.dialect.lines;
    let out = egui::TextEdit::multiline(&mut e.text)
        .id(te_id)
        .lock_focus(raw)
        .return_key(egui::KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
        .desired_rows(lines)
        .font(mono(m))
        .text_color(p.txt_strong)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .desired_width(r.width() - 2.0 * m.cell_pad_x - 2.0)
        .show(&mut inner);
    if !out.response.has_focus() && !out.response.lost_focus() {
        out.response.request_focus();
        if !e.fresh {
            let mut s = out.state;
            let end = egui::text::CCursor::new(e.text.chars().count());
            let range = if e.select { egui::text::CCursorRange::two(egui::text::CCursor::new(0), end) } else { egui::text::CCursorRange::one(end) };
            s.cursor.set_char_range(Some(range));
            s.store(ui.ctx(), te_id);
        }
        e.fresh = false;
        e.select = false;
    }
    let (enter, tab_key, esc, shift, cmd) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Tab) && !raw, i.key_pressed(Key::Escape), i.modifiers.shift, i.modifiers.command));
    // Shift+Enter went into the text as a line break.
    let enter = enter && !shift;
    // Enter on a block: the value in every selected cell; Ctrl+Enter: a series from it.
    let block = range.filter(|(r, cs)| r.contains(&e.at.row) && cs.contains(&e.at.col) && (r.start() != r.end() || cs.start() != cs.end()));
    if esc {
        tab.grid.editing = None;
        resp.request_focus();
    } else if enter && !shift && block.is_some() {
        let (r, cs) = block.unwrap();
        let value = std::mem::take(&mut e.text);
        tab.grid.editing = None;
        if cmd {
            let sel = series::Rect { r0: *r.start(), r1: *r.end(), c0: *cs.start(), c1: *cs.end() };
            let (st2, view, vis) = (st.clone(), tab.view.clone(), (0..ncols).filter(|c| !tab.grid.hidden.contains(c)).collect::<Vec<_>>());
            tab.grid.fill_job = Some(Job::spawn(ui.ctx(), move |cancel, _| series::fill_value(&st2, &ids_of(&st2, &view), &vis, sel, &value, cmd, cancel)));
        } else if let Some(a) = fill_selection(tab, value, false, &ui.ctx().clone()) {
            // The same value everywhere: one text for the whole selection.
            if matches!(&a, Action::Patch { patch, .. } if !patch.is_empty()) { actions.push(a); }
        }
        resp.request_focus();
    } else if out.response.lost_focus() || enter || tab_key {
        // Enter or Tab: written, the cell stays the active one (as the web's editor).
        let at = e.at;
        commit_edit(tab, st, actions);
        if enter || tab_key {
            tab.grid.select(at, false);
            tab.grid.reveal(rows, view_w);
            resp.request_focus();
        }
    }
}

/// The bands' fills over `clip`: titles `surf`, then the charts' gradient (one block with the
/// titles), filters `filter_bg`.
fn band_fills(pt: &egui::Painter, band: &Band, p: &Palette) {
    pt.rect_filled(band.head, 0.0, p.surf);
    if let Some(r) = band.charts {
        // Vertical gradient surf → spk_bottom.
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(r.left_top(), p.surf);
        mesh.colored_vertex(r.right_top(), p.surf);
        mesh.colored_vertex(r.right_bottom(), p.spk_bottom);
        mesh.colored_vertex(r.left_bottom(), p.spk_bottom);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        pt.add(egui::Shape::mesh(mesh));
    }
    if let Some(r) = band.filt { pt.rect_filled(r, 0.0, p.filter_bg); }
}

#[allow(clippy::too_many_arguments)]
fn header(ui: &mut Ui, tab: &mut Tab, st: &State, painter: &egui::Painter, band: &Band, vis_scroll: &[(usize, f32, f32)], vis_frz: &[(usize, f32, f32)], cells_x0: f32, scroll_x0: f32, full: Rect, p: &Palette, m: &Metrics, actions: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let bottom = band.filt.or(band.charts).unwrap_or(band.head).max.y;
    let all = Rect::from_min_max(band.head.min, pos2(band.head.max.x, bottom));
    let mut new_sort = None;
    let mut panel_seen = false;
    let mut drop: Option<(usize, bool)> = None;
    let heads = titles(st, tab);
    for (list, clip, frz) in [(vis_scroll, Rect::from_min_max(pos2(scroll_x0, all.min.y), all.max), false), (vis_frz, Rect::from_min_max(pos2(cells_x0, all.min.y), pos2(scroll_x0, all.max.y)), true)] {
        if list.is_empty() { continue; }
        band_fills(&painter.with_clip_rect(clip), band, p);
        for (k, &(c, x, w)) in list.iter().enumerate() {
            let last_frozen = frz && k + 1 == list.len();
            header_col(ui, tab, st, &heads, painter, band, all, clip, c, x, w, last_frozen, p, m, actions, &mut new_sort, &mut panel_seen, &mut drop);
        }
    }
    // A title dragged: the drop marker on the title under the pointer; moved on release.
    if let Some(from) = tab.grid.col_drag {
        let q = ui.input(|i| i.pointer.interact_pos());
        let target = q.and_then(|q| vis_scroll.iter().chain(vis_frz.iter()).find(|v| q.x >= v.1 && q.x < v.1 + v.2 && q.x >= cells_x0).map(|v| (v.0, v.1, v.2, q.x < v.1 + v.2 / 2.0)));
        let ph = painter.with_clip_rect(Rect::from_min_max(pos2(cells_x0, all.min.y), all.max));
        if let Some(&(_, x, w)) = vis_scroll.iter().chain(vis_frz.iter()).find(|v| v.0 == from) {
            ph.rect_filled(Rect::from_min_size(pos2(x, band.head.min.y), vec2(w - 1.0, m.header_h - 1.0)), 0.0, p.surf.gamma_multiply(0.55));
        }
        if let Some((tc, x, w, before)) = target {
            let bx = if before { x } else { x + w - 4.0 };
            ph.rect_filled(Rect::from_min_size(pos2(bx, band.head.min.y), vec2(3.0, m.header_h - 1.0)), 0.0, p.prim);
            if tc != from { drop = Some((tc, before)); }
        }
        if !ui.input(|i| i.pointer.primary_down()) {
            tab.grid.col_drag = None;
            if let Some((tc, before)) = drop {
                let mut to = if before { tc } else { tc + 1 };
                if to > from { to -= 1; }
                move_column(tab, from, to, &ctx);
            }
        } else {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            // Near the left or right edge the columns scroll under it, faster the closer (and
            // past it): far titles are reached by dragging (the frame clamps `xoff`).
            if let Some(q) = q {
                const ZONE: f32 = 32.0;
                let depth = if q.x > all.max.x - ZONE { ((q.x - (all.max.x - ZONE)) / ZONE).min(1.5) } else if q.x < scroll_x0 + ZONE { -((scroll_x0 + ZONE - q.x) / ZONE).min(1.5) } else { 0.0 };
                if depth != 0.0 {
                    let dt = ui.input(|i| i.stable_dt).min(0.05);
                    tab.grid.xoff = (tab.grid.xoff + depth * 900.0 * dt).max(0.0);
                    ctx.request_repaint();
                }
            }
        }
    }
    if let Some(s) = new_sort { actions.push(Action::Sort(s)); }
    if !panel_seen { tab.grid.panel = None; }

    // Gutter cells of the bands: icons that toggle them.
    let gx = full.min.x;
    let gw = m.gutter_w - m.gutter_rule_w;
    painter.rect_filled(Rect::from_min_max(pos2(gx, all.min.y), pos2(gx + gw, all.max.y)), 0.0, p.surf_2);
    // The columns manager: show, hide, reorder the columns.
    let sr = Rect::from_min_max(pos2(gx, band.head.min.y), pos2(gx + gw, band.head.max.y));
    let sresp = ui.interact(sr, ui.scope_id().with("g-columns"), Sense::click());
    let pid = egui::Popup::default_response_id(&sresp);
    if crate::shot::take("columns").is_some() || std::mem::take(&mut tab.grid.open_manager) { egui::Popup::open_id(&ctx, pid); }
    let open = egui::Popup::is_id_open(&ctx, pid);
    let hot = sresp.hovered() || open;
    if hot { painter.rect_filled(Rect::from_center_size(sr.center(), vec2(28.0, 26.0)), 5.0, p.surf_3); }
    icons::sliders(painter, Rect::from_center_size(pos2(gx + gw / 2.0, band.head.center().y), vec2(16.0, 16.0)), if hot { p.txt_hi } else { p.txt });
    let nh = tab.grid.hidden.len();
    if nh > 0 {
        // Columns hidden: their count in a small prim badge by the icon.
        let g = painter.layout_no_wrap(nh.to_string(), mono_bold(9.0), p.on_prim);
        let br = Rect::from_min_size(pos2(sr.center().x + 6.0, sr.center().y - 13.0), vec2((g.size().x + 8.0).max(14.0), 14.0));
        painter.rect_filled(br, 7.0, p.prim);
        painter.galley(pos2(((br.center().x - g.size().x / 2.0) * 2.0).round() / 2.0, ((br.center().y - g.size().y / 2.0) * 2.0).round() / 2.0), g, p.on_prim);
    }
    if sresp.hovered() && !open {
        tip(&ctx, p, m, sr, &if nh > 0 { format!("Columns: show, hide, reorder — {nh} hidden") } else { "Columns: show, hide, reorder".to_string() });
    }
    let shown = egui::Popup::menu(&sresp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(4.0)
        .frame(crate::theme::menu_frame(p, m))
        .show(|ui| colpanel::manager(ui, tab, p, &ctx));
    if shown.is_none() { tab.grid.manager = None; }
    if let Some(r) = band.charts {
        let ir = Rect::from_min_max(pos2(gx, r.min.y), pos2(gx + gw, r.max.y));
        let b = ui.interact(ir, ui.scope_id().with("g-charts"), Sense::click());
        icons::bars(painter, Rect::from_center_size(ir.center(), vec2(15.0, 15.0)), if b.hovered() { p.txt_mut } else { p.txt_faint });
        if b.hovered() { tip(&ctx, p, m, ir, &crate::spark_ui::gutter_tip(tab)); }
        if b.clicked() { actions.push(Action::ToggleCharts); }
    }
    if let Some(r) = band.filt {
        let ir = Rect::from_min_max(pos2(gx, r.min.y), pos2(gx + gw, r.max.y));
        let b = ui.interact(ir, ui.scope_id().with("g-filters"), Sense::click());
        icons::funnel(painter, Rect::from_center_size(ir.center(), vec2(13.0, 13.0)), true, if b.hovered() { p.txt_mut } else { p.txt_faint });
        if b.hovered() { tip(&ctx, p, m, ir, "Hide the filter row (its filters are cleared)"); }
        if b.clicked() { actions.push(Action::ToggleFilters); }
    }
    // Titles and charts are one block: no rule between them.
    for yy in [Some(band.head.max.y).filter(|_| band.charts.is_none()), band.charts.map(|r| r.max.y), band.filt.map(|r| r.max.y)].into_iter().flatten() {
        painter.hline(full.min.x..=all.max.x, yy - 0.5, Stroke::new(1.0, p.brd));
    }
}

/// A column's kind as its badge shows it: glyph (None: the calendar), tint, text colour,
/// name; a sorted column in `prim`.
pub(crate) fn kind_style(tab: &Tab, c: usize, p: &Palette) -> (Option<&'static str>, Color32, Color32, &'static str) {
    let kind = tab.kinds.get(c).copied().unwrap_or(Kind::Text);
    let rich = tab.rich.get(c).and_then(|r| r.as_ref());
    let (glyph, base, col, name) = match (kind, rich) {
        (Kind::Text, Some(r)) => match r {
            Rich::Url => (Some("↗"), p.accent, p.accent_2, "links"),
            Rich::Email => (Some("@"), p.accent, p.accent_2, "e-mails"),
            Rich::Bool { .. } => (Some("✓"), p.accent, p.accent_2, "yes / no"),
            Rich::Color => (Some("◐"), p.accent, p.accent_2, "colours"),
            Rich::Json => (Some("{}"), p.accent, p.accent_2, "JSON"),
        },
        (Kind::Number, _) => (Some("#"), p.cyan, p.cyan_txt, "numbers"),
        (Kind::Date, _) => (None, p.violet, p.violet_txt, "dates"),
        (Kind::Text, None) => (Some("Aa"), p.gray, p.txt_dim, "text"),
    };
    if tab.sort.iter().any(|k| k.col == c) { (glyph, p.prim, p.prim, name) } else { (glyph, base, col, name) }
}

/// One column of the header: its title (type icon, title, sort arrow, number, ▾, resize
/// handle) and its filter box; the charts are spark_ui's.
#[allow(clippy::too_many_arguments)]
fn header_col(ui: &mut Ui, tab: &mut Tab, st: &State, heads: &[String], painter: &egui::Painter, band: &Band, all: Rect, clip: Rect, c: usize, x: f32, w: f32, last_frozen: bool, p: &Palette, m: &Metrics, actions: &mut Vec<Action>, new_sort: &mut Option<Vec<SortKey>>, panel_seen: &mut bool, _drop: &mut Option<(usize, bool)>) {
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let ph = painter.with_clip_rect(clip);
    let title_font = semibold(m.font_header);
    let bold_mono = mono_bold(10.0);
    let hr = Rect::from_min_size(pos2(x, band.head.min.y), vec2(w, m.header_h));
    let hclip = ph.with_clip_rect(hr.intersect(clip));
    let hresp = ui.interact(hr.intersect(clip), ui.scope_id().with(("head", c)), Sense::click_and_drag());
    if tab.grid.focus_col == Some(c) {
        let top = band.charts.unwrap_or(band.head).max.y;
        ph.rect_filled(Rect::from_min_max(hr.min, pos2(hr.max.x - 1.0, top - 1.0)), 0.0, alpha(p.accent, 0.07));
    }
    let cy = hr.center().y - 0.5;
    let sort_pos = tab.sort.iter().position(|k| k.col == c);
    let title = heads.get(c).cloned().unwrap_or_default();
    let tcolor = if hresp.hovered() && tab.grid.col_drag.is_none() { p.txt_strong } else if st.header { p.txt_hi } else { p.txt_dim };
    // The badge: the column's kind on a quiet tint (stronger while the whole column is
    // selected). A click on it selects the whole column.
    let (glyph, base, col, kind_name) = kind_style(tab, c, p);
    let gg = glyph.map(|g| hclip.layout_no_wrap(g.into(), bold_mono.clone(), col));
    let glyph_w = gg.as_ref().map_or(11.0, |g| g.size().x);
    const BH: f32 = 18.0; // height (radius half of it)
    let bw = (glyph_w + 14.0).max(BH).round();
    let x0 = snap(x + m.cell_pad_x);
    let top = snap(cy - BH / 2.0);
    let br = Rect::from_min_size(pos2(x0, top), vec2(bw, BH));
    let bresp = ui.interact(br.intersect(clip), ui.scope_id().with(("badge", c)), Sense::click());
    let on = tab.grid.cursor.is_some_and(|k| k.col == c) && tab.grid.range().is_some_and(|(r, _)| *r.start() == 0 && r.end() + 1 >= tab.shown());
    let k = if bresp.hovered() { 1.4 } else { 1.0 };
    let bf = mix(base, p.surf, if on { 0.30 } else { 0.09 } * k);
    hclip.rect_filled(br, BH / 2.0, bf);
    let gc = mix(col, bf, 0.7);
    let ymid = top + BH / 2.0;
    let gx = x0 + (bw - glyph_w) / 2.0;
    match gg {
        Some(g) => hclip.galley_with_override_text_color(pos2(snap(gx), snap(ymid - g.size().y / 2.0)), g, gc),
        None => icons::calendar(&hclip, Rect::from_min_size(pos2(snap(gx), snap(ymid - 5.5)), vec2(11.0, 11.0)), 1.8, gc),
    }
    if bresp.clicked() && tab.shown() > 0 {
        let shift = ui.input(|i| i.modifiers.shift);
        let from = if shift { tab.grid.anchor.map_or(c, |a| a.col) } else { c };
        tab.grid.anchor = Some(Cell { row: 0, col: from });
        tab.grid.cursor = Some(Cell { row: tab.shown() - 1, col: c });
    }
    let on_badge = bresp.hovered();
    if on_badge { tip(&ctx, p, m, br, &format!("Column {c} · {title} · {kind_name}")); }
    // Not sorted on, the title hovered (not its badge, ▾ or edge): the arrow a click would
    // bring, in grey, where the sort arrow goes (its room taken from the title as for one).
    let renaming = tab.grid.renaming.as_ref().is_some_and(|r| r.0 == c);
    let ghost = sort_pos.is_none() && !on_badge && !renaming && tab.grid.col_drag.is_none()
        && hresp.hover_pos().is_some_and(|q| q.x < x + w - 7.0 - m.col_menu - 2.0 && q.x < x + w - m.resize_w);
    let arrow_w = if sort_pos.is_some() || ghost { 17.0 + if sort_pos.is_some() && tab.sort.len() > 1 { 7.0 } else { 0.0 } } else { 0.0 };
    // Title, its number small and faint just after it, sort arrow; the ▾ button at the right.
    let tx = br.max.x + 7.0;
    let room = (x + w - m.col_title_pad_r - tx).max(0.0);
    let full_title_w = ctx.fonts_mut(|f| f.layout_no_wrap(title.clone(), title_font.clone(), tcolor).size().x);
    let num_g = hclip.layout_no_wrap(c.to_string(), semibold(9.5), p.txt_faint);
    let num_w = num_g.size().x + 5.0;
    // Crowded: the title ellipsizes first (down to a few letters), then the number goes.
    let with_num = room - arrow_w - num_w >= full_title_w.min(28.0);
    let mut tf = TextFormat { font_id: title_font.clone(), color: tcolor, extra_letter_spacing: 0.125, ..Default::default() };
    tf.italics = !st.header;
    let mut job = LayoutJob::default();
    if crate::bidi::has_rtl(&title) { crate::bidi::append(&mut job, &title, &tf); } else { job.append(&title, 0.0, tf); }
    job.wrap = TextWrapping { max_width: (room - arrow_w - if with_num { num_w } else { 0.0 }).max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = hclip.layout_job(job);
    let mut ex = tx + g.size().x;
    let title_h = g.size().y;
    hclip.galley(pos2(snap(tx), snap(cy - title_h / 2.0)), g, tcolor);
    if with_num {
        // On the title's baseline.
        let ny = cy + title_h / 2.0 - num_g.size().y - 0.5;
        hclip.galley(pos2(snap(ex + 5.0), snap(ny)), num_g, p.txt_faint);
        ex += num_w;
    }
    if ghost {
        icons::arrow(&hclip, Rect::from_min_size(pos2(snap(ex + 5.0), snap(cy - 6.0)), vec2(12.0, 12.0)), true, 2.2, p.txt_faint);
    }
    if let Some(i) = sort_pos {
        let ar = Rect::from_min_size(pos2(snap(ex + 5.0), snap(cy - 6.0)), vec2(12.0, 12.0));
        icons::arrow(&hclip, ar, !tab.sort[i].desc, 2.2, p.prim);
        if tab.sort.len() > 1 {
            hclip.text(pos2(ar.max.x + 1.0, snap(cy - 8.0)), Align2::LEFT_TOP, format!("{}", i + 1), semibold(9.0), p.prim);
        }
    }
    // ▾: the column panel.
    let mr = Rect::from_center_size(pos2(snap(x + w - 7.0 - m.col_menu / 2.0), snap(cy)), vec2(m.col_menu, m.col_menu));
    let mresp = ui.interact(mr.intersect(clip), ui.scope_id().with(("colmenu", c)), Sense::click());
    if crate::shot::take(&format!("col:{c}")).is_some() || tab.grid.open_panel.take_if(|k| *k == c).is_some() { egui::Popup::open_id(&ctx, egui::Popup::default_response_id(&mresp)); }
    let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&mresp));
    if mresp.hovered() || open { hclip.rect_filled(mr, 4.0, p.surf_3); }
    let valued = tab.filter.hidden.iter().any(|(hc, h)| *hc == c && !h.is_empty());
    if valued {
        icons::funnel(&hclip, Rect::from_center_size(mr.center(), vec2(13.0, 13.0)), true, p.prim);
    } else {
        icons::chevron_down(&hclip, Rect::from_center_size(mr.center(), vec2(13.0, 13.0)), 2.0, if mresp.hovered() || open { p.txt_hi } else { p.txt_dim });
    }
    let shown = egui::Popup::menu(&mresp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(4.0)
        .frame(crate::theme::menu_frame(p, m))
        .show(|ui| colpanel::show(ui, tab, c, p, m, actions));
    if shown.is_some() && tab.grid.panel.as_ref().is_some_and(|pn| pn.col == c) { *panel_seen = true; }
    egui::Popup::context_menu(&hresp).frame(crate::theme::menu_frame(p, m)).show(|ui| colpanel::title_menu(ui, tab, c, p, actions));
    let on_menu = |q: Option<Pos2>| q.is_some_and(|q| mr.contains(q));
    // Renamed in place: a field over the title; Enter or a click away writes it, Escape drops it.
    if tab.grid.renaming.as_ref().is_some_and(|r| r.0 == c) {
        let er = Rect::from_min_max(pos2(tx - 4.0, cy - 12.0), pos2((x + w - m.col_title_pad_r).max(tx + 60.0), cy + 12.0));
        let fg = ui.painter().clone().with_clip_rect(clip);
        fg.rect(er, 4.0, p.bg_deep, Stroke::new(1.5, p.prim), StrokeKind::Inside);
        let id = ui.scope_id().with(("rename", c));
        let (_, text, started) = tab.grid.renaming.as_mut().unwrap();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(er.shrink2(vec2(5.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let out = egui::TextEdit::singleline(text).id(id).font(title_font.clone()).text_color(p.txt_strong).frame(egui::Frame::NONE).desired_width(er.width() - 10.0).show(&mut child);
        if !*started {
            *started = true;
            out.response.request_focus();
            let mut s = out.state;
            s.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()))));
            s.store(&ctx, id);
        } else if ui.input(|i| i.key_pressed(Key::Escape)) {
            tab.grid.renaming = None;
        } else if out.response.lost_focus() {
            let (_, text, _) = tab.grid.renaming.take().unwrap();
            let text = text.trim().to_string();
            if !text.is_empty() && text != title { actions.push(Action::RenameTo(c, text)); }
        }
    }
    if hresp.double_clicked() && !on_menu(hresp.interact_pointer_pos()) {
        // Rename, as in the web; the sort the first click changed goes back.
        if let Some((_, s)) = tab.grid.pre_click_sort.take().filter(|x| x.0 == c) { *new_sort = Some(s); }
        // In the title itself (as the web's); a file without titles asks first (one is added).
        if st.header { tab.grid.renaming = Some((c, title.clone(), false)); } else { actions.push(Action::RenameColumn(c)); }
    } else if hresp.clicked() && !on_menu(hresp.interact_pointer_pos()) {
        tab.grid.focus_col = Some(c);
        tab.grid.pre_click_sort = Some((c, tab.sort.clone()));
        let shift = ui.input(|i| i.modifiers.shift);
        let mut s = tab.sort.clone();
        match sort_pos {
            Some(i) if !s[i].desc => s[i].desc = true,
            Some(i) => { s.remove(i); }
            None => {
                if !shift { s.clear(); }
                s.push(SortKey { col: c, desc: false });
            }
        }
        if !shift && sort_pos.is_some() { s.retain(|k| k.col == c); }
        *new_sort = Some(s);
    }
    if hresp.drag_started() && !on_menu(hresp.interact_pointer_pos()) && tab.grid.resizing.is_none() {
        tab.grid.col_drag = Some(c);
    }
    // A title cut short, in full (the clicks are in F1's list, not on every title).
    let cut = full_title_w > room - arrow_w - if with_num { num_w } else { 0.0 };
    if cut && hresp.hovered() && tab.grid.col_drag.is_none() && !open && !on_badge {
        if hresp.hover_pos().is_some_and(|q| q.x < mr.min.x - 2.0 && q.x < x + w - m.resize_w) {
            tip(&ctx, p, m, hr, &title);
        }
    }
    ph.vline(x + w - 0.5, all.min.y..=all.max.y, Stroke::new(1.0, if last_frozen { p.frz_rule } else { p.brd }));
    // Resize handle: the last 5 px of the title cell.
    let handle = Rect::from_min_max(pos2(x + w - m.resize_w, band.head.min.y), pos2(x + w, band.head.max.y));
    let rresp = ui.interact(handle.intersect(clip), ui.scope_id().with(("resize", c)), Sense::click_and_drag());
    if rresp.hovered() || rresp.dragged() {
        ctx.set_cursor_icon(CursorIcon::ResizeColumn);
        ph.rect_filled(handle, 0.0, p.prim);
    }
    if rresp.drag_started() { tab.grid.resizing = Some((c, w, rresp.interact_pointer_pos().map_or(0.0, |q| q.x))); tab.grid.col_drag = None; tab.grid.width_to = None; }
    if let (Some((rc, w0, x0)), Some(q)) = (tab.grid.resizing, rresp.interact_pointer_pos()) {
        if rresp.dragged() && rc == c { tab.grid.widths[c] = (w0 + q.x - x0).max(36.0); }
    }
    if rresp.drag_stopped() { tab.grid.resizing = None; }
    if rresp.double_clicked() { tab.grid.width_to = Some((c, tab.grid.widths[c], fit_width(&ctx, st, tab, c, m), Instant::now())); }

    // Filter box.
    if let Some(fb) = band.filt {
        let fr = Rect::from_min_size(pos2(x + m.filter_cell_pad_x, fb.min.y + ((m.filter_h - 1.0 - m.filter_box_h) / 2.0).round()), vec2(w - 2.0 * m.filter_cell_pad_x - 1.0, m.filter_box_h));
        let fclip_r = Rect::from_min_max(pos2(clip.min.x, fb.min.y), pos2(clip.max.x, fb.max.y));
        if fr.max.x > clip.min.x + 20.0 && c < tab.col_filters.len() {
            let fid = Id::unique(("col-filter", c));
            let has = !tab.col_filters[c].is_empty();
            let focused = ctx.memory(|mem| mem.has_focus(fid));
            let hovered = ui.rect_contains_pointer(fr.intersect(fclip_r));
            let bad = tab.filter_error_col == Some(c) && tab.filter_error.is_some();
            let (fill, border) = if bad { (if focused { p.bg } else { mix(p.danger, p.bg, 0.08) }, p.danger) } else if focused { (p.bg, p.prim) } else if has { (mix(p.prim, p.bg, 0.1), alpha(p.prim, 0.55)) } else { (p.bg, if hovered { p.surf_3 } else { Color32::TRANSPARENT }) };
            let fclip = painter.with_clip_rect(fclip_r);
            if focused { fclip.rect_stroke(fr, 6.0, Stroke::new(3.0, p.focus_ring_sm), StrokeKind::Outside); }
            fclip.rect(fr, 5.0, fill, Stroke::new(1.0, border), StrokeKind::Inside);
            icons::magnifier(&fclip, Rect::from_center_size(pos2(fr.min.x + 7.0 + 5.5, fr.center().y), vec2(11.0, 11.0)), 2.5, Color32::from_rgb(0x6e, 0x76, 0x81));
            let inner = Rect::from_min_max(pos2(fr.min.x + 24.0, fr.min.y), pos2(fr.max.x - 8.0, fr.max.y)).intersect(fclip_r);
            if inner.width() > 4.0 {
                let te = egui::TextEdit::singleline(&mut tab.col_filters[c])
                    .id(fid)
                    .hint_text(egui::RichText::new("Filter").font(FontId::proportional(12.0)).color(p.txt_faint))
                    .font(FontId::proportional(12.0))
                    .text_color(p.txt_hi)
                    .frame(egui::Frame::NONE);
                let r = crate::widgets::centered_edit(ui, inner, te);
                let r = if bad { r.on_hover_text(tab.filter_error.clone().unwrap_or_default()) } else { r };
                if tab.grid.focus_filter == Some(c) { r.request_focus(); tab.grid.focus_filter = None; }
                if r.changed() { actions.push(Action::FiltersChanged); }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn strips(ui: &mut Ui, tab: &mut Tab, st: &State, painter: &egui::Painter, full: Rect, rows: usize, page: usize, total_w: f32, view_w: f32, cells_x0: f32, frz_w: f32, p: &Palette, m: &Metrics) {
    let ctx = ui.ctx().clone();
    let sw = m.strip_w;
    let _ = page;
    // Vertical: from the header's top to the horizontal strip.
    let vtrack = Rect::from_min_max(pos2(full.max.x - sw, full.min.y), pos2(full.max.x, full.max.y - sw));
    painter.rect_filled(vtrack, 0.0, p.surf_2);
    painter.vline(vtrack.min.x + 0.5, vtrack.y_range(), Stroke::new(1.0, p.brd));
    let r = ui.interact(vtrack, ui.scope_id().with("vscroll"), Sense::click_and_drag());
    let hot = r.hovered() || r.dragged();
    let track = vtrack.shrink2(vec2(0.0, 2.0));
    let body_h = tab.grid.body_h;
    let th = if rows == 0 { track.height() } else { (track.height() * (body_h / (rows as f32 * m.row_h)).min(1.0)).max(22.0).min(track.height()) };
    let g = look::StripGeom::new(track.height() as f64, 0.0, m.row_h as f64, rows, 0.0);
    let ppp = ctx.pixels_per_point();
    if rows > 0 {
        let label_font = semibold(9.0);
        let sort = tab.sort.first().copied().filter(|k| k.col < st.ncols);
        let labels: Vec<(f64, String)> = match (sort, &tab.view) {
            (Some(k), Some(v)) if v.len() == rows => {
                let key = (k.col, Arc::as_ptr(&st.base) as usize);
                let kind = match tab.grid.sort_kind {
                    Some((kk, kind)) if kk == key => kind,
                    _ => {
                        let kind = look::sort_kind(st, k.col);
                        tab.grid.sort_kind = Some((key, kind));
                        kind
                    }
                };
                look::strip_labels(st, v, k.col, kind, &g.label_rows(rows))
            }
            _ => g.ticks(rows),
        };
        let lc = if sort.is_some() { p.txt_mut } else { p.txt_dim };
        for (y, label) in labels {
            let gl = painter.layout_no_wrap(label, label_font.clone(), lc);
            let cy = track.min.y + y as f32;
            painter.galley(pos2(((vtrack.center().x + 0.5 - gl.size().x / 2.0) * ppp).round() / ppp, ((cy - gl.size().y / 2.0) * ppp).round() / ppp), gl, lc);
        }
        strip_marks(tab, st, painter, &ctx, vtrack, track, &g, rows, p);
        // The selection's band.
        if let Some((rr, _)) = tab.grid.range() {
            let a = track.min.y + track.height() * *rr.start() as f32 / rows as f32;
            let b = track.min.y + track.height() * (*rr.end() + 1) as f32 / rows as f32;
            painter.rect_filled(Rect::from_min_max(pos2(vtrack.min.x + 1.0, a), pos2(vtrack.min.x + 4.0, b.max(a + 2.0))), 0.0, p.prim);
        }
    }
    if tab.grid.max_scroll(rows) > 0.0 {
        let maxs = tab.grid.max_scroll(rows).max(1.0);
        let ty = track.min.y + (track.height() - th) * (tab.grid.scroll / maxs) as f32;
        let thumb = Rect::from_min_size(pos2(vtrack.min.x + 2.0, ty), vec2(sw - 4.0, th));
        if r.drag_started() {
            if let Some(q) = r.interact_pointer_pos() {
                tab.grid.thumb_grab = Some(if thumb.contains(q) { q.y - thumb.min.y } else { th / 2.0 });
            }
        }
        if let (Some(gb), Some(q)) = (tab.grid.thumb_grab, r.interact_pointer_pos()) {
            if r.dragged() || r.drag_started() {
                let f = ((q.y - gb - track.min.y) / (track.height() - th)).clamp(0.0, 1.0);
                tab.grid.scroll = f as f64 * maxs;
            }
        }
        if r.clicked() {
            if let Some(q) = r.interact_pointer_pos() {
                let f = ((q.y - th / 2.0 - track.min.y) / (track.height() - th)).clamp(0.0, 1.0);
                tab.grid.scroll = f as f64 * maxs;
            }
        }
        if r.drag_stopped() { tab.grid.thumb_grab = None; }
        if r.dragged() { ctx.set_cursor_icon(CursorIcon::Grabbing); }
        painter.rect(thumb, 5.0, if hot { p.thumb_hot } else { p.thumb }, Stroke::new(1.0, if hot { p.thumb_hot_brd } else { p.thumb_brd }), StrokeKind::Inside);
        // The row there (and the sort value), beside the strip, while hovering or dragging.
        if let Some(q) = r.hover_pos().or(r.interact_pointer_pos()) {
            let pos = if r.dragged() { tab.grid.top } else { ((((q.y - track.min.y) / track.height()).clamp(0.0, 1.0)) as f64 * rows as f64) as usize };
            let pos = pos.min(rows.saturating_sub(1));
            // Its number in the file, as the gutter shows it (a row added here has none).
            let id = (pos < tab.shown()).then(|| tab.shown_id(pos));
            let mut text = match id {
                Some(id) if (id as usize) < st.base.n => format!("Row {}", fmt_int((id as usize + 1).saturating_sub(st.header as usize))),
                Some(_) => "A new row".to_string(),
                None => format!("Row {}", fmt_int(pos + 1)),
            };
            if let Some(k) = tab.sort.first().filter(|k| k.col < st.ncols && rows > 0) {
                let v = st.cell(tab.shown_id(pos), k.col);
                let v: String = v.trim().chars().take(40).collect();
                if !v.is_empty() { text = format!("{text} · {v}"); }
            }
            let gl = painter.layout_no_wrap(text, FontId::proportional(12.0), p.txt_hi);
            let tr = Rect::from_min_size(pos2(vtrack.min.x - 8.0 - gl.size().x - 18.0, q.y - gl.size().y / 2.0 - 4.0), gl.size() + vec2(18.0, 8.0));
            let tr = Rect::from_min_size(((tr.min.to_vec2() * ppp).round() / ppp).to_pos2(), tr.size());
            let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("strip-tip")));
            fg.add(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(102) }.as_shape(tr, 6.0));
            fg.rect(tr, 6.0, p.surf, Stroke::new(1.0, p.brd_2), StrokeKind::Inside);
            fg.galley(tr.min + vec2(9.0, 4.0), gl, p.txt_hi);
        }
    }

    // Horizontal: every column's share of the width (1 px separators), a few of them labelled
    // — the wide ones, the sorted ones and the cursor's, the others evenly spaced — so the
    // names stay readable however many columns there are.
    let htrack = Rect::from_min_max(pos2(full.min.x, full.max.y - sw), pos2(full.max.x - sw, full.max.y));
    painter.rect_filled(Rect::from_min_max(htrack.min, full.max), 0.0, p.surf_2);
    painter.hline(full.x_range(), htrack.min.y + 0.5, Stroke::new(1.0, p.brd));
    let lane = Rect::from_min_max(pos2(cells_x0, htrack.min.y), htrack.max);
    let laid = tab.grid.laid();
    let all_w: f32 = laid.iter().sum();
    let heads = titles(st, tab);
    let scale = if all_w > 0.0 { lane.width() / all_w } else { 0.0 };
    // (column, x, width) of each column shown, on the strip.
    let mut slots: Vec<(usize, f32, f32)> = Vec::new();
    {
        let mut x = lane.min.x;
        for (c, w) in laid.iter().enumerate() {
            if *w > 0.0 { slots.push((c, x, w * scale)); }
            x += w * scale;
        }
    }
    let slot_at = |qx: f32| slots.iter().find(|s| qx >= s.1 && qx < s.1 + s.2).or(slots.last()).map(|s| s.0);
    if all_w > 0.0 {
        let mut seps = Quads::default();
        for &(_, x, cw) in &slots {
            if cw >= 3.0 { let sx = (x * ppp).round() / ppp; seps.add(Rect::from_min_max(pos2(sx, lane.min.y + 1.0), pos2(sx + 1.0, lane.max.y)), lane, p.brd); }
        }
        seps.flush(painter, None);
        const SPACING: f32 = 96.0;
        // Slots are in column order: a binary search (a file may have thousands of columns).
        let center = |c: usize| slots.binary_search_by_key(&c, |s| s.0).ok().map(|k| slots[k].1 + slots[k].2 / 2.0);
        let mut picks: Vec<(usize, f32)> = Vec::new();
        let free = |picks: &Vec<(usize, f32)>, x: f32| picks.iter().all(|&(_, px)| (px - x).abs() >= SPACING);
        let pri = tab.sort.iter().map(|k| k.col).chain(tab.grid.cursor.map(|k| k.col));
        for c in pri.chain(slots.iter().filter(|s| s.2 >= 56.0).map(|s| s.0).collect::<Vec<_>>()).chain(slots.iter().map(|s| s.0).collect::<Vec<_>>()) {
            if let Some(x) = center(c) {
                if !picks.iter().any(|p| p.0 == c) && free(&picks, x) && x - lane.min.x >= 16.0 && lane.max.x - x >= 16.0 { picks.push((c, x)); }
            }
        }
        let pl = painter.with_clip_rect(lane);
        let font = semibold(9.0);
        let min_w = 5.0 * 5.2;
        for &(c, x) in &picks {
            let near = picks.iter().filter(|q| q.0 != c).map(|q| (q.1 - x).abs()).fold(f32::INFINITY, f32::min);
            let edge = 2.0 * (x - lane.min.x).min(lane.max.x - x);
            let max_w = (near - 12.0).min(edge - 4.0).min(200.0);
            if max_w < min_w { continue; }
            let sorted = tab.sort.iter().any(|k| k.col == c);
            let col = if sorted { p.prim } else if tab.grid.cursor.is_some_and(|k| k.col == c) { p.txt_hi } else { p.txt_dim };
            let mut job = LayoutJob::simple_singleline(heads.get(c).cloned().unwrap_or_default(), font.clone(), col);
            job.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let gl = pl.layout_job(job);
            // A label over its slot's separators: the strip's fill behind it.
            let bg = Rect::from_center_size(pos2(x, lane.center().y + 0.5), vec2(gl.size().x + 6.0, 13.0));
            pl.rect_filled(bg, 3.0, p.surf_2);
            pl.galley(pos2(((x - gl.size().x / 2.0) * ppp).round() / ppp, ((lane.center().y + 0.5 - gl.size().y / 2.0) * ppp).round() / ppp), gl, col);
        }
        // The gutter's slot: the row numbers.
        let gslot = Rect::from_min_max(htrack.min, pos2(cells_x0, htrack.max.y));
        painter.text(gslot.center(), Align2::CENTER_CENTER, "#", FontId::proportional(9.0), p.txt_faint);
        // The selection's columns: 3 px on top.
        if let Some((_, cs)) = tab.grid.range() {
            let a: f32 = laid[..*cs.start()].iter().sum::<f32>() * scale;
            let b: f32 = laid[..=*cs.end()].iter().sum::<f32>() * scale;
            painter.rect_filled(Rect::from_min_max(pos2(lane.min.x + a, lane.min.y + 1.0), pos2(lane.min.x + b.max(a + 2.0), lane.min.y + 4.0)), 0.0, p.prim);
        }
    }
    let r = ui.interact(lane, ui.scope_id().with("hscroll"), Sense::click_and_drag());
    let mut tip_at: Option<(f32, usize)> = r.hover_pos().filter(|_| !r.dragged()).and_then(|q| slot_at(q.x).map(|c| (q.x, c)));
    if total_w > view_w && all_w > 0.0 {
        let maxoff = total_w - view_w;
        let tw = (view_w * scale).max(22.0);
        let x0 = lane.min.x + frz_w * scale;
        let span = (lane.max.x - x0 - tw).max(1.0);
        let tx = x0 + span * (tab.grid.xoff / maxoff).min(1.0);
        let thumb = Rect::from_min_size(pos2(tx, lane.min.y + 2.0), vec2(tw, sw - 4.0));
        if r.drag_started() {
            if let Some(q) = r.interact_pointer_pos() { tab.grid.hthumb_grab = Some(if thumb.contains(q) { q.x - thumb.min.x } else { tw / 2.0 }); }
        }
        if let (Some(gb), Some(q)) = (tab.grid.hthumb_grab, r.interact_pointer_pos()) {
            if r.dragged() || r.drag_started() {
                tab.grid.xoff = ((q.x - gb - x0) / span).clamp(0.0, 1.0) * maxoff;
            }
        }
        if r.drag_stopped() { tab.grid.hthumb_grab = None; }
        if r.dragged() {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            // While dragging: the column at the thumb's centre, the tip following the pointer.
            if let Some(q) = r.interact_pointer_pos() { tip_at = slot_at(thumb.center().x).map(|c| (q.x, c)); }
        }
        let hot = r.hovered() || r.dragged();
        painter.rect(thumb, 5.0, if hot { p.thumb_hot } else { p.thumb }, Stroke::new(1.0, if hot { p.thumb_hot_brd } else { p.thumb_brd }), StrokeKind::Inside);
    }
    // The tip: the kind's glyph in its colour, then "Column N · title", just above the strip.
    if let Some((qx, c)) = tip_at {
        let (glyph, _, kc, _) = kind_style(tab, c, p);
        let title = heads.get(c).cloned().unwrap_or_default();
        let gl = painter.layout_no_wrap(format!("Column {c} · {}", title.chars().take(60).collect::<String>()), FontId::proportional(12.0), p.txt_hi);
        let gg = glyph.map(|g| painter.layout_no_wrap(g.into(), mono_bold(10.0), kc));
        let lead = gg.as_ref().map_or(11.0, |g| g.size().x) + 7.0;
        let size = vec2(lead + gl.size().x + 18.0, gl.size().y + 8.0);
        let screen = ctx.content_rect();
        let x = (qx - size.x / 2.0).min(screen.max.x - 6.0 - size.x).max(screen.min.x + 6.0);
        let tr = Rect::from_min_size(pos2((x * ppp).round() / ppp, ((htrack.min.y - 8.0 - size.y) * ppp).round() / ppp), size);
        let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("hstrip-tip")));
        fg.add(egui::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(102) }.as_shape(tr, 6.0));
        fg.rect(tr, 6.0, p.surf, Stroke::new(1.0, p.brd_2), StrokeKind::Inside);
        let cy = tr.center().y;
        match gg {
            Some(g) => fg.galley(pos2(tr.min.x + 9.0, ((cy - g.size().y / 2.0) * ppp).round() / ppp), g, kc),
            None => icons::calendar(&fg, Rect::from_min_size(pos2(tr.min.x + 9.0, cy - 5.5), vec2(11.0, 11.0)), 1.8, kc),
        }
        fg.galley(pos2(tr.min.x + 9.0 + lead, ((cy - gl.size().y / 2.0) * ppp).round() / ppp), gl, p.txt_hi);
    }

    // The corner where the strips meet: a navigator — the whole table in small (its columns
    // as faint bands), the part on screen as a window; a click or a drag goes there in both
    // directions at once, a double-click back to the start.
    let corner = Rect::from_min_max(pos2(full.max.x - sw, full.max.y - sw), full.max);
    painter.rect_filled(corner, 0.0, p.surf);
    painter.vline(corner.min.x + 0.5, corner.y_range(), Stroke::new(1.0, p.brd));
    painter.hline(corner.x_range(), corner.min.y + 0.5, Stroke::new(1.0, p.brd));
    let map = Rect::from_min_max(corner.min + vec2(5.0, 5.0), corner.max - vec2(4.0, 4.0));
    let cr = ui.interact(corner.shrink(1.0), ui.scope_id().with("strip-corner"), Sense::click_and_drag());
    let hot = cr.hovered() || cr.dragged();
    let rows_h = (rows as f64 * m.row_h as f64).max(1.0);
    if all_w > 0.0 {
        // Every other column a faint band.
        let mut q = Quads::default();
        let (mut x, k) = (map.min.x, map.width() / all_w);
        for (n, &(_, _, cw)) in slots.iter().enumerate() {
            let w = cw / scale * k;
            if n % 2 == 1 && w >= 0.5 { q.add(Rect::from_min_max(pos2(x, map.min.y), pos2(x + w, map.max.y)), map, alpha(p.gray, 0.09)); }
            x += w;
        }
        q.flush(painter, None);
    }
    painter.rect_stroke(map, 2.0, Stroke::new(1.0, alpha(p.gray, if hot { 0.45 } else { 0.28 })), StrokeKind::Inside);
    let (fx0, fx1) = if all_w > 0.0 { ((frz_w + tab.grid.xoff) / all_w, (frz_w + tab.grid.xoff + view_w) / all_w) } else { (0.0, 1.0) };
    let (fy0, fy1) = ((tab.grid.scroll / rows_h) as f32, ((tab.grid.scroll + tab.grid.body_h as f64) / rows_h) as f32);
    let snapv = |v: f32| (v * ppp).round() / ppp;
    let mut win = Rect::from_min_max(pos2(snapv(map.min.x + map.width() * fx0.clamp(0.0, 1.0)), snapv(map.min.y + map.height() * fy0.clamp(0.0, 1.0))), pos2(snapv(map.min.x + map.width() * fx1.clamp(0.0, 1.0)), snapv(map.min.y + map.height() * fy1.clamp(0.0, 1.0))));
    // At least 3 px each way, kept inside.
    if win.width() < 3.0 { win.max.x = (win.min.x + 3.0).min(map.max.x); win.min.x = win.max.x - 3.0; }
    if win.height() < 3.0 { win.max.y = (win.min.y + 3.0).min(map.max.y); win.min.y = win.max.y - 3.0; }
    painter.rect(win, 1.5, if hot { p.thumb_hot } else { p.thumb }, Stroke::new(1.0, if hot { p.accent } else { alpha(p.accent, 0.8) }), StrokeKind::Inside);
    let at = |q: Pos2| (((q.x - map.min.x) / map.width()).clamp(0.0, 1.0), ((q.y - map.min.y) / map.height()).clamp(0.0, 1.0));
    // Goes to the shares (fx, fy) of the table: the window centred there.
    let go = |tab: &mut Tab, fx: f32, fy: f32| {
        tab.grid.xoff = (fx * all_w - frz_w - view_w / 2.0).clamp(0.0, (total_w - view_w).max(0.0));
        tab.grid.scroll = (fy as f64 * rows_h - tab.grid.body_h as f64 / 2.0).clamp(0.0, tab.grid.max_scroll(rows));
    };
    if cr.double_clicked() {
        tab.grid.scroll = 0.0;
        tab.grid.xoff = 0.0;
        tab.grid.nav_hold = None;
    } else if cr.is_pointer_button_down_on() {
        if let Some(q) = cr.interact_pointer_pos() {
            let (_, big) = nav_rects(corner);
            match tab.grid.nav_hold {
                // Pressed: there at once, and the magnifier opens.
                None => {
                    let (fx, fy) = at(q);
                    tab.grid.nav_hold = Some(false);
                    go(tab, fx, fy);
                }
                // Held: once the pointer is on the magnified map, the view goes where it points
                // (held at the map's edges, so overshooting towards the corner reaches the end).
                Some(on) => if on || big.contains(q) {
                    tab.grid.nav_hold = Some(true);
                    go(tab, ((q.x - big.min.x) / big.width()).clamp(0.0, 1.0), ((q.y - big.min.y) / big.height()).clamp(0.0, 1.0));
                }
            }
        }
    } else {
        tab.grid.nav_hold = None;
    }
    if cr.dragged() { ctx.set_cursor_icon(CursorIcon::Grabbing); }
    if tab.grid.nav_hold.is_some() {
        let heads = titles(&st, tab);
        let widths: Vec<(usize, f32)> = slots.iter().map(|&(c, _, cw)| (c, cw / scale)).collect();
        magnifier(&ctx, tab, corner, &heads, &widths, all_w, (fx0, fx1, fy0, fy1), rows, p, m);
    } else if let Some(q) = cr.hover_pos() {
        let (fx, fy) = at(q);
        let row = ((fy as f64 * rows as f64) as usize).min(rows.saturating_sub(1)) + 1;
        let col = slot_at(lane.min.x + fx * lane.width()).unwrap_or(0);
        let ncols = st.ncols;
        tip(&ctx, p, m, corner, &format!("Row {} of {} · column {} of {}\nHold, then slide onto the map · double-click: back to the start", fmt_int(row.min(rows.max(1))), fmt_int(rows), col, fmt_int(ncols)));
    }
}

/// The magnified navigator (its box, its map): a square five times the corner's, just above
/// it with its map's bottom-right next to the corner — every part of the table a short move
/// up-left of the pointer, which sits by the screen's edge.
fn nav_rects(corner: Rect) -> (Rect, Rect) {
    let side = corner.width() * 5.0;
    let outer = Rect::from_min_max(pos2(corner.max.x - side - 20.0, corner.min.y - side - 50.0), pos2(corner.max.x - 4.0, corner.min.y - 6.0));
    let map = Rect::from_min_max(pos2(outer.min.x + 8.0, outer.max.y - 8.0 - side), outer.max - vec2(8.0, 8.0));
    (outer, map)
}

/// The navigator held down: the whole table, magnified, just above the corner — its columns
/// as bands named when they have room, the part on screen as the accent window, the row and
/// column it stands on spelled out above.
#[allow(clippy::too_many_arguments)]
fn magnifier(ctx: &egui::Context, tab: &Tab, corner: Rect, heads: &[String], widths: &[(usize, f32)], all_w: f32, (fx0, fx1, fy0, fy1): (f32, f32, f32, f32), rows: usize, p: &Palette, m: &Metrics) {
    let ppp = ctx.pixels_per_point();
    let snapv = |v: f32| (v * ppp).round() / ppp;
    let (outer, map) = nav_rects(corner);
    let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("nav-magnifier")));
    fg.add(egui::Shape::Rect(egui::epaint::RectShape::filled(outer.translate(vec2(0.0, 6.0)).expand(6.0), 12.0, Color32::from_black_alpha(70))));
    fg.rect(outer, 8.0, p.surf, Stroke::new(1.0, p.brd_2), StrokeKind::Inside);
    fg.rect_filled(map, 4.0, p.bg);
    // Columns: every other a band, named where the name fits.
    if all_w > 0.0 {
        let k = map.width() / all_w;
        let mut x = map.min.x;
        for (n, &(c, w)) in widths.iter().enumerate() {
            let cw = w * k;
            if n % 2 == 1 { fg.rect_filled(Rect::from_min_max(pos2(snapv(x), map.min.y), pos2(snapv(x + cw), map.max.y)), 0.0, alpha(p.gray, 0.10)); }
            if cw >= 34.0 {
                let mut job = LayoutJob::simple_singleline(heads.get(c).cloned().unwrap_or_default(), semibold(9.5), p.txt_dim);
                job.wrap = TextWrapping { max_width: cw - 6.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let g = fg.layout_job(job);
                fg.galley(pos2(snapv(x + 3.0), snapv(map.min.y + 3.0)), g, p.txt_dim);
            }
            x += cw;
        }
    }
    // The part on screen: the accent window, at least 6 px each way, a crosshair through it.
    let mut win = Rect::from_min_max(
        pos2(map.min.x + map.width() * fx0.clamp(0.0, 1.0), map.min.y + map.height() * fy0.clamp(0.0, 1.0)),
        pos2(map.min.x + map.width() * fx1.clamp(0.0, 1.0), map.min.y + map.height() * fy1.clamp(0.0, 1.0)),
    );
    let c = win.center();
    if win.width() < 6.0 { win = Rect::from_center_size(pos2(c.x, win.center().y), vec2(6.0, win.height())); }
    if win.height() < 6.0 { win = Rect::from_center_size(pos2(win.center().x, c.y), vec2(win.width(), 6.0)); }
    let win = Rect::from_min_max(pos2(snapv(win.min.x), snapv(win.min.y)), pos2(snapv(win.max.x), snapv(win.max.y)));
    fg.hline(map.x_range(), snapv(c.y) + 0.5, Stroke::new(1.0, alpha(p.accent, 0.35)));
    fg.vline(snapv(c.x) + 0.5, map.y_range(), Stroke::new(1.0, alpha(p.accent, 0.35)));
    fg.rect(win, 2.5, alpha(p.accent, 0.30), Stroke::new(1.5, p.accent), StrokeKind::Inside);
    fg.rect_stroke(map, 4.0, Stroke::new(1.0, p.brd), StrokeKind::Inside);
    // Where it stands.
    let row = ((fy0.max(0.0) as f64 * rows as f64) as usize + 1).min(rows.max(1));
    let col = widths.iter().scan(0.0f32, |acc, &(c, w)| { let a = *acc; *acc += w; Some((c, a, *acc)) }).find(|&(_, a, b)| { let x = fx0 * all_w; x >= a && x < b }).map_or(0, |x| x.0);
    // Above the map, cut to its width: the row (semibold 11 `txt_hi`), the column (11 `txt_mut`).
    let lines = [(format!("Row {}", fmt_int(row)), semibold(11.0), p.txt_hi), (heads.get(col).cloned().unwrap_or_default(), FontId::proportional(11.0), p.txt_mut)];
    for (i, (text, font, color)) in lines.into_iter().enumerate() {
        let mut job = LayoutJob::simple_singleline(text, font, color);
        job.wrap = TextWrapping { max_width: map.width(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        fg.galley(pos2(map.min.x, snapv(outer.min.y + 6.0 + i as f32 * 14.0)), fg.layout_job(job), color);
    }
    let _ = (tab, m);
}

/// The marks along the vertical strip, each computed in the background and kept until the
/// next is ready: irregular and marked rows (left), the find's matches (right), the rows a
/// chart part holds (left edge, by density).
#[allow(clippy::too_many_arguments)]
fn strip_marks(tab: &mut Tab, st: &State, painter: &egui::Painter, ctx: &egui::Context, vtrack: Rect, track: Rect, g: &look::StripGeom, rows: usize, p: &Palette) {
    let view_key = tab.view.as_ref().map_or(0, |v| Arc::as_ptr(v) as usize);
    let h = track.height().round() as u32;
    // The pictures' size in their keys, with the row height: few rows stand where they are drawn.
    let size = h | (g.row_h as u32) << 20;
    let view = tab.view.clone();
    // Irregular rows, duplicate groups and marked rows (as the web's strip: warn, violet, orange).
    let marked = !tab.marks.rows.is_empty() || tab.marks.dups.is_some();
    if !st.base.odd.is_empty() || marked {
        let key = (view_key, rows, tab.generation, Arc::as_ptr(&tab.marks) as usize, size);
        if let Some((j, k)) = &mut tab.grid.marks_job {
            if let Some(r) = j.poll() {
                let k = *k;
                tab.grid.marks_job = None;
                if let Some(v) = r { tab.grid.marks_pic = Some(StripPic { key: k, data: v.into_iter().map(u32::from).collect() }); }
            }
        }
        let have = tab.grid.marks_pic.as_ref().is_some_and(|x| x.key == key) || tab.grid.marks_job.as_ref().is_some_and(|x| x.1 == key);
        if !have && !tab.busy() {
            let (st2, view, marks, g2) = (st.clone(), view.clone(), tab.marks.clone(), *g);
            tab.grid.marks_job = Some((Job::spawn(ctx, move |cancel, _| {
                let ids = ids_of(&st2, &view);
                let irr = look::Irregular::new(&st2);
                let dup = |id| marks.dups.as_ref().is_some_and(|d| d.group(id).is_some());
                look::strip_marks(&ids, &g2, |id| if marks.rows.contains(&id) { 3 } else if dup(id) { 2 } else if irr.as_ref().is_some_and(|i| i.is(id)) { 1 } else { 0 }, cancel)
            }), key));
        }
        if let Some(pic) = &tab.grid.marks_pic {
            let bands: Vec<u8> = pic.data.iter().map(|&v| v as u8).collect();
            for (a, b, lvl) in look::mark_runs(&bands) {
                let r = Rect::from_min_max(pos2(vtrack.min.x + 1.0, track.min.y + a as f32), pos2(vtrack.min.x + 6.0, track.min.y + b.max(a + 2) as f32));
                painter.rect_filled(r, 0.0, match lvl { 3 => p.orange_txt, 2 => p.violet_txt, _ => p.warn });
            }
        }
    } else {
        tab.grid.marks_pic = None;
    }
    // The find's matches: ticks on the right edge.
    if tab.find.open && !tab.find.matches.is_empty() {
        let key = (Arc::as_ptr(&tab.find.matches) as usize, rows, size);
        if tab.grid.find_pic.as_ref().is_none_or(|x| x.key != key) {
            let mut v = vec![0u32; h as usize + 2];
            for &(pos, _) in tab.find.matches.iter() {
                let y = g.row_px(pos).min(v.len() - 1);
                v[y] += 1;
            }
            tab.grid.find_pic = Some(StripPic { key, data: v });
        }
        if let Some(pic) = &tab.grid.find_pic {
            for (y, &n) in pic.data.iter().enumerate() {
                if n > 0 { painter.rect_filled(Rect::from_min_size(pos2(vtrack.max.x - 5.0, track.min.y + y as f32), vec2(4.0, 2.0)), 0.0, p.ok); }
            }
        }
    }
    // The rows the chart part under the pointer holds.
    match (tab.spark.lit, tab.spark.base.clone()) {
        (Some((col, pick)), Some(spark)) if col < spark.cols.len() => {
            let key = (col, format!("{pick:?}"), view_key ^ rows, size);
            if let Some((j, k)) = &mut tab.grid.glow_job {
                if let Some(r) = j.poll() {
                    let k = k.clone();
                    tab.grid.glow_job = None;
                    if let Some(v) = r { tab.grid.glow_pic = Some(StripPic { key: k, data: v }); }
                }
            }
            let have = tab.grid.glow_pic.as_ref().is_some_and(|x| x.key == key) || tab.grid.glow_job.as_ref().is_some_and(|x| x.1 == key);
            if !have {
                let (st2, view, g2) = (st.clone(), view.clone(), *g);
                tab.grid.glow_job = Some((Job::spawn(ctx, move |cancel, _| {
                    let ids = ids_of(&st2, &view);
                    let keep = spark.keep(col, pick);
                    look::strip_counts(&ids, &g2, |id| { let mut tmp = Vec::new(); st2.with_cell(id, col, &mut tmp, |v| keep.test(v)) }, cancel)
                }), key.clone()));
            }
            if let Some(pic) = tab.grid.glow_pic.as_ref().filter(|x| x.key == key) {
                let max = pic.data.iter().copied().max().unwrap_or(0).max(1) as f32;
                for (y, &n) in pic.data.iter().enumerate() {
                    if n > 0 {
                        let a = 0.45 + 0.55 * (n as f32 / max).sqrt();
                        painter.rect_filled(Rect::from_min_size(pos2(vtrack.min.x + 1.0, track.min.y + y as f32), vec2(4.0, 1.0)), 0.0, alpha(p.accent_2, a));
                    }
                }
            }
        }
        _ => {
            tab.grid.glow_job = None;
        }
    }
}

fn commit_edit(tab: &mut Tab, st: &State, actions: &mut Vec<Action>) {
    if let Some(e) = tab.grid.editing.take() {
        if st.cell(e.id, e.at.col) != e.text {
            actions.push(Action::SetCells(vec![(e.id, e.at.col, e.text)]));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn keys(ui: &mut Ui, tab: &mut Tab, st: &State, rows: usize, ncols: usize, page: usize, view_w: f32, actions: &mut Vec<Action>) {
    if rows == 0 || ncols == 0 { return; }
    // Fill by example's ghosts: Tab takes them, Escape drops them.
    if let Some(g) = tab.ghost.clone() {
        let (take, drop) = ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Tab), i.consume_key(Modifiers::NONE, Key::Escape)));
        if take {
            let mut tmp = Vec::new();
            let ch: Vec<(RowId, usize, String)> = g.values.iter().filter(|(id, _)| st.with_cell(**id, g.col, &mut tmp, |s| s.is_empty())).map(|(id, v)| (*id, g.col, v.clone())).collect();
            if !ch.is_empty() { actions.push(Action::SetCells(ch)); }
            tab.ghost = None;
            return;
        }
        if drop { tab.ghost = None; return; }
    }
    let cur = tab.grid.cursor.unwrap_or(Cell { row: tab.grid.top, col: 0 });
    let mut moved: Option<(Cell, bool)> = None;
    let mut start_edit: Option<(String, bool)> = None;
    let mut select_all = false;
    let mut clear = false;
    let mut fill_down = std::mem::take(&mut tab.grid.want_fill_down);
    // Sideways, the hidden columns are stepped over.
    let hidden = tab.grid.hidden.clone();
    let first = (0..ncols).find(|c| !hidden.contains(c)).unwrap_or(0);
    let last = (0..ncols).rev().find(|c| !hidden.contains(c)).unwrap_or(ncols - 1);
    let next = |c: usize, d: i64| step_col(&hidden, c, d, ncols);
    ui.input_mut(|i| {
        let m = i.modifiers;
        let (shift, jump) = (m.shift, m.command);
        let last_row = rows - 1;
        let k = |i: &mut egui::InputState, key: Key| {
            i.consume_key(Modifiers::NONE, key) || i.consume_key(Modifiers::SHIFT, key) || i.consume_key(Modifiers::COMMAND, key) || i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, key)
        };
        let mut c = cur;
        let mut any = true;
        if k(i, Key::ArrowDown) { c.row = if jump { last_row } else { (c.row + 1).min(last_row) } }
        else if k(i, Key::ArrowUp) { c.row = if jump { 0 } else { c.row.saturating_sub(1) } }
        else if k(i, Key::ArrowRight) { c.col = if jump { last } else { next(c.col, 1) } }
        else if k(i, Key::ArrowLeft) { c.col = if jump { first } else { next(c.col, -1) } }
        else if k(i, Key::PageDown) { c.row = (c.row + page).min(last_row) }
        else if k(i, Key::PageUp) { c.row = c.row.saturating_sub(page) }
        else if k(i, Key::Home) { if jump { c.row = 0; } c.col = first }
        else if k(i, Key::End) { if jump { c.row = last_row; } c.col = last }
        else if i.consume_key(Modifiers::NONE, Key::Tab) { c.col = next(c.col, 1) }
        else if i.consume_key(Modifiers::SHIFT, Key::Tab) { c.col = next(c.col, -1) }
        else { any = false; }
        if any { moved = Some((c, shift)); }
        if i.consume_key(Modifiers::COMMAND, Key::A) {
            tab.grid.anchor = Some(Cell { row: 0, col: 0 });
            tab.grid.cursor = Some(Cell { row: last_row, col: last });
        }
        if i.consume_key(Modifiers::COMMAND, Key::D) { fill_down = true; }
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            start_edit = Some((String::new(), false));
            select_all = true;
        } else if i.consume_key(Modifiers::NONE, Key::F2) {
            start_edit = Some((String::new(), false));
        }
        if i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace) { clear = true; }
        if i.consume_key(Modifiers::COMMAND, Key::I) { actions.push(Action::RowCard); }
        let mut typed = None;
        i.events.retain(|e| match e {
            egui::Event::Text(t) if typed.is_none() && !m.command => { typed = Some(t.clone()); false }
            egui::Event::Copy => { actions.push(Action::Copy); false }
            egui::Event::Cut => { actions.push(Action::Copy); clear = true; false }
            egui::Event::Paste(s) => { actions.push(Action::Paste(s.clone())); false }
            _ => true,
        });
        if let Some(t) = typed { start_edit = Some((t, true)); }
    });
    if let Some((c, ext)) = moved {
        tab.grid.select(c, ext);
        tab.grid.reveal(rows, view_w);
    }
    if fill_down {
        // Ctrl+D: the filled cells at the top of each column seed a series through the rest.
        if tab.grid.block().is_none_or(|b| b.r1 == b.r0) {
            tab.grid.notice = Some("Fill series down: select the cells first — the values at the top of the selection, then the empty cells below them.".into());
        }
        if let Some(sel) = tab.grid.block().filter(|b| b.r1 > b.r0) {
            let (st2, view, vis) = (st.clone(), tab.view.clone(), (0..ncols).filter(|c| !tab.grid.hidden.contains(c)).collect::<Vec<_>>());
            tab.grid.fill_job = Some(Job::spawn(ui.ctx(), move |cancel, _| series::fill_down(&st2, &ids_of(&st2, &view), &vis, sel, cancel).map(|r| r.unwrap_or_default())));
        }
    }
    if let Some((text, fresh)) = start_edit {
        let id = tab.shown_id(cur.row);
        // Enter, F2 or a key typed keep a block selected: its value then goes to every cell, as the
        // web's (Ctrl+Enter: a series).
        let keep_block = tab.grid.range().is_some_and(|(r, cs)| r.contains(&cur.row) && cs.contains(&cur.col));
        if !keep_block {
            tab.grid.anchor = Some(cur);
            tab.grid.cursor = Some(cur);
        }
        let text = if fresh { text } else { st.cell(id, cur.col) };
        tab.grid.editing = Some(Editing { at: cur, id, text, fresh, select: select_all });
        tab.grid.reveal(rows, view_w);
    }
    if clear {
        if let Some(a) = fill_selection(tab, String::new(), false, &ui.ctx().clone()) {
            if matches!(&a, Action::Patch { patch, .. } if !patch.is_empty()) { actions.push(a); }
        }
    }
}

fn cell_menu(ui: &mut Ui, tab: &mut Tab, st: &State, c: Cell, heads: &[String], p: &Palette, actions: &mut Vec<Action>) {
    use crate::widgets::{head, sep, width, Item};
    if c.row >= tab.shown() { return; }
    let ctx = ui.ctx().clone();
    width(ui, 230.0);
    let id = tab.shown_id(c.row);
    let value = st.cell(id, c.col);
    let short: String = if value.chars().count() > 28 { value.chars().take(27).collect::<String>() + "…" } else { value.clone() };
    let col = heads.get(c.col).map_or("", |s| s.as_str());
    let rec = (id as usize + 1).saturating_sub(st.header as usize);
    head(ui, p, &format!("{col} · row {}", fmt_int(rec)));
    let many = tab.grid.range().is_some_and(|(r, cs)| r.start() != r.end() || cs.start() != cs.end());
    if Item::new(if many { "Copy the selection" } else { "Copy" }).icon(Icon::Copy).keys("Ctrl+C").show(ui, p).clicked() { actions.push(Action::Copy); ui.close(); }
    if Item::new("Paste").icon(Icon::Paste).keys("Ctrl+V").show(ui, p).clicked() { actions.push(Action::RequestPaste); ui.close(); }
    if Item::new("Copy the whole row").icon(Icon::Copy).show(ui, p).clicked() {
        // The columns shown, in their order (as the web's): hidden ones stay out.
        let cols = tab.grid.shown_cols(st.ncols);
        let tsv: Vec<String> = cols.iter().map(|&c| st.cell(id, c)).map(|v| if v.contains(['\t', '\n', '"']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v }).collect();
        ctx.copy_text(tsv.join("\t"));
        tab.grid.notice = Some(format!("Row {} copied.", fmt_int(rec)));
        ui.close();
    }
    sep(ui, p);
    let blank = value.trim().is_empty();
    let (keep, hide) = if blank { ("Only the rows where it is empty".to_string(), "Hide the rows where it is empty".to_string()) } else { (format!("Only the rows with “{short}”"), format!("Hide the rows with “{short}”")) };
    if Item::new(&keep).icon(Icon::Keep).show(ui, p).clicked() {
        actions.push(Action::FilterValue { col: c.col, value: value.clone(), keep: true });
        ui.close();
    }
    if Item::new(&hide).icon(Icon::Hide).show(ui, p).clicked() {
        actions.push(Action::FilterValue { col: c.col, value: value.clone(), keep: false });
        ui.close();
    }
    sep(ui, p);
    if Item::new("Fill by example").icon(Icon::Sparkles).keys("Ctrl+E").show(ui, p).clicked() { actions.push(Action::RunCommand("Fill by example")); ui.close(); }
    if Item::new("Row card").icon(Icon::Card).keys("Ctrl+I").show(ui, p).clicked() { actions.push(Action::RowCard); ui.close(); }
    if Item::new("Why does this row look wrong?").icon(Icon::Code).show(ui, p).clicked() { actions.push(Action::Diagnose(id)); ui.close(); }
    if Item::new("Insert a row below").icon(Icon::RowBelow).show(ui, p).clicked() { actions.push(Action::InsertRows { before: c.row + 1, count: 1 }); ui.close(); }
    let n = tab.grid.range().map_or(1, |(r, _)| r.end() - r.start() + 1);
    let label = if n > 1 { format!("Delete {} rows", fmt_int(n)) } else { "Delete the row".into() };
    if Item::new(&label).icon(Icon::Trash).danger().show(ui, p).clicked() {
        let ids = tab.grid.range().map(|(r, _)| r.map(|pos| tab.shown_id(pos)).collect()).unwrap_or_default();
        actions.push(Action::DeleteRows(ids));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_numbers_unit_is_found() {
        let at = |s: &'static str| super::unit_of(s).map(|(a, b)| &s[a..b]);
        assert_eq!(at("12 €"), Some("€"));
        assert_eq!(at("1 234,50 €"), Some("€"));
        assert_eq!(at("3$"), Some("$"));
        assert_eq!(at("$3"), Some("$"));
        assert_eq!(at("-€ 3"), Some("€"));
        assert_eq!(at("45 %"), Some("%"));
        assert_eq!(at("12"), None);
        assert_eq!(at("1.5e3"), None);
        assert_eq!(at("12 km"), None);
    }

    use super::*;

    #[test]
    fn thousands_french_style() {
        assert_eq!(fmt_int(1_000_000), "1\u{202f}000\u{202f}000");
        assert_eq!(fmt_int(999), "999");
    }

    #[test]
    fn json_indented_as_stringify() {
        assert_eq!(pretty_json(r#"{"a":1,"b":[1,2],"c":{}}"#), "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2\n  ],\n  \"c\": {}\n}");
        assert_eq!(pretty_json(r#"{"s":"x, {y}"}"#), "{\n  \"s\": \"x, {y}\"\n}");
    }

    #[test]
    fn hex_colours() {
        assert_eq!(parse_hex("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_hex("#ff0000"), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(parse_hex("#ff00"), Some(Color32::from_rgba_unmultiplied(255, 255, 0, 0)));
        assert_eq!(parse_hex("red"), None);
    }

    #[test]
    fn row_read_in_one_pass_matches_cell_by_cell() {
        use csvfab_engine::{Base, OpenOptions, Table};
        let csv = "a,b,c,d,e\n1,\"x, y\",\"he said \"\"hi\"\"\",,é\n2,short\n3,\"multi\nline\",z,w,v\n";
        let mut t = Table::new(Arc::new(Base::from_bytes(csv.as_bytes().to_vec(), OpenOptions::default())), None);
        t.set_cells(&[(1, 3, "edited".into())]);
        let st = &t.st;
        let mut rf = RowFields::default();
        let mut tmp = Vec::new();
        for want in [vec![(0, 0), (2, 2), (4, 4)], vec![(1, 1), (3, 3), (4, 4)], (0..5).map(|c| (c, c)).collect()] {
            for p in 0..st.len() {
                let id = st.data_id(p);
                rf.load(st, id, &want, &mut tmp);
                for &(c, _) in &want {
                    assert_eq!(rf.get(st, id, c).unwrap(), st.cell(id, c), "row {p} col {c}");
                }
            }
        }
    }

    #[test]
    fn easing_curves_end_where_css_does() {
        for (a, b, c, d) in [(0.12, 0.7, 0.25, 1.0), (0.6, 0.0, 0.8, 0.4), (0.42, 0.0, 1.0, 1.0)] {
            assert!(motion::bezier(a, b, c, d, 0.0).abs() < 1e-3);
            assert!((motion::bezier(a, b, c, d, 1.0) - 1.0).abs() < 1e-3);
            let mut last = 0.0;
            for k in 0..=20 {
                let v = motion::bezier(a, b, c, d, k as f32 / 20.0);
                assert!(v + 1e-4 >= last);
                last = v;
            }
        }
        // ease-in at half time is about .32 (CSS).
        assert!((motion::ease_in(0.5) - 0.315).abs() < 0.01);
    }

    #[test]
    fn quads_are_clipped_to_their_pane() {
        let mut q = Quads::default();
        let clip = Rect::from_min_max(pos2(10.0, 0.0), pos2(20.0, 10.0));
        q.add(Rect::from_min_max(pos2(0.0, 0.0), pos2(15.0, 5.0)), clip, Color32::RED);
        q.add(Rect::from_min_max(pos2(30.0, 0.0), pos2(40.0, 5.0)), clip, Color32::RED);
        assert_eq!(q.0.vertices.len(), 4);
        assert_eq!(q.0.vertices[0].pos.x, 10.0);
    }

    #[test]
    fn search_marks_ignore_accents() {
        let mut v = Vec::new();
        matches("Élodie", "elo", &mut v);
        assert_eq!(v, vec![(0, 4)]);
    }
}
