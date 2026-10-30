//! Column statistics: a column's values with their counts and its profile (the
//! column panel), the profile of every column (the file profile), and the
//! totals row — exact, over every row given, in parallel and cancellable.
//!
//! A cell reads as the JS read it (cellType: a number or a date starts with a
//! digit, a sign, a decimal mark or a currency) through `text::parse_number`
//! and `text::parse_date`. Distinct values are counted in hash tables whose
//! strings live end to end in one arena: a value costs its bytes once, never
//! an allocation of its own, and a cell already counted costs a lookup.

use crate::table::{RowId, State};
use crate::text::{fold_into, parse_date, parse_number};
use crate::view::Kind;
use hashbrown::HashTable;
use std::sync::Arc;
use rayon::prelude::*;
use std::cmp::Ordering as Ord;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Reading a cell

/// What a cell holds (None: empty or blank). Text unless it starts like a
/// number or a date — the cheap test first: most text cells never reach the
/// parsers (which allocate for anything but a plain number).
#[inline]
pub fn cell_kind(v: &str) -> Option<Kind> {
    let s = v.trim();
    let c = s.chars().next()?;
    if !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ',' | '€' | '$' | '£')) {
        return Some(Kind::Text);
    }
    if parse_date(s).is_some() {
        Some(Kind::Date)
    } else if parse_number(s).is_some() {
        Some(Kind::Number)
    } else {
        Some(Kind::Text)
    }
}

/// `cell_kind` with what it parsed: the number, the date's key (NaN for text).
#[inline]
fn cell_read(v: &str) -> Option<(Kind, f64)> {
    let s = v.trim();
    let c = s.chars().next()?;
    if !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ',' | '€' | '$' | '£')) {
        return Some((Kind::Text, f64::NAN));
    }
    if let Some(d) = parse_date(s) { return Some((Kind::Date, d)); }
    Some(parse_number(s).map_or((Kind::Text, f64::NAN), |x| (Kind::Number, x)))
}

/// A cell's number, when it reads as one (not a date, not text): `cell_kind`'s Number,
/// parsed once.
#[inline]
pub fn num_cell(v: &str) -> Option<f64> {
    let s = v.trim();
    let c = s.chars().next()?;
    if !(c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | ',' | '€' | '$' | '£')) || parse_date(s).is_some() {
        return None;
    }
    parse_number(s).filter(|x| x.is_finite())
}

/// The key a value sorts and bins on: its number, or its date as YYYYMMDDhhmmss.
#[inline]
pub fn key_of(kind: Kind, s: &str) -> Option<f64> {
    match kind {
        Kind::Number => parse_number(s),
        Kind::Date => parse_date(s),
        Kind::Text => None,
    }
}

