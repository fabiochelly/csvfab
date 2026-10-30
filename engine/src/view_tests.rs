//! The filter and sort's tests: examples, and the fast paths (column caches,
//! block search, radix sorts) against the reference (`view_old.rs`) on
//! generated tables with every awkward thing: accents, case, quotes, line
//! breaks, separators in cells, numbers and dates as people write them, edits,
//! rows and columns added, moved, removed.

use super::*;
use crate::colcache::{self, Need};
use crate::base::{Base, OpenOptions};
use crate::table::Table;
use std::sync::Arc;
use std::collections::HashSet;

fn table(s: &str) -> Table {
    Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
}

fn cells(t: &Table, ids: &[RowId], col: usize) -> Vec<String> {
    ids.iter().map(|&id| t.st.cell(id, col)).collect()
}

#[test]
fn filters() {
    let t = table("name;city;amount\nÉmile;Lyon;10\nZoé;Paris;1 200,50\nAna;Villeurbanne;\nBob;\"Lyon; 3e\";5\n");
    let no = AtomicBool::new(false);
    let run = |spec: FilterSpec| cells(&t, &filter(&t.st, &Filter::compile(&spec).unwrap(), &no).unwrap(), 0);
    assert_eq!(run(FilterSpec { global: "lyon".into(), ..Default::default() }), vec!["Émile", "Bob"]);
    assert_eq!(run(FilterSpec { global: "emile".into(), ..Default::default() }), vec!["Émile"]);
    assert_eq!(run(FilterSpec { global: "lyon".into(), invert: true, ..Default::default() }), vec!["Zoé", "Ana"]);
    assert_eq!(run(FilterSpec { cols: vec![(2, ">8".into())], ..Default::default() }), vec!["Émile", "Zoé"]);
    assert_eq!(run(FilterSpec { cols: vec![(2, "=".into())], ..Default::default() }), vec!["Ana"]);
    assert_eq!(run(FilterSpec { cols: vec![(1, "=lyon".into())], ..Default::default() }), vec!["Émile"]);
    assert_eq!(run(FilterSpec { global: "^[ZB]".into(), regex: true, ..Default::default() }), vec!["Zoé", "Bob"]);
}

/// The search as the web's: every word somewhere in the row, any order, across cells;
/// "a phrase" kept whole; inverted, the rows that do not hold them all. Many rows, so the
/// block search runs too.
#[test]
fn search_terms() {
    assert_eq!(terms("dupont  lyon"), vec!["dupont", "lyon"]);
    assert_eq!(terms("\"rue de la paix\" paris"), vec!["rue de la paix", "paris"]);
    assert_eq!(terms("  "), vec!["  "]);
    let mut csv = String::from("name,city,street\n");
    for i in 0..200 {
        csv += &match i % 4 {
            0 => format!("Dupont {i},Lyon,rue de la Paix\n"),
            1 => format!("Durand {i},Lyon,rue de Paris\n"),
            2 => format!("Dupont {i},Paris,la paix rue\n"),
            _ => format!("Émile {i},Nice,Promenade\n"),
        };
    }
    let t = table(&csv);
    let no = AtomicBool::new(false);
    let n = |g: &str, invert: bool| filter(&t.st, &Filter::compile(&FilterSpec { global: g.into(), invert, ..Default::default() }).unwrap(), &no).unwrap().len();
    assert_eq!(n("dupont lyon", false), 50);
    assert_eq!(n("lyon dupont", false), 50);
    assert_eq!(n("dupont", false), 100);
    assert_eq!(n("\"rue de la paix\"", false), 50);
    assert_eq!(n("rue paix", false), 100);
    assert_eq!(n("dupont lyon", true), 150);
    assert_eq!(n("emile nice", false), 50);
    // A column filter in plain text: its words too, in that column.
    let c = |text: &str| filter(&t.st, &Filter::compile(&FilterSpec { cols: vec![(2, text.into())], ..Default::default() }).unwrap(), &no).unwrap().len();
    assert_eq!(c("paix rue"), 100);
    assert_eq!(c("\"rue de\""), 100);
    assert_eq!(c("paix lyon"), 0);
}

