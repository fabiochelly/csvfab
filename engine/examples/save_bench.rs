//! Saving a big file over itself, as the app does: `save_bench FILE CASE`
//! (CASE: none, first, last, column). FILE is replaced (work on a copy).
use csvfab_engine::save::{self, SaveOptions};
use csvfab_engine::{Base, OpenOptions, Table};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let path = std::path::PathBuf::from(a.next().expect("file"));
    let case = a.next().unwrap_or_else(|| "none".into());
    let base = Arc::new(Base::open(&path, OpenOptions::default(), None).unwrap());
    let stamp = base.stamp;
    let mut t = Table::new(base, None);
    let n = t.st.len();
    match case.as_str() {
        "first" => t.set_cells(&[(t.st.data_id(10), 1, "Edited".into())]),
        "last" => t.set_cells(&[(t.st.data_id(n - 10), 1, "Edited".into())]),
        "column" => {
            let ch: Vec<_> = (0..n).map(|p| (t.st.data_id(p), 8, "Lyon".to_string())).collect();
            t.set_cells(&ch)
        }
        _ => {}
    }
    let t0 = Instant::now();
    let _ = save::write_to(&t.st, &SaveOptions::default(), std::io::sink()).unwrap();
    let mem = t0.elapsed();
    let t0 = Instant::now();
    let o = SaveOptions { backup: false, ..Default::default() };
    let ix = save::save_indexed(&t.st, &path, &o, stamp, &mut false).unwrap().1.is_some();
    let disk = t0.elapsed();
    println!("{case:>7}: write_to sink {:7.1} ms, save to disk (fsync) {:7.1} ms{}", mem.as_secs_f64() * 1e3, disk.as_secs_f64() * 1e3, if ix { ", index reused" } else { "" });
}
