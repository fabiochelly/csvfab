//! A table being edited: the file as read (shared, immutable) plus what was
//! changed. Untouched cells stay bytes in the file; changed cells live in
//! per-column layers — each edit adds a layer (sorted row ids + one text
//! arena), the newest wins. A million changed cells of one column is one
//! arena of their text, not a million rows of strings; undo puts the previous
//! list of layers back, for free. Everything is behind `Arc`s, so a background
//! job (filter, sort, save) takes a snapshot for the cost of a few reference
//! counts.

use crate::base::Base;
use crate::scan;
use std::sync::Arc;

/// A row: the index of its record in the file, or ≥ base.n for a row created here.
pub type RowId = u32;

/// One edit's cells of one column: sorted ids, their texts end to end.
pub struct Layer {
    ids: Vec<RowId>,
    offs: Vec<u32>,
    arena: Vec<u8>,
}

impl Layer {
    /// From (id, text) pairs sorted by id, the last of equal ids winning.
    fn build(cells: &[(RowId, &str)]) -> Layer {
        let mut l = Layer { ids: Vec::with_capacity(cells.len()), offs: Vec::with_capacity(cells.len() + 1), arena: Vec::new() };
        l.arena.reserve(cells.iter().map(|c| c.1.len()).sum());
        l.offs.push(0);
        for (k, &(id, s)) in cells.iter().enumerate() {
            if cells.get(k + 1).is_some_and(|n| n.0 == id) { continue; }
            l.ids.push(id);
            l.arena.extend_from_slice(s.as_bytes());
            l.offs.push(l.arena.len() as u32);
        }
        l
    }

    #[inline]
    fn get(&self, id: RowId) -> Option<&str> {
        let k = self.ids.binary_search(&id).ok()?;
        let b = &self.arena[self.offs[k] as usize..self.offs[k + 1] as usize];
        // SAFETY: built from &str slices end to end, cut on their boundaries.
        Some(unsafe { std::str::from_utf8_unchecked(b) })
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// The rows this layer changes, sorted.
    pub fn ids(&self) -> &[RowId] {
        &self.ids
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.ids.capacity() * 4 + self.offs.capacity() * 4 + self.arena.capacity()
    }
}

/// A column's layers, oldest first.
pub type Layers = Arc<Vec<Arc<Layer>>>;

/// Past this many layers a column's are merged into one: lookups stay a few binary searches.
const MAX_LAYERS: usize = 12;

/// An arena's offsets are u32: a bigger edit is split into several layers.
const MAX_ARENA: usize = u32::MAX as usize - (1 << 20);

#[derive(Clone)]
pub struct State {
    pub base: Arc<Base>,
    /// Per current column, the cells changed here.
    pub cols: Arc<Vec<Layers>>,
    /// Every row in file order, header record included; None: the file's records as they are.
    pub order: Option<Arc<Vec<RowId>>>,
    /// Current column → field of the file's records (-1: a column added here); None: identity.
    pub cmap: Option<Arc<Vec<i32>>>,
    pub header: bool,
    pub ncols: usize,
    /// The line break a save writes between records; None: each record's own, as read.
    pub eol: Option<&'static [u8]>,
}

impl State {
    pub fn all_len(&self) -> usize {
        self.order.as_ref().map_or(self.base.n, |o| o.len())
    }