#[test]
fn sorts() {
    let t = table("name,amount,date\nb,10,15/03/2024\nA,9,2023-01-01\nc,,01/01/2025\nÉ,100,\n");
    let no = AtomicBool::new(false);
    let kinds = detect_kinds(&t.st);
    assert_eq!(kinds, vec![Kind::Text, Kind::Number, Kind::Date]);
    let all = filter(&t.st, &Filter::compile(&FilterSpec::default()).unwrap(), &no).unwrap();
    let mut ids = all.clone();
    sort(&t.st, &mut ids, &[SortKey { col: 1, desc: false }], &kinds, &no);
    assert_eq!(cells(&t, &ids, 0), vec!["A", "b", "É", "c"]);
    let mut ids = all.clone();
    sort(&t.st, &mut ids, &[SortKey { col: 1, desc: true }], &kinds, &no);
    assert_eq!(cells(&t, &ids, 0), vec!["É", "b", "A", "c"]);
    let mut ids = all.clone();
    sort(&t.st, &mut ids, &[SortKey { col: 0, desc: false }], &kinds, &no);
    assert_eq!(cells(&t, &ids, 0), vec!["A", "b", "c", "É"]);
    let mut ids = all;
    sort(&t.st, &mut ids, &[SortKey { col: 2, desc: false }], &kinds, &no);
    assert_eq!(cells(&t, &ids, 0), vec!["A", "b", "c", "É"]);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[self.below(v.len())]
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

const WORDS: &[&str] = &[
    "Lyon", "lyon", "LYON", "Émile", "emile", "Zoé", "Straße", "œuvre", "Œuf", "Ærø", "  Lyon ", "a\u{a0}b", "Ly", "on", "\u{212A}elvin",
    "x;y", "x,y", "say \"hi\"", "line\nbreak", "", " ", "日本", "İstanbul", "ﬁne", "Ångström", ";", "\u{37e}", "tab\there", "Paris",
    "paris ", "Villeurbanne", "Dubois, Laurent & Associés", "associes", "  ", "LyOn-sur-Saône", "\u{2003}x\u{2003}", "-", "é", "E\u{301}mile", "a:b", "x:", "Lÿôn", "ŁYŃ", "5 €", "ℓyon",
];
const NUMS: &[&str] = &[
    "1234.5", "1 234,50 €", "-12,5 %", "1,234,567", "(42)", "-0", "0", "0.0", "1e3", "", "abc", "15.03.2024", "2024-03-15", "1e999",
    "-1e999", "75011", " 12 ", "10", "9", "100", "-3", "1.5", "2", "2", "1,5", "+7",
];
const DATES: &[&str] = &[
    "2024-03-15", "15/03/2024", "2023-01-01 10:20", "", "13/13/2024", "2024-03-15T10:20:30Z", "1999-12-31", "01/01/2025", "2024-03-15",
];

fn cell(r: &mut Rng, col: usize) -> String {
    match col % 4 {
        0 => r.pick(WORDS).to_string(),
        1 => r.pick(NUMS).to_string(),
        2 => r.pick(DATES).to_string(),
        _ => {
            // Many distinct values: no dictionary.
            let w = r.pick(WORDS);
            format!("{w}{}", r.below(100_000))
        }
    }
}

fn csv(r: &mut Rng, rows: usize, cols: usize, delim: u8) -> String {
    let d = delim as char;
    let mut s = String::new();
    for c in 0..cols {
        if c > 0 { s.push(d); }
        s.push_str(&format!("c{c}"));
    }
    s.push('\n');
    for _ in 0..rows {
        // Now and then a record a field short or long.
        let n = match r.below(40) { 0 => cols - 1, 1 => cols + 1, _ => cols };
        for c in 0..n {
            if c > 0 { s.push(d); }
            let v = cell(r, c);
            let must = v.contains(['"', '\n', '\r', d]) || v.starts_with(' ') || v.ends_with(' ');
            if must || r.chance(10) {
                s.push('"');
                s.push_str(&v.replace('"', "\"\""));
                s.push('"');
                // Spaces between a closing quote and the delimiter.
                if r.chance(5) { s.push_str("  "); }
            } else {
                s.push_str(&v);
            }
        }
        s.push_str(if r.chance(10) { "\r\n" } else { "\n" });
    }
    s
}

const NEEDLES: &[&str] = &["a:b", ":", "lyon", "LY", "émile", "e", "o", " ", "  ", "\t", "ss", "oe", "x;y", "\"", "日", "k", "i", "1", "2024", "ﬁ", "Lyon", "é", "-", "ass", "zzz", "on ", "\u{37e}"];
const REGEXES: &[&str] = &["^Ly", "on$", "[0-9]+", "é", "a|b", "^$", r"\d{4}-\d{2}", "x;y", "\"", ".*", "(?i)LYON", "^ *Lyon", "s{2}", "lyon|paris", "\\bon"];
const COLTESTS: &[&str] = &["lyon", "=lyon", "=", "!", "!lyon", "!=Lyon", ">10", ">=2024-01-01", "<0", "<=1234.5", "e", "=Émile", "=lyon ", ">-1e999", "<15.03.2024", "ly", "=2", "!e", "on"];

fn spec(r: &mut Rng, cols: usize) -> FilterSpec {
    let mut s = FilterSpec { regex: r.chance(15), match_case: r.chance(20), invert: r.chance(20), ..Default::default() };
    if r.chance(70) {
        s.global = if s.regex { r.pick(REGEXES) } else { r.pick(NEEDLES) }.to_string();
    }
    for _ in 0..r.below(3) {
        let t = if s.regex { r.pick(REGEXES) } else { r.pick(COLTESTS) };
        s.cols.push((r.below(cols + 1), t.to_string()));
    }
    if r.chance(25) {
        let c = r.below(cols);
        let k = 1 + r.below(4);
        let h: HashSet<String> = (0..k).map(|_| cell(r, c).trim().to_string()).collect();
        s.hidden.push((c, Arc::new(h)));
    }
    s
}

/// Equal row lists, or where they part.
fn same(a: Vec<RowId>, b: Vec<RowId>, what: &str) {
    if a != b {
        let k = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
        panic!("{what}: {} vs {} rows, first difference at {k}: {:?} vs {:?}", a.len(), b.len(), a.get(k), b.get(k));
    }
}

/// The fast paths agree with the reference on this table, cached or not.
fn agree(t: &Table, r: &mut Rng, rounds: usize) {
    let st = &t.st;
    let no = AtomicBool::new(false);
    let cols = st.ncols;
    let all = filter(st, &Filter::compile(&FilterSpec::default()).unwrap(), &no).unwrap();
    assert_eq!(all, old::filter(st, &old::Filter::compile(&FilterSpec::default()).unwrap(), &no).unwrap());
    let mut subset: Vec<RowId> = all.iter().copied().filter(|_| r.chance(60)).collect();
    for k in (1..subset.len()).rev() {
        if r.chance(30) { subset.swap(k, r.below(k + 1)); }
    }
    for _ in 0..rounds {
        let sp = spec(r, cols);
        let (Ok(f), Ok(fo)) = (Filter::compile(&sp), old::Filter::compile(&sp)) else { continue };
        same(filter(st, &f, &no).unwrap(), old::filter(st, &fo, &no).unwrap(), &format!("{sp:?}"));
        same(filter_ids(st, &f, &subset, &no).unwrap(), old::filter_ids(st, &fo, &subset, &no).unwrap(), &format!("{sp:?} (subset)"));
    }
    let detected = detect_kinds(st);
    for _ in 0..rounds {
        let kinds: Vec<Kind> = if r.chance(50) { detected.clone() } else { (0..cols).map(|_| [Kind::Text, Kind::Number, Kind::Date][r.below(3)]).collect() };
        let nk = 1 + r.below(3);
        let keys: Vec<SortKey> = (0..nk).map(|_| SortKey { col: r.below(cols + 1), desc: r.chance(50) }).collect();
        for input in [&all, &subset] {
            let (mut a, mut b) = (input.clone(), input.clone());
            assert!(sort(st, &mut a, &keys, &kinds, &no));
            assert!(old::sort(st, &mut b, &keys, &kinds, &no));
            same(a, b, &format!("{keys:?} {kinds:?}"));
        }
    }
}

/// Every cache of every column, built now.
fn cache_all(t: &Table) {
    let no = AtomicBool::new(false);
    for c in 0..t.st.ncols {
        for need in [Need::Dict, Need::Text, Need::Rank, Need::Num, Need::Date] {
            colcache::ensure(&t.st, c, need, &no);
        }
    }
}

fn edit(t: &mut Table, r: &mut Rng) {
    let n = t.st.len();
    let cols = t.st.ncols;
    let ch: Vec<(RowId, usize, String)> = (0..20)
        .map(|_| {
            let (p, c, k) = (r.below(n), r.below(cols), r.below(4));
            (t.st.data_id(p), c, cell(r, k))
        })
        .collect();
    t.set_cells(&ch);
    match r.below(5) {
        0 => { let ids = t.insert_rows(r.below(n), 3); t.set_cells(&ids.iter().map(|&id| (id, 0, "Lyon".to_string())).collect::<Vec<_>>()); }
        1 => { let del: HashSet<RowId> = (0..5).map(|_| t.st.data_id(r.below(n - 5))).collect(); t.delete_rows(&del); }
        2 => t.move_column(0, cols - 1),
        3 => t.insert_column(1, "new"),
        _ => {}
    }
}

#[test]
fn fast_paths_agree() {
    for seed in 1..=6u64 {
        let mut r = Rng(0x9E3779B97F4A7C15 ^ seed * 0x2545F4914F6CDD1D);
        let delim = [b',', b';', b'\t', b'|'][seed as usize % 4];
        let (rows, cols) = (1500 + r.below(1500), 4 + r.below(4));
        let mut t = table_with(&csv(&mut r, rows, cols, delim));
        agree(&t, &mut r, 40);
        cache_all(&t);
        agree(&t, &mut r, 40);
        for _ in 0..3 {
            edit(&mut t, &mut r);
            agree(&t, &mut r, 15);
        }
    }
}

/// Bytes that are not UTF-8 (read as UTF-8, and as Windows-1252).
#[test]
fn fast_paths_agree_other_bytes() {
    use crate::Enc;
    for (seed, enc, delim) in [(11u64, Enc::Utf8, None), (12, Enc::Windows1252, None), (13, Enc::MacRoman, None), (14, Enc::Utf8, Some(0)), (15, Enc::Windows1252, Some(0)), (16, Enc::Utf8, Some(b':'))] {
        let mut r = Rng(0x2545F4914F6CDD1D ^ seed);
        let text = csv(&mut r, 2500, 5, delim.filter(|&d| d != 0).unwrap_or(b';'));
        // Some characters as single bytes, some broken sequences.
        let mut bytes = Vec::new();
        for ch in text.chars() {
            match ch {
                'é' if r.chance(50) => bytes.push(0xE9),
                'ö' | 'ß' if r.chance(50) => bytes.push(0xF6),
                'o' if r.chance(3) => bytes.extend([0xC3]),
                'n' if r.chance(2) => bytes.extend([0xFF, b'n']),
                c => bytes.extend(c.to_string().as_bytes()),
            }
        }
        let opts = OpenOptions { enc: Some(enc), delim };
        let t = Table::new(Arc::new(Base::from_bytes(bytes, opts)), Some(true));
        assert_eq!(t.st.base.enc, enc);
        agree(&t, &mut r, 40);
        cache_all(&t);
        agree(&t, &mut r, 40);
    }
}

/// Past the chunk sizes (filter chunks, cache arenas, dictionary sampling).
#[test]
fn fast_paths_agree_big() {
    let mut r = Rng(0x1234_5678_9ABC_DEF1);
    let mut t = table_with(&csv(&mut r, 140_000, 5, b','));
    agree(&t, &mut r, 12);
    cache_all(&t);
    agree(&t, &mut r, 12);
    edit(&mut t, &mut r);
    agree(&t, &mut r, 6);
}

fn table_with(s: &str) -> Table {
    Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), Some(true))
}

