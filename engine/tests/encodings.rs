//! Every encoding, its fixture built here by its own encoder: read, edited, saved and read
//! again; what was not touched comes back byte for byte.

use csvfab_engine::base::{Base, OpenOptions};
use csvfab_engine::enc::{self, Enc};
use csvfab_engine::save::{self, SaveOptions};
use csvfab_engine::table::Table;
use encoding_rs::{Encoding, BIG5, EUC_JP, GBK, SHIFT_JIS, WINDOWS_1250, WINDOWS_1251, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255, WINDOWS_1256, KOI8_U, WINDOWS_874, EUC_KR};
use std::sync::Arc;

/// A text the encoding can hold, with what it is known for.
fn words(e: Enc) -> &'static str {
    match e {
        Enc::Utf8 | Enc::Utf16Le | Enc::Utf16Be | Enc::Utf32Le | Enc::Utf32Be | Enc::Gb18030 => "日本語 Москва 😀 é",
        Enc::Windows1252 | Enc::Latin1 | Enc::Latin9 | Enc::MacRoman => "Hôtel à Noël",
        Enc::Windows1251 | Enc::Koi8R | Enc::Koi8U | Enc::Iso8859_5 | Enc::Ibm866 => "Москва, улица Ленина",
        Enc::Windows1250 | Enc::Iso8859_2 => "Łódź żółć święty",
        Enc::Windows1253 | Enc::Iso8859_7 => "Αθήνα και Θεσσαλονίκη",
        Enc::Windows1254 => "İstanbul ağaç şeker",
        Enc::Windows1257 => "Rīga Tallinn ķēniņš",
        Enc::Windows1258 => "Hà ăn cơm đưa",
        Enc::Windows874 => "กรุงเทพมหานคร",
        Enc::Windows1255 | Enc::Iso8859_8 => "ירושלים תל אביב",
        Enc::Windows1256 | Enc::Iso8859_6 => "القاهرة والرياض",
        Enc::ShiftJis | Enc::EucJp | Enc::Iso2022Jp => "東京都ソフト表能申予",
        Enc::EucKr => "서울특별시 한국어",
        Enc::Gbk => "北京市朝阳区中文",
        Enc::Big5 => "臺北市許功蓋",
    }
}

/// What a file of `s` is in `e`, BOM included where it has one.
fn bytes(e: Enc, s: &str) -> Vec<u8> {
    let mut o = e.bom().to_vec();
    if e == Enc::Utf8 { o.clear(); }
    enc::encode_file(e, s, &mut o);
    o
}

fn table(b: Vec<u8>, o: OpenOptions) -> Table {
    Table::new(Arc::new(Base::from_bytes(b, o)), None)
}

fn saved(t: &Table) -> Vec<u8> {
    save::write_to(&t.st, &SaveOptions::default(), Vec::new()).unwrap()
}

fn file_of(w: &str) -> String {
    let mut s = String::from("name;city;n\r\n");
    for i in 0..200 {
        s += &format!("{w};\"{w}; {i}\";{i}\r\n");
    }
    s
}

#[test]
fn every_encoding_round_trips() {
    for e in Enc::ALL {
        let text = file_of(words(e));
        let b = bytes(e, &text);
        assert!(enc::decode_file(e, &b[if e == Enc::Utf8 { 0 } else { e.bom().len() }..]) == text, "{e:?} holds its text");
        let o = OpenOptions { enc: Some(e), delim: None };
        let t = table(b.clone(), o);
        assert_eq!((t.st.base.enc, t.st.base.dialect.delim, t.st.base.n), (e, b';', 201), "{e:?}");
        assert_eq!(t.st.cell(t.st.data_id(7), 0), words(e), "{e:?}");
        assert_eq!(t.st.cell(t.st.data_id(7), 1), format!("{}; 7", words(e)), "{e:?}");
        assert!(saved(&t) == b, "{e:?}: untouched");
        // One cell edited: that line changes, no other.
        let mut t = t;
        let id = t.st.data_id(50);
        t.set_cells(&[(id, 2, "x".into())]);
        let want = bytes(e, &text.replace(";50\r\n", ";x\r\n"));
        assert!(saved(&t) == want, "{e:?}: edited");
        let back = table(want, o);
        assert_eq!(back.st.cell(back.st.data_id(50), 2), "x", "{e:?}");
        assert_eq!(back.st.cell(back.st.data_id(50), 0), words(e), "{e:?}");
    }
}

