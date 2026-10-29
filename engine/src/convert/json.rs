//! JSON → CSV, as the page flattened it (jsonRecords, 27-import.js): an array
//! of objects — or the first array of objects found in an object, or one
//! object per line (JSON Lines) — gives one column per key, nested keys joined
//! with a dot, arrays kept as JSON text. An array of arrays is rows already.
//!
//! Streamed: the page parsed the whole file into objects; here the file is
//! mapped and parsed once per pass (shape, columns, rows), one record in memory
//! at a time. JavaScript's semantics are kept where they show: numbers are
//! doubles written as String(n) writes them, a key repeated in an object keeps
//! its first place and its last value, and integer-like keys come first in
//! ascending order, as Object.keys() lists them.

use super::{check, io_err, js_number, tick, CsvOut, Imported};
use serde::Deserialize as _;
use serde::de::{self, DeserializeSeed, Deserializer as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// A JSON value as JavaScript holds it.
#[derive(Debug, PartialEq)]
enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Val>),
    /// Entries in insertion order, keys unique.
    Obj(Vec<(String, Val)>),
}

impl<'de> de::Deserialize<'de> for Val {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Val, D::Error> {
        d.deserialize_any(ValVisitor)
    }
}

struct ValVisitor;

impl<'de> Visitor<'de> for ValVisitor {
    type Value = Val;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<Val, E> { Ok(Val::Null) }
    fn visit_bool<E>(self, b: bool) -> Result<Val, E> { Ok(Val::Bool(b)) }
    fn visit_i64<E>(self, n: i64) -> Result<Val, E> { Ok(Val::Num(n as f64)) }
    fn visit_u64<E>(self, n: u64) -> Result<Val, E> { Ok(Val::Num(n as f64)) }
    fn visit_f64<E>(self, n: f64) -> Result<Val, E> { Ok(Val::Num(n)) }
    fn visit_str<E>(self, s: &str) -> Result<Val, E> { Ok(Val::Str(s.to_string())) }
    fn visit_string<E>(self, s: String) -> Result<Val, E> { Ok(Val::Str(s)) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Val, A::Error> {
        let mut v = Vec::with_capacity(s.size_hint().unwrap_or(0));
        while let Some(x) = s.next_element()? { v.push(x); }
        Ok(Val::Arr(v))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Val, A::Error> {
        let mut entries: Vec<(String, Val)> = Vec::new();
        // A lookup only for big objects: most records have a handful of keys.
        let mut index: Option<HashMap<String, usize>> = None;
        while let Some(k) = m.next_key::<String>()? {
            let v: Val = m.next_value()?;
            let at = match &index {
                Some(ix) => ix.get(&k).copied(),
                None => entries.iter().position(|(x, _)| *x == k),
            };
            match at {
                Some(p) => entries[p].1 = v,
                None => {
                    if let Some(ix) = &mut index { ix.insert(k.clone(), entries.len()); }
                    entries.push((k, v));
                    if index.is_none() && entries.len() > 32 {
                        index = Some(entries.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)).collect());
                    }
                }
            }
        }
        Ok(Val::Obj(entries))
    }
}

/// An array index as JavaScript sees one: "0", "7", "42" — not "07", not "-1".
fn array_index(k: &str) -> Option<u32> {
    let b = k.as_bytes();
    if b.is_empty() || b.len() > 10 || (b[0] == b'0' && b.len() > 1) || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    k.parse::<u64>().ok().filter(|&n| n < u32::MAX as u64).map(|n| n as u32)
}

/// Object.keys() order: integer-like keys ascending, then the others as inserted.
fn js_order<T>(items: &[T], key: impl Fn(&T) -> &str) -> Vec<usize> {
    let mut ix: Vec<usize> = (0..items.len()).collect();
    if items.iter().any(|x| array_index(key(x)).is_some()) {
        ix.sort_by_key(|&i| array_index(key(&items[i])).map_or((1, 0), |n| (0, n)));
    }
    ix
}

/// The page's flat(): `f(key, value)` for each leaf, nested objects' keys joined with a dot.
fn flat<'a>(o: &'a [(String, Val)], key: &mut String, f: &mut impl FnMut(&str, &'a Val)) {
    for i in js_order(o, |e| &e.0) {
        let (k, v) = &o[i];
        let at = key.len();
        if at > 0 { key.push('.'); }
        key.push_str(k);
        match v {
            Val::Obj(inner) => flat(inner, key, f),
            _ => f(key, v),
        }
        key.truncate(at);
    }
}

