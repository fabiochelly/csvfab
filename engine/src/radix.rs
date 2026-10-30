//! Stable sorts of row positions by u64 keys, in parallel: one bucket pass on
//! the keys' high bits (a counting sort when the keys are few), then each
//! bucket sorted on its own, in cache. Ties keep their order, so sorting by
//! several keys is sorting by each, last key first.
//!
//! Text (`text_sort`) holds 16 bytes a row whatever its length: the first bytes
//! of each key and the row's position. Rows go straight to buckets of a sample's
//! making, each bucket is sorted in cache; only rows whose first bytes tie have
//! their whole keys made, on demand, from the file or the fold.

use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Positions 0..w.len() ordered by their key, ties in position order.
pub fn sort_perm(w: &[u64]) -> Vec<u32> {
    let n = w.len();
    if n < 2 {
        return (0..n as u32).collect();
    }
    let (lo, hi) = w
        .par_iter()
        .with_min_len(1 << 16)
        .fold(|| (u64::MAX, 0), |(a, b), &x| (a.min(x), b.max(x)))
        .reduce(|| (u64::MAX, 0), |x, y| (x.0.min(y.0), x.1.max(y.1)));
    if lo == hi {
        return (0..n as u32).collect();
    }
    let width = 64 - (hi - lo).leading_zeros();
    if width <= 32 {
        return sort_narrow(n, width, |i| w[i] - lo);
    }
    if n < 1 << 15 {
        let mut v: Vec<(u64, u32)> = w.iter().enumerate().map(|(i, &x)| (x, i as u32)).collect();
        v.sort_unstable();
        return v.into_iter().map(|x| x.1).collect();
    }
    let (bits, shift) = spread(n, width);
    let items = |i: usize| (w[i], i as u32);
    let pairs = scatter(n, 1 << bits, |i| ((w[i] - lo) >> shift) as usize, items);
    finish(pairs, shift, n, |x: &(u64, u32)| x.1)
}

/// `sort_perm` of keys of `width` bits at most (32 or fewer), given by `word`.
pub fn sort_narrow(n: usize, width: u32, word: impl Fn(usize) -> u64 + Sync) -> Vec<u32> {
    let items = |i: usize| (word(i) << 32) | i as u64;
    if n < 1 << 15 {
        // Small: one sort of (key, position) pairs; positions are distinct, so unstable is stable.
        let mut v: Vec<u64> = (0..n).map(items).collect();
        v.sort_unstable();
        return v.into_iter().map(|x| x as u32).collect();
    }
    let (bits, shift) = spread(n, width.max(1));
    let pairs = scatter(n, 1 << bits, |i| (word(i) >> shift) as usize, items);
    finish(pairs, shift, n, |x: &u64| *x as u32)
}

/// Bucket on the top bits: about 512 rows a bucket, at most 2^16 buckets. (Bits, their shift.)
fn spread(n: usize, width: u32) -> (u32, u32) {
    let bits = (usize::BITS - (n >> 9).leading_zeros()).clamp(4, 16).min(width);
    (bits, width - bits)
}

/// The items in bucket order, each bucket's items in position order (a stable
/// counting pass, in parallel over slices of positions), and where each bucket starts.
fn scatter<T: Copy + Send + Sync>(n: usize, nb: usize, bucket: impl Fn(usize) -> usize + Sync, item: impl Fn(usize) -> T + Sync) -> (Vec<T>, Vec<usize>) {
    let parts = (rayon::current_num_threads() * 2).clamp(1, 64).min(n.div_ceil(1 << 14));
    let size = n.div_ceil(parts);
    let mut counts: Vec<Vec<u32>> = (0..parts)
        .into_par_iter()
        .map(|p| {
            let mut c = vec![0u32; nb];
            for i in p * size..((p + 1) * size).min(n) {
                c[bucket(i)] += 1;
            }
            c
        })
        .collect();
    // Bucket b of slice p goes after bucket b of the slices before it.
    let mut starts = Vec::with_capacity(nb + 1);
    let mut at = 0usize;
    for b in 0..nb {
        starts.push(at);
        for c in counts.iter_mut() {
            let k = c[b] as usize;
            c[b] = at as u32;
            at += k;
        }
    }
    starts.push(at);
    let mut out: Vec<T> = Vec::with_capacity(n);
    let dst = SendPtr(out.as_mut_ptr());
    counts.into_par_iter().enumerate().for_each(|(p, mut off)| {
        let dst = &dst;
        for i in p * size..((p + 1) * size).min(n) {
            let b = bucket(i);
            // SAFETY: each (slice, bucket) owns a disjoint range of out, all of 0..n covered once.
            unsafe { dst.0.add(off[b] as usize).write(item(i)) };
            off[b] += 1;
        }
    });
    // SAFETY: every index below n was written above.
    unsafe { out.set_len(n) };
    (out, starts)
}

