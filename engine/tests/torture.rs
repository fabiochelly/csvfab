//! Torture tests: many random and hostile files and edit sequences, seeded.
//!
//! Every test runs cases from one seed (fixed by default: the suite is reproducible).
//! `TORTURE_SEED=<n>` replays from that seed (`random`: a new one each run), `TORTURE_N=<k>`
//! multiplies the number of cases. A failing case prints its own seed: the same test run with
//! `TORTURE_SEED=<it>` replays it first.

use csvfab_engine::base::{detect_dialect, Base, OpenOptions};
use csvfab_engine::enc::{self, Enc};
use csvfab_engine::save::{self, SaveOptions};
use csvfab_engine::scan;
use csvfab_engine::table::{RowId, Table};
use csvfab_engine::colcache::{self, Need};
use csvfab_engine::view::{self, FilterSpec, Kind, SortKey};
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

// ---- harness ------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn pick<'a, T: ?Sized>(&mut self, v: &[&'a T]) -> &'a T {
        v[self.below(v.len())]
    }
}

fn env_seed() -> u64 {
    match std::env::var("TORTURE_SEED").ok().as_deref() {
        None | Some("") => 0x00C5_F0AB_2026,
        Some("random") => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64,
        Some(s) => s.parse().expect("TORTURE_SEED: a number or `random`"),
    }
}

/// `n` times `TORTURE_N`.
fn scale(n: usize) -> usize {
    n * std::env::var("TORTURE_N").ok().and_then(|s| s.parse().ok()).unwrap_or(1usize)
}

