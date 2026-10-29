//! Timings on a million rows: `cargo test -p csvfab-engine --release edit::bench -- --ignored --nocapture`
//! (CSVFAB_BENCH: the file, else the session's customers-1m.csv).

use super::series::Rect;
use super::*;
use crate::base::{Base, OpenOptions};
use std::sync::Arc;
use std::time::Instant;

fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    s.split_whitespace().nth(1).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) * 4096.0 / 1048576.0
}

fn time<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let t0 = Instant::now();
    let r = f();
    println!("{what:<48} {:>8.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    r
}

#[test]
#[ignore]
fn bench_1m() {
    let path = std::env::var("CSVFAB_BENCH").unwrap_or_else(|_| {
        "../benchmark/data/customers-1m.csv".into()
    });
    let open = || Table::new(Arc::new(Base::open(std::path::Path::new(&path), OpenOptions::default(), None).unwrap()), None);
    let mut t = time("open", open);
    let no = AtomicBool::new(false);
    let st = t.st.clone();
    let ids = all_rows(&st);
    let h = st.headers();
    let c = |name: &str| h.iter().position(|x| x == name).unwrap();
    println!("{} rows × {} columns", ids.len(), st.ncols);

    let rep = |find: &str, with: &str, regex: bool, case: bool| {
        replace::Replace::new(&replace::ReplaceSpec { find: find.into(), replace: with.into(), regex, match_case: case }, &st).unwrap().unwrap()
    };
    let r = rep("Street", "St", false, true);
    let n = time("replace 'Street' (case) in street", || changes(&st, &ids, &[c("street")], &r, &no).unwrap().len());
    println!("  {n} cells");
    let r = rep("street", "St", false, false);
    let n = time("replace 'street' (no case) in street", || changes(&st, &ids, &[c("street")], &r, &no).unwrap().len());
    println!("  {n} cells");
    let r = rep("zzqx", "", false, false);
    time("replace, no match, all 15 columns", || changes(&st, &ids, &(0..st.ncols).collect::<Vec<_>>(), &r, &no).unwrap().len());
    let r = rep(r"^(\d+) (\w+)", "$2 n°$1", true, true);
    let n = time("regex replace in street", || changes(&st, &ids, &[c("street")], &r, &no).unwrap().len());
    println!("  {n} cells");
    let n = time("bulk upper case of city (preview)", || scan(&st, &ids, &[c("city")], &replace::Bulk::Upper, false, &no).unwrap().changed);
    println!("  {n} cells");

    use convert_values::*;
    time("convert: guess (signup_date)", || guess(&st, &ids, c("signup_date"), "signup_date", "33"));
    let conv = Converter::new(Convert::Date { month_first: false, out: DateOut::Dmy });
    time("convert dates ISO → dd/mm/yyyy (preview)", || scan(&st, &ids, &[c("signup_date")], &conv, false, &no).unwrap());
    let ch = time("convert dates ISO → dd/mm/yyyy (changes)", || changes(&st, &ids, &[c("signup_date")], &conv, &no).unwrap());
    let m0 = rss_mb();
    time("  Table::set_cells of those 1M cells", || t.set_cells(&ch));
    println!("  RSS +{:.0} MB for {} edited rows", rss_mb() - m0, ch.len());
    drop(ch);
    time("  undo", || t.undo());
    let conv = Converter::new(Convert::Phone { cc: "33".into(), out: PhoneOut::E164 });
    let s = time("convert phones → E.164 (preview)", || scan(&st, &ids, &[c("phone")], &conv, false, &no).unwrap());
    println!("  {} change, {} same, {} bad", s.changed, s.same, s.bad);
    let conv = Converter::new(Convert::Number { dec: '.', out: NumOut::Fr, decimals: Some(2) });
    time("convert numbers → 1 234,50 (preview)", || scan(&st, &ids, &[c("total_spent")], &conv, false, &no).unwrap());

    let cs = time("clean: scan (all cells)", || clean::scan(&st, &no).unwrap());
    println!("  counts {:?}", cs.counts);
    let on: Vec<_> = clean::CLEAN_OPS.iter().filter(|o| o.2).map(|o| o.0).collect();
    let p = time("clean: plan (default options)", || clean::plan(&st, &on, &cs, &no).unwrap());
    println!("  {} cells, {} rows", p.changes.len(), p.rows.len());

    let vis: Vec<usize> = (0..st.ncols).collect();
    let all = Rect { r0: 0, r1: ids.len() - 1, c0: c("orders"), c1: c("orders") };
    let n = time("series: Ctrl+Enter '1' down 1M rows", || series::fill_value(&st, &ids, &vis, all, "1", true, &no).unwrap().len());
    println!("  {n} cells");
    let all = Rect { c0: c("signup_date"), c1: c("signup_date"), ..all };
    time("series: date down 1M rows", || series::fill_value(&st, &ids, &vis, all, "2024-01-31", true, &no).unwrap().len());
    let one = Rect { r0: 5, r1: 5, c0: c("company"), c1: c("company") };
    time("fill blanks from above (whole column)", || series::fill_blanks(&st, &ids, &vis, one, &no).unwrap().unwrap().len());

    let sp = columns::Splitter::new("@", false, true, true).unwrap().unwrap();
    let s = time("split email on @: analyse", || columns::analyse(&st, &ids, c("email"), &sp, &no).unwrap());
    let n = s.proposed();
    time("split email on @: plan", || columns::split_plan(&st, &ids, c("email"), &sp, n, false, columns::split_names("email", &[], n), &no).unwrap().changes.len());
    time("merge first + last name: plan", || columns::merge_plan(&st, &ids, &[c("first_name"), c("last_name")], " ", true, true, "", &no).unwrap().unwrap().changes.len());

    let src = time("lookup: open the source", open);
    let sids = all_rows(&src.st);
    let idx = time("lookup: index on id", || lookup::Index::build(&src.st, &sids, 0, lookup::Match::Loose, &no).unwrap());
    time("lookup: stats", || lookup::stats(&st, &ids, 0, &idx, &no).unwrap());
    let n = time("lookup: plan (2 columns)", || lookup::plan(&st, &ids, 0, &src.st, &idx, &[c("email"), c("city")], vec!["a".into(), "b".into()], &no).unwrap().changes.len());
    println!("  {n} cells");

    let sel = Rect { r0: 0, r1: ids.len() - 1, c0: 0, c1: 3 };
    let cp = time("copy 1M × 4 as TSV", || clipboard::copy(&st, &ids, &vis, sel, &no).unwrap());
    println!("  {} MB", cp.text.len() / 1048576);
    let g = time("parse that TSV", || clipboard::parse_tsv(&cp.text));
    time("paste it back (no change)", || clipboard::paste(&st, &ids, &vis, sel, &g, false, true, &no).unwrap().changes.len());

    let del: Vec<RowId> = ids.iter().copied().step_by(2).collect();
    time("hidden rows (half shown)", || replace::hidden_rows(&st, &del, true, &no).unwrap().unwrap().len());
}
