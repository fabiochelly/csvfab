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
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A row: the index of its record in the file, or ≥ base.n for a row created here.
pub type RowId = u32;

/// One edit's cells of one column: sorted ids, their texts end to end — or one text for
/// them all (a selection cleared or filled with a value: 4 bytes a cell, the ids shared by
/// every column it spans).
pub struct Layer {
    ids: Arc<Vec<RowId>>,
    /// Cell k is `arena[offs[k]..offs[k + 1]]`; empty: every cell is the whole arena.
    offs: Vec<u32>,
    arena: Vec<u8>,
}

impl Layer {
    /// From (id, text) pairs sorted by id, the last of equal ids winning.
    fn build(cells: &[(RowId, &str)]) -> Layer {
        let (mut ids, mut offs, mut arena) = (Vec::with_capacity(cells.len()), Vec::with_capacity(cells.len() + 1), Vec::with_capacity(cells.iter().map(|c| c.1.len()).sum()));
        offs.push(0);
        for (k, &(id, s)) in cells.iter().enumerate() {
            if cells.get(k + 1).is_some_and(|n| n.0 == id) { continue; }
            ids.push(id);
            arena.extend_from_slice(s.as_bytes());
            offs.push(arena.len() as u32);
        }
        Layer { ids: Arc::new(ids), offs, arena }
    }

    /// `text` in every row of `ids` (sorted, no repeats).
    fn uniform(ids: Arc<Vec<RowId>>, text: &str) -> Layer {
        Layer { ids, offs: Vec::new(), arena: text.as_bytes().to_vec() }
    }

    /// Cell k's text.
    #[inline]
    fn text(&self, k: usize) -> &str {
        let b = if self.offs.is_empty() { &self.arena[..] } else { &self.arena[self.offs[k] as usize..self.offs[k + 1] as usize] };
        // SAFETY: built from &str slices end to end, cut on their boundaries.
        unsafe { std::str::from_utf8_unchecked(b) }
    }

    #[inline]
    fn get(&self, id: RowId) -> Option<&str> {
        let k = self.ids.binary_search(&id).ok()?;
        Some(self.text(k))
    }

    /// The one text of a layer that gives every row the same.
    fn uniform_text(&self) -> Option<&[u8]> {
        self.offs.is_empty().then_some(&self.arena[..])
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

    /// Its ids, cell ends (empty: one text for all) and text, as a journal keeps them.
    pub(crate) fn parts(&self) -> (&[RowId], &[u32], &[u8]) {
        (&self.ids, &self.offs, &self.arena)
    }

    /// A layer from `parts` read back: None unless the ids increase, the ends fit the text and
    /// every cell is whole UTF-8 (cells are read unchecked).
    pub(crate) fn from_parts(ids: Vec<RowId>, offs: Vec<u32>, arena: Vec<u8>) -> Option<Layer> {
        let text = std::str::from_utf8(&arena).ok()?;
        let ok = ids.windows(2).all(|w| w[0] < w[1])
            && (offs.is_empty() || (offs.len() == ids.len() + 1 && offs[0] == 0 && offs.windows(2).all(|w| w[0] <= w[1]) && *offs.last()? as usize == arena.len()
                && offs.iter().all(|&o| text.is_char_boundary(o as usize))));
        ok.then(|| Layer { ids: Arc::new(ids), offs, arena })
    }

    /// Its memory, the ids shared with the other columns of the same edit counted once among them.
    pub fn bytes(&self) -> usize {
        self.ids.capacity() * 4 / Arc::strong_count(&self.ids) + self.offs.capacity() * 4 + self.arena.capacity()
    }
}

/// Cells of one column being gathered, ids in order with their texts end to end: pieces
/// built side by side (a computed column, chunk by chunk), made one layer by `Patch::column`.
/// A piece holds less than 4 GB of text.
#[derive(Default)]
pub struct Cells {
    ids: Vec<RowId>,
    ends: Vec<u32>,
    text: Vec<u8>,
}

impl Cells {
    pub fn with_capacity(n: usize) -> Cells {
        Cells { ids: Vec::with_capacity(n), ends: Vec::with_capacity(n), text: Vec::new() }
    }

    #[inline]
    pub fn push(&mut self, id: RowId, s: &str) {
        self.ids.push(id);
        self.text.extend_from_slice(s.as_bytes());
        self.ends.push(self.text.len() as u32);
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    #[inline]
    fn text(&self, k: usize) -> &str {
        let start = if k == 0 { 0 } else { self.ends[k - 1] as usize };
        // SAFETY: pushed as whole &strs.
        unsafe { std::str::from_utf8_unchecked(&self.text[start..self.ends[k] as usize]) }
    }

    /// Strictly increasing ids.
    fn sorted(&self) -> bool {
        self.ids.windows(2).all(|w| w[0] < w[1])
    }
}

/// Pieces whose ids follow each other (each sorted, each after the previous one), their
/// text under MAX_ARENA in all, as one layer: copied into place side by side.
fn join(pieces: &[Cells]) -> Layer {
    let n: usize = pieces.iter().map(Cells::len).sum();
    let bytes: usize = pieces.iter().map(|p| p.text.len()).sum();
    let (mut ids, mut offs, mut arena) = (vec![0 as RowId; n], vec![0u32; n + 1], vec![0u8; bytes]);
    let mut work = Vec::with_capacity(pieces.len());
    let (mut ir, mut or, mut ar) = (&mut ids[..], &mut offs[1..], &mut arena[..]);
    let mut base = 0u32;
    for p in pieces {
        let (i, rest) = std::mem::take(&mut ir).split_at_mut(p.len());
        ir = rest;
        let (o, rest) = std::mem::take(&mut or).split_at_mut(p.len());
        or = rest;
        let (a, rest) = std::mem::take(&mut ar).split_at_mut(p.text.len());
        ar = rest;
        work.push((p, i, o, a, base));
        base += p.text.len() as u32;
    }
    work.into_par_iter().for_each(|(p, i, o, a, base)| {
        i.copy_from_slice(&p.ids);
        for (o, &e) in o.iter_mut().zip(&p.ends) { *o = base + e; }
        a.copy_from_slice(&p.text);
    });
    Layer { ids: Arc::new(ids), offs, arena }
}

/// (id, text) pairs sorted by id as layers, cut where an arena would outgrow its u32 offsets.
fn build_layers(cells: &[(RowId, &str)]) -> Vec<Arc<Layer>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        let (mut end, mut size) = (start, 0usize);
        while end < cells.len() && size + cells[end].1.len() < MAX_ARENA { size += cells[end].1.len(); end += 1; }
        // Never between two cells of the same row: the last of them wins within one layer.
        while end > start + 1 && end < cells.len() && cells[end].0 == cells[end - 1].0 { end -= 1; }
        out.push(Arc::new(Layer::build(&cells[start..end.max(start + 1)])));
        start = end.max(start + 1);
    }
    out
}

