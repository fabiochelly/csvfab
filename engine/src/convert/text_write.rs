//! A table as JSON, JSON Lines, an HTML table or a Markdown table: rows made in parallel, written
//! in order, atomically. A column whose cells are all numbers (or all `true`/`false`) gives bare
//! JSON values and right-aligned cells; anything that would not come back the same stays text.

use super::{check, io_err, tick, write_beside, Exported, Rows};
use crate::table::{RowId, State};
use rayon::prelude::*;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    /// An array of objects.
    Json,
    JsonLines,
    Html,
    Markdown,
}

impl TextFormat {
    pub fn ext(self) -> &'static str {
        match self { TextFormat::Json => ".json", TextFormat::JsonLines => ".jsonl", TextFormat::Html => ".html", TextFormat::Markdown => ".md" }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Text,
    Number,
    Bool,
}

/// A JSON number that says exactly what the cell says: no `007`, no `+1`, no `1.`, and few enough
/// digits for a reader's double to keep them.
fn json_number(v: &str) -> bool {
    let b = v.as_bytes();
    let mut i = (b.first() == Some(&b'-')) as usize;
    let digits = |i: &mut usize| { let s = *i; while *i < b.len() && b[*i].is_ascii_digit() { *i += 1; } *i - s };
    let start = i;
    let int = digits(&mut i);
    if int == 0 || (int > 1 && b[start] == b'0') { return false; }
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        frac = digits(&mut i);
        if frac == 0 { return false; }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) { i += 1; }
        if digits(&mut i) == 0 { return false; }
    }
    i == b.len() && int + frac <= 15
}

fn json_str(s: &str, o: &mut Vec<u8>) {
    o.push(b'"');
    let mut at = 0;
    for (i, &c) in s.as_bytes().iter().enumerate() {
        if c >= 0x20 && c != b'"' && c != b'\\' { continue; }
        o.extend_from_slice(&s.as_bytes()[at..i]);
        at = i + 1;
        match c {
            b'"' => o.extend_from_slice(b"\\\""),
            b'\\' => o.extend_from_slice(b"\\\\"),
            b'\n' => o.extend_from_slice(b"\\n"),
            b'\r' => o.extend_from_slice(b"\\r"),
            b'\t' => o.extend_from_slice(b"\\t"),
            _ => { let _ = write!(o, "\\u{c:04x}"); }
        }
    }
    o.extend_from_slice(&s.as_bytes()[at..]);
    o.push(b'"');
}

fn html(s: &str, o: &mut Vec<u8>) {
    let mut at = 0;
    for (i, &c) in s.as_bytes().iter().enumerate() {
        let r: &[u8] = match c { b'&' => b"&amp;", b'<' => b"&lt;", b'>' => b"&gt;", _ => continue };
        o.extend_from_slice(&s.as_bytes()[at..i]);
        o.extend_from_slice(r);
        at = i + 1;
    }
    o.extend_from_slice(&s.as_bytes()[at..]);
}

fn markdown(s: &str, o: &mut Vec<u8>) {
    let mut at = 0;
    for (i, &c) in s.as_bytes().iter().enumerate() {
        let r: &[u8] = match c { b'|' => b"\\|", b'\\' => b"\\\\", b'\n' => b"<br>", b'\r' => b"", _ => continue };
        o.extend_from_slice(&s.as_bytes()[at..i]);
        o.extend_from_slice(r);
        at = i + 1;
    }
    o.extend_from_slice(&s.as_bytes()[at..]);
}

/// The titles as JSON keys: unique, none empty.
fn keys(st: &State) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    st.headers().into_iter().take(st.ncols).enumerate().map(|(c, h)| {
        let base = if h.is_empty() { format!("column {}", c + 1) } else { h };
        let mut k = base.clone();
        let mut n = 1;
        while !seen.insert(k.clone()) {
            n += 1;
            k = format!("{base} ({n})");
        }
        k
    }).collect()
}

const CHUNK: usize = 8192;
const BATCH: usize = 64;

