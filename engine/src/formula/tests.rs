//! The web app's formula tests (tests/test_formula_safety.py,
//! tests/test_regex_guard.py), on the same file, plus the engine's runners.

use super::*;
use crate::base::{Base, OpenOptions};
use crate::table::Table;
use std::sync::Arc;

/// tests/test_formula_safety.py's file: a ;-separated one, so numbers come out with a decimal comma.
fn csv() -> String {
    let mut s = String::from("id;nom;ville;montant;date;email\n");
    for i in 1..=60 {
        s += &format!("{i};Nom{i} Dupont;{};{},{:02};{:02}/{:02}/20{};user{i}@exemple.fr\n",
            if i % 3 != 0 { "Lyon" } else { "Paris" }, i * 13 % 997, i % 100, 1 + i % 28, 1 + i % 12, 10 + i % 15);
    }
    s
}

fn table(s: &str) -> Table {
    Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
}

fn no() -> AtomicBool {
    AtomicBool::new(false)
}

fn compiled(t: &Table, src: &str) -> Result<Expr, Error> {
    compile(src, &t.st.headers()).map(|mut e| {
        e.set_decimal_comma(decimal_comma(&t.st));
        e
    })
}

/// The value on the first row, or the error, as the web app's evalRow gave it.
fn first(t: &Table, src: &str) -> String {
    match compiled(t, src) {
        Err(e) => format!("ERREUR {}", e.msg),
        Ok(e) => e.eval_row(&t.st, t.st.data_id(0)).unwrap_or_else(|m| format!("ERR:{m}")),
    }
}

#[test]
fn useful_syntax_still_works() {
    let t = table(&csv());
    for (src, want) in [
        ("row % 2 === 0", "false"),
        ("{montant}.num() > 100 ? \"big\" : \"small\"", "small"),
        ("Math.round({montant}.num())", "13"),
        ("Math.max(1, 2) ** 2", "4"),
        ("-{montant}.num()", "-13,01"),
        ("matches({email}, /^user\\d+@/)", "true"),
        ("[{ville}, \"x\"].join(\"-\")", "Lyon-x"),
        ("{ville}.length", "4"),
        ("Math.PI > 3", "true"),
        ("{nom}.replace(/\\d+/g, \"#\")", "Nom# Dupont"),
        ("!empty({nom}) && {ville} !== \"Paris\"", "true"),
        ("{date}.date().year() >= 2010", "true"),
        ("first(\"\", {ville})", "Lyon"),
        ("\"a\\\"b\" + 'c'", "a\"bc"),
        ("0x1F + .5", "31,5"),
        ("1e3", "1000"),
    ] {
        assert_eq!(first(&t, src), want, "{src}");
    }
}

#[test]
fn escapes_are_refused() {
    let t = table(&csv());
    let refused = [
        "fetch(\"/api/file\")", "window.CSVFAB_TOKEN", "CSVFAB_TOKEN", "this", "globalThis", "self", "top", "parent",
        "document.cookie", "location", "eval(\"1\")", "(0, eval)(\"1\")", "Function(\"return 1\")()", "new Date()",
        "\"\".constructor.constructor(\"return 1\")()", "{nom}.constructor", "{nom}.__proto__", "{nom}.prototype",
        "[].constructor", "/a/.constructor", "Math.constructor", "Math", "{nom}.toString.call", "{nom}.at.bind({nom})",
        "upper.call(null, \"x\")", "upper", "today", "{nom}.upper", "x = 1", "{nom} = \"x\"", "$[0]", "$", "row = 1",
        "a => a", "(() => 1)()", "`${1}`", "1; 2", "1, 2", "{nom}?.length", "import(\"x\")", "require(\"fs\")",
        "__xss()", "fxAt({nom}, 0)", "srvFetch(\"/api/file\")", "{nom}[\"constructor\"][\"constructor\"](\"return 1\")()",
        "{nom}.split(\" \")[\"constructor\"](\"x\")", "typeof {nom}", "void 0", "delete {nom}", "{nom} in {nom}",
        "upper({nom})()", "({nom}).upper()()", "\"x\"(\"y\")", "{inconnue}", "{nom", "\"non fermé", "/x",
    ];
    let accepted: Vec<_> = refused.iter().filter(|s| compiled(&t, s).is_ok()).collect();
    assert!(accepted.is_empty(), "accepted: {accepted:?}");
}