struct SendPtr<T>(*mut T);
unsafe impl<T> Sync for SendPtr<T> {}
unsafe impl<T> Send for SendPtr<T> {}

/// Sorts each bucket (unless all its keys are equal: shift 0) and returns the positions.
fn finish<T: Ord + Copy + Send + Sync>((mut v, starts): (Vec<T>, Vec<usize>), shift: u32, n: usize, pos: impl Fn(&T) -> u32 + Sync + Send) -> Vec<u32> {
    if shift > 0 {
        let mut rest = v.as_mut_slice();
        let mut slices = Vec::with_capacity(starts.len());
        for k in 1..starts.len() {
            let (a, b) = rest.split_at_mut(starts[k] - starts[k - 1]);
            if a.len() > 1 { slices.push(a); }
            rest = b;
        }
        slices.into_par_iter().for_each(|s| if s.len() > n / 16 { s.par_sort_unstable() } else { s.sort_unstable() });
    }
    v.par_iter().with_min_len(1 << 14).map(|x| pos(x)).collect()
}

/// (key, position) pairs in the order `sort_unstable` gives them, in parallel: one bucket pass
/// on the keys' top bits (spread keys: hashes), then each bucket sorted on its own, in cache.
pub fn sort_pairs(v: &mut Vec<(u64, u32)>) {
    let n = v.len();
    if n < 1 << 15 {
        v.sort_unstable();
        return;
    }
    let bits = (usize::BITS - (n >> 9).leading_zeros()).clamp(4, 16);
    let shift = 64 - bits;
    let src = &*v;
    let (mut out, starts) = scatter(n, 1 << bits, |i| (src[i].0 >> shift) as usize, |i| src[i]);
    let mut rest = out.as_mut_slice();
    let mut slices = Vec::with_capacity(starts.len());
    for k in 1..starts.len() {
        let (a, b) = rest.split_at_mut(starts[k] - starts[k - 1]);
        if a.len() > 1 { slices.push(a); }
        rest = b;
    }
    slices.into_par_iter().for_each(|s| if s.len() > n / 16 { s.par_sort_unstable() } else { s.sort_unstable() });
    *v = out;
}

/// A key's first 8 bytes, big-endian, zero padded: ordered as the bytes are
/// (equal prefixes still need the whole keys compared).
#[inline]
pub fn prefix(k: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    let m = k.len().min(8);
    b[..m].copy_from_slice(&k[..m]);
    u64::from_be_bytes(b)
}

/// A row while its text is sorted: its key's head in the top 96 bits (`text::head_of`), its
/// position in the low 32. Sorted as numbers, equal heads stay in position order.
type Item = u128;

#[inline]
fn pos(x: Item) -> usize {
    x as u32 as usize
}

/// Equal heads.
#[inline]
fn same(a: Item, b: Item) -> bool {
    (a ^ b) >> 32 == 0
}

/// The head holds the end of the key: heads equal, keys equal.
#[inline]
fn ended(x: Item) -> bool {
    x >> 32 & 0xFF == 0
}

/// Keys a run of this many bytes at most are held at once to be compared, per thread; a run
/// with more is sorted in place, `HEAD` more bytes at a time.
const RUN_BYTES: usize = if cfg!(test) { 1 << 12 } else { 1 << 26 };

/// Items in a bucket, about: in a core's cache.
const BUCKET: usize = 1 << 15;
const MAX_BUCKETS: usize = 1 << 12;
/// Items per block of a bucket, and per thread and bucket before they go there.
const BLOCK: usize = 1 << 12;
const LOCAL: usize = 1 << 6;

/// What a text sort reads of item `i`, on demand: `head`, its key's head (of all of them,
/// once); `key`, the whole key (when heads tie); `tie`, what tells equal keys apart
/// (`text::tie_key_into`); `alike`: of equal keys, those of one non-zero number tie as equals,
/// not read.
pub struct Text<H, K, A, T> {
    pub head: H,
    pub key: K,
    pub alike: A,
    pub tie: T,
}

