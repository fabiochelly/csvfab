//! Group by: the rows shown, grouped on one or more columns, a count per
//! group and, for the columns asked, a sum, average, min, max or number of
//! distinct values. The result opens as a new tab (`to_csv`). Keys compare as
//! in the lookup (exact, loose, slug); the first spelling met is the one shown.
//!
//! Parallel: each rayon job aggregates its slice of rows into its own map,
//! the maps are merged. Per group only numbers and row ids are kept (the
//! values shown are read back at the end), so millions of groups stay small.
//!
//! Also the helpers pivot and compare share: key matching, the collation of
//! labels, reading several cells of a row in one pass, numbers as text.

use crate::dupes::{js_space, slug_into};
use crate::table::{RowId, State};
use crate::text::{fold_into, parse_date, parse_number};
use crate::view::Kind;
use hashbrown::HashTable;
use rayon::prelude::*;
use std::cmp::Ordering;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AO};

/// How keys compare (the lookup's choices).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Match {
    Exact,
    /// Trimmed, case ignored.
    #[default]
    Loose,
    /// No case, accents nor symbols.
    Slug,
}

impl Match {
    /// Appends `v` as compared.
    #[inline]
    pub fn apply(self, v: &str, out: &mut String) {
        match self {
            Match::Exact => out.push_str(v),
            Match::Loose => {
                let t = v.trim_matches(js_space);
                if t.is_ascii() { out.extend(t.bytes().map(|b| b.to_ascii_lowercase() as char)) } else { out.extend(t.chars().flat_map(char::to_lowercase)) }
            }
            Match::Slug => slug_into(v, out),
        }
    }

    pub fn norm(self, v: &str) -> String {
        let mut s = String::with_capacity(v.len());
        self.apply(v, &mut s);
        s
    }
}

/// The hasher of every table here: fast, seeded once per table.
pub(crate) type Hasher = foldhash::fast::RandomState;
pub(crate) type HashMap<K, V> = hashbrown::HashMap<K, V, Hasher>;
pub(crate) type HashSet<T> = hashbrown::HashSet<T, Hasher>;

// ---- Collation ---------------------------------------------------------------

#[inline]
fn class(b: u8) -> u8 {
    match b {
        b' ' | b'\t' | b'\n' | b'\r' => 1,
        b'0'..=b'9' => 3,
        b'a'..=b'z' | b'A'..=b'Z' | 0x80.. => 4,
        _ => 2,
    }
}

/// A label's sort key: compared as bytes, labels order as with the web app's
/// collator (French, digit-aware, accents and case ignored) — spaces, then
/// punctuation, digits, letters; a digit run by its value ("Lot 2" < "Lot 10").
/// Each byte goes with its class; a digit run is its class, its length
/// (leading zeros aside), its digits. `fold` is scratch.
pub fn coll_key_into(s: &str, fold: &mut String, out: &mut Vec<u8>) {
    out.clear();
    fold.clear();
    fold_into(s, fold);
    let b = fold.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let mut k = i;
            while k < b.len() && b[k] == b'0' { k += 1 }
            let st = k;
            while k < b.len() && b[k].is_ascii_digit() { k += 1 }
            out.push(3);
            out.push((k - st).min(255) as u8);
            out.extend_from_slice(&b[st..k]);
            i = k;
        } else {
            out.push(class(b[i]));
            out.push(b[i]);
            i += 1;
        }
    }
}

pub fn coll_key(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 * s.len());
    coll_key_into(s, &mut String::new(), &mut out);
    out
}

// ---- Several cells of a row --------------------------------------------------

/// Reads a few columns of a row in one pass over its record (when it is still
/// the file's bytes): `v[k]` is the cell of `cols[k]`.
pub(crate) struct Reader {
    cols: Vec<usize>,
    /// Field → the slots it fills.
    slots: Vec<Vec<usize>>,
    tmp: Vec<u8>,
    pub v: Vec<String>,
}