#[test]
fn messages_say_what_to_write() {
    let t = table(&csv());
    let msg = |s: &str| compiled(&t, s).err().unwrap();
    assert_eq!(msg("fetch(\"/x\")").msg, "Unknown name: fetch — a column is written {fetch}, a helper is called with ( )");
    assert_eq!(msg("{nom} = \"x\"").msg, "Use === to compare (= would assign)");
    assert_eq!(msg("{nom} = \"x\"").span, 6..7);
    assert_eq!(msg("upper").msg, "upper is a function: write upper(…)");
    assert_eq!(msg("{nom}.upper").msg, ".upper is a method: write .upper(…)");
    assert_eq!(msg("{nom}.constructor").msg, ".constructor is not available in a formula");
    assert_eq!(msg("Math").msg, "Expected “.” at the end");
    assert_eq!(msg("Math.constructor").msg, "Math.constructor is not available in a formula");
    assert_eq!(msg("1, 2").msg, "Unexpected near “,”");
    assert_eq!(msg("{inconnue}").msg, "Unknown column: {inconnue}");
    assert_eq!(msg("{nom}?.length").msg, "Optional chaining ?. is not supported: test the value first");
    assert_eq!(msg("1 +").msg, "A value is missing at the end");
    assert_eq!(msg("\"a").msg, "A text in quotes is not closed");
    assert_eq!(msg("12abc").msg, "“12a” is not a number");
    assert_eq!(msg("08").msg, "Decimals with leading zeros are not allowed in strict mode.");
    assert!(msg("/(/").msg.starts_with("Invalid regular expression: /(/"));
    assert!(msg("/(a)\\1/").msg.contains("back-references"));
    assert!(compile("  ", &[]).unwrap_err().is_empty());
    // A column is found ignoring case, accents and symbols.
    let e = compile("{CITY } + {prénom}", &["city".into(), "Prenom".into()]).unwrap();
    assert_eq!(e.columns(), &[0, 1]);
}

#[test]
fn computed_index_is_neutralised() {
    let t = table(&csv());
    let all = |src: &str| {
        let e = compiled(&t, src).unwrap_or_else(|e| panic!("{src}: {}", e.msg));
        let o = column(&t.st, &e, None, &no()).unwrap();
        let mut v = o.out;
        v.dedup();
        v
    };
    for src in ["{nom}[\"constructor\"]", "{nom}[\"__proto__\"]", "{nom}[\"length\"]", "{nom}.split(\" \")[\"constructor\"]", "[1, 2][\"constructor\"]", "{nom}[{nom}]"] {
        assert_eq!(all(src), [""], "{src}");
    }
    assert_eq!(&all("{nom}.split(\" \")[0]")[..2], ["Nom1", "Nom2"]);
    assert_eq!(all("{nom}[0]"), ["N"]);
}

#[test]
fn duplicates_formula_is_checked_too() {
    assert_eq!(compile_value("left(slug(v), 3)").unwrap().eval_value("Éléonore").unwrap(), "ele");
    assert!(compile_value("fetch(\"/x\")").unwrap_err().msg.contains("Unknown name: fetch"));
    assert!(compile_value("{nom}").unwrap_err().msg.contains("Write v"));
}

/// Every documented example compiles and runs on every row, its columns put in (fxExample).
#[test]
fn every_documented_example_runs() {
    let t = table(&csv());
    let mut count = 0;
    for d in doc::CATALOGUE {
        for (ex, _) in d.ex {
            let text = if d.pick == Some(doc::Pick::Email) { "email" } else { "nom" };
            let src = doc::fill_example(ex, "montant", "date", text, "ville");
            let e = compiled(&t, &src).unwrap_or_else(|e| panic!("{src}: {}", e.msg));
            let o = column(&t.st, &e, None, &no()).unwrap();
            assert_eq!(o.out.len(), 60);
            // A text read as a date that is none, or a sign test on a missing value, may fail a row; none of these do.
            assert_eq!(o.errors, 0, "{src}: {:?}", o.first_error);
            count += 1;
        }
    }
    assert!(count > 80, "{count}");
}