/// The sorted items of a text sort, ties in position order, empty keys last.
pub struct Sorted {
    pub perm: Vec<u32>,
    /// A bit per place of `perm`: an item unlike the one before (its key, or its ties).
    new: Vec<u64>,
    /// Items with a key, before the empty ones.
    filled: usize,
    /// Values told apart among them.
    pub distinct: u32,
}

impl Sorted {
    /// Each item's dense rank: equal keys of equal ties share one, the empty ones have `distinct`.
    pub fn ranks(&self) -> Vec<u32> {
        let n = self.perm.len();
        let mut rank = vec![0; n];
        let out = SendPtr(rank.as_mut_ptr());
        const WORDS: usize = 1 << 12;
        let before: Vec<u32> = self.new.chunks(WORDS).scan(0, |r, c| {
            let at = *r;
            *r += c.iter().map(|w| w.count_ones()).sum::<u32>();
            Some(at)
        }).collect();
        self.new.par_chunks(WORDS).zip(before).enumerate().for_each(|(c, (words, mut r))| {
            let out = &out;
            let at = c * WORDS * 64;
            for k in at..(at + words.len() * 64).min(self.filled) {
                r += (words[(k - at) / 64] >> (k % 64) & 1) as u32;
                // SAFETY: positions are distinct and below n; r ≥ 1 from the first item on.
                unsafe { out.0.add(self.perm[k] as usize).write(r - 1) };
            }
        });
        for &p in &self.perm[self.filled..] { rank[p as usize] = self.distinct; }
        rank
    }

    /// The order the other way round: the values reversed, each one's items still in position
    /// order, the empty ones still last.
    pub fn reversed(mut self) -> Vec<u32> {
        let filled = self.filled;
        self.perm[..filled].reverse();
        if self.distinct as usize != filled {
            // Runs of equal values turned back.
            let mut start = 0;
            for k in (1..=filled).filter(|&k| k == filled || self.new[k / 64] >> (k % 64) & 1 == 1) {
                self.perm[filled - k..filled - start].reverse();
                start = k;
            }
        }
        self.perm
    }
}

/// Where the items of a sort too large for one go wait for their bucket: blocks taken from
/// one allocation, a little more than the items' size, nothing copied twice as a bucket grows.
struct Pool {
    _mem: Vec<Item>,
    base: SendPtr<Item>,
    next: AtomicUsize,
    buckets: Vec<Mutex<Bucket>>,
}

#[derive(Default)]
struct Bucket {
    blocks: Vec<u32>,
    /// Items in all, the last block taking the ones past the others' full `BLOCK`.
    len: usize,
}

impl Pool {
    fn new(n: usize, nb: usize) -> Pool {
        let mut mem = Vec::with_capacity((n / BLOCK + nb + 1) * BLOCK);
        let base = SendPtr(mem.as_mut_ptr());
        Pool { _mem: mem, base, next: AtomicUsize::new(0), buckets: (0..nb).map(|_| Mutex::default()).collect() }
    }

    fn add(&self, bucket: usize, mut items: &[Item]) {
        let mut b = self.buckets[bucket].lock().unwrap_or_else(|e| e.into_inner());
        while !items.is_empty() {
            let used = b.len % BLOCK;
            if used == 0 { b.blocks.push(self.next.fetch_add(1, Ordering::Relaxed) as u32); }
            let k = items.len().min(BLOCK - used);
            let block = *b.blocks.last().unwrap() as usize;
            // SAFETY: a block goes to one bucket, under its lock; the blocks a bucket needs are
            // its items' number over BLOCK, rounded up: within the capacity asked in `new`.
            unsafe { std::ptr::copy_nonoverlapping(items.as_ptr(), self.base.0.add(block * BLOCK + used), k) };
            b.len += k;
            items = &items[k..];
        }
    }

    /// Bucket `b`'s items, copied to `out`.
    fn take(&self, b: &Bucket, out: &mut Vec<Item>) {
        out.clear();
        out.reserve(b.len);
        for (k, &block) in b.blocks.iter().enumerate() {
            let m = (b.len - k * BLOCK).min(BLOCK);
            // SAFETY: the first `m` items of this block were written by `add`, its writers done.
            out.extend_from_slice(unsafe { std::slice::from_raw_parts(self.base.0.add(block as usize * BLOCK), m) });
        }
    }
}

