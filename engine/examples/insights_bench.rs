//! Timings of the insights, the schema inference and validation on a big file:
//! `cargo run --release --example insights_bench -- file.csv`.

use csvfab_engine::{insights, schema, Base, OpenOptions, Table};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("a CSV file");
    let t0 = Instant::now();
    let base = Base::open(path.as_ref(), OpenOptions::default(), None).expect("open");
    let t = Table::new(Arc::new(base), None);
    println!("open: {:?} ({} rows)", t0.elapsed(), t.st.len());
    let no = AtomicBool::new(false);
    let h = t.st.headers();

    let t1 = Instant::now();
    let rep = insights::insights(&t.st, &no, None).unwrap();
    println!("insights: {:?} — {}", t1.elapsed(), rep.summary());
    for it in &rep.items {
        println!("  [{:?} {}] {} | {}", it.sev, it.score, it.title.plain(&h), it.sub.plain(&h));
    }
    for it in rep.items.iter().filter(|it| it.show.is_some()) {
        let t2 = Instant::now();
        let m = insights::marks(&t.st, it.show.as_ref().unwrap(), &no).unwrap();
        println!("  marks «{}»: {} rows in {:?}", m.label, m.rows.len(), t2.elapsed());
    }

    let t3 = Instant::now();
    let inf = schema::infer(&t.st, &no, None).unwrap();
    println!("schema infer: {:?}", t3.elapsed());
    print!("{}", schema::to_json(&inf.schema));
    let t4 = Instant::now();
    let v = schema::validate(&t.st, &inf.schema, &no, None).unwrap().unwrap();
    println!("schema validate: {:?} — {}", t4.elapsed(), v.summary());
}