#[test]
fn documented_values() {
    let t = table(&csv());
    // row 1: 1;Nom1 Dupont;Lyon;13,01;02/02/2011;user1@exemple.fr
    for (src, want) in [
        ("{montant}.num() * 1.2", "15,612"),
        ("{montant}.num().round(1)", "13"),
        ("{montant}.num().fixed(2)", "13,01"),
        ("{montant}.num().fixed(1)", "13,0"),
        ("sum({montant}, 100)", "113,01"),
        ("avg({montant}, \"\")", "13,01"),
        ("min({montant}, 1000)", "13,01"),
        ("{date}.date()", "2011-02-02"),
        ("{date}.addDays(30)", "2011-03-04"),
        ("{date}.addMonths(1)", "2011-03-02"),
        ("{date}.year() === 2011", "true"),
        ("{date}.month()", "2"),
        ("{date}.weekday()", "3"),
        ("{date}.week()", "5"),
        ("{date}.fmtDate(\"yyyy-mm-dd\")", "2011-02-02"),
        ("{date}.fmtDate()", "02/02/2011"),
        ("{date}.days(\"2011-03-02\")", "28"),
        ("{date}.months(\"2012-02-01\")", "11"),
        ("{date}.years(\"2021-02-02\")", "10"),
        ("{nom}.upper()", "NOM1 DUPONT"),
        ("{nom}.capitalize()", "Nom1 Dupont"),
        ("trim(\"  a   b \")", "a b"),
        ("\"Émile  O'Brien\".slug()", "emile o brien"),
        ("{nom}.len() > 20", "false"),
        ("{nom}.left(3)", "Nom"),
        ("{nom}.right(3)", "ont"),
        ("{nom}.mid(2, 3)", "om1"),
        ("split({nom}, \" \", -1)", "Dupont"),
        ("{montant}.pad(8, \"0\")", "00013,01"),
        ("\"68 - Rhin\".firstNumber()", "68"),
        ("\"Rhin -3,75 €\".firstNumber()", "-3,75"),
        ("\"a-3\".firstNumber()", "3"),
        ("\"06 12-34\".digits()", "061234"),
        ("{nom}.phonetic()", "NN TPN"),
        ("\"Dupont\".similarity(\"Dupond\")", "0,8333333333"),
        ("{email}.extract(\"@(.+)$\")", "exemple.fr"),
        ("{email}.extract(\"(x)?@\")", "@"),
        ("replace({nom}, \"\\\\s+\", \"-\")", "Nom1-Dupont"),
        ("replace({nom}, \"\\s+\", \"-\")", "Nom1 Dupont"),
        ("{ville}.contains(\"YON\")", "true"),
        ("\"Émile\".contains(\"emi\")", "true"),
        ("startsWith({nom}, \"nom\")", "true"),
        ("{nom}.matches(\"^[a-m]\")", "false"),
        ("{nom}.matches(\"^[n-z]\")", "true"),
        ("!{email}.isEmail()", "false"),
        ("\"06 12 34 56 78\".isPhone()", "true"),
        ("\"+1 (203) 893-3004\".isPhone()", "true"),
        ("\"12\".isPhone()", "false"),
        ("{montant}.isNumber()", "true"),
        ("\"1e5\".isNumber()", "true"),
        ("!{date}.isDate()", "false"),
        ("ifs({montant}.num() > 1000, \"big\", {montant}.num() > 10, \"medium\", \"small\")", "medium"),
        ("{ville}.cases(\"Paris\", \"75\", \"Lyon\", \"69\", \"other\")", "69"),
        ("{date}.month().cases(1, \"January\", 2, \"February\", \"other\")", "February"),
        ("first(\"\", \" \", {ville}, \"none\")", "Lyon"),
        ("join(\" \", {nom}, \"\", {ville})", "Nom1 Dupont Lyon"),
        ("{email}.md5()", "43ba5b472e159c9127a5ddcd14409d02"),
        ("\"\".sha256()", ""),
        ("{nom}.startsWith(\"N\")", "true"),
        ("{nom}.includes(\"-\")", "false"),
        ("{nom}.slice(-4)", "pont"),
        ("{nom}.replaceAll(\" \", \"\")", "Nom1Dupont"),
        ("{montant}.padStart(6, \"0\")", "013,01"),
        ("{nom} + \" \" + {ville}", "Nom1 Dupont Lyon"),
        ("{montant}.num() / 2", "6,505"),
        ("{date}.date() < today()", "true"),
        ("uuid().length", "36"),
        ("uuid7().length", "36"),
    ] {
        assert_eq!(first(&t, src), want, "{src}");
    }
}