/// A record's leaves, as the page's `out` object holds them: a non-object is `{value: item}`.
fn leaves<'a>(item: &'a Val, key: &mut String, f: &mut impl FnMut(&str, &'a Val)) {
    match item {
        Val::Obj(o) => flat(o, key, f),
        v => f("value", v),
    }
}

/// JSON.stringify(v).
fn stringify(v: &Val, out: &mut String) {
    match v {
        Val::Null => out.push_str("null"),
        Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Val::Num(x) if x.is_finite() => js_number(*x, out),
        Val::Num(_) => out.push_str("null"),
        Val::Str(s) => quote(s, out),
        Val::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 { out.push(','); }
                stringify(x, out);
            }
            out.push(']');
        }
        Val::Obj(o) => {
            out.push('{');
            for (n, i) in js_order(o, |e| &e.0).into_iter().enumerate() {
                if n > 0 { out.push(','); }
                quote(&o[i].0, out);
                out.push(':');
                stringify(&o[i].1, out);
            }
            out.push('}');
        }
    }
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => { use fmt::Write; let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// The page's jsonCell(): a cell's text.
fn cell<'a>(v: &'a Val, comma: bool, buf: &'a mut String) -> &'a str {
    buf.clear();
    match v {
        Val::Null => "",
        Val::Bool(b) => if *b { "true" } else { "false" },
        Val::Str(s) => s,
        Val::Num(x) => {
            js_number(*x, buf);
            if comma {
                if let Some(k) = buf.find('.') { buf.replace_range(k..k + 1, ","); }
            }
            buf
        }
        Val::Arr(_) | Val::Obj(_) => {
            stringify(v, buf);
            buf
        }
    }
}

// --- reading the file, pass by pass ----------------------------------------------

/// Where the records are.
#[derive(Debug, PartialEq)]
enum Shape {
    /// The top-level array's items.
    Array,
    /// The items of this key's array (its last occurrence: the value JSON.parse keeps).
    Key(String, usize),
    /// The top-level object, as the only record.
    One,
    /// One record per non-blank line.
    Lines,
}

/// Whether a value is what `typeof x === 'object'` says: an object, an array or null.
struct IsObject;

impl<'de> DeserializeSeed<'de> for IsObject {
    type Value = bool;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<bool, D::Error> {
        d.deserialize_any(Probe { first: false })
    }
}

/// `first`: for an array, whether it is non-empty with an object first (a key
/// worth taking); otherwise whether the value is an object (IsObject).
struct Probe {
    first: bool,
}

impl<'de> Visitor<'de> for Probe {
    type Value = bool;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<bool, E> { Ok(!self.first) }
    fn visit_bool<E>(self, _: bool) -> Result<bool, E> { Ok(false) }
    fn visit_i64<E>(self, _: i64) -> Result<bool, E> { Ok(false) }
    fn visit_u64<E>(self, _: u64) -> Result<bool, E> { Ok(false) }
    fn visit_f64<E>(self, _: f64) -> Result<bool, E> { Ok(false) }
    fn visit_str<E>(self, _: &str) -> Result<bool, E> { Ok(false) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<bool, A::Error> {
        let r = if self.first { s.next_element_seed(IsObject)?.unwrap_or(false) } else { true };
        while s.next_element::<IgnoredAny>()?.is_some() {}
        Ok(r)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<bool, A::Error> {
        while m.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(!self.first)
    }
}

impl<'de> DeserializeSeed<'de> for Probe {
    type Value = bool;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<bool, D::Error> {
        d.deserialize_any(self)
    }
}

/// Pass 1: the shape, from the top-level value (None: a single value, not records).
struct ShapeOf;

impl<'de> Visitor<'de> for ShapeOf {
    type Value = Option<Shape>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<Option<Shape>, E> { Ok(None) }
    fn visit_bool<E>(self, _: bool) -> Result<Option<Shape>, E> { Ok(None) }
    fn visit_i64<E>(self, _: i64) -> Result<Option<Shape>, E> { Ok(None) }
    fn visit_u64<E>(self, _: u64) -> Result<Option<Shape>, E> { Ok(None) }
    fn visit_f64<E>(self, _: f64) -> Result<Option<Shape>, E> { Ok(None) }
    fn visit_str<E>(self, _: &str) -> Result<Option<Shape>, E> { Ok(None) }
    fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Option<Shape>, A::Error> {
        while s.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Some(Shape::Array))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Option<Shape>, A::Error> {
        // Per key: (name, its last value qualifies, how many times it appeared).
        let mut keys: Vec<(String, bool, usize)> = Vec::new();
        let mut ix: HashMap<String, usize> = HashMap::new();
        while let Some(k) = m.next_key::<String>()? {
            let ok = m.next_value_seed(Probe { first: true })?;
            match ix.get(&k) {
                Some(&i) => { keys[i].1 = ok; keys[i].2 += 1; }
                None => { ix.insert(k.clone(), keys.len()); keys.push((k, ok, 1)); }
            }
        }
        Ok(Some(match js_order(&keys, |e| &e.0).into_iter().find(|&i| keys[i].1) {
            Some(i) => { let (k, _, n) = std::mem::take(&mut keys[i]); Shape::Key(k, n - 1) }
            None => Shape::One,
        }))
    }
}

impl<'de> DeserializeSeed<'de> for ShapeOf {
    type Value = Option<Shape>;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Option<Shape>, D::Error> {
        d.deserialize_any(self)
    }
}

/// Feeds each item of an array to `f`; an error of f's is kept in `err`.
struct Items<'f, F> {
    f: &'f mut F,
    err: &'f mut Option<String>,
}

impl<'de, F: FnMut(Val) -> Result<(), String>> Visitor<'de> for Items<'_, F> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<(), A::Error> {
        while let Some(v) = s.next_element::<Val>()? {
            if let Err(e) = (self.f)(v) {
                *self.err = Some(e);
                return Err(de::Error::custom("stopped"));
            }
        }
        Ok(())
    }
}