#[test]
fn refines() {
    let g = |s: &str| FilterSpec { global: s.into(), ..Default::default() };
    assert!(g("lyon").refines(&g("ly")));
    assert!(g("Lyon").refines(&g("ly")));
    assert!(!g("ly").refines(&g("lyon")));
    assert!(g("ly").refines(&FilterSpec::default()));
    let inv = |s: &str| FilterSpec { global: s.into(), invert: true, ..Default::default() };
    assert!(inv("ly").refines(&inv("lyon")));
    assert!(!inv("lyon").refines(&inv("ly")));
    let c = |s: &str| FilterSpec { cols: vec![(1, s.into())], ..Default::default() };
    assert!(c("lyon").refines(&c("ly")));
    assert!(!c(">10").refines(&c(">1")));
    assert!(!c("=ly").refines(&c("ly")));
    let both = FilterSpec { global: "a".into(), cols: vec![(1, "ly".into())], ..Default::default() };
    assert!(both.refines(&c("ly")) && both.refines(&g("a")) && !c("ly").refines(&both));
    let mut h1 = HashSet::new();
    h1.insert("x".to_string());
    let mut h2 = h1.clone();
    h2.insert("y".to_string());
    let hid = |h: &HashSet<String>| FilterSpec { hidden: vec![(0, Arc::new(h.clone()))], ..Default::default() };
    assert!(hid(&h2).refines(&hid(&h1)) && !hid(&h1).refines(&hid(&h2)));
}

