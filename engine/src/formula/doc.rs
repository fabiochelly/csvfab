//! What a formula can use, for the people writing one (FX_DOC): the function
//! picker's catalogue, the expression box's autocomplete, and the examples,
//! which run as they are once {N} {D} {T} {T2} are replaced by a number, date,
//! text and second text column of the file.

use super::fx::{self, Fx};
use crate::table::State;
use crate::view::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Email,
    Phone,
}

/// One entry: family, name, signature (its first argument is selected on
/// insert), what it does, examples (formula, what it shows), and the column
/// to prefer — by its values — over the first text one.
#[derive(Debug)]
pub struct Doc {
    pub fam: &'static str,
    pub name: &'static str,
    pub sig: &'static str,
    pub desc: &'static str,
    pub ex: &'static [(&'static str, &'static str)],
    pub pick: Option<Pick>,
}

pub const FAMILIES: &[&str] = &["Numbers", "Dates", "Text", "Tests", "Values", "IDs & hashes", "Methods", "Operators"];
/// The picker's tabs.
pub const TABS: &[&str] = &["All", "Numbers", "Dates", "Text", "Tests", "Values", "IDs & hashes", "Methods", "Operators", "Columns"];

/// The expression box's placeholder.
pub const PLACEHOLDER: &str = "e.g.  {Amount}.num() > 1000 && {City}.contains(\"lyon\")";

macro_rules! d {
    ($f:literal, $n:literal, $s:literal, $d:literal, [$(($e:literal, $w:literal)),*]) => { d!($f, $n, $s, $d, [$(($e, $w)),*], None) };
    ($f:literal, $n:literal, $s:literal, $d:literal, [$(($e:literal, $w:literal)),*], $p:expr) => {
        Doc { fam: $f, name: $n, sig: $s, desc: $d, ex: &[$(($e, $w)),*], pick: $p }
    };
}

