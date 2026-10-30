//! Saving a big file, as the app does: `save_bench FILE CASE [DEST]` (CASE: none, first,
//! last, column, computed, latin1, utf16, move, insert, eol). Without DEST, FILE is replaced
//! (work on a copy); with it, FILE is only read. With DEST, also the table exported as a
//! workbook (`DEST.xlsx`) and its edits as a recovery journal (`DEST.journal`), each timed
//! and removed.
use csvfab_engine::enc::Enc;
use csvfab_engine::save::{self, SaveOptions};
use csvfab_engine::table::{Cells, Patch};
use csvfab_engine::{Base, OpenOptions, Table};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut a = std::env::args().skip(1);
    let path = std::path::PathBuf::from(a.next().expect("file"));
    let case = a.next().unwrap_or_else(|| "none".into());
    let dest = a.next().map(std::path::PathBuf::from);
    let base = Arc::new(Base::open(&path, OpenOptions::default(), None).unwrap());
    let stamp = base.stamp;
    let mut t = Table::new(base, None);
    let n = t.st.len();
    let mut o = SaveOptions { backup: false, ..Default::default() };
    match case.as_str() {
        "first" => t.set_cells(&[(t.st.data_id(10), 1, "Edited".into())]),
        "last" => t.set_cells(&[(t.st.data_id(n - 10), 1, "Edited".into())]),
        "column" => {
            let ch: Vec<_> = (0..n).map(|p| (t.st.data_id(p), 8, "Lyon".to_string())).collect();
            t.set_cells(&ch)
        }
        "computed" => {
            // A new column, every row computed (as the formula tool's patch).
            let c = t.st.ncols;
            t.insert_column(c, "Computed");
            let mut cells = Cells::with_capacity(n);
            for p in 0..n { cells.push(t.st.data_id(p), &format!("{}", p as f64 * 1.5)); }
            t.apply(Patch::column(c, vec![cells]));
        }
        "latin1" => o.enc = Some(Enc::Windows1252),
        "utf16" => o.enc = Some(Enc::Utf16Le),
        "move" => t.move_column(0, 5),
        "insert" => t.insert_column(2, "New"),
        "eol" => t.set_eol(b"\r\n"),
        _ => {}
    }
    let t0 = Instant::now();
    let _ = save::write_to(&t.st, &o, std::io::sink()).unwrap();
    let mem = t0.elapsed();
    let t0 = Instant::now();
    let (to, expect) = match &dest { Some(d) => (d.clone(), None), None => (path.clone(), stamp.map(Into::into)) };
    let ix = save::save_indexed(&t.st, &to, &o, expect, &mut false).unwrap().1.is_some();
    let disk = t0.elapsed();
    println!("{case:>8}: write_to sink {:7.1} ms, save to disk (fsync) {:7.1} ms{}", mem.as_secs_f64() * 1e3, disk.as_secs_f64() * 1e3, if ix { ", index reused" } else { "" });
    if let Some(d) = &dest {
        let x = d.with_extension("xlsx");
        let t0 = Instant::now();
        csvfab_engine::convert::export(csvfab_engine::convert::Format::Xlsx, &t.st, None, &x, "bench", None, &Default::default()).unwrap();
        println!("{case:>8}: workbook {:7.1} ms, {} MB", t0.elapsed().as_secs_f64() * 1e3, std::fs::metadata(&x).map_or(0, |m| m.len()) >> 20);
        let _ = std::fs::remove_file(&x);
        let j = d.with_extension("journal");
        let t0 = Instant::now();
        csvfab_engine::journal::write(&t.st, OpenOptions::default(), std::io::BufWriter::new(std::fs::File::create(&j).unwrap())).unwrap();
        println!("{case:>8}: journal {:7.1} ms, {} KB", t0.elapsed().as_secs_f64() * 1e3, std::fs::metadata(&j).map_or(0, |m| m.len()) >> 10);
        let _ = std::fs::remove_file(&j);
    }
}