impl<'de, F: FnMut(Val) -> Result<(), String>> DeserializeSeed<'de> for Items<'_, F> {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

/// Feeds the items of the `n`-th occurrence of `key`'s array to `f`.
struct KeyItems<'f, F> {
    key: &'f str,
    n: usize,
    items: Items<'f, F>,
}

impl<'de, F: FnMut(Val) -> Result<(), String>> Visitor<'de> for KeyItems<'_, F> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<(), A::Error> {
        let mut items = Some(self.items);
        let mut seen = 0;
        while let Some(k) = m.next_key::<String>()? {
            if k == self.key {
                seen += 1;
                if seen == self.n + 1 {
                    if let Some(it) = items.take() {
                        m.next_value_seed(it)?;
                        continue;
                    }
                }
            }
            m.next_value::<IgnoredAny>()?;
        }
        Ok(())
    }
}

fn parse_err(e: serde_json::Error) -> String {
    format!("not JSON: {e}")
}

/// Non-blank lines, as `text.split(/\r?\n/).filter(l => l.trim())`.
fn lines(b: &[u8]) -> impl Iterator<Item = &[u8]> {
    b.split(|&c| c == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| std::str::from_utf8(l).map_or(true, |s| !s.trim().is_empty()))
}

/// Pass 1: where the records are, or the page's error.
fn shape(b: &[u8]) -> Result<Shape, String> {
    let mut de = serde_json::Deserializer::from_slice(b);
    match ShapeOf.deserialize(&mut de).and_then(|s| de.end().map(|_| s)) {
        Ok(Some(s)) => Ok(s),
        Ok(None) => Err("the file holds a single value, not records".into()),
        Err(whole) => {
            // JSON Lines: one record per line, every line valid.
            for l in lines(b) {
                let mut de = serde_json::Deserializer::from_slice(l);
                if IgnoredAny::deserialize(&mut de).and_then(|_| de.end()).is_err() {
                    return Err(parse_err(whole));
                }
            }
            Ok(Shape::Lines)
        }
    }
}

/// Each record of the file, in order, to `f`.
fn each(b: &[u8], sh: &Shape, mut f: impl FnMut(Val) -> Result<(), String>) -> Result<(), String> {
    let mut err = None;
    let mut de = serde_json::Deserializer::from_slice(b);
    let res = match sh {
        Shape::Array => Items { f: &mut f, err: &mut err }.deserialize(&mut de),
        Shape::Key(k, n) => (&mut de).deserialize_map(KeyItems { key: k, n: *n, items: Items { f: &mut f, err: &mut err } }),
        Shape::One => return Val::deserialize(&mut de).map_err(parse_err).and_then(f),
        Shape::Lines => {
            for l in lines(b) {
                f(serde_json::from_slice(l).map_err(parse_err)?)?;
            }
            return Ok(());
        }
    };
    match (err, res) {
        (Some(e), _) => Err(e),
        (None, r) => r.map_err(parse_err),
    }
}

