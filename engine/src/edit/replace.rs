//! Find & replace over the rows shown, the bulk edits of one column (set,
//! clear, trim, change case), and deleting the rows a filter hides.

use super::{fmt_int, js_space, not_saved, par_chunks, CellOp, Outcome};
use crate::table::{RowId, State};
use memchr::memmem;
use regex::{Captures, Regex, RegexBuilder, Replacer};
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

/* ---- Find & replace ------------------------------------------------ */

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReplaceSpec {
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub match_case: bool,
}

/// A piece of a regex replacement, written as in JavaScript.
#[derive(Debug, PartialEq)]
enum Tok {
    Lit(String),
    Group(usize),
    Named(String),
    Whole,
    Before,
    After,
}

enum How {
    /// Case kept: a plain substring.
    Lit(memmem::Finder<'static>),
    /// Case ignored: the text as an escaped regex, its replacement literal
    /// (a `$` in it stays a `$`).
    LitCi(Regex),
    Re(Regex, Vec<Tok>),
}

/// Plain text that cannot straddle two fields: a record whose raw bytes do
/// not hold it has no cell that does, so the row is skipped unsplit.
enum Raw {
    Case(memmem::Finder<'static>),
    NoCase(regex::bytes::Regex),
}

pub struct Replace {
    how: How,
    rep: String,
    raw: Option<Raw>,
}

const SIZE_LIMIT: usize = 1 << 22;

impl Replace {
    /// None when there is nothing to find; Err: the message for a regex that
    /// does not compile. Unlike the web app's, a regex here runs in linear
    /// time, so no pattern needs to be tried out first, nor refused as too slow.
    pub fn new(spec: &ReplaceSpec, st: &State) -> Result<Option<Replace>, String> {
        let f = &spec.find;
        if f.is_empty() {
            return Ok(None);
        }
        let build = |p: &str| RegexBuilder::new(p).case_insensitive(!spec.match_case).size_limit(SIZE_LIMIT).build();
        if spec.regex {
            let re = build(f).map_err(|e| format!("This regular expression is not valid: {}", crate::view::regex_error(&e)))?;
            let toks = js_template(&spec.replace, &re);
            return Ok(Some(Replace { how: How::Re(re, toks), rep: spec.replace.clone(), raw: None }));
        }
        let b = &st.base;
        let raw_ok = !f.bytes().any(|c| matches!(c, b'"' | b'\r' | b'\n') || c == b.dialect.delim)
            && !f.contains('\u{fffd}')
            && (b.enc.mem_utf8() || f.is_ascii());
        let (how, raw) = if spec.match_case {
            let fi = memmem::Finder::new(f.as_bytes()).into_owned();
            (How::Lit(fi.clone()), raw_ok.then_some(Raw::Case(fi)))
        } else {
            let esc = regex::escape(f);
            let re = build(&esc).map_err(|e| e.to_string())?;
            let raw = raw_ok.then(|| regex::bytes::RegexBuilder::new(&esc).case_insensitive(true).size_limit(SIZE_LIMIT).build().ok()).flatten();
            (How::LitCi(re), raw.map(Raw::NoCase))
        };
        Ok(Some(Replace { how, rep: spec.replace.clone(), raw }))
    }

    /// The text of one cell after the replacement; Borrowed when untouched.
    pub fn apply<'a>(&self, v: &'a str) -> Cow<'a, str> {
        match &self.how {
            How::Lit(fi) => {
                let mut it = fi.find_iter(v.as_bytes()).peekable();
                if it.peek().is_none() {
                    return Cow::Borrowed(v);
                }
                let (mut out, mut last) = (String::with_capacity(v.len() + self.rep.len()), 0);
                for at in it {
                    out.push_str(&v[last..at]);
                    out.push_str(&self.rep);
                    last = at + fi.needle().len();
                }
                out.push_str(&v[last..]);
                Cow::Owned(out)
            }
            How::LitCi(re) => re.replace_all(v, regex::NoExpand(&self.rep)),
            How::Re(re, toks) => re.replace_all(v, JsRep { toks, hay: v }),
        }
    }
}

impl CellOp for Replace {
    type Scratch = ();

    fn row_may_change(&self, st: &State, id: RowId, _: &mut ()) -> bool {
        match &self.raw {
            Some(raw) if st.pristine(id) => {
                let rec = st.base.record(id as usize);
                match raw {
                    Raw::Case(fi) => fi.find(rec).is_some(),
                    Raw::NoCase(re) => re.is_match(rec),
                }
            }
            _ => true,
        }
    }