impl Reader {
    pub fn new(cols: &[usize]) -> Reader {
        let max = cols.iter().copied().max().map_or(0, |m| m + 1);
        let mut slots = vec![Vec::new(); max];
        for (k, &c) in cols.iter().enumerate() { slots[c].push(k) }
        Reader { cols: cols.to_vec(), slots, tmp: Vec::new(), v: vec![String::new(); cols.len()] }
    }

    pub fn read(&mut self, st: &State, id: RowId) {
        let Reader { cols, slots, tmp, v } = self;
        v.iter_mut().for_each(String::clear);
        if !st.pristine(id) {
            for (k, &c) in cols.iter().enumerate() {
                st.with_cell(id, c, tmp, |s| v[k].push_str(s));
            }
            return;
        }
        let b = &st.base;
        let rec = b.record(id as usize);
        if b.dialect.lines {
            for &k in slots.first().map_or(&[][..], |s| s.as_slice()) { v[k].push_str(&crate::enc::decode(b.enc, rec)) }
            return;
        }
        crate::scan::visit_fields(rec, b.dialect.delim, tmp, |i, f| {
            let Some(ks) = slots.get(i) else { return false };
            if let Some((&k0, rest)) = ks.split_first() {
                v[k0].push_str(&crate::enc::decode(b.enc, f));
                for &k in rest { let (a, c) = (v[k0].clone(), &mut v[k]); c.push_str(&a) }
            }
            i + 1 < slots.len()
        });
    }
}

// ---- Numbers as text -----------------------------------------------------------

/// JavaScript's Math.round (halves up).
#[inline]
pub(crate) fn js_round(x: f64, scale: f64) -> f64 {
    (x * scale + 0.5).floor() / scale
}