pub static CATALOGUE: &[Doc] = &[
    d!("Numbers", "num", "num(v)", "The value as a number: reads 1 234,50 as well as 1,234.50; an empty cell counts as 0.", [("{N}.num() * 1.2", "plus 20 %")]),
    d!("Numbers", "round", "round(n, digits)", "Rounded to that many decimals (none by default).", [("{N}.num().round(1)", "one decimal")]),
    d!("Numbers", "fixed", "fixed(n, digits)", "Text with exactly that many decimals, written with the file's decimal mark.", [("{N}.num().fixed(2)", "two decimals")]),
    d!("Numbers", "abs", "abs(n)", "The value without its sign.", [("{N}.num().abs()", "absolute value")]),
    d!("Numbers", "sum", "sum(a, b, …)", "The sum of the values given; empty cells are left out.", [("sum({N}, 100)", "add two values")]),
    d!("Numbers", "avg", "avg(a, b, …)", "The average of the values given; empty cells are left out.", [("avg({N}, 0)", "average")]),
    d!("Numbers", "min", "min(a, b, …)", "The smallest of the values given.", [("min({N}, 1000)", "capped at 1000")]),
    d!("Numbers", "max", "max(a, b, …)", "The largest of the values given.", [("max({N}, 0)", "no negatives")]),
    d!("Dates", "date", "date(v)", "The value as a date: yyyy-mm-dd or day first (dd/mm/yyyy), a time kept.", [("{D}.date()", "read a date")]),
    d!("Dates", "today", "today()", "Today's date.", [("today()", "today")]),
    d!("Dates", "days", "days(from, to)", "Days from one date to the other, negative when the second is earlier.", [("{D}.days(today())", "days since")]),
    d!("Dates", "months", "months(from, to)", "Whole months from one date to the other.", [("{D}.months(today())", "months since")]),
    d!("Dates", "years", "years(from, to)", "Whole years from one date to the other: an age.", [("{D}.years(today())", "age")]),
    d!("Dates", "addDays", "addDays(date, n)", "The date n days later (earlier when n is negative).", [("{D}.addDays(30)", "thirty days later")]),
    d!("Dates", "addMonths", "addMonths(date, n)", "The date n months later; the 31st becomes the month's last day when needed.", [("{D}.addMonths(1)", "a month later")]),
    d!("Dates", "year", "year(date)", "The year.", [("{D}.year()", "year"), ("{D}.year() === 2024", "dated 2024")]),
    d!("Dates", "month", "month(date)", "The month, 1 to 12.", [("{D}.month()", "month")]),
    d!("Dates", "day", "day(date)", "The day of the month.", [("{D}.day()", "day")]),
    d!("Dates", "weekday", "weekday(date)", "The day of the week, 1 for Monday to 7 for Sunday.", [("{D}.weekday() >= 6", "on a weekend")]),
    d!("Dates", "week", "week(date)", "The ISO week number.", [("{D}.week()", "week number")]),
    d!("Dates", "fmtDate", "fmtDate(date, \"dd/mm/yyyy\")", "The date written with a pattern: yyyy yy mm dd hh mi ss.", [("{D}.fmtDate(\"yyyy-mm-dd\")", "ISO date")]),
    d!("Text", "upper", "upper(s)", "In capitals.", [("{T}.upper()", "capitals")]),
    d!("Text", "lower", "lower(s)", "In lower case.", [("{T}.lower()", "lower case")]),
    d!("Text", "capitalize", "capitalize(s)", "Every Word Capitalised, the rest in lower case.", [("{T}.capitalize()", "capitalised")]),
    d!("Text", "trim", "trim(s)", "Spaces removed at both ends, repeated spaces made one.", [("trim({T})", "clean spaces")]),
    d!("Text", "slug", "slug(s)", "Lower case without accents or symbols: for comparing loosely.", [("{T}.slug()", "slug")]),
    d!("Text", "len", "len(s)", "The number of characters.", [("{T}.len() > 20", "long values")]),
    d!("Text", "left", "left(s, n)", "The first n characters.", [("{T}.left(3)", "first three")]),
    d!("Text", "right", "right(s, n)", "The last n characters.", [("{T}.right(3)", "last three")]),
    d!("Text", "mid", "mid(s, start, n)", "n characters from position start (the first is 1).", [("{T}.mid(2, 3)", "three from the 2nd")]),
    d!("Text", "split", "split(s, sep, n)", "The nth part of the text cut at sep (the first is 1, -1 the last).", [("split({T}, \" \", 1)", "first word"), ("split({T}, \" \", -1)", "last word")]),
    d!("Text", "pad", "pad(s, n, \"0\")", "Filled on the left up to n characters.", [("{N}.pad(8, \"0\")", "eight digits")]),
    d!("Text", "firstNumber", "firstNumber(s)", "The first number in the text, as written: \"68 - Rhin\" and \"Rhin 68\" give 68, \"01 - Ain\" keeps 01; a decimal part kept. Empty when there is none.", [("{T}.firstNumber()", "first number"), ("{T}.firstNumber().num() > 50", "as a number")]),
    d!("Text", "digits", "digits(s)", "Every digit of the text, the rest removed: \"06 12-34\" gives 061234.", [("{T}.digits()", "digits only")]),
    d!("Text", "phonetic", "phonetic(s)", "The sound of the text (Metaphone adapted to French): Dupont and Dupond give the same code, as Lefebvre and Lefèvre.", [("{T}.phonetic()", "phonetic code")]),
    d!("Text", "similarity", "similarity(a, b)", "How alike two texts are, from 0 to 1 (1 − edit distance / the longer length).", [("{T}.similarity({T2})", "similarity"), ("{T}.slug().similarity({T2}.slug()) > 0.8", "nearly the same")]),
    d!("Text", "extract", "extract(s, regex, group)", "The part a regular expression captures (group 1 by default).", [("{T}.extract(\"@(.+)$\")", "e-mail domain")], Some(Pick::Email)),
    // "\\s+" as the web app wrote it: inside "…" a \s is an s, so it replaces runs of s; /\s+/g was meant. Kept as it was.
    d!("Text", "replace", "replace(s, regex, by)", "Every match of a regular expression replaced; $1… reuse its groups.", [("replace({T}, \"\\s+\", \"-\")", "spaces to dashes")]),
    d!("Tests", "contains", "contains(s, part)", "True when the text contains the part, ignoring case and accents.", [("{T}.contains(\"a\")", "contains an a")]),
    d!("Tests", "startsWith", "startsWith(s, part)", "True when the text starts with the part, ignoring case and accents.", [("startsWith({T}, \"a\")", "starts with a")]),
    d!("Tests", "endsWith", "endsWith(s, part)", "True when the text ends with the part, ignoring case and accents.", [("endsWith({T}, \"e\")", "ends with e")]),
    d!("Tests", "matches", "matches(s, regex)", "True when a regular expression matches (case ignored when given as text).", [("{T}.matches(\"^[a-m]\")", "starts with a to m")]),
    d!("Tests", "empty", "empty(v)", "True when the cell is empty or only spaces.", [("{T}.empty()", "empty"), ("!{T}.empty()", "filled")]),
    d!("Tests", "isEmail", "isEmail(v)", "True when the value looks like an e-mail address.", [("!{T}.isEmail()", "not an e-mail")], Some(Pick::Email)),
    d!("Tests", "isPhone", "isPhone(v)", "True when the value reads as a phone number (national numbers as French).", [("!{T}.isPhone()", "not a phone")], Some(Pick::Phone)),
    d!("Tests", "isNumber", "isNumber(v)", "True when the value reads as a number.", [("{N}.isNumber()", "a number")]),
    d!("Tests", "isDate", "isDate(v)", "True when the value reads as a date.", [("!{D}.isDate()", "not a date")]),
    d!("Values", "ifs", "ifs(test1, value1, test2, value2, …, otherwise)", "The value of the first test that holds, else the last argument.", [("ifs({N}.num() > 1000, \"big\", {N}.num() > 100, \"medium\", \"small\")", "three sizes")]),
    d!("Values", "cases", "cases(v, value1, result1, …, otherwise)", "The result paired with the value v equals, else the last argument.", [("{T}.cases(\"Paris\", \"75\", \"Lyon\", \"69\", \"other\")", "a code per value"), ("{D}.month().cases(1, \"January\", 2, \"February\", \"other\")", "month names")]),
    d!("Values", "first", "first(a, b, …)", "The first value that is not empty.", [("first({T}, {T2}, \"none\")", "first filled")]),
    d!("Values", "join", "join(sep, a, b, …)", "The values joined with sep, empty ones skipped.", [("join(\" \", {T}, {T2})", "join two columns")]),
    d!("Values", "row", "row", "The row's number in the file.", [("row % 2 === 0", "even rows")]),
    d!("IDs & hashes", "uuid", "uuid()", "A random UUID (version 4), a new one per row.", [("uuid()", "random id")]),
    d!("IDs & hashes", "uuid7", "uuid7()", "A UUID that sorts in creation order (version 7, time first): row after row, they come out in order.", [("uuid7()", "sortable id")]),
    d!("IDs & hashes", "md5", "md5(s)", "MD5 of the text, in hex. Fine for spotting identical values, not for security.", [("{T}.md5()", "MD5")], Some(Pick::Email)),
    d!("IDs & hashes", "sha1", "sha1(s)", "SHA-1 of the text, in hex.", [("{T}.sha1()", "SHA-1")], Some(Pick::Email)),
    d!("IDs & hashes", "sha256", "sha256(s)", "SHA-256 of the text, in hex, as sha256sum prints it.", [("{T}.sha256()", "SHA-256"), ("{T}.lower().trim().sha256()", "normalised e-mail")], Some(Pick::Email)),
    d!("IDs & hashes", "sha3_256", "sha3_256(s)", "SHA3-256 of the text, in hex.", [("{T}.sha3_256()", "SHA3-256")], Some(Pick::Email)),
    d!("IDs & hashes", "sha3_512", "sha3_512(s)", "SHA3-512 of the text, in hex.", [("{T}.sha3_512()", "SHA3-512")], Some(Pick::Email)),
    d!("IDs & hashes", "blake2b", "blake2b(s)", "BLAKE2b-512 of the text, in hex, as b2sum prints it.", [("{T}.blake2b()", "BLAKE2b")], Some(Pick::Email)),
    d!("IDs & hashes", "blake3", "blake3(s)", "BLAKE3 (256 bits) of the text, in hex, as b3sum prints it.", [("{T}.blake3()", "BLAKE3")], Some(Pick::Email)),
    d!("Methods", ".startsWith", ".startsWith(text)", "A column is a string: its own methods work too. True when it starts with the text, case sensitive.", [("{T}.startsWith(\"A\")", "starts with A")]),
    d!("Methods", ".endsWith", ".endsWith(text)", "True when the string ends with the text, case sensitive.", [("{T}.endsWith(\"e\")", "ends with e")]),
    d!("Methods", ".includes", ".includes(text)", "True when the string contains the text, case sensitive.", [("{T}.includes(\"-\")", "contains a dash")]),
    d!("Methods", ".length", ".length", "The number of characters.", [("{T}.length", "length")]),
    d!("Methods", ".slice", ".slice(start, end)", "The characters from start to end (counted from 0; negative from the end).", [("{T}.slice(-4)", "last four")]),
    d!("Methods", ".split", ".split(sep)", "The parts as a list; [0] is the first.", [("{T}.split(\" \")[0]", "first word")]),
    d!("Methods", ".replaceAll", ".replaceAll(text, by)", "Every occurrence of the text replaced.", [("{T}.replaceAll(\" \", \"\")", "without spaces")]),
    d!("Methods", ".toUpperCase", ".toUpperCase()", "In capitals.", [("{T}.toUpperCase()", "capitals")]),
    d!("Methods", ".trim", ".trim()", "Spaces removed at both ends.", [("{T}.trim()", "trimmed")]),
    d!("Methods", ".padStart", ".padStart(n, \"0\")", "Filled on the left up to n characters.", [("{N}.padStart(6, \"0\")", "six digits")]),
    d!("Operators", "===", " === ", "Equal (the value as written: compare text with text). !== for different.", [("{T} === \"\"", "empty cell")]),
    d!("Operators", "> < >= <=", " > ", "Compare numbers or dates: read them with num() or date() first.", [("{N}.num() >= 100", "at least 100"), ("{D}.date() < today()", "in the past")]),
    d!("Operators", "&&", " && ", "And: both must hold.", [("!{T}.empty() && {N}.num() > 0", "both")]),
    d!("Operators", "||", " || ", "Or: either may hold.", [("{T}.empty() || {T2}.empty()", "one of them empty")]),
    d!("Operators", "!", "!", "Not: reverses a test.", [("!{T}.contains(\"test\")", "without \"test\"")]),
    d!("Operators", "? :", " ? \"yes\" : \"no\"", "If … then … else.", [("{N}.num() > 1000 ? \"big\" : \"small\"", "a condition")]),
    d!("Operators", "+ - * /", " + ", "Arithmetic on numbers; + also joins text.", [("{T} + \" \" + {T2}", "join text"), ("{N}.num() / 2", "half")]),
];