/// Runs `cases` cases of `f`, each from its own seed (the first one: the seed itself); a
/// failing case tells how to replay it.
fn cases(name: &str, n: usize, f: impl Fn(&mut Rng, u64)) {
    let seed = env_seed();
    for i in 0..scale(n) {
        let s = if i == 0 { seed } else { seed.wrapping_mul(6364136223846793005).wrapping_add((i as u64).wrapping_mul(1442695040888963407)) };
        let r = catch_unwind(AssertUnwindSafe(|| f(&mut Rng::new(s), s)));
        if let Err(e) = r {
            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            panic!("{name}: case {i} failed: {msg}\n  replay (it is the first case then): TORTURE_SEED={s} cargo test -p csvfab-engine --test torture {name}");
        }
    }
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("csvfab-torture-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn show(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(&b[..b.len().min(600)]).into_owned();
    format!("{s:?}{}", if b.len() > 600 { format!("… ({} bytes)", b.len()) } else { String::new() })
}

// ---- generators -----------------------------------------------------------------

const WORDS: &[&str] = &[
    "a", "bc", "Lyon", "Saint-Étienne", "café crème", "12", "-3.5", "1 234,56", "0", "007", "1e9", "NaN", "inf", "2024-02-29", "31/12/1999",
    "12:30", "true", "", " ", "  padded  ", "x y", "日本語", "😀👍🏽", "e\u{301}", "ﬁ", "İstanbul", "ß", "Ǆ", "\u{200B}", "\u{FEFF}", "مرحبا",
    "jean.dupont@example.com", "+33 6 12 34 56 78", "06 12 34 56 78", "N/A", "null", "-", "\u{FFFD}", "tab\there", "a\u{0}b",
];

/// A field as it may appear in a file: plain, quoted, broken.
fn raw_field(r: &mut Rng, d: u8) -> Vec<u8> {
    let w = r.pick(WORDS).as_bytes().to_vec();
    match r.below(16) {
        0 => { let mut v = b"\"".to_vec(); v.extend(&w); v.push(b'"'); v }
        1 => { let mut v = b"\"".to_vec(); v.extend(&w); v.extend(b"\"\"inner\"\" "); v.push(d); v.extend(b"\n\r\n\"" as &[u8]); v }
        2 => b"\"unclosed".to_vec(),
        3 => { let mut v = w.clone(); v.extend(b"\"stray"); v }
        4 => b"\"closed\"junk".to_vec(),
        5 => b"\"x\"  ".to_vec(),
        6 => vec![0xE9, b'l', 0xE8, b'v', 0xE9],              // Latin-1 bytes
        7 => vec![0xC3, 0x28, 0xFF, 0xFE, 0x80],              // invalid UTF-8
        8 => vec![0],
        9 => b"\"\"".to_vec(),
        10 => b"\r".to_vec(),
        _ => w,
    }
}

const DELIMS: &[u8] = b",;\t|";
const EOLS: &[&[u8]] = &[b"\n", b"\r\n", b"\r", b"\n\n", b"\r\n\r\n", b"\n \n", b"\n\t\n"];

/// A hostile CSV: regular-ish rows of hostile fields, with now and then something broken.
fn gen_csv(r: &mut Rng) -> Vec<u8> {
    let d = if r.chance(90) { DELIMS[r.below(4)] } else { *r.pick(&[&b' ', &b':', &b'#', &b'~']) };
    let eol: &[u8] = if r.chance(70) { EOLS[r.below(3)] } else { EOLS[r.below(EOLS.len())] };
    let mut b = Vec::new();
    match r.below(12) {
        0 => b.extend(b"\xEF\xBB\xBF"),
        1 if r.chance(30) => b.extend(b"\n\n"),
        _ => {}
    }
    let rows = match r.below(10) { 0 => 0, 1 => 1, 2..=6 => r.range(2, 20), _ => r.range(20, 400) };
    let width = match r.below(10) { 0 => 1, 1 => r.range(30, 120), _ => r.range(2, 8) };
    for row in 0..rows {
        let w = if r.chance(15) { r.range(0, width + 3) } else { width };
        for c in 0..w {
            if c > 0 { b.push(d); }
            if row == 0 && r.chance(60) { b.extend(format!("col{c}").as_bytes()); continue; }
            if r.chance(25) { b.extend(raw_field(r, d)); } else { b.extend(r.pick(WORDS).as_bytes()); }
        }
        if r.chance(5) { b.push(d); }                                         // trailing delimiter
        let e: &[u8] = if r.chance(8) { EOLS[r.below(EOLS.len())] } else { eol };
        if row + 1 < rows || r.chance(70) { b.extend(e); }                   // no final newline sometimes
    }
    if r.chance(5) { b.extend(b"   \n\t\n"); }
    b
}

/// Random bytes, biased towards the ones a scanner cares about.
fn gen_bytes(r: &mut Rng) -> Vec<u8> {
    let n = match r.below(4) { 0 => r.below(8), 1 => r.below(64), 2 => r.below(1000), _ => r.below(5000) };
    let special = b"\",;\t|\n\r \0\xEF\xBB\xBF\xFF\xFE\xC3\xE9";
    (0..n).map(|_| if r.chance(60) { special[r.below(special.len())] } else { r.next() as u8 }).collect()
}

/// A random UTF-16 file (BOM or not), sometimes with lone surrogates or an odd length.
fn gen_utf16(r: &mut Rng) -> Vec<u8> {
    let text = String::from_utf8_lossy(&gen_csv(r)).into_owned();
    let be = r.chance(50);
    let mut out = if r.chance(70) { if be { vec![0xFE, 0xFF] } else { vec![0xFF, 0xFE] } } else { Vec::new() };
    enc::utf8_to_utf16(&text, be, &mut out);
    if r.chance(15) { out.extend(if be { [0xD8, 0x00] } else { [0x00, 0xD8] }); out.extend(if be { [0, b'x'] } else { [b'x', 0] }); }
    if r.chance(10) { out.push(b'z'); }
    out
}

/// One of the special shapes.
fn gen_special(r: &mut Rng) -> Vec<u8> {
    match r.below(14) {
        0 => Vec::new(),
        1 => b"\n\n\n\r\n\r\r".to_vec(),
        2 => b"only,a,header".to_vec(),
        3 => b"only,a,header\n".to_vec(),
        4 => b"   \n\t\n  \n".to_vec(),
        5 => b"\xEF\xBB\xBF".to_vec(),
        6 => vec![0xFF, 0xFE],
        7 => b"\"".to_vec(),
        8 => b"\"\"\"".to_vec(),
        9 => b",,,\n,,,\n".to_vec(),
        10 => b"a,b\n1,\"never closed\n2,3\n4,5\n".to_vec(),
        11 => vec![0u8; r.range(1, 3000)],
        12 => b"a\rb\rc\r\"d\re\"\r".to_vec(),
        _ => b"a;b\r\n1;2\r\n\r\n\r\n3;4".to_vec(),
    }
}

fn any_file(r: &mut Rng) -> Vec<u8> {
    match r.below(10) {
        0 | 1 => gen_bytes(r),
        2 => gen_utf16(r),
        3 => gen_special(r),
        _ => gen_csv(r),
    }
}

/// A value an edit may put in a cell.
fn gen_value(r: &mut Rng) -> String {
    match r.below(14) {
        0 => String::new(),
        1 => "\"".into(),
        2 => "a,b;c\td|e".into(),
        3 => "line\nbreak".into(),
        4 => "cr\ronly".into(),
        5 => "crlf\r\n".into(),
        6 => "nul\0byte".into(),
        7 => " lead and trail ".into(),
        8 => "😀 émoji ünïcode 日本".into(),
        9 => "x".repeat(r.range(1, 20_000)),
        10 => "\"quoted\"".into(),
        11 => "\n".into(),
        12 => (0..r.range(1, 12)).map(|_| *r.pick(&[&'"', &',', &'\n', &'\r', &' ', &'a', &';', &'\t', &'é', &'|'])).collect(),
        _ => r.pick(WORDS).to_string(),
    }
}

// ---- checks -----------------------------------------------------------------------

fn table_of(bytes: &[u8], o: OpenOptions) -> Table {
    Table::new(Arc::new(Base::from_bytes(bytes.to_vec(), o)), None)
}

fn saved(t: &Table) -> Vec<u8> {
    save::write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap()
}

/// Whether the file read as UTF-16 comes back as it was once written again (no lone
/// surrogate, no odd byte): only then can a save give its bytes back.
fn utf16_exact(bytes: &[u8], b: &Base) -> bool {
    if !b.enc.is_utf16() { return true; }
    let mut back = if b.bom { b.enc.bom().to_vec() } else { Vec::new() };
    enc::utf8_to_utf16(std::str::from_utf8(b.bytes()).unwrap(), b.enc == Enc::Utf16Be, &mut back);
    back == bytes
}

/// Everything a reader of the base may ask, on every record: no panic, sane answers.
fn read_all(b: &Base, ctx: &str) {
    assert_eq!(b.starts.len(), b.n + 1, "{ctx}: starts");
    let mut joined = b.prefix().to_vec();
    let mut f = Vec::new();
    for i in 0..b.n {
        assert!(b.starts.get(i) < b.starts.get(i + 1) || b.dialect.lines, "{ctx}: record {i} empty");
        joined.extend_from_slice(b.span(i));
        b.fields(i, &mut f);
        assert!(!f.is_empty(), "{ctx}: record {i} has no field");
        let _ = b.record(i);
    }
    assert_eq!(joined.len(), b.bytes().len(), "{ctx}: spans do not cover the file");
    assert!(joined == b.bytes(), "{ctx}: spans differ from the file");
    if b.enc.mem_utf8() && b.enc != Enc::Utf8 { assert!(std::str::from_utf8(b.bytes()).is_ok()); }
}

/// Every cell of the table read three ways: they agree.
fn cells_agree(t: &Table, ctx: &str) {
    let st = &t.st;
    let mut row = Vec::new();
    let mut tmp = Vec::new();
    for p in 0..st.all_len() {
        let id = st.all_id(p);
        st.row(id, &mut row);
        for c in 0..st.ncols {
            let v = st.cell(id, c);
            st.with_cell(id, c, &mut tmp, |s| assert_eq!(s, v, "{ctx}: with_cell"));
            assert_eq!(row.get(c).map_or("", |s| s.as_str()), v, "{ctx}: row {p} col {c}");
        }
        // The one-scan visitors: the very fields `row` gives, the very cells `with_cell` gives.
        let mut seen: Vec<String> = Vec::new();
        st.visit(id, |c, v| {
            assert_eq!(c, seen.len(), "{ctx}: visit, row {p}: columns in order");
            seen.push(v.to_string());
        });
        assert_eq!(seen, row, "{ctx}: visit, row {p}");
        for cols in [(0..st.ncols + 2).step_by(2).collect::<Vec<_>>(), (0..st.ncols + 1).rev().chain([0, 1, 0]).collect()] {
            let mut n = 0;
            st.visit_cols(id, &cols, |k, v| {
                assert_eq!(v, st.cell(id, cols[k]), "{ctx}: visit_cols, row {p} col {}", cols[k]);
                n += 1;
            });
            assert_eq!(n, cols.len(), "{ctx}: visit_cols, row {p}");
        }
    }
    let _ = st.headers();
}

// ---- 1. parsing ----------------------------------------------------------------------

/// Any bytes open, every record reads, the spans give the file back, the parallel scan is the
/// sequential one, and saving untouched gives the very bytes read.
#[test]
fn any_bytes_open_and_round_trip() {
    cases("any_bytes_open_and_round_trip", 800, |r, _| {
        let bytes = any_file(r);
        let ctx = show(&bytes);
        let t = table_of(&bytes, OpenOptions::default());
        let b = &t.st.base;
        read_all(b, &ctx);
        cells_agree(&t, &ctx);
        if utf16_exact(&bytes, b) {
            assert!(saved(&t) == bytes, "{ctx}: untouched save differs: {}", show(&saved(&t)));
        }
        // The sequential scanner finds the same records.
        let d = b.dialect;
        let (seq, _) = scan::scan_range(b.bytes(), 0, b.bytes().len(), d, None, usize::MAX, None);
        assert_eq!(seq.starts.len(), b.n, "{ctx}: sequential scan");
        for i in 0..b.n { assert_eq!(seq.starts.get(i), b.starts.get(i), "{ctx}: record {i}"); }
        // Forced the other ways: still no panic, and the bytes still come back.
        for delim in [None, Some(b','), Some(b';'), Some(b'\t'), Some(b'|'), Some(0), Some(b' '), Some(b'"'), Some(b'\n'), Some(b'\r'), Some(0xE9), Some(0xC3)] {
            let encs: &[Option<Enc>] = if r.chance(20) { &[None, Some(Enc::Utf8), Some(Enc::Windows1252), Some(Enc::MacRoman), Some(Enc::Utf16Le), Some(Enc::Utf16Be)] } else { &[None] };
            for &e in encs {
                let o = OpenOptions { enc: e, delim };
                let t = table_of(&bytes, o);
                let ctx = format!("{ctx} {o:?}");
                read_all(&t.st.base, &ctx);
                cells_agree(&t, &ctx);
                if utf16_exact(&bytes, &t.st.base) && !e.is_some_and(|e| e.is_utf16()) {
                    assert!(saved(&t) == bytes, "{ctx}: untouched save differs");
                }
            }
        }
    });
}

/// Files of several MB (parallel slices, a record across each cut), opened from disk whole and
/// head first: the same records as one sequential scan, the same bytes saved.
#[test]
fn big_files_parallel_and_head() {
    let dir = scratch("big");
    cases("big_files_parallel_and_head", 3, |r, s| {
        let mut b = Vec::new();
        let target = r.range(2 << 20, 6 << 20);
        let quoted_nl = r.chance(70);
        let mut i = 0usize;
        while b.len() < target {
            if quoted_nl && r.chance(3) {
                b.extend(format!("{i},\"multi\nline,{i}\r\n\",\"\"\"q\"\"\"\n").as_bytes());
            } else if r.chance(1) {
                b.extend(raw_field(r, b','));
                b.push(b'\n');
            } else {
                b.extend(format!("{i},caf\u{e9} {},{}\n", r.pick(WORDS), r.below(1000)).as_bytes());
            }
            i += 1;
            // A huge field now and then, and a record running over a whole slice.
            if i == 5000 && r.chance(50) { b.extend(b"big,\""); b.extend(std::iter::repeat_n(b'z', r.range(1 << 20, 3 << 20))); b.extend(b"\n\"\n"); }
        }
        if r.chance(30) { b.extend(b"\xE9\n"); }
        let ctx = format!("big file {} bytes, seed {s}", b.len());
        let p = dir.join(format!("big-{s}.csv"));
        std::fs::write(&p, &b).unwrap();
        let full = Base::open(&p, OpenOptions::default(), None).unwrap();
        read_all(&full, &ctx);
        let (seq, w) = scan::scan_range(full.bytes(), 0, full.bytes().len(), full.dialect, None, usize::MAX, None);
        assert_eq!((seq.starts.len(), w as usize, seq.qerr), (full.n, full.width, full.qerr), "{ctx}");
        for k in 0..full.n { assert_eq!(seq.starts.get(k), full.starts.get(k), "{ctx}: record {k}"); }
        let head = Base::open_head(&p, OpenOptions::default()).unwrap();
        if head.partial {
            assert!(head.n <= full.n);
            for k in 0..head.n { assert_eq!(head.starts.get(k), full.starts.get(k), "{ctx}: head record {k}"); }
            let done = head.complete(OpenOptions::default(), None);
            assert_eq!((done.n, done.width, done.enc, done.qerr), (full.n, full.width, full.enc, full.qerr), "{ctx}: completed");
            for k in 0..=done.n { assert_eq!(done.starts.get(k), full.starts.get(k)); }
        }
        let t = Table::new(Arc::new(full), None);
        assert!(saved(&t) == b, "{ctx}: untouched save differs");
        let _ = std::fs::remove_file(&p);
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// Very wide (10k+ columns), very long (a field of several MB), very many tiny rows.
#[test]
fn extreme_shapes() {
    let wide: Vec<u8> = (0..3).flat_map(|r| (0..12_000).map(move |c| format!("{}{c}", if r == 0 { "h" } else { "" })).collect::<Vec<_>>().join(",").into_bytes().into_iter().chain(*b"\n")).collect();
    let mut huge = b"a,b\n1,\"".to_vec();
    huge.extend(std::iter::repeat_n(b'x', 5 << 20));
    huge.extend(b"\"\n2,y\n");
    let mut huge_plain = b"a\n".to_vec();
    huge_plain.extend(std::iter::repeat_n(b'w', 4 << 20));
    let tiny: Vec<u8> = b"x\n".repeat(300_000);
    let crs: Vec<u8> = b"a,b\r".repeat(100_000);
    let quotes: Vec<u8> = b"\"".repeat(100_001);
    let unclosed: Vec<u8> = [b"a,b\n\"".to_vec(), b"x,\n".repeat(200_000)].concat();
    for (name, bytes) in [("wide", wide), ("huge", huge), ("huge plain", huge_plain), ("tiny", tiny), ("cr", crs), ("quotes", quotes), ("unclosed", unclosed)] {
        let t = table_of(&bytes, OpenOptions::default());
        read_all(&t.st.base, name);
        assert!(saved(&t) == bytes, "{name}: untouched save differs");
        let mut t = t;
        let id = t.st.data_id(t.st.len().saturating_sub(1).min(1));
        t.set_cells(&[(id, 0, "edited\n\"x\"".into())]);
        let back = table_of(&saved(&t), OpenOptions { enc: Some(t.st.base.enc), delim: Some(if t.st.base.dialect.lines { 0 } else { t.st.base.dialect.delim }) });
        if !t.st.base.dialect.lines {
            assert_eq!(back.st.cell(back.st.all_id(t.st.header as usize + t.st.len().saturating_sub(1).min(1)), 0), "edited\n\"x\"", "{name}");
        }
    }
    let t = table_of(&b"h,".repeat(20_000), OpenOptions::default());
    assert_eq!(t.st.ncols, 20_001);
}

/// Detection on any bytes: no panic.
#[test]
fn dialect_detection_any_bytes() {
    cases("dialect_detection_any_bytes", 2000, |r, _| {
        let b = any_file(r);
        let _ = detect_dialect(&b, None);
        let _ = enc::sniff(&b);
        let _ = enc::valid_utf8(&b);
        let mut f = Vec::new();
        scan::visit_fields(&b, DELIMS[r.below(4)], &mut f, |_, _| true);
    });
}

// ---- 2-3. edits against a model ---------------------------------------------------------

/// The table as plain strings: every row (the title row included) × every column.
#[derive(Clone, Debug, PartialEq)]
struct Model {
    ids: Vec<RowId>,
    rows: Vec<Vec<String>>,
    header: bool,
    ncols: usize,
}

impl Model {
    fn of(t: &Table) -> Model {
        let st = &t.st;
        let ids: Vec<RowId> = (0..st.all_len()).map(|p| st.all_id(p)).collect();
        let rows = ids.iter().map(|&id| (0..st.ncols).map(|c| st.cell(id, c)).collect()).collect();
        Model { ids, rows, header: st.header, ncols: st.ncols }
    }

    fn grow(&mut self, n: usize) {
        if n <= self.ncols { return; }
        self.ncols = n;
        for r in &mut self.rows { r.resize(n, String::new()); }
    }

    fn head(&self) -> usize {
        self.header as usize
    }
}

fn same_as(t: &Table, m: &Model, ctx: &str) {
    let st = &t.st;
    assert_eq!((st.all_len(), st.ncols, st.header), (m.rows.len(), m.ncols, m.header), "{ctx}: shape");
    for (p, (id, row)) in m.ids.iter().zip(&m.rows).enumerate() {
        assert_eq!(st.all_id(p), *id, "{ctx}: id at {p}");
        for (c, v) in row.iter().enumerate() {
            let got = st.cell(*id, c);
            assert!(got == *v, "{ctx}: row {p} col {c}: {got:?}, want {v:?}");
        }
    }
}

/// What a cell holds once written in `enc` and read back.
fn through(enc: Enc, v: &str) -> String {
    if !enc.limited() { return v.to_string(); }
    let mut b = Vec::new();
    enc::encode_file(enc, v, &mut b);
    enc::decode_file(enc, &b)
}

/// The saved bytes read again (the file's encoding, delimiter and title row): the model's cells,
/// as the encoding can hold them.
fn reads_as_model(bytes: &[u8], like: &Base, m: &Model, rows: Option<&[usize]>, ctx: &str) {
    let o = OpenOptions { enc: Some(like.enc), delim: Some(if like.dialect.lines { 0 } else { like.dialect.delim }) };
    let back = Table::new(Arc::new(Base::from_bytes(bytes.to_vec(), o)), Some(m.header));
    // Raw text cannot tell a CR file holding LFs from a CRLF one (`a\r\nb`): read otherwise, no
    // comparison. A CSV file has its quotes: it must read as written.
    if like.dialect.lines && back.st.base.dialect.nl != like.dialect.nl && m.rows.iter().any(|r| r.iter().any(|v| v.contains(['\r', '\n']))) { return; }
    let st = &back.st;
    let want: Vec<&Vec<String>> = match rows {
        None => m.rows.iter().collect(),
        Some(r) => m.rows.iter().take(m.head()).chain(r.iter().map(|&p| &m.rows[p])).collect(),
    };
    let ctx = format!("{ctx}\n  saved: {}", show(bytes));
    // A first cell written first in a file without BOM, starting with U+FEFF: read again, it is
    // taken for a BOM (not fixed: raw text could not escape it).
    let bom_like = !like.bom && bytes.starts_with(like.enc.bom()) && !like.enc.bom().is_empty();
    for (p, row) in want.iter().enumerate() {
        assert!(p < st.all_len(), "{ctx}: {} rows read back, {} written", st.all_len(), want.len());
        let id = st.all_id(p);
        for (c, v) in row.iter().enumerate() {
            let got = st.cell(id, c);
            let v = through(like.enc, v);
            let v = if bom_like && p == 0 && c == 0 { v.strip_prefix('\u{feff}').unwrap_or(&v).to_string() } else { v };
            if got != v {
                let k = got.bytes().zip(v.bytes()).take_while(|(a, b)| a == b).count();
                panic!("{ctx}: read back, row {p} col {c}: {} bytes, want {}; from byte {k}: {} / {}", got.len(), v.len(), show(&got.as_bytes()[k..]), show(&v.as_bytes()[k..]));
            }
        }
        for c in row.len()..st.ncols { assert_eq!(st.cell(id, c), "", "{ctx}: read back, row {p}: a cell past the last column"); }
    }
    assert_eq!(st.all_len(), want.len(), "{ctx}: rows read back");
}

#[derive(Debug)]
enum Op {
    Set(Vec<(usize, usize, String)>),
    Insert(usize, usize),
    Delete(Vec<usize>),
    Move(usize, usize, bool),
    InsCol(usize, String),
    DelCols(Vec<usize>),
    MoveCol(usize, usize),
    Header(bool),
    Eol(&'static [u8]),
    Undo,
    Redo,
    Batch(Vec<Op>),
}

/// `text`: raw text, and its line break byte.
fn gen_op(r: &mut Rng, m: &Model, text: Option<u8>, depth: usize) -> Op {
    let lines = text.is_some();
    let all = m.rows.len();
    let data = all - m.head().min(all);
    loop {
        return match r.below(if depth > 0 { 11 } else { 14 }) {
            0..=2 => Op::Set((0..r.range(1, 6)).map(|_| {
                let col = if lines { 0 } else if r.chance(10) { r.range(m.ncols, m.ncols + 2) } else { r.below(m.ncols.max(1)) };
                let mut v = gen_value(r);
                if lines { v.retain(|c| c != '\n' && c != '\r'); }
                (r.below(all.max(1)), col, v)
            }).filter(|_| all > 0).collect()),
            3 => Op::Insert(r.below(data + 1), r.range(1, 3)),
            4 if data > 0 => Op::Delete((0..r.range(1, 4)).map(|_| r.below(all)).collect()),
            5 if data > 1 => Op::Move(r.below(all), r.below(all), r.chance(50)),
            6 if !lines => Op::InsCol(r.below(m.ncols + 2), if r.chance(50) { gen_value(r) } else { String::new() }),
            7 if !lines && m.ncols > 0 => {
                let k = if r.chance(5) { m.ncols } else { 2 };
                Op::DelCols((0..r.range(1, k)).map(|_| r.below(m.ncols)).collect())
            }
            8 if !lines && m.ncols > 1 => Op::MoveCol(r.below(m.ncols + 1), r.below(m.ncols + 1)),
            9 => Op::Header(r.chance(50)),
            // Raw text has no quotes: a CR in a LF file's line would end it once CR ends lines, a
            // line's last CR would be taken for a CRLF's once LF ends them.
            10 if text == Some(b'\r') => continue,
            10 if lines => Op::Eol(b"\r\n"),
            10 => Op::Eol([&b"\n"[..], b"\r\n", b"\r"][r.below(3)]),
            11 | 12 => if r.chance(50) { Op::Undo } else { Op::Redo },
            13 => Op::Batch((0..r.range(1, 4)).map(|_| gen_op(r, m, text, depth + 1)).collect()),
            _ => continue,
        };
    }
}

/// Applies `op` to the table and to the model; the model's undo history follows the table's.
fn apply(t: &mut Table, m: &mut Model, undo: &mut Vec<Model>, redo: &mut Vec<Model>, op: &Op) {
    let before = m.clone();
    let v0 = t.version();
    match op {
        Op::Undo => {
            // Line 1's reading (titles or data) is no edit: undo leaves it as it is.
            if t.undo().is_some() { let h = m.header; redo.push(std::mem::replace(m, undo.pop().expect("model undo"))); m.header = h; }
            return;
        }
        Op::Redo => {
            if t.redo().is_some() { let h = m.header; undo.push(std::mem::replace(m, redo.pop().expect("model redo"))); m.header = h; }
            return;
        }
        Op::Batch(ops) => {
            let (mut u, mut rd) = (Vec::new(), Vec::new());
            t.batch(|t| for o in ops { if !matches!(o, Op::Undo | Op::Redo) { apply(t, m, &mut u, &mut rd, o) } });
        }
        Op::Set(cells) => {
            let ch: Vec<(RowId, usize, String)> = cells.iter().filter(|c| c.0 < m.rows.len()).map(|(p, c, v)| (m.ids[*p], *c, v.clone())).collect();
            t.set_cells(&ch);
            let all = m.rows.len();
            for (p, c, v) in cells.iter().filter(|c| c.0 < all) {
                m.grow(c + 1);
                m.rows[*p][*c] = v.clone();
            }
        }
        Op::Insert(pos, n) => {
            let ids = t.insert_rows(*pos, *n);
            let at = (pos + m.head()).min(m.rows.len());
            for (k, id) in ids.into_iter().enumerate() {
                m.ids.insert(at + k, id);
                m.rows.insert(at + k, vec![String::new(); m.ncols]);
            }
        }
        Op::Delete(ps) => {
            let gone: HashSet<RowId> = ps.iter().filter(|&&p| p < m.ids.len()).map(|&p| m.ids[p]).collect();
            t.delete_rows(&gone);
            let keep: Vec<bool> = m.ids.iter().enumerate().map(|(p, id)| (p == 0 && m.header) || !gone.contains(id)).collect();
            let mut k = keep.iter();
            m.rows.retain(|_| *k.next().unwrap());
            let mut k = keep.iter();
            m.ids.retain(|_| *k.next().unwrap());
        }
        Op::Move(a, b, after) => {
            if *a >= m.ids.len() || *b >= m.ids.len() { return; }
            let (id, target) = (m.ids[*a], m.ids[*b]);
            let got = t.move_row(id, target, *after);
            let head = m.head();
            let want = (|| {
                if id == target || *a < head || *b < head { return None; }
                let t2 = if b > a { b - 1 } else { *b };
                let at = (t2 + *after as usize).max(head);
                (at != *a).then_some((*a, at))
            })();
            assert_eq!(got, want.map(|(f, at)| (f - head, at - head)), "move_row({a}, {b}, {after})");
            if let Some((f, at)) = want {
                let (i, row) = (m.ids.remove(f), m.rows.remove(f));
                m.ids.insert(at, i);
                m.rows.insert(at, row);
            }
        }
        Op::InsCol(at, title) => {
            t.insert_column(*at, title);
            let at = (*at).min(m.ncols);
            for r in &mut m.rows { r.insert(at, String::new()); }
            m.ncols += 1;
            if m.header && !title.is_empty() && !m.rows.is_empty() { m.rows[0][at] = title.clone(); }
        }
        Op::DelCols(cs) => {
            t.delete_columns(cs);
            for r in &mut m.rows {
                let mut c = 0;
                r.retain(|_| { c += 1; !cs.contains(&(c - 1)) });
            }
            m.ncols -= (0..m.ncols).filter(|c| cs.contains(c)).count();
        }
        Op::MoveCol(from, to) => {
            t.move_column(*from, *to);
            if from != to && *from < m.ncols {
                for r in &mut m.rows {
                    let v = r.remove(*from);
                    r.insert((*to).min(m.ncols - 1), v);
                }
            }
        }
        Op::Header(h) => {
            t.set_header(*h);
            m.header = *h;
        }
        Op::Eol(e) => t.set_eol(e),
    }
    if t.version() != v0 {
        undo.push(before);
        redo.clear();
    }
}

/// Random edit sequences on random files: after each edit the table is the model; saved and
/// read again, the model; everything undone, the file as read (its very bytes when saved);
/// everything redone, the same as before the undo.
#[test]
fn edits_follow_the_model() {
    let dir = scratch("edits");
    cases("edits_follow_the_model", 300, |r, s| {
        let bytes = if r.chance(85) { gen_csv(r) } else { any_file(r) };
        let o = if r.chance(10) { OpenOptions { enc: None, delim: Some(0) } } else { OpenOptions::default() };
        let mut t = table_of(&bytes, o);
        let d = t.st.base.dialect;
        let text = d.lines.then_some(d.nl);
        let exact = utf16_exact(&bytes, &t.st.base) && saved(&t) == bytes;
        let first = Model::of(&t);
        let mut m = first.clone();
        let (mut undo, mut redo) = (Vec::new(), Vec::new());
        let small = m.rows.len() * m.ncols.max(1) < 3000;
        let mut log = Vec::new();
        for _ in 0..r.range(1, 40) {
            let op = gen_op(r, &m, text, 0);
            log.push(format!("{op:?}").chars().take(300).collect::<String>());
            apply(&mut t, &mut m, &mut undo, &mut redo, &op);
            if small { same_as(&t, &m, &format!("seed {s} file {}\n  ops: {log:#?}", show(&bytes))); }
        }
        // What "all redone" comes back to: the newest state, past any undo left to redo.
        if !redo.is_empty() && r.chance(50) {
            while !redo.is_empty() { apply(&mut t, &mut m, &mut undo, &mut redo, &Op::Redo); }
            log.push("redo all".into());
        }
        let tip = redo.first().cloned();
        let ctx = format!("seed {s} file {}\n  read as {:?} {:?} eol {:?}\n  ops: {log:#?}", show(&bytes), t.st.base.dialect, t.st.base.enc, t.st.base.eol);
        same_as(&t, &m, &ctx);
        cells_agree(&t, &ctx);
        let out = saved(&t);
        reads_as_model(&out, &t.st.base, &m, None, &ctx);
        // Saved to disk (the index handed back is the file's), then read from it.
        let p = dir.join(format!("e{s}.csv"));
        std::fs::write(&p, &bytes).unwrap();
        let disk = Arc::new(Base::open(&p, o, None).unwrap());
        let mut td = Table::new(disk, Some(first.header));
        td.st.cols = t.st.cols.clone();
        td.st.order = t.st.order.clone();
        td.st.cmap = t.st.cmap.clone();
        td.st.ncols = t.st.ncols;
        td.st.header = t.st.header;
        td.st.eol = t.st.eol;
        let (_, ix) = save::save_indexed(&td.st, &p, &SaveOptions { backup: false, ..Default::default() }, td.st.base.stamp.map(Into::into), &mut false).unwrap();
        let written = std::fs::read(&p).unwrap();
        assert!(written == out, "{ctx}: the file saved differs from the bytes written");
        if let Some(g) = ix {
            let d = t.st.base.dialect;
            let read = Base::open(&p, OpenOptions { enc: Some(t.st.base.enc), delim: Some(if d.lines { 0 } else { d.delim }) }, None).unwrap();
            assert_eq!((g.n, g.width, &g.odd, g.bom), (read.n, read.width, &read.odd, read.bom), "{ctx}: saved index");
            for i in 0..=g.n { assert_eq!(g.starts.get(i), read.starts.get(i), "{ctx}: saved index, record {i}"); }
        }
        let _ = std::fs::remove_file(&p);
        // A view: some rows, in some order, saved alone.
        let sel: Vec<usize> = (m.head()..m.rows.len()).filter(|_| r.chance(60)).collect();
        let mut sel = sel;
        if r.chance(50) { sel.reverse(); }
        let rows: Vec<RowId> = sel.iter().map(|&p| m.ids[p]).collect();
        let part = save::write_to(&t.st, &SaveOptions { rows: Some(rows), ..Default::default() }, Vec::new()).unwrap();
        reads_as_model(&part, &t.st.base, &m, Some(&sel), &format!("{ctx}\n  rows {sel:?}"));
        // All undone: the file as read; all redone: as before.
        let (last, last_out) = (m.clone(), out);
        while t.undo().is_some() {}
        assert!(!t.dirty(), "{ctx}: dirty once all undone");
        // Line 1's reading is no edit: it stays as last set.
        let first_now = Model { header: t.st.header, ..first.clone() };
        same_as(&t, &first_now, &format!("{ctx}: all undone"));
        if exact { assert!(saved(&t) == bytes, "{ctx}: all undone, the save differs from the file"); }
        while t.redo().is_some() {}
        let redone = Model { header: t.st.header, ..tip.clone().unwrap_or(last) };
        same_as(&t, &redone, &format!("{ctx}: all redone"));
        if tip.is_none() { assert!(saved(&t) == last_out, "{ctx}: all redone, the save differs"); }
    });
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 4. views: filters and sorts against a plain reading -------------------------------

use csvfab_engine::text::{fold, parse_date, parse_number};

/// A table from a hostile file with some edits on it (rows made, cells changed, columns moved).
fn messy_table(r: &mut Rng) -> Table {
    let bytes = if r.chance(85) { gen_csv(r) } else { any_file(r) };
    let o = match r.below(12) { 0 => OpenOptions { enc: None, delim: Some(0) }, 1 => OpenOptions { enc: None, delim: Some(b';') }, _ => OpenOptions::default() };
    let mut t = table_of(&bytes, o);
    let lines = t.st.base.dialect.lines;
    let text = lines.then_some(t.st.base.dialect.nl);
    let (mut m, mut u, mut rd) = (Model::of(&t), Vec::new(), Vec::new());
    for _ in 0..r.below(8) {
        let op = gen_op(r, &m, text, 0);
        apply(&mut t, &mut m, &mut u, &mut rd, &op);
    }
    t
}

/// A cell of the table, or a piece of one, to search for.
fn needle(r: &mut Rng, t: &Table) -> String {
    let st = &t.st;
    if st.all_len() == 0 || st.ncols == 0 || r.chance(15) { return gen_value(r).chars().take(6).collect(); }
    let v = st.cell(st.all_id(r.below(st.all_len())), r.below(st.ncols));
    let ch: Vec<char> = v.chars().collect();
    let (a, b) = (r.below(ch.len() + 1), r.below(ch.len() + 1));
    let s: String = ch[a.min(b)..a.max(b)].iter().collect();
    match r.below(4) { 0 => s.to_uppercase(), 1 => v, _ => s }
}

fn gen_spec(r: &mut Rng, t: &Table) -> FilterSpec {
    let mut sp = FilterSpec { regex: r.chance(15), match_case: r.chance(30), invert: r.chance(25), ..Default::default() };
    if r.chance(70) {
        sp.global = needle(r, t);
        if sp.regex && r.chance(50) { sp.global = regex::escape(&sp.global); }
        if sp.regex && r.chance(20) { sp.global = r.pick(&["^a", "\\d+", "(?i)é", "[", "a|b", "\\bx", ".*", "^$", "(", "\\p{L}{3}"]).to_string(); }
    }
    for _ in 0..r.below(3) {
        let c = r.below(t.st.ncols + 1);
        let n = needle(r, t);
        let text = match r.below(9) { 0 => format!("={n}"), 1 => format!("!{n}"), 2 => "!".into(), 3 => "=".into(), 4 => format!("!={n}"), 5 => format!(">{}", r.below(100)), 6 => format!("<={n}"), 7 => format!(">=2020-01-0{}", r.range(1, 9)), _ => n };
        sp.cols.push((c, text));
    }
    if r.chance(30) && t.st.ncols > 0 {
        let c = r.below(t.st.ncols);
        let h: HashSet<String> = (0..r.range(1, 4)).map(|_| needle(r, t).trim().to_string()).collect();
        sp.hidden.push((c, Arc::new(h)));
    }
    sp
}

/// The filter as written, read cell by cell. None: it does not compile.
fn plain_filter(t: &Table, sp: &FilterSpec, ids: &[RowId]) -> Option<Vec<RowId>> {
    let case = sp.match_case;
    let pat = |text: &str| -> Option<Box<dyn Fn(&str) -> bool>> {
        if sp.regex {
            let re = regex::RegexBuilder::new(text).case_insensitive(!case).size_limit(1 << 22).build().ok()?;
            return Some(Box::new(move |s: &str| re.is_match(s)));
        }
        let needle = if case { text.to_string() } else { fold(text) };
        Some(Box::new(move |s: &str| needle.is_empty() || if case { s.contains(&needle) } else { fold(s).contains(&needle) }))
    };
    let norm = |s: &str| if case { s.to_string() } else { fold(s) };
    let num = |s: &str| parse_number(s).or_else(|| parse_date(s));
    let mut tests: Vec<(usize, Box<dyn Fn(&str) -> bool>)> = Vec::new();
    for (c, text) in &sp.cols {
        if text.is_empty() { continue; }
        let eq = move |want: String| move |s: &str| if case { s.trim() == want } else { fold(s.trim()) == want };
        let f: Box<dyn Fn(&str) -> bool> = if sp.regex {
            pat(text)?
        } else if let Some(x) = text.strip_prefix(">=") { let v = num(x)?; Box::new(move |s| num(s).is_some_and(|x| x >= v)) }
        else if let Some(x) = text.strip_prefix("<=") { let v = num(x)?; Box::new(move |s| num(s).is_some_and(|x| x <= v)) }
        else if let Some(x) = text.strip_prefix('>') { let v = num(x)?; Box::new(move |s| num(s).is_some_and(|x| x > v)) }
        else if let Some(x) = text.strip_prefix('<') { let v = num(x)?; Box::new(move |s| num(s).is_some_and(|x| x < v)) }
        else if let Some(x) = text.strip_prefix("!=") { let e = eq(norm(x)); Box::new(move |s| !e(s)) }
        else if let Some(x) = text.strip_prefix('!') { if x.is_empty() { let e = eq(String::new()); Box::new(move |s| !e(s)) } else { let p = pat(x)?; Box::new(move |s| !p(s)) } }
        else if let Some(x) = text.strip_prefix('=') { Box::new(eq(norm(x))) }
        else {
            // Plain text: every word (or "phrase") in the cell.
            let ps = view::terms(text).iter().map(|t| pat(t)).collect::<Option<Vec<_>>>()?;
            Box::new(move |s: &str| ps.iter().all(|p| p(s)))
        };
        tests.push((*c, f));
    }
    // The search: every term (words, "phrases") in some cell of the row; a regex is one term.
    let global: Vec<Box<dyn Fn(&str) -> bool>> = if sp.global.is_empty() {
        Vec::new()
    } else if sp.regex {
        vec![pat(&sp.global)?]
    } else {
        view::terms(&sp.global).iter().map(|t| pat(t)).collect::<Option<_>>()?
    };
    let st = &t.st;
    let mut row = Vec::new();
    Some(ids.iter().copied().filter(|&id| {
        sp.hidden.iter().all(|(c, h)| h.is_empty() || !h.contains(&st.cell(id, *c)))
            && tests.iter().all(|(c, f)| f(&st.cell(id, *c)))
            && (global.is_empty() || { st.row(id, &mut row); global.iter().all(|g| row.iter().any(|v| g(v))) != sp.invert })
    }).collect())
}

/// The sort as written: a stable sort on each key (empty or unreadable values last).
fn plain_sort(t: &Table, ids: &[RowId], keys: &[SortKey], kinds: &[Kind]) -> Vec<RowId> {
    #[derive(PartialEq, PartialOrd)]
    enum K { Num(f64), Text(Vec<u8>), None }
    let st = &t.st;
    let key = |id: RowId, k: &SortKey| -> K {
        let v = st.cell(id, k.col);
        let v = v.trim();
        match kinds.get(k.col).copied().unwrap_or(Kind::Text) {
            Kind::Text => if v.is_empty() { K::None } else { K::Text(csvfab_engine::text::sort_key(v)) },
            Kind::Number => parse_number(v).map_or(K::None, |x| K::Num(if x == 0.0 { 0.0 } else { x })),
            Kind::Date => parse_date(v).map_or(K::None, |x| K::Num(if x == 0.0 { 0.0 } else { x })),
        }
    };
    let mut v: Vec<(Vec<K>, RowId)> = ids.iter().map(|&id| (keys.iter().map(|k| key(id, k)).collect(), id)).collect();
    v.sort_by(|a, b| {
        for (k, (x, y)) in keys.iter().zip(a.0.iter().zip(&b.0)) {
            let o = match (x, y) {
                (K::None, K::None) => std::cmp::Ordering::Equal,
                (K::None, _) => std::cmp::Ordering::Greater,
                (_, K::None) => std::cmp::Ordering::Less,
                _ => { let o = x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal); if k.desc { o.reverse() } else { o } }
            };
            if o != std::cmp::Ordering::Equal { return o; }
        }
        std::cmp::Ordering::Equal
    });
    v.into_iter().map(|x| x.1).collect()
}

/// Filters and sorts on hostile tables give what a plain reading of the cells gives, with the
/// column caches cold and warm.
#[test]
fn views_match_a_plain_reading() {
    cases("views_match_a_plain_reading", 200, |r, s| {
        let t = messy_table(r);
        let st = &t.st;
        let no = AtomicBool::new(false);
        let all: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
        let mut subset: Vec<RowId> = all.iter().copied().filter(|_| r.chance(60)).collect();
        for k in (1..subset.len()).rev() { if r.chance(30) { subset.swap(k, r.below(k + 1)); } }
        let ctx = format!("seed {s} {:?}", st.base.dialect);
        let kinds = view::detect_kinds(st);
        for round in 0..6 {
            if round == 3 {
                // Every cache built: the fast paths read them now.
                for c in 0..st.ncols { for need in [Need::Dict, Need::Text, Need::Rank, Need::Num, Need::Date] { let _ = colcache::ensure(st, c, need, &no); } }
            }
            let sp = gen_spec(r, &t);
            let want = plain_filter(&t, &sp, &all);
            match view::Filter::compile(&sp) {
                Err(_) => assert!(want.is_none() || sp.regex, "{ctx}: {sp:?} does not compile"),
                Ok(f) => {
                    let Some(want) = want else { panic!("{ctx}: {sp:?} compiles, but not as written") };
                    let got = view::filter(st, &f, &no).unwrap();
                    if got != want {
                        let diff: Vec<String> = got.iter().filter(|x| !want.contains(x)).map(|&id| format!("+{id}")).chain(want.iter().filter(|x| !got.contains(x)).map(|&id| format!("-{id}")))
                            .map(|d| { let id: RowId = d[1..].parse().unwrap(); format!("{d}: {:?} raw {}", (0..st.ncols.min(4)).map(|c| st.cell(id, c)).collect::<Vec<_>>(), if (id as usize) < st.base.n { show(st.base.record(id as usize)) } else { "new".into() }) }).collect();
                        panic!("{ctx}: {sp:?}\n  diff (+ kept, - dropped): {diff:#?}");
                    }
                    assert_eq!(view::filter_ids(st, &f, &subset, &no).unwrap(), plain_filter(&t, &sp, &subset).unwrap(), "{ctx}: {sp:?} (subset)");
                }
            }
            let kinds: Vec<Kind> = if r.chance(50) { kinds.clone() } else { (0..st.ncols).map(|_| [Kind::Text, Kind::Number, Kind::Date][r.below(3)]).collect() };
            let keys: Vec<SortKey> = (0..r.range(1, 3)).map(|_| SortKey { col: r.below(st.ncols + 1), desc: r.chance(50) }).collect();
            for input in [&all, &subset] {
                let mut got = input.clone();
                assert!(view::sort(st, &mut got, &keys, &kinds, &no));
                assert_eq!(got, plain_sort(&t, input, &keys, &kinds), "{ctx}: sort {keys:?} {kinds:?}");
            }
        }
    });
}

// ---- 4b. every analysis on hostile tables: no panic, sane answers -----------------------

use csvfab_engine::{dupes, group, insights, pivot, schema, sparkline, stats};

/// Runs `f`, telling which analysis panicked.
fn step<R>(what: &str, ctx: &str, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(e) => {
            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            panic!("{what} panicked: {msg}\n  {ctx}")
        }
    }
}

#[test]
fn analyses_never_panic() {
    cases("analyses_never_panic", 60, |r, s| {
        let t = messy_table(r);
        let st = &t.st;
        let ctx = format!("seed {s} {:?} {:?} rows {} cols {}", st.base.dialect, st.base.enc, st.len(), st.ncols);
        let no = AtomicBool::new(false);
        let all: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
        let some: Vec<RowId> = all.iter().copied().filter(|_| r.chance(50)).collect();
        let ids = if r.chance(50) { &all } else { &some };
        let kinds = step("detect_kinds", &ctx, || view::detect_kinds(st));
        let col = r.below(st.ncols.max(1));
        step("profile", &ctx, || {
            let p = stats::profile(st, ids, &no).unwrap();
            assert_eq!(p.len(), st.ncols);
            let _ = stats::profile_col(st, ids, col, &no);
            let _ = stats::profile_cols(st, ids, &(0..st.ncols).collect::<Vec<_>>(), &no);
        });
        step("value_counts", &ctx, || {
            if col < st.ncols {
                let vc = stats::value_counts(st, ids, col, r.chance(50), &no).unwrap();
                let total: u64 = vc.iter().map(|(_, n)| n as u64).sum();
                assert_eq!(total as usize, ids.len(), "{ctx}: value counts");
                let _ = (vc.by_count(), vc.by_value(), stats::panel_profile(&vc));
                let ix = vc.by_value();
                let _ = vc.search(&ix, &needle(&mut Rng::new(s), &t));
            }
        });
        step("totals", &ctx, || {
            let reqs: Vec<stats::TotalReq> = (0..st.ncols).map(|c| stats::TotalReq { col: c, kind: kinds[c], median: true, distinct: true }).collect();
            for tot in stats::totals(st, ids, &reqs, &no).unwrap() {
                for f in stats::TotalFn::for_kind(tot.kind) { let _ = tot.value(*f); }
                assert!(tot.filled + tot.empty <= ids.len() as u64 + 1, "{ctx}: totals");
            }
        });
        step("dupes", &ctx, || {
            use dupes::{Clean, DupSpec, Field};
            let cleans = [Clean::Loose, Clean::Exact, Clean::Slug, Clean::Digits, Clean::Phone, Clean::Email, Clean::Sound { tol: 0.8 }, Clean::E164 { cc: 33 }, Clean::Mail { canon: true }];
            for _ in 0..3 {
                let keys: Vec<Vec<Field>> = (0..r.below(3)).map(|_| (0..r.range(1, 2)).map(|_| Field { col: r.below(st.ncols + 1), clean: cleans[r.below(cleans.len())].clone() }).collect()).collect();
                let spec = DupSpec { keys };
                let d = dupes::find(st, ids, &spec, &no, None).unwrap();
                let del = d.to_delete(r.chance(50));
                assert!(del.len() <= d.rows(), "{ctx}: dupes");
                for g in d.order([dupes::Order::File, dupes::Order::LeastSure, dupes::Order::SurestFirst][r.below(3)]).into_iter().take(5) {
                    let _ = (d.reason(g), d.confidence(g), d.identical(g), d.diff_cols(st, &spec, g));
                    let m = d.members(g);
                    if m.len() > 1 { let _ = dupes::merge(st, m[0], &m[1..]); }
                }
                let _ = d.only_dups(ids);
            }
            let roles = dupes::roles(st);
            let headers = st.headers();
            for sug in dupes::suggest(&roles, &headers) { let _ = dupes::find_rules(st, ids, &sug.rules, &roles.names(), &no, None); }
        });
        step("group_by", &ctx, || {
            let all_aggs = group::Aggs { sum: true, avg: true, min: true, max: true, distinct: true };
            let spec = group::GroupSpec { by: (0..r.range(1, 2)).map(|_| r.below(st.ncols + 1)).collect(), aggs: (0..st.ncols).filter(|_| r.chance(40)).map(|c| (c, all_aggs)).collect(), how: [group::Match::Exact, group::Match::Loose, group::Match::Slug][r.below(3)] };
            if let Some(g) = group::group_by(st, ids, &spec, &kinds, &no, None) {
                let mut row = Vec::new();
                for i in 0..g.len() { g.row(i, b',', &mut row); }
                let n: usize = (0..g.len()).count();
                assert!(n <= ids.len().max(1), "{ctx}: more groups than rows");
                let csv = g.to_csv(b';');
                let back = table_of(&csv, OpenOptions { enc: Some(Enc::Utf8), delim: Some(b';') });
                assert_eq!(back.st.all_len(), g.len() + 1, "{ctx}: group-by CSV");
            }
        });
        step("pivot", &ctx, || {
            let mut spec = pivot::PivotSpec::guess(st, ids, &kinds, None);
            let _ = pivot::pivot(st, ids, &spec, &kinds, &no, None);
            for fun in pivot::Fun::ALL {
                spec.fun = fun;
                spec.value = Some(r.below(st.ncols.max(1)));
                spec.rows = r.below(st.ncols.max(1));
                spec.cols = if r.chance(50) { Some(r.below(st.ncols.max(1))) } else { None };
                spec.rows_by = [pivot::Dates::Month, pivot::Dates::Quarter, pivot::Dates::Year, pivot::Dates::Day, pivot::Dates::AsWritten][r.below(5)];
                let _ = pivot::pivot(st, ids, &spec, &kinds, &no, None);
                let _ = pivot::drill(st, ids, &spec, &kinds, None, None, &no);
            }
        });
        step("insights", &ctx, || {
            if let Some(rep) = insights::insights(st, &no, None) {
                let _ = rep.summary();
                let headers = st.headers();
                for it in &rep.items {
                    let _ = (it.title.plain(&headers), it.sub.plain(&headers));
                    if let Some(show) = &it.show { let _ = insights::marks(st, show, &no); }
                }
            }
        });
        step("sparkline", &ctx, || {
            for sample in [sparkline::Sample::Js, sparkline::Sample::All, sparkline::Sample::Rows(r.range(1, 50))] {
                if let Some(base) = sparkline::spark_base(st, &kinds, sample, &no) { let _ = sparkline::spark_filtered(st, &base, ids, sample, &no); }
            }
        });
        step("schema", &ctx, || {
            if let Some(inf) = schema::infer(st, &no, None) {
                let json = schema::to_json(&inf.schema);
                let back = schema::parse(&json).unwrap_or_else(|e| panic!("{ctx}: the schema written does not read back: {e}\n{json}"));
                let _ = schema::validate(st, &back, &no, None);
                let draft = schema::Draft::new(inf);
                let _ = schema::validate(st, &draft.to_schema(), &no, None);
            }
            let _ = schema::parse(&String::from_utf8_lossy(&gen_bytes(r)));
        });
        step("diagnose", &ctx, || { for &id in ids.iter().take(50) { let _ = csvfab_engine::diagnose::diagnose(st, id); } let _ = csvfab_engine::diagnose::dominant_width(st); });
        step("review", &ctx, || { let _ = csvfab_engine::review::changes(st, &no); });
        step("clean", &ctx, || { let _ = csvfab_engine::edit::clean::scan(st, &no); });
        step("anonymize", &ctx, || {
            let modes = csvfab_engine::anonymize::guess_all(st);
            for c in 0..st.ncols.min(20) { let _ = csvfab_engine::anonymize::example(st, c, modes[c]); }
            let _ = csvfab_engine::anonymize::anonymize(st, ids, &modes, &no, None);
        });
        step("split", &ctx, || { if st.ncols > 0 { let c = csvfab_engine::split::guess_column(st, ids); let _ = csvfab_engine::split::groups(st, ids, c, csvfab_engine::split::Match::Loose, "t.csv", &no, None); } });
        step("look", &ctx, || {
            let rich = csvfab_engine::look::rich_cols(st, &kinds);
            let _ = csvfab_engine::look::pills_from_rows(st, &kinds, &rich);
            if col < st.ncols { let _ = (csvfab_engine::look::heat(st, ids, col, &no), csvfab_engine::look::data_bars(st, ids, col, &no), csvfab_engine::look::has_groups(st, ids, col), csvfab_engine::look::sort_kind(st, col)); }
        });
        step("kinds", &ctx, || { let _ = (csvfab_engine::kinds::column_kinds(st), csvfab_engine::kinds::file_mojibake(st)); });
        // Two tables together: compared, combined, one looked up in the other.
        let other = messy_table(r);
        let os = &other.st;
        let oids: Vec<RowId> = (0..os.len()).map(|p| os.data_id(p)).collect();
        step("compare", &ctx, || {
            let (k1, k2) = csvfab_engine::compare::guess_keys(&st.headers(), &os.headers(), r.below(st.ncols.max(1)));
            let _ = csvfab_engine::compare::compare(st, os, k1, k2, [group::Match::Exact, group::Match::Loose, group::Match::Slug][r.below(3)], &no, None);
        });
        step("combine", &ctx, || {
            use csvfab_engine::combine;
            let srcs = vec![combine::Source { name: "a.csv".into(), st: st.clone(), rows: None }, combine::Source { name: "b\"x.csv".into(), st: os.clone(), rows: Some(Arc::new(oids.clone())) }];
            let cols = combine::columns(&srcs, r.chance(50), r.chance(30));
            let f = combine::format(&srcs);
            let out = combine::write_to(&srcs, &cols, r.chance(50), &f, Vec::new(), &no, None).unwrap();
            let back = table_of(&out, OpenOptions { enc: Some(f.enc), delim: Some(f.delim) });
            assert!(back.st.all_len() >= st.len().min(1), "{ctx}: combined");
        });
        step("lookup", &ctx, || {
            use csvfab_engine::edit::lookup;
            if st.ncols == 0 || os.ncols == 0 { return; }
            let (key, skey) = lookup::pick_keys(&st.headers(), &os.headers(), r.below(st.ncols), r.chance(50));
            if key >= st.ncols || skey >= os.ncols { return; }
            let idx = lookup::Index::build(os, &oids, skey, [lookup::Match::Exact, lookup::Match::Loose, lookup::Match::Slug][r.below(3)], &no).unwrap();
            let _ = lookup::stats(st, ids, key, &idx, &no);
            let cols: Vec<usize> = (0..os.ncols).filter(|&c| c != skey && r.chance(40)).collect();
            let _ = lookup::preview(st, ids, key, os, &idx, &cols, 5);
            let names = lookup::new_names(&st.headers(), &os.headers(), &cols, "b");
            if let Some(p) = lookup::plan(st, ids, key, os, &idx, &cols, names, &no) {
                let mut t2 = Table::new(st.base.clone(), Some(st.header));
                t2.st = st.clone();
                let _ = lookup::apply(&mut t2, p, "b");
                cells_agree(&t2, &ctx);
            }
        });
        step("formula", &ctx, || {
            let headers = st.headers();
            let srcs = ["1+1", "[a]&\"x\"", "upper(trim(left(\"é\",3)))", "if(len(\"x\")>0,1/0,2)", "(((", "\"unclosed", "sum(1,2", "date(2024,2,30)", "mid(\"abc\",-5,99999999999)", "repeat(\"x\",100000000000)", "regexmatch(\"a\",\"[\")", "0/0", "-(-(-1))^999999"];
            let mut exprs: Vec<String> = srcs.iter().map(|x| x.to_string()).collect();
            exprs.extend(headers.iter().take(3).map(|h| format!("[{h}]&[{h}]")));
            exprs.push(String::from_utf8_lossy(&gen_bytes(r)[..]).chars().take(40).collect());
            for e in exprs {
                if let Ok(x) = csvfab_engine::formula::compile(&e, &headers) {
                    let _ = csvfab_engine::formula::column(st, &x, Some(ids), &no);
                    let _ = csvfab_engine::formula::filter(st, &x, Some(ids), &no);
                }
            }
        });
    });
}

// ---- 3b. the editing tools on hostile tables -------------------------------------------------

use csvfab_engine::edit::{self, clipboard, series, columns as split_cols, replace, clean};

/// Each tool run on a hostile table and applied: the table stays readable, its save reads back
/// as it is, and one undo puts it back as it was.
#[test]
fn edit_tools_apply_and_undo() {
    cases("edit_tools_apply_and_undo", 100, |r, s| {
        let mut t = messy_table(r);
        let lines = t.st.base.dialect.lines;
        let no = AtomicBool::new(false);
        for round in 0..6 {
            let st = t.st.clone();
            let rows: Vec<RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
            let vis: Vec<usize> = (0..st.ncols).filter(|_| r.chance(85)).collect();
            let ctx = format!("seed {s} round {round} {:?} rows {} cols {}", st.base.dialect, rows.len(), st.ncols);
            let before = Model::of(&t);
            let v0 = t.version();
            let rect = |r: &mut Rng| {
                let (a, b) = (r.below(rows.len().max(1)), r.below(rows.len().max(1)));
                let (c, d) = (r.below(st.ncols.max(1)), r.below(st.ncols.max(1)));
                series::Rect { r0: a.min(b), r1: a.max(b), c0: c.min(d), c1: c.max(d) }
            };
            let tool = r.below(9);
            let name = ["copy+paste", "fill", "bulk", "replace", "clean", "split", "merge", "convert", "tsv"][tool];
            let changes: Option<Vec<edit::Change>> = step(name, &ctx, || {
                if rows.is_empty() || st.ncols == 0 { return None; }
                match tool {
                    0 => {
                        let sel = rect(r);
                        let c = clipboard::copy(&st, &rows, &vis, sel, &no).unwrap();
                        let grid = clipboard::parse_tsv(&c.text);
                        // What is copied is what the cells hold.
                        let cols: Vec<usize> = vis.iter().copied().filter(|c| (sel.c0..=sel.c1).contains(c)).collect();
                        if !cols.is_empty() && !(sel.r0 == sel.r1 && cols.len() == 1) {
                            let want: Vec<Vec<String>> = (sel.r0..=sel.r1).map(|p| cols.iter().map(|&c| st.cell(rows[p], c)).collect()).collect();
                            assert_eq!(grid, want, "{ctx}: copied {:?}", c.text);
                        }
                        let to = rect(r);
                        let grid = if r.chance(30) { vec![vec![gen_value(r)]] } else { grid };
                        let p = clipboard::paste(&st, &rows, &vis, to, &grid, r.chance(20), !lines && r.chance(50), &no).unwrap();
                        let _ = clipboard::apply_paste(&mut t, p, "pasted");
                        None
                    }
                    1 => {
                        let sel = rect(r);
                        Some(match r.below(4) {
                            0 => series::fill_down(&st, &rows, &vis, sel, &no).unwrap().unwrap_or_default(),
                            1 => series::fill_blanks(&st, &rows, &vis, sel, &no).unwrap().unwrap_or_default(),
                            2 => series::fill_value(&st, &rows, &vis, sel, &gen_value(r), r.chance(50), &no).unwrap(),
                            _ => {
                                let dir = [series::Dir::Down, series::Dir::Up, series::Dir::Right, series::Dir::Left][r.below(4)];
                                let target = rect(r);
                                series::fill_handle(&st, &rows, &vis, sel, dir, target, r.chance(50), &no).unwrap_or_default()
                            }
                        })
                    }
                    2 => {
                        let ops = [replace::Bulk::Set(gen_value(r)), replace::Bulk::Clear, replace::Bulk::Trim, replace::Bulk::Upper, replace::Bulk::Lower, replace::Bulk::Title];
                        edit::changes(&st, &rows, &vis, &ops[r.below(ops.len())], &no)
                    }
                    3 => {
                        let spec = replace::ReplaceSpec { find: needle(r, &t), replace: if r.chance(30) { "$1$&$$${x}".into() } else { gen_value(r) }, regex: r.chance(30), match_case: r.chance(50) };
                        match replace::Replace::new(&spec, &st) {
                            Ok(Some(rep)) => Some(rows.iter().flat_map(|&id| vis.iter().map(move |&c| (id, c))).filter_map(|(id, c)| {
                                let v = st.cell(id, c);
                                let w = rep.apply(&v).into_owned();
                                (w != v).then_some((id, c, w))
                            }).collect()),
                            _ => None,
                        }
                    }
                    4 => {
                        let sc = clean::scan(&st, &no).unwrap();
                        let on: Vec<clean::CleanOp> = clean::CLEAN_OPS.iter().filter(|_| r.chance(60)).map(|o| o.0).collect();
                        if let Some(p) = clean::plan(&st, &on, &sc, &no) { let _ = clean::apply(&mut t, p); }
                        None
                    }
                    5 if !lines => {
                        let sep = [",", " ", ";", "-", "\\s+", "(", "", "é"][r.below(8)];
                        if let Ok(Some(sp)) = split_cols::Splitter::new(sep, r.chance(30), r.chance(50), r.chance(50)) {
                            let col = r.below(st.ncols);
                            let stats = split_cols::analyse(&st, &rows, col, &sp, &no).unwrap();
                            let n = stats.proposed().clamp(1, 6);
                            if let Some(p) = split_cols::split_plan(&st, &rows, col, &sp, n, r.chance(50), (0..n).map(|k| format!("p{k}")).collect(), &no) { let _ = split_cols::apply_split(&mut t, p); }
                        }
                        None
                    }
                    6 if !lines && st.ncols > 1 => {
                        let order: Vec<usize> = (0..st.ncols).filter(|_| r.chance(40)).collect();
                        if let Some(Ok(p)) = split_cols::merge_plan(&st, &rows, &order, &gen_value(r).chars().take(3).collect::<String>(), r.chance(50), r.chance(50), "merged", &no) { let _ = split_cols::apply_merge(&mut t, p); }
                        None
                    }
                    7 => {
                        use edit::convert_values as cv;
                        let col = r.below(st.ncols);
                        let g = cv::guess(&st, &rows, col, "t", "33");
                        let _ = g;
                        let kinds = [cv::Convert::Number { dec: [',', '.'][r.below(2)], out: [cv::NumOut::Point, cv::NumOut::Comma, cv::NumOut::Fr, cv::NumOut::En][r.below(4)], decimals: if r.chance(50) { Some(r.below(4)) } else { None } }, cv::Convert::Date { month_first: r.chance(50), out: [cv::DateOut::Iso, cv::DateOut::Dmy, cv::DateOut::Mdy, cv::DateOut::DmyDot, cv::DateOut::DmyDash][r.below(5)] }, cv::Convert::Phone { cc: "33".into(), out: [cv::PhoneOut::E164, cv::PhoneOut::Intl, cv::PhoneOut::Nat][r.below(3)] }];
                        let conv = cv::Converter::new(kinds[r.below(kinds.len())].clone());
                        let mut out = String::new();
                        Some(rows.iter().filter_map(|&id| { out.clear(); let v = st.cell(id, col); (conv.convert(&v, &mut out) && out != v).then(|| (id, col, out.clone())) }).collect())
                    }
                    _ => {
                        // Clipboard text of any cells parses back to them.
                        let grid: Vec<Vec<String>> = (0..r.range(1, 5)).map(|_| (0..r.range(2, 4)).map(|_| gen_value(r)).collect()).collect();
                        let text = grid.iter().map(|row| { let mut l = String::new(); for (k, v) in row.iter().enumerate() { if k > 0 { l.push('\t'); } clipboard::tsv_quote(v, &mut l); } l }).collect::<Vec<_>>().join("\r\n");
                        assert_eq!(clipboard::parse_tsv(&text), grid, "{ctx}: TSV {text:?}");
                        None
                    }
                }
            });
            if let Some(ch) = changes {
                let ch: Vec<edit::Change> = if lines { ch.into_iter().filter(|c| c.1 == 0).map(|(id, c, v)| (id, c, v.replace(['\r', '\n'], " "))).collect() } else { ch };
                t.set_cells(&ch);
            }
            cells_agree(&t, &ctx);
            let m = Model::of(&t);
            let out = saved(&t);
            // Raw text has no quotes: a line break pasted into a line makes two (not compared).
            if !(lines && m.rows.iter().any(|row| row.iter().any(|v| v.contains(['\r', '\n'])))) {
                reads_as_model(&out, &t.st.base, &m, None, &format!("{ctx} after {name}"));
            }
            if t.version() != v0 && r.chance(50) {
                t.undo();
                same_as(&t, &before, &format!("{ctx}: {name} undone"));
            }
        }
    });
}

// ---- 4c. conversions: JSON, Excel, SQLite, both ways, and broken inputs -----------------

use csvfab_engine::convert::{self, json, sqlite, xlsx_read, xlsx_write};

fn gen_json(r: &mut Rng, depth: usize) -> String {
    let leaf = |r: &mut Rng| -> String {
        match r.below(8) {
            0 => "null".into(), 1 => "true".into(), 2 => format!("{}", r.next() as i64), 3 => format!("{:e}", r.next() as f64 / 7.0),
            4 => "1e400".into(), 5 => "-0".into(), 6 => serde_json::to_string(&gen_value(r)).unwrap(),
            _ => serde_json::to_string(r.pick(WORDS)).unwrap(),
        }
    };
    if depth == 0 || r.chance(40) { return leaf(r); }
    if r.chance(50) {
        let n = r.below(6);
        format!("[{}]", (0..n).map(|_| gen_json(r, depth - 1)).collect::<Vec<_>>().join(","))
    } else {
        let n = r.below(6);
        let keys = ["a", "b", "0", "10", "2", "a.b", "", "é", "__proto__", "a"];
        format!("{{{}}}", (0..n).map(|_| format!("{}:{}", serde_json::to_string(keys[r.below(keys.len())]).unwrap(), gen_json(r, depth - 1))).collect::<Vec<_>>().join(","))
    }
}

/// JSON of any shape (and broken JSON, and JSON Lines): converted or refused, never a panic;
/// what is converted reads back as a table of the rows it says.
#[test]
fn json_import_any_input() {
    cases("json_import_any_input", 400, |r, s| {
        let text = match r.below(8) {
            0 => String::from_utf8_lossy(&gen_bytes(r)).into_owned(),
            1 => (0..r.range(1, 20)).map(|_| gen_json(r, 3)).collect::<Vec<_>>().join("\n"),
            2 => { let d = r.range(100, 3000); format!("{}{}", "[".repeat(d), "]".repeat(d)) }
            3 => { let d = r.range(100, 3000); format!("{}1{}", "{\"a\":".repeat(d), "}".repeat(d)) }
            4 => { let j = gen_json(r, 4); let cut = r.below(j.len() + 1); j.chars().take(cut).collect() }
            _ => format!("[{}]", (0..r.range(0, 40)).map(|_| gen_json(r, 3)).collect::<Vec<_>>().join(",")),
        };
        let mut bytes = text.into_bytes();
        if r.chance(10) { bytes.splice(0..0, *b"\xEF\xBB\xBF"); }
        if r.chance(5) { bytes.push(0xFF); }
        let no = AtomicBool::new(false);
        let delim = DELIMS[r.below(4)];
        let ctx = format!("seed {s} {}", show(&bytes));
        let res = step("json::convert", &ctx, || json::convert(&bytes, delim, Vec::new(), None, &no).map(|i| i));
        if let Ok(info) = res {
            let mut out = Vec::new();
            json::convert(&bytes, delim, &mut out, None, &no).unwrap();
            let t = table_of(&out, OpenOptions { enc: Some(Enc::Utf8), delim: Some(delim) });
            // (A row of no column, `{}`, is an empty line: no record.)
            assert!(t.st.base.n as u64 >= info.rows.min(1) || info.cols == 0, "{ctx}: {info:?} but {} records", t.st.base.n);
        }
    });
}

/// Hostile tables exported to Excel and SQLite, and read back: the same shape; text cells the same.
#[test]
fn xlsx_and_sqlite_round_trip() {
    let dir = scratch("convert");
    cases("xlsx_and_sqlite_round_trip", 20, |r, s| {
        let t = messy_table(r);
        let st = &t.st;
        let no = AtomicBool::new(false);
        let ctx = format!("seed {s} {:?} rows {} cols {}", st.base.dialect, st.len(), st.ncols);
        let rows: Option<Vec<RowId>> = r.chance(30).then(|| (0..st.len()).filter(|_| r.chance(50)).map(|p| st.data_id(p)).collect());
        let n = rows.as_ref().map_or(st.len(), |v| v.len());
        let x = dir.join(format!("t{s}.xlsx"));
        let ex = step("xlsx export", &ctx, || xlsx_write::write(st, rows.as_deref(), &x, &needle(r, &t), None, &no));
        match ex {
            Err(e) => assert!(st.ncols == 0, "{ctx}: xlsx export refused: {e}"),
            Ok(e) => {
                assert_eq!(e.rows as usize, n + st.header as usize, "{ctx}: xlsx rows");
                let sheets = step("xlsx sheets", &ctx, || xlsx_read::sheets(&x)).unwrap();
                assert_eq!(sheets.len(), 1, "{ctx}");
                let mut out = Vec::new();
                let imp = step("xlsx import", &ctx, || xlsx_read::to_csv(&x, None, b',', &mut out, None, &no));
                let imp = imp.unwrap_or_else(|e| panic!("{ctx}: xlsx read back: {e}"));
                assert!(imp.rows as usize <= n + 1, "{ctx}: {imp:?}");
                let _ = table_of(&out, OpenOptions { enc: Some(Enc::Utf8), delim: Some(b',') });
            }
        }
        let q = dir.join(format!("t{s}.sqlite"));
        let name = if r.chance(50) { needle(r, &t) } else { "t".into() };
        match step("sqlite export", &ctx, || sqlite::write(st, rows.as_deref(), &q, &name, None, &no)) {
            Err(e) => assert!(st.ncols == 0, "{ctx}: sqlite export refused: {e}"),
            Ok(e) => {
                // (A database's rows: the data rows; its columns are named after the titles.)
                assert_eq!(e.rows as usize, n, "{ctx}: sqlite rows {e:?}");
                let tables = step("sqlite tables", &ctx, || sqlite::tables(&q)).unwrap();
                assert!(!tables.is_empty(), "{ctx}");
                let mut out = Vec::new();
                let imp = step("sqlite import", &ctx, || sqlite::to_csv(&q, Some(&tables[0].name), b';', &mut out, None, &no)).unwrap_or_else(|e| panic!("{ctx}: sqlite read back: {e}"));
                assert_eq!(imp.rows as usize, n, "{ctx}: sqlite rows read back");
                let back = table_of(&out, OpenOptions { enc: Some(Enc::Utf8), delim: Some(b';') });
                assert_eq!(back.st.base.n, n + 1, "{ctx}: sqlite CSV records\n{}", show(&out));
            }
        }
        let _ = std::fs::remove_file(&x);
        let _ = std::fs::remove_file(&q);
    });
    // Broken files of each kind: refused, never a panic.
    cases("broken_workbooks_and_databases", 40, |r, s| {
        let no = AtomicBool::new(false);
        let mut t = table_of(b"a,b\n1,2\n3,x\n", OpenOptions::default());
        if r.chance(50) { t = messy_table(r); }
        let x = dir.join(format!("b{s}.xlsx"));
        let q = dir.join(format!("b{s}.sqlite"));
        let _ = xlsx_write::write(&t.st, None, &x, "s", None, &no);
        let _ = sqlite::write(&t.st, None, &q, "t", None, &no);
        for p in [&x, &q] {
            let mut b = std::fs::read(p).unwrap_or_default();
            match r.below(4) {
                0 => b.truncate(r.below(b.len() + 1)),
                1 => for _ in 0..r.range(1, 50) { let k = r.below(b.len().max(1)); if k < b.len() { b[k] = r.next() as u8; } },
                2 => b = gen_bytes(r),
                _ => b.extend(gen_bytes(r)),
            }
            std::fs::write(p, &b).unwrap();
        }
        let ctx = format!("seed {s}");
        step("xlsx sheets (broken)", &ctx, || { let _ = xlsx_read::sheets(&x); });
        step("xlsx import (broken)", &ctx, || { let _ = xlsx_read::to_csv(&x, None, b',', Vec::new(), None, &no); });
        step("sqlite tables (broken)", &ctx, || { let _ = sqlite::tables(&q); });
        step("sqlite import (broken)", &ctx, || { let _ = sqlite::to_csv(&q, None, b',', Vec::new(), None, &no); });
        step("import (broken)", &ctx, || { let _ = convert::import(convert::Format::Xlsx, &x, None, b',', &dir.join(format!("o{s}.csv")), false, None, &no); });
        for p in [&x, &q, &dir.join(format!("o{s}.csv"))] { let _ = std::fs::remove_file(p); }
    });
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 5. cancellation -----------------------------------------------------------------------

/// Long jobs cancelled at random moments from another thread: each returns (cancelled or
/// done), no panic, no hang.
#[test]
fn cancelled_anywhere() {
    use std::time::{Duration, Instant};
    let mut b = b"id,name,city,amount,date,mail\n".to_vec();
    let mut r = Rng::new(env_seed());
    for i in 0..60_000 {
        b.extend(format!("{i},{} {},{},{}.{},2024-0{}-1{},n{}@x.fr\n", r.pick(WORDS), i % 977, r.pick(&["Lyon", "Paris", "Nîmes", "\"Saint, Étienne\""]), r.below(10000), r.below(100), 1 + i % 9, i % 10, i % 5000).as_bytes());
    }
    let dir = scratch("cancel");
    let p = dir.join("big.csv");
    std::fs::write(&p, &b).unwrap();
    let t = Table::new(Arc::new(Base::open(&p, OpenOptions::default(), None).unwrap()), None);
    let st = t.st.clone();
    let ids: Vec<RowId> = (0..st.len()).map(|k| st.data_id(k)).collect();
    let kinds = view::detect_kinds(&st);
    type Job<'a> = Box<dyn Fn(&AtomicBool) + Sync + 'a>;
    let jobs: Vec<(&str, Job)> = vec![
        ("filter", Box::new(|c| { let f = view::Filter::compile(&FilterSpec { global: "ly".into(), cols: vec![(3, ">50".into())], ..Default::default() }).unwrap(); let _ = view::filter(&st, &f, c); })),
        ("sort", Box::new(|c| { let mut v = ids.clone(); let _ = view::sort(&st, &mut v, &[SortKey { col: 1, desc: true }, SortKey { col: 3, desc: false }], &kinds, c); })),
        ("profile", Box::new(|c| { let _ = stats::profile(&st, &ids, c); })),
        ("dupes", Box::new(|c| { let _ = dupes::find(&st, &ids, &dupes::DupSpec::default(), c, None); })),
        ("group", Box::new(|c| { let _ = group::group_by(&st, &ids, &group::GroupSpec { by: vec![2], aggs: vec![(3, group::Aggs::SUM)], how: group::Match::Loose }, &kinds, c, None); })),
        ("insights", Box::new(|c| { let _ = insights::insights(&st, c, None); })),
        ("schema", Box::new(|c| { let _ = schema::infer(&st, c, None); })),
        ("cache", Box::new(|c| { colcache::clear(); let _ = colcache::ensure(&st, 1, Need::Rank, c); })),
        ("xlsx", Box::new(|c| { let _ = xlsx_write::write(&st, None, &dir.join("c.xlsx"), "s", None, c); })),
        ("sqlite", Box::new(|c| { let _ = sqlite::write(&st, None, &dir.join("c.sqlite"), "t", None, c); })),
        ("anonymize", Box::new(|c| { let m = csvfab_engine::anonymize::guess_all(&st); let _ = csvfab_engine::anonymize::anonymize(&st, &ids, &m, c, None); })),
    ];
    for (name, job) in &jobs {
        for k in 0..scale(2) {
            let cancel = AtomicBool::new(false);
            let delay = Duration::from_micros([0, 300, 3000, 30_000][(k + r.below(4)) % 4]);
            let t0 = Instant::now();
            std::thread::scope(|sc| {
                let h = sc.spawn(|| catch_unwind(AssertUnwindSafe(|| job(&cancel))));
                std::thread::sleep(delay);
                cancel.store(true, Ordering::Relaxed);
                assert!(h.join().unwrap().is_ok(), "{name}: panicked when cancelled after {delay:?}");
            });
            // A cancelled job stops promptly (generous bound: debug builds, a busy machine).
            assert!(t0.elapsed() < Duration::from_secs(60), "{name}: took {:?} once cancelled", t0.elapsed());
        }
    }
    // Progress counted while scanning a file being read: never past the file.
    let progress = AtomicUsize::new(0);
    let b2 = Base::open(&p, OpenOptions::default(), Some(&progress)).unwrap();
    assert!(progress.load(Ordering::Relaxed) <= b2.bytes().len() + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 6. encodings ----------------------------------------------------------------------------

/// Random text saved in every encoding and read back in it: what the encoding cannot hold is
/// told beforehand (`unencodable`), and is all that changes.
#[test]
fn encodings_save_what_they_can() {
    cases("encodings_save_what_they_can", 150, |r, s| {
        let n = r.range(1, 6);
        let mut rows: Vec<Vec<String>> = vec![(0..3).map(|c| format!("h{c}")).collect()];
        for _ in 0..n { rows.push((0..3).map(|_| if r.chance(30) { (0..r.range(1, 8)).map(|_| char::from_u32(r.below(0x11000) as u32).unwrap_or('?')).collect() } else { gen_value(r).chars().take(50).collect() }).collect()); }
        let src = b"h0,h1,h2\n".to_vec();
        let mut t = table_of(&src, OpenOptions::default());
        let ids = t.insert_rows(0, n);
        let ch: Vec<(RowId, usize, String)> = ids.iter().enumerate().flat_map(|(k, &id)| rows[k + 1].iter().enumerate().map(move |(c, v)| (id, c, v.clone()))).collect();
        t.set_cells(&ch);
        for e in Enc::ALL {
            for bom in [None, Some(true), Some(false)] {
                let o = SaveOptions { enc: Some(e), bom, ..Default::default() };
                let out = save::write_to(&t.st, &o, Vec::new()).unwrap();
                let ctx = format!("seed {s} {e:?} bom {bom:?}\n  {}", show(&out));
                let back = Table::new(Arc::new(Base::from_bytes(out.clone(), OpenOptions { enc: Some(e), delim: Some(b',') })), Some(true));
                assert_eq!(back.st.len(), n, "{ctx}: rows");
                let wrote_bom = !e.bom().is_empty() && out.starts_with(e.bom());
                if let Some(b) = bom { if !e.bom().is_empty() { assert_eq!(wrote_bom, b, "{ctx}: BOM"); } }
                for (k, row) in rows.iter().enumerate().skip(1) {
                    for (c, v) in row.iter().enumerate() {
                        let got = back.st.cell(back.st.data_id(k - 1), c);
                        let lossy = enc::unencodable(e, v).is_some();
                        if lossy { assert_ne!(&got, v, "{ctx}: said unencodable, yet it came back"); } else { assert_eq!(&got, v, "{ctx}: row {k} col {c}"); }
                        assert_eq!(got, through(e, v), "{ctx}: row {k} col {c} not as the encoding holds it");
                    }
                }
            }
        }
    });
}

// ---- regressions: one per bug the torture found -------------------------------------------

/// Opens `bytes`, applies `edit`, saves; the saved bytes read back (same encoding, delimiter
/// and title row) as the table's cells.
fn edit_and_reread(bytes: &[u8], o: OpenOptions, edit: impl FnOnce(&mut Table)) -> (Vec<u8>, Model, Table) {
    let mut t = table_of(bytes, o);
    edit(&mut t);
    let out = saved(&t);
    let m = Model::of(&t);
    reads_as_model(&out, &t.st.base, &m, None, &format!("{}", show(bytes)));
    (out, m, t)
}

/// A UTF-8 BOM with a bad byte further on: still UTF-8 (the BOM decides), and the BOM is
/// written back (it was lost: the file was taken for 1252, which has none).
#[test]
fn regression_bom_with_a_bad_byte() {
    let src = b"\xEF\xBB\xBFa,b\n1,\xE9\n";
    let t = table_of(src, OpenOptions::default());
    assert_eq!((t.st.base.enc, t.st.base.bom), (Enc::Utf8, true));
    assert_eq!(saved(&t), src);
    assert_eq!(t.st.cell(t.st.data_id(0), 1), "\u{FFFD}");
}

/// A delimiter forced to a byte above 0x7F cuts UTF-8 characters: the fields are decoded (they
/// were taken as text unchecked, invalid UTF-8 in a `&str`).
#[test]
fn regression_non_ascii_delimiter() {
    let t = table_of("é,ü\n".as_bytes(), OpenOptions { enc: None, delim: Some(0xC3) });
    assert!(!t.st.base.dialect.lines);
    for c in 0..t.st.ncols { assert!(t.st.cell(t.st.all_id(0), c).chars().all(|ch| ch == '\u{FFFD}' || ch.is_ascii())); }
}

/// Rows of empty cells were written as empty lines, which no reading takes as a record: gone
/// once the file was opened again.
#[test]
fn regression_empty_rows_survive() {
    let (out, ..) = edit_and_reread(b"a,b\n1,2\n", OpenOptions::default(), |t| { t.insert_rows(1, 2); });
    assert_eq!(out, b"a,b\n1,2\n,\n,\n");
    let (out, ..) = edit_and_reread(b"a\n1\n2\n", OpenOptions::default(), |t| { let id = t.st.data_id(0); t.set_cells(&[(id, 0, String::new())]); });
    assert_eq!(out, b"a\n\"\"\n2\n");
}

/// `x\r\r\n`: the CR before the CRLF is the record's. Put after a quoted value it left the quote
/// open, and the next rows in it.
#[test]
fn regression_cr_before_crlf() {
    edit_and_reread(b"a,b\r\n1,x\r\r\n2,y\r\n3,z\r\n", OpenOptions::default(), |t| { let id = t.st.data_id(0); t.set_cells(&[(id, 1, "\"q\"".into())]); });
}

/// The last line's lone CR (a LF file) is no line break: the last row moved up was glued to the next.
#[test]
fn regression_last_lone_cr() {
    edit_and_reread(b"a\n1\n2\r", OpenOptions::default(), |t| { let (two, one) = (t.st.data_id(1), t.st.data_id(0)); t.move_row(two, one, false); });
}

/// A CR file whose first title holds a quoted LF was read as LF: one row.
#[test]
fn regression_cr_file_with_quoted_lf() {
    let t = table_of(b"\"a\nb\",c\r1,2\r3,4\r", OpenOptions::default());
    assert_eq!((t.st.base.dialect.nl, t.st.base.eol, t.st.len()), (b'\r', &b"\r"[..], 2));
    // A stray quote in a title (not opening a field) changes nothing.
    let t = table_of(b"5\" screen,b\r\n1,\"x\ry\"\r\n", OpenOptions::default());
    assert_eq!((t.st.base.dialect.nl, t.st.len()), (b'\n', 1));
}

/// CR chosen for a LF file: a record holding a CR as a value was copied as it was, and cut there
/// once read again; the empty lines before the first record kept their LF (read as a LF file).
#[test]
fn regression_cr_chosen_for_a_lf_file() {
    edit_and_reread(b"a,b\n1,x\ry\n2,z\n", OpenOptions::default(), |t| t.set_eol(b"\r"));
    edit_and_reread(b"\n\na,b\n1,2\n", OpenOptions::default(), |t| t.set_eol(b"\r"));
    edit_and_reread(b"a,b\r\n\r\r\n3,4\r\n", OpenOptions::default(), |t| t.set_eol(b"\n"));
}

/// A last record whose quote never closes runs to the end: moved up, it took in every row after
/// it once read again. In a CR file its last `\r\n` also made a row of the LF.
#[test]
fn regression_open_last_record_moved() {
    edit_and_reread(b"a,b\n1,2\n3,\"open\n4,5\n", OpenOptions::default(), |t| { let (l, f) = (t.st.data_id(t.st.len() - 1), t.st.data_id(0)); t.move_row(l, f, false); });
    edit_and_reread(b"a,b\r1,2\r3,\"open\r4,5\r\n", OpenOptions::default(), |t| { t.insert_column(0, "c"); });
}

/// Rows made after the file's last record, one of the later ones edited: the copy of the run
/// before it went past the records (index out of bounds).
#[test]
fn regression_new_rows_after_the_last_record() {
    edit_and_reread(b"a\n1\n", OpenOptions::default(), |t| {
        let ids = t.insert_rows(1, 3);
        t.set_cells(&[(ids[2], 0, "x".into())]);
    });
}

/// Raw text: an empty last line made here had no line break, and was no line once read again.
#[test]
fn regression_text_empty_last_line() {
    edit_and_reread(b"one\ntwo", OpenOptions { enc: None, delim: Some(0) }, |t| { t.insert_rows(2, 1); });
}

/// A record put first that the file did not start with: its lone CR (LF file), its LF (CR file),
/// or a LF starting the second record (CR file) made the file read with other line breaks.
#[test]
fn regression_shifted_first_record() {
    edit_and_reread(b"a,b\n1,x\ry\n", OpenOptions::default(), |t| { let (one, h) = (t.st.data_id(0), t.st.all_id(0)); t.set_header(false); t.move_row(one, h, false); });
    edit_and_reread(b"a,b\r1,x\ny\r", OpenOptions::default(), |t| { t.set_header(false); let (one, h) = (t.st.data_id(1), t.st.data_id(0)); t.move_row(one, h, false); });
    edit_and_reread(b"a,b\r1,2\r\n3,4\r", OpenOptions::default(), |t| { t.set_header(false); let (x, h) = (t.st.data_id(2), t.st.data_id(1)); t.move_row(x, h, false); });
}

/// A U+FEFF starting the first cell of a file without BOM was read back as a BOM.
#[test]
fn regression_zero_width_no_break_space_first() {
    let (out, ..) = edit_and_reread(b"a,b\n1,2\n", OpenOptions::default(), |t| { let h = t.st.all_id(0); t.set_cells(&[(h, 0, "\u{feff}x".into())]); });
    assert!(out.starts_with(b"\"\xEF\xBB\xBFx\""));
}

/// A file of no record but a last lone CR: rows made here were glued to it.
#[test]
fn regression_rows_after_a_lone_cr() {
    edit_and_reread(b"\n\n\r", OpenOptions::default(), |t| { let ids = t.insert_rows(0, 2); t.set_cells(&[(ids[1], 0, "x".into())]); });
}

/// A row moved inside a run of records: the run's ends still looked consecutive, and a column's
/// substring filter on its folded cache gave each row the verdict of another.
#[test]
fn regression_filter_after_a_row_moved() {
    let src: String = std::iter::once("v\n".to_string()).chain((0..200).map(|i| format!("{}{i}\n", if i % 2 == 0 { "yes" } else { "no" }))).collect();
    let mut t = table_of(src.as_bytes(), OpenOptions::default());
    let (a, b) = (t.st.data_id(10), t.st.data_id(100));
    t.move_row(a, b, true);
    let no = AtomicBool::new(false);
    let sp = FilterSpec { cols: vec![(0, "yes".into())], ..Default::default() };
    let all: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
    let _ = colcache::ensure(&t.st, 0, Need::Text, &no);
    assert_eq!(view::filter(&t.st, &view::Filter::compile(&sp).unwrap(), &no).unwrap(), plain_filter(&t, &sp, &all).unwrap());
}

/// Spaces after a closing quote (`"x"  ,`) are in the raw bytes, not in the cell: a search for a
/// space found the row.
#[test]
fn regression_blank_search_after_a_quote() {
    let src: String = std::iter::once("a,b\n".to_string()).chain((0..100).map(|i| format!("\"x{i}\"  ,y\n"))).collect();
    let t = table_of(src.as_bytes(), OpenOptions::default());
    let no = AtomicBool::new(false);
    for case in [false, true] {
        let f = view::Filter::compile(&FilterSpec { global: " ".into(), match_case: case, ..Default::default() }).unwrap();
        assert!(view::filter(&t.st, &f, &no).unwrap().is_empty());
    }
}

/// A closing quote followed by a lone CR (a LF file) is a stray quote for the scan, which went on:
/// the fields stopped there, the rest of the record unread (not searched, not saved).
#[test]
fn regression_quote_then_lone_cr() {
    let t = table_of(b"a,b\n\"x\"\ry\",2\n", OpenOptions::default());
    assert_eq!((t.st.cell(t.st.data_id(0), 0), t.st.cell(t.st.data_id(0), 1)), ("x\"\ry".to_string(), "2".to_string()));
}

/// A space delimiter: a closing quote before it was taken for a quote followed by spaces.
#[test]
fn regression_space_delimiter() {
    edit_and_reread(b"a b\n1 2\n", OpenOptions { enc: None, delim: Some(b' ') }, |t| { let id = t.st.data_id(0); t.set_cells(&[(id, 0, "x y".into())]); });
}

/// A CR file whose first record grew past the 256 KB looked at for its line break: read again
/// as LF, its rows in one.
#[test]
fn regression_cr_file_with_a_long_first_record() {
    edit_and_reread(b"a;b\r1;2\r", OpenOptions::default(), |t| { let h = t.st.all_id(0); t.set_cells(&[(h, 0, "x".repeat(300 << 10))]); });
}

/// A table of no column (all deleted) diagnosed; an e-mail rule on "Élodie@…" (cut inside É);
/// a NUL in a title exported to SQLite (the statement was refused).
#[test]
fn regression_tools_on_odd_cells() {
    let mut t = table_of(b"a,b\n1,2,3\n4\n", OpenOptions::default());
    t.delete_columns(&[0, 1]);
    for p in 0..t.st.len() { let _ = csvfab_engine::diagnose::diagnose(&t.st, t.st.data_id(p)); }
    let t = table_of("mail\nÉlodie@x.fr\nmailto:a@b.fr\n".as_bytes(), OpenOptions::default());
    let spec = dupes::DupSpec { keys: vec![vec![dupes::Field { col: 0, clean: dupes::Clean::Mail { canon: true } }]] };
    let no = AtomicBool::new(false);
    let _ = dupes::find(&t.st, &[1, 2], &spec, &no, None);
    let t = table_of(b"a\0b,ab\n1,2\n", OpenOptions::default());
    let dir = scratch("nul");
    let q = dir.join("t.sqlite");
    sqlite::write(&t.st, None, &q, "t\0x", None, &no).unwrap();
    let mut out = Vec::new();
    sqlite::to_csv(&q, None, b',', &mut out, None, &no).unwrap();
    assert_eq!(out, b"ab,ab_2\n1,2\n");
    let _ = std::fs::remove_dir_all(&dir);
    // Files combined into one column: an empty cell was an empty line, the row gone once read.
    use csvfab_engine::combine;
    let t = table_of(b"a\n1\n\"\"\n2\n", OpenOptions::default());
    let srcs = vec![combine::Source { name: "a.csv".into(), st: t.st.clone(), rows: None }];
    let out = combine::write_to(&srcs, &combine::columns(&srcs, false, false), false, &combine::format(&srcs), Vec::new(), &no, None).unwrap();
    assert_eq!(table_of(&out, OpenOptions::default()).st.len(), 3, "{}", show(&out));
}