/// Positions 0..n in chunks, one accumulator each, in parallel; the
/// accumulators come back in order, for merges that keep "the first one".
/// None if cancelled.
pub(crate) fn chunked<A: Send>(n: usize, cancel: &AtomicBool, init: impl Fn() -> A + Sync + Send, step: impl Fn(&mut A, usize) + Sync + Send) -> Option<Vec<A>> {
    let size = (n / (rayon::current_num_threads() * 4)).max(8192);
    let out: Vec<A> = (0..n.div_ceil(size).max(1))
        .into_par_iter()
        .map(|c| {
            let mut a = init();
            for p in c * size..((c + 1) * size).min(n) {
                if p & 1023 == 0 && cancel.load(Ordering::Relaxed) {
                    break;
                }
                step(&mut a, p);
            }
            a
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

// ---------------------------------------------------------------------------
// Counting distinct values

/// A fast string hash (wyhash-style folded multiplies, 16 bytes a step): the
/// tables compare the bytes anyway, the hash only has to spread them.
#[inline]
pub(crate) fn hash(b: &[u8]) -> u64 {
    const P0: u64 = 0xa076_1d64_78bd_642f;
    const P1: u64 = 0xe703_7ed1_a0b4_28db;
    #[inline]
    fn mix(a: u64, b: u64) -> u64 {
        let r = (a as u128).wrapping_mul(b as u128);
        (r as u64) ^ ((r >> 64) as u64)
    }
    let word = |x: &[u8]| u64::from_le_bytes(x.try_into().unwrap());
    let mut h = (b.len() as u64) ^ P0;
    let (blocks, r) = b.as_chunks::<16>();
    for x in blocks {
        h = mix(word(&x[..8]) ^ P1, word(&x[8..]) ^ h);
    }
    if !r.is_empty() {
        let mut buf = [0u8; 16];
        buf[..r.len()].copy_from_slice(r);
        h = mix(word(&buf[..8]) ^ P1, word(&buf[8..]) ^ h);
    }
    mix(h ^ P1, P0)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ent {
    pub hash: u64,
    pub off: u32,
    pub len: u32,
    pub count: u32,
    /// Where the value was first met (a position in the rows read).
    pub first: u32,
}

/// Values and their counts, in order of first insertion.
#[derive(Default)]
pub(crate) struct Counter {
    arena: Vec<u8>,
    pub ents: Vec<Ent>,
    table: HashTable<u32>,
}

impl Counter {
    #[inline]
    pub fn bytes(&self, e: &Ent) -> &[u8] {
        &self.arena[e.off as usize..e.off as usize + e.len as usize]
    }

    #[inline]
    pub fn str(&self, e: &Ent) -> &str {
        // SAFETY: the arena only ever receives whole `&str`s.
        unsafe { std::str::from_utf8_unchecked(self.bytes(e)) }
    }

    #[inline]
    pub fn find(&self, h: u64, s: &[u8]) -> Option<usize> {
        let (arena, ents) = (&self.arena, &self.ents);
        self.table
            .find(h, |&i| {
                let e = &ents[i as usize];
                e.hash == h && &arena[e.off as usize..e.off as usize + e.len as usize] == s
            })
            .map(|&i| i as usize)
    }

    /// Adds `n` to the value; a new one is kept unless `cap` values are
    /// held already (false then).
    #[inline]
    pub fn add(&mut self, s: &[u8], h: u64, n: u32, first: u32, cap: usize) -> bool {
        if let Some(i) = self.find(h, s) {
            let e = &mut self.ents[i];
            e.count += n;
            e.first = e.first.min(first);
            return true;
        }
        if self.ents.len() >= cap {
            return false;
        }
        let i = self.ents.len() as u32;
        self.ents.push(Ent { hash: h, off: self.arena.len() as u32, len: s.len() as u32, count: n, first });
        self.arena.extend_from_slice(s);
        let ents = &self.ents;
        self.table.insert_unique(h, i, |&j| ents[j as usize].hash);
        true
    }

    /// Another counter's values added to this one's.
    pub fn absorb(&mut self, o: &Counter) {
        for e in &o.ents {
            self.add(o.bytes(e), e.hash, e.count, e.first, usize::MAX);
        }
    }
}

/// Counters split by hash: merging the chunks' counts is then one parallel
/// job per shard instead of one long serial merge (a column of e-mails has as
/// many values as rows).
const NSH: usize = 64;

pub(crate) struct Sharded(Vec<Counter>);

impl Sharded {
    pub fn new() -> Sharded {
        Sharded((0..NSH).map(|_| Counter::default()).collect())
    }

    #[inline]
    pub fn add(&mut self, s: &str, first: u32) {
        self.add_n(s, 1, first);
    }

    /// `n` rows holding `s`, the first at position `first`.
    #[inline]
    pub fn add_n(&mut self, s: &str, n: u32, first: u32) {
        let h = hash(s.as_bytes());
        // Bits the tables use neither for their buckets (low) nor their tags (top 7).
        self.0[(h >> 40) as usize & (NSH - 1)].add(s.as_bytes(), h, n, first, usize::MAX);
    }

    /// The chunks' counts as one, shard by shard in parallel.
    pub fn merge(parts: Vec<Sharded>) -> Vec<Counter> {
        let mut by: Vec<Vec<Counter>> = (0..NSH).map(|_| Vec::with_capacity(parts.len())).collect();
        for p in parts {
            for (s, c) in p.0.into_iter().enumerate() {
                by[s].push(c);
            }
        }
        by.into_par_iter()
            .map(|mut cs| {
                // The biggest one absorbs the others: the fewest strings copied.
                let Some(big) = (0..cs.len()).max_by_key(|&i| cs[i].ents.len()) else { return Counter::default() };
                let mut base = cs.swap_remove(big);
                for c in &cs {
                    base.absorb(c);
                }
                base
            })
            .collect()
    }
}

/// A column's distinct values and how many rows hold each, in order of first
/// appearance among the rows counted.
pub struct ValueCounts {
    shards: Vec<Counter>,
    /// (shard, index) in order of first appearance.
    order: Vec<(u32, u32)>,
    /// Rows counted.
    pub rows: usize,
}

impl ValueCounts {
    pub(crate) fn from_shards(shards: Vec<Counter>, rows: usize) -> ValueCounts {
        let mut order: Vec<(u32, u32, u32)> = shards
            .iter()
            .enumerate()
            .flat_map(|(s, c)| c.ents.iter().enumerate().map(move |(i, e)| (e.first, s as u32, i as u32)))
            .collect();
        order.par_sort_unstable_by_key(|x| x.0);
        ValueCounts { order: order.into_iter().map(|(_, s, i)| (s, i)).collect(), shards, rows }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    #[inline]
    fn ent(&self, i: usize) -> (&Counter, &Ent) {
        let (s, k) = self.order[i];
        let c = &self.shards[s as usize];
        (c, &c.ents[k as usize])
    }

    /// Value i (in order of first appearance).
    #[inline]
    pub fn value(&self, i: usize) -> &str {
        let (c, e) = self.ent(i);
        c.str(e)
    }

    #[inline]
    pub fn count(&self, i: usize) -> u32 {
        self.ent(i).1.count
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, u32)> + '_ {
        (0..self.len()).map(|i| (self.value(i), self.count(i)))
    }

    /// How many rows hold exactly `v`.
    pub fn count_of(&self, v: &str) -> u32 {
        let h = hash(v.as_bytes());
        let c = &self.shards[(h >> 40) as usize & (NSH - 1)];
        c.find(h, v.as_bytes()).map_or(0, |i| c.ents[i].count)
    }

    /// The values by count, most frequent first; ties in collation order —
    /// past 20 000 values (an ID column, every count 1) in order of first
    /// appearance, as the column panel lists them.
    pub fn by_count(&self) -> Vec<u32> {
        let mut ix: Vec<u32> = (0..self.len() as u32).collect();
        if self.len() > 20_000 {
            ix.par_sort_by(|&a, &b| self.count(b as usize).cmp(&self.count(a as usize)));
        } else {
            let keys = self.collation_keys();
            ix.par_sort_by(|&a, &b| {
                self.count(b as usize).cmp(&self.count(a as usize)).then_with(|| natural_cmp(keys.get(a as usize), keys.get(b as usize)))
            });
        }
        ix
    }

    /// The values in collation order (accents and case aside, digits as numbers).
    pub fn by_value(&self) -> Vec<u32> {
        let keys = self.collation_keys();
        let mut ix: Vec<u32> = (0..self.len() as u32).collect();
        ix.par_sort_by(|&a, &b| natural_cmp(keys.get(a as usize), keys.get(b as usize)));
        ix
    }

    /// Every value folded once, end to end: the sort compares without folding.
    fn collation_keys(&self) -> Arena {
        let parts: Vec<Arena> = (0..self.len())
            .into_par_iter()
            .with_min_len(4096)
            .fold(Arena::default, |mut a, i| {
                a.push_folded(self.value(i));
                a
            })
            .collect();
        Arena::concat(parts)
    }

    /// Among `ix`, the values containing `query` (accents and case aside).
    pub fn search(&self, ix: &[u32], query: &str) -> Vec<u32> {
        let q = crate::text::fold(query.trim());
        if q.is_empty() {
            return ix.to_vec();
        }
        let finder = memchr::memmem::Finder::new(q.as_bytes());
        ix.par_iter()
            .with_min_len(4096)
            .map_init(String::new, |buf, &i| {
                buf.clear();
                fold_into(self.value(i as usize), buf);
                finder.find(buf.as_bytes()).map(|_| i)
            })
            .flatten()
            .collect()
    }
}

/// Strings end to end with their ends.
#[derive(Default)]
struct Arena {
    text: String,
    ends: Vec<usize>,
}

impl Arena {
    fn push_folded(&mut self, s: &str) {
        fold_into(s, &mut self.text);
        self.ends.push(self.text.len());
    }

    fn get(&self, i: usize) -> &[u8] {
        &self.text.as_bytes()[if i == 0 { 0 } else { self.ends[i - 1] }..self.ends[i]]
    }

    fn concat(parts: Vec<Arena>) -> Arena {
        let mut out = Arena { text: String::with_capacity(parts.iter().map(|p| p.text.len()).sum()), ends: Vec::with_capacity(parts.iter().map(|p| p.ends.len()).sum()) };
        for p in parts {
            let base = out.text.len();
            out.text.push_str(&p.text);
            out.ends.extend(p.ends.iter().map(|e| e + base));
        }
        out
    }
}

/// Folded strings compared with their digit runs as numbers ("lot 2" < "lot 10"),
/// as the French numeric collator the panel used.
pub fn natural_cmp(a: &[u8], b: &[u8]) -> Ord {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() { i += 1; }
            while j < b.len() && b[j].is_ascii_digit() { j += 1; }
            let x = trim_zeros(&a[si..i]);
            let y = trim_zeros(&b[sj..j]);
            let o = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if o != Ord::Equal { return o; }
            continue;
        }
        if a[i] != b[j] {
            return a[i].cmp(&b[j]);
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let k = d.iter().position(|&c| c != b'0').unwrap_or(d.len());
    &d[k..]
}

/// The values of column `col` over rows `ids`, with their counts. `trim`:
/// counted trimmed (" Lyon" is "Lyon"); otherwise as written, as the column
/// panel lists them. None if cancelled.
pub fn value_counts(st: &State, ids: &[RowId], col: usize, trim: bool, cancel: &AtomicBool) -> Option<ValueCounts> {
    // A column with a dictionary (`colcache`): its codes counted, each distinct value
    // hashed once; only the rows edited or created here are read.
    if let Some(dc) = crate::colcache::dict_counts(st, col, ids, cancel) {
        let mut sh = Sharded::new();
        for (j, v) in dc.dict.values.iter().enumerate() {
            if dc.count[j] > 0 { sh.add_n(if trim { v.trim() } else { v }, dc.count[j], dc.first[j]); }
        }
        let mut tmp = Vec::new();
        for &p in &dc.live {
            st.with_cell(ids[p as usize], col, &mut tmp, |v| sh.add(if trim { v.trim() } else { v }, p));
        }
        return Some(ValueCounts::from_shards(Sharded::merge(vec![sh]), ids.len()));
    }
    let parts = chunked(ids.len(), cancel, || (Sharded::new(), Vec::new()), |(sh, tmp), p| {
        st.with_cell(ids[p], col, tmp, |v| sh.add(if trim { v.trim() } else { v }, p as u32));
    })?;
    let shards = Sharded::merge(parts.into_iter().map(|p| p.0).collect());
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    Some(ValueCounts::from_shards(shards, ids.len()))
}

// ---------------------------------------------------------------------------
// The column panel's profile

/// One bar of the panel's histogram: its values as written (smallest, largest) and how many.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistBin {
    pub count: u64,
    pub first: String,
    pub last: String,
}

pub const PANEL_BINS: usize = 28;

/// What the column panel says of a column, from its value counts.
#[derive(Clone, Debug, Default)]
pub struct PanelProfile {
    /// Numbers or dates when ≥ 90 % of the filled cells are.
    pub kind: Option<Kind>,
    pub rows: usize,
    pub filled: u64,
    pub empty: u64,
    /// Distinct filled values.
    pub distinct: usize,
    /// Smallest and largest: (key, value as written, trimmed) — for text the
    /// key is the length in characters.
    pub min: Option<(f64, String)>,
    pub max: Option<(f64, String)>,
    pub sum: f64,
    pub count: u64,
    pub median: Option<f64>,
    /// Dates: the middle one as written, and the mean date.
    pub date_median: Option<String>,
    pub date_mean: Option<String>,
    /// Numbers or dates spread over PANEL_BINS bars, when they are not all one value.
    pub hist: Vec<HistBin>,
}

impl PanelProfile {
    pub fn avg(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// The profile the column panel shows, from the counts of the rows it looks at.
pub fn panel_profile(vc: &ValueCounts) -> PanelProfile {
    let mut p = PanelProfile { rows: vc.rows, ..Default::default() };
    // Every distinct value read once: its kind and its key.
    let ents: Vec<(Option<Kind>, u32)> = (0..vc.len()).into_par_iter().with_min_len(4096).map(|i| (cell_kind(vc.value(i)), vc.count(i))).collect();
    let (mut nums, mut dates) = (0u64, 0u64);
    for &(k, n) in &ents {
        match k {
            None => p.empty += n as u64,
            Some(k) => {
                p.distinct += 1;
                p.filled += n as u64;
                match k { Kind::Number => nums += n as u64, Kind::Date => dates += n as u64, Kind::Text => {} }
            }
        }
    }
    let kind = if p.filled > 0 && nums * 10 >= p.filled * 9 { Kind::Number } else if p.filled > 0 && dates * 10 >= p.filled * 9 { Kind::Date } else { Kind::Text };
    p.kind = (p.filled > 0).then_some(kind);
    if p.filled == 0 {
        return p;
    }
    let key = |v: &str| -> Option<f64> {
        let v = v.trim();
        match kind {
            Kind::Text => Some(v.chars().count() as f64),
            k => key_of(k, v).filter(|x| !x.is_nan()),
        }
    };
    let keyed: Vec<(f64, u32, u32)> = (0..vc.len())
        .into_par_iter()
        .with_min_len(4096)
        .filter_map(|i| if ents[i].0.is_some() { key(vc.value(i)).map(|k| (k, ents[i].1, i as u32)) } else { None })
        .collect();
    let (mut lo, mut hi) = (Best::NONE, Best::NONE);
    for &(k, n, i) in &keyed {
        lo.lower(k, i);
        hi.higher(k, i);
        p.sum += k * n as f64;
        p.count += n as u64;
    }
    if lo.pos == u32::MAX {
        return p;
    }
    p.min = Some((lo.key, vc.value(lo.pos as usize).trim().to_string()));
    p.max = Some((hi.key, vc.value(hi.pos as usize).trim().to_string()));
    if kind != Kind::Text {
        // The middle of the values in order (the mean of the two middle ones for an even count).
        let mut ks: Vec<(f64, u32, u32)> = keyed.clone();
        ks.par_sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let at = |pos: u64| {
            let mut s = 0u64;
            for &(k, n, i) in &ks {
                s += n as u64;
                if s > pos { return (k, i); }
            }
            (f64::NAN, 0)
        };
        let c = p.count;
        if kind == Kind::Number {
            p.median = Some(if c % 2 == 1 { at((c - 1) / 2).0 } else { (at(c / 2 - 1).0 + at(c / 2).0) / 2.0 });
        } else {
            // Keys are YYYYMMDDhhmmss: the mean is taken in seconds, a median is a date of the column.
            p.date_median = Some(vc.value(at((c - 1) / 2).1 as usize).trim().to_string());
            let (mut secs, mut n) = (0.0, 0u64);
            for &(_, k, i) in &keyed {
                if let Some(t) = crate::sparkline::time_key(vc.value(i as usize)) { secs += t * k as f64; n += k as u64; }
            }
            if n > 0 {
                let t = crate::sparkline::time_text(secs / n as f64);
                p.date_mean = t.split(' ').next().map(str::to_string);
            }
        }
    }
    // Where a value stands on the bars: dates by their time, keys jump from month to month.
    let place = |k: f64, i: u32| if kind == Kind::Date { crate::sparkline::time_key(vc.value(i as usize)).unwrap_or(k) } else { k };
    let (first, span) = (place(lo.key, lo.pos), place(hi.key, hi.pos) - place(lo.key, lo.pos));
    if kind != Kind::Text && hi.key > lo.key && span > 0.0 {
        let mut bins = vec![HistBin::default(); PANEL_BINS];
        let mut ends = vec![(Best::NONE, Best::NONE); PANEL_BINS];
        for &(k, n, i) in &keyed {
            let b = (((place(k, i) - first) / span * PANEL_BINS as f64).floor().max(0.0) as usize).min(PANEL_BINS - 1);
            bins[b].count += n as u64;
            ends[b].0.lower(k, i);
            ends[b].1.higher(k, i);
        }
        for (b, (f, l)) in bins.iter_mut().zip(ends) {
            if f.pos != u32::MAX {
                b.first = vc.value(f.pos as usize).trim().to_string();
                b.last = vc.value(l.pos as usize).trim().to_string();
            }
        }
        p.hist = bins;
    }
    p
}

// ---------------------------------------------------------------------------
// The file profile

/// One column of the file profile.
#[derive(Clone, Debug, Default)]
pub struct ColProfile {
    pub filled: u64,
    pub empty: u64,
    pub nums: u64,
    pub dates: u64,
    pub texts: u64,
    /// Distinct filled values (trimmed), exact.
    pub distinct: u64,
    /// Smallest and largest of the column's kind: (key, as written); None for text.
    pub min: Option<(f64, String)>,
    pub max: Option<(f64, String)>,
    /// Sum and count of its numbers.
    pub sum: f64,
    pub cnt: u64,
    /// The first shortest and first longest filled values.
    pub short: Option<String>,
    pub long: Option<String>,
}

impl ColProfile {
    /// Numbers or dates when ≥ 90 % of the filled cells are; None: an empty column.
    pub fn kind(&self) -> Option<Kind> {
        if self.filled == 0 { None }
        else if self.nums * 10 >= self.filled * 9 { Some(Kind::Number) }
        else if self.dates * 10 >= self.filled * 9 { Some(Kind::Date) }
        else { Some(Kind::Text) }
    }

    /// Every filled cell different: a key?
    pub fn unique(&self) -> bool {
        self.filled > 0 && self.distinct == self.filled
    }

    pub fn avg(&self) -> Option<f64> {
        (self.cnt > 0).then(|| self.sum / self.cnt as f64)
    }
}

/// The smallest (or largest) key met and where: the first of equal ones.
#[derive(Clone, Copy)]
pub(crate) struct Best {
    pub key: f64,
    /// u32::MAX: none yet.
    pub pos: u32,
}

impl Best {
    pub const NONE: Best = Best { key: f64::NAN, pos: u32::MAX };
    #[inline]
    pub fn lower(&mut self, key: f64, pos: u32) {
        if self.pos == u32::MAX || key < self.key { *self = Best { key, pos }; }
    }
    #[inline]
    pub fn higher(&mut self, key: f64, pos: u32) {
        if self.pos == u32::MAX || key > self.key { *self = Best { key, pos }; }
    }
}

struct ProfAcc {
    p: ColProfile,
    nlo: Best,
    nhi: Best,
    dlo: Best,
    dhi: Best,
    short: Best,
    long: Best,
    set: Sharded,
    tmp: Vec<u8>,
}

/// Every column of the file at a glance, over rows `ids` (usually all of
/// them; an evenly spaced sample of them gives the JS's sampled profile):
/// one parallel pass per column — a column's distinct values are held only
/// while it is read. None if cancelled.
pub fn profile(st: &State, ids: &[RowId], cancel: &AtomicBool) -> Option<Vec<ColProfile>> {
    (0..st.ncols).map(|c| profile_col(st, ids, c, cancel)).collect()
}

impl ProfAcc {
    fn new() -> ProfAcc {
        ProfAcc { p: ColProfile::default(), nlo: Best::NONE, nhi: Best::NONE, dlo: Best::NONE, dhi: Best::NONE, short: Best::NONE, long: Best::NONE, set: Sharded::new(), tmp: Vec::new() }
    }

    /// One cell, of the row at `pos` among those profiled.
    #[inline]
    fn add(&mut self, v: &str, pos: u32) {
        let v = v.trim();
        let Some((k, x)) = cell_read(v) else {
            self.p.empty += 1;
            return;
        };
        self.p.filled += 1;
        self.set.add(v, pos);
        match k {
            Kind::Number => {
                self.p.nums += 1;
                self.p.sum += x;
                self.p.cnt += 1;
                self.nlo.lower(x, pos);
                self.nhi.higher(x, pos);
            }
            Kind::Date => {
                self.p.dates += 1;
                self.dlo.lower(x, pos);
                self.dhi.higher(x, pos);
            }
            Kind::Text => self.p.texts += 1,
        }
        let len = v.chars().count() as f64;
        self.short.lower(len, pos);
        self.long.higher(len, pos);
    }

    /// The chunks' accumulators (in row order) as the column's profile.
    fn finish(parts: Vec<ProfAcc>, st: &State, ids: &[RowId], col: usize) -> Option<ColProfile> {
        // In order: on ties the earlier chunk's value stays, as a single pass keeps the first.
        let mut it = parts.into_iter();
        let mut acc = it.next()?;
        let mut sets = Vec::new();
        for b in it {
            let (p, q) = (&mut acc.p, &b.p);
            p.filled += q.filled; p.empty += q.empty; p.nums += q.nums; p.dates += q.dates; p.texts += q.texts;
            p.sum += q.sum; p.cnt += q.cnt;
            for (x, y, lo) in [(&mut acc.nlo, b.nlo, true), (&mut acc.nhi, b.nhi, false), (&mut acc.dlo, b.dlo, true), (&mut acc.dhi, b.dhi, false), (&mut acc.short, b.short, true), (&mut acc.long, b.long, false)] {
                if y.pos != u32::MAX { if lo { x.lower(y.key, y.pos) } else { x.higher(y.key, y.pos) } }
            }
            sets.push(b.set);
        }
        sets.push(std::mem::replace(&mut acc.set, Sharded(Vec::new())));
        acc.p.distinct = Sharded::merge(sets).iter().map(|c| c.ents.len() as u64).sum();
        let mut tmp = Vec::new();
        let cell = |b: Best, tmp: &mut Vec<u8>| (b.pos != u32::MAX).then(|| (b.key, st.with_cell(ids[b.pos as usize], col, tmp, |v| v.trim().to_string())));
        let mut p = acc.p;
        match p.kind() {
            Some(Kind::Number) => { p.min = cell(acc.nlo, &mut tmp); p.max = cell(acc.nhi, &mut tmp); }
            Some(Kind::Date) => { p.min = cell(acc.dlo, &mut tmp); p.max = cell(acc.dhi, &mut tmp); }
            _ => {}
        }
        p.short = cell(acc.short, &mut tmp).map(|x| x.1);
        p.long = cell(acc.long, &mut tmp).map(|x| x.1);
        Some(p)
    }
}

pub fn profile_col(st: &State, ids: &[RowId], col: usize, cancel: &AtomicBool) -> Option<ColProfile> {
    let parts = chunked(ids.len(), cancel, ProfAcc::new, |a, p| {
        let mut tmp = std::mem::take(&mut a.tmp);
        st.with_cell(ids[p], col, &mut tmp, |v| a.add(v, p as u32));
        a.tmp = tmp;
    })?;
    ProfAcc::finish(parts, st, ids, col)
}

/// The profiles of columns `cols` (ascending) in one pass over the rows: each record
/// read and split once for all of them, instead of once per column — a big file's
/// profile is bound by reading its records. Their distinct values are held together.
pub fn profile_cols(st: &State, ids: &[RowId], cols: &[usize], cancel: &AtomicBool) -> Option<Vec<ColProfile>> {
    if cols.is_empty() { return Some(Vec::new()); }
    // Each record split once for all of them, moved or edited columns alike.
    let parts = chunked(ids.len(), cancel, || cols.iter().map(|_| ProfAcc::new()).collect::<Vec<_>>(), |accs, p| {
        st.visit_cols(ids[p], cols, |k, v| accs[k].add(v, p as u32));
    })?;
    // Chunk-major → column-major.
    let mut by: Vec<Vec<ProfAcc>> = cols.iter().map(|_| Vec::with_capacity(parts.len())).collect();
    for part in parts {
        for (k, acc) in part.into_iter().enumerate() { by[k].push(acc); }
    }
    by.into_iter().zip(cols).map(|(parts, &c)| ProfAcc::finish(parts, st, ids, c)).collect()
}

// ---------------------------------------------------------------------------
// Totals

/// A function of the totals row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TotalFn {
    Sum,
    Avg,
    Median,
    Min,
    Max,
    Filled,
    Empty,
    Distinct,
    Pct,
}

impl TotalFn {
    /// The functions a column of this kind offers, its own first.
    pub fn for_kind(kind: Kind) -> &'static [TotalFn] {
        use TotalFn::*;
        match kind {
            Kind::Number => &[Sum, Avg, Median, Min, Max, Filled, Empty, Distinct, Pct],
            Kind::Date => &[Min, Max, Filled, Empty, Distinct, Pct],
            Kind::Text => &[Filled, Empty, Distinct, Pct],
        }
    }

    /// The short label in the cell, the name in the menu.
    pub fn labels(self, kind: Kind) -> (&'static str, &'static str) {
        use TotalFn::*;
        match (self, kind) {
            (Min, Kind::Date) => ("First", "Earliest"),
            (Max, Kind::Date) => ("Last", "Latest"),
            (Sum, _) => ("Sum", "Sum"),
            (Avg, _) => ("Avg", "Average"),
            (Median, _) => ("Median", "Median"),
            (Min, _) => ("Min", "Smallest"),
            (Max, _) => ("Max", "Largest"),
            (Filled, _) => ("Filled", "Filled cells"),
            (Empty, _) => ("Empty", "Empty cells"),
            (Distinct, _) => ("Distinct", "Distinct values"),
            (Pct, _) => ("Filled", "Share filled"),
        }
    }
}

/// A column to total: what beyond the running counts it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TotalReq {
    pub col: usize,
    pub kind: Kind,
    pub median: bool,
    pub distinct: bool,
}

/// A column's totals over the rows shown: everything but the median and the
/// distinct count comes with every pass; those two only when asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Totals {
    pub col: usize,
    pub kind: Kind,
    /// Rows shown.
    pub rows: usize,
    /// Numbers (or dates) read.
    pub n: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
    /// A date column's bounds as written.
    pub min_s: String,
    pub max_s: String,
    pub filled: u64,
    pub empty: u64,
    /// Filled cells that are not of the column's kind.
    pub bad: u64,
    /// Decimals the column writes (from its first TOT_DEC_ROWS numbers), six at most.
    pub dec: usize,
    pub median: Option<f64>,
    pub distinct: Option<u64>,
}

/// What a total shows: the text in the cell, the text copied, the tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TotalValue {
    pub text: String,
    pub raw: String,
    pub tip: String,
}