    /// Data rows (header excluded).
    pub fn len(&self) -> usize {
        self.all_len().saturating_sub(self.header as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn all_id(&self, pos: usize) -> RowId {
        self.order.as_ref().map_or(pos as RowId, |o| o[pos])
    }

    /// The id of data row `pos` (in file order).
    #[inline]
    pub fn data_id(&self, pos: usize) -> RowId {
        self.all_id(pos + self.header as usize)
    }

    #[inline]
    fn field_of(&self, col: usize) -> Option<usize> {
        match &self.cmap {
            None => Some(col),
            Some(m) => m.get(col).and_then(|&f| if f < 0 { None } else { Some(f as usize) }),
        }
    }

    /// The cell's text if it was changed here.
    #[inline]
    pub fn edited(&self, id: RowId, col: usize) -> Option<&str> {
        let layers = self.cols.get(col)?;
        layers.iter().rev().find_map(|l| l.get(id))
    }

    /// Runs `f` on one cell's text ("" when the row has no such field).
    #[inline]
    pub fn with_cell<R>(&self, id: RowId, col: usize, tmp: &mut Vec<u8>, f: impl FnOnce(&str) -> R) -> R {
        if let Some(s) = self.edited(id, col) {
            return f(s);
        }
        let b = &self.base;
        let Some(field) = self.field_of(col) else { return f("") };
        if id as usize >= b.n {
            return f("");
        }
        let rec = b.record(id as usize);
        if b.dialect.lines {
            return if field == 0 { f(&b.text(rec)) } else { f("") };
        }
        let mut f = Some(f);
        let r = scan::with_field(rec, b.dialect.delim, field, tmp, |v| (f.take().unwrap())(&b.text(v)));
        match r {
            Some(r) => r,
            None => (f.take().unwrap())(""),
        }
    }

    pub fn cell(&self, id: RowId, col: usize) -> String {
        self.with_cell(id, col, &mut Vec::new(), |s| s.to_string())
    }

    /// A row's fields in the current column layout. A record with more fields
    /// than there are columns keeps them (as read) when nothing remapped the columns.
    pub fn row(&self, id: RowId, out: &mut Vec<String>) {
        out.clear();
        if (id as usize) < self.base.n {
            self.base.fields(id as usize, out);
            if let Some(m) = &self.cmap {
                let mut fields = std::mem::take(out);
                out.extend(m.iter().map(|&f| if f < 0 { String::new() } else { fields.get_mut(f as usize).map(std::mem::take).unwrap_or_default() }));
            }
        }
        if !self.row_edited(id) {
            return;
        }
        if out.len() < self.ncols {
            out.resize(self.ncols, String::new());
        }
        for c in 0..self.ncols {
            if let Some(s) = self.edited(id, c) {
                out[c] = s.to_string();
            }
        }
        // An edited row is written with exactly its columns: trailing empties of a short record are not invented.
        while out.len() > self.ncols.max(1) && out.last().is_some_and(|s| s.is_empty()) {
            out.pop();
        }
    }

    /// Whether any cell of the row was changed here.
    #[inline]
    pub fn row_edited(&self, id: RowId) -> bool {
        self.cols.iter().any(|layers| layers.iter().any(|l| l.get(id).is_some()))
    }

    /// Whether the row is still its record's bytes as read.
    #[inline]
    pub fn pristine(&self, id: RowId) -> bool {
        self.cmap.is_none() && (id as usize) < self.base.n && !self.row_edited(id)
    }

    pub fn headers(&self) -> Vec<String> {
        let mut h = Vec::new();
        if self.header && self.all_len() > 0 {
            self.row(self.all_id(0), &mut h);
        }
        h.resize(self.ncols.max(h.len()), String::new());
        if !self.header {
            for (i, s) in h.iter_mut().enumerate() {
                *s = format!("Column {}", i + 1);
            }
        }
        h
    }

    /// Memory held by the edits.
    pub fn edit_bytes(&self) -> usize {
        self.cols.iter().flat_map(|l| l.iter()).map(|l| l.bytes()).sum()
    }
}

/// A set of rows as one bit per id: membership without hashing (deleting or dropping
/// rows among millions).
pub struct RowMask(Vec<u64>);

impl RowMask {
    pub fn new(ids: impl IntoIterator<Item = RowId>) -> RowMask {
        let mut m = RowMask(Vec::new());
        for id in ids {
            let w = id as usize / 64;
            if w >= m.0.len() { m.0.resize(w + 1, 0); }
            m.0[w] |= 1 << (id % 64);
        }
        m
    }

    #[inline]
    pub fn contains(&self, id: RowId) -> bool {
        self.0.get(id as usize / 64).is_some_and(|w| w >> (id % 64) & 1 == 1)
    }

    pub fn remove(&mut self, id: RowId) {
        if let Some(w) = self.0.get_mut(id as usize / 64) { *w &= !(1 << (id % 64)); }
    }
}

enum Change {
    /// Columns' previous layers.
    Cells(Vec<(usize, Layers)>),
    /// The previous structure, whole (rows or columns added, removed, moved).
    State { cols: Arc<Vec<Layers>>, order: Option<Arc<Vec<RowId>>>, cmap: Option<Arc<Vec<i32>>>, ncols: usize, header: bool, eol: Option<&'static [u8]> },
}

struct Op {
    serial: u64,
    change: Change,
}

/// What an undo or redo touched, for the view to follow.
pub enum Touched {
    Cells(Vec<usize>),
    Structure,
}

pub struct Table {
    pub st: State,
    next_id: RowId,
    undo: Vec<Op>,
    redo: Vec<Op>,
    serial: u64,
    saved: u64,
}

/// Whether line 1 looks like titles, as the reading guessed (the header's "Auto").
pub fn detected_header(base: &Base) -> bool {
    if base.dialect.lines || base.n == 0 {
        return false;
    }
    let mut f = Vec::new();
    base.fields(0, &mut f);
    crate::base::guess_header(&f)
}

impl Table {
    pub fn new(base: Arc<Base>, header: Option<bool>) -> Table {
        let width = base.odd.iter().map(|&(_, f)| f as usize).fold(base.width, usize::max).max(1);
        let header = header.unwrap_or_else(|| detected_header(&base));
        let next_id = base.n as RowId;
        Table {
            st: State { base, cols: Arc::new(vec![Layers::default(); width]), order: None, cmap: None, header, ncols: width, eol: None },
            next_id,
            undo: Vec::new(),
            redo: Vec::new(),
            serial: 0,
            saved: 0,
        }
    }

    pub fn dirty(&self) -> bool {
        self.version() != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.version();
    }

    /// The edit the table is at: a save in the background marks this one
    /// saved, even if more edits came meanwhile.
    pub fn version(&self) -> u64 {
        self.undo.last().map_or(0, |o| o.serial)
    }

    pub fn mark_saved_at(&mut self, version: u64) {
        self.saved = version;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Edits that Undo / Redo can still step through.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// The version Redo would bring back.
    pub fn redo_version(&self) -> Option<u64> {
        self.redo.last().map(|o| o.serial)
    }

    /// The versions Undo can step back through, oldest first.
    pub fn undo_versions(&self) -> Vec<u64> {
        self.undo.iter().map(|o| o.serial).collect()
    }

    fn push(&mut self, change: Change) {
        self.serial += 1;
        self.undo.push(Op { serial: self.serial, change });
        self.redo.clear();
    }

    fn snapshot_change(&self) -> Change {
        let s = &self.st;
        Change::State { cols: s.cols.clone(), order: s.order.clone(), cmap: s.cmap.clone(), ncols: s.ncols, header: s.header, eol: s.eol }
    }

    /// Runs several edits as one undo step.
    pub fn batch(&mut self, f: impl FnOnce(&mut Table)) {
        let before = self.snapshot_change();
        let n = self.undo.len();
        f(self);
        if self.undo.len() > n + 1 {
            self.undo.truncate(n);
            self.push(before);
        }
    }

    /// Grows the columns to `n` (a paste past the last one).
    fn grow(&mut self, n: usize) {
        if n <= self.st.ncols { return; }
        let mut m = self.current_cmap();
        m.resize(n, -1);
        self.st.cmap = Some(Arc::new(m));
        Arc::make_mut(&mut self.st.cols).resize(n, Layers::default());
        self.st.ncols = n;
    }

    /// Adds `cells` (sorted by id) as a layer of column `col`, merging its layers when there are too many.
    fn add_layer(cols: &mut Vec<Layers>, col: usize, cells: &[(RowId, &str)]) {
        let mut start = 0;
        while start < cells.len() {
            // Cut where the arena would outgrow its u32 offsets.
            let (mut end, mut size) = (start, 0usize);
            while end < cells.len() && size + cells[end].1.len() < MAX_ARENA { size += cells[end].1.len(); end += 1; }
            let layer = Arc::new(Layer::build(&cells[start..end]));
            Arc::make_mut(&mut cols[col]).push(layer);
            start = end.max(start + 1);
        }
        if cols[col].len() > MAX_LAYERS {
            let merged = merge(&cols[col]);
            cols[col] = Arc::new(merged);
        }
    }

    /// Sets cells: (row, column, text). One undo step.
    pub fn set_cells(&mut self, changes: &[(RowId, usize, String)]) {
        if changes.is_empty() {
            return;
        }
        let grow = changes.iter().map(|c| c.1 + 1).max().unwrap_or(0);
        let structural = grow > self.st.ncols;
        let before = if structural { Some(self.snapshot_change()) } else { None };
        self.grow(grow);
        let mut by_col: Vec<Vec<(RowId, &str)>> = vec![Vec::new(); self.st.ncols];
        for (id, col, text) in changes {
            by_col[*col].push((*id, text.as_str()));
        }
        let mut prev = Vec::new();
        let cols = Arc::make_mut(&mut self.st.cols);
        for (c, mut cells) in by_col.into_iter().enumerate() {
            if cells.is_empty() { continue; }
            // Stable: of the same cell set twice, the last one stays last.
            cells.sort_by_key(|x| x.0);
            prev.push((c, cols[c].clone()));
            Self::add_layer(cols, c, &cells);
        }
        self.push(before.unwrap_or(Change::Cells(prev)));
    }

    fn order_mut(&mut self) -> &mut Vec<RowId> {
        let n = self.st.base.n;
        let o = self.st.order.get_or_insert_with(|| Arc::new((0..n as RowId).collect()));
        Arc::make_mut(o)
    }

    /// Inserts `count` empty rows before data row `pos` (file order); returns their ids.
    pub fn insert_rows(&mut self, pos: usize, count: usize) -> Vec<RowId> {
        let c = self.snapshot_change();
        let ids: Vec<RowId> = (0..count as RowId).map(|k| self.next_id + k).collect();
        self.next_id += count as RowId;
        let at = (pos + self.st.header as usize).min(self.st.all_len());
        self.order_mut().splice(at..at, ids.iter().copied());
        self.push(c);
        ids
    }

    /// Moves row `id` next to row `target` (after it when `after`), in file order: a row
    /// dragged by its number (07-columns-structure.js "Move rows"). The title row stays
    /// first. Returns the row's data position before and after (None: nothing moved).
    pub fn move_row(&mut self, id: RowId, target: RowId, after: bool) -> Option<(usize, usize)> {
        if id == target { return None; }
        let head = self.st.header as usize;
        let all = self.st.all_len();
        let pos_of = |st: &State, x: RowId| (0..all).find(|&p| st.all_id(p) == x);
        let from = pos_of(&self.st, id).filter(|&p| p >= head)?;
        let to_t = pos_of(&self.st, target).filter(|&p| p >= head)?;
        // Its place once taken out of the order.
        let t = if to_t > from { to_t - 1 } else { to_t };
        let at = (t + after as usize).max(head);
        if at == from { return None; }
        let c = self.snapshot_change();
        let order = self.order_mut();
        order.remove(from);
        order.insert(at, id);
        self.push(c);
        Some((from - head, at - head))
    }

    /// Puts the data rows in the order `data` gives (every data row once: a sort's
    /// order written to the file). The title row stays first. False: `data` is not
    /// the table's rows, nothing changed.
    pub fn reorder(&mut self, data: &[RowId]) -> bool {
        let head = self.st.header as usize;
        if data.len() != self.st.len() { return false; }
        let mut seen = RowMask::new((0..self.st.len()).map(|p| self.st.data_id(p)));
        for &id in data {
            if !seen.contains(id) { return false; }
            seen.remove(id);
        }
        if (0..data.len()).all(|p| self.st.data_id(p) == data[p]) { return true; }
        let c = self.snapshot_change();
        let order = self.order_mut();
        order.truncate(head);
        order.extend_from_slice(data);
        self.push(c);
        true
    }

    pub fn delete_rows(&mut self, ids: &std::collections::HashSet<RowId>) {
        if ids.is_empty() { return; }
        let c = self.snapshot_change();
        let mut gone = RowMask::new(ids.iter().copied());
        if self.st.header { gone.remove(self.st.all_id(0)); }
        // The new order in one pass over the old (shared with the undo step: not copied
        // first), a bit per row instead of a hash lookup — a few rows or millions alike.
        // On the calling thread: a few ms for millions, whatever the machine is busy with.
        let keep = |id: &RowId| !gone.contains(*id);
        let mut order = Vec::with_capacity(self.st.all_len());
        match &self.st.order {
            Some(o) => order.extend(o.iter().copied().filter(keep)),
            None => order.extend((0..self.st.base.n as RowId).filter(keep)),
        }
        self.st.order = Some(Arc::new(order));
        self.push(c);
    }

    fn current_cmap(&self) -> Vec<i32> {
        match &self.st.cmap {
            Some(m) => (**m).clone(),
            None => (0..self.st.ncols as i32).collect(),
        }
    }

    /// Inserts an empty column before `at`, titled `title` when the file has a header.
    pub fn insert_column(&mut self, at: usize, title: &str) {
        let c = self.snapshot_change();
        let at = at.min(self.st.ncols);
        let mut m = self.current_cmap();
        m.insert(at, -1);
        self.st.cmap = Some(Arc::new(m));
        let head = (self.st.header && !title.is_empty() && self.st.all_len() > 0).then(|| self.st.all_id(0));
        let cols = Arc::make_mut(&mut self.st.cols);
        cols.insert(at, Layers::default());
        self.st.ncols += 1;
        if let Some(h) = head {
            Self::add_layer(cols, at, &[(h, title)]);
        }
        self.push(c);
    }

    pub fn delete_columns(&mut self, del: &[usize]) {
        if del.is_empty() { return; }
        let c = self.snapshot_change();
        let keep: Vec<bool> = (0..self.st.ncols).map(|i| !del.contains(&i)).collect();
        let m: Vec<i32> = self.current_cmap().into_iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, f)| f).collect();
        let cols: Vec<Layers> = self.st.cols.iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, l)| l.clone()).collect();
        self.st.ncols = m.len();
        self.st.cmap = Some(Arc::new(m));
        self.st.cols = Arc::new(cols);
        self.push(c);
    }

