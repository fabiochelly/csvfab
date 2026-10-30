//! Stable sorts of row positions by u64 keys, in parallel: one bucket pass on
//! the keys' high bits (a counting sort when the keys are few), then each
//! bucket sorted on its own, in cache. Ties keep their order, so sorting by
//! several keys is sorting by each, last key first.

use rayon::prelude::*;

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
    if n < 1 << 15 {
        // Small: one sort of (key, position) pairs; positions are distinct, so unstable is stable.
        if width <= 32 {
            let mut v: Vec<u64> = w.iter().enumerate().map(|(i, &x)| ((x - lo) << 32) | i as u64).collect();
            v.sort_unstable();
            return v.into_iter().map(|x| x as u32).collect();
        }
        let mut v: Vec<(u64, u32)> = w.iter().enumerate().map(|(i, &x)| (x, i as u32)).collect();
        v.sort_unstable();
        return v.into_iter().map(|x| x.1).collect();
    }
    // Bucket on the top bits: about 512 rows a bucket, at most 2^16 buckets.
    let bits = (usize::BITS - (n >> 9).leading_zeros()).clamp(4, 16).min(width);
    let shift = width - bits;
    if width <= 32 {
        let items = |i: usize| ((w[i] - lo) << 32) | i as u64;
        let pairs = scatter(n, 1 << bits, |i| ((w[i] - lo) >> shift) as usize, items);
        finish(pairs, shift, n, |x: &u64| *x as u32)
    } else {
        let items = |i: usize| (w[i], i as u32);
        let pairs = scatter(n, 1 << bits, |i| ((w[i] - lo) >> shift) as usize, items);
        finish(pairs, shift, n, |x: &(u64, u32)| x.1)
    }
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

/// Byte keys end to end, in arenas of `KEYS_CH` keys (no allocation per key).
pub struct Keys(Vec<(Vec<u8>, Vec<u32>)>);

const KEYS_CH: usize = 1 << 14;

impl Keys {
    /// Key `i` of `n`, each written by `put(i, out)` (in parallel, by arena).
    pub fn build(n: usize, put: impl Fn(usize, &mut Vec<u8>) + Sync) -> Keys {
        Keys((0..n.div_ceil(KEYS_CH)).into_par_iter().map(|c| {
            let r = c * KEYS_CH..((c + 1) * KEYS_CH).min(n);
            let mut arena = Vec::with_capacity(r.len() * 12);
            let mut offs = Vec::with_capacity(r.len() + 1);
            offs.push(0u32);
            for i in r {
                put(i, &mut arena);
                offs.push(arena.len() as u32);
            }
            (arena, offs)
        }).collect())
    }

    #[inline]
    pub fn get(&self, i: usize) -> &[u8] {
        let (a, o) = &self.0[i / KEYS_CH];
        let k = i % KEYS_CH;
        &a[o[k] as usize..o[k + 1] as usize]
    }
}

/// The order of n text keys (byte order, empty keys last, ties in position
/// order) and each one's dense rank in it (equal keys share a rank; empty:
/// the number of distinct non-empty keys).
pub fn text_order<'a>(n: usize, key: impl Fn(usize) -> &'a [u8] + Sync) -> (Vec<u32>, Vec<u32>, u32) {
    let (perm, rank, distinct, _) = order_ranks(n, &key, false);
    (perm, rank, distinct)
}

/// As `text_order`, equal keys then ordered (and ranked apart) by the bytes `tie(i, out)` writes:
/// asked only of the items whose keys are equal (`text::tie_key_into`: the cells as written), and
/// not of a run whose items are all `plain` (their ties known equal).
pub fn text_order_ties<'a>(n: usize, key: impl Fn(usize) -> &'a [u8] + Sync, plain: impl Fn(usize) -> bool + Sync, tie: impl Fn(usize, &mut Vec<u8>) + Sync) -> (Vec<u32>, Vec<u32>, u32) {
    let (mut perm, mut rank, distinct, runs) = order_ranks(n, &key, true);
    if runs.is_empty() {
        return (perm, rank, distinct);
    }
    // Each run of equal keys sorted on its ties (stably); its items' places among them.
    let mut slices = Vec::with_capacity(runs.len());
    let mut rest = &mut perm[..];
    let mut at = 0;
    for &(s, len) in &runs {
        let (_, b) = rest.split_at_mut(s as usize - at);
        let (run, b) = b.split_at_mut(len as usize);
        slices.push(run);
        rest = b;
        at = (s + len) as usize;
    }
    let subs: Vec<(u32, Vec<(u32, u32)>)> = slices
        .into_par_iter()
        .with_min_len(64)
        .map_init(|| (Vec::new(), Vec::new(), Vec::new()), |(arena, offs, idx), run| {
            if run.iter().all(|&p| plain(p as usize)) { return (0, Vec::new()); }
            arena.clear();
            offs.clear();
            offs.push(0usize);
            for &p in run.iter() {
                tie(p as usize, arena);
                offs.push(arena.len());
            }
            let k = |j: usize| &arena[offs[j]..offs[j + 1]];
            idx.clear();
            idx.extend(0..run.len() as u32);
            idx.sort_by(|&a, &b| k(a as usize).cmp(k(b as usize)));
            let mut sub = 0u32;
            let mut moved = Vec::new();
            for w in 1..idx.len() {
                if k(idx[w] as usize) != k(idx[w - 1] as usize) { sub += 1; }
                if sub > 0 { moved.push((run[idx[w] as usize], sub)); }
            }
            if sub > 0 {
                let items: Vec<u32> = idx.iter().map(|&j| run[j as usize]).collect();
                run.copy_from_slice(&items);
            }
            (sub, moved)
        })
        .collect();
    let total: u32 = subs.iter().map(|s| s.0).sum();
    if total == 0 {
        return (perm, rank, distinct);
    }
    // Ranks after a run move up by the places its ties took.
    let mut shift = vec![0u32; distinct as usize + 1];
    let mut cum = 0;
    let mut next = 0;
    for (&(s, _), &(extra, _)) in runs.iter().zip(&subs) {
        let r = rank[perm[s as usize] as usize] as usize;
        shift[next..=r].fill(cum);
        cum += extra;
        next = r + 1;
    }
    shift[next..].fill(cum);
    rank.par_iter_mut().with_min_len(1 << 14).for_each(|x| *x += shift[*x as usize]);
    for (_, moved) in &subs {
        for &(p, sub) in moved { rank[p as usize] += sub; }
    }
    (perm, rank, distinct + total)
}