/// A thread's items on their way to the pool's buckets, handed over when `LOCAL` wait.
struct Local<'a> {
    pool: &'a Pool,
    items: Vec<Item>,
    len: Vec<u8>,
}

impl Local<'_> {
    #[inline]
    fn push(&mut self, bucket: usize, x: Item) {
        let k = self.len[bucket] as usize;
        self.items[bucket * LOCAL + k] = x;
        if k + 1 == LOCAL {
            self.pool.add(bucket, &self.items[bucket * LOCAL..(bucket + 1) * LOCAL]);
            self.len[bucket] = 0;
        } else {
            self.len[bucket] = k as u8 + 1;
        }
    }
}

impl Drop for Local<'_> {
    fn drop(&mut self) {
        for (bucket, &k) in self.len.iter().enumerate() {
            self.pool.add(bucket, &self.items[bucket * LOCAL..bucket * LOCAL + k as usize]);
        }
    }
}

/// Heads' first 8 bytes that split a sample of the items in `nb` (a power of 2) parts of equal
/// size, as a binary tree in an array (`bucket_of`): equal heads go together, however many.
fn splitters(n: usize, nb: usize, head: &(impl Fn(usize) -> u128 + Sync)) -> Vec<u64> {
    // Neighbours taken together: a file not yet in memory is read at few places.
    const NEAR: usize = 8;
    let places = (nb * 32 / NEAR).min(n / NEAR).max(1);
    let mut s: Vec<u64> = (0..places).into_par_iter().flat_map_iter(|p| {
        let at = p * (n - NEAR) / places;
        (at..at + NEAR).map(|i| (head(i) >> 64) as u64)
    }).collect();
    s.par_sort_unstable();
    let mut tree = vec![0; nb];
    fn fill(tree: &mut [u64], s: &[u64], node: usize, lo: usize, hi: usize) {
        if node >= tree.len() { return; }
        let mid = (lo + hi) / 2;
        tree[node] = s[mid * s.len() / tree.len()];
        fill(tree, s, node * 2, lo, mid);
        fill(tree, s, node * 2 + 1, mid, hi);
    }
    fill(&mut tree, &s, 1, 0, nb);
    tree
}

/// The bucket of a head: heads up to a splitter on its left, no branch to mispredict.
#[inline]
fn bucket_of(tree: &[u64], h: u128) -> usize {
    let h = (h >> 64) as u64;
    let mut i = 1;
    while i < tree.len() {
        i = i * 2 + (h > tree[i]) as usize;
    }
    i - tree.len()
}

/// Gives the items of equal keys and equal ties one mark, unlike all others' in the sort: the
/// first one's position, in a head of zeros (a key's starts with a byte + 1).
fn mark(run: &mut [Item]) {
    let m = (pos(run[0]) as Item) << 32;
    for x in run { *x = m | pos(*x) as Item; }
}

#[derive(Default)]
struct Scratch {
    keys: Vec<u8>,
    ends: Vec<usize>,
    idx: Vec<u32>,
    items: Vec<Item>,
}

thread_local! {
    static SCRATCH: std::cell::Cell<Option<Box<Scratch>>> = const { std::cell::Cell::new(None) };
}

/// Orders a run of items whose keys agree on their first `d` bytes (in position order), each set
/// of equal keys handed to `done`, every other item marked. Keys come on demand.
fn settle<K, D>(run: &mut [Item], d: usize, key: &K, done: &D)
where K: Fn(usize, &mut Vec<u8>) + Sync, D: Fn(&mut [Item]) + Sync {
    if run.len() * 8 <= RUN_BYTES {
        let mut s = SCRATCH.take().unwrap_or_default();
        let held = settle_held(run, d, key, done, &mut s);
        // A large run's room is not kept on the thread.
        if s.keys.capacity() <= 1 << 20 { SCRATCH.set(Some(s)); }
        if held { return; }
    }
    // Too much to hold: the next bytes of each key take the head's place.
    run.par_iter_mut().with_min_len(1 << 10).for_each_init(Vec::new, |buf, x| {
        buf.clear();
        key(pos(*x), buf);
        *x = crate::text::head_of(&buf[d.min(buf.len())..]) | pos(*x) as Item;
    });
    run.par_sort_unstable();
    run.par_chunk_by_mut(|a, b| same(*a, *b)).for_each(|g| {
        if g.len() == 1 {
            mark(g);
        } else if ended(g[0]) {
            done(g);
        } else {
            settle(g, d + crate::text::HEAD, key, done);
        }
    });
}

