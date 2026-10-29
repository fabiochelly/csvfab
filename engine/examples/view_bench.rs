//! Filter and sort timings, medians of several runs:
//! `cargo run --release --example view_bench -- file.csv [runs]`.
//! Prints one line per case: `case  median_ms  cpu_ms  result_len  checksum` (the checksum
//! lets two builds be compared for identical results).
use csvfab_engine::view::{self, FilterSpec, SortKey};
use csvfab_engine::{Base, OpenOptions, RowId, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn sum(ids: &[RowId]) -> u64 {
    ids.iter().enumerate().fold(0u64, |h, (k, &id)| h.wrapping_mul(1_000_003).wrapping_add(id as u64 ^ k as u64))
}

/// CPU time of the whole process (every thread), ms: the work done, whatever
/// else the machine runs.
fn cpu() -> f64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t) };
    t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let runs: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let only = std::env::args().nth(3).unwrap_or_default();
    let base = Base::open(path.as_ref(), OpenOptions::default(), None).unwrap();
    let table = Table::new(Arc::new(base), None);
    let st = &table.st;
    let no = AtomicBool::new(false);
    let kinds = view::detect_kinds(st);
    let h = st.headers();
    let col = |n: &str| h.iter().position(|x| x == n).unwrap_or(0);
    let g = |s: &str| FilterSpec { global: s.into(), ..Default::default() };
    let c = |n: &str, s: &str| FilterSpec { cols: vec![(col(n), s.into())], ..Default::default() };
    let mut hidden = std::collections::HashSet::new();
    hidden.insert("Lyon".to_string());
    hidden.insert("Paris".to_string());
    let cases: Vec<(&str, FilterSpec)> = vec![
        ("f global lyon", g("lyon")),
        ("f global émile", g("émile")),
        ("f global l", g("l")),
        ("f global ly", g("ly")),
        ("f global lyo", g("lyo")),
        ("f global associes", g("associes")),
        ("f global Lyon case", FilterSpec { global: "Lyon".into(), match_case: true, ..Default::default() }),
        ("f global lyon invert", FilterSpec { global: "lyon".into(), invert: true, ..Default::default() }),
        ("f global regex ^Lyo", FilterSpec { global: "^Lyo".into(), regex: true, ..Default::default() }),
        ("f city=lyon", c("city", "lyon")),
        ("f city exact =lyon", c("city", "=lyon")),
        ("f city !lyon", c("city", "!lyon")),
        ("f company associés", c("company", "associés")),
        ("f total>1000", c("total_spent", ">1000")),
        ("f signup>=2024-01-01", c("signup_date", ">=2024-01-01")),
        ("f hidden city", FilterSpec { hidden: vec![(col("city"), Arc::new(hidden))], ..Default::default() }),
        ("f email gmail", c("email", "gmail")),
        ("f phone 555", c("phone", "555")),
        ("f global 2024-0", g("2024-0")),
        ("f global regex lyon|paris", FilterSpec { global: "lyon|paris".into(), regex: true, ..Default::default() }),
        ("f global regex \\d{3}-\\d{4}", FilterSpec { global: r"\d{3}-\d{4}".into(), regex: true, ..Default::default() }),
        ("f city regex ^Ly", FilterSpec { cols: vec![(col("city"), "^Ly".into())], regex: true, ..Default::default() }),
        ("f city=lyon & total>1000", FilterSpec { cols: vec![(col("city"), "lyon".into()), (col("total_spent"), ">1000".into())], ..Default::default() }),
    ];
    let all = view::filter(st, &view::Filter::compile(&FilterSpec::default()).unwrap(), &no).unwrap();
    let mut lyon = Vec::new();
    for (label, spec) in &cases {
        if !label.contains(only.as_str()) { continue; }
        let f = view::Filter::compile(spec).unwrap();
        let (mut times, mut cpus) = (Vec::new(), Vec::new());
        let mut ids = Vec::new();
        for _ in 0..runs {
            let (t, c) = (Instant::now(), cpu());
            ids = view::filter(st, &f, &no).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1e3);
            cpus.push(cpu() - c);
        }
        if *label == "f city=lyon" { lyon = ids.clone(); }
        println!("{label:<28} {:>8.1} ms {:>8.0} cpu {:>9} {:016x}", median(times), median(cpus), ids.len(), sum(&ids));
    }
    let k = |n: &str, desc| SortKey { col: col(n), desc };
    let sorts: Vec<(&str, Vec<SortKey>, &Vec<RowId>)> = vec![
        ("s total_spent", vec![k("total_spent", false)], &all),
        ("s total_spent desc", vec![k("total_spent", true)], &all),
        ("s last_name", vec![k("last_name", false)], &all),
        ("s last_name desc", vec![k("last_name", true)], &all),
        ("s email", vec![k("email", false)], &all),
        ("s company desc", vec![k("company", true)], &all),
        ("s signup_date", vec![k("signup_date", false)], &all),
        ("s city,last_name", vec![k("city", false), k("last_name", false)], &all),
        ("s status,total desc", vec![k("status", false), k("total_spent", true)], &all),
        ("s lyon by last_name", vec![k("last_name", false)], &lyon),
    ];
    for (label, keys, input) in &sorts {
        if !label.contains(only.as_str()) { continue; }
        let (mut times, mut cpus) = (Vec::new(), Vec::new());
        let mut ids = Vec::new();
        for _ in 0..runs {
            ids = (*input).clone();
            let (t, c) = (Instant::now(), cpu());
            view::sort(st, &mut ids, keys, &kinds, &no);
            times.push(t.elapsed().as_secs_f64() * 1e3);
            cpus.push(cpu() - c);
        }
        println!("{label:<28} {:>8.1} ms {:>8.0} cpu {:>9} {:016x}", median(times), median(cpus), ids.len(), sum(&ids));
    }
}