/// JavaScript's String(number).
pub fn js_num(x: f64) -> String {
    if x.is_nan() { return "NaN".into() }
    if x.is_infinite() { return if x > 0.0 { "Infinity" } else { "-Infinity" }.into() }
    if x == 0.0 { return "0".into() }
    let a = x.abs();
    if !(1e-6..1e21).contains(&a) {
        let s = format!("{x:e}");
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    format!("{x}")
}

/// A number written as the app's formulas do: decimal comma when the file's
/// delimiter is ';'.
pub fn num_text(x: f64, comma: bool) -> String {
    let s = js_num(x);
    if comma { s.replacen('.', ",", 1) } else { s }
}

/// French display ("1 234,5", narrow no-break spaces), `min`–`max` decimals.
pub fn fr_num(x: f64, min: usize, max: usize) -> String {
    let s = format!("{:.*}", max, x.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut frac = frac.trim_end_matches('0');
    if frac.len() < min { frac = &s[s.len() - max..][..min] }
    let mut out = String::with_capacity(s.len() + 8);
    if x < 0.0 && (int.bytes().any(|b| b != b'0') || !frac.is_empty()) { out.push('-') }
    for (k, ch) in int.chars().enumerate() {
        if k > 0 && (int.len() - k) % 3 == 0 { out.push('\u{202f}') }
        out.push(ch);
    }
    if !frac.is_empty() {
        out.push(',');
        out.push_str(frac);
    }
    out
}

/// A field for a CSV written with `delim`: quoted when it holds the delimiter,
/// a quote or a line break.
pub(crate) fn csv_field(v: &str, delim: u8, out: &mut Vec<u8>) {
    if v.bytes().any(|b| b == delim || b == b'"' || b == b'\n' || b == b'\r') {
        out.push(b'"');
        out.extend_from_slice(v.replace('"', "\"\"").as_bytes());
        out.push(b'"');
    } else {
        out.extend_from_slice(v.as_bytes());
    }
}

/// A file name without the characters file systems refuse.
pub fn file_safe(name: &str) -> String {
    name.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect()
}

pub(crate) fn csv_bytes(rows: impl Iterator<Item = Vec<String>>, delim: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        for (k, v) in r.iter().enumerate() {
            if k > 0 { out.push(delim) }
            csv_field(v, delim, &mut out);
        }
        out.push(b'\n');
    }
    out
}

const AMOUNT: &[&str] = &["montant", "amount", "total", "totaux", "prix", "price", "tarif", "ca", "chiffre", "revenue", "revenu", "qty", "quantite",
    "quantity", "nb", "nombre", "count", "somme", "sum", "cout", "cost", "budget", "salaire", "salary", "poids", "weight", "volume", "surface",
    "duree", "duration", "heures", "hours", "points", "score", "solde", "balance", "ht", "ttc", "tva", "vat", "remise", "discount", "marge", "margin"];

/// Whether a column is summed by default: numbers that read as a quantity —
/// an amount-like title, or decimals among its values — not an ID, a phone
/// number or a postal code.
pub fn sum_by_default(st: &State, headers: &[String], kinds: &[Kind], c: usize) -> bool {
    if kinds.get(c) != Some(&Kind::Number) { return false }
    if headers.get(c).is_some_and(|h| crate::dupes::slug(h).split(' ').any(|w| AMOUNT.contains(&w))) { return true }
    let (mut seen, mut dec, mut codes) = (0, 0, 0);
    let mut tmp = Vec::new();
    for p in 0..st.len() {
        st.with_cell(st.data_id(p), c, &mut tmp, |v| {
            let v = v.trim_matches(js_space).as_bytes();
            if v.is_empty() { return }
            if v.windows(2).any(|w| matches!(w[0], b'.' | b',') && w[1].is_ascii_digit()) { dec += 1 }
            // A leading zero, or nine digits and more: a code, a phone.
            if (v[0] == b'0' && v.get(1).is_some_and(u8::is_ascii_digit)) || v.iter().filter(|b| b.is_ascii_digit()).count() >= 9 { codes += 1 }
            seen += 1;
        });
        if seen >= 400 { break }
    }
    seen > 0 && dec > 0 && codes == 0
}

// ---- Interning ------------------------------------------------------------------

/// Keys numbered in the order met, their bytes end to end in one arena (no
/// allocation per key), with the position of the first row of each.
#[derive(Default)]
pub(crate) struct Interner {
    table: HashTable<u32>,
    bytes: Vec<u8>,
    ends: Vec<usize>,
    pub first: Vec<u32>,
}

impl Interner {
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    #[inline]
    pub fn get(&self, i: usize) -> &[u8] {
        span(&self.bytes, &self.ends, i)
    }

    /// Keys are only ever added from `&str`s.
    pub fn str(&self, i: usize) -> &str {
        std::str::from_utf8(self.get(i)).unwrap_or_default()
    }

    /// The key's number, and whether it is new.
    #[inline]
    pub fn id(&mut self, h: &Hasher, key: &[u8], pos: u32) -> (u32, bool) {
        let hash = h.hash_one(key);
        let Interner { table, bytes, ends, first } = self;
        if let Some(&i) = table.find(hash, |&i| span(bytes, ends, i as usize) == key) {
            first[i as usize] = first[i as usize].min(pos);
            return (i, false);
        }
        let i = ends.len() as u32;
        bytes.extend_from_slice(key);
        ends.push(bytes.len());
        first.push(pos);
        table.insert_unique(hash, i, |&i| h.hash_one(span(bytes, ends, i as usize)));
        (i, true)
    }
}

#[inline]
fn span<'a>(bytes: &'a [u8], ends: &[usize], i: usize) -> &'a [u8] {
    &bytes[if i == 0 { 0 } else { ends[i - 1] }..ends[i]]
}

// ---- Group by --------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aggs {
    pub sum: bool,
    pub avg: bool,
    pub min: bool,
    pub max: bool,
    pub distinct: bool,
}

impl Aggs {
    pub const SUM: Aggs = Aggs { sum: true, avg: false, min: false, max: false, distinct: false };

    fn any(self) -> bool {
        self.sum || self.avg || self.min || self.max || self.distinct
    }

    fn order(self) -> [bool; 5] {
        [self.sum, self.avg, self.min, self.max, self.distinct]
    }
}