/// The helper's documentation entry.
pub fn doc_of(name: &str) -> Option<&'static Doc> {
    CATALOGUE.iter().find(|d| d.name == name)
}

/// Every helper's name, in the web app's order.
pub fn functions() -> impl Iterator<Item = &'static str> {
    Fx::ALL.iter().map(|f| f.name())
}

/// Whether the helper is also a method of texts: {City}.upper().
pub fn is_method(name: &str) -> bool {
    Fx::by_name(name).is_some_and(|f| fx::ON_TEXT.contains(&f))
}

/// A helper's method signature, from its function one: left(s, n) → .left(n).
pub fn method_sig(d: &Doc) -> Option<String> {
    if !is_method(d.name) {
        return None;
    }
    let p = d.sig.find('(')?;
    let inner = &d.sig[p + 1..d.sig.rfind(')')?];
    let rest = inner.split(',').skip(1).collect::<Vec<_>>().join(",");
    Some(format!(".{}({})", d.name, rest.trim()))
}

/// Accents off and lower case, one character for one (so match positions are the label's).
fn fold_chars(s: &str) -> Vec<char> {
    s.chars().map(|c| {
        let mut f = String::new();
        super::fx::remove_accents(&c.to_lowercase().collect::<String>(), &mut f);
        f.chars().next().unwrap_or(c)
    }).collect()
}