pub const TOT_DEC_ROWS: usize = 2000;

/// Decimals as the column writes them: "12,50" two, "1 234" none — and
/// "1,234" none either, a whole number with three digits after its mark being
/// thousands. Six at most.
pub fn decimals(s: &str, x: f64) -> usize {
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 && !b[end - 1].is_ascii_digit() { end -= 1; }
    let mut start = end;
    while start > 0 && b[start - 1].is_ascii_digit() { start -= 1; }
    if start == 0 || end == start || !matches!(b[start - 1], b'.' | b',') {
        return 0;
    }
    let d = end - start;
    if d == 3 && x.fract() == 0.0 { 0 } else { d.min(6) }
}

struct TotAcc {
    n: u64,
    sum: f64,
    lo: Best,
    hi: Best,
    filled: u64,
    empty: u64,
    bad: u64,
    decs: Vec<u8>,
    vals: Vec<f64>,
    set: Option<Sharded>,
}

/// One cell read for a column's totals: empty, not of the column's kind, or its key and decimals.
#[derive(Clone, Copy)]
enum Read {
    Empty,
    Bad,
    Val(f64, u8),
    /// Filled, in a text column.
    Text,
}

#[inline]
fn read_total(kind: Kind, v: &str) -> Read {
    let v = v.trim();
    if v.is_empty() {
        return Read::Empty;
    }
    match kind {
        Kind::Number => num_cell(v).map_or(Read::Bad, |x| Read::Val(x, decimals(v, x) as u8)),
        Kind::Date => parse_date(v).map_or(Read::Bad, |x| Read::Val(x, 0)),
        Kind::Text => Read::Text,
    }
}