    fn cell(&self, v: &str, _: usize, _: &mut ()) -> Outcome {
        if v.is_empty() {
            return Outcome::Skip;
        }
        match self.apply(v) {
            Cow::Owned(s) if s != v => Outcome::Change(s),
            _ => Outcome::Skip,
        }
    }
}

/// A replacement as JavaScript's `String.prototype.replace` reads it: `$$`,
/// `$&`, `` $` ``, `$'`, `$1`…`$99` (a number past the groups: one digit, or
/// the text as is), `$<name>` when the regex names groups.
fn js_template(rep: &str, re: &Regex) -> Vec<Tok> {
    let m = re.captures_len() - 1;
    let named = re.capture_names().any(|n| n.is_some());
    let c: Vec<char> = rep.chars().collect();
    let (mut out, mut lit, mut i) = (Vec::new(), String::new(), 0);
    let tok = |t: Tok, lit: &mut String, out: &mut Vec<Tok>| {
        if !lit.is_empty() {
            out.push(Tok::Lit(std::mem::take(lit)));
        }
        out.push(t);
    };
    while i < c.len() {
        if c[i] != '$' || i + 1 == c.len() {
            lit.push(c[i]);
            i += 1;
            continue;
        }
        let d = |k: usize| c.get(k).and_then(|x| x.to_digit(10)).map(|x| x as usize);
        match c[i + 1] {
            '$' => { lit.push('$'); i += 2 }
            '&' => { tok(Tok::Whole, &mut lit, &mut out); i += 2 }
            '`' => { tok(Tok::Before, &mut lit, &mut out); i += 2 }
            '\'' => { tok(Tok::After, &mut lit, &mut out); i += 2 }
            '0'..='9' => {
                let d1 = d(i + 1).unwrap();
                match d(i + 2).map(|d2| d1 * 10 + d2) {
                    Some(nn) if (1..=m).contains(&nn) => { tok(Tok::Group(nn), &mut lit, &mut out); i += 3 }
                    _ if (1..=m).contains(&d1) => { tok(Tok::Group(d1), &mut lit, &mut out); i += 2 }
                    _ => { lit.push('$'); i += 1 }
                }
            }
            '<' if named => match c[i + 2..].iter().position(|&x| x == '>') {
                Some(k) => {
                    tok(Tok::Named(c[i + 2..i + 2 + k].iter().collect()), &mut lit, &mut out);
                    i += k + 3;
                }
                None => { lit.push('$'); i += 1 }
            },
            _ => { lit.push('$'); i += 1 }
        }
    }
    if !lit.is_empty() {
        out.push(Tok::Lit(lit));
    }
    out
}

struct JsRep<'a> {
    toks: &'a [Tok],
    hay: &'a str,
}

impl Replacer for JsRep<'_> {
    fn replace_append(&mut self, caps: &Captures<'_>, dst: &mut String) {
        let m = caps.get(0).unwrap();
        for t in self.toks {
            match t {
                Tok::Lit(s) => dst.push_str(s),
                Tok::Group(g) => dst.push_str(caps.get(*g).map_or("", |x| x.as_str())),
                Tok::Named(n) => dst.push_str(caps.name(n).map_or("", |x| x.as_str())),
                Tok::Whole => dst.push_str(m.as_str()),
                Tok::Before => dst.push_str(&self.hay[..m.start()]),
                Tok::After => dst.push_str(&self.hay[m.end()..]),
            }
        }
    }
}

/// The replace bar's line once done.
pub fn replace_done(n: usize) -> String {
    if n > 0 { not_saved(&format!("{} cells replaced", fmt_int(n))) } else { "No match in the filtered rows.".into() }
}

/* ---- Bulk edits of one column ------------------------------------- */

#[derive(Clone, Debug, PartialEq)]
pub enum Bulk {
    Set(String),
    Clear,
    /// Runs of spaces as one, none at the ends.
    Trim,
    Upper,
    Lower,
    /// "Capitalized Words": lower case, then a capital at the start and after
    /// a space, a hyphen, an apostrophe or an opening parenthesis.
    Title,
}

impl CellOp for Bulk {
    type Scratch = String;