const LABELS: [&str; 5] = ["sum", "average", "min", "max", "distinct"];
/// Distinct values counted per group, at most (a memory guard).
const DISTINCT_MAX: usize = 200_000;

#[derive(Clone, Debug, Default)]
pub struct GroupSpec {
    /// The columns grouped on, in that order.
    pub by: Vec<usize>,
    /// The columns aggregated (a grouped column is not).
    pub aggs: Vec<(usize, Aggs)>,
    pub how: Match,
}

#[derive(Clone, Default)]
struct Acc {
    sum: f64,
    cnt: u64,
    /// Smallest / largest: (number, position); text: (sort key, position).
    min: Option<(f64, u32)>,
    max: Option<(f64, u32)>,
    tmin: Option<(Box<[u8]>, u32)>,
    tmax: Option<(Box<[u8]>, u32)>,
    /// Distinct values (counted from Local::seen at the end).
    distinct: u32,
}

impl Acc {
    /// All but the distinct values, which merge through the sets.
    fn merge(&mut self, o: Acc) {
        self.sum += o.sum;
        self.cnt += o.cnt;
        // Ties go to the first row met, as in a pass in order.
        let better = |a: &Option<(f64, u32)>, b: &Option<(f64, u32)>, lt: bool| match (a, b) {
            (_, None) => false,
            (None, _) => true,
            (Some((x, p)), Some((y, q))) => if lt { y < x || (y == x && q < p) } else { y > x || (y == x && q < p) },
        };
        if better(&self.min, &o.min, true) { self.min = o.min }
        if better(&self.max, &o.max, false) { self.max = o.max }
        let tbetter = |a: &Option<(Box<[u8]>, u32)>, b: &Option<(Box<[u8]>, u32)>, want: Ordering| match (a, b) {
            (_, None) => false,
            (None, _) => true,
            (Some((x, p)), Some((y, q))) => { let o = y.cmp(x); o == want || (o == Ordering::Equal && q < p) }
        };
        if tbetter(&self.tmin, &o.tmin, Ordering::Less) { self.tmin = o.tmin }
        if tbetter(&self.tmax, &o.tmax, Ordering::Greater) { self.tmax = o.tmax }
    }
}

/// One job's groups, flat: no allocation per group.
#[derive(Default)]
struct Local {
    keys: Interner,
    n: Vec<u64>,
    /// `k` per group (k: the columns aggregated).
    accs: Vec<Acc>,
    /// (group, column aggregated, value's hash): the distinct values of every group.
    seen: HashSet<(u32, u32, u64)>,
}

impl Local {
    fn group(&mut self, h: &Hasher, key: &[u8], pos: u32, k: usize) -> usize {
        let (g, new) = self.keys.id(h, key, pos);
        if new {
            self.n.push(0);
            self.accs.resize_with(self.accs.len() + k, Acc::default);
        }
        g as usize
    }

    fn merge(mut self, mut o: Local, h: &Hasher, k: usize) -> Local {
        if self.keys.len() < o.keys.len() { std::mem::swap(&mut self, &mut o) }
        let map: Vec<usize> = (0..o.keys.len()).map(|i| self.group(h, o.keys.get(i), o.keys.first[i], k)).collect();
        let mut accs = std::mem::take(&mut o.accs).into_iter();
        for (i, &g) in map.iter().enumerate() {
            self.n[g] += o.n[i];
            for j in 0..k { self.accs[g * k + j].merge(accs.next().unwrap()) }
        }
        // Counted once at the end: a merge is a plain union.
        self.seen.extend(o.seen.into_iter().map(|(g, j, x)| (map[g as usize] as u32, j, x)));
        self
    }
}

struct AggOut {
    sum: f64,
    cnt: u64,
    min: Option<RowId>,
    max: Option<RowId>,
    distinct: usize,
}

/// A group-by result: groups by decreasing count (ties: first met first).
pub struct Grouped {
    st: State,
    by: Vec<usize>,
    /// (column, what, its kind), by column.
    aggs: Vec<(usize, Aggs, Kind)>,
    header: Vec<String>,
    /// Per group: count, first row.
    heads: Vec<(u64, RowId)>,
    /// Per group, each aggregated column (aggs.len() a group).
    outs: Vec<AggOut>,
}