impl TotAcc {
    #[inline]
    fn add(&mut self, r: &TotalReq, c: Read, pos: u32) {
        match c {
            Read::Empty => self.empty += 1,
            Read::Bad => { self.filled += 1; self.bad += 1; }
            Read::Text => self.filled += 1,
            Read::Val(x, d) => {
                self.filled += 1;
                self.n += 1;
                self.lo.lower(x, pos);
                self.hi.higher(x, pos);
                if r.kind == Kind::Number {
                    self.sum += x;
                    if r.median { self.vals.push(x); }
                    if self.decs.len() < TOT_DEC_ROWS { self.decs.push(d); }
                }
            }
        }
    }
}

/// The totals of columns `reqs` over rows `ids`, in one parallel pass (a row
/// still as read is split once for all of them). None if cancelled. A column
/// with a dictionary (`colcache`) is not read at all: each distinct value is
/// read once, every row only gives its code (the rows edited here aside).
///
/// Incremental as the JS: the caller keeps each column's `Totals` for the
/// rows they were computed on, and asks again only for the columns whose
/// rows changed or whose new function needs what they lack (`has`) — and a
/// few cells edited are told to their totals (`Totals::edit`).
pub fn totals(st: &State, ids: &[RowId], reqs: &[TotalReq], cancel: &AtomicBool) -> Option<Vec<Totals>> {
    if reqs.is_empty() {
        return Some(Vec::new());
    }
    // Per request with a dictionary: each value read once, and the rows the dictionary does not give.
    let dicts: Vec<Option<(Arc<crate::colcache::Dict>, Vec<Read>, Option<crate::colcache::Bits>)>> = reqs.iter().map(|r| {
        if r.distinct { return None; }
        let d = crate::colcache::get(st, r.col).dict?;
        let reads = d.values.par_iter().map(|v| read_total(r.kind, v)).collect();
        Some((d, reads, crate::colcache::edited_rows(st, r.col)))
    }).collect();
    // The other columns in ascending order for one split of the row; `slot[k]`: where req k's cells come.
    let mut cols: Vec<usize> = reqs.iter().zip(&dicts).filter(|(_, d)| d.is_none()).map(|(r, _)| r.col).collect();
    cols.sort_unstable();
    cols.dedup();
    let by_slot: Vec<Vec<usize>> = cols.iter().map(|&c| (0..reqs.len()).filter(|&k| dicts[k].is_none() && reqs[k].col == c).collect()).collect();
    let coded: Vec<usize> = (0..reqs.len()).filter(|&k| dicts[k].is_some()).collect();
    let n = st.base.n;
    let new = || {
        let accs: Vec<TotAcc> = reqs.iter().map(|r| TotAcc { n: 0, sum: 0.0, lo: Best::NONE, hi: Best::NONE, filled: 0, empty: 0, bad: 0, decs: Vec::new(), vals: Vec::new(), set: r.distinct.then(Sharded::new) }).collect();
        (accs, Vec::new())
    };
    let parts = chunked(ids.len(), cancel, new, |(accs, tmp), p| {
        let (pos, id) = (p as u32, ids[p]);
        for &k in &coded {
            let (d, reads, edited) = dicts[k].as_ref().unwrap();
            let r = &reqs[k];
            if (id as usize) < n && !edited.as_ref().is_some_and(|e| e.has(id)) {
                accs[k].add(r, reads[d.codes[id as usize] as usize], pos);
            } else {
                let c = st.with_cell(id, r.col, tmp, |v| read_total(r.kind, v));
                accs[k].add(r, c, pos);
            }
        }
        if cols.is_empty() { return; }
        st.visit_cols(id, &cols, |s, v| {
            for &k in &by_slot[s] {
                let (r, a) = (&reqs[k], &mut accs[k]);
                let c = read_total(r.kind, v);
                if let (Some(set), Read::Bad | Read::Text | Read::Val(..)) = (&mut a.set, c) { set.add(v.trim(), pos); }
                a.add(r, c, pos);
            }
        });
    })?;
    let mut out = Vec::with_capacity(reqs.len());
    let mut chunks: Vec<Vec<TotAcc>> = parts.into_iter().map(|p| p.0).collect();
    let mut tmp = Vec::new();
    for (k, r) in reqs.iter().enumerate() {
        let mut t = Totals { col: r.col, kind: r.kind, rows: ids.len(), n: 0, sum: 0.0, min: f64::INFINITY, max: f64::NEG_INFINITY, min_s: String::new(), max_s: String::new(), filled: 0, empty: 0, bad: 0, dec: 0, median: None, distinct: None };
        let (mut lo, mut hi, mut seen) = (Best::NONE, Best::NONE, 0);
        let mut vals = Vec::new();
        let mut sets = Vec::new();
        for c in chunks.iter_mut() {
            let a = &mut c[k];
            t.n += a.n; t.sum += a.sum; t.filled += a.filled; t.empty += a.empty; t.bad += a.bad;
            if a.lo.pos != u32::MAX { lo.lower(a.lo.key, a.lo.pos); hi.higher(a.hi.key, a.hi.pos); }
            for &d in a.decs.iter().take(TOT_DEC_ROWS - seen) { t.dec = t.dec.max(d as usize); }
            seen += a.decs.len().min(TOT_DEC_ROWS - seen);
            if r.median { vals.push(std::mem::take(&mut a.vals)); }
            if let Some(s) = a.set.take() { sets.push(s); }
        }
        if lo.pos != u32::MAX {
            t.min = lo.key;
            t.max = hi.key;
            if r.kind == Kind::Date {
                t.min_s = st.with_cell(ids[lo.pos as usize], r.col, &mut tmp, |v| v.trim().to_string());
                t.max_s = st.with_cell(ids[hi.pos as usize], r.col, &mut tmp, |v| v.trim().to_string());
            }
        }
        if r.median {
            t.median = median(vals.concat());
        }
        if r.distinct {
            t.distinct = Some(Sharded::merge(sets).iter().map(|c| c.ents.len() as u64).sum());
        }
        out.push(t);
    }
    if cancel.load(Ordering::Relaxed) { None } else { Some(out) }
}