    fn cell(&self, v: &str, _: usize, buf: &mut String) -> Outcome {
        buf.clear();
        match self {
            Bulk::Set(s) => return if v == s { Outcome::Skip } else { Outcome::Change(s.clone()) },
            Bulk::Clear => return if v.is_empty() { Outcome::Skip } else { Outcome::Change(String::new()) },
            Bulk::Trim => {
                let mut gap = false;
                for c in v.chars() {
                    if js_space(c) {
                        gap = true;
                        continue;
                    }
                    if gap && !buf.is_empty() {
                        buf.push(' ');
                    }
                    gap = false;
                    buf.push(c);
                }
            }
            Bulk::Upper => upper_into(v, buf),
            Bulk::Lower => lower_into(v, buf),
            Bulk::Title => {
                lower_into(v, buf);
                if buf.chars().any(char::is_alphabetic) {
                    let low = std::mem::take(buf);
                    let mut prev: Option<char> = None;
                    for c in low.chars() {
                        let start = prev.is_none_or(|p| js_space(p) || matches!(p, '-' | '\'' | '’' | '('));
                        if start && c.is_alphabetic() { buf.extend(c.to_uppercase()) } else { buf.push(c) }
                        prev = Some(c);
                    }
                }
            }
        }
        if buf != v { Outcome::Change(buf.clone()) } else { Outcome::Skip }
    }
}

fn upper_into(v: &str, out: &mut String) {
    if v.is_ascii() {
        out.extend(v.bytes().map(|b| b.to_ascii_uppercase() as char));
    } else {
        out.extend(v.chars().flat_map(char::to_uppercase));
    }
}

fn lower_into(v: &str, out: &mut String) {
    if v.is_ascii() {
        out.extend(v.bytes().map(|b| b.to_ascii_lowercase() as char));
    } else if v.contains('Σ') {
        // A final sigma depends on its neighbours: only the whole string knows.
        out.push_str(&v.to_lowercase());
    } else {
        out.extend(v.chars().flat_map(char::to_lowercase));
    }
}

/// The dialog's note: what Apply would do.
pub fn bulk_note(changed: usize, shown: usize, filtered: bool) -> String {
    let head = if changed > 0 { format!("{} cells will change", fmt_int(changed)) } else { "No cell would change".into() };
    format!("{head} in the {} {}rows.", fmt_int(shown), if filtered { "filtered " } else { "" })
}

pub fn bulk_done(changed: usize, title: &str) -> String {
    not_saved(&format!("{} cells edited in {title}", fmt_int(changed)))
}

/* ---- Delete the hidden rows ---------------------------------------- */

pub const NO_HIDDEN_ROW: &str = "No row is hidden: set a filter first — every row it hides will be deleted.";

/// The data rows not among `shown`, for `Table::delete_rows`; Err: the
/// message when nothing is filtered out. The app then clears its filters,
/// which would now match every row.
pub fn hidden_rows(st: &State, shown: &[RowId], filtered: bool, cancel: &AtomicBool) -> Option<Result<HashSet<RowId>, &'static str>> {
    let n = st.len();
    if !filtered || shown.len() >= n {
        return Some(Err(NO_HIDDEN_ROW));
    }
    let top = shown.iter().copied().max().unwrap_or(0) as usize;
    let mut seen = vec![false; top + 1];
    for &id in shown {
        seen[id as usize] = true;
    }
    let parts = par_chunks(n, cancel, |r, _: &mut ()| {
        r.map(|p| st.data_id(p)).filter(|&id| !seen.get(id as usize).copied().unwrap_or(false)).collect::<Vec<_>>()
    })?;
    let hidden: HashSet<RowId> = parts.into_iter().flatten().collect();
    Some(if hidden.is_empty() { Err(NO_HIDDEN_ROW) } else { Ok(hidden) })
}

pub fn hidden_confirm(hidden: usize, shown: usize) -> String {
    format!("Delete the {} hidden rows?\n\nThe {} rows shown are kept, and the filters are cleared.", fmt_int(hidden), fmt_int(shown))
}