/// `text_order`, and the runs of equal non-empty keys in the order (start, length) if `runs`.
fn order_ranks<'a>(n: usize, key: &(impl Fn(usize) -> &'a [u8] + Sync), runs: bool) -> (Vec<u32>, Vec<u32>, u32, Vec<(u32, u32)>) {
    let words: Vec<u64> = (0..n).into_par_iter().with_min_len(1 << 14).map(|i| {
        let k = key(i);
        // Text is UTF-8: no byte is 0xFF, so no prefix of a key reaches u64::MAX.
        if k.is_empty() { u64::MAX } else { prefix(k) }
    }).collect();
    let mut perm = sort_perm(&words);
    // Equal first 8 bytes: ordered on the next 8, and so on.
    perm.par_chunk_by_mut(|&a, &b| words[a as usize] == words[b as usize])
        .for_each(|run| {
            if run.len() > 1 && words[run[0] as usize] != u64::MAX {
                refine(run, 8, key);
            }
        });
    // Ranks: a new one where the key changes.
    let mut rank = vec![0u32; n];
    let mut r = 0u32;
    let mut prev: Option<&[u8]> = None;
    let mut found = Vec::new();
    let mut start = 0;
    for (i, &p) in perm.iter().enumerate() {
        let k = key(p as usize);
        if k.is_empty() { break; }
        if prev.is_some_and(|q| q != k) {
            r += 1;
            if runs && i - start > 1 { found.push((start as u32, (i - start) as u32)); }
            start = i;
        }
        prev = Some(k);
        rank[p as usize] = r;
    }
    let distinct = if prev.is_some() { r + 1 } else { 0 };
    let filled = perm.iter().rev().position(|&p| !key(p as usize).is_empty()).map_or(0, |e| n - e);
    if runs && filled - start > 1 { found.push((start as u32, (filled - start) as u32)); }
    for &p in &perm[filled..] {
        rank[p as usize] = distinct;
    }
    (perm, rank, distinct, found)
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
        let (perm, rank, d) = text_order_ties(cells.len(), |i| low[i].as_bytes(), |i| cells[i] == low[i] && i % 2 == 0, |i, out| out.extend_from_slice(cells[i].as_bytes()));
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
        let (p2, _, _) = text_order_ties(3, |i| ["b", "a", "c"][i].as_bytes(), |_| false, |_, _| unreachable!());
        assert_eq!(p2, [1, 0, 2]);
    }

    #[test]
    fn text_long_keys() {
        // Long shared prefixes, keys differing only in length or trailing NULs, ties.
        let mut x = 7u64;
        let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let parts = ["", "a", "ab", "\0", "abcdefgh", "zz", "abcdefghabcdefgh"];
        let keys: Vec<String> = (0..20_000).map(|_| (0..1 + next() % 5).map(|_| parts[(next() % 7) as usize]).collect()).collect();
        let (perm, _, _) = text_order(keys.len(), |i| keys[i].as_bytes());
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
        let (perm, rank, d) = text_order(keys.len(), |i| keys[i].as_bytes());
        let got: Vec<&str> = perm.iter().map(|&p| keys[p as usize]).collect();
        assert_eq!(got, ["a", "ab", "ab\0", "abcdefghi", "abcdefghij", "b", "b", "", ""]);
        assert_eq!(perm[5..7], [0, 5]);
        assert_eq!(d, 6);
        assert_eq!(rank, [5, 6, 1, 0, 2, 5, 6, 4, 3]);
    }
}