/// Of rows `0..n` (row `p` is `id(p)`), the first whose number in `col` is closest to `x`
/// (the column panel's "go to"), read in parallel. None if cancelled; Some(None): no number.
pub fn closest(st: &State, n: usize, id: impl Fn(usize) -> RowId + Sync + Send, col: usize, x: f64, cancel: &AtomicBool) -> Option<Option<usize>> {
    let parts = chunked(n, cancel, || (None::<(f64, usize)>, Vec::new()), |(best, tmp), p| {
        st.with_cell(id(p), col, tmp, |s| {
            if let Some(v) = num_cell(s) {
                let d = (v - x).abs();
                if best.is_none_or(|b| d < b.0) { *best = Some((d, p)); }
            }
        });
    })?;
    Some(parts.into_iter().filter_map(|p| p.0).min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))).map(|b| b.1))
}

/// The decimals a number column's totals write: the most among its first TOT_DEC_ROWS numbers
/// over `rows` (what a pass of `totals` finds), for totals kept up by `Totals::edit`.
pub fn total_decimals(st: &State, rows: impl Iterator<Item = RowId>, col: usize) -> usize {
    let (mut seen, mut dec, mut tmp) = (0, 0, Vec::new());
    for id in rows {
        if let Read::Val(_, d) = st.with_cell(id, col, &mut tmp, |v| read_total(Kind::Number, v)) {
            dec = dec.max(d as usize);
            seen += 1;
            if seen == TOT_DEC_ROWS { break; }
        }
    }
    dec
}

/// The middle value (the mean of the two middle ones for an even count), by selection, not a sort.
pub fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let (h, odd) = (v.len() / 2, v.len() % 2 == 1);
    let (low, m, _) = v.select_nth_unstable_by(h, f64::total_cmp);
    let m = *m;
    if odd {
        return Some(m);
    }
    let below = low.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some((below + m) / 2.0)
}

/// The value at fraction `p` of the sorted values (index rounded), by selection.
pub fn quantile(v: &mut [f64], p: f64) -> f64 {
    let i = (p * (v.len() - 1) as f64).round() as usize;
    *v.select_nth_unstable_by(i, f64::total_cmp).1
}

/// The values at fractions `a` ≤ `b`: the second selected among the values
/// right of the first only.
pub fn quantiles(v: &mut [f64], a: f64, b: f64) -> (f64, f64) {
    let at = |p: f64| (p * (v.len() - 1) as f64).round() as usize;
    let (i, j) = (at(a), at(b));
    let (_, x, right) = v.select_nth_unstable_by(i, f64::total_cmp);
    let x = *x;
    if j <= i {
        return (x, x);
    }
    (x, *right.select_nth_unstable_by(j - i - 1, f64::total_cmp).1)
}

impl Totals {
    /// One cell of the rows counted changed from `old` to `new`: the totals follow without
    /// counting the rows again. False when they cannot — a median or a distinct count kept, a
    /// bound that may move inwards, a date bound whose text is the first row's —: count again.
    /// The decimals written stay to be found again (`total_decimals`).
    pub fn edit(&mut self, old: &str, new: &str) -> bool {
        if self.median.is_some() || self.distinct.is_some() {
            return false;
        }
        let dec = |a: &mut u64| a.checked_sub(1).map(|x| *a = x).is_some();
        match read_total(self.kind, old) {
            Read::Empty => if !dec(&mut self.empty) { return false },
            Read::Text => if !dec(&mut self.filled) { return false },
            Read::Bad => if !dec(&mut self.filled) || !dec(&mut self.bad) { return false },
            Read::Val(x, _) => {
                if !dec(&mut self.filled) || !dec(&mut self.n) || x <= self.min || x >= self.max { return false; }
                if self.kind == Kind::Number { self.sum -= x; }
            }
        }
        match read_total(self.kind, new) {
            Read::Empty => self.empty += 1,
            Read::Text => self.filled += 1,
            Read::Bad => { self.filled += 1; self.bad += 1; }
            Read::Val(x, _) => {
                if self.kind == Kind::Date && (x == self.min || x == self.max) { return false; }
                self.filled += 1;
                self.n += 1;
                if self.kind == Kind::Number { self.sum += x; }
                let text = || if self.kind == Kind::Date { new.trim().to_string() } else { String::new() };
                if x < self.min { self.min_s = text(); self.min = x; }
                if x > self.max { self.max_s = text(); self.max = x; }
            }
        }
        true
    }

    /// Whether this pass computed what `f` needs.
    pub fn has(&self, f: TotalFn) -> bool {
        match f {
            TotalFn::Median => self.median.is_some() || self.n == 0,
            TotalFn::Distinct => self.distinct.is_some(),
            _ => true,
        }
    }