/// `settle` with the run's keys side by side. False: they take too much room, nothing done.
fn settle_held<K, D>(run: &mut [Item], d: usize, key: &K, done: &D, s: &mut Scratch) -> bool
where K: Fn(usize, &mut Vec<u8>) + Sync, D: Fn(&mut [Item]) + Sync {
    let Scratch { keys, ends, idx, items } = s;
    keys.clear();
    ends.clear();
    ends.push(0);
    for x in run.iter() {
        key(pos(*x), keys);
        ends.push(keys.len());
        if keys.len() > RUN_BYTES { return false; }
    }
    let (keys, ends) = (&*keys, &*ends);
    let k = |j: usize| &keys[(ends[j] + d).min(ends[j + 1])..ends[j + 1]];
    idx.clear();
    idx.extend(0..run.len() as u32);
    refine(idx, 0, &k);
    items.clear();
    items.extend(idx.iter().map(|&j| run[j as usize]));
    run.copy_from_slice(items);
    let mut start = 0;
    for e in 1..=run.len() {
        if e < run.len() && k(idx[e] as usize) == k(idx[e - 1] as usize) { continue; }
        let g = &mut run[start..e];
        if g.len() == 1 { mark(g); } else { done(g); }
        start = e;
    }
    true
}

/// The order of n text keys: byte order, empty keys last, equal keys ordered by their ties, then
/// by position. Memory: 16 bytes an item, however long the keys. None if cancelled.
pub fn text_sort<H, K, A, T>(n: usize, t: &Text<H, K, A, T>, cancel: &AtomicBool) -> Option<Sorted>
where
    H: Fn(usize) -> u128 + Sync,
    K: Fn(usize, &mut Vec<u8>) + Sync,
    A: Fn(usize) -> usize + Sync,
    T: Fn(usize, &mut Vec<u8>) + Sync,
{
    let mut perm = vec![0u32; n];
    let new: Vec<AtomicU64> = (0..n.div_ceil(64)).map(|_| AtomicU64::new(0)).collect();
    let ties = |g: &mut [Item]| {
        let a = (t.alike)(pos(g[0]));
        if a != 0 && g.iter().all(|x| (t.alike)(pos(*x)) == a) { mark(g); } else { settle(g, 0, &t.tie, &mark); }
    };
    // Sorts a bucket's items, first place `at` in the whole order; gives what it found: values
    // and empty keys.
    let sort = |v: &mut [Item], out: &mut [u32], at: usize| -> (u32, usize) {
        if v.len() > 1 << 20 { v.par_sort_unstable() } else { v.sort_unstable() }
        let run = |r: &mut [Item]| {
            if r.len() < 2 || r[0] >> 32 == crate::text::EMPTY_HEAD >> 32 { return; }
            if ended(r[0]) { ties(r); } else { settle(r, crate::text::HEAD, &t.key, &ties); }
        };
        if v.len() > 1 << 18 {
            v.par_chunk_by_mut(|a, b| same(*a, *b)).for_each(run);
        } else {
            v.chunk_by_mut(|a, b| same(*a, *b)).for_each(run);
        }
        for (o, x) in out.iter_mut().zip(v.iter()) { *o = pos(*x) as u32; }
        let filled = v.partition_point(|x| *x >> 32 != crate::text::EMPTY_HEAD >> 32);
        let (mut values, mut bits) = (0, 0u64);
        for k in 0..filled {
            let place = at + k;
            if k == 0 || !same(v[k - 1], v[k]) {
                bits |= 1 << (place % 64);
                values += 1;
            }
            if place % 64 == 63 {
                new[place / 64].fetch_or(bits, Ordering::Relaxed);
                bits = 0;
            }
        }
        if bits != 0 { new[(at + filled - 1) / 64].fetch_or(bits, Ordering::Relaxed); }
        (values, v.len() - filled)
    };
    let (distinct, empty) = if n < 1 << 14 {
        let mut v: Vec<Item> = (0..n).map(|i| (t.head)(i) | i as Item).collect();
        sort(&mut v, &mut perm, 0)
    } else {
        let nb = (n / BUCKET).max(rayon::current_num_threads() * 8).next_power_of_two().min(MAX_BUCKETS);
        let tree = splitters(n, nb, &t.head);
        let pool = Pool::new(n, nb);
        const CHUNK: usize = 1 << 14;
        (0..n.div_ceil(CHUNK)).into_par_iter().for_each_init(|| Local { pool: &pool, items: vec![0; nb * LOCAL], len: vec![0; nb] }, |l, c| {
            if cancel.load(Ordering::Relaxed) { return; }
            for i in c * CHUNK..((c + 1) * CHUNK).min(n) {
                let h = (t.head)(i);
                l.push(bucket_of(&tree, h), h | i as Item);
            }
        });
        if cancel.load(Ordering::Relaxed) { return None; }
        let buckets: Vec<Bucket> = pool.buckets.iter().map(|b| std::mem::take(&mut *b.lock().unwrap_or_else(|e| e.into_inner()))).collect();
        let mut rest = perm.as_mut_slice();
        let mut parts = Vec::with_capacity(nb);
        let mut at = 0;
        for b in &buckets {
            let (out, r) = rest.split_at_mut(b.len);
            parts.push((b, out, at));
            at += b.len;
            rest = r;
        }
        parts.into_par_iter().map_init(Vec::new, |v, (b, out, at)| {
            if cancel.load(Ordering::Relaxed) { return (0, 0); }
            pool.take(b, v);
            sort(v, out, at)
        }).reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    };
    if cancel.load(Ordering::Relaxed) { return None; }
    Some(Sorted { perm, new: new.into_iter().map(AtomicU64::into_inner).collect(), filled: n - empty, distinct })
}

