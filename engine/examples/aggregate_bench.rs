//! Timings of duplicates, group by, pivot and compare on a big file:
//! `cargo run --release -p csvfab-engine --example aggregate_bench -- customers-5m.csv [other.csv]`
//! (columns of the generated customers files: email 3, phone 4, last/first
//! name 2/1, postal code 7, city 8, signup date 10, total spent 13, status 14).

use csvfab_engine::dupes::{self, Clean, DupSpec, Field};
use csvfab_engine::group::{self, Aggs, GroupSpec, Match};
use csvfab_engine::pivot::{self, Dates, Fun, PivotSort, PivotSpec};
use csvfab_engine::{compare, view, Base, OpenOptions, RowId, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn open(path: &str) -> Table {
    let t0 = Instant::now();
    let t = Table::new(Arc::new(Base::open(path.as_ref(), OpenOptions::default(), None).expect("open")), None);
    println!("open {path}: {} rows in {:.0?}", t.st.len(), t0.elapsed());
    t
}

fn time<R>(what: &str, f: impl FnOnce() -> R) -> R {
    let t0 = Instant::now();
    let r = f();
    println!("{what:<46} {:>8.0?}", t0.elapsed());
    r
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let t = open(&args[0]);
    let st = &t.st;
    let no = AtomicBool::new(false);
    let ids: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
    let kinds = time("detect kinds", || view::detect_kinds(st));

    let piv = |r: usize, c: Option<usize>, fun: Fun, v: Option<usize>, by: Dates| PivotSpec { rows: r, rows_by: by, cols: c, cols_by: Dates::Month, fun, value: v };
    let s = piv(8, Some(14), Fun::Sum, Some(13), Dates::Month);
    let p = time("pivot city × status, sum of total_spent", || pivot::pivot(st, &ids, &s, &kinds, &no, None).unwrap());
    println!("  {} × {}, total {:?}", p.row_keys.len(), p.col_keys.len(), p.total);
    time("pivot: order by total + TSV", || p.to_tsv(&p.order(PivotSort { by: pivot::SortBy::Total, flip: false }), false).len());
    time("pivot drill (one city × one status)", || pivot::drill(st, &ids, &s, &kinds, Some(&p.row_keys[0]), Some(&p.col_keys[0]), &no).unwrap().len());
    let p = time("pivot signup month × status, count", || pivot::pivot(st, &ids, &piv(10, Some(14), Fun::Count, None, Dates::Month), &kinds, &no, None).unwrap());
    println!("  {} × {}", p.row_keys.len(), p.col_keys.len());
    let p = time("pivot city, distinct e-mails", || pivot::pivot(st, &ids, &piv(8, None, Fun::Distinct, Some(3), Dates::Month), &kinds, &no, None).unwrap());
    println!("  {} rows, total {:?}", p.row_keys.len(), p.total);
    let p = time("pivot e-mail (one row each), avg total_spent", || pivot::pivot(st, &ids, &piv(3, None, Fun::Avg, Some(13), Dates::Month), &kinds, &no, None).unwrap());
    println!("  {} rows", p.row_keys.len());

    let all = Aggs { sum: true, avg: true, min: true, max: true, distinct: false };
    let g = time("group by city, status (+ sum/avg/min/max)", || group::group_by(st, &ids, &GroupSpec { by: vec![8, 14], aggs: vec![(13, all), (12, Aggs::SUM)], how: Match::Loose }, &kinds, &no, None).unwrap());
    println!("  {} groups, csv {} bytes", g.len(), g.to_csv(b',').len());
    let g = time("group by e-mail (slug), count + distinct city", || group::group_by(st, &ids, &GroupSpec { by: vec![3], aggs: vec![(8, Aggs { distinct: true, ..Default::default() })], how: Match::Slug }, &kinds, &no, None).unwrap());
    println!("  {} groups", g.len());
    time("  … as CSV", || g.to_csv(b',').len());

    let f = |col, clean| Field { col, clean };
    let specs = [
        ("dupes: e-mail", DupSpec { keys: vec![vec![f(3, Clean::Email)]] }),
        ("dupes: e-mail OR phone OR name + postal code", DupSpec { keys: vec![vec![f(3, Clean::Email)], vec![f(4, Clean::Phone)], vec![f(2, Clean::Slug), f(1, Clean::Slug), f(7, Clean::Digits)]] }),
        ("dupes: whole row", DupSpec::default()),
    ];
    for (what, spec) in specs {
        let d = time(what, || dupes::find(st, &ids, &spec, &no, None).unwrap());
        println!("  {} groups, {} rows, {} to delete, hits {:?}", d.groups(), d.rows(), d.removable(), d.hits);
        time("  … rows to delete (keep first)", || d.to_delete(false).len());
    }

    let other = args.get(1).map(|p| open(p));
    let there = other.as_ref().map_or(st, |o| &o.st);
    let c = time("compare on id (loose)", || compare::compare(st, there, 0, 0, Match::Loose, &no, None).unwrap());
    println!("  same {}, changed {}, only here {}, only there {}", c.same, c.changed.len(), c.only_here.len(), c.only_there.len());
}