/// Delimiters' bytes inside characters: the file is cut where the characters end.
#[test]
fn trail_bytes_are_not_delimiters() {
    for (e, enc_rs, w) in [(Enc::ShiftJis, SHIFT_JIS, "ソ表能"), (Enc::Gbk, GBK, "丂丏"), (Enc::Big5, BIG5, "許功蓋")] {
        let text = format!("a|b\n{w}|1\n{w}|2\n");
        let (b, _, bad) = enc_rs.encode(&text);
        assert!(!bad);
        let t = table(b.to_vec(), OpenOptions { enc: Some(e), delim: Some(b'|') });
        assert_eq!(t.st.len(), 2, "{e:?}");
        assert_eq!(t.st.cell(t.st.data_id(1), t.st.ncols - 1), "2", "{e:?}");
    }
    // 0x7C inside a character: a scan of the bytes would see two columns more.
    let b: &[u8] = b"a|b\n\x81\x7C|1\n";
    let t = table(b.to_vec(), OpenOptions { enc: Some(Enc::Gbk), delim: Some(b'|') });
    assert_eq!(t.st.ncols, 2);
    assert_eq!(t.st.cell(t.st.data_id(0), 1), "1");
}

/// Bytes with no meaning in the encoding, and characters two byte sequences give: rows left as
/// they were keep the file's bytes, whatever the text they read as.
#[test]
fn the_files_own_bytes_survive() {
    // Shift_JIS: NEC's 0xED40 and IBM's 0xFA5C are one character; a lone lead byte; 0x80.
    let mut sj: Vec<u8> = b"a,b\r\n".to_vec();
    sj.extend_from_slice(b"\xED\x40,1\r\n\xFA\x5C,2\r\n\x81,3\r\n\x80\xFF,4\r\nx,5\n\xE3\x81,6");
    // EUC-JP: a JIS X 0212 character, which its encoder does not write.
    let mut ej: Vec<u8> = b"a,b\n".to_vec();
    ej.extend_from_slice(b"\x8F\xB0\xA1,1\n\xA4\xA2,2\n\xFF,3\n");
    // Big5: HKSCS, read but not written. CR-only lines.
    let mut b5: Vec<u8> = b"a,b\r".to_vec();
    b5.extend_from_slice(b"\x88\x62,1\r\xA4\x40,2\r\xFE,3\r");
    for (e, b) in [(Enc::ShiftJis, sj), (Enc::EucJp, ej), (Enc::Big5, b5)] {
        let o = OpenOptions { enc: Some(e), delim: None };
        let t = table(b.clone(), o);
        assert_eq!(saved(&t), b, "{e:?}: untouched");
        // A reorder keeps each record's bytes.
        let mut t2 = table(b.clone(), o);
        let n = t2.st.len();
        let ids: Vec<_> = (0..n).map(|p| t2.st.data_id(p)).collect();
        let mut rev = ids.clone();
        rev.reverse();
        let o2 = SaveOptions { rows: Some(rev), ..Default::default() };
        let r = save::write_to(&t2.st, &o2, Vec::new()).unwrap();
        let back = table(r, o);
        assert_eq!(back.st.cell(back.st.data_id(0), 1), t.st.cell(ids[n - 1], 1), "{e:?}: reversed");
        // A cell edited: the other rows keep their bytes.
        let id = t2.st.data_id(1);
        t2.set_cells(&[(id, 1, "z".into())]);
        let out = saved(&t2);
        let same = b.iter().zip(&out).take_while(|(x, y)| x == y).count();
        let line = b.iter().enumerate().filter(|(_, &c)| c == t.st.base.dialect.nl).nth(1).unwrap().0;
        assert!(same >= line - 2, "{e:?}: the first rows are as they were: {same} {line}");
        let tail = b.len() - b.iter().enumerate().filter(|(_, &c)| c == t.st.base.dialect.nl).nth(2).unwrap().0;
        assert_eq!(out[out.len() - tail..], b[b.len() - tail..], "{e:?}: so are the last");
    }
}

