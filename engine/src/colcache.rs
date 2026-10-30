//! Column caches: one field of the file read once and kept in the form filters
//! and sorts scan at memory speed — a dictionary (a code per row, each distinct
//! value once) when the column repeats itself, else the folded text of every
//! row end to end; numbers and dates parsed once; text sort ranks.
//!
//! Keyed by (file, field): the file's bytes never change, so a cache never goes
//! stale. Edits stay in the table's layers: users read the rows edited in a
//! column (and rows created here) live, every other row from the cache.
//! A file closed or reloaded drops its caches. Built only while memory allows
//! (`allowed`), on demand (`ensure`) or in the background at low priority (`warm`).

use crate::base::Base;
use crate::table::{RowId, State};
use crate::text::{fold_into, parse_date, parse_number};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};

/// Records per arena chunk (offsets stay u32).
pub const CH: usize = 1 << 16;

/// fold(cell) of every record of the file, end to end.
pub struct Folded {
    chunks: Vec<(Vec<u8>, Vec<u32>)>,
    /// A bit per record: its fold is not the text as read (capitals, accents…).
    changed: Vec<u64>,
}

impl Folded {
    /// Record `id`'s folded text (`id` < the file's records).
    #[inline]
    pub fn get(&self, id: usize) -> &str {
        let (a, o) = &self.chunks[id / CH];
        let k = id % CH;
        // SAFETY: the arena holds whole Strings end to end, cut on their boundaries.
        unsafe { std::str::from_utf8_unchecked(&a[o[k] as usize..o[k + 1] as usize]) }
    }

    /// Whether record `id`'s text is its own fold (plain lower case, no accent).
    #[inline]
    pub fn plain(&self, id: usize) -> bool {
        self.changed[id / 64] >> (id % 64) & 1 == 0
    }

    /// Whether every record's text is its own fold (equal keys are then equal texts).
    pub fn all_plain(&self) -> bool {
        self.changed.par_iter().with_min_len(1 << 12).all(|&w| w == 0)
    }

    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(|(a, o)| a.capacity() + o.capacity() * 4).sum::<usize>() + self.changed.capacity() * 8
    }

    /// The arena chunk holding record `id`: its bytes, offsets, and first record.
    #[inline]
    pub fn chunk(&self, id: usize) -> (&[u8], &[u32], usize) {
        let (a, o) = &self.chunks[id / CH];
        (a, o, id / CH * CH)
    }
}

/// A column that repeats itself: a code per record, each distinct value once.
pub struct Dict {
    pub codes: Vec<u32>,
    /// The text as read (decoded), per code.
    pub values: Vec<Box<str>>,
    /// fold(value), per code.
    pub folded: Vec<Box<str>>,
    /// parse_number / parse_date of each value (NaN: not one).
    pub num: Vec<f64>,
    pub date: Vec<f64>,
    /// Text sort order: dense rank of the value's sort key (`text::sort_key`); empty values rank `nranks`.
    pub rank: Vec<u32>,
    pub nranks: u32,
    /// The same for `num` and `date` (equal numbers share a rank; NaN ranks last).
    pub num_rank: (Vec<u32>, u32),
    pub date_rank: (Vec<u32>, u32),
}

/// Dense ranks of numbers in order, NaN ranked after all.
fn num_ranks(v: &[f64]) -> (Vec<u32>, u32) {
    let mut idx: Vec<u32> = (0..v.len() as u32).filter(|&i| !v[i as usize].is_nan()).collect();
    idx.par_sort_unstable_by(|&a, &b| v[a as usize].partial_cmp(&v[b as usize]).unwrap());
    let mut rank = vec![0u32; v.len()];
    let mut r = 0u32;
    for (k, &i) in idx.iter().enumerate() {
        if k > 0 && v[i as usize] != v[idx[k - 1] as usize] { r += 1; }
        rank[i as usize] = r;
    }
    let distinct = if idx.is_empty() { 0 } else { r + 1 };
    for (i, x) in v.iter().enumerate() {
        if x.is_nan() { rank[i] = distinct; }
    }
    (rank, distinct)
}

impl Dict {
    pub fn bytes(&self) -> usize {
        self.codes.capacity() * 4 + self.values.iter().chain(&self.folded).map(|s| s.len() + 16).sum::<usize>() + self.values.len() * 28
    }
}

/// Text sort rank of every record, as `Dict::rank`.
pub struct Ranks {
    pub rank: Vec<u32>,
    pub nranks: u32,
}