/// A column's layers, oldest first.
pub type Layers = Arc<Vec<Arc<Layer>>>;

/// Past this many layers some of a column's are merged into one: lookups stay a few binary searches.
const MAX_LAYERS: usize = 12;

/// An arena's offsets are u32: a bigger edit is split into several layers.
const MAX_ARENA: usize = u32::MAX as usize - (1 << 20);

/// Cells of some columns made ready to go in (`Table::apply`): each column's layers built —
/// sorted, their text in one arena — where it suits the caller, in a job, so that the table
/// only links them in. The same cells as `Table::set_cells` would write.
pub struct Patch {
    /// Per column (each once), its new layers, oldest first.
    cols: Vec<(usize, Vec<Arc<Layer>>)>,
    cells: usize,
    blank: bool,
}

impl Patch {
    /// From (row, column, text) changes, the last of the same cell winning; the columns sorted in parallel.
    pub fn new(changes: &[(RowId, usize, String)]) -> Patch {
        let ncols = changes.iter().map(|c| c.1 + 1).max().unwrap_or(0);
        let mut count = vec![0usize; ncols];
        for c in changes { count[c.1] += 1; }
        let mut by_col: Vec<Vec<(RowId, &str)>> = count.iter().map(|&n| Vec::with_capacity(n)).collect();
        for (id, col, text) in changes { by_col[*col].push((*id, text.as_str())); }
        let cols = by_col.into_par_iter().enumerate().filter(|(_, c)| !c.is_empty()).map(|(c, mut cells)| {
            // Stable: of the same cell set twice, the last one stays last.
            if !cells.windows(2).all(|w| w[0].0 <= w[1].0) {
                if cells.len() > 1 << 16 { cells.par_sort_by_key(|x| x.0) } else { cells.sort_by_key(|x| x.0) }
            }
            (c, build_layers(&cells))
        }).collect();
        Patch { cols, cells: changes.len(), blank: changes.iter().all(|c| c.2.is_empty()) }
    }

    /// `value` in the cells of `cols` over rows `ids` that do not hold it already (a selection
    /// cleared or filled): one layer per column, all sharing one text and — where every row
    /// changes — one list of ids. None if cancelled.
    pub fn fill(st: &State, ids: &[RowId], cols: &[usize], value: &str, cancel: &AtomicBool) -> Option<Patch> {
        let mut rows = ids.to_vec();
        rows.par_sort_unstable();
        rows.dedup();
        let mut cols = cols.to_vec();
        cols.sort_unstable();
        cols.dedup();
        // Per chunk of rows (in id order: the file read front to back), per column, the rows
        // whose cell changes; each row split once for all the columns.
        let parts = crate::stats::chunked(rows.len(), cancel, || vec![Vec::new(); cols.len()], |acc: &mut Vec<Vec<RowId>>, p| {
            let id = rows[p];
            st.visit_cols(id, &cols, |k, v| if v != value { acc[k].push(id) });
        })?;
        let rows = Arc::new(rows);
        let per_col: Vec<(usize, Vec<Arc<Layer>>, usize)> = cols.par_iter().enumerate().filter_map(|(k, &c)| {
            let n: usize = parts.iter().map(|p| p[k].len()).sum();
            if n == 0 { return None; }
            let ids = if n == rows.len() { rows.clone() } else {
                let mut v = Vec::with_capacity(n);
                for p in &parts { v.extend_from_slice(&p[k]); }
                Arc::new(v)
            };
            Some((c, vec![Arc::new(Layer::uniform(ids, value))], n))
        }).collect();
        let cells = per_col.iter().map(|x| x.2).sum();
        Some(Patch { cols: per_col.into_iter().map(|(c, l, _)| (c, l)).collect(), cells, blank: value.is_empty() })
    }