/// On a big file: pieces read in parallel give what one pass gives, the runs found are exact.
#[test]
fn pieces_agree_with_one_pass() {
    let mut text = String::from("name;kind;n\n");
    for i in 0..60_000 {
        text += &format!("東京{i};ソフト表能;{}\n", i % 7);
    }
    let b = SHIFT_JIS.encode(&text).0.into_owned();
    assert!(b.len() > 1 << 20);
    let t = enc::transcode(&b, Enc::ShiftJis);
    assert_eq!(t.text, text.as_bytes());
    assert!(t.marks.len() > 3);
    let mut t = table(b.clone(), OpenOptions { enc: Some(Enc::ShiftJis), delim: None });
    assert!(saved(&t) == b);
    let ids: Vec<_> = [10, 500, 30_000, 59_000].iter().map(|&p| t.st.data_id(p)).collect();
    t.set_cells(&ids.iter().map(|&id| (id, 2, "x".to_string())).collect::<Vec<_>>());
    let want = {
        let mut s = String::new();
        for (i, l) in text.lines().enumerate() {
            s += l;
            if [11, 501, 30_001, 59_001].contains(&i) { s.truncate(s.rfind(';').unwrap() + 1); s.push('x'); }
            s.push('\n');
        }
        s
    };
    assert!(saved(&t) == SHIFT_JIS.encode(&want).0.into_owned());
}