pub fn hidden_done(hidden: usize, kept: usize) -> String {
    not_saved(&format!("{} hidden rows deleted, {} kept", fmt_int(hidden), fmt_int(kept)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::tests::{col, table, NO};
    use crate::edit::{all_rows, changes, scan};

    fn rep(t: &crate::table::Table, find: &str, with: &str, regex: bool, case: bool, cols: &[usize]) -> Vec<(u32, usize, String)> {
        let spec = ReplaceSpec { find: find.into(), replace: with.into(), regex, match_case: case };
        let r = Replace::new(&spec, &t.st).unwrap().unwrap();
        changes(&t.st, &all_rows(&t.st), cols, &r, &NO).unwrap()
    }

    #[test]
    fn literal_replace() {
        let mut t = table("name;city\nÉmile;Lyon\nZoé;LYON 3e\nAna;Paris\n");
        assert_eq!(rep(&t, "lyon", "Lugdunum", false, true, &[0, 1]).len(), 0);
        let ch = rep(&t, "lyon", "$& Lug", false, false, &[0, 1]);
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[1].2, "$& Lug 3e");
        t.set_cells(&ch);
        assert_eq!(col(&t, 1), vec!["$& Lug", "$& Lug 3e", "Paris"]);
        // A needle holding the delimiter can't be checked on the raw record.
        assert_eq!(rep(&t, "é;", "x", false, true, &[0, 1]).len(), 0);
        assert_eq!(rep(&t, "é", "e", false, true, &[0]).len(), 1);
        assert_eq!(rep(&t, "É", "E", false, false, &[0]).len(), 2);
    }

    #[test]
    fn regex_replace_js_template() {
        let t = table("a,b\nJean Dupont,x\nMarie Curie,y\n,z\n");
        let ch = rep(&t, r"(\w+) (\w+)", "$2, $1 ($&) $$ $3 $0", true, true, &[0]);
        assert_eq!(ch[0].2, "Dupont, Jean (Jean Dupont) $ $3 $0");
        let ch = rep(&t, r"(?<first>\w+) (?<last>\w+)", "$<last> $<nope>|$<first", true, true, &[0]);
        assert_eq!(ch[1].2, "Curie |$<first");
        let ch = rep(&t, "Cu", "[$`|$']", true, true, &[0]);
        assert_eq!(ch[0].2, "Marie [Marie |rie]rie");
        let ch = rep(&t, "o", "0", true, false, &[0]);
        assert_eq!(ch[0].2, "Jean Dup0nt");
        assert!(Replace::new(&ReplaceSpec { find: "(".into(), regex: true, ..Default::default() }, &t.st).err().unwrap().starts_with("This regular expression is not valid"));
        assert!(Replace::new(&ReplaceSpec::default(), &t.st).unwrap().is_none());
        // Ten groups: $10 is the tenth, $11 the first then "1".
        let t = table("a\nabcdefghij\n");
        assert_eq!(rep(&t, "(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)", "$10$11", true, true, &[0])[0].2, "ja1");
    }

    #[test]
    fn bulk_ops() {
        let t = table("v,w\n  Jean-pierre   DE LA  fontaine ,1\nl'été (paris),2\n,3\nΟΔΟΣ,4\n");
        let ids = all_rows(&t.st);
        let run = |b: Bulk| changes(&t.st, &ids, &[0], &b, &NO).unwrap().into_iter().map(|c| c.2).collect::<Vec<_>>();
        assert_eq!(run(Bulk::Trim), vec!["Jean-pierre DE LA fontaine"]);
        assert_eq!(run(Bulk::Title), vec!["  Jean-Pierre   De La  Fontaine ", "L'Été (Paris)", "Οδος"]);
        assert_eq!(run(Bulk::Lower)[1], "οδος");
        assert_eq!(run(Bulk::Upper)[1], "L'ÉTÉ (PARIS)");
        assert_eq!(run(Bulk::Set("x".into())).len(), 4);
        assert_eq!(run(Bulk::Clear).len(), 3);
        let s = scan(&t.st, &ids, &[0], &Bulk::Clear, false, &NO).unwrap();
        assert_eq!(bulk_note(s.changed, 4, false), "3 cells will change in the 4 rows.");
        assert_eq!(s.examples[1].old, "l'été (paris)");
        assert!(s.changes.is_empty());
    }

    #[test]
    fn hidden() {
        let mut t = table("a,b\n1,x\n2,x\n3,x\n4,x\n");
        let ids = all_rows(&t.st);
        assert_eq!(hidden_rows(&t.st, &ids, false, &NO).unwrap().unwrap_err(), NO_HIDDEN_ROW);
        let h = hidden_rows(&t.st, &[ids[1], ids[3]], true, &NO).unwrap().unwrap();
        assert_eq!(h.len(), 2);
        t.delete_rows(&h);
        assert_eq!(col(&t, 0), vec!["2", "4"]);
    }
}
