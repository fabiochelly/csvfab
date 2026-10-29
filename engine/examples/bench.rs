//! Times open / filter / sort on a file: `cargo run --release --example bench -- file.csv`.
use csvfab_engine::view::{self, FilterSpec, SortKey};
use csvfab_engine::{Base, OpenOptions, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let t = Instant::now();
    let base = Base::open(path.as_ref(), OpenOptions::default(), None).unwrap();
    println!("open    {:>7.0} ms  {} records, {} cols, {:?}, delim {:?}, index {} MB",
        t.elapsed().as_secs_f64() * 1e3, base.n, base.width, base.enc, base.dialect.delim as char, base.heap_bytes() >> 20);
    let table = Table::new(Arc::new(base), None);
    let st = &table.st;
    let no = AtomicBool::new(false);
    let t = Instant::now();
    let kinds = view::detect_kinds(st);
    println!("kinds   {:>7.0} ms  {:?}", t.elapsed().as_secs_f64() * 1e3, kinds);
    let h = st.headers();
    let col = |n: &str| h.iter().position(|x| x == n).unwrap_or(0);
    for (label, spec) in [
        ("global lyon", FilterSpec { global: "lyon".into(), ..Default::default() }),
        ("global émile", FilterSpec { global: "émile".into(), ..Default::default() }),
        ("city=lyon", FilterSpec { cols: vec![(col("city"), "lyon".into())], ..Default::default() }),
        ("total>1000", FilterSpec { cols: vec![(col("total_spent"), ">1000".into())], ..Default::default() }),
    ] {
        let t = Instant::now();
        let f = view::Filter::compile(&spec).unwrap();
        let ids = view::filter(st, &f, &no).unwrap();
        println!("filter  {:>7.0} ms  {label}: {}", t.elapsed().as_secs_f64() * 1e3, ids.len());
    }
    let all = view::filter(st, &view::Filter::compile(&FilterSpec::default()).unwrap(), &no).unwrap();
    for name in ["total_spent", "last_name", "signup_date"] {
        let mut ids = all.clone();
        let t = Instant::now();
        view::sort(st, &mut ids, &[SortKey { col: col(name), desc: false }], &kinds, &no);
        println!("sort    {:>7.0} ms  {name}", t.elapsed().as_secs_f64() * 1e3);
    }
    let t = Instant::now();
    let out = csvfab_engine::save::write_to(st, &Default::default(), std::io::sink()).unwrap();
    let _ = out;
    println!("save    {:>7.0} ms  (to /dev/null)", t.elapsed().as_secs_f64() * 1e3);
}