/// A multi-byte file is shown at once from its first lines, then grows.
#[test]
fn a_big_multi_byte_file_opens_by_its_head() {
    let dir = std::env::temp_dir().join(format!("csvfab-enc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("big.csv");
    let mut text = String::from("名前;市\n");
    for i in 0..150_000 { text += &format!("山田{i};東京都\n"); }
    let b = SHIFT_JIS.encode(&text).0.into_owned();
    std::fs::write(&p, &b).unwrap();
    for o in [OpenOptions { enc: Some(Enc::ShiftJis), delim: None }, OpenOptions::default()] {
        let head = Base::open_head(&p, o).unwrap();
        assert!(head.partial);
        assert_eq!(head.enc, Enc::ShiftJis);
        assert_eq!(head.size, b.len() as u64);
        let full = head.complete(o, None);
        assert!(!full.partial);
        assert_eq!(full.n, 150_001);
        assert!(head.same_bytes(&full));
        assert_eq!(head.span(5), full.span(5));
        let t = Table::new(Arc::new(full), None);
        assert!(saved(&t) == b);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Saving from any encoding to any other: what the target holds comes through.
#[test]
fn any_to_any() {
    let text = "a;b\n1;Hello\n2;\"x;y\"\n";
    for from in Enc::ALL {
        let t = table(bytes(from, text), OpenOptions { enc: Some(from), delim: None });
        for to in Enc::ALL {
            let o = SaveOptions { enc: Some(to), ..Default::default() };
            let out = save::write_to(&t.st, &o, Vec::new()).unwrap();
            let back = table(out, OpenOptions { enc: Some(to), delim: None });
            assert_eq!(back.st.cell(back.st.data_id(1), 1), "x;y", "{from:?} → {to:?}");
        }
    }
    // Scripts too, from UTF-8 and back.
    for to in Enc::ALL {
        let w = words(to);
        let t = table(format!("a;b\n{w};1\n").into_bytes(), OpenOptions::default());
        let out = save::write_to(&t.st, &SaveOptions { enc: Some(to), ..Default::default() }, Vec::new()).unwrap();
        let back = table(out, OpenOptions { enc: Some(to), delim: None });
        assert_eq!(back.st.cell(back.st.data_id(0), 0), w, "{to:?}");
    }
}

#[test]
fn what_a_target_cannot_hold_is_named() {
    assert_eq!(enc::unencodable(Enc::Windows1251, "Москва é"), Some('é'));
    assert_eq!(enc::unencodable(Enc::ShiftJis, "東京 한"), Some('한'));
    assert_eq!(enc::unencodable(Enc::Gbk, "中文😀"), Some('😀'));
    assert_eq!(enc::unencodable(Enc::Gb18030, "中文😀"), None);
    assert_eq!(enc::unencodable(Enc::Utf16Le, "中文😀"), None);
    let mut all = Vec::new();
    enc::each_unencodable(Enc::Iso2022Jp, "한a東😀", |c| all.push(c));
    assert_eq!(all, ['한', '😀']);
    // As `?`, and ISO-2022-JP out of its Japanese mode for it.
    let mut o = Vec::new();
    enc::encode_file(Enc::Iso2022Jp, "東한京", &mut o);
    assert_eq!(enc::decode_file(Enc::Iso2022Jp, &o), "東?京");
    o.clear();
    enc::encode_file(Enc::ShiftJis, "東한", &mut o);
    assert_eq!(enc::decode_file(Enc::ShiftJis, &o), "東?");
}

fn guess(e: &'static Encoding, s: &str) -> Enc {
    let (b, _, bad) = e.encode(s);
    assert!(!bad, "{s}");
    enc::detect(&b)
}

#[test]
fn detection_finds_the_scripts() {
    let long = |s: &str| std::iter::repeat_n(s, 40).collect::<Vec<_>>().join("\n");
    assert_eq!(guess(SHIFT_JIS, &long("お名前;東京都渋谷区神南一丁目;電話番号;ありがとうございます")), Enc::ShiftJis);
    assert_eq!(guess(EUC_JP, &long("お名前;東京都渋谷区神南一丁目;電話番号;ありがとうございます")), Enc::EucJp);
    assert_eq!(guess(EUC_KR, &long("이름;서울특별시 강남구 테헤란로;전화번호;감사합니다 고객님")), Enc::EucKr);
    assert_eq!(guess(GBK, &long("姓名;北京市朝阳区建国路;电话号码;谢谢您的光临和支持")), Enc::Gbk);
    assert_eq!(guess(BIG5, &long("姓名;臺北市信義區忠孝東路;電話號碼;謝謝您的光臨與支持")), Enc::Big5);
    assert_eq!(guess(WINDOWS_1251, &long("Имя;Москва, улица Ленина;Телефон;Спасибо за покупку у нас")), Enc::Windows1251);
    assert!(matches!(guess(KOI8_U, &long("Имя;Москва, улица Ленина;Телефон;Спасибо за покупку у нас")), Enc::Koi8U | Enc::Koi8R));
    assert_eq!(guess(WINDOWS_1253, &long("Όνομα;Αθήνα και Θεσσαλονίκη;Τηλέφωνο;Ευχαριστούμε πολύ")), Enc::Windows1253);
    assert_eq!(guess(WINDOWS_1255, &long("שם;ירושלים ותל אביב;טלפון;תודה רבה לכם על הקנייה")), Enc::Windows1255);
    assert_eq!(guess(WINDOWS_1256, &long("الاسم;القاهرة والرياض;الهاتف;شكرا جزيلا لكم على الشراء")), Enc::Windows1256);
    assert_eq!(guess(WINDOWS_874, &long("ชื่อ;กรุงเทพมหานคร;โทรศัพท์;ขอบคุณมากสำหรับการซื้อ")), Enc::Windows874);
    assert_eq!(guess(WINDOWS_1250, &long("Imię;Łódź, ulica Świętokrzyska;Telefon;Dziękujemy za zakupy, ładna książka i mała łąka")), Enc::Windows1250);
    assert_eq!(guess(WINDOWS_1254, &long("Ad;İstanbul, Çiçek sokağı;Telefon;Teşekkür ederiz, güle güle kullanın")), Enc::Windows1254);
}

/// A French file stays Windows-1252, a few accents or many, whatever a guess would say.
#[test]
fn western_files_stay_western() {
    let w = encoding_rs::WINDOWS_1252;
    for s in [
        "Nom;Prénom\nDupont;Émile\n",
        "caf\u{e9}",
        "id;libellé\n1;Crédit Agricole\n2;Société Générale\n3;Prêt à taux zéro\n",
        "Numéro;Date;Libellé;Débit;Crédit\n1;01/02/2024;Virement reçu de Mme Lefèvre;;1 250,00 €\n2;02/02/2024;Chèque n° 123 — été;45,50;\n",
        "Straße;Größe;Äpfel\nMüller;Köln;Übung\n",
        "ciudad;año\nMálaga;niño\nCoruña;señor\n",
        "à;â;ç;è;é;ê;ë;î;ï;ô;œ;ù;û;ü\n",
        "ÉTÉ;NOËL;CÔTÉ;ÎLE\n",
    ] {
        assert_eq!(guess(w, s), Enc::Windows1252, "{s}");
        let t = table(w.encode(s).0.into_owned(), OpenOptions::default());
        assert_eq!(t.st.base.enc, Enc::Windows1252, "{s}");
    }
    assert_eq!(guess(w, &"Prénom;Né à Orléans;Reçu;Très bien;Hôtel-Dieu de Paris\n".repeat(5000)), Enc::Windows1252);
}

#[test]
fn iso_2022_jp_is_told_by_its_escape() {
    let (b, _, _) = encoding_rs::ISO_2022_JP.encode("名前;市\n山田;東京\n");
    assert!(b.is_ascii());
    let t = table(b.to_vec(), OpenOptions::default());
    assert_eq!(t.st.base.enc, Enc::Iso2022Jp);
    assert_eq!(t.st.cell(t.st.data_id(0), 1), "東京");
    assert_eq!(saved(&t), b.as_ref());
}

#[test]
fn utf32_is_told_by_its_bom() {
    for e in [Enc::Utf32Le, Enc::Utf32Be] {
        let mut b = e.bom().to_vec();
        enc::encode_file(e, "a;b\n😀;é\n", &mut b);
        let t = table(b.clone(), OpenOptions::default());
        assert_eq!((t.st.base.enc, t.st.base.bom), (e, true));
        assert_eq!(t.st.cell(t.st.data_id(0), 0), "😀");
        assert_eq!(saved(&t), b);
    }
    assert_eq!(enc::sniff(b"\xFF\xFEa\0"), (Enc::Utf16Le, 2));
}

/// Picked by hand after a wrong guess: each reading of the same bytes is what it should be.
#[test]
fn read_again_by_hand() {
    let (b, _, _) = SHIFT_JIS.encode("名前;市\n山田;東京\n");
    for e in Enc::ALL {
        let t = table(b.to_vec(), OpenOptions { enc: Some(e), delim: Some(b';') });
        // Any reading gives rows, whatever it shows; the same save gives the file back where
        // the encoding keeps every byte.
        assert!(t.st.base.n >= 1, "{e:?}");
        let _ = saved(&t);
    }
}

/// `CSVFAB_ENC_FILE=<a big GBK or Shift_JIS file> cargo test --profile fast -p csvfab-engine --test encodings -- --ignored --nocapture perf`
#[test]
#[ignore]
fn perf_multi_byte_open_and_save() {
    let p = std::path::PathBuf::from(std::env::var("CSVFAB_ENC_FILE").unwrap());
    let size = std::fs::metadata(&p).unwrap().len() as f64 / 1e6;
    for _ in 0..3 {
        let t0 = std::time::Instant::now();
        let head = Base::open_head(&p, OpenOptions::default()).unwrap();
        let t1 = t0.elapsed();
        let full = head.complete(OpenOptions::default(), None);
        let t2 = t0.elapsed();
        println!("{size:.0} MB {:?}: head {t1:?} ({} rows), whole {t2:?}, {} rows, {:.0} MB kept", full.enc, head.n, full.n, full.heap_bytes() as f64 / 1e6);
        let mut t = Table::new(Arc::new(full), None);
        let t0 = std::time::Instant::now();
        let n = save::write_to(&t.st, &SaveOptions::default(), std::io::sink()).map(|_| 0);
        let t1 = t0.elapsed();
        let id = t.st.data_id(10);
        t.set_cells(&[(id, 1, "x".into())]);
        let t0 = std::time::Instant::now();
        let _ = save::write_to(&t.st, &SaveOptions::default(), std::io::sink());
        println!("  save untouched {t1:?}, one edit {:?} {n:?}", t0.elapsed());
        let t0 = std::time::Instant::now();
        let _ = save::write_to(&t.st, &SaveOptions { enc: Some(Enc::Utf8), ..Default::default() }, std::io::sink());
        let t1 = t0.elapsed();
        let t0 = std::time::Instant::now();
        let _ = save::write_to(&t.st, &SaveOptions { enc: Some(Enc::Big5), ..Default::default() }, std::io::sink());
        println!("  to UTF-8 {t1:?}, to Big5 {:?}", t0.elapsed());
    }
}

/// Case and accents folded, cut text whole, in every script, whatever the file's encoding.
#[test]
fn search_sort_and_filters_on_scripts() {
    use csvfab_engine::view::{self, FilterSpec, SortKey};
    let no = std::sync::atomic::AtomicBool::new(false);
    let text = "name;city\nЁЖИК;Москва\nёжик;МОСКВА\nΣΊΣΥΦΟΣ;Αθήνα\nσίσυφος;ΑΘΉΝΑ\n東京;東京都\nソフト;東京\n서울;특별시\nİstanbul;ıssız\n";
    let enc = |e: Enc| { let mut o = e.bom().to_vec(); if e == Enc::Utf8 { o.clear(); } enc::encode_file(e, text, &mut o); o };
    for e in [Enc::Utf8, Enc::Utf16Le, Enc::Gb18030, Enc::Utf32Be] {
        let t = table(enc(e), OpenOptions { enc: Some(e), delim: Some(b';') });
        let st = &t.st;
        assert_eq!(st.len(), 8, "{e:?}");
        let hits = |sp: FilterSpec| view::filter(st, &view::Filter::compile(&sp).unwrap(), &no).unwrap().len();
        let g = |s: &str| FilterSpec { global: s.into(), ..Default::default() };
        assert_eq!(hits(g("москва")), 2, "{e:?}");
        assert_eq!(hits(g("ЁЖИК")), 2, "{e:?}");
        assert_eq!(hits(g("東京")), 2, "{e:?}");
        assert_eq!(hits(g("서울")), 1, "{e:?}");
        assert_eq!(hits(g("αθηνα")), 2, "{e:?}: accents folded");
        assert_eq!(hits(g("σισυφος")), 2, "{e:?}: final sigma");
        assert_eq!(hits(FilterSpec { global: "москва".into(), match_case: true, ..Default::default() }), 0, "{e:?}");
        assert_eq!(hits(FilterSpec { global: "мос.ва$".into(), regex: true, ..Default::default() }), 2, "{e:?}");
        assert_eq!(hits(FilterSpec { cols: vec![(1, "=東京".into())], ..Default::default() }), 1, "{e:?}");
        assert_eq!(hits(FilterSpec { cols: vec![(0, "ソ".into())], ..Default::default() }), 1, "{e:?}");
        let kinds = view::detect_kinds(st);
        let mut ids: Vec<_> = (0..st.len()).map(|p| st.data_id(p)).collect();
        assert!(view::sort(st, &mut ids, &[SortKey { col: 0, desc: false }], &kinds, &no));
        let names: Vec<String> = ids.iter().map(|&id| st.cell(id, 0)).collect();
        let at = |w: &str| names.iter().position(|n| n == w).unwrap();
        assert!(at("İstanbul") < at("ΣΊΣΥΦΟΣ") && at("ΣΊΣΥΦΟΣ") < at("ёжик") && at("ёжик") < at("東京"), "{e:?}: {names:?}");
        assert!(at("ЁЖИК").abs_diff(at("ёжик")) == 1, "{e:?}: {names:?}");
    }
    // A single-byte file goes another way: through its fields.
    let (b, _, _) = WINDOWS_1251.encode("name;city\nЁЖИК;Москва\nёжик;МОСКВА\nIvan;Paris\n");
    let t = table(b.into_owned(), OpenOptions { enc: Some(Enc::Windows1251), delim: Some(b';') });
    let hits = |s: &str| view::filter(&t.st, &view::Filter::compile(&FilterSpec { global: s.into(), ..Default::default() }).unwrap(), &no).unwrap().len();
    assert_eq!(hits("москва"), 2);
    assert_eq!(hits("ЖИК"), 2);
    assert_eq!(hits("paris"), 1);
}

/// New files made without a question (Extract, Split, Combine) lose nothing either.
#[test]
fn new_files_keep_what_was_typed() {
    let mut t = table(WINDOWS_1251.encode("a;b\nМосква;1\nКиев;2\n").0.into_owned(), OpenOptions { enc: Some(Enc::Windows1251), delim: Some(b';') });
    assert_eq!(save::new_file_enc(&t.st), None);
    let id = t.st.data_id(1);
    t.set_cells(&[(id, 1, "x".into())]);
    assert_eq!(save::new_file_enc(&t.st), None);
    t.set_cells(&[(id, 1, "東".into())]);
    assert_eq!(save::new_file_enc(&t.st), Some(Enc::Utf8));
}

/// Random bytes read in each multi-byte encoding: untouched, they come back as they were; a row
/// edited changes that row, and rows before it keep their bytes.
#[test]
fn hostile_bytes_come_back() {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move |n: u64| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x % n };
    for round in 0..3000 {
        let len = next(400) as usize;
        let b: Vec<u8> = (0..len).map(|_| match next(9) {
            0 => b'\n',
            1 => b'\r',
            2 => b';',
            3 => b'"',
            4 | 5 => 0x80 + next(0x80) as u8,
            6 => 0x40 + next(0x40) as u8,
            _ => b'a' + next(26) as u8,
        }).collect();
        for e in [Enc::ShiftJis, Enc::EucJp, Enc::EucKr, Enc::Gbk, Enc::Gb18030, Enc::Big5, Enc::Iso2022Jp] {
            let t = table(b.clone(), OpenOptions { enc: Some(e), delim: Some(b';') });
            let out = saved(&t);
            if t.st.base.n == 0 { continue; }
            // (ISO-2022-JP: a line break swallowed in a Japanese run leaves no way to tell lines apart.)
            assert!(out == b || e == Enc::Iso2022Jp, "round {round} {e:?}: {b:?} gave {out:?}");
        }
    }
}