/// What is cached of one column (each part may be missing).
#[derive(Clone, Default)]
pub struct Col {
    pub dict: Option<Arc<Dict>>,
    pub folded: Option<Arc<Folded>>,
    pub ranks: Option<Arc<Ranks>>,
    pub num: Option<Arc<Vec<f64>>>,
    pub date: Option<Arc<Vec<f64>>>,
}

impl Col {
    /// fold(cell) of record `id`, if cached.
    #[inline]
    pub fn folded(&self, id: usize) -> Option<&str> {
        if let Some(d) = &self.dict { return Some(&d.folded[d.codes[id] as usize]); }
        self.folded.as_ref().map(|f| f.get(id))
    }

    /// Whether record `id`'s text is known to be its own fold (false: not known).
    #[inline]
    pub fn plain(&self, id: usize) -> bool {
        if let Some(d) = &self.dict { let c = d.codes[id] as usize; return d.folded[c] == d.values[c]; }
        self.folded.as_ref().is_some_and(|f| f.plain(id))
    }

    /// parse_number(cell) of record `id` (NaN: not a number), if cached.
    #[inline]
    pub fn num(&self, id: usize) -> Option<f64> {
        if let Some(d) = &self.dict { return Some(d.num[d.codes[id] as usize]); }
        self.num.as_ref().map(|v| v[id])
    }

    /// parse_date(cell) of record `id` (NaN: not a date), if cached.
    #[inline]
    pub fn date(&self, id: usize) -> Option<f64> {
        if let Some(d) = &self.dict { return Some(d.date[d.codes[id] as usize]); }
        self.date.as_ref().map(|v| v[id])
    }

    /// Text sort rank of record `id` and the empty values' rank, if cached.
    #[inline]
    pub fn rank(&self, id: usize) -> Option<u32> {
        if let Some(d) = &self.dict { return Some(d.rank[d.codes[id] as usize]); }
        self.ranks.as_ref().map(|r| r.rank[id])
    }

    pub fn nranks(&self) -> Option<u32> {
        self.dict.as_ref().map(|d| d.nranks).or(self.ranks.as_ref().map(|r| r.nranks))
    }

    pub fn has(&self, need: Need) -> bool {
        match need {
            Need::Dict => self.dict.is_some(),
            Need::Text => self.dict.is_some() || self.folded.is_some(),
            Need::Rank => self.dict.is_some() || self.ranks.is_some(),
            Need::Num => self.dict.is_some() || self.num.is_some(),
            Need::Date => self.dict.is_some() || self.date.is_some(),
        }
    }

    fn bytes(&self) -> usize {
        self.dict.as_ref().map_or(0, |d| d.bytes())
            + self.folded.as_ref().map_or(0, |f| f.bytes())
            + self.ranks.as_ref().map_or(0, |r| r.rank.capacity() * 4)
            + self.num.as_ref().map_or(0, |v| v.capacity() * 8)
            + self.date.as_ref().map_or(0, |v| v.capacity() * 8)
    }
}

/// What a user of a column needs cached.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Need {
    /// The values as a dictionary (only if the column repeats itself).
    Dict,
    /// fold(cell) (substring and exact filters).
    Text,
    /// Text sort ranks.
    Rank,
    /// parse_number(cell).
    Num,
    /// parse_date(cell).
    Date,
}

struct Slot {
    base: Weak<Base>,
    field: usize,
    col: Col,
    /// A dictionary was tried and the column does not repeat enough.
    no_dict: bool,
}

static REG: Mutex<Vec<Slot>> = Mutex::new(Vec::new());
/// Bytes held by every cache.
static USED: AtomicUsize = AtomicUsize::new(0);

fn reg() -> std::sync::MutexGuard<'static, Vec<Slot>> {
    let mut r = REG.lock().unwrap_or_else(|e| e.into_inner());
    // Files closed meanwhile: their caches go.
    r.retain(|s| {
        let live = s.base.strong_count() > 0;
        if !live { USED.fetch_sub(s.col.bytes(), Ordering::Relaxed); }
        live
    });
    r
}

/// The field of the file a column reads (None: a column added here).
pub fn field_of(st: &State, col: usize) -> Option<usize> {
    match &st.cmap {
        None => Some(col),
        Some(m) => m.get(col).and_then(|&f| (f >= 0).then_some(f as usize)),
    }
}