#[test]
fn javascript_semantics() {
    let t = table("a,b\n10,x\n");
    for (src, want) in [
        ("{a} + 1", "101"),
        ("{a} - 1", "9"),
        ("{a} * \"2\"", "20"),
        ("{a} > 9", "true"),
        ("{a} > \"9\"", "false"),
        ("{a} == 10", "true"),
        ("{a} === 10", "false"),
        ("null == undefined", "true"),
        ("null === undefined", "false"),
        ("NaN === NaN", "false"),
        ("[1, 2] + 1", "1,21"),
        ("1 / 0", ""),
        ("0.1 + 0.2", "0.3"),
        ("2 ** -1", "0.5"),
        ("-2 ** 2", "4"),
        ("2 ** 3 ** 2", "512"),
        ("7 % -3", "1"),
        ("\"\" || \"none\"", "none"),
        ("0 ?? 5", "0"),
        ("null ?? 5", "5"),
        ("{b}.toUpperCase().charCodeAt(0)", "88"),
        ("(1.005).toFixed(2)", "1.00"),
        ("(2.5).toFixed(0)", "3"),
        ("(255).toString(16)", "ff"),
        ("\"a,b,,c\".split(\",\").length", "4"),
        ("\"abc\".at(-1)", "c"),
        ("\"abc\".substring(2, 0)", "ab"),
        ("\"aXbX\".replace(\"X\", \"[$&]\")", "a[X]bX"),
        ("\"2024-03-15\".match(/(\\d+)-(\\d+)/)[2]", "03"),
        ("\"a1b22\".match(/\\d+/g)", "1,22"),
        ("\"x\".match(/y/)", ""),
        ("\"abc\".search(/c/)", "2"),
        ("[3, 1, 2].reverse().join(\"\")", "213"),
        ("[1, [2, 3]].concat(4).length", "3"),
        ("[NaN].includes(NaN)", "true"),
        ("[NaN].indexOf(NaN)", "-1"),
        ("/a/i.test(\"A\")", "true"),
        ("String", "ERREUR Unknown name: String — a column is written {String}, a helper is called with ( )"),
        ("date(\"2024-02-30\")", ""),
        ("\"2024-03-15 10:20\".date()", "2024-03-15T10:20:00Z"),
        ("\"2024-03-15\".date().getTime()", "1710460800000"),
        ("\"2024-03-15\".date().toISOString()", "2024-03-15T00:00:00.000Z"),
        ("\"2024-01-31\".addMonths(1)", "2024-02-29"),
        ("\"é😀\".length", "3"),
        ("Math.min()", ""),
        ("Math.sign(-3)", "-1"),
        ("true + 1", "2"),
        ("round(2.5)", "3"),
        ("round(-2.5)", "-2"),
        ("round(1.005, 2)", "1"),
        ("num(\"1 234,50 €\")", "1234.5"),
        ("num(\"\")", "0"),
        ("num(\"abc\")", ""),
        ("\"%\" + num(\"12 kg\")", "%12"),
        ("1e21", "1e+21"),
        ("123456789.123456789", "123456789.12345679"),
    ] {
        assert_eq!(first(&t, src), want, "{src}");
    }
}