const CH: usize = 1 << 14;
/// Parts of the rows when keys are many (by their hash's top byte).
const PARTS: usize = 256;

/// Aggregates rows in parallel. Few keys (a sample of the first rows says):
/// each rayon job fills its own state (`local`), the states are merged. Many
/// keys: each job's state would hold most of them, and merging would cost
/// more than the work — the rows are then split into parts by their key's
/// hash (`key`, a first pass), and each part is aggregated alone: the parts
/// returned hold different keys. `add` gets the row's position in `ids`.
/// `progress` counts rows (each pass half of them when there are two).
#[allow(clippy::too_many_arguments)]
pub(crate) fn aggregate<L: Send, S: Send>(
    ids: &[RowId],
    local: impl Fn() -> L + Sync + Send,
    scratch: impl Fn() -> S + Sync + Send,
    key: impl Fn(&mut S, RowId) -> u64 + Sync,
    add: impl Fn(&mut L, &mut S, u32, RowId) + Sync,
    merge: impl Fn(L, L) -> L + Sync + Send,
    cancel: &AtomicBool,
    progress: Option<&AtomicUsize>,
) -> Option<Vec<L>> {
    let tick = |n: usize| if let Some(p) = progress { p.fetch_add(n, AO::Relaxed); };
    let sample = &ids[..ids.len().min(CH)];
    let many = ids.len() > 4 * CH && {
        let mut s = scratch();
        let seen: HashSet<u64> = sample.iter().map(|&id| key(&mut s, id)).collect();
        seen.len() * 8 > sample.len()
    };
    if !many {
        let l = ids
            .par_chunks(CH)
            .enumerate()
            .fold(
                || (local(), scratch()),
                |(mut l, mut s), (ci, chunk)| {
                    if !cancel.load(AO::Relaxed) {
                        for (k, &id) in chunk.iter().enumerate() { add(&mut l, &mut s, (ci * CH + k) as u32, id) }
                    }
                    tick(chunk.len());
                    (l, s)
                },
            )
            .map(|t| t.0)
            .reduce_with(&merge);
        return (!cancel.load(AO::Relaxed)).then(|| l.into_iter().collect());
    }
    let tags: Vec<u8> = ids
        .par_chunks(CH)
        .map_init(&scratch, |s, chunk| {
            if cancel.load(AO::Relaxed) { return Vec::new() }
            tick(chunk.len() / 2);
            chunk.iter().map(|&id| (key(s, id) >> 56) as u8).collect::<Vec<u8>>()
        })
        .flat_map_iter(|v| v)
        .collect();
    if cancel.load(AO::Relaxed) || tags.len() != ids.len() { return None }
    // A counting sort: each part's positions, in order.
    let mut starts = [0usize; PARTS + 1];
    for &t in &tags { starts[t as usize + 1] += 1 }
    for k in 0..PARTS { starts[k + 1] += starts[k] }
    let mut fill = starts;
    let mut pos = vec![0u32; ids.len()];
    for (p, &t) in tags.iter().enumerate() {
        pos[fill[t as usize]] = p as u32;
        fill[t as usize] += 1;
    }
    drop(tags);
    let parts: Vec<L> = (0..PARTS)
        .into_par_iter()
        .map_init(&scratch, |s, k| {
            let mut l = local();
            let part = &pos[starts[k]..starts[k + 1]];
            if cancel.load(AO::Relaxed) { return l }
            for &p in part { add(&mut l, s, p, ids[p as usize]) }
            tick(part.len() - part.len() / 2);
            l
        })
        .collect();
    (!cancel.load(AO::Relaxed)).then_some(parts)
}