    /// Column `col`'s cells gathered in pieces (in row order or not; each row once).
    pub fn column(col: usize, mut pieces: Vec<Cells>) -> Patch {
        pieces.retain(|p| !p.is_empty());
        let cells = pieces.iter().map(Cells::len).sum();
        let blank = pieces.iter().all(|p| p.text.is_empty());
        let follow = pieces.iter().all(Cells::sorted) && pieces.windows(2).all(|w| w[0].ids.last() < w[1].ids.first());
        let layers = if follow {
            // Cut into layers where the text would outgrow an arena's offsets.
            let mut out = Vec::new();
            let (mut start, mut size) = (0, 0usize);
            for (k, p) in pieces.iter().enumerate() {
                if size + p.text.len() >= MAX_ARENA && k > start {
                    out.push(Arc::new(join(&pieces[start..k])));
                    (start, size) = (k, 0);
                }
                size += p.text.len();
            }
            if start < pieces.len() { out.push(Arc::new(join(&pieces[start..]))); }
            out
        } else {
            // Rows in another order than their ids (moved, inserted here): sorted first.
            let mut refs: Vec<(RowId, &str)> = pieces.iter().flat_map(|p| (0..p.len()).map(move |k| (p.ids[k], p.text(k)))).collect();
            refs.par_sort_by_key(|x| x.0);
            build_layers(&refs)
        };
        Patch { cols: if cells > 0 { vec![(col, layers)] } else { Vec::new() }, cells, blank }
    }

    /// Cells written (a cell given twice counted twice, as the changes said them).
    pub fn cells(&self) -> usize {
        self.cells
    }

    pub fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    /// Every cell emptied.
    pub fn blank(&self) -> bool {
        self.blank
    }

