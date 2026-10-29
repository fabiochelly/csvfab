//! Column caches: what they cost to build and hold, and filters and sorts
//! with them; typing in a filter (each letter refining the previous result).
//! `cargo run --release --example cache_bench -- file.csv [runs]`
use csvfab_engine::colcache::{self, Need};
use csvfab_engine::view::{self, FilterSpec, SortKey};
use csvfab_engine::{Base, OpenOptions, RowId, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let runs: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let base = Base::open(path.as_ref(), OpenOptions::default(), None).unwrap();
    let table = Table::new(Arc::new(base), None);
    let st = &table.st;
    let no = AtomicBool::new(false);
    let kinds = view::detect_kinds(st);
    let h = st.headers();
    let col = |n: &str| h.iter().position(|x| x == n).unwrap_or(0);
    let time = |f: &mut dyn FnMut() -> Vec<RowId>| {
        let mut ids = Vec::new();
        let t: Vec<f64> = (0..runs).map(|_| { let t = Instant::now(); ids = f(); ms(t) }).collect();
        (median(t), ids.len())
    };

    println!("## Typing, no cache yet: each letter a full pass vs refining the previous result");
    for (name, texts) in [("global", ["l", "ly", "lyo", "lyon"]), ("city", ["l", "ly", "lyo", "lyon"])] {
        let spec = |s: &str| if name == "global" { FilterSpec { global: s.into(), ..Default::default() } } else { FilterSpec { cols: vec![(col("city"), s.into())], ..Default::default() } };
        let mut prev: Option<Vec<RowId>> = None;
        for t in texts {
            let f = view::Filter::compile(&spec(t)).unwrap();
            let (full, n) = time(&mut || view::filter(st, &f, &no).unwrap());
            let (refined, _) = match &prev { Some(p) => time(&mut || view::filter_ids(st, &f, p, &no).unwrap()), None => (full, n) };
            println!("{name:<7} {t:<5} full {full:>7.1} ms   refined {refined:>7.1} ms   {n:>8} rows");
            prev = Some(view::filter(st, &f, &no).unwrap());
        }
    }

    println!("\n## First sort of each column: its cache built on the way");
    let all = view::filter(st, &view::Filter::compile(&FilterSpec::default()).unwrap(), &no).unwrap();
    for name in ["total_spent", "last_name", "email", "company", "signup_date"] {
        let mut v = Vec::new();
        for _ in 0..runs.min(3) {
            colcache::clear();
            let mut ids = all.clone();
            let t = Instant::now();
            view::sort(st, &mut ids, &[SortKey { col: col(name), desc: false }], &kinds, &no);
            v.push(ms(t));
        }
        println!("{name:<12} {:>7.1} ms  (cache {} MB)", median(v), colcache::used_bytes() >> 20);
    }
    colcache::clear();

    println!("\n## Building the caches (all cores)");
    for (name, needs) in [
        ("city", &[Need::Text][..]),
        ("status", &[Need::Text]),
        ("company", &[Need::Text]),
        ("email", &[Need::Text, Need::Rank]),
        ("last_name", &[Need::Rank]),
        ("total_spent", &[Need::Num]),
        ("signup_date", &[Need::Date, Need::Num]),
    ] {
        for &need in needs {
            let before = colcache::used_bytes();
            let t = Instant::now();
            let c = colcache::ensure(st, col(name), need, &no).unwrap();
            let kind = if c.dict.is_some() { format!("dict of {}", c.dict.as_ref().unwrap().values.len()) } else { "plain".into() };
            println!("{name:<12} {need:<5?} {:>7.1} ms  +{:>5} MB  ({kind})", ms(t), (colcache::used_bytes() - before) >> 20);
        }
    }
    println!("total held: {} MB", colcache::used_bytes() >> 20);

    println!("\n## With the caches");
    let g = |s: &str| FilterSpec { global: s.into(), ..Default::default() };
    let c = |n: &str, s: &str| FilterSpec { cols: vec![(col(n), s.into())], ..Default::default() };
    let mut hidden = std::collections::HashSet::new();
    hidden.insert("Lyon".to_string());
    hidden.insert("Paris".to_string());
    for (label, spec) in [
        ("f city=lyon", c("city", "lyon")),
        ("f city exact =lyon", c("city", "=lyon")),
        ("f city !lyon", c("city", "!lyon")),
        ("f company associés", c("company", "associés")),
        ("f email gmail", c("email", "gmail")),
        ("f total>1000", c("total_spent", ">1000")),
        ("f signup>=2024-01-01", c("signup_date", ">=2024-01-01")),
        ("f hidden city", FilterSpec { hidden: vec![(col("city"), Arc::new(hidden.clone()))], ..Default::default() }),
        ("f city=lyon & total>1000", FilterSpec { cols: vec![(col("city"), "lyon".into()), (col("total_spent"), ">1000".into())], ..Default::default() }),
        ("f global lyon", g("lyon")),
    ] {
        let f = view::Filter::compile(&spec).unwrap();
        let (t, n) = time(&mut || view::filter(st, &f, &no).unwrap());
        println!("{label:<28} {t:>8.1} ms {n:>9}");
    }
    let lyon = view::filter(st, &view::Filter::compile(&c("city", "lyon")).unwrap(), &no).unwrap();
    let k = |n: &str, desc| SortKey { col: col(n), desc };
    for (label, keys, input) in [
        ("s total_spent", vec![k("total_spent", false)], &all),
        ("s total_spent desc", vec![k("total_spent", true)], &all),
        ("s last_name", vec![k("last_name", false)], &all),
        ("s email", vec![k("email", false)], &all),
        ("s signup_date", vec![k("signup_date", false)], &all),
        ("s city,last_name", vec![k("city", false), k("last_name", false)], &all),
        ("s lyon by last_name", vec![k("last_name", false)], &lyon),
    ] {
        let (t, n) = time(&mut || { let mut ids = input.clone(); view::sort(st, &mut ids, &keys, &kinds, &no); ids });
        println!("{label:<28} {t:>8.1} ms {n:>9}");
    }

    println!("\n## Typing with the caches");
    let mut prev: Option<Vec<RowId>> = None;
    for t in ["l", "ly", "lyo", "lyon"] {
        let f = view::Filter::compile(&c("city", t)).unwrap();
        let (full, n) = time(&mut || view::filter(st, &f, &no).unwrap());
        let (refined, _) = match &prev { Some(p) => time(&mut || view::filter_ids(st, &f, p, &no).unwrap()), None => (full, n) };
        println!("city    {t:<5} full {full:>7.1} ms   refined {refined:>7.1} ms   {n:>8} rows");
        prev = Some(view::filter(st, &f, &no).unwrap());
    }
}