/// Groups `ids` (the rows shown). None if cancelled. `progress` counts rows read.
pub fn group_by(st: &State, ids: &[RowId], spec: &GroupSpec, kinds: &[Kind], cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Grouped> {
    let mut aggs: Vec<(usize, Aggs, Kind)> = spec.aggs.iter()
        .filter(|(c, a)| a.any() && !spec.by.contains(c))
        .map(|&(c, a)| (c, a, kinds.get(c).copied().unwrap_or(Kind::Text)))
        .collect();
    aggs.sort_by_key(|a| a.0);
    aggs.dedup_by_key(|a| a.0);
    let cols: Vec<usize> = spec.by.iter().copied().chain(aggs.iter().map(|a| a.0)).collect();
    let (nb, k) = (spec.by.len(), aggs.len());
    let hasher = Hasher::default();
    let key_of = |rd: &mut Reader, key: &mut String, id: RowId| {
        rd.read(st, id);
        key.clear();
        for (j, v) in rd.v[..nb].iter().enumerate() {
            if j > 0 { key.push('\u{1}') }
            spec.how.apply(v, key);
        }
    };
    let parts = aggregate(
        ids,
        Local::default,
        || (Reader::new(&cols), String::new(), String::new(), Vec::new()),
        |(rd, key, _, _), id| { key_of(rd, key, id); hasher.hash_one(key.as_bytes()) },
        |l, (rd, key, fold, ck), pos, id| {
            key_of(rd, key, id);
            let g = l.group(&hasher, key.as_bytes(), pos, k);
            l.n[g] += 1;
            for (j, &(_, a, kind)) in aggs.iter().enumerate() {
                let v = rd.v[nb + j].trim_matches(js_space);
                if v.is_empty() { continue }
                if a.distinct { l.seen.insert((g as u32, j as u32, hasher.hash_one(v))); }
                if !(a.sum || a.avg || a.min || a.max) { continue }
                let acc = &mut l.accs[g * k + j];
                if kind == Kind::Text {
                    coll_key_into(v, fold, ck);
                    let f = ck.as_slice();
                    if acc.tmin.as_ref().is_none_or(|(m, _)| f < &m[..]) { acc.tmin = Some((f.into(), pos)) }
                    if acc.tmax.as_ref().is_none_or(|(m, _)| f > &m[..]) { acc.tmax = Some((f.into(), pos)) }
                    continue;
                }
                let Some(x) = (if kind == Kind::Number { parse_number(v) } else { parse_date(v) }) else { continue };
                if kind == Kind::Number { acc.sum += x; acc.cnt += 1; }
                if acc.min.is_none_or(|(m, _)| x < m) { acc.min = Some((x, pos)) }
                if acc.max.is_none_or(|(m, _)| x > m) { acc.max = Some((x, pos)) }
            }
        },
        |a, b| a.merge(b, &hasher, k),
        cancel,
        progress,
    )?;
    let mut parts = parts;
    parts.par_iter_mut().for_each(|l| {
        // Counted by sorting: a group's values in a column are a run of (group, column, …).
        let mut t: Vec<(u32, u32, u64)> = std::mem::take(&mut l.seen).into_iter().collect();
        t.par_sort_unstable();
        for run in t.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
            l.accs[run[0].0 as usize * k + run[0].1 as usize].distinct = run.len().min(DISTINCT_MAX) as u32;
        }
    });
    // The parts hold different keys: their groups are simply put together.
    let mut order: Vec<(u64, u32, u32, u32)> = parts.par_iter().enumerate()
        .flat_map_iter(|(pi, l)| (0..l.n.len()).map(move |g| (l.n[g], l.keys.first[g], pi as u32, g as u32)))
        .collect();
    order.par_sort_unstable_by_key(|g| (std::cmp::Reverse(g.0), g.1));
    let at = |p: Option<u32>| p.map(|p| ids[p as usize]);
    let heads: Vec<(u64, RowId)> = order.par_iter().map(|g| (g.0, ids[g.1 as usize])).collect();
    let outs: Vec<AggOut> = order.par_iter().flat_map_iter(|&(_, _, pi, g)| {
        let g = g as usize;
        parts[pi as usize].accs[g * k..(g + 1) * k].iter().zip(&aggs).map(|(a, &(_, _, kind))| {
            let (min, max) = if kind == Kind::Text { (a.tmin.as_ref().map(|m| m.1), a.tmax.as_ref().map(|m| m.1)) } else { (a.min.map(|m| m.1), a.max.map(|m| m.1)) };
            AggOut { sum: a.sum, cnt: a.cnt, min: at(min), max: at(max), distinct: a.distinct as usize }
        })
    }).collect();
    parts.into_par_iter().for_each(drop);
    let titles = st.headers();
    let title = |c: usize| titles.get(c).cloned().unwrap_or_default();
    let mut header: Vec<String> = spec.by.iter().map(|&c| title(c)).collect();
    header.push("Count".into());
    for &(c, a, _) in &aggs {
        for (on, label) in a.order().into_iter().zip(LABELS) {
            if on { header.push(format!("{} ({label})", title(c))) }
        }
    }
    Some(Grouped { st: st.clone(), by: spec.by.clone(), aggs, header, heads, outs })
}