    /// What the cell shows for `f`; None when this pass did not compute it.
    pub fn value(&self, f: TotalFn) -> Option<TotalValue> {
        if !self.has(f) {
            return None;
        }
        let rows = self.rows as u64;
        let date = self.kind == Kind::Date;
        let what = if date { "date" } else { "number" };
        let num = |x: f64, tip: String| TotalValue { text: fmt_num(x, self.dec, self.dec, true), raw: fmt_num(x, 0, self.dec, false), tip };
        let left = if self.bad > 0 { format!(" · {} value{} not {} left out", fmt_int(self.bad), plural(self.bad), if date { "a date" } else { "a number" }) } else { String::new() };
        let of = format!("{} {what}{} in {} row{} shown{left}", fmt_int(self.n), plural(self.n), fmt_int(rows), plural(rows));
        let none = || TotalValue { text: "—".into(), raw: String::new(), tip: format!("No {what} in the {} rows shown", fmt_int(rows)) };
        let cnt = |k: u64, tip: String| TotalValue { text: fmt_int(k), raw: k.to_string(), tip };
        Some(match f {
            TotalFn::Sum => if self.n == 0 { none() } else { num(self.sum, format!("Sum of {of}")) },
            TotalFn::Avg | TotalFn::Median => {
                if self.n == 0 { return Some(none()); }
                let (x, name) = if f == TotalFn::Avg { (self.sum / self.n as f64, "Average") } else { (self.median?, "Median") };
                let d1 = self.dec.max(2);
                TotalValue { text: fmt_num(x, self.dec, d1, true), raw: fmt_num(x, 0, d1, false), tip: format!("{name} of {of}") }
            }
            TotalFn::Min | TotalFn::Max => {
                if self.n == 0 { return Some(none()); }
                let lo = f == TotalFn::Min;
                let name = f.labels(self.kind).1;
                if date {
                    let s = if lo { &self.min_s } else { &self.max_s };
                    TotalValue { text: s.clone(), raw: s.clone(), tip: format!("{name} of {of}") }
                } else {
                    num(if lo { self.min } else { self.max }, format!("{name} of {of}"))
                }
            }
            TotalFn::Filled => cnt(self.filled, format!("{} filled cells in {} rows shown", fmt_int(self.filled), fmt_int(rows))),
            TotalFn::Empty => cnt(self.empty, format!("{} empty cells in {} rows shown", fmt_int(self.empty), fmt_int(rows))),
            TotalFn::Pct => {
                let p = if rows > 0 { self.filled as f64 / rows as f64 * 100.0 } else { 0.0 };
                let s = fmt_num(p, 0, if p > 0.0 && p < 1.0 { 1 } else { 0 }, true) + " %";
                TotalValue { text: s.clone(), raw: s, tip: format!("{} of {} rows shown are filled", fmt_int(self.filled), fmt_int(rows)) }
            }
            TotalFn::Distinct => {
                let k = self.distinct?;
                cnt(k, format!("{} distinct values in {} rows shown", fmt_int(k), fmt_int(rows)))
            }
        })
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

const AMOUNT_WORDS: &[&str] = &[
    "montant", "amount", "total", "totaux", "prix", "price", "tarif", "ca", "chiffre", "revenue", "revenu", "qty", "quantite", "quantity", "nb", "nombre",
    "count", "somme", "sum", "cout", "cost", "budget", "salaire", "salary", "poids", "weight", "volume", "surface", "duree", "duration", "heures", "hours",
    "points", "score", "solde", "balance", "ht", "ttc", "tva", "vat", "remise", "discount", "marge", "margin",
];

/// Whether the totals row sums a column by default: a number column that
/// reads as a quantity — an amount-like title, or decimals in its values —
/// not an ID, a phone number or a postal code.
pub fn sum_by_default(st: &State, col: usize, kind: Kind, title: &str) -> bool {
    if kind != Kind::Number {
        return false;
    }
    let slug = crate::text::fold(title);
    if slug.split(|c: char| !c.is_alphanumeric()).any(|w| AMOUNT_WORDS.contains(&w)) {
        return true;
    }
    let (mut seen, mut dec, mut codes) = (0, 0, 0);
    let mut tmp = Vec::new();
    for p in 0..st.len() {
        let stop = st.with_cell(st.data_id(p), col, &mut tmp, |v| {
            let v = v.trim();
            if v.is_empty() { return false; }
            let b = v.as_bytes();
            if b.windows(2).any(|w| matches!(w[0], b'.' | b',') && w[1].is_ascii_digit()) { dec += 1; }
            // A leading zero, or nine digits and more: a code, a phone.
            if (b.len() > 1 && b[0] == b'0' && b[1].is_ascii_digit()) || b.iter().filter(|c| c.is_ascii_digit()).count() >= 9 { codes += 1; }
            seen += 1;
            seen >= 400
        });
        if stop { break; }
    }
    seen > 0 && dec > 0 && codes == 0
}

// ---------------------------------------------------------------------------
// Numbers as fr-FR writes them

/// `x` as fr-FR writes it, between `min_dec` and `max_dec` decimals:
/// `1 234,5` (narrow no-break spaces between thousands when `group`).
pub fn fmt_num(x: f64, min_dec: usize, max_dec: usize, group: bool) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "∞".into() } else { "-∞".into() };
    }
    let (int, mut frac) = round_dec(x.abs(), max_dec);
    frac.truncate(frac.trim_end_matches('0').len());
    while frac.len() < min_dec.min(max_dec) {
        frac.push('0');
    }
    let mut out = String::with_capacity(int.len() + frac.len() + 8);
    if x < 0.0 && (int.bytes().any(|c| c != b'0') || frac.bytes().any(|c| c != b'0')) {
        out.push('-');
    }
    let n = int.len();
    for (i, c) in int.chars().enumerate() {
        if group && i > 0 && (n - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push(',');
        out.push_str(&frac);
    }
    out
}