#[test]
fn runtime_errors_are_counted() {
    let t = Table::new(Arc::new(Base::from_bytes(b"d\n2024-01-01\nnot a date\nx\n".to_vec(), OpenOptions::default())), Some(true));
    let e = compiled(&t, "{d}.date().year()").unwrap();
    let o = column(&t.st, &e, None, &no()).unwrap();
    assert_eq!(o.out, ["2024", "", ""]);
    assert_eq!(o.errors, 2);
    assert_eq!(o.first_error.as_deref(), Some("Cannot read properties of null (reading 'year')"));
    let e = compiled(&t, "{d}.num().upper()").unwrap();
    assert_eq!(e.eval_row(&t.st, t.st.data_id(0)).unwrap_err(), "{d}.num().upper is not a function");
    let e = compiled(&t, "{d}.length()").unwrap();
    assert!(e.eval_row(&t.st, t.st.data_id(0)).unwrap_err().ends_with(".length is not a function"));
    // A pattern built from a cell that is no valid regex fails that row only.
    let t = table("p,s\n(,x\nx,x\n");
    let e = compiled(&t, "matches({s}, {p})").unwrap();
    let o = filter(&t.st, &e, None, &no()).unwrap();
    assert_eq!((o.out.len(), o.errors), (1, 1));
    assert!(o.first_error.unwrap().starts_with("Invalid regular expression: /(/"));
    // A constant one that is invalid fails every row, as it did; one too big is refused at once.
    let e = compiled(&t, "extract({s}, \"(\")").unwrap();
    assert_eq!(column(&t.st, &e, None, &no()).unwrap().errors, 2);
    assert_eq!(compiled(&t, "{s}.matches(\"(a{1000}){1000}\")").err().unwrap().msg, REGEX_TOO_BIG);
    assert_eq!(compiled(&t, "{s}.repeat(-1)").unwrap().eval_row(&t.st, t.st.data_id(0)).unwrap_err(), "Invalid count value: -1");
}

/// A computed column as a patch: the same cells as its values written one by one (empty
/// ones and errors left out), the same errors counted, rows moved or not.
#[test]
fn column_patch_as_the_values() {
    let mut src = String::from("n,s\n");
    for i in 0..20_000 { src += &format!("{},{}\n", if i % 9 == 0 { "x".to_string() } else { i.to_string() }, if i % 4 == 0 { "" } else { "s" }); }
    for moved in [false, true] {
        let mut t = table(&src);
        if moved { let (a, b) = (t.st.data_id(3), t.st.data_id(15_000)); t.move_row(a, b, true); }
        for f in ["{n}.num() * 2", "{s} + {n}.date().year()", "{s}"] {
            let e = compiled(&t, f).unwrap();
            let o = column(&t.st, &e, None, &no()).unwrap();
            let p = column_patch(&t.st, &e, 2, &no()).unwrap();
            assert_eq!((p.errors, p.first_error.clone()), (o.errors, o.first_error.clone()), "{f}");
            let ch: Vec<_> = o.out.into_iter().enumerate().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (t.st.data_id(k), 2, v)).collect();
            assert_eq!(p.out.cells(), ch.len(), "{f}");
            let mut a = Table::new(t.st.base.clone(), None);
            a.st = t.st.clone();
            let mut b = Table::new(t.st.base.clone(), None);
            b.st = t.st.clone();
            a.apply(p.out);
            b.set_cells(&ch);
            for k in 0..t.st.len() {
                let id = t.st.data_id(k);
                assert_eq!(a.st.cell(id, 2), b.st.cell(id, 2), "{f} row {k}");
            }
        }
    }
}