    pub fn move_column(&mut self, from: usize, to: usize) {
        if from == to || from >= self.st.ncols { return; }
        let c = self.snapshot_change();
        let mut m = self.current_cmap();
        let f = m.remove(from);
        let to = to.min(m.len());
        m.insert(to, f);
        self.st.cmap = Some(Arc::new(m));
        let cols = Arc::make_mut(&mut self.st.cols);
        let l = cols.remove(from);
        cols.insert(to, l);
        self.push(c);
    }

    /// Every record's line break made `eol` at the next save (one undo step); the file's own
    /// (`base.eol`, its first) again: each record keeps its own. Quoted line breaks stay.
    pub fn set_eol(&mut self, eol: &'static [u8]) {
        let want = (eol != self.st.base.eol).then_some(eol);
        if want == self.st.eol { return; }
        let c = self.snapshot_change();
        self.st.eol = want;
        self.push(c);
    }

    pub fn set_header(&mut self, header: bool) {
        if header == self.st.header { return; }
        let c = self.snapshot_change();
        self.st.header = header;
        self.push(c);
    }

    fn apply(&mut self, change: Change) -> (Change, Touched) {
        match change {
            Change::Cells(prev) => {
                let cols = Arc::make_mut(&mut self.st.cols);
                let mut inverse = Vec::with_capacity(prev.len());
                let mut touched = Vec::with_capacity(prev.len());
                for (c, layers) in prev {
                    inverse.push((c, std::mem::replace(&mut cols[c], layers)));
                    touched.push(c);
                }
                (Change::Cells(inverse), Touched::Cells(touched))
            }
            Change::State { cols, order, cmap, ncols, header, eol } => {
                let inv = self.snapshot_change();
                self.st.cols = cols;
                self.st.order = order;
                self.st.cmap = cmap;
                self.st.ncols = ncols;
                self.st.header = header;
                self.st.eol = eol;
                (inv, Touched::Structure)
            }
        }
    }