/// What is cached of this column now (nothing for a column added here).
pub fn get(st: &State, col: usize) -> Col {
    field_of(st, col).map_or_else(Col::default, |f| get_field(&st.base, f))
}

pub fn get_field(base: &Arc<Base>, field: usize) -> Col {
    reg().iter().find(|s| s.field == field && std::ptr::eq(s.base.as_ptr(), Arc::as_ptr(base))).map(|s| s.col.clone()).unwrap_or_default()
}

/// Memory held by every column cache.
pub fn used_bytes() -> usize {
    drop(reg()); // prunes closed files first
    USED.load(Ordering::Relaxed)
}

fn meminfo() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |k: &str| s.lines().find(|l| l.starts_with(k))?.split_whitespace().nth(1)?.parse::<u64>().ok().map(|v| v * 1024);
    Some((kb("MemTotal:")?, kb("MemAvailable:")?))
}

/// Whether `extra` more bytes of cache are worth holding: caches stay under a
/// third of the machine's memory and never take more than a third of what is free.
pub fn allowed(extra: usize) -> bool {
    if std::env::var_os("CSVFAB_COLCACHE").is_some_and(|v| v == "0") {
        return false;
    }
    let used = USED.load(Ordering::Relaxed) as u64;
    match meminfo() {
        Some((total, avail)) => used + extra as u64 <= total / 3 && (extra as u64) <= avail / 3,
        None => used + (extra as u64) < 1 << 30,
    }
}

fn update(base: &Arc<Base>, field: usize, f: impl FnOnce(&mut Slot)) {
    // Its thread frees closed files' caches.
    drop(worker());
    let mut r = reg();
    let k = match r.iter().position(|s| s.field == field && std::ptr::eq(s.base.as_ptr(), Arc::as_ptr(base))) {
        Some(k) => k,
        None => {
            r.push(Slot { base: Arc::downgrade(base), field, col: Col::default(), no_dict: false });
            r.len() - 1
        }
    };
    let before = r[k].col.bytes();
    f(&mut r[k]);
    let after = r[k].col.bytes();
    USED.fetch_add(after, Ordering::Relaxed);
    USED.fetch_sub(before, Ordering::Relaxed);
}

/// After a save that left every row in its place (record k of `new` is row k of `old`; no
/// `order`), the caches of the columns no edit touched serve the new file as they are: field
/// `field_of(old, c)` of the old file holds what field `c` of the new one does.
pub fn carry(old: &State, new: &Arc<Base>) {
    if old.order.is_some() || new.n != old.base.n { return; }
    let to: Vec<(usize, usize)> = (0..old.ncols).filter(|&c| old.cols.get(c).is_some_and(|l| l.is_empty())).filter_map(|c| field_of(old, c).map(|f| (f, c))).collect();
    let mut r = reg();
    let carried: Vec<Slot> = r.iter()
        .filter(|s| std::ptr::eq(s.base.as_ptr(), Arc::as_ptr(&old.base)))
        .filter_map(|s| to.iter().find(|t| t.0 == s.field).map(|&(_, c)| Slot { base: Arc::downgrade(new), field: c, col: s.col.clone(), no_dict: s.no_dict }))
        .collect();
    for s in carried {
        USED.fetch_add(s.col.bytes(), Ordering::Relaxed);
        r.push(s);
    }
}

/// Drops every cache (tests, benchmarks).
pub fn clear() {
    let mut r = reg();
    for s in r.iter() { USED.fetch_sub(s.col.bytes(), Ordering::Relaxed); }
    r.clear();
}

/// Runs `f` on a record's field as read, decoded ("" when the record has no such field) —
/// what `State::with_cell` gives for a row nobody edited.
#[inline]
pub fn with_base_cell<R>(b: &Base, id: usize, field: usize, tmp: &mut Vec<u8>, f: impl FnOnce(&str) -> R) -> R {
    if id >= b.n {
        return f("");
    }
    let rec = b.record(id);
    if b.dialect.lines {
        return if field == 0 { f(&b.text(rec)) } else { f("") };
    }
    let mut f = Some(f);
    match crate::scan::with_field(rec, b.dialect.delim, field, tmp, |v| (f.take().unwrap())(&b.text(v))) {
        Some(r) => r,
        None => (f.take().unwrap())(""),
    }
}