/// tests/test_regex_guard.py: (a+)+$ on « aaa…ab » froze the page; here it cannot.
#[test]
fn catastrophic_pattern_runs_in_linear_time() {
    let mut s = String::from("id;mot\n");
    for i in 1..=400 {
        s += &format!("{i};{}\n", if i % 2 == 1 { format!("{}b", "a".repeat(30)) } else { format!("abc{i}") });
    }
    let t = table(&s);
    let t0 = std::time::Instant::now();
    let e = compiled(&t, "matches({mot}, \"^(a+)+$\")").unwrap();
    assert!(filter(&t.st, &e, None, &no()).unwrap().out.is_empty());
    let e = compiled(&t, "matches({mot}, /a{30}b/)").unwrap();
    assert_eq!(filter(&t.st, &e, None, &no()).unwrap().out.len(), 200);
    assert!(t0.elapsed().as_millis() < 1000);
    assert!(js_regex("^(a+)+$", false).is_ok());
}

#[test]
fn runners() {
    let t = table(&csv());
    let e = compiled(&t, "num({montant}) > 400 && contains({ville}, \"LYON\")").unwrap();
    let all = filter(&t.st, &e, None, &no()).unwrap();
    let want: Vec<RowId> = (0..60).filter(|&p| { let i = p + 1; (i * 13 % 997) > 400 && i % 3 != 0 }).map(|p| t.st.data_id(p)).collect();
    assert_eq!(all.out, want);
    // among rows given, with their numbers kept
    let some: Vec<RowId> = (0..60).step_by(2).map(|p| t.st.data_id(p)).collect();
    let e = compiled(&t, "row").unwrap();
    let v = column(&t.st, &e, Some(&some), &no()).unwrap().out;
    assert_eq!(&v[..3], ["1", "3", "5"]);
    // changes: only cells that differ
    let e = compiled(&t, "{ville}.upper() === \"LYON\" ? \"Lyon\" : \"PARIS\"").unwrap();
    let ch = changes(&t.st, &e, None, 2, &no()).unwrap().out;
    assert_eq!(ch.len(), 20);
    assert!(ch.iter().all(|(_, c, v)| *c == 2 && v == "PARIS"));
    let mut t = t;
    t.set_cells(&ch);
    assert_eq!(t.st.cell(t.st.data_id(2), 2), "PARIS");
    // an edited row reads its edit
    let e = compiled(&t, "{ville} + {nom}").unwrap();
    assert_eq!(e.eval_row(&t.st, t.st.data_id(2)).unwrap(), "PARISNom3 Dupont");
    // cancelled
    let yes = AtomicBool::new(true);
    assert!(filter(&t.st, &e, None, &yes).is_none());
    // uuid7 comes out in row order
    let e = compiled(&t, "uuid7()").unwrap();
    let v = column(&t.st, &e, None, &no()).unwrap().out;
    assert!(v.windows(2).all(|w| w[0] < w[1]));
}