/// Columns moved or added: the records are still searched as raw bytes, block by block, and
/// find what a reading of their cells finds — an empty cell matched (an added column, a short
/// record), a removed column's text not found, edited rows read from their cells.
#[test]
fn search_after_columns_moved_or_added() {
    let mut src = String::from("a,b,c\n");
    for i in 0..3000 {
        src.push_str(&match i % 7 { 0 => format!("x{i},\"q, {i}\"\n"), 3 => format!("lyon{i},Émile,{i},extra\n"), _ => format!("n{i},Lyon {i},{}\n", i % 10) });
    }
    let no = AtomicBool::new(false);
    let edits: [fn(&mut Table); 4] = [|t| t.move_column(0, 2), |t| t.insert_column(1, "new"), |t| t.delete_columns(&[1]), |t| {
        let id = t.st.data_id(40);
        t.set_cells(&[(id, 0, "lyon edited".into())]);
    }];
    let mut t = table(&src);
    for e in edits {
        e(&mut t);
        for (g, regex) in [("lyon", false), ("emile", false), ("q, 1", false), ("extra", false), ("^$", true), ("^x1", true), ("9$", true)] {
            let spec = FilterSpec { global: g.into(), regex, ..Default::default() };
            let got = filter(&t.st, &Filter::compile(&spec).unwrap(), &no).unwrap();
            assert_eq!(got, old::filter(&t.st, &old::Filter::compile(&spec).unwrap(), &no).unwrap(), "{g}");
        }
    }
}