/// Fuzzy match of the (folded, lower-case) query in a label: the characters in
/// order, runs and word starts scoring more. The hits are character indexes.
pub fn fuzzy(q: &str, text: &str) -> Option<(f64, Vec<usize>)> {
    let s = fold_chars(text);
    let (mut from, mut prev, mut score) = (0usize, -2isize, 0.0);
    let mut hits = Vec::new();
    for ch in q.chars() {
        let k = (from..s.len()).find(|&k| s[k] == ch)?;
        score += if k as isize == prev + 1 { 6.0 } else { 1.0 };
        if k == 0 || s[k - 1].is_whitespace() || matches!(s[k - 1], ':' | '—' | '-' | '_' | '(') {
            score += 5.0;
        }
        hits.push(k);
        prev = k as isize;
        from = k + 1;
    }
    let first = hits.first().copied().unwrap_or(0) as f64;
    Some((score - first * 0.05 - s.len() as f64 * 0.01, hits))
}

fn fold_query(q: &str) -> String {
    let mut out = String::new();
    super::fx::remove_accents(&q.trim().to_lowercase(), &mut out);
    out
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuggestKind {
    Column,
    /// A helper or a method, with its signature.
    Function(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub label: String,
    /// What replaces `start..end` of the text.
    pub insert: String,
    pub kind: SuggestKind,
    /// Matched characters of the label (char indexes), to show in bold.
    pub hits: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub items: Vec<Suggestion>,
    /// Byte range of `text` the chosen insert replaces.
    pub start: usize,
    pub end: usize,
}

/// The best first: by score, at most 12; all of them in order for an empty query.
fn rank(q: &str, list: Vec<(String, String, SuggestKind)>) -> Vec<Suggestion> {
    let mut out: Vec<(f64, Suggestion)> = if q.is_empty() {
        list.into_iter().map(|(label, insert, kind)| (0.0, Suggestion { label, insert, kind, hits: vec![] })).collect()
    } else {
        list.into_iter().filter_map(|(label, insert, kind)| fuzzy(q, &label).map(|(s, hits)| (s, Suggestion { label, insert, kind, hits }))).collect()
    };
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().take(12).map(|x| x.1).collect()
}

/// The expression box's suggestions for what sits before the caret (a byte
/// offset): inside an open brace the columns ({ alone lists them all), after a
/// value and a dot its methods, elsewhere the helpers once a word is begun.
pub fn complete(text: &str, caret: usize, headers: &[String]) -> Option<Completion> {
    let pos = caret.min(text.len());
    let before = &text[..pos];
    let open = before.rfind('{');
    let close = open.and_then(|o| text[o..].find('}').map(|c| o + c));
    let (items, start, mut end) = if let (Some(o), true) = (open, close.is_none_or(|c| c >= pos)) {
        let q = fold_query(&text[o + 1..pos]);
        let list = headers.iter().map(|h| (h.clone(), format!("{{{h}}}"), SuggestKind::Column)).collect();
        let end = if close == Some(pos) { pos + 1 } else { pos }; // the caret before an existing }: replace it too
        (rank(&q, list), o, end)
    } else {
        let w0 = before.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + before[i..].chars().next().unwrap().len_utf8());
        let mut word = &before[w0..];
        // [A-Za-z_]\w*$: a word does not start with a digit
        while word.starts_with(|c: char| c.is_ascii_digit()) {
            word = &word[1..];
        }
        let start = pos - word.len();
        let prev = &before[..start];
        let after_value_dot = prev.ends_with('.') && prev[..prev.len() - 1].ends_with(|c: char| matches!(c, '}' | ']' | ')' | '"' | '\'') || c.is_alphanumeric() || c == '_');
        if after_value_dot {
            let mut list: Vec<_> = CATALOGUE.iter().filter(|d| is_method(d.name))
                .map(|d| (d.name.to_string(), format!("{}(", d.name), SuggestKind::Function(method_sig(d).unwrap_or_default()))).collect();
            list.extend(CATALOGUE.iter().filter(|d| d.fam == "Methods").map(|d| {
                let ins = &d.sig[1..];
                (d.name[1..].to_string(), ins.find('(').map_or(ins.to_string(), |p| ins[..=p].to_string()), SuggestKind::Function(d.sig.to_string()))
            }));
            (rank(&word.to_lowercase(), list), start, pos)
        } else if !word.is_empty() && !prev.ends_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '}' | '"' | '\'' | '.')) {
            let list = functions().map(|n| (n.to_string(), format!("{n}("), SuggestKind::Function(doc_of(n).map_or(String::new(), |d| d.sig.to_string())))).collect();
            (rank(&word.to_lowercase(), list), start, pos)
        } else {
            return None;
        }
    };
    if items.is_empty() {
        return None;
    }
    end = end.min(text.len());
    Some(Completion { items, start, end })
}