/// The bench file of 5 million rows: `cargo test -p csvfab-engine --release -- --ignored --nocapture formula_bench`.
#[test]
#[ignore]
fn formula_bench() {
    let path = std::env::var("CSVFAB_BENCH").unwrap_or_else(|_| "../benchmark/data/customers-5m.csv".into());
    let t0 = std::time::Instant::now();
    let base = Base::open(std::path::Path::new(&path), OpenOptions::default(), None).unwrap();
    let t = Table::new(Arc::new(base), None);
    println!("\nopen {} rows: {:?}", t.st.len(), t0.elapsed());
    let h = t.st.headers();
    for src in [
        "num({total_spent}) > 1000 && contains({city}, \"lyon\")",
        "{city} === \"Lyon\"",
        "{total_spent}.num() > 30000",
        "{email}.matches(\"@gmx\\\\.de$\")",
        "{signup_date}.year() === 2023",
        "{last_name}.upper() + \" \" + {first_name}",
        "{total_spent}.num().fixed(1)",
        "ifs({orders}.num() > 20, \"big\", {orders}.num() > 5, \"medium\", \"small\")",
        "{signup_date}.days({last_order})",
        "{first_name}.sha256()",
    ] {
        let t1 = std::time::Instant::now();
        let e = compile(src, &h).unwrap();
        let compile_us = t1.elapsed().as_secs_f64() * 1e6;
        let t1 = std::time::Instant::now();
        let f = filter(&t.st, &e, None, &no()).unwrap();
        let tf = t1.elapsed();
        let t1 = std::time::Instant::now();
        let c = column(&t.st, &e, None, &no()).unwrap();
        let tc = t1.elapsed();
        // As a new column: written as changes (one String a row, sorted and merged by the table),
        // and as a patch built in place.
        let t1 = std::time::Instant::now();
        let ch: Vec<_> = c.out.into_iter().enumerate().filter(|(_, v)| !v.is_empty()).map(|(k, v)| (t.st.data_id(k), 0, v)).collect();
        let mut a = Table::new(t.st.base.clone(), None);
        a.set_cells(&ch);
        drop(ch);
        let tw = tc + t1.elapsed();
        let t1 = std::time::Instant::now();
        let p = column_patch(&t.st, &e, 0, &no()).unwrap();
        let tp = t1.elapsed();
        let t1 = std::time::Instant::now();
        let mut b = Table::new(t.st.base.clone(), None);
        b.apply(p.out);
        let ta = t1.elapsed();
        println!("{src:<80} compile {compile_us:6.0} µs  filter {:>8.1} ms ({} rows)  column {:>8.1} ms ({} errors)  as changes {:>8.1} ms, as a patch {:>8.1} ms + {:.2} ms to apply",
            tf.as_secs_f64() * 1e3, f.out.len(), tc.as_secs_f64() * 1e3, c.errors, tw.as_secs_f64() * 1e3, tp.as_secs_f64() * 1e3, ta.as_secs_f64() * 1e3);
        if src.starts_with("num({total_spent}) > 1000") {
            assert!(f.out.len() > 0 && f.out.len() <= 75_010);
        }
    }
}

/// Row by row against the web app's own engine run in Node on the same file:
/// FXCHECK=<file of `src \t row \t value` lines> (see the report for the harness).
#[test]
#[ignore]
fn js_parity() {
    let Ok(path) = std::env::var("FXCHECK") else { return };
    let unesc = |s: &str| s.replace("\\t", "\t").replace("\\n", "\n").replace("\\\\", "\\");
    let t = table(&csv());
    let h = t.st.headers();
    let (mut same, mut diff) = (0, 0);
    let mut cache: Option<(String, Result<Expr, Error>)> = None;
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let mut f = line.splitn(3, '\t');
        let (src, row, want) = (unesc(f.next().unwrap()), f.next().unwrap().parse::<i64>().unwrap(), unesc(f.next().unwrap_or("")));
        if cache.as_ref().is_none_or(|c| c.0 != src) {
            cache = Some((src.clone(), compile(&src, &h).map(|mut e| { e.set_decimal_comma(true); e })));
        }
        let got = match &cache.as_ref().unwrap().1 {
            Err(e) => format!("COMPILE:{}", e.msg),
            Ok(_) if row < 0 => "COMPILE:(compiled)".into(),
            Ok(e) => e.eval_row(&t.st, t.st.data_id(row as usize)).unwrap_or_else(|m| format!("ERR:{m}")),
        };
        if got == want { same += 1 } else {
            diff += 1;
            println!("{src}  [row {row}]\n    js:   {want}\n    rust: {got}");
        }
    }
    println!("{same} same, {diff} different");
}

#[test]
fn tally_counts_without_building_the_column() {
    let t = table(&csv());
    let e = compiled(&t, "{ville} === \"Lyon\" ? {nom} : \"\"").unwrap();
    let full = column(&t.st, &e, None, &no()).unwrap();
    let n = tally(&t.st, &e, None, None, &no()).unwrap();
    assert_eq!(n.out, full.out.iter().filter(|v| !v.is_empty()).count());
    // Written into its own column: only the rows whose value differs.
    let e = compiled(&t, "{ville} === \"Lyon\" ? \"Lyon\" : \"x\"").unwrap();
    let ch = changes(&t.st, &e, None, 2, &no()).unwrap();
    assert_eq!(tally(&t.st, &e, None, Some(2), &no()).unwrap().out, ch.out.len());
    assert_eq!(ch.out.len(), 20);
}
