//! `.z64` files opened and saved as they are: what the rest of the engine sees is the content,
//! and what lands on the disk is a z64 file holding exactly the bytes a plain save would write.

use csvfab_engine::base::{Base, OpenOptions};
use csvfab_engine::save::{self, SaveError, SaveOptions};
use csvfab_engine::table::Table;
use csvfab_engine::{combine, convert, journal, split, tail, Enc};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("csvfab-z64-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pack(b: &[u8]) -> Vec<u8> {
    z64::compress(b, z64::Options::default()).unwrap()
}

fn unpacked(p: &Path) -> Vec<u8> {
    z64::decompress(&std::fs::read(p).unwrap(), 0).unwrap()
}

fn open(p: &Path) -> Table {
    Table::new(Arc::new(Base::open(p, OpenOptions::default(), None).unwrap()), None)
}

fn body(rows: usize) -> Vec<u8> {
    let mut b = b"id;name;note\r\n".to_vec();
    for i in 0..rows { b.extend_from_slice(format!("{i};name {i};\"a \"\"quoted\"\" {}\"\r\n", i * 31).as_bytes()); }
    b
}

fn names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn opens_with_its_content() {
    let d = dir("open");
    let p = d.join("t.csv.z64");
    let b = body(5000);
    std::fs::write(&p, pack(&b)).unwrap();
    let t = open(&p);
    let base = &t.st.base;
    assert_eq!(base.bytes(), &b[..]);
    assert_eq!(base.size, b.len() as u64);
    assert_eq!(base.dialect.delim, b';');
    assert_eq!(base.eol, b"\r\n");
    assert_eq!(t.st.cell(t.st.data_id(4999), 1), "name 4999");
    assert!(base.is_packed() && base.file_mapped() && !base.mapped() && !base.unfinished());
    assert_eq!(names(&d), ["t.csv.z64"], "nothing unpacked beside it");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn save_in_place_is_exact() {
    let d = dir("save");
    let p = d.join("t.csv.z64");
    let b = body(100_000);
    std::fs::write(&p, pack(&b)).unwrap();
    let mut t = open(&p);
    let id = t.st.data_id(0);
    t.set_cells(&[(id, 1, "edited".into())]);
    let expect = t.st.base.stamp.map(Into::into);
    let mut backed = false;
    let (_, again) = save::save_indexed(&t.st, &p, &SaveOptions::default(), expect, &mut backed).unwrap();
    let want = String::from_utf8(b.clone()).unwrap().replacen("0;name 0;", "0;edited;", 1).into_bytes();
    assert_eq!(unpacked(&p), want);
    assert!(std::fs::metadata(&p).unwrap().len() < want.len() as u64 / 2, "it is compressed");
    let again = again.expect("the index comes from the save");
    assert!(again.is_packed());
    assert_eq!(again.bytes(), &want[..]);
    assert_eq!(again.size, want.len() as u64);
    let t2 = Table::new(Arc::new(again), None);
    assert_eq!(t2.st.cell(t2.st.data_id(0), 1), "edited");
    assert_eq!(t2.st.cell(t2.st.data_id(99_999), 1), "name 99999");
    let left = names(&d);
    assert_eq!(left.len(), 2, "{left:?}");
    let bak = d.join(left.iter().find(|n| n.ends_with(".bak")).unwrap());
    assert_eq!(unpacked(&bak), b, "the original, kept");
    let _ = std::fs::remove_dir_all(&d);
}

/// Whatever the target's name asks: to z64, from z64, in another encoding. From a z64 to a plain
/// file is where the kernel must not copy: a plain save hands it the long runs of untouched rows
/// from the file read, and that file is compressed here.
#[test]
fn save_as_goes_by_the_name() {
    let d = dir("as");
    let (csv, z, back) = (d.join("a.csv"), d.join("a.csv.z64"), d.join("b.csv"));
    let b = body(100_000);
    std::fs::write(&csv, &b).unwrap();
    let t = open(&csv);
    save::save(&t.st, &z, &SaveOptions::default(), None, &mut true).unwrap();
    assert_eq!(unpacked(&z), b);
    assert_ne!(std::fs::read(&z).unwrap(), b);
    let t = open(&z);
    save::save(&t.st, &back, &SaveOptions::default(), None, &mut true).unwrap();
    assert_eq!(std::fs::read(&back).unwrap(), b, "from a .z64: the content, not its compressed bytes");
    let latin = d.join("c.csv.z64");
    let o = SaveOptions { enc: Some(Enc::Windows1252), bom: Some(false), ..Default::default() };
    let src = "n\nZoé\n";
    std::fs::write(&csv, src).unwrap();
    let t = open(&csv);
    save::save(&t.st, &latin, &o, None, &mut true).unwrap();
    assert_eq!(unpacked(&latin), b"n\nZo\xE9\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_subset_written_to_z64_holds_those_rows() {
    let d = dir("rows");
    let p = d.join("a.csv");
    std::fs::write(&p, "a,b\n1,2\n3,4\n5,6\n").unwrap();
    let t = open(&p);
    let (first, last) = (t.st.data_id(0), t.st.data_id(2));
    let z = d.join("some.csv.z64");
    let o = SaveOptions { rows: Some(vec![last, first]), ..Default::default() };
    save::save(&t.st, &z, &o, None, &mut true).unwrap();
    assert_eq!(unpacked(&z), b"a,b\n5,6\n1,2\n");
    let _ = std::fs::remove_dir_all(&d);
}

/// Its first rows from its first blocks, the whole after; edits made meanwhile are kept by the app
/// because the two count as the same bytes.
#[test]
fn head_then_whole() {
    let d = dir("head");
    let p = d.join("t.csv.z64");
    let b = body(200_000);
    std::fs::write(&p, pack(&b)).unwrap();
    let head = Base::open_head(&p, OpenOptions::default()).unwrap();
    assert!(head.partial && head.is_packed());
    assert!(head.n > 100 && head.n < 200_000);
    assert_eq!(head.size, b.len() as u64);
    assert_eq!(&b[..head.bytes().len()], head.bytes());
    let whole = head.complete(OpenOptions::default(), None).unwrap();
    assert!(!whole.partial);
    assert_eq!(whole.n, 200_001);
    assert_eq!(whole.bytes(), &b[..]);
    assert!(head.same_bytes(&whole));
    assert!(whole.sample().is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_small_file_has_no_head() {
    let d = dir("small");
    let p = d.join("t.csv.z64");
    std::fs::write(&p, pack(b"a,b\n1,2\n")).unwrap();
    let b = Base::open_head(&p, OpenOptions::default()).unwrap();
    assert!(!b.partial && b.is_packed());
    assert_eq!(b.n, 2);
    let _ = std::fs::remove_dir_all(&d);
}

/// Encodings that cut the head differently: read through UTF-8, or read whole.
#[test]
fn head_of_other_encodings() {
    let d = dir("enc");
    let text: String = std::iter::once("a;b\n".to_string()).chain((0..100_000).map(|i| format!("{i};東京 é {i}\n"))).collect();
    let mut utf16 = vec![0xFF, 0xFE];
    csvfab_engine::enc::utf8_to_utf16(&text, false, &mut utf16);
    let mut sjis = Vec::new();
    csvfab_engine::enc::encode_into(Enc::ShiftJis, &text.replace('é', "e"), &mut sjis);
    for (name, bytes, enc) in [("u16", utf16, None), ("sjis", sjis, Some(Enc::ShiftJis))] {
        let p = d.join(format!("{name}.csv.z64"));
        std::fs::write(&p, pack(&bytes)).unwrap();
        let o = OpenOptions { enc, delim: None };
        let head = Base::open_head(&p, o).unwrap();
        let whole = if head.partial { head.complete(o, None).unwrap() } else { head };
        assert_eq!(whole.n, 100_001, "{name}");
        let t = Table::new(Arc::new(whole), None);
        assert!(t.st.cell(t.st.data_id(99_999), 1).ends_with(" 99999"), "{name}");
        assert_eq!(t.st.cell(t.st.data_id(7), 0), "7", "{name}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The file another program left behind is not ours to replace; our own file, only rewritten by a
/// sync client (a new stamp, the same bytes), is.
#[test]
fn a_change_on_disk_is_seen() {
    let d = dir("moved");
    let p = d.join("t.csv.z64");
    let b = body(100);
    std::fs::write(&p, pack(&b)).unwrap();
    let mut t = open(&p);
    let id = t.st.data_id(0);
    t.set_cells(&[(id, 1, "edited".into())]);
    let expect = t.st.base.stamp.map(Into::into);
    // Rewritten, the same bytes: a new inode, a new time.
    let same = std::fs::read(&p).unwrap();
    let aside = d.join("aside");
    std::fs::write(&aside, &same).unwrap();
    std::fs::rename(&aside, &p).unwrap();
    let r = save::save(&t.st, &p, &SaveOptions { backup: false, ..Default::default() }, expect, &mut true);
    assert!(r.is_ok(), "the same content: still ours");
    let on_disk = std::fs::read(&p).unwrap();
    let mut u = open(&p);
    let id = u.st.data_id(0);
    u.set_cells(&[(id, 1, "again".into())]);
    let expect = u.st.base.stamp.map(Into::into);
    std::fs::write(&p, pack(b"theirs\n")).unwrap();
    let r = save::save(&u.st, &p, &SaveOptions { backup: false, ..Default::default() }, expect, &mut true);
    assert!(matches!(r, Err(SaveError::Changed)));
    assert_ne!(std::fs::read(&p).unwrap(), on_disk);
    assert_eq!(unpacked(&p), b"theirs\n");
    let _ = std::fs::remove_dir_all(&d);
}

/// A writer that stopped before its table: every complete block shows, and it is said.
#[test]
fn a_file_cut_short_is_read_and_told() {
    let d = dir("cut");
    let p = d.join("t.csv.z64");
    let b = body(100_000);
    let good = pack(&b);
    std::fs::write(&p, &good[..good.len() - good.len() / 3]).unwrap();
    let t = open(&p);
    let base = &t.st.base;
    assert!(base.unfinished());
    assert!(!base.bytes().is_empty() && base.bytes().len() < b.len());
    assert_eq!(base.bytes(), &b[..base.bytes().len()]);
    let held = base.bytes().to_vec();
    save::save(&t.st, &p, &SaveOptions::default(), None, &mut true).unwrap();
    assert_eq!(unpacked(&p), held, "saved, the file is complete");
    assert!(!open(&p).st.base.unfinished());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn damage_is_an_error() {
    let d = dir("bad");
    let p = d.join("t.csv.z64");
    let mut bytes = pack(&body(100_000));
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x55;
    std::fs::write(&p, &bytes).unwrap();
    assert!(Base::open(&p, OpenOptions::default(), None).is_err(), "a damaged block");
    let head = Base::open_head(&p, OpenOptions::default()).unwrap();
    assert!(head.partial, "its first blocks are fine");
    assert!(head.complete(OpenOptions::default(), None).is_err(), "the rest is not");
    std::fs::write(&p, b"id;name\n1;not z64 at all\n").unwrap();
    assert!(Base::open(&p, OpenOptions::default(), None).is_err(), "another format with that name");
    assert!(Base::open_head(&p, OpenOptions::default()).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

/// Nearly nothing on disk, far too much once unpacked.
#[test]
fn a_bomb_is_refused() {
    let d = dir("bomb");
    let p = d.join("t.csv.z64");
    std::fs::write(&p, pack(&vec![b'a'; 300 << 20])).unwrap();
    let e = Base::open(&p, OpenOptions::default(), None).err().unwrap();
    assert!(e.to_string().contains("booby-trapped"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn empty_and_joined_files() {
    let d = dir("edge");
    let p = d.join("t.csv.z64");
    std::fs::write(&p, pack(b"")).unwrap();
    let t = open(&p);
    assert_eq!(t.st.base.n, 0);
    assert!(!t.st.base.unfinished());
    // `cat a.z64 b.z64`.
    let mut both = pack(b"a,b\n1,2\n");
    both.extend_from_slice(&pack(b"3,4\n"));
    std::fs::write(&p, both).unwrap();
    let t = open(&p);
    assert_eq!(t.st.base.bytes(), b"a,b\n1,2\n3,4\n");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn another_encoding_or_delimiter_reads_the_same_bytes() {
    let d = dir("reopen");
    let p = d.join("t.csv.z64");
    std::fs::write(&p, pack(b"a,b;c\n1,2;3\n")).unwrap();
    let b = Base::open(&p, OpenOptions::default(), None).unwrap();
    let again = b.reopen(OpenOptions { enc: None, delim: Some(b';') }, None).expect("nothing to read");
    assert!(again.is_packed());
    assert_eq!(again.dialect.delim, b';');
    assert_eq!(again.size, b.size);
    let _ = std::fs::remove_dir_all(&d);
}

/// Crash recovery stands on the file: read again, the edits land on the same rows.
#[test]
fn a_journal_replays_over_it() {
    let d = dir("journal");
    let p = d.join("t.csv.z64");
    std::fs::write(&p, pack(&body(2000))).unwrap();
    let mut t = open(&p);
    let id = t.st.data_id(1500);
    t.set_cells(&[(id, 1, "edited".into())]);
    let mut j = Vec::new();
    journal::write(&t.st, OpenOptions::default(), &mut j).unwrap();
    let b = Arc::new(Base::open(&p, OpenOptions::default(), None).unwrap());
    let back = journal::replay(b, &j).unwrap();
    assert_eq!(back.st.cell(back.st.data_id(1500), 1), "edited");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_compressed_log_is_not_followed() {
    let d = dir("tail");
    let p = d.join("app.log.z64");
    std::fs::write(&p, pack(b"one\ntwo\n")).unwrap();
    let b = Base::open(&p, OpenOptions { enc: None, delim: Some(0) }, None).unwrap();
    assert_eq!(tail::cannot_follow(&b), Some("A compressed file cannot be followed."));
    assert!(b.append().unwrap().is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn names_see_through_the_suffix() {
    assert_eq!(split::stem_of("ventes.csv.z64"), "ventes");
    assert_eq!(split::stem_of("ventes.z64"), "ventes");
    assert_eq!(split::csv_ext("ventes.tsv.z64"), ".tsv");
    assert_eq!(convert::export_name("ventes.csv.z64", convert::Format::Parquet), "ventes.parquet");
    assert_eq!(convert::export_sheet_name("ventes.csv.z64"), "ventes");
    assert_eq!(csvfab_engine::schema::path_beside(Path::new("/x/ventes.csv.z64")), Path::new("/x/ventes.schema.json"));
    assert_eq!(convert::import_format(Path::new("a.csv.z64")), None);
    assert_eq!(convert::import_format(Path::new("a.z64")), None);
    assert_eq!(convert::import_format(Path::new("a.json.Z64")), Some(convert::Format::Packed));
    assert_eq!(convert::import_format(Path::new("a.xlsx.z64")), Some(convert::Format::Packed));
}

/// What a text view cannot show goes through its import, as a `.json.gz` does.
#[test]
fn a_z64_of_json_is_unpacked_beside_it() {
    let d = dir("json");
    let p = d.join("a.json.z64");
    let json = br#"[{"a":1},{"a":2}]"#;
    std::fs::write(&p, pack(json)).unwrap();
    let dest = convert::dest_for(convert::Format::Packed, &p, None);
    assert_eq!(dest, d.join("a.json"));
    convert::import(convert::Format::Packed, &p, None, b',', &dest, false, None, &AtomicBool::new(false)).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), json);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn combine_writes_z64_by_the_name() {
    let d = dir("combine");
    let (a, b) = (d.join("a.csv.z64"), d.join("b.csv"));
    std::fs::write(&a, pack(b"k,v\n1,2\n")).unwrap();
    std::fs::write(&b, "k,v\n3,4\n").unwrap();
    let sources = vec![combine::Source::open(&a).unwrap(), combine::Source::open(&b).unwrap()];
    let m = combine::columns(&sources, false, false);
    let f = combine::format(&sources);
    let out = d.join("all.csv.z64");
    combine::write(&sources, &m, false, &f, &out, &AtomicBool::new(false), None).unwrap();
    assert_eq!(unpacked(&out), b"k,v\n1,2\n3,4\n");
    let _ = std::fs::remove_dir_all(&d);
}
