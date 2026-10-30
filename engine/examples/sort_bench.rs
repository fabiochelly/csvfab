//! First text sorts, the column caches dropped before each run, medians of several:
//! `sort_bench file.csv col1,col2 [runs] [third]`. `third`: a third of the rows, keys made on the spot.
//! Prints `col  first_ms  again_ms  desc_ms  checksum`; the checksum tells two builds' orders apart.
use csvfab_engine::view::{self, SortKey};
use csvfab_engine::{colcache, Base, OpenOptions, RowId, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn sum(ids: &[RowId]) -> u64 {
    ids.iter().enumerate().fold(0u64, |h, (k, &id)| h.wrapping_mul(1_000_003).wrapping_add(id as u64 ^ k as u64))
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let t = Instant::now();
    let base = Base::open(a[1].as_ref(), OpenOptions::default(), None).unwrap();
    let table = Table::new(Arc::new(base), None);
    let st = &table.st;
    eprintln!("open {:.0} ms, {} rows", t.elapsed().as_secs_f64() * 1e3, st.len());
    let no = AtomicBool::new(false);
    let kinds = view::detect_kinds(st);
    let h = st.headers();
    let runs: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let third = a.get(4).is_some_and(|s| s == "third");
    let all: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).filter(|&id| !third || id % 3 == 0).collect();
    for name in a[2].split(',') {
        let col = h.iter().position(|x| x == name).expect("column");
        let (mut first, mut again, mut desc) = (Vec::new(), Vec::new(), Vec::new());
        let mut check = 0;
        for _ in 0..runs {
            colcache::clear();
            let mut ids = all.clone();
            let t = Instant::now();
            view::sort(st, &mut ids, &[SortKey { col, desc: false }], &kinds, &no);
            first.push(t.elapsed().as_secs_f64() * 1e3);
            check = sum(&ids);
            drop(ids);
            let mut ids = all.clone();
            let t = Instant::now();
            view::sort(st, &mut ids, &[SortKey { col, desc: false }], &kinds, &no);
            again.push(t.elapsed().as_secs_f64() * 1e3);
            let mut ids = all.clone();
            let t = Instant::now();
            view::sort(st, &mut ids, &[SortKey { col, desc: true }], &kinds, &no);
            desc.push(t.elapsed().as_secs_f64() * 1e3);
            check ^= sum(&ids).rotate_left(1);
        }
        println!("{name:<20} {:>9.1} {:>9.1} {:>9.1} {check:016x}", median(first), median(again), median(desc));
    }
}