    /// The columns it writes.
    pub fn columns(&self) -> impl Iterator<Item = usize> + '_ {
        self.cols.iter().map(|c| c.0)
    }

    /// The texts of its first column, by row id.
    pub fn texts(&self) -> impl Iterator<Item = &str> + '_ {
        self.cols.iter().take(1).flat_map(|c| c.1.iter()).flat_map(|l| (0..l.len()).map(move |k| l.text(k)))
    }
}

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

    /// The data rows of `rows` in the table's order: sorted, without an order of its own (the
    /// file's); else one parallel pass over it.
    pub fn in_order(&self, rows: &std::collections::HashSet<RowId>) -> Vec<RowId> {
        let head = self.header as RowId;
        if self.order.is_none() {
            let mut v: Vec<RowId> = rows.iter().copied().filter(|&id| id >= head && (id as usize) < self.base.n).collect();
            v.sort_unstable();
            return v;
        }
        (0..self.len()).into_par_iter().with_min_len(1 << 14).map(|p| self.data_id(p)).filter(|id| rows.contains(id)).collect()
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

    /// `edited`, the layers searched only in a column that has some.
    #[inline]
    fn edited_in(&self, id: RowId, col: usize) -> Option<&str> {
        self.cols.get(col).filter(|l| !l.is_empty()).and_then(|_| self.edited(id, col))
    }

    /// Runs `f(column, text)` on the row's cells in column order: the fields `row` gives,
    /// without a String each. The record is split once, its fields mapped to the current
    /// columns (moved, inserted, deleted ones alike), and only the columns holding edits
    /// look in their layers.
    pub fn visit(&self, id: RowId, mut f: impl FnMut(usize, &str)) {
        let b = &*self.base;
        let real = (id as usize) < b.n;
        let edited = self.row_edited(id);
        if self.cmap.is_none() && !edited {
            // As read: the record's fields in order, straight from its bytes.
            if real { with_gather(|g| fields_of(b, id, &mut g.tmp, |i, v| f(i, &b.text(v)))) }
            return;
        }
        with_gather(|g| {
            let rec: &[u8] = if real { b.record(id as usize) } else { b"" };
            let nf = match &self.cmap {
                _ if !real => {
                    g.spans.clear();
                    0
                }
                None => {
                    g.gather(b, rec, |i| Some(i));
                    g.spans.len()
                }
                Some(m) => {
                    g.inv.clear();
                    g.inv.resize(m.iter().map(|&x| x + 1).max().unwrap_or(0).max(0) as usize, u32::MAX);
                    for (c, &x) in m.iter().enumerate() { if x >= 0 { g.inv[x as usize] = c as u32; } }
                    let inv = std::mem::take(&mut g.inv);
                    g.gather(b, rec, |i| inv.get(i).filter(|&&c| c != u32::MAX).map(|&c| c as usize));
                    g.inv = inv;
                    g.spans.resize(m.len(), Span::None);
                    m.len()
                }
            };
            let mut len = nf;
            if edited {
                // As `row`: every column, an edited row's trailing empty extra fields dropped.
                len = len.max(self.ncols);
                while len > self.ncols.max(1) && g.bytes(rec, len - 1).is_empty() { len -= 1; }
            }
            for c in 0..len {
                match self.edited_in(id, c) {
                    Some(s) => f(c, s),
                    None => f(c, &b.text(g.bytes(rec, c))),
                }
            }
        })
    }

    /// Runs `f(k, text)` on the cells `cols[k]` of the row (as `with_cell`: "" where there is
    /// none), any columns in any order: the record split once for all of them.
    pub fn visit_cols(&self, id: RowId, cols: &[usize], mut f: impl FnMut(usize, &str)) {
        let b = &*self.base;
        if cols.len() < 2 || b.dialect.lines || id as usize >= b.n {
            return with_gather(|g| for (k, &c) in cols.iter().enumerate() { self.with_cell(id, c, &mut g.tmp, |v| f(k, v)) });
        }
        let rec = b.record(id as usize);
        let edited = cols.iter().any(|&c| self.edited_in(id, c).is_some());
        if self.cmap.is_none() && !edited && cols.windows(2).all(|w| w[0] < w[1]) {
            // Columns in field order, none edited: the fields streamed, those wanted decoded.
            let mut k = 0;
            with_gather(|g| scan::visit_fields(rec, b.dialect.delim, &mut g.tmp, |i, v| {
                if i == cols[k] {
                    f(k, &b.text(v));
                    k += 1;
                }
                k < cols.len()
            }));
            for j in k..cols.len() { f(j, ""); }
            return;
        }
        with_gather(|g| {
            // (field, k) of the cells read from the record, in field order.
            g.want.clear();
            g.want.extend(cols.iter().enumerate().filter(|&(_, &c)| self.edited_in(id, c).is_none()).filter_map(|(k, &c)| self.field_of(c).map(|fl| (fl, k))));
            g.want.sort_unstable();
            let want = std::mem::take(&mut g.want);
            g.spans.clear();
            g.spans.resize(cols.len(), Span::None);
            g.arena.clear();
            let mut j = 0;
            let Gather { tmp, arena, spans, .. } = g;
            if !want.is_empty() {
                scan::visit_fields(rec, b.dialect.delim, tmp, |i, v| {
                    while j < want.len() && want[j].0 < i { j += 1; }
                    while j < want.len() && want[j].0 == i {
                        spans[want[j].1] = Span::of(rec, v, arena);
                        j += 1;
                    }
                    j < want.len()
                });
            }
            g.want = want;
            for (k, &c) in cols.iter().enumerate() {
                match self.edited_in(id, c) {
                    Some(s) => f(k, s),
                    None => f(k, &b.text(g.bytes(rec, k))),
                }
            }
        })
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

/// Where a field's bytes are: in the record, or unescaped (`""` → `"`) in the arena.
#[derive(Clone, Copy)]
enum Span {
    None,
    Rec(usize, usize),
    Arena(usize, usize),
}

impl Span {
    #[inline]
    fn of(rec: &[u8], v: &[u8], arena: &mut Vec<u8>) -> Span {
        let at = (v.as_ptr() as usize).wrapping_sub(rec.as_ptr() as usize);
        if at <= rec.len() && at + v.len() <= rec.len() { return Span::Rec(at, at + v.len()); }
        arena.extend_from_slice(v);
        Span::Arena(arena.len() - v.len(), arena.len())
    }
}

/// The row visitors' scratch, one per thread: a record's fields gathered by column.
#[derive(Default)]
struct Gather {
    tmp: Vec<u8>,
    arena: Vec<u8>,
    spans: Vec<Span>,
    /// Field → current column (u32::MAX: none).
    inv: Vec<u32>,
    want: Vec<(usize, usize)>,
}

impl Gather {
    /// Each field `i` of the record whose column `to(i)` gives, by column.
    fn gather(&mut self, b: &Base, rec: &[u8], to: impl Fn(usize) -> Option<usize>) {
        self.spans.clear();
        self.arena.clear();
        let Gather { tmp, arena, spans, .. } = self;
        let mut put = |c: usize, s: Span| {
            if c >= spans.len() { spans.resize(c + 1, Span::None); }
            spans[c] = s;
        };
        if b.dialect.lines {
            if let Some(c) = to(0) { put(c, Span::Rec(0, rec.len())); }
            return;
        }
        scan::visit_fields(rec, b.dialect.delim, tmp, |i, v| {
            if let Some(c) = to(i) { put(c, Span::of(rec, v, arena)); }
            true
        });
    }

    #[inline]
    fn bytes<'a>(&'a self, rec: &'a [u8], c: usize) -> &'a [u8] {
        match self.spans.get(c) {
            Some(&Span::Rec(a, e)) => &rec[a..e],
            Some(&Span::Arena(a, e)) => &self.arena[a..e],
            _ => b"",
        }
    }
}

thread_local! {
    static GATHER: std::cell::RefCell<Gather> = std::cell::RefCell::default();
}

/// The thread's scratch (a fresh one when a visitor runs inside another's callback).
fn with_gather<R>(f: impl FnOnce(&mut Gather) -> R) -> R {
    GATHER.with(|g| match g.try_borrow_mut() {
        Ok(mut g) => f(&mut g),
        Err(_) => f(&mut Gather::default()),
    })
}

/// Record `id`'s fields as read (unescaped bytes), in order; a text file's line is one field.
#[inline]
fn fields_of(b: &Base, id: RowId, tmp: &mut Vec<u8>, mut f: impl FnMut(usize, &[u8])) {
    let rec = b.record(id as usize);
    if b.dialect.lines { return f(0, rec); }
    scan::visit_fields(rec, b.dialect.delim, tmp, |i, v| { f(i, v); true });
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
    /// The previous structure, whole (rows or columns added, removed, moved). `header`: line 1's
    /// reading before, only when the edit changed it (`set_header_edit`); a plain toggle
    /// (`set_header`) is no edit, which Undo leaves as it is.
    State { cols: Arc<Vec<Layers>>, order: Option<Arc<Vec<RowId>>>, cmap: Option<Arc<Vec<i32>>>, ncols: usize, header: Option<bool>, eol: Option<&'static [u8]> },
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
    // Up to five records below it, for the types of their columns.
    let sample: Vec<Vec<String>> = (1..base.n.min(6)).map(|i| { let mut r = Vec::new(); base.fields(i, &mut r); r }).collect();
    crate::base::guess_header(&f, &sample)
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

    /// A table in state `st` (a journal replayed): no undo history, unsaved.
    pub(crate) fn restored(st: State) -> Table {
        let ids = st.order.iter().flat_map(|o| o.iter()).chain(st.cols.iter().flat_map(|c| c.iter()).flat_map(|l| l.ids().iter()));
        let next_id = ids.map(|&id| id + 1).max().unwrap_or(0).max(st.base.n as RowId);
        Table { st, next_id, undo: Vec::new(), redo: Vec::new(), serial: 0, saved: u64::MAX }
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

    /// Edits between the file as last saved (or read) and now — what Save would write, counted
    /// in undo steps (undone past the save: those steps). As the web's pending count.
    pub fn pending_edits(&self) -> usize {
        if !self.dirty() { return 0; }
        // The steps in order: the undo stack, then the redo stack from its top.
        let at = |serial: u64| -> Option<usize> {
            if serial == 0 { return Some(0); }
            self.undo.iter().chain(self.redo.iter().rev()).position(|o| o.serial == serial).map(|k| k + 1)
        };
        match at(self.saved) {
            Some(k) => self.undo.len().abs_diff(k),
            None => self.undo.len().max(1),
        }
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
        Change::State { cols: s.cols.clone(), order: s.order.clone(), cmap: s.cmap.clone(), ncols: s.ncols, header: None, eol: s.eol }
    }

    /// Runs several edits as one undo step.
    pub fn batch(&mut self, f: impl FnOnce(&mut Table)) {
        let mut before = self.snapshot_change();
        let h0 = self.st.header;
        let n = self.undo.len();
        f(self);
        if self.undo.len() > n + 1 {
            // Line 1's reading comes back with the step when one of its edits changed it.
            let header_edited = self.undo[n..].iter().any(|o| matches!(o.change, Change::State { header: Some(_), .. }));
            if let Change::State { header, .. } = &mut before { if header_edited { *header = Some(h0); } }
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

    /// Sets cells: (row, column, text). One undo step.
    pub fn set_cells(&mut self, changes: &[(RowId, usize, String)]) {
        self.apply(Patch::new(changes));
    }

    /// Puts a patch's cells in (built beforehand, maybe in a job, on this table or one it was
    /// before cell edits): its layers linked in. One undo step.
    pub fn apply(&mut self, p: Patch) {
        let Some(grow) = p.cols.iter().map(|c| c.0 + 1).max() else { return };
        let before = (grow > self.st.ncols).then(|| self.snapshot_change());
        self.grow(grow);
        let cols = Arc::make_mut(&mut self.st.cols);
        let mut prev = Vec::with_capacity(p.cols.len());
        for (c, layers) in p.cols {
            prev.push((c, cols[c].clone()));
            let l = Arc::make_mut(&mut cols[c]);
            l.extend(layers);
            compact(l);
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
        // The new order in one pass over the old one (which the undo step keeps): never a
        // copy of it first, then the rows after moved aside.
        self.st.order = Some(Arc::new(spliced(self.st.order.as_deref().map(|o| o.as_slice()), self.st.all_len(), at, &ids)));
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
        let old = self.st.order.clone();
        let pos_of = |x: RowId| match old.as_deref() {
            None => ((x as usize) < all).then_some(x as usize),
            Some(o) => o.par_iter().with_min_len(1 << 16).position_any(|&y| y == x),
        };
        let from = pos_of(id).filter(|&p| p >= head)?;
        let to_t = pos_of(target).filter(|&p| p >= head)?;
        // Its place once taken out of the order.
        let t = if to_t > from { to_t - 1 } else { to_t };
        let at = (t + after as usize).max(head);
        if at == from { return None; }
        let c = self.snapshot_change();
        // The new order in one copy of the old one (which the undo step keeps), the row put in
        // its place on the way: not a copy, then everything after it moved twice.
        let mut v: Vec<RowId> = Vec::with_capacity(all);
        let put = |r: std::ops::Range<usize>, v: &mut Vec<RowId>| match old.as_deref() {
            Some(o) => v.extend_from_slice(&o[r]),
            None => v.extend(r.map(|p| p as RowId)),
        };
        if at < from {
            put(0..at, &mut v);
            v.push(id);
            put(at..from, &mut v);
            put(from + 1..all, &mut v);
        } else {
            put(0..from, &mut v);
            put(from + 1..at + 1, &mut v);
            v.push(id);
            put(at + 1..all, &mut v);
        }
        self.st.order = Some(Arc::new(v));
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
        self.delete_mask(&RowMask::new(ids.iter().copied()));
    }

    /// `delete_rows` of the rows set in `gone` (millions selected: no hash set of them made);
    /// the title row stays.
    pub fn delete_mask(&mut self, gone: &RowMask) {
        let c = self.snapshot_change();
        let head = self.st.header.then(|| self.st.all_id(0));
        // The new order in one pass over the old (shared with the undo step: not copied
        // first), a bit per row instead of a hash lookup — a few rows or millions alike.
        // On the calling thread: a few ms for millions, whatever the machine is busy with.
        let keep = |id: &RowId| !gone.contains(*id) || Some(*id) == head;
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
            Arc::make_mut(&mut cols[at]).push(Arc::new(Layer::build(&[(h, title)])));
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

    /// Whether line 1 holds the titles: how the file is read, not an edit — the bytes saved
    /// are the same either way. No undo step, the table stays as clean or dirty as it was,
    /// and Undo / Redo of other edits leave it as it is.
    pub fn set_header(&mut self, header: bool) {
        self.st.header = header;
    }

    /// Line 1's reading changed as part of an edit (a title line added to a file without
    /// one): Undo brings the reading back with it.
    pub fn set_header_edit(&mut self, header: bool) {
        if header == self.st.header { return; }
        let mut c = self.snapshot_change();
        if let Change::State { header: h, .. } = &mut c { *h = Some(self.st.header); }
        self.st.header = header;
        self.push(c);
    }

    fn revert(&mut self, change: Change) -> (Change, Touched) {
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
                let mut inv = self.snapshot_change();
                if let Some(h) = header {
                    if let Change::State { header: ih, .. } = &mut inv { *ih = Some(self.st.header); }
                    self.st.header = h;
                }
                self.st.cols = cols;
                self.st.order = order;
                self.st.cmap = cmap;
                self.st.ncols = ncols;
                self.st.eol = eol;
                (inv, Touched::Structure)
            }
        }
    }

    pub fn undo(&mut self) -> Option<Touched> {
        let op = self.undo.pop()?;
        let (inv, t) = self.revert(op.change);
        self.redo.push(Op { serial: op.serial, change: inv });
        Some(t)
    }

    pub fn redo(&mut self) -> Option<Touched> {
        let op = self.redo.pop()?;
        let (inv, t) = self.revert(op.change);
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
                Arc::new(Layer { ids: Arc::new(l.ids.iter().map(|&x| map(x)).collect()), offs: l.offs.clone(), arena: l.arena.clone() })
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

/// `ids` with `ins` put before position `at`, in one pass (a view of millions of rows a row
/// is inserted in: never copied first, then moved aside).
pub fn splice_ids(ids: &[RowId], at: usize, ins: &[RowId]) -> Vec<RowId> {
    spliced(Some(ids), ids.len(), at.min(ids.len()), ins)
}

/// The order `old` (None: the file's, `n` rows) with `ins` put before position `at`, written
/// in parallel blocks: a few copies each, for millions of rows.
fn spliced(old: Option<&[RowId]>, n: usize, at: usize, ins: &[RowId]) -> Vec<RowId> {
    const B: usize = 1 << 16;
    let k = ins.len();
    let mut v = vec![0 as RowId; n + k];
    let from_old = |dst: &mut [RowId], start: usize| match old {
        Some(o) => dst.copy_from_slice(&o[start..start + dst.len()]),
        None => for (j, x) in dst.iter_mut().enumerate() { *x = (start + j) as RowId },
    };
    v.par_chunks_mut(B).enumerate().for_each(|(c, out)| {
        let (b, e) = (c * B, c * B + out.len());
        let s1 = e.min(at).max(b);
        from_old(&mut out[..s1 - b], b);
        let s2 = e.min(at + k).max(s1);
        if s2 > s1 { out[s1 - b..s2 - b].copy_from_slice(&ins[s1 - at..s2 - at]); }
        from_old(&mut out[s2 - b..], s2 - k.min(s2));
    });
    v
}

/// Past MAX_LAYERS, the run of layers holding the fewest cells is merged into one, down to
/// half as many layers: a column's big edits (a computed column, a selection cleared) are
/// not read again for every few cells typed after them.
fn compact(layers: &mut Vec<Arc<Layer>>) {
    if layers.len() <= MAX_LAYERS { return; }
    let w = layers.len() - MAX_LAYERS / 2 + 1;
    let size: Vec<usize> = layers.iter().map(|l| l.len()).collect();
    let mut sum: usize = size[..w].iter().sum();
    let mut best = (sum, 0);
    for s in 1..=layers.len() - w {
        sum = sum + size[s + w - 1] - size[s - 1];
        if sum < best.0 { best = (sum, s); }
    }
    let s = best.1;
    if let Some(m) = merge(&layers[s..s + w]) {
        layers.splice(s..s + w, [Arc::new(m)]);
    }
}

/// Consecutive layers of a column as one, the newest text of each cell winning: their sorted
/// ids merged in one pass, ranges of ids side by side. None when the text would outgrow an arena.
fn merge(layers: &[Arc<Layer>]) -> Option<Layer> {
    let total: usize = layers.iter().map(|l| l.len()).sum();
    // Cut where the biggest layer's ids are evenly spaced.
    let big = layers.iter().max_by_key(|l| l.len())?;
    let parts = (total >> 16).clamp(1, rayon::current_num_threads() * 4).min(big.len().max(1));
    let mut cuts: Vec<RowId> = (1..parts).map(|k| big.ids[k * big.len() / parts]).collect();
    cuts.dedup();
    let bounds: Vec<(Option<RowId>, Option<RowId>)> = (0..=cuts.len()).map(|k| (k.checked_sub(1).map(|j| cuts[j]), cuts.get(k).copied())).collect();
    let pieces: Vec<Cells> = bounds.into_par_iter().map(|(lo, hi)| merge_range(layers, lo, hi)).collect();
    if pieces.iter().map(|p| p.text.len()).sum::<usize>() >= MAX_ARENA {
        return None;
    }
    let m = join(&pieces);
    // Layers of one same text (selections cleared one after the other) stay one text.
    match layers[0].uniform_text() {
        Some(u) if layers.iter().all(|l| l.uniform_text() == Some(u)) => Some(Layer { ids: m.ids, offs: Vec::new(), arena: u.to_vec() }),
        _ => Some(m),
    }
}

/// The merged cells of ids in `lo..hi` (None: unbounded).
fn merge_range(layers: &[Arc<Layer>], lo: Option<RowId>, hi: Option<RowId>) -> Cells {
    let at = |l: &Layer, b: Option<RowId>, none: usize| b.map_or(none, |b| l.ids.partition_point(|&x| x < b));
    let mut pos: Vec<usize> = layers.iter().map(|l| at(l, lo, 0)).collect();
    let end: Vec<usize> = layers.iter().map(|l| at(l, hi, l.len())).collect();
    let mut out = Cells::with_capacity(pos.iter().zip(&end).map(|(a, b)| b - a).sum());
    loop {
        let mut min: Option<RowId> = None;
        for (j, l) in layers.iter().enumerate() {
            if pos[j] < end[j] && min.is_none_or(|m| l.ids[pos[j]] < m) { min = Some(l.ids[pos[j]]); }
        }
        let Some(id) = min else { break };
        // Every layer holding the row steps over it; the newest (last) gives its text.
        let mut win = 0;
        for (j, l) in layers.iter().enumerate() {
            if pos[j] < end[j] && l.ids[pos[j]] == id { win = j; pos[j] += 1; }
        }
        out.push(id, layers[win].text(pos[win] - 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::OpenOptions;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    #[test]
    fn pending_edits_count_since_the_save() {
        let mut t = Table::new(Arc::new(Base::from_bytes(b"a\n1\n2\n".to_vec(), Default::default())), None);
        let id = t.st.data_id(0);
        for v in ["x", "y", "z"] { t.set_cells(&[(id, 0, v.into())]); }
        assert_eq!(t.pending_edits(), 3);
        t.mark_saved();
        assert_eq!(t.pending_edits(), 0);
        t.set_cells(&[(id, 0, "w".into())]);
        assert_eq!(t.pending_edits(), 1);
        t.undo();
        t.undo();
        assert_eq!(t.pending_edits(), 1, "undone past the save");
        t.redo();
        assert_eq!(t.pending_edits(), 0);
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
        // Marked rows in the table's order, with and without an order of its own.
        let set: std::collections::HashSet<RowId> = [id(&t, 3), id(&t, 0), 999].into_iter().collect();
        assert_eq!(t.st.in_order(&set), vec![id(&t, 0), id(&t, 3)]);
        let f = table("a\n1\n2\n3\n");
        let set: std::collections::HashSet<RowId> = [id(&f, 2), id(&f, 0), 0, 999].into_iter().collect();
        assert_eq!(f.st.in_order(&set), vec![id(&f, 0), id(&f, 2)]);
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

    /// Every cell of the table, by data row.
    fn grid(t: &Table) -> Vec<Vec<String>> {
        (0..t.st.len()).map(|p| (0..t.st.ncols).map(|c| t.st.cell(t.st.data_id(p), c)).collect()).collect()
    }

    /// A selection filled (or cleared) as a patch: the same cells as the changes the grid
    /// made, the cells holding the value already left as they were (their rows still as read).
    #[test]
    fn patch_fill_as_changes() {
        let mut t = table("a,b,c\n1,,x\n,2,\n3,4,5\n,,\n");
        let ids: Vec<RowId> = vec![t.st.data_id(2), t.st.data_id(0), t.st.data_id(1), t.st.data_id(0), t.st.data_id(3)];
        let no = AtomicBool::new(false);
        let mut r = table("a,b,c\n1,,x\n,2,\n3,4,5\n,,\n");
        let ch: Vec<(RowId, usize, String)> = ids.iter().flat_map(|&id| [0, 2].map(|c| (id, c))).filter(|&(id, c)| !r.st.cell(id, c).is_empty()).map(|(id, c)| (id, c, String::new())).collect();
        r.set_cells(&ch);
        let p = Patch::fill(&t.st, &ids, &[2, 0], "", &no).unwrap();
        assert_eq!((p.cells(), p.blank()), (4, true));
        t.apply(p);
        assert_eq!(grid(&t), grid(&r));
        assert!(t.st.pristine(t.st.data_id(3)), "an empty row stays as read");
        // Shared ids: a column whose every row changes takes the list of rows itself.
        let p = Patch::fill(&t.st, &ids, &[0, 1, 2], "z", &no).unwrap();
        assert_eq!(p.cells(), 12);
        let id_lists: Vec<*const Vec<RowId>> = p.cols.iter().map(|c| Arc::as_ptr(&c.1[0].ids)).collect();
        assert!(id_lists.windows(2).all(|w| w[0] == w[1]));
        t.apply(p);
        assert!(grid(&t).iter().all(|r| r.iter().all(|v| v == "z")));
        t.undo();
        assert_eq!(grid(&t), grid(&r));
        t.undo();
        assert!(!t.dirty());
        assert_eq!(t.st.cell(t.st.data_id(0), 2), "x");
        // Nothing to change: no undo step.
        let p = Patch::fill(&t.st, &[t.st.data_id(3)], &[0, 1], "", &no).unwrap();
        assert!(p.is_empty());
        t.apply(p);
        assert!(!t.can_undo());
    }

    /// A column gathered in pieces (a computed column): the cells `set_cells` would write,
    /// rows in id order or not.
    #[test]
    fn patch_column_as_changes() {
        let src: String = std::iter::once("a\n".to_string()).chain((0..300).map(|i| format!("{i}\n"))).collect();
        for moved in [false, true] {
            let mut t = table(&src);
            if moved { let (a, b) = (t.st.data_id(5), t.st.data_id(200)); t.move_row(a, b, true); }
            let mut r = Table { st: t.st.clone(), next_id: t.next_id, undo: Vec::new(), redo: Vec::new(), serial: 0, saved: 0 };
            let vals: Vec<(RowId, String)> = (0..t.st.len()).filter(|p| p % 7 != 3).map(|p| (t.st.data_id(p), format!("v{p}"))).collect();
            let pieces: Vec<Cells> = vals.chunks(64).map(|ch| { let mut c = Cells::default(); for (id, v) in ch { c.push(*id, v); } c }).collect();
            let p = Patch::column(1, pieces);
            assert_eq!(p.cells(), vals.len());
            t.apply(p);
            r.set_cells(&vals.iter().map(|(id, v)| (*id, 1, v.clone())).collect::<Vec<_>>());
            assert_eq!(grid(&t), grid(&r));
            assert_eq!(t.st.ncols, 2);
            t.undo();
            assert_eq!(t.st.ncols, 1);
        }
    }

    /// Hundreds of edits of every size, fills and computed columns among them, against a
    /// plain map of every cell: the layers merged as they pile up give the same cells, a big
    /// layer is not merged again for the small edits after it, and Undo walks back through all.
    #[test]
    fn layers_merge_as_a_model() {
        let n = 3000;
        let src: String = std::iter::once("a,b\n".to_string()).chain((0..n).map(|i| format!("{i},b{i}\n"))).collect();
        let mut t = table(&src);
        let mut model: Vec<Vec<Vec<String>>> = vec![grid(&t)];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |m: usize| { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; (seed % m as u64) as usize };
        let no = AtomicBool::new(false);
        for step in 0..400 {
            let mut cur = model.last().unwrap().clone();
            let col = rnd(2);
            match step % 10 {
                // A whole column computed.
                0 => {
                    let mut c = Cells::default();
                    for p in 0..n { if p % 5 != 0 { c.push(t.st.data_id(p), &format!("c{step}.{p}")); cur[p][col] = format!("c{step}.{p}"); } }
                    t.apply(Patch::column(col, vec![c]));
                }
                // A selection filled (or cleared).
                1 | 2 => {
                    let (a, len) = (rnd(n), rnd(500) + 1);
                    let v = if step % 10 == 1 { String::new() } else { format!("f{step}") };
                    let rows: Vec<RowId> = (a..(a + len).min(n)).map(|p| t.st.data_id(p)).collect();
                    t.apply(Patch::fill(&t.st, &rows, &[col], &v, &no).unwrap());
                    for p in a..(a + len).min(n) { cur[p][col] = v.clone(); }
                }
                // A few cells typed, some twice in one edit.
                _ => {
                    let mut ch = Vec::new();
                    for k in 0..rnd(4) + 1 {
                        let p = rnd(n);
                        ch.push((t.st.data_id(p), col, format!("t{step}.{k}")));
                        cur[p][col] = format!("t{step}.{k}");
                    }
                    t.set_cells(&ch);
                }
            }
            assert!(t.st.cols.iter().all(|l| l.len() <= MAX_LAYERS));
            assert_eq!(grid(&t), cur, "step {step}");
            model.push(cur);
        }
        // The last computed column's layer survives the small edits typed after it, untouched.
        let big = t.st.cols.iter().flat_map(|l| l.iter()).filter(|l| l.len() >= n * 4 / 5 - 1).count();
        assert!(big >= 1);
        for k in (0..model.len() - 1).rev() {
            t.undo();
            if k % 37 == 0 { assert_eq!(grid(&t), model[k], "undo to {k}"); }
        }
        assert_eq!(grid(&t), model[0]);
        assert!(!t.dirty());
    }

    /// A merge of millions of cells: the small layers typed over a big one are merged, not the big one.
    #[test]
    #[ignore]
    fn perf_layer_merge() {
        let n = 5_000_000;
        let src: String = std::iter::once("a\n".to_string()).chain((0..n).map(|i| format!("{i}\n"))).collect();
        let mut t = table(&src);
        let t0 = std::time::Instant::now();
        let mut c = Cells::default();
        for p in 0..n { c.push(t.st.data_id(p), "computed"); }
        let p = Patch::column(0, vec![c]);
        eprintln!("patch of {n} cells: {:?}", t0.elapsed());
        let t0 = std::time::Instant::now();
        t.apply(p);
        eprintln!("apply: {:?}", t0.elapsed());
        let mut worst = std::time::Duration::ZERO;
        for k in 0..40 {
            let t0 = std::time::Instant::now();
            t.set_cells(&[(t.st.data_id(k * 1000), 0, format!("v{k}"))]);
            worst = worst.max(t0.elapsed());
        }
        eprintln!("40 cells typed after it, the slowest: {worst:?}");
        let t0 = std::time::Instant::now();
        let all: Vec<Arc<Layer>> = t.st.cols[0].iter().cloned().collect();
        let _ = merge(&all);
        eprintln!("a merge of every layer ({} cells): {:?}", all.iter().map(|l| l.len()).sum::<usize>(), t0.elapsed());
        let no = AtomicBool::new(false);
        let ids: Vec<RowId> = (0..n).map(|p| t.st.data_id(p)).collect();
        let t0 = std::time::Instant::now();
        let p = Patch::fill(&t.st, &ids, &[0], "", &no).unwrap();
        eprintln!("fill patch over {n} rows: {:?}", t0.elapsed());
        let t0 = std::time::Instant::now();
        let ch: Vec<(RowId, usize, String)> = ids.iter().map(|&id| (id, 0, String::new())).collect();
        let q = Patch::new(&ch);
        eprintln!("the same as {n} changes: {:?}", t0.elapsed());
        drop((p, q));
    }

    /// Rows inserted anywhere, in a file never reordered and in one already reordered, across
    /// the blocks the new order is written in.
    #[test]
    fn rows_inserted_across_blocks() {
        let n = 150_000usize;
        let src: String = std::iter::once("a\n".to_string()).chain((0..n).map(|i| format!("{i}\n"))).collect();
        let mut t = table(&src);
        let mut model: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        for &pos in &[0usize, 65_535, 65_536, 70_000, 131_071, 150_000, 3] {
            let ids = t.insert_rows(pos, 3);
            let at = pos.min(model.len());
            model.splice(at..at, ids);
            assert_eq!((0..t.st.len()).map(|p| t.st.data_id(p)).collect::<Vec<_>>(), model, "at {pos}");
            assert_eq!(t.st.all_id(0), 0, "the titles stay first");
        }
        while t.undo().is_some() {}
        assert_eq!(t.st.len(), n);
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