impl Grouped {
    pub fn len(&self) -> usize {
        self.heads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }

    pub fn header(&self) -> &[String] {
        &self.header
    }

    /// Group i's line: its values (as first met), the count, the aggregates.
    /// Numbers get a decimal comma when `delim` is ';'.
    pub fn row(&self, i: usize, delim: u8, out: &mut Vec<String>) {
        out.clear();
        let comma = delim == b';';
        let num = |x: f64| num_text(js_round(x, 1e6), comma);
        let (n, first) = &self.heads[i];
        let k = self.aggs.len();
        let aggs = &self.outs[i * k..(i + 1) * k];
        let mut tmp = Vec::new();
        for &c in &self.by { out.push(self.st.with_cell(*first, c, &mut tmp, |s| s.to_string())) }
        out.push(n.to_string());
        let cell = |id: Option<RowId>, c: usize, tmp: &mut Vec<u8>| id.map_or(String::new(), |id| self.st.with_cell(id, c, tmp, |s| s.trim_matches(js_space).to_string()));
        for (a, &(c, w, kind)) in aggs.iter().zip(&self.aggs) {
            if w.sum { out.push(if kind == Kind::Number { num(a.sum) } else { String::new() }) }
            if w.avg { out.push(if a.cnt > 0 { num(a.sum / a.cnt as f64) } else { String::new() }) }
            if w.min { out.push(cell(a.min, c, &mut tmp)) }
            if w.max { out.push(cell(a.max, c, &mut tmp)) }
            if w.distinct { out.push(a.distinct.to_string()) }
        }
    }

    /// The new tab's name: "<stem> — by <columns>.csv".
    pub fn tab_name(&self, stem: &str) -> String {
        let h = self.st.headers();
        let cols: Vec<&str> = self.by.iter().map(|&c| h.get(c).map_or("", |s| s.as_str())).collect();
        file_safe(&format!("{stem} — by {}.csv", cols.join(", ")))
    }