/// `x` (≥ 0) to `d` decimals as Intl rounds: its shortest decimal form, half
/// away from zero (2.675 → 2.68, where the binary value 2.67499… gives 2.67).
fn round_dec(x: f64, d: usize) -> (String, String) {
    let s = format!("{x}");
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    if frac.len() <= d {
        return (int.to_string(), frac.to_string());
    }
    let mut digits: Vec<u8> = int.bytes().chain(frac[..d].bytes()).collect();
    if frac.as_bytes()[d] >= b'5' {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let k = digits.len() - d;
    let s = String::from_utf8(digits).unwrap();
    (s[..k].to_string(), s[k..].to_string())
}

/// A count as fr-FR writes it: `1 234 567`.
pub fn fmt_int(n: u64) -> String {
    fmt_num(n as f64, 0, 0, true)
}

/// A share as the charts write it: `60 %`, `2,5 %` under a tenth.
pub fn fmt_pct(x: f64) -> String {
    fmt_num(x * 100.0, 0, if x < 0.1 { 1 } else { 0 }, true) + " %"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    pub(crate) fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    pub(crate) fn all(t: &Table) -> Vec<RowId> {
        (0..t.st.len()).map(|p| t.st.data_id(p)).collect()
    }

    const STATUTS: [&str; 4] = ["actif", "vip", "prospect", "inactif"];
    const MONTANTS: [&str; 4] = ["3", "12,5", "1 250,00", "47,10"];

    /// tests/test_totals.py's file.
    fn ventes() -> String {
        let mut s = String::from("id;statut;montant;date;note\n");
        for i in 1..=120 {
            let m = if i == 60 { "n/a" } else { MONTANTS[i % 4] };
            let d = if i % 10 != 0 { format!("2024-{:02}-{:02}", i % 12 + 1, i % 27 + 1) } else { String::new() };
            s += &format!("{i};{};{m};{d};texte {i}\n", STATUTS[i % 4]);
        }
        s
    }

    fn num(s: &str) -> f64 {
        s.replace(' ', "").replace(',', ".").parse().unwrap()
    }

    #[test]
    fn kinds_of_cells() {
        assert_eq!(cell_kind("  "), None);
        assert_eq!(cell_kind("12,5"), Some(Kind::Number));
        assert_eq!(cell_kind("2024-03-15"), Some(Kind::Date));
        assert_eq!(cell_kind("Lyon"), Some(Kind::Text));
        assert_eq!(cell_kind("12 rue"), Some(Kind::Text));
        assert_eq!(num_cell("1 250,00"), Some(1250.0));
        assert_eq!(decimals("12,50", 12.5), 2);
        assert_eq!(decimals("1,234", 1234.0), 0);
        assert_eq!(decimals("1 250,00 €", 1250.0), 2);
        assert_eq!(decimals("3", 3.0), 0);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_num(1631.2, 2, 2, true), "1\u{202f}631,20");
        assert_eq!(fmt_num(1234567.0, 0, 0, true), "1\u{202f}234\u{202f}567");
        assert_eq!(fmt_num(-0.001, 0, 2, true), "0");
        assert_eq!(fmt_num(-12.5, 0, 2, false), "-12,5");
        assert_eq!(fmt_num(0.25, 0, 1, true), "0,3");   // half away from zero, as Intl
        assert_eq!(fmt_num(2.675, 2, 2, true), "2,68");
        assert_eq!(fmt_num(9.999, 0, 2, true), "10");
        assert_eq!(fmt_num(999.96, 1, 1, true), "1\u{202f}000,0");
        assert_eq!(fmt_num(5.0, 2, 2, false), "5,00");
        assert_eq!(fmt_num(1e21, 0, 0, true).chars().count(), 22 + 7);   // no exponent
        assert_eq!(fmt_pct(0.6), "60 %");
        assert_eq!(fmt_pct(0.025), "2,5 %");
    }

    #[test]
    fn totals_as_the_js() {
        let t = table(&ventes());
        let no = AtomicBool::new(false);
        let ids = all(&t);
        let kinds = crate::view::detect_kinds(&t.st);
        assert_eq!(kinds, vec![Kind::Number, Kind::Text, Kind::Number, Kind::Date, Kind::Text]);
        let h = t.st.headers();
        let def: Vec<bool> = (0..5).map(|c| sum_by_default(&t.st, c, kinds[c], &h[c])).collect();
        assert_eq!(def, vec![false, false, true, false, false]);   // the id is not summed
        let reqs: Vec<TotalReq> = (0..5).map(|c| TotalReq { col: c, kind: kinds[c], median: c == 2, distinct: c == 1 }).collect();
        let r = totals(&t.st, &ids, &reqs, &no).unwrap();
        let vals: Vec<f64> = (1..=120).filter(|&i| i != 60).map(|i| num(MONTANTS[i % 4])).collect();
        let tot: f64 = vals.iter().sum();
        let m = &r[2];
        assert_eq!(m.n, 119);
        assert_eq!(m.bad, 1);
        assert_eq!(m.dec, 2);
        let v = m.value(TotalFn::Sum).unwrap();
        assert_eq!(v.text, fmt_num(tot, 2, 2, true));
        assert!(v.tip.contains("119 numbers"), "{}", v.tip);
        assert!(v.tip.contains("1 value not a number left out"));
        assert_eq!(m.value(TotalFn::Avg).unwrap().text, fmt_num(tot / 119.0, 2, 2, true));
        let mut s = vals.clone();
        s.sort_by(f64::total_cmp);
        assert_eq!(m.median, Some(s[59]));
        assert_eq!(r[1].value(TotalFn::Distinct).unwrap().text, "4");
        assert!(r[1].value(TotalFn::Median).is_none() || r[1].n == 0);
        assert!(r[2].value(TotalFn::Distinct).is_none());
        let dates: Vec<String> = (1..=120).filter(|i| i % 10 != 0).map(|i| format!("2024-{:02}-{:02}", i % 12 + 1, i % 27 + 1)).collect();
        assert_eq!(r[3].value(TotalFn::Max).unwrap().text, *dates.iter().max().unwrap());
        assert_eq!(r[3].value(TotalFn::Min).unwrap().text, *dates.iter().min().unwrap());
        assert!(r[3].value(TotalFn::Max).unwrap().tip.starts_with("Latest of"));
        assert_eq!(r[3].value(TotalFn::Empty).unwrap().text, "12");
        assert_eq!(r[3].value(TotalFn::Pct).unwrap().text, "90 %");
        // The rows shown only: "vip" rows write 12,5 — one decimal.
        let vip: Vec<RowId> = ids.iter().copied().filter(|&id| t.st.cell(id, 1) == "vip").collect();
        let r = totals(&t.st, &vip, &reqs[2..3], &no).unwrap();
        let tot: f64 = (1..=120).filter(|&i| i != 60 && i % 4 == 1).map(|i| num(MONTANTS[i % 4])).sum();
        assert_eq!(r[0].value(TotalFn::Sum).unwrap().text, fmt_num(tot, 1, 1, true));
    }

    /// A column with a dictionary is counted from its codes: the same totals as read cell by
    /// cell, its edited and new rows read live.
    #[test]
    fn totals_from_the_dictionary() {
        let mut t = table(&ventes());
        let no = AtomicBool::new(false);
        let kinds = crate::view::detect_kinds(&t.st);
        let reqs: Vec<TotalReq> = (0..5).map(|c| TotalReq { col: c, kind: kinds[c], median: c == 2, distinct: false }).collect();
        let id = t.st.data_id(4);
        t.set_cells(&[(id, 2, "99,99".into()), (id, 3, "".into())]);
        let fresh = t.insert_rows(0, 1)[0];
        t.set_cells(&[(fresh, 2, "1".into()), (fresh, 1, "vip".into())]);
        let ids = all(&t);
        let read = totals(&t.st, &ids, &reqs, &no).unwrap();
        for c in 0..5 { crate::colcache::ensure(&t.st, c, crate::colcache::Need::Dict, &no); }
        assert!([1, 2].iter().all(|&c| crate::colcache::get(&t.st, c).dict.is_some()));
        let coded = totals(&t.st, &ids, &reqs, &no).unwrap();
        assert_eq!(coded, read);
        assert_eq!(read[2].dec, 2);
    }

    /// Cells edited one by one, told to the totals: what a count over the rows gives (the sum to
    /// rounding), or a refusal where only a count can say.
    #[test]
    fn totals_follow_edits() {
        let mut t = table(&ventes());
        let no = AtomicBool::new(false);
        let kinds = crate::view::detect_kinds(&t.st);
        let reqs: Vec<TotalReq> = (0..5).map(|c| TotalReq { col: c, kind: kinds[c], median: false, distinct: false }).collect();
        let ids = all(&t);
        let mut kept = totals(&t.st, &ids, &reqs, &no).unwrap();
        let vals = ["", "7", "1 250,00", "n/a", "2024-01-03", "2030-12-31", "2001-01-01", "0,5", "  ", "-4"];
        let (mut told, mut refused) = (0, 0);
        for step in 0..300usize {
            let (p, c) = ((step * 37) % ids.len(), [2, 3, 1, 0][step % 4]);
            let v = vals[(step * 7 + c) % vals.len()];
            let old = t.st.cell(ids[p], c);
            t.set_cells(&[(ids[p], c, v.into())]);
            let full = totals(&t.st, &ids, &reqs[c..c + 1], &no).unwrap().remove(0);
            if kept[c].edit(&old, v) {
                told += 1;
                kept[c].dec = if kinds[c] == Kind::Number { total_decimals(&t.st, ids.iter().copied(), c) } else { 0 };
                let k = &kept[c];
                assert!((k.sum - full.sum).abs() < 1e-6 * full.sum.abs().max(1.0), "step {step}");
                assert_eq!((k.n, k.filled, k.empty, k.bad, k.min, k.max, &k.min_s, &k.max_s, k.dec), (full.n, full.filled, full.empty, full.bad, full.min, full.max, &full.min_s, &full.max_s, full.dec), "step {step}: {old:?} → {v:?} in {c}");
                kept[c].sum = full.sum;
            } else {
                refused += 1;
                kept[c] = full;
            }
        }
        assert!(told > 200 && refused > 0, "{told} told, {refused} refused");
    }

    #[test]
    fn panel_profile_and_values() {
        let t = table("v;n\n b ;3\na;1\nb;2\n;2\nA;x\nlot 10;4\nlot 2;\n");
        let no = AtomicBool::new(false);
        let ids = all(&t);
        let vc = value_counts(&t.st, &ids, 0, false, &no).unwrap();
        assert_eq!(vc.iter().collect::<Vec<_>>(), vec![(" b ", 1), ("a", 1), ("b", 1), ("", 1), ("A", 1), ("lot 10", 1), ("lot 2", 1)]);
        let by: Vec<&str> = vc.by_value().iter().map(|&i| vc.value(i as usize)).collect();
        assert_eq!(by, vec!["", " b ", "a", "A", "b", "lot 2", "lot 10"]);
        assert_eq!(vc.search(&vc.by_count(), "LOT").len(), 2);
        let p = panel_profile(&vc);
        assert_eq!((p.kind, p.filled, p.empty, p.distinct), (Some(Kind::Text), 6, 1, 6));
        assert_eq!(p.min.unwrap().1, "b");
        assert_eq!(p.max.unwrap().1, "lot 10");
        let vc = value_counts(&t.st, &ids, 1, false, &no).unwrap();
        assert_eq!(vc.count_of("2"), 2);
        assert_eq!(vc.by_count()[0] as usize, 2);   // "2", twice
        let p = panel_profile(&vc);
        // 5 numbers of 6 filled: under 90 %, text.
        assert_eq!(p.kind, Some(Kind::Text));
        let t = table("n;x\n1;a\n2;a\n2;a\n10;a\n");
        let p = panel_profile(&value_counts(&t.st, &all(&t), 0, false, &no).unwrap());
        assert_eq!(p.kind, Some(Kind::Number));
        assert_eq!((p.sum, p.count, p.median), (15.0, 4, Some(2.0)));
        assert_eq!(p.hist.len(), PANEL_BINS);
        assert_eq!(p.hist[0], HistBin { count: 1, first: "1".into(), last: "1".into() });
        assert_eq!(p.hist[3].count, 2);   // floor((2 - 1) / 9 × 28)
        assert_eq!(p.hist[27].first, "10");
        // Dates: the middle one as written, the mean in days (not in YYYYMMDD keys).
        let t = table("d;x\n2024-01-30;a\n2024-02-02;a\n2024-02-03;a\n");
        let p = panel_profile(&value_counts(&t.st, &all(&t), 0, false, &no).unwrap());
        assert_eq!(p.kind, Some(Kind::Date));
        assert_eq!((p.median, p.date_median.as_deref(), p.date_mean.as_deref()), (None, Some("2024-02-02"), Some("2024-02-01")));
        // The bars by time: the last days of January beside the first of February.
        assert_eq!(p.hist.iter().rposition(|b| b.count > 0), Some(27));
        assert_eq!(p.hist[21].count, 1);   // the 2nd, 3 days of 4: floor(3 / 4 × 28)
    }

    #[test]
    fn value_counts_from_a_dictionary_are_the_same() {
        let mut csv = String::from("a,b\n");
        for i in 0..40_000 { csv.push_str(&format!("{},{i}\n", ["Lyon", " Lyon", "Paris", "", "Nice "][i % 5])); }
        let edit = |t: &mut Table| t.set_cells(&[(7, 0, "Brest".into()), (8, 0, "Paris".into())]);
        // Two tables of the same bytes: one without dictionary, one with (the caches are per file).
        let (mut plain, mut fast) = (table(&csv), table(&csv));
        edit(&mut plain);
        edit(&mut fast);
        let no = AtomicBool::new(false);
        crate::colcache::ensure(&fast.st, 0, crate::colcache::Need::Dict, &no).unwrap();
        let ids: Vec<RowId> = all(&plain).into_iter().rev().collect();
        let dump = |t: &Table, trim: bool| value_counts(&t.st, &ids, 0, trim, &no).unwrap().iter().map(|(s, n)| format!("{s:?}={n}")).collect::<Vec<_>>();
        for trim in [false, true] {
            assert!(crate::colcache::get(&plain.st, 0).dict.is_none());
            assert_eq!(dump(&plain, trim), dump(&fast, trim));
        }
    }

    #[test]
    fn profile_in_one_pass_is_profile_by_column() {
        let mut t = table("a,b,c,d\n1,\"x, y\",2024-01-02,é\n2,short\n , \"q\"\"q\",2023-12-31,z\n4,x,,\n1.5,yy,2024-01-02,é\n");
        t.set_cells(&[(2, 3, "edited".into()), (3, 0, " 7 ".into())]);
        let ids = all(&t);
        let no = AtomicBool::new(false);
        let by_col: Vec<String> = (0..4).map(|c| format!("{:?}", profile_col(&t.st, &ids, c, &no).unwrap())).collect();
        for cols in [vec![0, 1, 2, 3], vec![1, 3], vec![2]] {
            let one = profile_cols(&t.st, &ids, &cols, &no).unwrap();
            for (p, &c) in one.iter().zip(&cols) { assert_eq!(format!("{p:?}"), by_col[c], "column {c} of {cols:?}"); }
        }
    }

    #[test]
    fn file_profile() {
        let t = table(&ventes());
        let no = AtomicBool::new(false);
        let p = profile(&t.st, &all(&t), &no).unwrap();
        assert_eq!(p[0].kind(), Some(Kind::Number));
        assert!(p[0].unique());
        assert_eq!(p[0].min.as_ref().unwrap().1, "1");
        assert_eq!(p[0].max.as_ref().unwrap().1, "120");
        assert_eq!(p[0].sum, 7260.0);
        assert_eq!((p[1].kind(), p[1].distinct), (Some(Kind::Text), 4));
        assert_eq!(p[1].short.as_deref(), Some("vip"));
        assert_eq!(p[1].long.as_deref(), Some("prospect"));
        assert_eq!((p[3].kind(), p[3].empty), (Some(Kind::Date), 12));
        assert_eq!(p[3].min.as_ref().unwrap().1, "2024-01-01");
        assert_eq!(p[2].kind(), Some(Kind::Number));
        assert_eq!(p[2].texts, 1);
    }

    #[test]
    fn selections() {
        let v: Vec<f64> = (0..1001).map(|i| ((i * 37) % 1001) as f64).collect();
        assert_eq!(quantiles(&mut v.clone(), 0.02, 0.98), (20.0, 980.0));
        assert_eq!(quantile(&mut v.clone(), 0.5), 500.0);
        assert_eq!(median(vec![4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(vec![]), None);
    }

    #[test]
    fn big_counts_merge_exactly() {
        // Enough rows for many chunks and shards: every count exact, order of first appearance kept.
        // (A title found again in its column would read as data: "flag" over "x".)
        let mut s = String::from("k;flag\n");
        for i in 0..200_000 { s += &format!("v{};x\n", (i * 7919) % 50_000); }
        let t = table(&s);
        let no = AtomicBool::new(false);
        let vc = value_counts(&t.st, &all(&t), 0, true, &no).unwrap();
        assert_eq!(vc.len(), 50_000);
        assert!(vc.iter().all(|(_, n)| n == 4));
        assert_eq!(vc.value(0), "v0");
        assert_eq!(vc.value(1), "v7919");
        let p = profile_col(&t.st, &all(&t), 0, &no).unwrap();
        assert_eq!(p.distinct, 50_000);
    }

    /// Timings on the bench files: `cargo test -p csvfab-engine --release perf_stats -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn perf_stats() {
        let dir = std::env::var("CSVFAB_BENCH").unwrap_or("../benchmark/data".into());
        for name in ["customers-1m.csv", "customers-5m.csv"] {
            let path = std::path::Path::new(&dir).join(name);
            if !path.exists() { eprintln!("{name}: missing"); continue; }
            let t0 = std::time::Instant::now();
            let t = Table::new(Arc::new(Base::open(&path, OpenOptions::default(), None).unwrap()), None);
            eprintln!("{name}: open {:?}, {} rows × {} cols", t0.elapsed(), t.st.len(), t.st.ncols);
            let no = AtomicBool::new(false);
            let ids = all(&t);
            let kinds = crate::view::detect_kinds(&t.st);
            let h = t.st.headers();
            let col = |n: &str| h.iter().position(|x| x == n).unwrap();
            let time = |what: &str, f: &dyn Fn() -> String| { let t0 = std::time::Instant::now(); let r = f(); eprintln!("  {what:<40} {:>9.1} ms  {r}", t0.elapsed().as_secs_f64() * 1e3); };
            for c in ["city", "email", "total_spent"] {
                time(&format!("value_counts {c}"), &|| { let vc = value_counts(&t.st, &ids, col(c), false, &no).unwrap(); format!("{} distinct", vc.len()) });
            }
            let vc = value_counts(&t.st, &ids, col("email"), false, &no).unwrap();
            time("by_count email", &|| format!("{}", vc.by_count().len()));
            time("panel_profile email", &|| format!("{:?}", panel_profile(&vc).kind));
            let vc = value_counts(&t.st, &ids, col("total_spent"), false, &no).unwrap();
            time("panel_profile total_spent", &|| format!("{:?}", panel_profile(&vc).median));
            time("profile (all columns)", &|| { let p = profile(&t.st, &ids, &no).unwrap(); format!("{} cols", p.len()) });
            let reqs: Vec<TotalReq> = [col("orders"), col("total_spent")].iter().map(|&c| TotalReq { col: c, kind: kinds[c], median: false, distinct: false }).collect();
            time("totals sum ×2", &|| { let r = totals(&t.st, &ids, &reqs, &no).unwrap(); r[1].value(TotalFn::Sum).unwrap().text });
            let reqs: Vec<TotalReq> = (0..t.st.ncols).map(|c| TotalReq { col: c, kind: kinds[c], median: false, distinct: false }).collect();
            time("totals all columns, counts", &|| { let r = totals(&t.st, &ids, &reqs, &no).unwrap(); format!("{}", r.len()) });
            let reqs = [TotalReq { col: col("total_spent"), kind: Kind::Number, median: true, distinct: true }];
            time("totals total_spent median+distinct", &|| { let r = totals(&t.st, &ids, &reqs, &no).unwrap(); format!("{:?} {:?}", r[0].median, r[0].distinct) });
            let half: Vec<RowId> = ids.iter().copied().filter(|i| i % 3 == 0).collect();
            let reqs = [TotalReq { col: col("total_spent"), kind: Kind::Number, median: false, distinct: false }];
            time("totals sum on a third of the rows", &|| totals(&t.st, &half, &reqs, &no).unwrap()[0].value(TotalFn::Sum).unwrap().text);
            use crate::sparkline::{spark_base, spark_filtered, refine, Pick, Sample};
            time("spark_base (JS sample)", &|| { let s = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap(); format!("{} rows read", s.s) });
            time("spark_base (every row)", &|| { let s = spark_base(&t.st, &kinds, Sample::All, &no).unwrap(); format!("{} rows read", s.s) });
            let sp = spark_base(&t.st, &kinds, Sample::Js, &no).unwrap();
            let lyon: Vec<RowId> = ids.iter().copied().filter(|&id| t.st.cell(id, col("city")) == "Lyon").collect();
            time("spark_filtered (JS sample, Lyon)", &|| { let f = spark_filtered(&t.st, &sp, &lyon, Sample::Js, &no).unwrap(); format!("{} of {}", f.s, f.n) });
            time("spark_filtered (every row, a third)", &|| { let f = spark_filtered(&t.st, &sp, &half, Sample::All, &no).unwrap(); format!("{} of {}", f.s, f.n) });
            let c = col("total_spent");
            time("refine total_spent bars 3..5", &|| format!("{} rows", refine(&t.st, &ids, c, &sp.keep(c, Pick::Bars(3, 5)), &no).unwrap().len()));
            time("heat total_spent", &|| format!("{:?}", crate::look::heat(&t.st, &ids, c, &no).unwrap().unwrap()));
            time("data_bars total_spent", &|| format!("{:?}", crate::look::data_bars(&t.st, &ids, c, &no).unwrap().unwrap()));
            let mut sorted = ids.clone();
            crate::view::sort(&t.st, &mut sorted, &[crate::view::SortKey { col: col("last_name"), desc: false }], &kinds, &no);
            let g = crate::look::StripGeom::new(900.0, 60.0, 28.0, sorted.len(), 0.0);
            time("strip labels (sorted by last_name)", &|| { let k = crate::look::sort_kind(&t.st, col("last_name")); crate::look::strip_labels(&t.st, &sorted, col("last_name"), k, &g.label_rows(sorted.len())).into_iter().map(|x| x.1).collect::<Vec<_>>().join("") });
            time("strip marks (every 7th row)", &|| { let b = crate::look::strip_marks(&sorted, &g, |id| (id % 7 == 0) as u8, &no).unwrap(); format!("{} runs", crate::look::mark_runs(&b).len()) });
            time("rich_cols + pills", &|| { let r = crate::look::rich_cols(&t.st, &kinds); let p = crate::look::pills_from_spark(&sp, &kinds, &r); format!("rich {:?} pills {:?}", r.iter().enumerate().filter_map(|(c, r)| r.as_ref().map(|r| (c, r.clone()))).collect::<Vec<_>>(), p.iter().enumerate().filter(|x| x.1.is_some()).map(|x| &h[x.0]).collect::<Vec<_>>()) });
        }
    }
}