/// Orders a run of keys equal on their first `d` bytes (in stable order) on
/// the rest: 8 more bytes at a time, radix sorted, down to the runs still equal.
fn refine<'a>(run: &mut [u32], d: usize, key: &(impl Fn(usize) -> &'a [u8] + Sync)) {
    let tail = |p: u32| { let k = key(p as usize); &k[d.min(k.len())..] };
    if run.len() <= 32 || run.iter().all(|&p| tail(p).is_empty()) {
        // Few, or nothing left to cut into words (keys may still differ in length): compare.
        run.sort_by(|&a, &b| key(a as usize).cmp(key(b as usize)));
        return;
    }
    let big = run.len() > 1 << 15;
    let w: Vec<u64> = if big { run.par_iter().map(|&p| prefix(tail(p))).collect() } else { run.iter().map(|&p| prefix(tail(p))).collect() };
    let order = sort_perm(&w);
    let sorted: Vec<(u64, u32)> = order.iter().map(|&j| (w[j as usize], run[j as usize])).collect();
    for (r, &(_, p)) in run.iter_mut().zip(&sorted) { *r = p; }
    let next = |sub: &mut [u32]| if sub.len() > 1 { refine(sub, d + 8, key) };
    // Sub-runs: equal words.
    let mut subs = Vec::new();
    let mut rest = &mut run[..];
    let mut k = 0;
    while k < sorted.len() {
        let mut e = k + 1;
        while e < sorted.len() && sorted[e].0 == sorted[k].0 { e += 1; }
        let (a, b) = rest.split_at_mut(e - k);
        if a.len() > 1 { subs.push(a); }
        rest = b;
        k = e;
    }
    if big { subs.into_par_iter().for_each(next) } else { subs.into_iter().for_each(next) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text_sort` of keys held as strings: the order, the ranks, the values.
    fn order(keys: &[String], alike: impl Fn(usize) -> usize + Sync, tie: impl Fn(usize, &mut Vec<u8>) + Sync) -> (Vec<u32>, Vec<u32>, u32) {
        let head = |i: usize| if keys[i].is_empty() { crate::text::EMPTY_HEAD } else { crate::text::head_of(keys[i].as_bytes()) };
        let key = |i: usize, out: &mut Vec<u8>| out.extend_from_slice(keys[i].as_bytes());
        let s = text_sort(keys.len(), &Text { head, key, alike, tie }, &AtomicBool::new(false)).unwrap();
        let (rank, d) = (s.ranks(), s.distinct);
        (s.perm, rank, d)
    }

    fn check(w: &[u64]) {
        let mut want: Vec<u32> = (0..w.len() as u32).collect();
        want.sort_by_key(|&i| w[i as usize]);
        assert_eq!(sort_perm(w), want);
    }

    #[test]
    fn sorts_stably() {
        let mut x = 0x9E3779B97F4A7C15u64;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        for &n in &[0usize, 1, 2, 100, 40_000, 300_000] {
            check(&(0..n).map(|_| next()).collect::<Vec<_>>());
            check(&(0..n).map(|_| next() % 7).collect::<Vec<_>>());
            check(&(0..n).map(|_| next() >> 20).collect::<Vec<_>>());
            check(&(0..n).map(|i| if i % 3 == 0 { u64::MAX } else { next() % 1000 << 40 }).collect::<Vec<_>>());
            check(&(0..n).map(|_| 5).collect::<Vec<_>>());
        }
    }

    #[test]
    fn pairs_as_sort_unstable() {
        let mut x = 0x2545F4914F6CDD1Du64;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        for &n in &[0usize, 5, 40_000, 300_000] {
            for spread in [u64::MAX, 1000, 1] {
                let mut v: Vec<(u64, u32)> = (0..n as u32).map(|i| (if spread == u64::MAX { next() } else { next() % spread << 50 }, i)).collect();
                let mut want = v.clone();
                want.sort_unstable();
                sort_pairs(&mut v);
                assert_eq!(v, want);
            }
        }
    }

    #[test]
    fn ties_ranked_apart() {
        // Keys: the letter; ties: the case. Runs of every length, empties, singletons.
        let mut x = 11u64;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let cells: Vec<String> = (0..50_000).map(|_| match next() % 9 {
            0 => String::new(),
            k => { let c = (b'a' + (next() % (1 + k * 40) % 26) as u8) as char; if next() % 3 == 0 { c.to_ascii_uppercase().to_string() } else { c.to_string() } }
        }).collect();
        let low: Vec<String> = cells.iter().map(|c| c.to_lowercase()).collect();
        let (perm, rank, d) = order(&low, |i| (cells[i] == low[i] && i % 2 == 0) as usize, |i, out| out.extend_from_slice(cells[i].as_bytes()));
        let mut want: Vec<u32> = (0..cells.len() as u32).collect();
        want.sort_by(|&a, &b| {
            let (x, y) = (a as usize, b as usize);
            (low[x].is_empty(), &low[x], &cells[x]).cmp(&(low[y].is_empty(), &low[y], &cells[y]))
        });
        assert_eq!(perm, want);
        let distinct: std::collections::BTreeSet<&String> = cells.iter().filter(|c| !c.is_empty()).collect();
        assert_eq!(d as usize, distinct.len());
        for w in perm.windows(2) {
            let (a, b) = (w[0] as usize, w[1] as usize);
            assert_eq!(rank[a] == rank[b], cells[a] == cells[b] || cells[b].is_empty() && cells[a].is_empty());
            assert!(rank[a] <= rank[b]);
        }
        let (p2, _, _) = order(&["b".to_string(), "a".to_string(), "c".to_string()], |_| 0, |_, _| unreachable!());
        assert_eq!(p2, [1, 0, 2]);
    }

    #[test]
    fn text_long_keys() {
        // Long shared prefixes, keys differing only in length or trailing NULs, ties.
        let mut x = 7u64;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let parts = ["", "a", "ab", "\0", "abcdefgh", "zz", "abcdefghabcdefgh"];
        let keys: Vec<String> = (0..20_000).map(|_| (0..1 + next() % 5).map(|_| parts[(next() % 7) as usize]).collect()).collect();
        let (perm, _, _) = order(&keys, |_| 1, |_, _| ());
        let mut want: Vec<u32> = (0..keys.len() as u32).collect();
        want.sort_by(|&a, &b| {
            let (x, y) = (&keys[a as usize], &keys[b as usize]);
            (x.is_empty(), x).cmp(&(y.is_empty(), y))
        });
        assert_eq!(perm, want);
    }

    #[test]
    fn text() {
        let keys = ["b", "", "ab", "a", "ab\0", "b", "", "abcdefghij", "abcdefghi"];
        let (perm, rank, d) = order(&keys.map(String::from), |_| 1, |_, _| ());
        let got: Vec<&str> = perm.iter().map(|&p| keys[p as usize]).collect();
        assert_eq!(got, ["a", "ab", "ab\0", "abcdefghi", "abcdefghij", "b", "b", "", ""]);
        assert_eq!(perm[5..7], [0, 5]);
        assert_eq!(d, 6);
        assert_eq!(rank, [5, 6, 1, 0, 2, 5, 6, 4, 3]);
    }
}
