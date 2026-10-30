//! What a column's first use costs: one field read from every record, and each
//! column cache built from nothing (`colcache::clear` before each), medians of `runs`;
//! before them, whole-table passes: the totals of every number column, a global search
//! ("lyon") and find's row test ("Lyon"), against a memmem over the bytes.
//! `cargo run --release --example col_bench -- file.csv [runs] [column…]`
use csvfab_engine::colcache::{self, Need};
use csvfab_engine::{scan, stats, view, Base, OpenOptions, Table};
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("file");
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let base = Arc::new(Base::open(path.as_ref(), OpenOptions::default(), None).unwrap());
    let table = Table::new(base.clone(), None);
    let st = &table.st;
    let h = st.headers();
    let names: Vec<String> = if args.len() > 3 { args[3..].to_vec() } else { vec![h[1].clone(), h[h.len() / 2].clone(), h[h.len() - 2].clone()] };
    let no = AtomicBool::new(false);
    let time = |f: &mut dyn FnMut() -> usize| -> (f64, usize) {
        let mut out = 0;
        let v = (0..runs).map(|_| { let t = Instant::now(); out = f(); t.elapsed().as_secs_f64() * 1e3 }).collect();
        (median(v), out)
    };
    let d = base.dialect.delim;
    // Whole-table passes: the totals of every number column, a global search and its floor
    // (the needle looked for in the raw bytes, nothing else).
    let kinds = view::detect_kinds(st);
    let reqs: Vec<stats::TotalReq> = (0..st.ncols).filter(|&c| kinds[c] == view::Kind::Number).map(|col| stats::TotalReq { col, kind: view::Kind::Number, median: false, distinct: false }).collect();
    let ids: Vec<csvfab_engine::RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
    let (ms, k) = time(&mut || stats::totals(st, &ids, &reqs, &no).unwrap().len());
    println!("totals of {k} number columns {ms:>7.1} ms");
    let f = view::Filter::compile(&view::FilterSpec { global: "lyon".into(), ..Default::default() }).unwrap();
    let (ms, k) = time(&mut || view::filter(st, &f, &no).unwrap().len());
    println!("global \"lyon\"       {ms:>7.1} ms  {k} rows");
    let op = csvfab_engine::edit::replace::Replace::new(&csvfab_engine::edit::replace::ReplaceSpec { find: "Lyon".into(), ..Default::default() }, st).unwrap().unwrap();
    let (ms, k) = time(&mut || (0..st.len()).into_par_iter().filter(|&p| { use csvfab_engine::edit::CellOp; op.row_may_change(st, st.data_id(p), &mut ()) }).count());
    println!("find's rows \"Lyon\"  {ms:>7.1} ms  {k} rows");
    let bytes = base.bytes();
    let fi = memchr::memmem::Finder::new("lyon");
    let (ms, k) = time(&mut || bytes.par_chunks(1 << 16).map(|c| fi.find_iter(c).count()).sum());
    println!("memmem floor        {ms:>7.1} ms  {k} hits");
    let (ms, _) = time(&mut || (0..base.n).into_par_iter().map(|i| base.record(i).len()).sum());
    println!("records only         {ms:>7.1} ms");
    let (ms, _) = time(&mut || (0..base.n).into_par_iter().map_init(Vec::new, |tmp, i| { let mut k = 0; scan::visit_fields(base.record(i), d, tmp, |_, f| { k += f.len(); true }); k }).sum());
    println!("every field visited  {ms:>7.1} ms");
    for name in &names {
        let c = h.iter().position(|x| x == name).unwrap_or_else(|| panic!("no column {name}"));
        let (ms, _) = time(&mut || (0..base.n).into_par_iter().map_init(Vec::new, |tmp, i| scan::with_field(base.record(i), d, c, tmp, |f| f.len()).unwrap_or(0)).sum());
        println!("{name:<14} field {c:>2}  read {ms:>7.1} ms");
        for need in [Need::Dict, Need::Num, Need::Date, Need::Text, Need::Rank] {
            let (ms, ok) = time(&mut || { colcache::clear(); colcache::ensure(st, c, need, &no).map_or(0, |col| col.has(need) as usize + 2 * col.dict.is_some() as usize) });
            println!("{name:<14} {need:?}{:<w$} {ms:>7.1} ms  {}", "", ["none", "built", "", "dict"][ok], w = 10 - format!("{need:?}").len());
        }
    }
}