    pub fn undo(&mut self) -> Option<Touched> {
        let op = self.undo.pop()?;
        let (inv, t) = self.apply(op.change);
        self.redo.push(Op { serial: op.serial, change: inv });
        Some(t)
    }

    pub fn redo(&mut self) -> Option<Touched> {
        let op = self.redo.pop()?;
        let (inv, t) = self.apply(op.change);
        self.undo.push(Op { serial: op.serial, change: inv });
        Some(t)
    }
}

/// Growing a table read in part (`Base::open_head`) into the whole file.
impl Table {
    /// Puts `base` — the same file with more records after the ones known —
    /// under the table, edits and undo history kept: rows created here move
    /// past the new records (their ids shift), the new records come after
    /// the last record known, in file order.
    pub fn rebase(&mut self, base: Arc<Base>) {
        let (old, new) = (self.st.base.n as RowId, base.n as RowId);
        debug_assert!(new >= old);
        let delta = new - old;
        let map = |id: RowId| if id >= old { id + delta } else { id };
        let remap_layers = |layers: &Layers| -> Layers {
            Arc::new(layers.iter().map(|l| {
                if l.ids.last().is_none_or(|&x| x < old) { return l.clone(); }
                Arc::new(Layer { ids: l.ids.iter().map(|&x| map(x)).collect(), offs: l.offs.clone(), arena: l.arena.clone() })
            }).collect())
        };
        let remap_cols = |cols: &Arc<Vec<Layers>>| -> Arc<Vec<Layers>> { Arc::new(cols.iter().map(remap_layers).collect()) };
        let remap_order = |order: &Option<Arc<Vec<RowId>>>| -> Option<Arc<Vec<RowId>>> {
            order.as_ref().map(|o| {
                let mut v: Vec<RowId> = o.iter().map(|&x| map(x)).collect();
                let at = v.iter().rposition(|&x| x < old).map_or(v.len(), |k| k + 1);
                v.splice(at..at, old..new);
                Arc::new(v)
            })
        };
        let remap_change = |c: &mut Change| match c {
            Change::Cells(v) => for (_, l) in v.iter_mut() { *l = remap_layers(l); },
            Change::State { cols, order, .. } => { *cols = remap_cols(cols); *order = remap_order(order); }
        };
        if delta > 0 {
            self.st.cols = remap_cols(&self.st.cols);
            self.st.order = remap_order(&self.st.order);
            for op in self.undo.iter_mut().chain(self.redo.iter_mut()) { remap_change(&mut op.change); }
            self.next_id += delta;
        }
        // Wider records further down: more columns (unless columns were rearranged here).
        let width = base.odd.iter().map(|&(_, f)| f as usize).fold(base.width, usize::max);
        if self.st.cmap.is_none() && width > self.st.ncols {
            Arc::make_mut(&mut self.st.cols).resize(width, Layers::default());
            self.st.ncols = width;
        }
        self.st.base = base;
    }
}

/// A column's layers as one, the newest text of each cell winning.
fn merge(layers: &[Arc<Layer>]) -> Vec<Arc<Layer>> {
    let mut all: Vec<(RowId, usize, &str)> = Vec::with_capacity(layers.iter().map(|l| l.len()).sum());
    for (age, l) in layers.iter().enumerate() {
        for k in 0..l.ids.len() {
            let s = unsafe { std::str::from_utf8_unchecked(&l.arena[l.offs[k] as usize..l.offs[k + 1] as usize]) };
            all.push((l.ids[k], age, s));
        }
    }
    all.sort_unstable_by_key(|x| (x.0, x.1));
    let cells: Vec<(RowId, &str)> = all.into_iter().map(|(id, _, s)| (id, s)).collect();
    if cells.iter().map(|c| c.1.len()).sum::<usize>() >= MAX_ARENA {
        return layers.to_vec();
    }
    vec![Arc::new(Layer::build(&cells))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::OpenOptions;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    #[test]
    fn rows_move() {
        let mut t = table("a\n1\n2\n3\n4\n");
        let id = |t: &Table, p: usize| t.st.data_id(p);
        let vals = |t: &Table| (0..t.st.len()).map(|p| t.st.cell(t.st.data_id(p), 0)).collect::<Vec<_>>().join("");
        let (one, three) = (id(&t, 0), id(&t, 2));
        assert_eq!(t.move_row(one, three, true), Some((0, 2)));
        assert_eq!(vals(&t), "2314");
        // Before the first data row: never above the titles.
        let four = id(&t, 3);
        assert_eq!(t.move_row(four, id(&t, 0), false), Some((3, 0)));
        assert_eq!(vals(&t), "4231");
        assert_eq!(t.st.cell(t.st.all_id(0), 0), "a");
        // Onto itself, or next to where it already is: nothing, no undo step.
        let two = id(&t, 1);
        assert_eq!(t.move_row(two, id(&t, 2), false), None);
        assert_eq!(t.move_row(two, two, true), None);
        let saved = crate::save::write_to(&t.st, &Default::default(), Vec::new()).unwrap();
        assert_eq!(saved, b"a\n4\n2\n3\n1\n");
        t.undo();
        assert_eq!(vals(&t), "2314");
        t.undo();
        assert_eq!(vals(&t), "1234");
        assert!(!t.dirty());
    }

    #[test]
    fn edit_undo_redo() {
        let mut t = table("a,b\n1,2\n3,4\n");
        assert!(t.st.header);
        assert_eq!(t.st.len(), 2);
        let id = t.st.data_id(1);
        t.set_cells(&[(id, 1, "x".into())]);
        assert_eq!(t.st.cell(id, 1), "x");
        assert!(t.dirty());
        assert!(!t.st.pristine(id));
        t.undo();
        assert_eq!(t.st.cell(id, 1), "4");
        assert!(t.st.pristine(id));
        assert!(!t.dirty());
        t.redo();
        assert_eq!(t.st.cell(id, 1), "x");
    }

    #[test]
    fn layers_newest_wins_and_merge() {
        let mut t = table("a\n0\n1\n2\n");
        for k in 0..40 {
            t.set_cells(&[(t.st.data_id(k % 3), 0, format!("v{k}"))]);
        }
        assert!(t.st.cols[0].len() <= MAX_LAYERS);
        assert_eq!(t.st.cell(t.st.data_id(0), 0), "v39");
        assert_eq!(t.st.cell(t.st.data_id(1), 0), "v37");
        assert_eq!(t.st.cell(t.st.data_id(2), 0), "v38");
        for _ in 0..40 { t.undo(); }
        assert_eq!(t.st.cell(t.st.data_id(0), 0), "0");
        // The same cell twice in one change: the last stays.
        t.set_cells(&[(1, 0, "a".into()), (1, 0, "b".into())]);
        assert_eq!(t.st.cell(1, 0), "b");
    }

    #[test]
    fn rows_and_columns() {
        let mut t = table("a,b\n1,2\n3,4\n");
        let ids = t.insert_rows(1, 1);
        assert_eq!(t.st.len(), 3);
        assert_eq!(t.st.data_id(1), ids[0]);
        assert_eq!(t.st.cell(ids[0], 0), "");
        t.insert_column(1, "mid");
        assert_eq!(t.st.headers(), vec!["a", "mid", "b"]);
        assert_eq!(t.st.cell(t.st.data_id(0), 2), "2");
        t.set_cells(&[(t.st.data_id(0), 2, "x".into())]);
        t.delete_columns(&[0]);
        assert_eq!(t.st.headers(), vec!["mid", "b"]);
        assert_eq!(t.st.cell(t.st.data_id(0), 1), "x");
        t.move_column(1, 0);
        assert_eq!(t.st.headers(), vec!["b", "mid"]);
        assert_eq!(t.st.cell(t.st.data_id(0), 0), "x");
        let mut del = std::collections::HashSet::new();
        del.insert(t.st.data_id(0));
        t.delete_rows(&del);
        assert_eq!(t.st.len(), 2);
        while t.undo().is_some() {}
        assert_eq!(t.st.headers(), vec!["a", "b"]);
        assert_eq!(t.st.len(), 2);
        assert!(!t.dirty());
    }

    #[test]
    fn paste_past_the_last_column_and_batch() {
        let mut t = table("a,b\n1,2\n");
        let id = t.st.data_id(0);
        t.batch(|t| {
            t.set_cells(&[(id, 3, "far".into())]);
            t.insert_rows(1, 1);
        });
        assert_eq!(t.st.ncols, 4);
        assert_eq!(t.st.len(), 2);
        let mut r = Vec::new();
        t.st.row(id, &mut r);
        assert_eq!(r, vec!["1", "2", "", "far"]);
        t.undo();
        assert_eq!(t.st.ncols, 2);
        assert_eq!(t.st.len(), 1);
        assert!(!t.can_undo());
    }

    #[test]
    fn rows_reorder() {
        let mut t = table("a\n3\n1\n2\n");
        let id = |t: &Table, p: usize| t.st.data_id(p);
        let vals = |t: &Table| (0..t.st.len()).map(|p| t.st.cell(t.st.data_id(p), 0)).collect::<Vec<_>>().join("");
        let (three, one, two) = (id(&t, 0), id(&t, 1), id(&t, 2));
        // Not the table's rows (one missing, one twice): nothing changes.
        assert!(!t.reorder(&[one, two]));
        assert!(!t.reorder(&[one, one, two]));
        assert!(!t.can_undo());
        // The order it already has: no undo step.
        assert!(t.reorder(&[three, one, two]));
        assert!(!t.can_undo());
        assert!(t.reorder(&[one, two, three]));
        assert_eq!(vals(&t), "123");
        assert_eq!(t.st.cell(t.st.all_id(0), 0), "a");
        let saved = crate::save::write_to(&t.st, &Default::default(), Vec::new()).unwrap();
        assert_eq!(saved, b"a\n1\n2\n3\n");
        t.undo();
        assert_eq!(vals(&t), "312");
    }

    #[test]
    fn a_million_cells_is_small() {
        let n = 1_000_000;
        let src: String = std::iter::once("a\n".to_string()).chain((0..n).map(|i| format!("{i}\n"))).collect();
        let mut t = table(&src);
        let ch: Vec<(RowId, usize, String)> = (0..n).map(|p| (t.st.data_id(p), 0, format!("2024-01-{:02}", p % 28 + 1))).collect();
        t.set_cells(&ch);
        assert!(t.st.edit_bytes() < 20 << 20, "{} bytes", t.st.edit_bytes());
        assert_eq!(t.st.cell(t.st.data_id(27), 0), "2024-01-28");
    }

    #[test]
    fn rebase_keeps_edits() {
        let full: Vec<u8> = (0..50).flat_map(|i| format!("{i},v{i}\n").into_bytes()).collect();
        let head = Base::from_bytes(full[..40].to_vec(), crate::OpenOptions::default());   // 0..9, cut mid-record
        let head_n = head.n;
        let mut t = Table::new(Arc::new(head), Some(false));
        t.set_cells(&[(2, 1, "edited".into())]);
        let ids = t.insert_rows(head_n, 1);
        t.set_cells(&[(ids[0], 0, "new".into())]);
        let fb = Base::from_bytes(full.clone(), crate::OpenOptions::default());
        let n = fb.n;
        t.rebase(Arc::new(fb));
        assert_eq!(t.st.len(), n + 1);
        assert_eq!(t.st.cell(2, 1), "edited");
        let last = t.st.data_id(n);
        assert_eq!(t.st.cell(last, 0), "new");
        assert_eq!(t.st.cell(t.st.data_id(n - 1), 1), format!("v{}", n - 1));
        t.undo();
        assert_eq!(t.st.len(), n + 1);
        assert_eq!(t.st.cell(t.st.data_id(n), 0), "");
        t.undo();
        assert_eq!(t.st.len(), n);
        assert_eq!(t.st.cell(2, 1), "edited");
    }
}