/// The raw bytes of a record's field (unescaped), as `with_base_cell` reads them.
#[inline]
fn with_raw<R>(b: &Base, id: usize, field: usize, tmp: &mut Vec<u8>, f: impl FnOnce(&[u8]) -> R) -> R {
    let rec = b.record(id);
    if b.dialect.lines {
        return if field == 0 { f(rec) } else { f(b"") };
    }
    let mut f = Some(f);
    match crate::scan::with_field(rec, b.dialect.delim, field, tmp, |v| (f.take().unwrap())(v)) {
        Some(r) => r,
        None => (f.take().unwrap())(b""),
    }
}

/// Average bytes of this field, from a sample of records.
fn avg_len(b: &Base, field: usize) -> usize {
    let n = b.n;
    let k = n.min(512);
    if k == 0 { return 0; }
    let mut tmp = Vec::new();
    let total: usize = (0..k).map(|j| with_raw(b, j * n / k, field, &mut tmp, |v| v.len())).sum();
    total / k + 1
}

/// Makes sure `need` is cached for this column, building it now (on the
/// caller's rayon pool) if memory allows. None: not cached (no room, cancelled,
/// or a column added here).
pub fn ensure(st: &State, col: usize, need: Need, cancel: &AtomicBool) -> Option<Col> {
    let field = field_of(st, col)?;
    ensure_field(&st.base, field, need, cancel)
}

/// As `ensure`, by field of the file. Never built twice at once: a build the background
/// worker has not started yet is taken over (here, on the caller's pool, all its cores);
/// one it is running is waited for (the caller cancelled: None at once).
pub fn ensure_field(base: &Arc<Base>, field: usize, need: Need, cancel: &AtomicBool) -> Option<Col> {
    let key = (Arc::as_ptr(base) as usize, field, need);
    let running = {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        match p.iter().position(|(k, _)| *k == key) {
            // Queued, not started: ours now (the worker skips what is no longer queued).
            Some(i) if !p[i].1 => { p.remove(i); false }
            Some(_) => true,
            None => false,
        }
    };
    if running {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        while p.iter().any(|(k, _)| *k == key) {
            if cancel.load(Ordering::Relaxed) { return None; }
            p = DONE.wait_timeout(p, std::time::Duration::from_millis(20)).unwrap_or_else(|e| e.into_inner()).0;
        }
        drop(p);
        let col = get_field(base, field);
        if col.has(need) { return Some(col); }
    }
    build(base, field, need, cancel)
}

fn build(base: &Arc<Base>, field: usize, need: Need, cancel: &AtomicBool) -> Option<Col> {
    let (col, no_dict) = {
        let r = reg();
        r.iter()
            .find(|s| s.field == field && std::ptr::eq(s.base.as_ptr(), Arc::as_ptr(base)))
            .map(|s| (s.col.clone(), s.no_dict))
            .unwrap_or_default()
    };
    if col.has(need) {
        return Some(col);
    }
    let b = &**base;
    let n = b.n;
    if !no_dict && col.dict.is_none() {
        if allowed(n * 4 + n / 4 * avg_len(b, field).min(64)) {
            // Numbers and dates are parsed in one pass of their own: a dictionary is tried for
            // them only when the sample says it will pay (a column of amounts, half distinct,
            // got through the sample and failed after building most of one).
            let sure = matches!(need, Need::Num | Need::Date);
            match build_dict(b, field, sure, cancel) {
                Ok(d) => {
                    update(base, field, |s| s.col.dict = Some(Arc::new(d)));
                    return Some(get_field(base, field));
                }
                Err(_) if cancel.load(Ordering::Relaxed) => return None,
                Err(true) => update(base, field, |s| s.no_dict = true),
                Err(false) => {}
            }
        } else {
            return None;
        }
    }
    if need == Need::Dict {
        return None;
    }
    let needs_text = matches!(need, Need::Text | Need::Rank) && col.folded.is_none();
    let extra = match need {
        Need::Text => n * (avg_len(b, field) + 4),
        Need::Rank => (if needs_text { n * (avg_len(b, field) + 4) } else { 0 }) + n * 4 + n * 28,
        Need::Num | Need::Date => n * 8,
        Need::Dict => 0,
    };
    if !allowed(extra) {
        return None;
    }
    match need {
        Need::Text | Need::Rank => {
            let folded = match &col.folded {
                Some(f) => f.clone(),
                None => {
                    let f = Arc::new(build_folded(b, field, cancel)?);
                    update(base, field, |s| s.col.folded = Some(f.clone()));
                    f
                }
            };
            if need == Need::Rank {
                // The sort compares keys many times: made once (`text::sort_key_into`), end to end.
                let keys = crate::radix::Keys::build(n, |i, out| crate::text::sort_key_into(folded.get(i).trim().as_bytes(), out));
                // Equal keys ("Émile", "emile") told apart by the cells as written, read again.
                // A run of plain texts (most e-mails) is equal as written: not read; a column of
                // nothing but plain texts has no ties to tell apart.
                let (_, rank, nranks) = if folded.all_plain() {
                    crate::radix::text_order(n, |i| keys.get(i))
                } else {
                    crate::radix::text_order_ties(n, |i| keys.get(i), |i| folded.plain(i), |i, out| {
                        if folded.plain(i) { return crate::text::tie_key_into(folded.get(i).trim(), out); }
                        TMP.with_borrow_mut(|tmp| with_base_cell(b, i, field, tmp, |v| crate::text::tie_key_into(v.trim(), out)))
                    })
                };
                drop(keys);
                // Kept even if the sort that asked was cancelled meanwhile: the next one uses it.
                update(base, field, |s| s.col.ranks = Some(Arc::new(Ranks { rank, nranks })));
            }
        }
        Need::Num | Need::Date => {
            let parse = if need == Need::Num { parse_number } else { parse_date };
            let v = build_nums(b, field, parse, cancel)?;
            update(base, field, |s| if need == Need::Num { s.col.num = Some(Arc::new(v)) } else { s.col.date = Some(Arc::new(v)) });
        }
        Need::Dict => {}
    }
    Some(get_field(base, field))
}