/// The JSON file at `path` as CSV into `out`. Progress: records done, twice over
/// (columns, then rows).
pub fn to_csv<W: Write>(path: &Path, delim: u8, out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let f = std::fs::File::open(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "file not found".into() } else { e.to_string() })?;
    let len = f.metadata().map_err(io_err)?.len();
    let map = if len > 0 { Some(crate::mapguard::Guarded::map(&f).map_err(io_err)?) } else { None };
    let r = convert(map.as_deref().unwrap_or(&[]), delim, out, progress, cancel);
    if map.as_ref().is_some_and(|m| m.cut_short()) {
        return Err("another program cut the file short while it was read".into());
    }
    r
}

/// JSON text → CSV; see [`to_csv`].
pub fn convert<W: Write>(bytes: &[u8], delim: u8, out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    // The page decoded the file as UTF-8, invalid bytes becoming U+FFFD.
    let owned;
    let b = if std::str::from_utf8(b).is_ok() { b } else {
        owned = String::from_utf8_lossy(b).into_owned();
        owned.as_bytes()
    };
    let sh = shape(b)?;

    // Pass 2: the columns, in order of first appearance; whether every record is an array.
    let mut cols: Vec<String> = Vec::new();
    let mut col_ix: HashMap<String, usize> = HashMap::new();
    let (mut all_arrays, mut widest, mut n) = (true, 0usize, 0usize);
    let mut key = String::new();
    let mut rec_keys: Vec<String> = Vec::new();
    each(b, &sh, |item| {
        n += 1;
        if n & 1023 == 0 {
            check(cancel)?;
            tick(progress, 1024);
        }
        match &item {
            Val::Arr(a) => widest = widest.max(a.len()),
            _ => all_arrays = false,
        }
        // The page's `out` is an object too: its integer-like keys (top-level ones)
        // come first. Rare: only then are the record's keys gathered and sorted.
        let reorder = matches!(&item, Val::Obj(o) if o.iter().any(|(k, _)| array_index(k).is_some()));
        if reorder {
            rec_keys.clear();
            leaves(&item, &mut key, &mut |k, _| if !rec_keys.iter().any(|x| x == k) { rec_keys.push(k.to_string()) });
            for i in js_order(&rec_keys, |k| k) {
                let k = &rec_keys[i];
                if !col_ix.contains_key(k) { col_ix.insert(k.clone(), cols.len()); cols.push(k.clone()); }
            }
        } else {
            leaves(&item, &mut key, &mut |k, _| if !col_ix.contains_key(k) { col_ix.insert(k.to_string(), cols.len()); cols.push(k.to_string()); });
        }
        Ok(())
    })?;
    tick(progress, n % 1024);

    // Pass 3: the rows.
    let comma = delim == b';';
    let mut csv = CsvOut::new(out, delim, false);
    let mut buf = String::new();
    if !all_arrays {
        for c in &cols { csv.field(c).map_err(io_err)?; }
        csv.end().map_err(io_err)?;
    } else if n == 0 {
        csv.end().map_err(io_err)?; // the page wrote "" + "\n"
    }
    let mut k = 0usize;
    each(b, &sh, |item| {
        k += 1;
        if k & 1023 == 0 {
            check(cancel)?;
            tick(progress, 1024);
        }
        if all_arrays {
            if let Val::Arr(a) = &item {
                for v in a { csv.field(cell(v, comma, &mut buf)).map_err(io_err)?; }
            }
        } else {
            // A repeated assignment keeps the last value, as `out[key] = v` did.
            let mut s: Vec<Option<&Val>> = vec![None; cols.len()];
            leaves(&item, &mut key, &mut |name, v| if let Some(&i) = col_ix.get(name) { s[i] = Some(v) });
            for v in &s { csv.field(v.map_or("", |v| cell(v, comma, &mut buf))).map_err(io_err)?; }
        }
        csv.end().map_err(io_err)
    })?;
    tick(progress, k % 1024);
    csv.w.flush().map_err(io_err)?;
    Ok(Imported { sheet: None, others: 0, rows: n as u64, cols: if all_arrays { widest } else { cols.len() }, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(s: &str, delim: u8) -> Result<(String, u64, usize), String> {
        let mut out = Vec::new();
        let i = convert(s.as_bytes(), delim, &mut out, None, &AtomicBool::new(false))?;
        Ok((String::from_utf8(out).unwrap(), i.rows, i.cols))
    }

    #[test]
    fn records_flattened() {
        let j = r#"[{"id": 1, "name": "Zoé", "addr": {"city": "Lyon", "geo": {"lat": 45.75}}, "tags": ["a", "b"]},
                    {"id": 2, "name": "A;B", "extra": null, "ok": true, "addr": {}}]"#;
        assert_eq!(conv(j, b';').unwrap(), (
            "id;name;addr.city;addr.geo.lat;tags;extra;ok\n1;Zoé;Lyon;45,75;\"[\"\"a\"\",\"\"b\"\"]\";;\n2;\"A;B\";;;;;true\n".into(), 2, 7));
        assert_eq!(conv(j, b',').unwrap().0.lines().nth(1).unwrap(), "1,Zoé,Lyon,45.75,\"[\"\"a\"\",\"\"b\"\"]\",,");
    }

    #[test]
    fn wrapped_lines_and_rows() {
        // The first key holding an array of objects.
        let j = r#"{"meta": {"n": 2}, "empty": [], "nums": [1, 2], "data": [{"a": 1}, {"b": 2}], "more": [{"z": 0}]}"#;
        assert_eq!(conv(j, b',').unwrap().0, "a,b\n1,\n,2\n");
        // No such key: the object itself is the record.
        assert_eq!(conv(r#"{"a": 1, "b": {"c": "x"}}"#, b',').unwrap().0, "a,b.c\n1,x\n");
        // JSON Lines.
        assert_eq!(conv("{\"a\": 1}\r\n\n{\"a\": 2, \"b\": 1.5e-7}\n", b',').unwrap().0, "a,b\n1,\n2,1.5e-7\n");
        // Rows already: no header line, ragged as given.
        assert_eq!(conv(r#"[["h1", "h2"], [1, null, {"x": [1]}], []]"#, b',').unwrap(), ("h1,h2\n1,,\"{\"\"x\"\":[1]}\"\n\n".into(), 3, 3));
        // Mixed items: a non-object is {value: item}.
        assert_eq!(conv(r#"[{"a": 1}, 5, [1, 2]]"#, b',').unwrap().0, "a,value\n1,\n,5\n,\"[1,2]\"\n");
        assert_eq!(conv("[]", b',').unwrap(), ("\n".into(), 0, 0));
    }

    #[test]
    fn javascript_semantics() {
        // Integer-like keys first, ascending; a repeated key keeps its first place, its last value.
        assert_eq!(conv(r#"[{"b": 1, "10": "x", "2": "y", "a": {"1": 3, "0": 4}, "b": 9}]"#, b',').unwrap().0,
                   "2,10,b,a.0,a.1\ny,x,9,4,3\n");
        assert_eq!(conv(r#"[{"n": 12345678901234567890, "f": 0.1, "big": 1e21, "neg": -0.0, "s": "a\"b\n"}]"#, b',').unwrap().0,
                   "n,f,big,neg,s\n12345678901234567000,0.1,1e+21,0,\"a\"\"b\n\"\n");
        assert_eq!(conv(r#"[{"o": {"z": {"b": 1, "1": [true, null, "\u0001"]}}}]"#, b';').unwrap().0, "o.z.1;o.z.b\n\"[true,null,\"\"\\u0001\"\"]\";1\n");
        assert_eq!(conv("\u{feff}[{\"a\": 1}]", b',').unwrap().0, "a\n1\n");
    }

    #[test]
    fn errors() {
        assert_eq!(conv("42", b',').unwrap_err(), "the file holds a single value, not records");
        assert_eq!(conv("null", b',').unwrap_err(), "the file holds a single value, not records");
        let e = conv("{\"a\": 1}\nnot json\n", b',').unwrap_err();
        assert!(e.starts_with("not JSON: "), "{e}");
        assert_eq!(conv("", b',').unwrap(), ("\n".into(), 0, 0));
        assert_eq!(conv("[1] [2]", b',').unwrap_err().split(':').next(), Some("not JSON"));
        let mut out = Vec::new();
        let big = format!("[{}]", vec!["{\"a\": 1}"; 5000].join(","));
        assert_eq!(convert(big.as_bytes(), b',', &mut out, None, &AtomicBool::new(true)).unwrap_err(), super::super::CANCELLED);
        let p = AtomicUsize::new(0);
        convert(big.as_bytes(), b',', &mut Vec::new(), Some(&p), &AtomicBool::new(false)).unwrap();
        assert_eq!(p.into_inner(), 10000);
    }
}
