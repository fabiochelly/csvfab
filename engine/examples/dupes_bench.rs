//! Duplicates timings: `cargo run --release -p csvfab-engine --example dupes_bench -- customers-5m.csv`
//! (customers files: first/last name 1/2, e-mail 3, phone 4, postal code 7, city 8, country 9;
//! another file's columns by `COLS=email,phone,last,first,postal`, e.g. `COLS=4,5,1,2,9`).
//! `ONLY=` a spec's first words (`ONLY=exact`: the whole row, `find_exact`). `CROSS=1`: the cross-checks suggested for the file instead, timed, with
//! `EXAMPLES=n` groups of each shown (their name, e-mail and phone columns).

use csvfab_engine::dupes::{self, Clean, DupSpec, Field};
use csvfab_engine::{Base, OpenOptions, RowId, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let t = Table::new(Arc::new(Base::open(args[0].as_ref(), OpenOptions::default(), None).expect("open")), None);
    let st = &t.st;
    let no = AtomicBool::new(false);
    let ids: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
    let reps: usize = std::env::var("REPS").ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    if std::env::var("CROSS").is_ok() { return cross(st, &ids, reps) }
    let f = |col, clean| Field { col, clean };
    // The columns e-mail, phone, last name, first name, postal code (`COLS=4,5,1,2,9` for another file).
    let c: Vec<usize> = std::env::var("COLS").ok().map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).filter(|v: &Vec<usize>| v.len() == 5).unwrap_or(vec![3, 4, 2, 1, 7]);
    let (email, phone, last, first, postal) = (c[0], c[1], c[2], c[3], c[4]);
    let specs = [
        ("e-mail", DupSpec { keys: vec![vec![f(email, Clean::Email)]] }),
        ("e-mail OR phone OR name + postal code", DupSpec { keys: vec![vec![f(email, Clean::Email)], vec![f(phone, Clean::Phone)], vec![f(last, Clean::Slug), f(first, Clean::Slug), f(postal, Clean::Digits)]] }),
        ("whole row", DupSpec::default()),
        ("last (sound) + first + postal", DupSpec { keys: vec![vec![f(last, Clean::Sound { tol: 0.8 }), f(first, Clean::Loose), f(postal, Clean::Digits)]] }),
    ];
    let only = std::env::var("ONLY").unwrap_or_default();
    // `ONLY=exact`: the whole row as `find_exact` reads it.
    if only == "exact" {
        let t0 = Instant::now();
        let d = dupes::find_exact(st, &ids, &[], &[], &no, None).unwrap();
        println!("whole row, exact {:>7.0} ms   {} groups, {} rows, digest {:016x}", t0.elapsed().as_secs_f64() * 1000.0, d.groups(), d.rows(), digest(&d));
        return;
    }
    for (what, spec) in specs {
        if !what.starts_with(only.as_str()) { continue }
        let mut ts = Vec::new();
        let mut last = None;
        for _ in 0..reps {
            let t0 = Instant::now();
            let d = dupes::find(st, &ids, &spec, &no, None).unwrap();
            ts.push(t0.elapsed().as_secs_f64() * 1000.0);
            last = Some(d);
        }
        ts.sort_by(f64::total_cmp);
        let d = last.unwrap();
        println!("{what:<46} median {:>7.0} ms   {} groups, {} rows, hits {:?}, digest {:016x}", ts[ts.len() / 2], d.groups(), d.rows(), d.hits, digest(&d));
    }
}

/// The groups, their rows, scores and reasons in one number: two runs compared.
fn digest(d: &dupes::Dupes) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut add = |x: u64| h = (h ^ x).wrapping_mul(0x100000001b3);
    for g in 0..d.groups() as u32 {
        add(g as u64);
        add(d.confidence(g) as u64);
        d.members(g).iter().for_each(|&id| add(id as u64));
        d.reason(g).bytes().for_each(|b| add(b as u64));
    }
    h
}

fn cross(st: &csvfab_engine::State, ids: &[RowId], reps: usize) {
    let no = AtomicBool::new(false);
    let t0 = Instant::now();
    let roles = dupes::roles(st);
    println!("roles in {:.0?}: {roles:?}", t0.elapsed());
    let h = st.headers();
    let show: Vec<usize> = roles.names().into_iter().chain(roles.emails.iter().copied()).chain(roles.phones.iter().copied()).chain(roles.company).chain(roles.city.or(roles.postal)).collect();
    let examples: usize = std::env::var("EXAMPLES").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    for sg in dupes::suggest(&roles, &h) {
        let mut ts = Vec::new();
        let mut last = None;
        for _ in 0..reps {
            let t0 = Instant::now();
            let d = dupes::find_rules(st, ids, &sg.rules, &roles.names(), &no, None).unwrap();
            ts.push(t0.elapsed().as_secs_f64() * 1000.0);
            last = Some(d);
        }
        ts.sort_by(f64::total_cmp);
        let d = last.unwrap();
        let (hi, mid, lo) = (d.count_scored(90, 101), d.count_scored(70, 90), d.count_scored(0, 70));
        println!("\n{:<58} median {:>6.0} ms  {} groups, {} rows (sure {hi} · likely {mid} · review {lo}), hits {:?}, digest {:016x}", sg.title, ts[ts.len() / 2], d.groups(), d.rows(), d.hits, digest(&d));
        let mut by_reason: std::collections::BTreeMap<String, Vec<u32>> = Default::default();
        for g in d.order(dupes::Order::LeastSure) { by_reason.entry(d.reason(g)).or_default().push(g) }
        for (why, gs) in &by_reason {
            println!("  [{}] {why}", gs.len());
            for &g in gs.iter().take(examples) {
                println!("    {:>3} %", d.confidence(g));
                for &id in d.members(g) {
                    let cells: Vec<String> = show.iter().map(|&c| st.cell(id, c)).collect();
                    println!("      row {:>5}: {}", id, cells.join(" | "));
                }
            }
        }
    }
}