/// An entry of the function picker: a catalogue entry or a column.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Doc(&'static Doc),
    Column(usize),
}

impl PartialEq for Doc {
    fn eq(&self, o: &Doc) -> bool {
        self.name == o.name
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickerItem {
    pub entry: Entry,
    pub hits: Vec<usize>,
}

/// The picker's list: the entries of the tab `tab` (see TABS) matching the
/// query, best first — by name (fuzzy), else by a word of what it does that
/// starts with the query ("age" finds years, not average).
pub fn picker(query: &str, tab: &str, headers: &[String]) -> Vec<PickerItem> {
    let docs = CATALOGUE.iter().filter(|d| tab == "All" || d.fam == tab).map(Entry::Doc);
    let cols = (0..headers.len()).filter(|_| tab == "All" || tab == "Columns").map(Entry::Column);
    let pool: Vec<Entry> = docs.chain(cols).collect();
    let q = fold_query(query);
    if q.is_empty() {
        return pool.into_iter().map(|entry| PickerItem { entry, hits: vec![] }).collect();
    }
    let qz: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out: Vec<(f64, PickerItem)> = Vec::new();
    for entry in pool {
        let (name, desc) = match &entry {
            Entry::Doc(d) => (d.name, d.desc),
            Entry::Column(c) => (headers[*c].as_str(), ""),
        };
        if let Some((s, hits)) = fuzzy(&qz, name) {
            out.push((s + 10.0, PickerItem { entry, hits }));
        } else if !desc.is_empty() {
            let mut words = String::from(" ");
            for c in fold_query(desc).chars() {
                words.push(if c.is_alphanumeric() { c } else { ' ' });
            }
            if words.contains(&format!(" {q}")) {
                out.push((0.0, PickerItem { entry, hits: vec![] }));
            }
        }
    }
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().map(|x| x.1).collect()
}

/// {N} {D} {T} {T2} → the columns given, as names: the example runs on the file.
pub fn fill_example(src: &str, n: &str, d: &str, t: &str, t2: &str) -> String {
    src.replace("{N}", &format!("{{{n}}}")).replace("{D}", &format!("{{{d}}}")).replace("{T2}", &format!("{{{t2}}}")).replace("{T}", &format!("{{{t}}}"))
}

/// The examples' columns for a file without any: the web app's stand-ins.
pub fn fill_example_default(src: &str) -> String {
    fill_example(src, "Amount", "Date", "Name", "City")
}

/// The column a value of `kind` most likely comes from: the selected column when
/// it fits, else the first visible of that kind, else the first of it; with a
/// `pick`, first the column whose values mostly read that way (60 % of its first
/// 50 filled cells among the first 400 rows), whatever its kind.
pub fn likely_column(st: &State, kinds: &[Kind], kind: Kind, pick: Option<Pick>, selected: Option<usize>, hidden: &dyn Fn(usize) -> bool) -> Option<usize> {
    if kinds.is_empty() {
        return None;
    }
    if let Some(pick) = pick {
        let test = |v: &str| match pick { Pick::Email => fx::is_email(v), Pick::Phone => fx::parse_phone(super::value::trim(v), "33").is_some() };
        let n = st.len().min(400);
        let mut tmp = Vec::new();
        let fits = |c: usize, tmp: &mut Vec<u8>| {
            let (mut filled, mut ok) = (0, 0);
            for p in 0..n {
                st.with_cell(st.data_id(p), c, tmp, |v| {
                    let v = v.trim();
                    if v.is_empty() { return }
                    filled += 1;
                    if test(v) { ok += 1 }
                });
                if filled >= 50 { break }
            }
            filled > 0 && ok * 10 >= filled * 6
        };
        if let Some(c) = selected.filter(|&c| fits(c, &mut tmp)) {
            return Some(c);
        }
        if let Some(c) = (0..kinds.len()).find(|&c| !hidden(c) && fits(c, &mut tmp)) {
            return Some(c);
        }
    }
    if let Some(c) = selected.filter(|&c| kinds.get(c) == Some(&kind)) {
        return Some(c);
    }
    (0..kinds.len()).find(|&c| kinds[c] == kind && !hidden(c)).or_else(|| kinds.iter().position(|&k| k == kind))
}

/// The kind an entry's examples give its first argument ({N} {D} {T} met first).
pub fn arg_kind(d: &Doc) -> Option<Kind> {
    d.ex.iter().find_map(|(e, _)| {
        let i = e.find("{N}").map(|i| (i, Kind::Number));
        let j = e.find("{D}").map(|i| (i, Kind::Date));
        let k = e.find("{T").map(|i| (i, Kind::Text));
        [i, j, k].into_iter().flatten().min_by_key(|x| x.0).map(|x| x.1)
    })
}

/// What inserting an entry puts in the box: the text, and the range of it to
/// select (to be typed over). `before` is the box's text before the caret;
/// `col` the likely column for the entry's first argument ("{City}"), if any.
/// A helper's value argument (v, s, n, a, date, from) becomes that column; a
/// helper also a method is inserted as a method after a value already there.
pub fn insertion(entry: &Entry, headers: &[String], before: &str, col: Option<&str>) -> (String, Option<(usize, usize)>) {
    let d = match entry {
        Entry::Column(c) => return (format!("{{{}}}", headers[*c]), None),
        Entry::Doc(d) => *d,
    };
    let mut text = d.sig.to_string();
    let before = before.trim_end();
    let after_value = before.ends_with(|c: char| matches!(c, '}' | ']' | ')' | '"' | '\'') || c.is_alphanumeric() || c == '_');
    let mut sel = None;
    let ms = if d.fam != "Methods" { method_sig(d) } else { None };
    if let Some(ms) = &ms {
        // Method form: after a value, only .name(…); else the likely column in front, or {Column} selected.
        text = if after_value { ms.clone() } else { format!("{}{ms}", col.unwrap_or("{Column}")) };
        let p = text[text.find(ms.as_str()).unwrap_or(0)..].find('(').map(|p| p + text.find(ms.as_str()).unwrap_or(0)).unwrap_or(0);
        let e0 = text[p..].find(')').map_or(text.len(), |e| p + e);
        if !after_value && col.is_none() {
            sel = Some((0, "{Column}".len()));
        } else if e0 > p + 1 {
            let e = text[p + 1..e0].find(',').map_or(e0, |k| p + 1 + k);
            sel = Some((p + 1, e));
        }
        return (text, sel);
    }
    if d.fam == "Methods" {
        if let Some(c) = col.filter(|_| !after_value) {
            text = format!("{c}{text}");
        }
        return (text, None);
    }
    if d.fam == "Operators" {
        return (text, None);
    }
    if let Some(p) = text.find('(') {
        if text[p + 1..].starts_with(')') {
            return (text, None);
        }
        let e = text[p + 1..].find([',', ')']).map_or(text.len(), |k| p + 1 + k);
        let first = text[p + 1..e].trim();
        match col {
            Some(c) if matches!(first, "v" | "s" | "n" | "a" | "date" | "from") => {
                text = format!("{}{c}{}", &text[..p + 1], &text[e..]);
                // the next argument, if any, to be typed over
                let mut k = p + 1 + c.len();
                if text[k..].starts_with(',') {
                    k += 1;
                    while text[k..].starts_with(' ') { k += 1 }
                    if !text[k..].starts_with(')') {
                        let e = text[k..].find([',', ')']).map_or(text.len(), |x| k + x);
                        sel = Some((k, e));
                    }
                }
            }
            _ => sel = Some((p + 1, e)),
        }
    }
    (text, sel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_helper_is_documented_and_the_reverse() {
        for d in CATALOGUE.iter().filter(|d| !matches!(d.fam, "Methods" | "Operators") && d.name != "row") {
            assert!(Fx::by_name(d.name).is_some(), "{}", d.name);
        }
        for f in Fx::ALL {
            assert!(doc_of(f.name()).is_some(), "{}", f.name());
        }
        assert_eq!(method_sig(doc_of("left").unwrap()).as_deref(), Some(".left(n)"));
        assert_eq!(method_sig(doc_of("fmtDate").unwrap()).as_deref(), Some(".fmtDate(\"dd/mm/yyyy\")"));
        assert_eq!(method_sig(doc_of("sum").unwrap()), None);
    }

    #[test]
    fn autocomplete() {
        let hs = h(&["Amount", "City", "Éléonore"]);
        let c = complete("{ci", 3, &hs).unwrap();
        assert_eq!((c.start, c.end, c.items[0].insert.as_str()), (0, 3, "{City}"));
        let c = complete("x > {el}", 7, &hs).unwrap();
        assert_eq!((c.start, c.end, c.items[0].label.as_str()), (4, 8, "Éléonore"));
        assert_eq!(complete("{", 1, &hs).unwrap().items.len(), 3);
        let c = complete("num({Amount}) > 1 && cont", 25, &hs).unwrap();
        assert_eq!(c.items[0].insert, "contains(");
        assert_eq!(c.start, 21);
        let c = complete("{City}.up", 9, &hs).unwrap();
        assert_eq!(c.items[0].insert, "upper(");
        assert!(complete("{City}.", 7, &hs).unwrap().items.len() == 12);
        assert!(complete("\"abc", 4, &hs).is_none());
        assert!(complete("12", 2, &hs).is_none());
    }

    #[test]
    fn picker_search() {
        let hs = h(&["Amount", "Birth date"]);
        assert_eq!(picker("", "Dates", &hs).len(), CATALOGUE.iter().filter(|d| d.fam == "Dates").count());
        assert_eq!(picker("", "Columns", &hs).len(), 2);
        let r = picker("age", "All", &hs);
        assert!(r.iter().any(|x| x.entry == Entry::Doc(doc_of("years").unwrap())));
        assert!(!r.iter().any(|x| x.entry == Entry::Doc(doc_of("avg").unwrap())));
        assert_eq!(picker("upper", "All", &hs)[0].entry, Entry::Doc(doc_of("upper").unwrap()));
        assert_eq!(picker("birth", "All", &hs)[0].entry, Entry::Column(1));
    }

    #[test]
    fn insert() {
        let hs = h(&["City"]);
        let up = Entry::Doc(doc_of("upper").unwrap());
        assert_eq!(insertion(&up, &hs, "", Some("{City}")), ("{City}.upper()".into(), None));
        assert_eq!(insertion(&up, &hs, "", None), ("{Column}.upper()".into(), Some((0, 8))));
        assert_eq!(insertion(&up, &hs, "{City}", None), (".upper()".into(), None));
        let left = Entry::Doc(doc_of("left").unwrap());
        assert_eq!(insertion(&left, &hs, "", Some("{City}")), ("{City}.left(n)".into(), Some((12, 13))));
        let sum = Entry::Doc(doc_of("sum").unwrap());
        assert_eq!(insertion(&sum, &hs, "", Some("{Amount}")), ("sum({Amount}, b, …)".into(), Some((14, 15))));
        let ifs = Entry::Doc(doc_of("ifs").unwrap());
        assert_eq!(insertion(&ifs, &hs, "", Some("{Amount}")).1, Some((4, 9)));
        assert_eq!(insertion(&Entry::Column(0), &hs, "", None).0, "{City}");
        assert_eq!(fill_example("{T}.similarity({T2}) + {N}", "nom", "date", "ville", "x"), "{ville}.similarity({x}) + {nom}");
    }
}