/// Each column's kind, from every cell written. Progress: rows.
fn kinds(st: &State, rows: &Rows, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Vec<Kind>, String> {
    #[derive(Clone, Copy)]
    struct K { filled: bool, num: bool, boolean: bool }
    let n = rows.len();
    let parts: Vec<Vec<K>> = (0..n).into_par_iter().step_by(CHUNK).map(|a| {
        let mut k = vec![K { filled: false, num: true, boolean: true }; st.ncols];
        if cancel.load(Ordering::Relaxed) { return k; }
        for p in a..(a + CHUNK).min(n) {
            st.visit(rows.id(p), |c, v| {
                let Some(x) = k.get_mut(c) else { return };
                if v.is_empty() { return; }
                x.filled = true;
                if x.num { x.num = json_number(v); }
                if x.boolean { x.boolean = matches!(v, "true" | "false"); }
            });
        }
        tick(progress, (a + CHUNK).min(n) - a);
        k
    }).collect();
    check(cancel)?;
    Ok((0..st.ncols).map(|c| {
        let (mut filled, mut num, mut boolean) = (false, true, true);
        for p in &parts { filled |= p[c].filled; num &= p[c].num; boolean &= p[c].boolean; }
        match (filled, num, boolean) { (true, true, _) => Kind::Number, (true, _, true) => Kind::Bool, _ => Kind::Text }
    }).collect())
}

/// One row's cells, gaps and all: `f(column, value)` for every column, in order.
fn each_cell(st: &State, id: RowId, mut f: impl FnMut(usize, &str)) {
    let mut next = 0;
    st.visit(id, |c, v| {
        if c < st.ncols {
            while next < c { f(next, ""); next += 1; }
            f(c, v);
            next = c + 1;
        }
    });
    while next < st.ncols { f(next, ""); next += 1; }
}

/// Exports the table to `dest` (`rows`: the data rows, in order; None: all). `name`: the page's
/// title in HTML. Progress: rows done, twice over (columns are typed in a first pass).
pub fn write(format: TextFormat, st: &State, rows: Option<&[RowId]>, dest: &Path, name: &str, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Exported, String> {
    if st.ncols == 0 { return Err("no column to write".into()); }
    let rows = Rows { st, ids: rows };
    let n = rows.len();
    let kinds = kinds(st, &rows, progress, cancel)?;
    let titles: Vec<String> = if format == TextFormat::Json || format == TextFormat::JsonLines { keys(st) } else { let mut h = st.headers(); h.truncate(st.ncols); h };
    // Each column's `"key":`, made once.
    let keyed: Vec<Vec<u8>> = titles.iter().map(|k| { let mut o = Vec::new(); json_str(k, &mut o); o.push(b':'); o }).collect();
    let row = |id: RowId, last: bool, o: &mut Vec<u8>| match format {
        TextFormat::Json | TextFormat::JsonLines => {
            o.push(b'{');
            each_cell(st, id, |c, v| {
                if c > 0 { o.push(b','); }
                o.extend_from_slice(&keyed[c]);
                match kinds[c] {
                    Kind::Text => json_str(v, o),
                    _ if v.is_empty() => o.extend_from_slice(b"null"),
                    _ => o.extend_from_slice(v.as_bytes()),
                }
            });
            o.push(b'}');
            if format == TextFormat::Json && !last { o.push(b','); }
            o.push(b'\n');
        }
        TextFormat::Html => {
            o.extend_from_slice(b"<tr>");
            each_cell(st, id, |c, v| {
                o.extend_from_slice(if kinds[c] == Kind::Number { b"<td class=n>" } else { b"<td>" });
                html(v, o);
                o.extend_from_slice(b"</td>");
            });
            o.extend_from_slice(b"</tr>\n");
        }
        TextFormat::Markdown => {
            o.push(b'|');
            each_cell(st, id, |_, v| {
                o.push(b' ');
                markdown(v, o);
                o.extend_from_slice(b" |");
            });
            o.push(b'\n');
        }
    };
    let (_, _, _) = write_beside(dest, false, |w| {
        match format {
            TextFormat::Json => w.write_all(b"[\n").map_err(io_err)?,
            TextFormat::JsonLines => {}
            TextFormat::Html => {
                let mut o = Vec::new();
                o.extend_from_slice(b"<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>");
                html(name, &mut o);
                o.extend_from_slice(b"</title>\n<style>table{border-collapse:collapse;font:14px system-ui,sans-serif}th,td{border:1px solid #ccc;padding:4px 8px;white-space:pre-wrap;unicode-bidi:plaintext;text-align:start}th{background:#f4f4f4}td.n{text-align:right}</style></head>\n<body><table>\n<thead><tr>");
                for t in &titles { o.extend_from_slice(b"<th>"); html(t, &mut o); o.extend_from_slice(b"</th>"); }
                o.extend_from_slice(b"</tr></thead>\n<tbody>\n");
                w.write_all(&o).map_err(io_err)?;
            }
            TextFormat::Markdown => {
                let mut o = vec![b'|'];
                for t in &titles { o.push(b' '); markdown(t, &mut o); o.extend_from_slice(b" |"); }
                o.extend_from_slice(b"\n|");
                for k in &kinds { o.extend_from_slice(if *k == Kind::Number { b" ---: |" } else { b" --- |" }); }
                o.push(b'\n');
                w.write_all(&o).map_err(io_err)?;
            }
        }
        let mut start = 0;
        while start < n {
            check(cancel)?;
            let end = (start + CHUNK * BATCH).min(n);
            let parts: Vec<Vec<u8>> = (start..end).into_par_iter().step_by(CHUNK).map(|a| {
                let mut o = Vec::with_capacity(CHUNK * 64);
                for p in a..(a + CHUNK).min(end) { row(rows.id(p), p + 1 == n, &mut o); }
                o
            }).collect();
            for p in &parts { w.write_all(p).map_err(io_err)?; }
            tick(progress, end - start);
            start = end;
        }
        match format {
            TextFormat::Json => w.write_all(b"]\n").map_err(io_err)?,
            TextFormat::Html => w.write_all(b"</tbody></table></body></html>\n").map_err(io_err)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(Exported { rows: n as u64 + st.header as u64, cols: st.ncols, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn out(f: TextFormat, s: &str, rows: Option<&[RowId]>) -> String {
        let t = table(s);
        let dir = std::env::temp_dir().join(format!("csvfab-text-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("x{}", f.ext()));
        write(f, &t.st, rows, &p, "a <b>", None, &AtomicBool::new(false)).unwrap();
        let r = std::fs::read_to_string(&p).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    #[test]
    fn numbers_that_come_back() {
        for v in ["0", "12", "-3", "1.5", "0.25", "1e5", "-1.5E-3"] { assert!(json_number(v), "{v}"); }
        for v in ["", "-", "007", "+1", "1.", ".5", "1e", "0x10", "1 000", "1234567890123456"] { assert!(!json_number(v), "{v}"); }
    }

    #[test]
    fn json_is_typed_by_column() {
        let s = "id,name,ok,zip,mixed\n1,Zoé,true,01000,1\n2,\"a\"\"b\",false,75001,x\n,\"l1\nl2\",,,\n";
        let v: serde_json::Value = serde_json::from_str(&out(TextFormat::Json, s, None)).unwrap();
        assert_eq!(v, serde_json::json!([
            {"id": 1, "name": "Zoé", "ok": true, "zip": "01000", "mixed": "1"},
            {"id": 2, "name": "a\"b", "ok": false, "zip": "75001", "mixed": "x"},
            {"id": null, "name": "l1\nl2", "ok": null, "zip": "", "mixed": ""},
        ]));
        let l = out(TextFormat::JsonLines, s, None);
        assert_eq!(l.lines().count(), 3);
        assert_eq!(l.lines().next().unwrap(), r#"{"id":1,"name":"Zoé","ok":true,"zip":"01000","mixed":"1"}"#);
    }

    #[test]
    fn empty_and_repeated_titles() {
        let l = out(TextFormat::JsonLines, "a,a,,x\n1,2,3,y\n", None);
        assert_eq!(l, "{\"a\":1,\"a (2)\":2,\"column 3\":3,\"x\":\"y\"}\n");
    }

    #[test]
    fn html_and_markdown() {
        let s = "n,t\n1,<a&b>\n2,x|y\\z\n";
        let h = out(TextFormat::Html, s, None);
        assert!(h.contains("<title>a &lt;b&gt;</title>"), "{h}");
        assert!(h.contains("<tr><td class=n>1</td><td>&lt;a&amp;b&gt;</td></tr>"), "{h}");
        assert!(h.contains("<th>n</th><th>t</th>"));
        assert_eq!(out(TextFormat::Markdown, s, None), "| n | t |\n| ---: | --- |\n| 1 | <a&b> |\n| 2 | x\\|y\\\\z |\n");
    }

    #[test]
    fn only_the_rows_given() {
        let t = table("a,b\n1,x\n2,y\n3,z\n");
        let ids = [t.st.data_id(2), t.st.data_id(0)];
        let l = out(TextFormat::JsonLines, "a,b\n1,x\n2,y\n3,z\n", Some(&ids));
        assert_eq!(l, "{\"a\":3,\"b\":\"z\"}\n{\"a\":1,\"b\":\"x\"}\n");
    }
}