    /// The whole result as a CSV file's bytes (UTF-8), for a new tab.
    pub fn to_csv(&self, delim: u8) -> Vec<u8> {
        let mut out = csv_bytes(std::iter::once(self.header.clone()), delim);
        let body: Vec<Vec<u8>> = (0..self.len()).into_par_iter().chunks(4096).map(|is| {
            let mut r = Vec::new();
            csv_bytes(is.into_iter().map(|i| { self.row(i, delim, &mut r); std::mem::take(&mut r) }), delim)
        }).collect();
        body.iter().for_each(|b| out.extend_from_slice(b));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use crate::view::detect_kinds;
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn lines(g: &Grouped, delim: u8) -> Vec<Vec<String>> {
        (0..g.len()).map(|i| { let mut r = Vec::new(); g.row(i, delim, &mut r); r }).collect()
    }

    #[test]
    fn helpers() {
        assert_eq!(js_num(1.0), "1");
        assert_eq!(js_num(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(js_num(-0.0), "0");
        assert_eq!(js_num(1e21), "1e+21");
        assert_eq!(js_num(1.5e-7), "1.5e-7");
        assert_eq!(js_round(2.0000004, 1e6), 2.0);
        assert_eq!(js_round(-2.5, 1.0), -2.0);
        assert_eq!(num_text(10.5, true), "10,5");
        assert_eq!(fr_num(229659.7, 2, 2), "229\u{202f}659,70");
        assert_eq!(fr_num(1234.5678, 0, 3), "1\u{202f}234,568");
        assert_eq!(fr_num(-12.0, 0, 3), "-12");
        assert_eq!(fr_num(100.0, 0, 3), "100");
        let k = coll_key;
        assert!(k("Lot 2") < k("lot 10"));
        assert!(k("Lot 02") == k("lot 2"));
        assert!(k("Lot 2b") > k("lot 2"));
        assert_eq!(k("Émile"), k("emile"));
        assert!(k("Zoé") < k("zoe b"));
        assert!(k("a-b") < k("a1"));
        assert!(k("a1") < k("ab"));
        assert!(k("a b") < k("a-b"));
        assert_eq!(Match::Loose.norm("  ÉMILE "), "émile");
        assert_eq!(Match::Slug.norm("  ÉMILE! "), "emile");
    }

    #[test]
    fn groups_like_the_web_app() {
        let t = table("city;status;amount;date;name\nParis;vip;10,50;15/01/2024;b\nparis ;actif;5;20/01/2024;a\nLyon;vip;2;03/02/2024;c\nLyon;vip;abc;04/02/2024;C\n;actif;1;;\n");
        let no = AtomicBool::new(false);
        let mut kinds = detect_kinds(&t.st);
        kinds[2] = Kind::Number;   // "abc" makes it text by sampling; as the app's 90 % rule would not
        let ids: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let all = Aggs { sum: true, avg: true, min: true, max: true, distinct: true };
        let spec = GroupSpec { by: vec![0], aggs: vec![(2, all), (3, Aggs { min: true, max: true, ..Default::default() }), (4, all), (0, Aggs::SUM)], how: Match::Loose };
        let g = group_by(&t.st, &ids, &spec, &kinds, &no, None).unwrap();
        assert_eq!(g.header(), &["city", "Count", "amount (sum)", "amount (average)", "amount (min)", "amount (max)", "amount (distinct)",
            "date (min)", "date (max)", "name (sum)", "name (average)", "name (min)", "name (max)", "name (distinct)"]);
        assert_eq!(lines(&g, b';'), vec![
            vec!["Paris", "2", "15,5", "7,75", "5", "10,50", "2", "15/01/2024", "20/01/2024", "", "", "a", "b", "2"],
            vec!["Lyon", "2", "2", "2", "2", "2", "2", "03/02/2024", "04/02/2024", "", "", "c", "c", "2"],
            vec!["", "1", "1", "1", "1", "1", "1", "", "", "", "", "", "", "0"],
        ]);
        let csv = String::from_utf8(g.to_csv(b',')).unwrap();
        assert!(csv.starts_with("city,Count,amount (sum),"));
        assert!(csv.contains("\nParis,2,15.5,7.75,5,\"10,50\",2,"));
        // Exact keys: "Paris" and "paris " apart.
        let spec = GroupSpec { by: vec![0, 1], aggs: vec![], how: Match::Exact };
        let g = group_by(&t.st, &ids, &spec, &kinds, &no, None).unwrap();
        assert_eq!(lines(&g, b','), vec![vec!["Lyon", "vip", "2"], vec!["Paris", "vip", "1"], vec!["paris ", "actif", "1"], vec!["", "actif", "1"]]);
    }

    #[test]
    fn sums_by_default() {
        let t = table("id,price,cp,qty\n1,2.50,75011,3\n2,3.10,06000,4\n");
        let kinds = detect_kinds(&t.st);
        let h = t.st.headers();
        let got: Vec<bool> = (0..4).map(|c| sum_by_default(&t.st, &h, &kinds, c)).collect();
        assert_eq!(got, vec![false, true, false, true]);
    }
}