thread_local! {
    /// A quoted field's unquoted bytes (`with_base_cell`), per thread.
    static TMP: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn chunks(n: usize) -> impl IndexedParallelIterator<Item = std::ops::Range<usize>> {
    (0..n.div_ceil(CH)).into_par_iter().map(move |c| c * CH..((c + 1) * CH).min(n))
}

fn build_folded(b: &Base, field: usize, cancel: &AtomicBool) -> Option<Folded> {
    let parts: Vec<((Vec<u8>, Vec<u32>), Vec<u64>)> = chunks(b.n)
        .map(|r| {
            let mut arena = String::new();
            let mut offs = Vec::with_capacity(r.len() + 1);
            offs.push(0u32);
            let mut changed = vec![0u64; r.len().div_ceil(64)];
            let mut tmp = Vec::new();
            let first = r.start;
            for id in r {
                if cancel.load(Ordering::Relaxed) { break; }
                with_base_cell(b, id, field, &mut tmp, |v| {
                    let at = arena.len();
                    fold_into(v, &mut arena);
                    if arena.as_bytes()[at..] != *v.as_bytes() { changed[(id - first) / 64] |= 1 << ((id - first) % 64); }
                });
                offs.push(arena.len() as u32);
            }
            arena.shrink_to_fit();
            ((arena.into_bytes(), offs), changed)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    // CH is a multiple of 64: the chunks' bits end to end.
    let (chunks, bits): (Vec<_>, Vec<_>) = parts.into_iter().unzip();
    Some(Folded { chunks, changed: bits.concat() })
}

fn build_nums(b: &Base, field: usize, parse: fn(&str) -> Option<f64>, cancel: &AtomicBool) -> Option<Vec<f64>> {
    let v: Vec<f64> = (0..b.n)
        .into_par_iter()
        .with_min_len(1 << 14)
        .map_init(Vec::new, |tmp, id| {
            if cancel.load(Ordering::Relaxed) { return f64::NAN; }
            with_base_cell(b, id, field, tmp, |s| parse(s).unwrap_or(f64::NAN))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(v) }
}

type Map = hashbrown::HashMap<Box<[u8]>, u32, foldhash::fast::FixedState>;

/// A dictionary of the field. Err(true): it holds too many distinct values to pay (more
/// than one row in 8); Err(false): cancelled, or `sure` and the sample left it in doubt
/// (more than three quarters of it distinct): not tried.
fn build_dict(b: &Base, field: usize, sure: bool, cancel: &AtomicBool) -> Result<Dict, bool> {
    let n = b.n;
    let cap = (n / 8).max(64);
    // A sample first: a column of mostly distinct values is told in a few thousand rows.
    if n > 1 << 16 {
        let k = 1 << 13;
        let mut seen = Map::default();
        let mut tmp = Vec::new();
        for j in 0..k {
            with_raw(b, j * (n / k), field, &mut tmp, |v| if !seen.contains_key(v) { seen.insert(v.into(), 0); });
        }
        // Nearly all distinct: tens of thousands of values or more.
        if seen.len() > k * 9 / 10 { return Err(true); }
        if sure && seen.len() > k * 3 / 4 { return Err(false); }
    }
    let abort = AtomicBool::new(false);
    let parts: Vec<(Vec<u32>, Vec<Box<[u8]>>)> = chunks(n)
        .map(|r| {
            let mut map = Map::default();
            let mut vals: Vec<Box<[u8]>> = Vec::new();
            let mut codes = Vec::with_capacity(r.len());
            let mut tmp = Vec::new();
            // Past half a chunk distinct, the column hardly repeats: the others stop too.
            let limit = (r.len() / 2).max(64);
            for id in r {
                if cancel.load(Ordering::Relaxed) || abort.load(Ordering::Relaxed) { break; }
                with_raw(b, id, field, &mut tmp, |v| {
                    let c = match map.get(v) {
                        Some(&c) => c,
                        None => {
                            let c = vals.len() as u32;
                            vals.push(v.into());
                            map.insert(v.into(), c);
                            c
                        }
                    };
                    codes.push(c);
                });
                if vals.len() > limit { abort.store(true, Ordering::Relaxed); }
            }
            (codes, vals)
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return Err(false); }
    if abort.load(Ordering::Relaxed) { return Err(true); }
    // One code space: each chunk's codes renumbered.
    let mut map = Map::default();
    let mut raw: Vec<Box<[u8]>> = Vec::new();
    let remaps: Vec<Vec<u32>> = parts
        .iter()
        .map(|(_, vals)| {
            vals.iter()
                .map(|v| {
                    *map.entry(v.clone()).or_insert_with(|| {
                        raw.push(v.clone());
                        raw.len() as u32 - 1
                    })
                })
                .collect()
        })
        .collect();
    if raw.len() > cap { return Err(true); }
    drop(map);
    let mut codes = Vec::with_capacity(n);
    for ((c, _), m) in parts.iter().zip(&remaps) {
        codes.extend(c.iter().map(|&k| m[k as usize]));
    }
    let values: Vec<Box<str>> = raw.par_iter().map(|v| b.text(v).into()).collect();
    let folded: Vec<Box<str>> = values.par_iter().map(|v| crate::text::fold(v).into()).collect();
    let num: Vec<f64> = values.par_iter().map(|v| parse_number(v).unwrap_or(f64::NAN)).collect();
    let date: Vec<f64> = values.par_iter().map(|v| parse_date(v).unwrap_or(f64::NAN)).collect();
    let keys = crate::radix::Keys::build(values.len(), |i, out| crate::text::sort_key_into(folded[i].trim().as_bytes(), out));
    let (_, rank, nranks) = crate::radix::text_order_ties(values.len(), |i| keys.get(i), |_| false, |i, out| crate::text::tie_key_into(values[i].trim(), out));
    let (num_rank, date_rank) = (num_ranks(&num), num_ranks(&date));
    Ok(Dict { codes, values, folded, num, date, rank, nranks, num_rank, date_rank })
}

type Job = (Arc<Base>, usize, Need);

/// Background builds asked for and not done yet: ((file, field, need), started).
static PENDING: Mutex<Vec<((usize, usize, Need), bool)>> = Mutex::new(Vec::new());
/// Signalled when a background build ends (`ensure_field` waits on it).
static DONE: std::sync::Condvar = std::sync::Condvar::new();

/// Builds `need` for this column in the background (half the cores, low
/// priority), if memory allows; users pick it up once there.
pub fn warm(st: &State, col: usize, need: Need) {
    let Some(field) = field_of(st, col) else { return };
    // Small files are read fast enough without; a file's first rows (the rest
    // being read) are about to be replaced.
    if st.base.n < 1 << 14 || st.base.partial || get_field(&st.base, field).has(need) {
        return;
    }
    let key = (Arc::as_ptr(&st.base) as usize, field, need);
    {
        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if p.iter().any(|(k, _)| *k == key) { return; }
        p.push((key, false));
    }
    let _ = worker().send(Some((st.base.clone(), field, need)));
}

/// The background builder: one thread taking the builds in turn, with a pool
/// of half the cores at low priority; between builds it lets closed files' caches go.
fn worker() -> mpsc::Sender<Option<Job>> {
    static TX: Mutex<Option<mpsc::Sender<Option<Job>>>> = Mutex::new(None);
    let mut tx = TX.lock().unwrap_or_else(|e| e.into_inner());
    tx.get_or_insert_with(|| {
        let (tx, rx) = mpsc::channel::<Option<Job>>();
        std::thread::Builder::new()
            .name("colcache".into())
            .spawn(move || {
                let threads = (std::thread::available_parallelism().map_or(2, |n| n.get()) / 2).max(1);
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .thread_name(|i| format!("colcache-{i}"))
                    .start_handler(|_| low_priority())
                    .build()
                    .ok();
                low_priority();
                let no = AtomicBool::new(false);
                loop {
                    let job = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
                        Ok(job) => job,
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    };
                    drop(reg()); // closed files' caches freed
                    let Some((base, field, need)) = job else { continue };
                    let key = (Arc::as_ptr(&base) as usize, field, need);
                    // Taken over by a user meanwhile (`ensure_field`): not ours any more.
                    let ours = {
                        let mut p = PENDING.lock().unwrap_or_else(|e| e.into_inner());
                        p.iter_mut().find(|(k, _)| *k == key).map(|e| e.1 = true).is_some()
                    };
                    // Last user gone (file closed): nothing to warm.
                    if ours && Arc::strong_count(&base) > 1 {
                        match &pool {
                            Some(p) => { p.install(|| build(&base, field, need, &no)); }
                            None => { build(&base, field, need, &no); }
                        }
                    }
                    PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(k, _)| *k != key);
                    DONE.notify_all();
                }
            })
            .expect("colcache thread");
        tx
    })
    .clone()
}

fn low_priority() {
    // SAFETY: plain syscall on the calling thread.
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}

/// Rows of this column whose cell is not the file's: edited here, or created here.
/// None: none edited (rows ≥ the file's records are always live).
pub fn edited_rows(st: &State, col: usize) -> Option<Bits> {
    let layers = st.cols.get(col)?;
    if layers.is_empty() { return None; }
    let mut b = Bits::default();
    for l in layers.iter() {
        for &id in l.ids() { b.set(id); }
    }
    Some(b)
}

/// A set of row ids.
#[derive(Default, Clone)]
pub struct Bits(Vec<u64>);

impl Bits {
    pub fn set(&mut self, id: RowId) {
        let w = id as usize / 64;
        if w >= self.0.len() { self.0.resize(w + 1, 0); }
        self.0[w] |= 1 << (id % 64);
    }

    pub fn or(&mut self, o: &Bits) {
        if o.0.len() > self.0.len() { self.0.resize(o.0.len(), 0); }
        for (a, b) in self.0.iter_mut().zip(&o.0) { *a |= b; }
    }

    #[inline]
    pub fn has(&self, id: RowId) -> bool {
        self.0.get(id as usize / 64).is_some_and(|w| w >> (id % 64) & 1 == 1)
    }
}

/// How often each value of a dictionary column occurs over some rows.
pub struct DictCounts {
    pub dict: Arc<Dict>,
    /// Per code: rows holding it, and the first position (in the rows given) that does (u32::MAX: none).
    pub count: Vec<u32>,
    pub first: Vec<u32>,
    /// Positions of the rows read live instead (edited here, created here): count them by their text.
    pub live: Vec<u32>,
}

/// Value counts of `ids` from the column's dictionary — no hashing, no cell
/// read (value counts, charts, group-by keys). None: the column has no
/// dictionary (yet), or cancelled.
pub fn dict_counts(st: &State, col: usize, ids: &[RowId], cancel: &AtomicBool) -> Option<DictCounts> {
    let dict = get(st, col).dict?;
    let edited = edited_rows(st, col);
    let n = st.base.n;
    let k = dict.values.len();
    let count_part = |range: std::ops::Range<usize>| {
        let (mut count, mut first, mut live) = (vec![0u32; k], vec![u32::MAX; k], Vec::new());
        for p in range {
            let id = ids[p];
            if id as usize >= n || edited.as_ref().is_some_and(|e| e.has(id)) {
                live.push(p as u32);
                continue;
            }
            let c = dict.codes[id as usize] as usize;
            if count[c] == 0 { first[c] = p as u32; }
            count[c] += 1;
        }
        (count, first, live)
    };
    // A small dictionary: counted in parallel, the parts added up.
    let parts: Vec<(Vec<u32>, Vec<u32>, Vec<u32>)> = if k <= 1 << 16 {
        let size = (ids.len() / (rayon::current_num_threads() * 2)).max(1 << 16);
        (0..ids.len().div_ceil(size)).into_par_iter().map(|c| count_part(c * size..((c + 1) * size).min(ids.len()))).collect()
    } else {
        vec![count_part(0..ids.len())]
    };
    if cancel.load(Ordering::Relaxed) { return None; }
    let (mut count, mut first, mut live) = (vec![0u32; k], vec![u32::MAX; k], Vec::new());
    for (c, f, l) in parts {
        for j in 0..k {
            count[j] += c[j];
            first[j] = first[j].min(f[j]);
        }
        live.extend(l);
    }
    Some(DictCounts { dict, count, first, live })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::OpenOptions;
    use crate::table::Table;

    #[test]
    fn dict_counts_match_cells() {
        let mut csv = String::from("a,b\n");
        for i in 0..5000 { csv.push_str(&format!("{},{}\n", ["x", "y", " x", ""][i % 4], i)); }
        let mut t = Table::new(Arc::new(Base::from_bytes(csv.into_bytes(), OpenOptions::default())), None);
        t.set_cells(&[(3, 0, "y".into())]);
        let no = AtomicBool::new(false);
        let col = ensure(&t.st, 0, Need::Dict, &no).unwrap();
        assert_eq!(col.dict.as_ref().unwrap().values.len(), 5); // the header record's too
        assert!(ensure(&t.st, 1, Need::Dict, &no).is_none()); // all distinct: no dictionary
        let ids: Vec<RowId> = (0..t.st.len()).rev().map(|p| t.st.data_id(p)).collect();
        let c = dict_counts(&t.st, 0, &ids, &no).unwrap();
        let mut got: std::collections::HashMap<String, u32> = Default::default();
        for (j, v) in c.dict.values.iter().enumerate() { if c.count[j] > 0 { *got.entry(v.to_string()).or_default() += c.count[j]; } }
        for &p in &c.live { *got.entry(t.st.cell(ids[p as usize], 0)).or_default() += 1; }
        let mut want: std::collections::HashMap<String, u32> = Default::default();
        for &id in &ids { *want.entry(t.st.cell(id, 0)).or_default() += 1; }
        assert_eq!(got, want);
        let j = c.dict.values.iter().position(|v| &**v == "x").unwrap();
        assert_eq!(t.st.cell(ids[c.first[j] as usize], 0), "x");
        assert!(ids[..c.first[j] as usize].iter().all(|&id| t.st.cell(id, 0) != "x" || c.live.iter().any(|&p| ids[p as usize] == id)));
    }

    /// A save in place keeps the rows where they were: the caches of the columns no edit
    /// touched serve the saved file (a column moved: under its new place), the edited
    /// column's are not carried; what they give is what the saved file holds.
    #[test]
    fn caches_carried_over_a_save() {
        let dir = std::env::temp_dir().join(format!("csvfab-carry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        let mut csv = String::from("city,n,note\n");
        for i in 0..3000 { csv.push_str(&format!("{},{},\"x, {}\"\n", ["Lyon", "Paris", "Nîmes"][i % 3], i % 17, i)); }
        std::fs::write(&p, &csv).unwrap();
        let base = Arc::new(Base::open(&p, OpenOptions::default(), None).unwrap());
        let mut t = Table::new(base, None);
        let no = AtomicBool::new(false);
        for c in 0..3 {
            ensure(&t.st, c, Need::Text, &no).unwrap();
            ensure(&t.st, c, Need::Num, &no).unwrap();
        }
        t.set_cells(&[(5, 1, "edited".into())]);
        t.move_column(2, 0);
        let (_, b) = crate::save::save_indexed(&t.st, &p, &crate::save::SaveOptions { backup: false, ..Default::default() }, t.st.base.stamp.map(Into::into), &mut false).unwrap();
        let new = Arc::new(b.unwrap());
        carry(&t.st, &new);
        let fresh = Table::new(new.clone(), None);
        // Columns now: note (moved, carried), city (carried), n (edited: not carried).
        assert!(get_field(&new, 2).folded.is_none() && get_field(&new, 2).dict.is_none());
        for c in [0, 1] {
            let col = get_field(&new, c);
            assert!(col.has(Need::Text) && col.has(Need::Num), "column {c}");
            for id in 0..new.n {
                assert_eq!(col.folded(id), Some(crate::text::fold(&fresh.st.cell(id as RowId, c)).as_str()), "{c} {id}");
            }
        }
        drop((t, fresh, new));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
