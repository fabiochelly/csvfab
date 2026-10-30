//! Duplicate rows. Rows are duplicates when ANY of several keys match (an
//! e-mail, OR a phone, OR last name + first name + postal code); a key is one
//! or more fields that must ALL match, each read through its own cleaning.
//! A key counts only when every one of its fields is filled once cleaned: two
//! contacts without an e-mail are not one contact. With no key at all, the
//! whole row is the key. Groups are chained (union-find): A and B share an
//! e-mail, B and C a phone, so A, B and C are one group.
//!
//! Each key is one parallel pass: every row's cleaned key is hashed (no
//! allocation: cleaned into a reused buffer), the (hash, position) pairs are
//! sorted, and each run of equal hashes is checked on the cleaned texts
//! themselves before its rows are joined — a hash collision never merges two
//! different keys. Memory: 16 bytes a row for the pass, 9 for the union-find.
//!
//! Cross-checks (`Rule`, `Part`): a key's part may also be several columns
//! read alike (`Part::Any`: a mobile in "phone" meets the same number in
//! another row's "mobile"), or a person's name taken from first / last / full
//! name columns as a set of words (`Part::Name`: "DUPONT Jean" = "Jean" +
//! "Dupont"), optionally by sound or with a first name reduced to its initial.
//! A row then gives several keys (one per value), all hashed in the same pass.
//!
//! Confidence: every rule has a strength (an e-mail is surer than a name +
//! city). Passes run strongest first, so the unions that build a group form a
//! maximum spanning tree (Kruskal): a group's score is its weakest link, and
//! for a pair the strengths of every rule that ties it combined. Identical
//! rows score 100. Each group keeps which rules tied it: its reasons.

use crate::group::{HashMap, Hasher};
use crate::table::{RowId, State};
use rayon::prelude::*;
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use unicode_normalization::char::decompose_canonical;

pub type CleanFn = Arc<dyn Fn(&str, &mut String) + Send + Sync>;

/// How a field is read before keys are compared.
#[derive(Clone)]
pub enum Clean {
    /// Case and spaces ignored (trimmed, runs of spaces as one).
    Loose,
    Exact,
    /// No case, accents nor symbols ("Société Générale" = "societe-generale").
    Slug,
    Digits,
    /// E.164 when it reads as a phone number (French national numbers
    /// recognised), else its digits.
    Phone,
    /// Trimmed, lower case, without a leading `mailto:`.
    Email,
    /// Same French phonetic code, and spellings at least `tol` similar (0–1).
    Sound { tol: f64 },
    /// E.164 (+33612345678), national numbers read in the country of calling
    /// code `cc`; nothing when it is no phone number (or a placeholder: 06 00 00 00 00).
    E164 { cc: u16 },
    /// An e-mail address (nothing without an @): trimmed, lower case, no
    /// `mailto:`; `canon`: its +tag ignored, and the dots of a gmail name.
    Mail { canon: bool },
    /// Anything else (the app's formulas): appends the cleaned text.
    Custom(CleanFn),
}

impl std::fmt::Debug for Clean {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Clean::Loose => write!(f, "Loose"),
            Clean::Exact => write!(f, "Exact"),
            Clean::Slug => write!(f, "Slug"),
            Clean::Digits => write!(f, "Digits"),
            Clean::Phone => write!(f, "Phone"),
            Clean::Email => write!(f, "Email"),
            Clean::Sound { tol } => write!(f, "Sound({tol})"),
            Clean::E164 { cc } => write!(f, "E164(+{cc})"),
            Clean::Mail { canon } => write!(f, "Mail({canon})"),
            Clean::Custom(_) => write!(f, "Custom"),
        }
    }
}

impl Clean {
    /// Appends `v` cleaned to `out`.
    pub fn apply(&self, v: &str, out: &mut String) {
        match self {
            Clean::Loose => loose_into(v, out),
            Clean::Exact => out.push_str(v),
            Clean::Slug => slug_into(v, out),
            Clean::Digits => out.extend(v.chars().filter(char::is_ascii_digit)),
            Clean::Phone => phone_into(v, out),
            Clean::Email => {
                let start = out.len();
                lower_into(v.trim_matches(js_space), out);
                if out[start..].starts_with("mailto:") {
                    out.replace_range(start..start + 7, "");
                }
            }
            Clean::Sound { .. } => out.push_str(&sound(v).1),
            Clean::E164 { cc } => e164_into(v, *cc, out),
            Clean::Mail { canon } => mail_into(v, *canon, out),
            Clean::Custom(f) => f(v, out),
        }
    }

    /// How sure a match on such a field alone is (0–1).
    fn strength(&self) -> f32 {
        match self {
            Clean::Email | Clean::Mail { .. } => 0.95,
            Clean::Phone | Clean::E164 { .. } => 0.85,
            Clean::Sound { .. } => 0.35,
            _ => 0.5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    pub col: usize,
    pub clean: Clean,
}

/// Keys, any of which ties two rows; each key's fields must all match.
/// No key (or only empty ones): the whole row, case and spaces ignored.
#[derive(Clone, Debug, Default)]
pub struct DupSpec {
    pub keys: Vec<Vec<Field>>,
}

impl DupSpec {
    /// The keys as rules ("key 1", "key 2"…), of strengths from their cleanings.
    pub fn rules(&self) -> Vec<Rule> {
        self.keys.iter().filter(|k| !k.is_empty()).enumerate()
            .map(|(i, k)| Rule::new(k.iter().cloned().map(Part::Field).collect(), format!("key {}", i + 1)))
            .collect()
    }
}

/// What a rule's rows must share, one of its parts.
#[derive(Clone, Debug)]
pub enum Part {
    /// One column, cleaned; must be filled.
    Field(Field),
    /// Every filled value of several columns, each through `clean`: two rows
    /// share the part when any value of one is any value of the other.
    /// `country`: for `Clean::E164`, the column naming each row's country,
    /// whose calling code then replaces the default.
    Any { cols: Vec<usize>, clean: Clean, country: Option<usize> },
    /// A person's name.
    Name(Names),
}

/// A person's name from first, last and full name columns: the set of its
/// words, without case, accents nor civility ("M.", "Mme"), in any order —
/// "DUPONT Jean" in a full name column is "Jean" + "Dupont" in two. Two words
/// at least (a first name alone is no one).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Names {
    pub cols: Vec<usize>,
    /// Also the names that sound alike (French phonetics), spellings at least
    /// this similar: a weaker match, flagged so.
    pub sound: Option<f64>,
    /// Also a first name written as its initial ("J. Dupont" ~ "Jean Dupont"),
    /// when only one first name of the file fits: a weaker match, flagged so.
    pub initials: bool,
}

/// A cross-check: rows that share all of its parts are duplicates. `label`
/// says why ("same name + phone"), `strength` (0–1) how sure it is.
#[derive(Clone, Debug)]
pub struct Rule {
    pub parts: Vec<Part>,
    pub label: String,
    pub strength: f32,
}

impl Rule {
    /// A rule of the strength its parts suggest: an e-mail .95, a phone .85, a
    /// name .6, anything else .5, sure-ness adding up (1 − Π(1 − s)).
    pub fn new(parts: Vec<Part>, label: impl Into<String>) -> Rule {
        let s = parts.iter().map(|p| match p {
            Part::Field(f) => f.clean.strength(),
            Part::Any { clean, .. } => clean.strength(),
            Part::Name(_) => 0.6,
        });
        Rule { strength: combine(s), parts, label: label.into() }
    }

    /// The cleaning a column is compared through in this rule, if it is one of its.
    pub fn clean_of(&self, col: usize) -> Option<Clean> {
        self.parts.iter().find_map(|p| match p {
            Part::Field(f) if f.col == col => Some(f.clean.clone()),
            Part::Any { cols, clean, .. } if cols.contains(&col) => Some(clean.clone()),
            Part::Name(n) if n.cols.contains(&col) => Some(Clean::Slug),
            _ => None,
        })
    }
}

/// Independent evidences together: 1 − Π(1 − s).
pub fn combine(s: impl IntoIterator<Item = f32>) -> f32 {
    1.0 - s.into_iter().fold(1.0, |acc, x| acc * (1.0 - x.clamp(0.0, 1.0)))
}

/// How the passes of `find_rules` are counted in `progress`: rows × this.
pub fn passes(rules: &[Rule]) -> usize {
    plan(rules).len().max(1)
}

/// The order groups are listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Order {
    /// By their first row.
    #[default]
    File,
    /// The least sure first: what to review.
    LeastSure,
    SurestFirst,
}

/// The groups found, of two rows or more, numbered in the order of their first row.
pub struct Dupes {
    /// RowId → group (u32::MAX: none).
    group_of: Vec<u32>,
    /// Group g's rows are members[starts[g]..starts[g + 1]], in the order given.
    starts: Vec<u32>,
    members: Vec<RowId>,
    /// Per key: how many rows it ties to another (empty for the whole row).
    pub hits: Vec<usize>,
    /// Per group: confidence 0–100.
    score: Vec<u8>,
    /// Per group: the passes that tied its rows (bit k: `labels[k]`; IDENTICAL: its rows are identical).
    why: Vec<u32>,
    /// Per pass, strongest first: why it ties rows.
    labels: Vec<String>,
    /// Per pass: the passes it implies (name + e-mail implies e-mail), not worth saying too.
    implied: Vec<u32>,
}

/// `why`'s bits beside the passes': identical rows, rows of different names,
/// names spelled differently.
const IDENTICAL: u32 = 1 << 31;
const OTHER_NAMES: u32 = 1 << 30;
const SPELLED: u32 = 1 << 29;

/// A pass's bit in `why` (passes past the 29th share the last).
#[inline]
fn bit(k: usize) -> u32 {
    1 << k.min(28)
}

const NONE: u32 = u32::MAX;
const CH: usize = 1 << 14;
/// A phonetic bucket keeps at most this many representatives: a code shared by
/// thousands of different names is not worth comparing them all.
const REPS: usize = 64;

impl Dupes {
    pub fn groups(&self) -> usize {
        self.starts.len() - 1
    }

    /// Rows in a group.
    pub fn rows(&self) -> usize {
        self.members.len()
    }

    /// What a deletion removes: every row of a group but one.
    pub fn removable(&self) -> usize {
        self.rows() - self.groups()
    }

    /// Group g's confidence, 0–100: 100 for identical rows; a pair, every
    /// rule that ties it combined; a bigger group, its weakest link.
    pub fn confidence(&self, g: u32) -> u8 {
        self.score[g as usize]
    }

    /// Why group g's rows are duplicates, surest reason first.
    pub fn reasons(&self, g: u32) -> Vec<&str> {
        let w = self.why[g as usize];
        let mut out: Vec<&str> = Vec::new();
        if w & IDENTICAL != 0 { out.push("identical rows"); }
        let said = (0..self.labels.len()).filter(|&k| w & bit(k) != 0).fold(0, |m, k| m | self.implied[k]);
        out.extend(self.labels.iter().enumerate().filter(|&(k, _)| w & bit(k) != 0 && said & bit(k) == 0).map(|(_, l)| l.as_str()));
        out.dedup();
        if w & OTHER_NAMES != 0 { out.push("different names") } else if w & SPELLED != 0 { out.push("names spelled differently") }
        out
    }

    /// The reasons in one line: "same e-mail · same name + phone".
    pub fn reason(&self, g: u32) -> String {
        self.reasons(g).join(" · ")
    }

    /// Whether group g's rows are identical, byte for byte.
    pub fn identical(&self, g: u32) -> bool {
        self.why[g as usize] & IDENTICAL != 0
    }

    /// The groups in this order (stable: file order among equals).
    pub fn order(&self, how: Order) -> Vec<u32> {
        let mut g: Vec<u32> = (0..self.groups() as u32).collect();
        match how {
            Order::File => {}
            Order::LeastSure => g.par_sort_by_key(|&g| self.score[g as usize]),
            Order::SurestFirst => g.par_sort_by_key(|&g| std::cmp::Reverse(self.score[g as usize])),
        }
        g
    }

    /// Every group's rows, groups in this order.
    pub fn side_by_side(&self, how: Order) -> Vec<RowId> {
        self.order(how).into_iter().flat_map(|g| self.members(g).iter().copied()).collect()
    }

    /// Rows in an order (a sort of the duplicates view) with each group kept together: at the
    /// place of its first row there, its rows in their order there.
    pub fn keep_together(&self, ids: &mut Vec<RowId>) {
        let mut first = vec![u32::MAX; self.groups()];
        for (p, &id) in ids.iter().enumerate() {
            if let Some(g) = self.group(id) { first[g as usize] = first[g as usize].min(p as u32); }
        }
        let keys: Vec<u64> = ids.par_iter().enumerate().map(|(p, &id)| self.group(id).map_or(p as u64, |g| first[g as usize] as u64)).collect();
        let perm = crate::radix::sort_perm(&keys);
        *ids = perm.par_iter().map(|&p| ids[p as usize]).collect();
    }

    /// How many groups score at least `lo` and below `hi`.
    pub fn count_scored(&self, lo: u8, hi: u16) -> usize {
        self.score.iter().filter(|&&s| s >= lo && (s as u16) < hi).count()
    }

    #[inline]
    pub fn group(&self, id: RowId) -> Option<u32> {
        self.group_of.get(id as usize).copied().filter(|&g| g != NONE)
    }

    pub fn members(&self, g: u32) -> &[RowId] {
        &self.members[self.starts[g as usize] as usize..self.starts[g as usize + 1] as usize]
    }

    /// The rows "remove duplicates" deletes: all but the first (or last) of each group.
    pub fn to_delete(&self, keep_last: bool) -> HashSet<RowId> {
        let mut out = HashSet::with_capacity(self.removable());
        for g in 0..self.groups() as u32 {
            let m = self.members(g);
            let keep = if keep_last { m.len() - 1 } else { 0 };
            out.extend(m.iter().enumerate().filter(|&(k, _)| k != keep).map(|(_, &id)| id));
        }
        out
    }

    /// The duplicates view: the rows of `view` that are in a group, groups side
    /// by side in group order, each group's rows in their view order.
    pub fn only_dups(&self, view: &[RowId]) -> Vec<RowId> {
        let mut out: Vec<RowId> = view.iter().copied().filter(|&id| self.group(id).is_some()).collect();
        out.par_sort_by_key(|&id| self.group_of[id as usize]);   // stable
        out
    }

    /// The columns whose filled values differ within group g, or that one row
    /// fills and another leaves empty. Values are compared as the keys read
    /// them (a key column through its first field's cleaning), the other
    /// columns ignoring case and spaces.
    pub fn diff_cols(&self, st: &State, spec: &DupSpec, g: u32) -> Vec<usize> {
        self.diff_cols_rules(st, &spec.rules(), g)
    }

    /// `diff_cols` for rules: a column of a rule read through its cleaning.
    pub fn diff_cols_rules(&self, st: &State, rules: &[Rule], g: u32) -> Vec<usize> {
        let rows = self.members(g);
        let mut tmp = Vec::new();
        let mut out = Vec::new();
        let mut set: std::collections::HashSet<String> = Default::default();
        for c in 0..st.ncols {
            let clean = rules.iter().find_map(|r| r.clean_of(c)).unwrap_or(Clean::Loose);
            let clean = &clean;
            set.clear();
            let mut empty = 0;
            for &id in rows {
                st.with_cell(id, c, &mut tmp, |v| {
                    let t = v.trim_matches(js_space);
                    if t.is_empty() { empty += 1; return; }
                    let mut k = String::new();
                    clean.apply(v, &mut k);
                    set.insert(if k.is_empty() { t.to_string() } else { k });
                });
            }
            if set.len() >= 2 || (!set.is_empty() && empty > 0) {
                out.push(c);
            }
        }
        out
    }
}

/// Row `keep` merged with `others` (in that order): its filled cells stay,
/// each empty one takes the first value the others have.
pub struct Merged {
    pub row: Vec<String>,
    /// Empty cells filled from another row.
    pub filled: usize,
    /// Filled cells kept although another row has a different value.
    pub kept: usize,
}

pub fn merge(st: &State, keep: RowId, others: &[RowId]) -> Merged {
    let mut row = Vec::new();
    st.row(keep, &mut row);
    row.resize(st.ncols.max(row.len()), String::new());
    let rest: Vec<Vec<String>> = others.iter().map(|&id| { let mut r = Vec::new(); st.row(id, &mut r); r }).collect();
    let (mut filled, mut kept) = (0, 0);
    let width = rest.iter().map(Vec::len).fold(row.len(), usize::max);
    row.resize(width, String::new());
    fn get(r: &[String], c: usize) -> &str {
        r.get(c).map_or("", |s| s.as_str())
    }
    for (c, own) in row.iter_mut().enumerate() {
        let o = own.trim_matches(js_space);
        if !o.is_empty() {
            if rest.iter().any(|r| { let v = get(r, c).trim_matches(js_space); !v.is_empty() && v != o }) { kept += 1; }
            continue;
        }
        if let Some(r) = rest.iter().find(|r| !get(r, c).trim_matches(js_space).is_empty()) {
            *own = get(r, c).to_string();
            filled += 1;
        }
    }
    Merged { row, filled, kept }
}

/// Union-find, with per root what tied its rows: the passes (bits) and the
/// weakest union that built it.
struct Uf {
    p: Vec<u32>,
    mask: Vec<u32>,
    low: Vec<u8>,
    /// Per root: its rows are identical (every union joined identical rows).
    same: Vec<bool>,
}

impl Uf {
    /// `stats`: keep the reasons and weakest links (one pass needs none: its own).
    fn new(n: usize, stats: bool) -> Uf {
        let m = if stats { n } else { 0 };
        Uf { p: (0..n as u32).collect(), mask: vec![0; m], low: vec![u8::MAX; m], same: vec![true; n] }
    }

    fn find(&mut self, mut i: u32) -> u32 {
        let p = &mut self.p;
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }

    /// The smaller position becomes the root: a group's root is its first row.
    /// A weak pass (names by sound, initials) counts as a reason only where it
    /// joins rows nothing surer had.
    /// `b`'s top bit (SAME): the two rows are identical.
    fn join(&mut self, a: u32, b: u32, bit: u32, strength: u8, weak: bool) {
        let same = b & SAME != 0;
        let (a, b) = (self.find(a), self.find(b & !SAME));
        if a == b {
            if !weak && !self.mask.is_empty() { self.mask[a as usize] |= bit }
            return;
        }
        let (r, o) = if a < b { (a, b) } else { (b, a) };
        self.p[o as usize] = r;
        let (r, o) = (r as usize, o as usize);
        // Identity is transitive: identical along a spanning tree, identical all.
        self.same[r] = self.same[r] && self.same[o] && same;
        if self.mask.is_empty() { return }
        self.mask[r] |= self.mask[o] | bit;
        self.low[r] = self.low[r].min(self.low[o]).min(strength);
    }
}

#[derive(Default)]
struct Scratch {
    tmp: Vec<u8>,
    buf: String,
    other: String,
    row: Vec<String>,
}

/// Row `id`'s cleaned key in `s.buf`; false when it does not count.
fn key_text(st: &State, id: RowId, key: Option<&[Field]>, s: &mut Scratch) -> bool {
    s.buf.clear();
    let Some(fields) = key else { return whole_row(st, id, s) };
    for f in fields {
        s.buf.push('\u{1}');
        let start = s.buf.len();
        let buf = &mut s.buf;
        st.with_cell(id, f.col, &mut s.tmp, |v| f.clean.apply(v, buf));
        if s.buf.len() == start {
            return false;
        }
    }
    true
}

/// Every cell, case and spaces ignored; false when all are empty.
fn whole_row(st: &State, id: RowId, s: &mut Scratch) -> bool {
    fn add(buf: &mut String, n: &mut usize, v: &str) {
        if *n > 0 { buf.push('\u{1}') }
        loose_into(v, buf);
        *n += 1;
    }
    let (buf, mut n) = (&mut s.buf, 0);
    st.visit(id, |_, v| add(buf, &mut n, v));
    // A short record and the same one padded with empty fields are one row.
    while n < st.ncols { add(buf, &mut n, "") }
    buf.bytes().any(|b| b != 1)
}

/// The duplicate groups among `ids` (all the table's rows, in file order, for
/// the app's behaviour: filtered or not, every row is checked). None if cancelled.
/// `progress` counts rows read, once per key.
pub fn find(st: &State, ids: &[RowId], spec: &DupSpec, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Dupes> {
    find_rules(st, ids, &spec.rules(), &[], cancel, progress)
}

/// How a cross-check pass reads names.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Via {
    Plain,
    Sound(f64),
    Initials,
}

enum PassKind<'a> {
    Whole,
    /// Fields only: the plain passes of before, tuned for one key a row.
    Fields(Vec<Field>),
    SoundFields(Vec<Field>),
    Cross(&'a [Part], Via),
}

struct Pass<'a> {
    rule: Option<usize>,
    kind: PassKind<'a>,
    strength: u8,
    weak: bool,
    label: String,
}

/// The passes of the rules, strongest first (no rule: the whole row).
fn plan(rules: &[Rule]) -> Vec<Pass<'_>> {
    let mut v = Vec::new();
    for (r, rule) in rules.iter().enumerate().filter(|(_, r)| !r.parts.is_empty()) {
        let s = (rule.strength.clamp(0.0, 1.0) * 100.0).round();
        let pass = |kind, f: f32, weak, label: String| Pass { rule: Some(r), kind, strength: (s * f).round() as u8, weak, label };
        let fields: Option<Vec<Field>> = rule.parts.iter().map(|p| if let Part::Field(f) = p { Some(f.clone()) } else { None }).collect();
        match fields {
            Some(f) if f.iter().any(|f| matches!(f.clean, Clean::Sound { .. })) => v.push(pass(PassKind::SoundFields(f), 1.0, false, rule.label.clone())),
            Some(f) => v.push(pass(PassKind::Fields(f), 1.0, false, rule.label.clone())),
            None => {
                v.push(pass(PassKind::Cross(&rule.parts, Via::Plain), 1.0, false, rule.label.clone()));
                let names = rule.parts.iter().find_map(|p| if let Part::Name(n) = p { Some(n) } else { None });
                if let Some(tol) = names.and_then(|n| n.sound) {
                    v.push(pass(PassKind::Cross(&rule.parts, Via::Sound(tol)), 0.6, true, format!("{} (name sounds alike)", rule.label)));
                }
                if names.is_some_and(|n| n.initials) {
                    v.push(pass(PassKind::Cross(&rule.parts, Via::Initials), 0.75, true, format!("{} (first name as an initial)", rule.label)));
                }
            }
        }
    }
    if v.is_empty() {
        v.push(Pass { rule: None, kind: PassKind::Whole, strength: 100, weak: false, label: "same row (case and spaces ignored)".into() });
    }
    v.sort_by_key(|p| std::cmp::Reverse(p.strength));   // stable
    v
}

/// `find` for cross-check rules: rows sharing every part of any rule are one
/// group. `names`: the columns of a person's name (else those of the rules'
/// names), against which every group is cross-checked — a phone shared by
/// rows of different names is a switchboard, not one person: to review.
pub fn find_rules(st: &State, ids: &[RowId], rules: &[Rule], names: &[usize], cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Dupes> {
    let n = ids.len();
    assert!(n < SAME as usize);
    let passes = plan(rules);
    let timing = std::env::var_os("CSVFAB_DUPES_TIMING").is_some();
    let t0 = std::time::Instant::now();
    let single = passes.len() == 1;
    let mut uf = Uf::new(n, !single);
    let words = n.div_ceil(64);
    let mut tied: Vec<Vec<u64>> = vec![Vec::new(); rules.len()];
    for (k, pass) in passes.iter().enumerate() {
        let mut pairs = match &pass.kind {
            PassKind::Whole => exact_pass(st, ids, None, cancel, progress)?,
            PassKind::Fields(f) => exact_pass(st, ids, Some(f), cancel, progress)?,
            PassKind::SoundFields(f) => sound_pass(st, ids, f, cancel, progress)?,
            PassKind::Cross(parts, via) => cross_pass(st, ids, parts, *via, cancel, progress)?,
        };
        if !matches!(pass.kind, PassKind::Whole | PassKind::Fields(_)) { mark_same(st, ids, &mut pairs) }
        // (One pass ties every row of every group: counted below.)
        if let Some(r) = pass.rule.filter(|_| !single) {
            let t = &mut tied[r];
            if t.is_empty() { t.resize(words, 0) }
            for &(a, b) in pairs.iter().flatten() {
                let b = b & !SAME;
                t[a as usize / 64] |= 1 << (a % 64);
                t[b as usize / 64] |= 1 << (b % 64);
            }
        }
        let b = bit(k);
        let np: usize = pairs.iter().map(Vec::len).sum();
        for (x, y) in pairs.into_iter().flatten() { uf.join(x, y, b, pass.strength, pass.weak) }
        if timing { eprintln!("dupes: pass {k} ({}) {np} pairs, at {:.0?}", pass.label, t0.elapsed()) }
    }
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut hits: Vec<usize> = if passes.iter().any(|p| p.rule.is_some()) { tied.iter().map(|t| t.iter().map(|w| w.count_ones() as usize).sum()).collect() } else { Vec::new() };
    // Number the groups of two rows or more in the order of their roots (first rows).
    // A parent is always before its child (unions keep the smaller root, halving
    // only shortens): in file order each row's root is its parent's, known already.
    let mut root = std::mem::take(&mut uf.p);
    let mut size = vec![0u32; n];
    for i in 0..n {
        let p = root[i] as usize;
        if p != i { root[i] = root[p] }
        size[root[i] as usize] += 1;
    }
    let mut gno = vec![NONE; n];
    let mut starts = vec![0u32];
    let (mut score, mut why, mut same) = (Vec::new(), Vec::new(), Vec::new());
    let strength: Vec<u8> = passes.iter().map(|p| p.strength).collect();
    for i in 0..n {
        if root[i] == i as u32 && size[i] >= 2 {
            gno[i] = starts.len() as u32 - 1;
            let last = *starts.last().unwrap();
            starts.push(last + size[i]);
            let m = if single { 1 } else { uf.mask[i] };
            let s = if single {
                strength[0]
            } else if size[i] == 2 {
                // One pair: every rule that ties it adds its evidence.
                let all = strength.iter().enumerate().filter(|&(k, _)| m & bit(k) != 0).map(|(_, &s)| s as f32 / 100.0);
                (combine(all) * 100.0).round().min(99.0) as u8
            } else {
                uf.low[i]
            };
            let s = s.min(99);
            score.push(s.max(1));
            why.push(m);
            same.push(uf.same[i]);
        }
    }
    if single && !hits.is_empty() {
        let r = passes[0].rule.unwrap();
        hits[r] = *starts.last().unwrap() as usize;
    }
    let mut fill = starts.clone();
    let mut members = vec![0 as RowId; *starts.last().unwrap() as usize];
    let max_id = ids.iter().copied().max().map_or(0, |m| m as usize + 1);
    let mut group_of = vec![NONE; max_id];
    for i in 0..n {
        let g = gno[root[i] as usize];
        if g == NONE { continue }
        members[fill[g as usize] as usize] = ids[i];
        fill[g as usize] += 1;
        group_of[ids[i] as usize] = g;
    }
    let weak = passes.iter().enumerate().filter(|(_, p)| p.weak).fold(0, |m, (k, _)| m | bit(k));
    if timing { eprintln!("dupes: groups numbered at {:.0?}", t0.elapsed()) }
    let implied = passes.iter().map(|p| passes.iter().enumerate().filter(|(_, q)| implies(p, q)).fold(0, |m, (k, _)| m | bit(k))).collect();
    let mut d = Dupes { group_of, starts, members, hits, score, why, labels: passes.into_iter().map(|p| p.label).collect(), implied };
    // Identical rows are sure whatever tied them; rows of different names are not.
    let mut name_cols: Vec<usize> = names.to_vec();
    if name_cols.is_empty() {
        name_cols = rules.iter().flat_map(|r| &r.parts).filter_map(|p| if let Part::Name(n) = p { Some(n.cols.iter().copied()) } else { None }).flatten().collect();
        name_cols.sort_unstable();
        name_cols.dedup();
    }
    let checks: Vec<u8> = if name_cols.is_empty() { Vec::new() } else {
        (0..d.groups() as u32).into_par_iter().map_init(XScratch::default, |x, g| name_conflict(st, d.members(g), &name_cols, x)).collect()
    };
    if timing { eprintln!("dupes: identical rows and names checked at {:.0?}", t0.elapsed()) }
    for g in 0..d.groups() {
        let names = checks.get(g).copied().unwrap_or(0);
        if same[g] {
            d.score[g] = 100;
            d.why[g] |= IDENTICAL;
        } else if names == 2 {
            d.score[g] = d.score[g].min(30);
            d.why[g] |= OTHER_NAMES;
        } else if names == 1 && d.why[g] & weak == 0 {
            // (A weak pass says so already: by sound, by initial.)
            d.score[g] = (d.score[g] as f32 * 0.9).round() as u8;
            d.why[g] |= SPELLED;
        }
    }
    Some(d)
}

/// Whether pass `p` ties only rows pass `q` ties too: its rule holds all of `q`'s parts (or is `q`'s).
fn implies(p: &Pass, q: &Pass) -> bool {
    let parts = |x: &Pass| -> Vec<String> {
        match &x.kind {
            PassKind::Whole => vec!["whole".into()],
            PassKind::Fields(f) | PassKind::SoundFields(f) => f.iter().map(|f| format!("{f:?}")).collect(),
            PassKind::Cross(parts, _) => parts.iter().map(|p| format!("{p:?}")).collect(),
        }
    };
    if std::ptr::eq(p, q) || p.weak || q.weak { return false }
    let (a, b) = (parts(p), parts(q));
    a.len() > b.len() && b.iter().all(|x| a.contains(x))
}

/// How the names of a group's rows agree (rows without a name aside): 0 alike,
/// 1 spelled differently (Jérémy / Jérémi, J. Dupont), 2 different people.
fn name_conflict(st: &State, rows: &[RowId], cols: &[usize], x: &mut XScratch) -> u8 {
    let names = Names { cols: cols.to_vec(), ..Default::default() };
    let mut first: Option<String> = None;
    let mut out = 0;
    for &id in rows {
        x.text.clear();
        if x.cands.is_empty() { x.cands.push(Vec::new()) }
        x.cands[0].clear();
        name_cands(st, id, &names, Via::Plain, x, 0);
        let Some(c) = x.cands[0].first().copied() else { continue };
        let k = range(&x.text, c.k);
        match &first {
            None => first = Some(k.to_string()),
            Some(f) if f == k => {}
            Some(f) => {
                if !names_alike(f, k) { return 2 }
                out = 1;
            }
        }
    }
    out
}

/// Two names (sorted words) that may be one person's: close spellings, or
/// the same once a first name is reduced to its initial.
fn names_alike(a: &str, b: &str) -> bool {
    // Word by word (sorted, they line up): Jérémy / Jérémi, Bonamie / Bonami.
    let (wa, wb): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    let close = |x: &str, y: &str| similarity(&x.chars().collect::<Vec<_>>(), &y.chars().collect::<Vec<_>>()) >= 0.75;
    if wa.len() == wb.len() && wa.iter().zip(&wb).all(|(x, y)| close(x, y)) { return true }
    let words = |s: &str| -> (Vec<String>, Vec<char>) {
        let (mut full, mut inits) = (Vec::new(), Vec::new());
        for w in s.split(' ') { if initial(w) { inits.extend(w.chars()) } else { full.push(w.to_string()) } }
        (full, inits)
    };
    let ((fa, ia), (fb, ib)) = (words(a), words(b));
    // The side with an initial: its full words are the other's but one, which the initial begins.
    let fits = |f1: &[String], i1: &[char], f2: &[String]| {
        i1.len() == 1 && !f1.is_empty() && f1.iter().all(|w| f2.contains(w)) && f2.len() == f1.len() + 1
            && f2.iter().filter(|w| !f1.contains(w)).all(|w| w.starts_with(i1[0]))
    };
    fits(&fa, &ia, &fb) || fits(&fb, &ib, &fa)
}


/// The pairs of rows (positions) a pass ties, in pieces as its threads found
/// them; SAME on the second when the two rows are identical.
type Pairs = Vec<Vec<(u32, u32)>>;
const SAME: u32 = 1 << 31;

/// Whether two rows are the same, byte for byte (their records, or their cells once edited).
fn same_rows(st: &State, a: RowId, b: RowId, s: &mut Scratch) -> bool {
    if st.pristine(a) && st.pristine(b) { return st.base.record(a as usize) == st.base.record(b as usize) }
    st.row(a, &mut s.row);
    let first = std::mem::take(&mut s.row);
    st.row(b, &mut s.row);
    let same = s.row == first;
    s.row = first;
    same
}

/// SAME set on the pairs of identical rows (the passes that did not while reading them).
fn mark_same(st: &State, ids: &[RowId], pairs: &mut Pairs) {
    pairs.par_iter_mut().for_each_init(Scratch::default, |s, v| {
        for (a, b) in v.iter_mut() {
            if same_rows(st, ids[*a as usize], ids[*b as usize], s) { *b |= SAME }
        }
    });
}

/// Runs of one hash, two entries or more: the candidates.
fn runs(hs: &[(u64, u32)]) -> Vec<std::ops::Range<usize>> {
    let mut runs = Vec::new();
    let mut a = 0;
    for b in 1..=hs.len() {
        if b == hs.len() || hs[b].0 != hs[a].0 {
            if b - a >= 2 { runs.push(a..b) }
            a = b;
        }
    }
    runs
}

/// One key (None: the whole row) through hashes: the pairs of rows (positions) it ties.
fn exact_pass(st: &State, ids: &[RowId], key: Option<&[Field]>, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Pairs> {
    let hasher = Hasher::default();
    let mut hs: Vec<(u64, u32)> = ids
        .par_chunks(CH)
        .enumerate()
        .map_init(Scratch::default, |s, (ci, chunk)| {
            let mut out = Vec::with_capacity(chunk.len());
            if cancel.load(Ordering::Relaxed) { return out; }
            for (k, &id) in chunk.iter().enumerate() {
                if key_text(st, id, key, s) {
                    out.push((hasher.hash_one(s.buf.as_bytes()), (ci * CH + k) as u32));
                }
            }
            if let Some(p) = progress { p.fetch_add(chunk.len(), Ordering::Relaxed); }
            out
        })
        .flat_map_iter(|v| v)
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    crate::radix::sort_pairs(&mut hs);
    let runs = runs(&hs);
    let joined: Vec<Vec<(u32, u32)>> = runs
        .par_iter()
        .map_init(Scratch::default, |s, r| {
            let run = &hs[r.clone()];
            let first = run[0].1;
            key_text(st, ids[first as usize], key, s);
            std::mem::swap(&mut s.buf, &mut s.other);
            let mut pairs = Vec::with_capacity(run.len() - 1);
            let mut odd = Vec::new();
            for &(_, p) in &run[1..] {
                key_text(st, ids[p as usize], key, s);
                // (The record was just read: comparing it whole costs nothing more.)
                if s.buf == s.other { pairs.push((p, first | if same_rows(st, ids[p as usize], ids[first as usize], s) { SAME } else { 0 })) } else { odd.push((std::mem::take(&mut s.buf), p)) }
            }
            odd_classes(odd, &mut pairs);
            pairs
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    Some(joined)
}

/// A true hash collision: the rows of other texts form their own classes.
fn odd_classes(odd: Vec<(String, u32)>, pairs: &mut Vec<(u32, u32)>) {
    if odd.is_empty() { return }
    let mut seen: HashMap<String, u32> = HashMap::default();
    for (t, p) in odd {
        match seen.get(&t) {
            Some(&f) => pairs.push((p, f)),
            None => { seen.insert(t, p); }
        }
    }
}

/// A key with a "sounds like" field: same codes, then spellings close enough to
/// one of the bucket's representatives, in row order. The keys are hashed
/// first (the phonetics of a value cached: names repeat); only the rows of a
/// shared hash have their spellings read.
fn sound_pass(st: &State, ids: &[RowId], key: &[Field], cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Pairs> {
    let tols: Vec<f64> = key.iter().filter_map(|f| if let Clean::Sound { tol } = f.clean { Some(tol) } else { None }).collect();
    let hasher = Hasher::default();
    // The row's key (codes and other fields) in s.buf; each sound field's transcription through `said`.
    let keyed = |id: RowId, s: &mut Scratch, said: &mut dyn FnMut(&str)| -> bool {
        s.buf.clear();
        for f in key {
            s.buf.push('\u{1}');
            let start = s.buf.len();
            let buf = &mut s.buf;
            st.with_cell(id, f.col, &mut s.tmp, |v| match f.clean {
                Clean::Sound { .. } => sound_cached(v, |t, c| { buf.push_str(c); said(t) }),
                _ => f.clean.apply(v, buf),
            });
            if s.buf.len() == start { return false }
        }
        true
    };
    let mut hs: Vec<(u64, u32)> = ids
        .par_chunks(CH)
        .enumerate()
        .map_init(Scratch::default, |s, (ci, chunk)| {
            let mut out = Vec::with_capacity(chunk.len());
            if cancel.load(Ordering::Relaxed) { return out; }
            for (k, &id) in chunk.iter().enumerate() {
                if keyed(id, s, &mut |_| {}) { out.push((hasher.hash_one(s.buf.as_bytes()), (ci * CH + k) as u32)) }
            }
            if let Some(p) = progress { p.fetch_add(chunk.len(), Ordering::Relaxed); }
            out
        })
        .flat_map_iter(|v| v)
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    crate::radix::sort_pairs(&mut hs);
    let runs = runs(&hs);
    let joined: Vec<Vec<(u32, u32)>> = runs
        .par_iter()
        .map_init(Scratch::default, |s, r| {
            // The buckets of this hash (one, but for a collision), rows in order.
            let mut buckets: HashMap<String, Vec<(u32, Vec<Vec<char>>)>> = HashMap::default();
            let mut pairs = Vec::new();
            for &(_, i) in &hs[r.clone()] {
                let mut said: Vec<Vec<char>> = Vec::new();
                if !keyed(ids[i as usize], s, &mut |t| said.push(t.chars().collect())) { continue }
                let reps = buckets.entry(std::mem::take(&mut s.buf)).or_default();
                if reps.is_empty() { reps.push((i, said)); continue; }
                let hit = reps.iter().find(|(_, t2)| t2.iter().zip(&said).zip(&tols).all(|((x, y), &tol)| similarity(x, y) >= tol)).map(|r| r.0);
                match hit {
                    Some(j) => pairs.push((i, j)),
                    None if reps.len() < REPS => reps.push((i, said)),
                    None => {}
                }
            }
            pairs
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    Some(joined)
}

/// `sound(v)`, cached per thread: `f(transcription, code)`.
fn sound_cached(v: &str, f: impl FnOnce(&str, &str)) {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, (String, String)>> = Default::default();
    }
    CACHE.with(|m| {
        let mut m = m.borrow_mut();
        if !m.contains_key(v) {
            if m.len() > 200_000 { m.clear() }
            m.insert(v.to_string(), sound(v));
        }
        let (t, c) = &m[v];
        f(t, c)
    })
}

// ---- Cross-check passes -------------------------------------------------------

/// A part's candidate value for a row: its key and its spelling (names by
/// sound: the transcriptions; initials: the first name the initial stands for),
/// as ranges of `XScratch::text`.
#[derive(Clone, Copy)]
struct Cand {
    k: (u32, u32),
    a: (u32, u32),
}

#[derive(Default)]
struct XScratch {
    tmp: Vec<u8>,
    text: String,
    cands: Vec<Vec<Cand>>,
    key: String,
    aux: String,
    slug: String,
    pre: String,
    toks: Vec<(u32, u32)>,
    hs: Vec<u64>,
    idx: Vec<usize>,
}

/// Titles and civilities, not names.
const CIVIL: [&str; 16] = ["mr", "mme", "mlle", "mrs", "ms", "miss", "dr", "pr", "prof", "monsieur", "madame", "mademoiselle", "mister", "sir", "melle", "mme."];
/// A row gives at most this many keys (values of its parts multiplied).
const MAX_KEYS: usize = 64;

thread_local! {
    /// A word's French (transcription, code): names repeat, the phonetics need not.
    static SOUNDS: std::cell::RefCell<HashMap<String, (String, String)>> = Default::default();
    /// A country cell's calling code.
    static COUNTRIES: std::cell::RefCell<HashMap<String, Option<u16>>> = Default::default();
}

fn range(s: &str, r: (u32, u32)) -> &str {
    &s[r.0 as usize..r.1 as usize]
}

/// Calls `f(key, spelling)` for every key of row `id` under these parts.
fn emit(st: &State, id: RowId, parts: &[Part], via: Via, s: &mut XScratch, f: &mut dyn FnMut(&str, &str)) {
    s.text.clear();
    if s.cands.len() < parts.len() { s.cands.resize_with(parts.len(), Vec::new) }
    for (pi, part) in parts.iter().enumerate() {
        s.cands[pi].clear();
        match part {
            Part::Field(fl) => {
                let start = s.text.len();
                let (tmp, text) = (&mut s.tmp, &mut s.text);
                st.with_cell(id, fl.col, tmp, |v| fl.clean.apply(v, text));
                let end = s.text.len() as u32;
                if end as usize > start { s.cands[pi].push(Cand { k: (start as u32, end), a: (end, end) }) }
            }
            Part::Any { cols, clean, country } => {
                let cc = match (clean, country) {
                    (Clean::E164 { cc }, Some(c)) => row_cc(st, id, *c, &mut s.tmp, *cc),
                    (Clean::E164 { cc }, None) => *cc,
                    _ => 0,
                };
                for &c in cols {
                    let start = s.text.len();
                    let (tmp, text) = (&mut s.tmp, &mut s.text);
                    st.with_cell(id, c, tmp, |v| match clean {
                        Clean::E164 { .. } => e164_into(v, cc, text),
                        _ => clean.apply(v, text),
                    });
                    let end = s.text.len();
                    if end == start { continue }
                    // The same value in two of the columns: one key.
                    if s.cands[pi].iter().any(|x| range(&s.text, x.k) == &s.text[start..end]) { s.text.truncate(start); continue }
                    s.cands[pi].push(Cand { k: (start as u32, end as u32), a: (end as u32, end as u32) });
                }
            }
            Part::Name(nm) => name_cands(st, id, nm, via, s, pi),
        }
        if s.cands[pi].is_empty() { return }
    }
    // Every combination of the parts' values.
    let (mut key, mut aux) = (std::mem::take(&mut s.key), std::mem::take(&mut s.aux));
    s.idx.clear();
    s.idx.resize(parts.len(), 0);
    for _ in 0..MAX_KEYS {
        key.clear();
        aux.clear();
        for pi in 0..parts.len() {
            if pi > 0 { key.push('\u{1}') }
            let c = s.cands[pi][s.idx[pi]];
            key.push_str(range(&s.text, c.k));
            aux.push_str(range(&s.text, c.a));
        }
        f(&key, &aux);
        // Next combination (an odometer).
        let mut pi = 0;
        loop {
            if pi == parts.len() { break }
            s.idx[pi] += 1;
            if s.idx[pi] < s.cands[pi].len() { break }
            s.idx[pi] = 0;
            pi += 1;
        }
        if pi == parts.len() { break }
    }
    s.key = key;
    s.aux = aux;
}

/// The calling code of the row's country, else `default`.
fn row_cc(st: &State, id: RowId, col: usize, tmp: &mut Vec<u8>, default: u16) -> u16 {
    st.with_cell(id, col, tmp, |v| {
        let v = v.trim_matches(js_space);
        if v.is_empty() { return default }
        COUNTRIES.with(|m| {
            let mut m = m.borrow_mut();
            if let Some(c) = m.get(v) { return c.unwrap_or(default) }
            if m.len() > 4096 { m.clear() }
            let c = crate::kinds::calling_code(v);
            m.insert(v.to_string(), c);
            c.unwrap_or(default)
        })
    })
}

/// A word of one letter: an initial.
fn initial(w: &str) -> bool {
    w.chars().nth(1).is_none()
}

/// "M. Dupont": M. is Monsieur, not an initial.
fn strip_monsieur(v: &str) -> &str {
    let t = v.trim_start_matches(js_space);
    match t.strip_prefix("M.").or_else(|| t.strip_prefix("M ")) {
        Some(r) if r.chars().any(char::is_alphabetic) => r,
        _ => v,
    }
}

/// The name's keys of row `id` as candidates of part `pi`.
fn name_cands(st: &State, id: RowId, nm: &Names, via: Via, s: &mut XScratch, pi: usize) {
    s.slug.clear();
    for &c in &nm.cols {
        let (tmp, slug, pre) = (&mut s.tmp, &mut s.slug, &mut s.pre);
        st.with_cell(id, c, tmp, |v| {
            let v = strip_monsieur(v);
            slug.push(' ');
            // ç is s to the phonetics (the slug would make it a c, read k).
            if matches!(via, Via::Sound(_)) && v.contains(['ç', 'Ç']) {
                pre.clear();
                pre.extend(v.chars().map(|c| if c == 'ç' || c == 'Ç' { 's' } else { c }));
                slug_into(pre, slug);
            } else {
                slug_into(v, slug);
            }
        });
    }
    // Its words, sorted, each once, without civilities nor numbers.
    s.toks.clear();
    let b = s.slug.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b' ' { i += 1; continue }
        let w0 = i;
        while i < b.len() && b[i] != b' ' { i += 1 }
        let w = &s.slug[w0..i];
        if CIVIL.contains(&w) || w.bytes().all(|c| c.is_ascii_digit()) { continue }
        s.toks.push((w0 as u32, i as u32));
    }
    let slug = &s.slug;
    s.toks.sort_unstable_by(|x, y| range(slug, *x).cmp(range(slug, *y)));
    s.toks.dedup_by(|x, y| range(slug, *x) == range(slug, *y));
    let toks = &s.toks;
    if toks.len() < 2 || toks.iter().all(|&t| initial(range(slug, t))) { return }
    let (text, cands) = (&mut s.text, &mut s.cands[pi]);
    let mark = |text: &String| text.len() as u32;
    match via {
        Via::Plain => {
            let k0 = mark(text);
            for (k, &t) in toks.iter().enumerate() {
                if k > 0 { text.push(' ') }
                text.push_str(range(slug, t));
            }
            let k1 = mark(text);
            cands.push(Cand { k: (k0, k1), a: (k1, k1) });
        }
        Via::Sound(_) => {
            let toks = &toks[..toks.len().min(8)];
            SOUNDS.with(|m| {
                let mut m = m.borrow_mut();
                if m.len() > 200_000 { m.clear() }
                for &t in toks {
                    let w = range(slug, t);
                    if !m.contains_key(w) { m.insert(w.to_string(), fr_sound(w)); }
                }
                // Words in the order of their codes, so that the spellings line up.
                let mut said: [(&str, &str); 8] = [("", ""); 8];
                for (k, &t) in toks.iter().enumerate() {
                    let (tr, code) = &m[range(slug, t)];
                    said[k] = (code.as_str(), tr.as_str());
                }
                let said = &mut said[..toks.len()];
                said.sort_unstable();
                let k0 = mark(text);
                for (k, (code, _)) in said.iter().enumerate() {
                    if k > 0 { text.push(' ') }
                    text.push_str(code);
                }
                let (k1, a0) = (mark(text), mark(text));
                for (k, (_, tr)) in said.iter().enumerate() {
                    if k > 0 { text.push(' ') }
                    text.push_str(tr);
                }
                let a1 = mark(text);
                cands.push(Cand { k: (k0, k1), a: (a0, a1) });
            });
        }
        Via::Initials => {
            let inits = toks.iter().filter(|&&t| initial(range(slug, t))).count();
            // "J. Dupont": its initial and the other words; no spelling.
            // "Jean Dupont": each word as an initial, the word as the spelling.
            let put = |text: &mut String, cands: &mut Vec<Cand>, which: (u32, u32), spell: bool| {
                let k0 = mark(text);
                text.extend(range(slug, which).chars().next());
                text.push('\u{2}');
                let mut first = true;
                for &t in toks.iter().filter(|&&t| t != which) {
                    if !first { text.push(' ') }
                    first = false;
                    text.push_str(range(slug, t));
                }
                let (k1, a0) = (mark(text), mark(text));
                if spell { text.push_str(range(slug, which)) }
                let a1 = mark(text);
                cands.push(Cand { k: (k0, k1), a: (a0, a1) });
            };
            match inits {
                1 => put(text, cands, *toks.iter().find(|&&t| initial(range(slug, t))).unwrap(), false),
                0 => for &t in toks.iter() { put(text, cands, t, true) },
                _ => {}
            }
        }
    }
}

/// A rule with several values a row (any of some columns, a name's words)
/// through hashes: every key of every row hashed, runs of equal hashes checked
/// on the texts, then by spelling (sounds) or for one meaning (initials).
fn cross_pass(st: &State, ids: &[RowId], parts: &[Part], via: Via, cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Pairs> {
    let hasher = Hasher::default();
    let mut hs: Vec<(u64, u32)> = ids
        .par_chunks(CH)
        .enumerate()
        .map_init(XScratch::default, |s, (ci, chunk)| {
            let mut out = Vec::with_capacity(chunk.len());
            if cancel.load(Ordering::Relaxed) { return out; }
            let mut row = std::mem::take(&mut s.hs);
            for (k, &id) in chunk.iter().enumerate() {
                row.clear();
                emit(st, id, parts, via, s, &mut |key, _| row.push(hasher.hash_one(key.as_bytes())));
                if row.len() > 1 { row.sort_unstable(); row.dedup(); }
                out.extend(row.iter().map(|&h| (h, (ci * CH + k) as u32)));
            }
            s.hs = row;
            if let Some(p) = progress { p.fetch_add(chunk.len(), Ordering::Relaxed); }
            out
        })
        .flat_map_iter(|v| v)
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    crate::radix::sort_pairs(&mut hs);
    let runs = runs(&hs);
    let joined: Vec<Vec<(u32, u32)>> = runs
        .par_iter()
        .map_init(XScratch::default, |s, r| {
            if cancel.load(Ordering::Relaxed) { return Vec::new(); }
            let run = &hs[r.clone()];
            let h = run[0].0;
            let mut pairs = Vec::new();
            if via == Via::Plain {
                // Compared with the first row's key in place: no text kept but the odd ones.
                let first = run[0].1;
                let mut reference = String::new();
                emit(st, ids[first as usize], parts, via, s, &mut |k, _| if reference.is_empty() && hasher.hash_one(k.as_bytes()) == h { reference = k.to_string() });
                let mut odd = Vec::new();
                for &(_, p) in &run[1..] {
                    let (mut same, mut other) = (None, None);
                    emit(st, ids[p as usize], parts, via, s, &mut |k, _| {
                        if same.is_none() && hasher.hash_one(k.as_bytes()) == h {
                            same = Some(k == reference);
                            if k != reference { other = Some(k.to_string()) }
                        }
                    });
                    match (same, other) {
                        (Some(true), _) => pairs.push((p, first)),
                        (_, Some(o)) => odd.push((o, p)),
                        _ => {}
                    }
                }
                odd_classes(odd, &mut pairs);
                return pairs;
            }
            // Sounds, initials: each row's key and spelling, rows of one key together.
            let mut rows: Vec<(String, String, u32)> = Vec::with_capacity(run.len());
            for &(_, p) in run {
                let mut got = None;
                emit(st, ids[p as usize], parts, via, s, &mut |k, a| if got.is_none() && hasher.hash_one(k.as_bytes()) == h { got = Some((k.to_string(), a.to_string())) });
                if let Some((k, a)) = got { rows.push((k, a, p)) }
            }
            rows.sort();
            for class in rows.chunk_by(|x, y| x.0 == y.0) {
                match via {
                    Via::Sound(tol) => spelled_alike(class, tol, &mut pairs),
                    _ => {
                        // An initial stands for one first name of the file at most here.
                        let (inits, full): (Vec<_>, Vec<_>) = class.iter().partition(|r| r.1.is_empty());
                        if inits.is_empty() || full.is_empty() || full.iter().any(|r| r.1 != full[0].1) { continue }
                        let first = class.iter().map(|r| r.2).min().unwrap();
                        pairs.extend(class.iter().filter(|r| r.2 != first).map(|r| (r.2, first)));
                    }
                }
            }
            pairs
        })
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    Some(joined)
}

/// Rows of one phonetic key: joined when their spellings are `tol` similar to
/// one of the first representatives', word by word (words line up: sorted by
/// code), in row order — a first name in common does not excuse another surname.
fn spelled_alike(class: &[(String, String, u32)], tol: f64, pairs: &mut Vec<(u32, u32)>) {
    let mut order: Vec<&(String, String, u32)> = class.iter().collect();
    order.sort_by_key(|r| r.2);
    let mut same: HashMap<&str, u32> = HashMap::default();
    let mut reps: Vec<(Vec<Vec<char>>, u32)> = Vec::new();
    let close = |a: &[Vec<char>], b: &[Vec<char>]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| similarity(x, y) >= tol);
    for r in order {
        if let Some(&j) = same.get(r.1.as_str()) { pairs.push((r.2, j)); continue }
        let said: Vec<Vec<char>> = r.1.split(' ').map(|w| w.chars().collect()).collect();
        match reps.iter().find(|(t, _)| close(t, &said)) {
            Some(&(_, j)) => { pairs.push((r.2, j)); same.insert(&r.1, j); }
            None => {
                same.insert(&r.1, r.2);
                if reps.len() < REPS { reps.push((said, r.2)) }
            }
        }
    }
}

// ---- What the columns hold (the cross-checks the dialog suggests) -------------

/// The columns a duplicate search can cross-check, found from their titles
/// (French and English) and the shapes of their values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roles {
    pub first: Option<usize>,
    pub last: Option<usize>,
    /// Columns holding the whole name ("Jean DUPONT"), checked against the first and last names.
    pub full: Vec<usize>,
    /// A person's own addresses (not who created the row, nor a billing address).
    pub emails: Vec<usize>,
    pub phones: Vec<usize>,
    pub company: Option<usize>,
    pub city: Option<usize>,
    pub postal: Option<usize>,
    pub country: Option<usize>,
    /// The calling code most rows point to (their country, their numbers): 33 when nothing says.
    pub cc: u16,
}

impl Roles {
    /// The name columns: first, last, full.
    pub fn names(&self) -> Vec<usize> {
        self.first.into_iter().chain(self.last).chain(self.full.iter().copied()).collect()
    }

    /// Whether a person's name can be read: first and last, or a full name.
    pub fn has_name(&self) -> bool {
        (self.first.is_some() && self.last.is_some()) || !self.full.is_empty()
    }
}

/// What a part compares, in words: "name", "phone", "e-mail", or its column's title.
pub fn part_label(p: &Part, headers: &[String]) -> String {
    let title = |c: usize| headers.get(c).cloned().unwrap_or_else(|| format!("column {}", c + 1));
    match p {
        Part::Name(_) => "name".into(),
        Part::Any { clean: Clean::E164 { .. } | Clean::Phone, .. } => "phone".into(),
        Part::Any { clean: Clean::Mail { .. } | Clean::Email, .. } => "e-mail".into(),
        Part::Any { cols, .. } => cols.iter().map(|&c| title(c)).collect::<Vec<_>>().join(" / "),
        Part::Field(f) => title(f.col),
    }
}

/// A rule's reason: "same name + phone".
pub fn auto_label(parts: &[Part], headers: &[String]) -> String {
    format!("same {}", parts.iter().map(|p| part_label(p, headers)).collect::<Vec<_>>().join(" + "))
}

/// A ready-made cross-check: its title, what it does, its rules.
#[derive(Clone, Debug)]
pub struct Suggestion {
    pub title: String,
    pub note: String,
    pub rules: Vec<Rule>,
    /// Weaker matches, to review rather than delete blindly.
    pub review: bool,
}

/// The cross-checks the columns allow, surest first, and last all of them at once.
pub fn suggest(r: &Roles, headers: &[String]) -> Vec<Suggestion> {
    let title = |c: usize| headers.get(c).cloned().unwrap_or_default();
    let list = |cs: &[usize]| cs.iter().map(|&c| title(c)).collect::<Vec<_>>().join(", ");
    let name = || Part::Name(Names { cols: r.names(), ..Default::default() });
    let phones = || Part::Any { cols: r.phones.clone(), clean: Clean::E164 { cc: r.cc }, country: r.country };
    let mails = || Part::Any { cols: r.emails.clone(), clean: Clean::Mail { canon: false }, country: None };
    let field = |c: usize| Part::Field(Field { col: c, clean: Clean::Slug });
    let rule = |parts: Vec<Part>| { let label = auto_label(&parts, headers); Rule::new(parts, label) };
    let place = r.city.or(r.postal);
    let mut out = Vec::new();
    let mut every = Vec::new();
    let named = r.has_name();
    if named && (!r.phones.is_empty() || !r.emails.is_empty()) {
        let mut rules = Vec::new();
        let mut how = Vec::new();
        if !r.phones.is_empty() { rules.push(rule(vec![name(), phones()])); how.push("name + phone") }
        if !r.emails.is_empty() { rules.push(rule(vec![name(), mails()])); how.push("name + e-mail") }
        every.extend(rules.iter().cloned());
        out.push(Suggestion { title: format!("Same person: {}", how.join(" OR ")), note: format!("The name ({}) in any order, and a phone or e-mail in common.", list(&r.names())), rules, review: false });
    }
    if !r.emails.is_empty() {
        let any = r.emails.len() > 1;
        let x = rule(vec![mails()]);
        every.push(x.clone());
        out.push(Suggestion { title: if any { "Same e-mail in any e-mail column".into() } else { "Same e-mail".into() }, note: format!("Case ignored; {}.", list(&r.emails)), rules: vec![x], review: false });
    }
    if !r.phones.is_empty() {
        let any = r.phones.len() > 1;
        let x = rule(vec![phones()]);
        every.push(x.clone());
        let from = r.country.map_or(String::new(), |c| format!(", the country from {}", title(c)));
        out.push(Suggestion { title: if any { "Same phone in any phone column".into() } else { "Same phone number".into() }, note: format!("As +{} numbers whatever their format{from}; {}.", r.cc, list(&r.phones)), rules: vec![x], review: false });
    }
    if named {
        if let Some(c) = place {
            let x = rule(vec![name(), field(c)]);
            every.push(x.clone());
            out.push(Suggestion { title: format!("Same name + {}", title(c)), note: "The name in any order, no case nor accents.".into(), rules: vec![x], review: false });
        }
        if let Some(c) = r.company {
            let x = rule(vec![name(), field(c)]);
            every.push(x.clone());
            out.push(Suggestion { title: format!("Same name + {}", title(c)), note: "The name in any order, no case nor accents.".into(), rules: vec![x], review: false });
        }
        out.push(Suggestion { title: "Same name, any order (to review)".into(), note: "First + last name = full name, swapped columns, no case, accents nor civility: namesakes too.".into(), rules: vec![rule(vec![name()])], review: true });
        let similar = Part::Name(Names { cols: r.names(), sound: Some(0.8), initials: true });
        out.push(Suggestion { title: "Similar names: sound alike or initials (to review)".into(), note: "Dupont ~ Dupond, J. Dupont ~ Jean Dupont: weaker matches, flagged as such.".into(), rules: vec![rule(vec![similar])], review: true });
    }
    if every.len() > 1 {
        out.push(Suggestion { title: "Every sure check at once".into(), note: "All but the ones to review, each group scored by what ties it.".into(), rules: every, review: false });
    }
    out
}

/// A title's words: camel case split ("phoneNumber" → "phone number"), then slugged.
fn title_words(h: &str) -> String {
    let mut t = String::with_capacity(h.len() + 4);
    let mut prev_lower = false;
    for c in h.chars() {
        if c.is_uppercase() && prev_lower { t.push(' ') }
        prev_lower = c.is_lowercase();
        t.push(c);
    }
    slug(&t)
}

fn has_any(words: &str, ws: &[&str]) -> bool {
    words.split(' ').any(|w| ws.contains(&w))
}

/// Up to `n` filled values of a column (trimmed), spread over the file.
fn column_sample(st: &State, col: usize, n: usize) -> Vec<String> {
    let len = st.len();
    let step = (len / (n * 4)).max(1);
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    let mut p = 0;
    while p < len && out.len() < n {
        st.with_cell(st.data_id(p), col, &mut tmp, |v| { let v = v.trim_matches(js_space); if !v.is_empty() { out.push(v.to_string()) } });
        p += if p < 2000 { 1 } else { step };
    }
    out
}

fn share(vals: &[String], f: impl Fn(&str) -> bool) -> f64 {
    if vals.is_empty() { 0.0 } else { vals.iter().filter(|v| f(v)).count() as f64 / vals.len() as f64 }
}

/// One value fills most of the column (`Created_by`: the same address on every
/// row): such a column would make the whole file one group.
fn shared_values(vals: &[String]) -> bool {
    let mut counts: HashMap<String, usize> = HashMap::default();
    for v in vals { *counts.entry(v.to_lowercase()).or_default() += 1 }
    counts.values().max().is_some_and(|&m| m * 4 > vals.len() && m > 2)
}

/// Record ids ("recXPi6SMAyHigsOJ"), not names: no space, letters and digits mixed.
fn id_shaped(v: &str) -> bool {
    v.len() >= 8 && !v.contains(' ') && v.bytes().any(|b| b.is_ascii_digit()) && v.bytes().any(|b| b.is_ascii_alphabetic())
}

/// A person's name: one to four words of letters.
fn person_shaped(v: &str) -> bool {
    let words = v.split_whitespace().count();
    (1..=4).contains(&words) && v.chars().all(|c| c.is_alphabetic() || matches!(c, ' ' | '-' | '\'' | '’' | '.'))
}

/// A phone number as written: digits with separators or a +, read as one.
fn phone_shaped(v: &str, cc: u16) -> bool {
    let digits = v.bytes().filter(u8::is_ascii_digit).count();
    (8..=15).contains(&digits) && (v.starts_with('+') || v.starts_with('0') || v.contains([' ', '.', '-', '('])) && e164(v, cc).is_some()
}

fn email_shaped(v: &str) -> bool {
    let mut s = String::new();
    mail_into(v, false, &mut s);
    !s.is_empty()
}

/// What the columns hold: names, e-mails, phones, company, place, country.
pub fn roles(st: &State) -> Roles {
    let heads: Vec<String> = st.headers().iter().map(|h| title_words(h)).collect();
    let kinds = crate::anonymize::guess_all(st);
    let samples: Vec<Vec<String>> = (0..st.ncols).into_par_iter().map(|c| column_sample(st, c, 400)).collect();
    use crate::anonymize::Mode;
    let mut r = Roles { cc: 33, ..Default::default() };
    // Not the person's own: who made or changed the row, billing, the company's switchboard.
    const NOT_OWN: [&str; 26] = ["created", "modified", "updated", "owner", "author", "auteur", "admin", "by", "par", "sender", "expediteur", "from", "billing", "facturation", "invoice", "compta", "comptabilite", "accounting", "assigned", "commercial", "sales", "support", "noreply", "fax", "standard", "switchboard"];
    let country_words = ["pays", "country", "nation", "nationalite", "nationality"];
    // The country first: the phones read with it.
    r.country = (0..st.ncols).find(|&c| has_any(&heads[c], &country_words) && share(&samples[c], |v| crate::kinds::calling_code(v).is_some()) >= 0.5)
        .or_else(|| (0..st.ncols).find(|&c| share(&samples[c], |v| v.len() > 3 && crate::kinds::calling_code(v).is_some()) >= 0.8));
    if let Some(c) = r.country {
        let mut n: HashMap<u16, usize> = HashMap::default();
        for v in &samples[c] { if let Some(k) = crate::kinds::calling_code(v) { *n.entry(k).or_default() += 1 } }
        if let Some((&k, _)) = n.iter().max_by_key(|e| (e.1, std::cmp::Reverse(*e.0))) { r.cc = k }
    }
    for c in 0..st.ncols {
        let (h, vals) = (&heads[c], &samples[c]);
        if vals.is_empty() || has_any(h, &NOT_OWN) || shared_values(vals) { continue }
        if (kinds[c] == Mode::Email || has_any(h, &["email", "mail", "courriel"])) && share(vals, email_shaped) >= 0.6 {
            r.emails.push(c);
        } else if has_any(h, &["tel", "telephone", "phone", "mobile", "portable", "gsm", "cell", "cellulaire", "telephones", "phones"]) || kinds[c] == Mode::Phone {
            if share(vals, |v| e164(v, r.cc).is_some()) >= 0.4 { r.phones.push(c) }
        } else if !id_like_title(h) && share(vals, |v| phone_shaped(v, r.cc)) >= 0.8 {
            r.phones.push(c);
        }
    }
    // Names: titled first / last; a generic "name" column is the last name next to a
    // first name, else the full name — when its values are a person's.
    let first_t = ["prenom", "prenoms", "firstname", "first", "given", "givenname", "forename"];
    let last_t = ["lastname", "last", "surname", "family", "familyname", "patronyme", "nom de famille"];
    let full_t = ["fullname", "full", "complet", "displayname", "display", "contact", "interlocuteur"];
    let generic = |h: &str| matches!(h, "name" | "nom" | "noms" | "names" | "nom prenom" | "prenom nom");
    let persons = |c: usize| share(&samples[c], person_shaped) >= 0.6 && share(&samples[c], id_shaped) < 0.3;
    let titled = |ws: &[&str], c: usize| has_any(&heads[c], ws) && !has_any(&heads[c], &NOT_OWN) && !id_like_title(&heads[c]);
    r.first = (0..st.ncols).find(|&c| titled(&first_t, c) && persons(c));
    r.last = (0..st.ncols).find(|&c| (titled(&last_t, c) || heads[c] == "nom" && r.first.is_some()) && persons(c) && Some(c) != r.first);
    let mut generic_left = Vec::new();
    for c in 0..st.ncols {
        if Some(c) == r.first || Some(c) == r.last || r.emails.contains(&c) || r.phones.contains(&c) { continue }
        let full = titled(&full_t, c) || kinds[c] == Mode::Full;
        if !(full || generic(&heads[c])) { continue }
        if r.first.is_some() || r.last.is_some() {
            // Next to first / last names: a full name holds them.
            if holds_names(st, c, r.first, r.last) >= 0.5 { r.full.push(c) } else if generic(&heads[c]) { generic_left.push(c) }
        } else if persons(c) && share(&samples[c], |v| v.split_whitespace().count() >= 2) >= 0.5 {
            r.full.push(c);
        }
    }
    // The company: titled, of names (not record ids); else a generic name that is no person's.
    let company_t = ["societe", "company", "entreprise", "organisation", "organization", "organisme", "employeur", "employer", "firme", "raison", "etablissement", "structure", "compagnie"];
    let named = |c: usize| share(&samples[c], id_shaped) < 0.3 && share(&samples[c], |v| v.chars().any(char::is_alphabetic)) >= 0.8;
    r.company = (0..st.ncols).find(|&c| (titled(&company_t, c) || kinds[c] == Mode::Company) && named(c) && !r.names().contains(&c))
        .or_else(|| generic_left.iter().copied().find(|&c| named(c)));
    r.city = (0..st.ncols).find(|&c| has_any(&heads[c], &["ville", "city", "commune", "localite", "town", "locality", "municipality"]) && !has_any(&heads[c], &NOT_OWN) && named(c));
    r.postal = (0..st.ncols).find(|&c| kinds[c] == Mode::Postal && share(&samples[c], |v| v.bytes().filter(u8::is_ascii_digit).count() >= 4 || v.len() >= 5) >= 0.6);
    if r.country.is_none() {
        // No country column: the code most numbers are written with.
        let mut n: HashMap<u16, usize> = HashMap::default();
        for &c in &r.phones {
            for v in &samples[c] {
                if let Some((y, m)) = e164(v, 33) {
                    let d = &y[1..m];
                    let cl = crate::kinds::calling_code_len(d);
                    let code = d[..cl].iter().fold(0u16, |a, b| a * 10 + (b - b'0') as u16);
                    if v.trim_start().starts_with(['+', '0']) && (v.trim_start().starts_with('+') || v.trim_start().starts_with("00")) { *n.entry(code).or_default() += 1 }
                }
            }
        }
        if let Some((&k, _)) = n.iter().max_by_key(|e| (e.1, std::cmp::Reverse(*e.0))) { r.cc = k }
    }
    r
}

/// A title saying the column identifies rows (an id, a reference, a code).
fn id_like_title(words: &str) -> bool {
    has_any(words, &["id", "ids", "uuid", "guid", "ref", "reference", "key", "code", "siret", "siren", "iban", "record", "num", "numero", "no", "number"]) && !has_any(words, &["phone", "tel", "telephone", "mobile", "portable"])
}

/// The share of rows (of the first 2000 with a first or last name) whose
/// value in `col` holds that first and last name.
fn holds_names(st: &State, col: usize, first: Option<usize>, last: Option<usize>) -> f64 {
    let (mut tmp, mut a, mut b, mut v) = (Vec::new(), String::new(), String::new(), String::new());
    let (mut seen, mut held) = (0, 0);
    for p in 0..st.len().min(2000) {
        let id = st.data_id(p);
        a.clear(); b.clear(); v.clear();
        if let Some(c) = first { st.with_cell(id, c, &mut tmp, |x| slug_into(x, &mut a)) }
        if let Some(c) = last { st.with_cell(id, c, &mut tmp, |x| slug_into(x, &mut b)) }
        if a.is_empty() && b.is_empty() { continue }
        st.with_cell(id, col, &mut tmp, |x| slug_into(x, &mut v));
        seen += 1;
        let words: Vec<&str> = v.split(' ').collect();
        if a.split(' ').chain(b.split(' ')).filter(|w| !w.is_empty()).all(|w| words.contains(&w)) { held += 1 }
    }
    if seen == 0 { 0.0 } else { held as f64 / seen as f64 }
}

// ---- Cleanings -------------------------------------------------------------

/// JavaScript's `\s` (what the web app trimmed and collapsed).
#[inline]
pub fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

#[inline]
fn lower_into(s: &str, out: &mut String) {
    if s.is_ascii() {
        out.extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
    } else {
        out.extend(s.chars().flat_map(char::to_lowercase));
    }
}

/// Trimmed, runs of spaces as one, lower case.
pub fn loose_into(v: &str, out: &mut String) {
    let mut gap = false;
    for c in v.trim_matches(js_space).chars() {
        if js_space(c) { gap = true; continue }
        if gap { out.push(' '); gap = false }
        if c.is_ascii() { out.push(c.to_ascii_lowercase()) } else { out.extend(c.to_lowercase()) }
    }
}

/// Lower case, no accents, every run of other than letters and digits as one
/// space, trimmed: "  L'Œuvre — Société " → "l oeuvre societe".
pub fn slug_into(v: &str, out: &mut String) {
    let start = out.len();
    let mut gap = false;
    let mut put = |c: char, out: &mut String| {
        if c.is_alphanumeric() {
            if gap && out.len() > start { out.push(' ') }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    };
    if v.is_ascii() {
        for b in v.bytes() { put(b.to_ascii_lowercase() as char, out) }
        return;
    }
    for c in v.chars() {
        for l in c.to_lowercase() {
            decompose_canonical(l, |d| match d {
                '\u{300}'..='\u{36f}' => {}
                'œ' => { put('o', out); put('e', out) }
                'æ' => { put('a', out); put('e', out) }
                'ß' => { put('s', out); put('s', out) }
                'ø' => put('o', out),
                'đ' | 'ð' => put('d', out),
                'ł' => put('l', out),
                'þ' => { put('t', out); put('h', out) }
                _ => put(d, out),
            });
        }
    }
}

pub fn slug(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    slug_into(v, &mut s);
    s
}

/// Unicode's format characters (Cf): marks pasted around numbers, zero-width spaces.
fn is_cf(c: char) -> bool {
    matches!(c as u32, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E | 0x200B..=0x200F
        | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD
        | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// A phone number as E.164 (+33612345678), French national numbers read with
/// +33; else its digits. Empty when the cell is.
fn phone_into(v: &str, out: &mut String) {
    if !v.chars().any(|c| !is_cf(c) && !js_space(c)) { return }
    if let Some(e) = parse_phone(v) {
        out.push_str(std::str::from_utf8(&e.0[..e.1]).unwrap());
    } else {
        out.extend(v.chars().filter(char::is_ascii_digit));
    }
}

/// A phone cell's digits and + (spaces, dots, dashes, slashes, brackets and a
/// "(0)" dropped), on a stack buffer; None when anything else is in it.
fn phone_chars(v: &str) -> Option<([u8; 24], usize)> {
    let mut x = [0u8; 24];
    let mut n = 0;
    let mut it = v.chars().filter(|&c| !is_cf(c));
    while let Some(c) = it.next() {
        if c == '(' {
            let mut la = it.clone();
            if la.next() == Some('0') && la.next() == Some(')') { it = la; continue }
        }
        if js_space(c) || matches!(c, '.' | '-' | '(' | ')' | '/') { continue }
        // Anything but digits and '+' can only fail the checks below.
        if !(c.is_ascii_digit() || c == '+') || n >= 20 { return None }
        x[n] = c as u8;
        n += 1;
    }
    Some((x, n))
}

/// The web app's parsePhoneCell(v, '33'), on a stack buffer.
fn parse_phone(v: &str) -> Option<([u8; 24], usize)> {
    let (x, n) = phone_chars(v)?;
    let d = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    let mut y = [0u8; 24];
    let mut m = 0;
    let put = |s: &[u8], y: &mut [u8; 24], m: &mut usize| { y[*m..*m + s.len()].copy_from_slice(s); *m += s.len(); };
    let s = &x[..n];
    if s.starts_with(b"00") {
        put(b"+", &mut y, &mut m);
        put(&s[2..], &mut y, &mut m);
    } else if s.starts_with(b"+") {
        put(s, &mut y, &mut m);
    } else if s.first() == Some(&b'0') && (7..=15).contains(&s.len()) && d(s) {
        put(b"+33", &mut y, &mut m);
        put(&s[1..], &mut y, &mut m);
    } else if s.len() == 9 && d(s) && s[0] != b'0' {
        put(b"+33", &mut y, &mut m);
        put(s, &mut y, &mut m);
    } else if d(s) && s.starts_with(b"33") && ((s.len() == 11 && s[2] != b'0') || (s.len() == 12 && s[2] == b'0' && s[3] != b'0')) {
        put(b"+33", &mut y, &mut m);
        put(&s[s.len() - 9..], &mut y, &mut m);
    } else {
        return None;
    }
    // +33 06…: the national 0 kept after the code.
    if m == 13 && y.starts_with(b"+330") && y[4] != b'0' && d(&y[1..13]) {
        y.copy_within(4..13, 3);
        m = 12;
    }
    let e = &y[..m];
    if !(9..=16).contains(&m) || e[0] != b'+' || e[1] == b'0' || !d(&e[1..]) { return None }
    if e.starts_with(b"+33") && m != 12 { return None }
    Some((y, m))
}

/// A phone number as E.164, national numbers read in the country of calling
/// code `cc` (France: the web app's reading; the US and Canada: ten digits;
/// Italy keeps its leading 0; elsewhere the trunk 0 goes).
pub fn e164(v: &str, cc: u16) -> Option<([u8; 24], usize)> {
    let (x, n) = phone_chars(v)?;
    let s = &x[..n];
    let d = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_digit);
    let intl = s.starts_with(b"+") || s.starts_with(b"00") || (cc == 1 && s.starts_with(b"011"));
    let mut y = [0u8; 24];
    let mut m = 1;
    y[0] = b'+';
    let mut put = |t: &[u8], y: &mut [u8; 24]| -> Option<()> {
        if m + t.len() > 20 { return None }
        y[m..m + t.len()].copy_from_slice(t);
        m += t.len();
        Some(())
    };
    if intl {
        let rest = if s[0] == b'+' { &s[1..] } else if s.starts_with(b"011") { &s[3..] } else { &s[2..] };
        if !d(rest) { return None }
        let cl = crate::kinds::calling_code_len(rest);
        if cl == 0 || rest.len() <= cl { return None }
        let (code, mut nat) = rest.split_at(cl);
        // +33 (0)6, +44 020…: the trunk 0 written after the code (Italy's 0 belongs to its numbers).
        if nat.len() > 1 && nat[0] == b'0' && code != b"39" && code != b"378" && code != b"379" { nat = &nat[1..] }
        put(code, &mut y)?;
        put(nat, &mut y)?;
    } else if cc == 33 {
        return parse_phone(v).filter(|e| !placeholder(&e.0[..e.1]));
    } else {
        if !d(s) { return None }
        // The code's digits, without allocating.
        let mut code = [0u8; 5];
        let mut cs = 0;
        for div in [10000, 1000, 100, 10, 1] {
            if cc >= div || cs > 0 || div == 1 { code[cs] = b'0' + (cc / div % 10) as u8; cs += 1; }
        }
        let code = &code[..cs];
        match cc {
            1 if s.len() == 10 => { put(b"1", &mut y)?; put(s, &mut y)?; }
            1 if s.len() == 11 && s[0] == b'1' => put(s, &mut y)?,
            1 => return None,
            39 | 378 | 379 => {
                if s.starts_with(code) && s.len() >= cs + 8 && s[cs] != b'0' { put(s, &mut y)?; } else { put(code, &mut y)?; put(s, &mut y)?; }
            }
            _ if s[0] == b'0' => { if s.get(1) == Some(&b'0') { return None } put(code, &mut y)?; put(&s[1..], &mut y)?; }
            _ if s.starts_with(code) && s.len() >= cs + 8 => put(s, &mut y)?,
            _ => { put(code, &mut y)?; put(s, &mut y)?; }
        }
    }
    let e = &y[..m];
    if !(9..=16).contains(&m) || e[1] == b'0' { return None }
    if (e.starts_with(b"+33") || e.starts_with(b"+1")) && m != 12 { return None }
    if placeholder(e) { return None }
    Some((y, m))
}

/// A number filled in for want of one: its digits after the first of the
/// national number all the same (+33 6 00 00 00 00).
fn placeholder(e: &[u8]) -> bool {
    let nat = &e[1 + crate::kinds::calling_code_len(&e[1..]).max(1).min(e.len() - 1)..];
    nat.len() > 2 && nat[1..].iter().all(|&c| c == nat[1])
}

/// `e164` appended to `out` (nothing when it is no phone number).
fn e164_into(v: &str, cc: u16, out: &mut String) {
    if let Some((y, m)) = e164(v, cc) { out.push_str(std::str::from_utf8(&y[..m]).unwrap()) }
}

/// An e-mail address, lower case: nothing without a name and a domain around
/// an @. `canon`: "+tag" ignored, and the dots of a gmail name
/// (J.Dupont+news@googlemail.com = jdupont@gmail.com).
fn mail_into(v: &str, canon: bool, out: &mut String) {
    let mut v = v.trim_matches(js_space);
    // On bytes: the 7th may fall inside a character (`v[..7]` panicked on "Élodie@…").
    if v.len() >= 7 && v.as_bytes()[..7].eq_ignore_ascii_case(b"mailto:") { v = &v[7..] }
    let Some(at) = v.rfind('@') else { return };
    let (name, domain) = (&v[..at], &v[at + 1..]);
    if name.is_empty() || !domain.contains('.') || domain.ends_with('.') || v.contains(js_space) { return }
    let start = out.len();
    if !canon {
        lower_into(v, out);
        return;
    }
    let dstart = out.len();
    lower_into(domain, out);
    let gmail = matches!(&out[dstart..], "gmail.com" | "googlemail.com");
    out.truncate(start);
    let name = name.split('+').next().unwrap_or(name);
    if name.is_empty() { return }
    if gmail {
        out.extend(name.chars().filter(|&c| c != '.').flat_map(char::to_lowercase));
        out.push_str("@gmail.com");
    } else {
        lower_into(name, out);
        out.push('@');
        lower_into(domain, out);
    }
}

// ---- French phonetics ("sounds like") -----------------------------------------
// Metaphone's principle on French spelling, per word of the slug: a French
// transcription (vowels kept: what the spelling distance compares), then
// Metaphone's merges on it (the code: the key). Dupont / Dupond, Lefebvre /
// Lefèvre, Philippe / Filippe share code and spelling; Martin / Martine share a
// code but not the spelling, kept apart by the distance.

/// (transcription, code) of a value, words separated by spaces.
pub fn sound(v: &str) -> (String, String) {
    let v: String = v.chars().map(|c| if c == 'ç' || c == 'Ç' { 's' } else { c }).collect();   // the slug would make it a c, read k
    let (mut trans, mut code) = (String::new(), String::new());
    for w in slug(&v).split(' ').filter(|w| !w.is_empty()) {
        let (t, c) = fr_sound(w);
        if !trans.is_empty() { trans.push(' '); code.push(' '); }
        trans.push_str(&t);
        code.push_str(&c);
    }
    (trans, code)
}

/// One global regex replacement: `f` sees the original text at i and returns
/// (length matched, replacement).
fn rw(s: &[char], f: impl Fn(&[char], usize) -> Option<(usize, &'static str)>) -> Vec<char> {
    let mut out = Vec::with_capacity(s.len() + 4);
    let mut i = 0;
    while i < s.len() {
        match f(s, i) {
            Some((n, r)) => { out.extend(r.chars()); i += n; }
            None => { out.push(s[i]); i += 1; }
        }
    }
    out
}

fn at(s: &[char], i: usize, p: &str) -> bool {
    p.chars().enumerate().all(|(k, c)| s.get(i + k) == Some(&c))
}

fn next_in(s: &[char], i: usize, set: &str) -> bool {
    s.get(i).is_some_and(|c| set.contains(*c))
}

fn fr_sound(w: &str) -> (String, String) {
    let s: Vec<char> = w.chars().collect();
    let lit = |p: &'static str, r: &'static str| move |s: &[char], i: usize| if at(s, i, p) { Some((p.chars().count(), r)) } else { None };
    let s = rw(&s, lit("ph", "f"));
    let s = rw(&s, lit("th", "t"));
    let s = rw(&s, |s, i| if at(s, i, "ch") && next_in(s, i + 2, "rl") { Some((2, "k")) } else { None });   // Christophe, Chloé
    let s = rw(&s, |s, i| if at(s, i, "sch") { Some((3, "S")) } else if at(s, i, "ch") || at(s, i, "sh") { Some((2, "S")) } else { None });
    let s = rw(&s, |s, i| if at(s, i, "ck") || at(s, i, "cq") || at(s, i, "qu") { Some((2, "k")) } else if at(s, i, "q") { Some((1, "k")) } else { None });
    let s = rw(&s, |s, i| if at(s, i, "gu") && next_in(s, i + 2, "eiy") { Some((2, "G")) } else { None });   // Guillaume stays hard
    let s = rw(&s, |s, i| if at(s, i, "ge") && next_in(s, i + 2, "aou") { Some((2, "j")) } else { None });
    let s = rw(&s, |s, i| if at(s, i, "g") && next_in(s, i + 1, "eiy") { Some((1, "j")) } else { None });
    let s = rw(&s, lit("G", "g"));
    let s = rw(&s, |s, i| if at(s, i, "c") { Some((1, if next_in(s, i + 1, "eiy") { "s" } else { "k" })) } else { None });
    let s = rw(&s, |s, i| match s[i] {
        'w' => Some((1, "v")),
        'z' => Some((1, "s")),
        'y' => Some((1, "i")),
        'x' if i + 1 == s.len() => Some((1, "")),
        'x' => Some((1, "ks")),
        'h' => Some((1, "")),
        _ => None,
    });
    // Nasals: before a consonant other than m / n, or at the end.
    let nasal = |s: &[char], j: usize| s.get(j).is_none_or(|c| !"aeioumn".contains(*c));
    let s = rw(&s, |s, i| if ["ain", "ein", "aim", "eim"].iter().any(|p| at(s, i, p)) && nasal(s, i + 3) { Some((3, "1")) } else { None });
    let s = rw(&s, |s, i| if ["in", "im", "un", "um"].iter().any(|p| at(s, i, p)) && nasal(s, i + 2) { Some((2, "1")) } else { None });
    let s = rw(&s, |s, i| if ["an", "am", "en", "em"].iter().any(|p| at(s, i, p)) && nasal(s, i + 2) { Some((2, "2")) } else { None });
    let s = rw(&s, |s, i| if ["on", "om"].iter().any(|p| at(s, i, p)) && nasal(s, i + 2) { Some((2, "3")) } else { None });
    let s = rw(&s, |s, i| if at(s, i, "aul") && next_in(s, i + 3, "tdx") { Some((3, "o")) } else { None });   // Thibault, Gaultier
    let s = rw(&s, |s, i| if at(s, i, "eau") { Some((3, "o")) } else if at(s, i, "au") { Some((2, "o")) } else { None });
    let s = rw(&s, lit("ou", "u"));
    let s = rw(&s, |s, i| if at(s, i, "ai") || at(s, i, "ei") { Some((2, "e")) } else { None });
    let s = rw(&s, |s, i| if at(s, i, "oeu") { Some((3, "e")) } else if at(s, i, "eu") { Some((2, "e")) } else { None });
    let mut s = rw(&s, lit("bv", "v"));
    s.dedup();
    for _ in 0..2 {
        if s.len() > 2 && "dtsxzp".contains(s[s.len() - 1]) { s.pop(); } else { break }
    }
    if s.len() > 2 && s[s.len() - 1] == 'e' { s.pop(); }
    // Metaphone's merges: the key casts wide, the spelling distance narrows it.
    let m: Vec<char> = s.iter().map(|&c| match c {
        '1' | '2' | '3' => 'N',
        'S' => 'X',
        'd' => 't',
        'v' => 'f',
        'g' => 'k',
        'z' => 's',
        c => c,
    }).collect();
    let mut code: Vec<char> = Vec::with_capacity(m.len());
    if let Some(&c0) = m.first() {
        code.push(if "aeiou".contains(c0) { 'A' } else { c0 });
        code.extend(m[1..].iter().filter(|c| !"aeiou".contains(**c)));
    }
    code.dedup();
    (s.into_iter().collect(), code.into_iter().collect::<String>().to_uppercase())
}

/// 1 − edit distance / the longer length (Levenshtein), 0 to 1.
pub fn similarity(a: &[char], b: &[char]) -> f64 {
    if a == b { return 1.0 }
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 { return 0.0 }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    1.0 - prev[m] as f64 / n.max(m) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;

    fn table(s: &str) -> Table {
        Table::new(Arc::new(Base::from_bytes(s.as_bytes().to_vec(), OpenOptions::default())), None)
    }

    fn all(t: &Table) -> Vec<RowId> {
        (0..t.st.len()).map(|p| t.st.data_id(p)).collect()
    }

    fn key(fields: &[(usize, Clean)]) -> Vec<Field> {
        fields.iter().map(|(c, k)| Field { col: *c, clean: k.clone() }).collect()
    }

    /// Each group as its rows' first cells.
    fn groups(t: &Table, d: &Dupes) -> Vec<Vec<String>> {
        (0..d.groups() as u32).map(|g| d.members(g).iter().map(|&id| t.st.cell(id, 0)).collect()).collect()
    }

    fn cleaned(c: Clean, v: &str) -> String {
        let mut s = String::new();
        c.apply(v, &mut s);
        s
    }

    #[test]
    fn cleanings() {
        assert_eq!(cleaned(Clean::Loose, "  Jean   DUPONT\t"), "jean dupont");
        assert_eq!(cleaned(Clean::Slug, "  L'Œuvre — Société Générale!"), "l oeuvre societe generale");
        assert_eq!(cleaned(Clean::Slug, "Straße"), "strasse");
        assert_eq!(cleaned(Clean::Digits, "75 011 Paris"), "75011");
        assert_eq!(cleaned(Clean::Email, " MailTo:Ana@X.io "), "ana@x.io");
        for v in ["06 12 34 56 78", "+33 6 12 34 56 78", "0033612345678", "612345678", "33612345678", "+33 (0)6 12 34 56 78", "\u{202d}06.12.34.56.78\u{202c}", "+33 06 12 34 56 78"] {
            assert_eq!(cleaned(Clean::Phone, v), "+33612345678", "{v}");
        }
        assert_eq!(cleaned(Clean::Phone, "+1 (203) 893-3004"), "+12038933004");
        assert_eq!(cleaned(Clean::Phone, "poste 12"), "12");
        assert_eq!(cleaned(Clean::Phone, " \u{200b} "), "");
    }

    #[test]
    fn sounds() {
        let code = |v: &str| sound(v).1;
        assert_eq!(code("Dupont"), code("Dupond"));
        assert_eq!(code("Dupont"), code("Dupon"));
        assert_eq!(code("Lefebvre"), code("Lefèvre"));
        assert_eq!(code("Philippe"), code("Filippe"));
        assert_eq!(code("Christophe"), code("Kristof"));
        assert_eq!(code("Schmitt"), code("Schmidt"));
        assert_eq!(code("Meyer"), code("Mayer"));
        assert_eq!(code("Martin"), code("Martine"));
        let close = |a: &str, b: &str| similarity(&sound(a).0.chars().collect::<Vec<_>>(), &sound(b).0.chars().collect::<Vec<_>>());
        assert!(close("Dupont", "Dupond") >= 0.8);
        assert!(close("Martin", "Martine") < 0.8);
        assert_eq!(code("François"), code("Fransoi"));
    }

    #[test]
    fn keys_chain_and_whole_row() {
        let t = table("name,email,phone\nA,ana@x.io,0612345678\nB,ANA@X.IO ,\nC,,+33 6 12 34 56 78\nD,d@x.io,\nE,,\nF,,\n");
        let no = AtomicBool::new(false);
        let spec = DupSpec { keys: vec![key(&[(1, Clean::Email)]), key(&[(2, Clean::Phone)])] };
        let d = find(&t.st, &all(&t), &spec, &no, None).unwrap();
        // A–B by e-mail, A–C by phone: one group; E and F have no key at all.
        assert_eq!(groups(&t, &d), vec![vec!["A", "B", "C"]]);
        assert_eq!(d.hits, vec![2, 2]);
        assert_eq!(d.removable(), 2);
        let first: HashSet<String> = d.to_delete(false).iter().map(|&id| t.st.cell(id, 0)).collect();
        assert_eq!(first, ["B", "C"].iter().map(|s| s.to_string()).collect());
        let last: HashSet<String> = d.to_delete(true).iter().map(|&id| t.st.cell(id, 0)).collect();
        assert_eq!(last, ["A", "B"].iter().map(|s| s.to_string()).collect());
        assert_eq!(d.diff_cols(&t.st, &spec, 0), vec![0, 1, 2]);

        let t = table("a,b\nx, Y\nz,w\nX,y\n,\n,\nz,w,\n");
        let d = find(&t.st, &all(&t), &DupSpec::default(), &no, None).unwrap();
        assert_eq!(groups(&t, &d), vec![vec!["x", "X"], vec!["z", "z"]]);   // empty rows are not duplicates
        assert!(d.hits.is_empty());
        let view: Vec<RowId> = all(&t).into_iter().rev().collect();
        assert_eq!(d.only_dups(&view).iter().map(|&id| t.st.cell(id, 0)).collect::<Vec<_>>(), vec!["X", "x", "z", "z"]);
    }

    #[test]
    fn multi_field_keys_and_modes() {
        let t = table("last,first,cp\nDupont,Jean,75011\nDUPONT,jean,75 011\nDupond,Jean,75011\nDupont,Jean,\nMartin,Ana,1\nMartine,Ana,1\n");
        let no = AtomicBool::new(false);
        let ids = all(&t);
        let exact = DupSpec { keys: vec![key(&[(0, Clean::Exact), (1, Clean::Exact)])] };
        assert_eq!(groups(&t, &find(&t.st, &ids, &exact, &no, None).unwrap()), vec![vec!["Dupont", "Dupont"]]);
        let slug = DupSpec { keys: vec![key(&[(0, Clean::Slug), (1, Clean::Slug), (2, Clean::Digits)])] };
        assert_eq!(groups(&t, &find(&t.st, &ids, &slug, &no, None).unwrap()), vec![vec!["Dupont", "DUPONT"]]);
        let snd = DupSpec { keys: vec![key(&[(0, Clean::Sound { tol: 0.8 }), (1, Clean::Loose)])] };
        let d = find(&t.st, &ids, &snd, &no, None).unwrap();
        assert_eq!(groups(&t, &d), vec![vec!["Dupont", "DUPONT", "Dupond", "Dupont"]]);
        let custom = DupSpec { keys: vec![key(&[(0, Clean::Custom(Arc::new(|v: &str, o: &mut String| o.push_str(&v.to_lowercase()[..3]))))])] };
        assert_eq!(find(&t.st, &ids, &custom, &no, None).unwrap().groups(), 2);
    }

    fn names(cols: &[usize]) -> Part {
        Part::Name(Names { cols: cols.to_vec(), ..Default::default() })
    }

    fn find_r(t: &Table, rules: &[Rule]) -> Dupes {
        find_rules(&t.st, &all(t), rules, &[], &AtomicBool::new(false), None).unwrap()
    }

    #[test]
    fn phones_and_mails() {
        let e = |v: &str, cc: u16| { let mut s = String::new(); e164_into(v, cc, &mut s); s };
        for v in ["06 12 34 56 78", "+33 (0)6 12 34 56 78", "0033 6 12 34 56 78", "\u{202d}+33612345678\u{202c}"] {
            assert_eq!(e(v, 33), "+33612345678", "{v}");
        }
        assert_eq!(e("06 00 00 00 00", 33), "", "a placeholder");
        assert_eq!(e("poste 12", 33), "");
        assert_eq!(e("(203) 893-3004", 1), "+12038933004");
        assert_eq!(e("1 203 893 3004", 1), "+12038933004");
        assert_eq!(e("+1 (203) 893-3004", 33), "+12038933004");
        assert_eq!(e("020 7946 0958", 44), "+442079460958");
        assert_eq!(e("+44 (0)20 7946 0958", 33), "+442079460958");
        assert_eq!(e("030 1234567", 49), "+49301234567");
        assert_eq!(e("06 1234 5678", 39), "+390612345678", "Italy keeps its 0");
        assert_eq!(e("0470 12 34 56", 32), "+32470123456");
        let m = |v: &str, canon: bool| cleaned(Clean::Mail { canon }, v);
        assert_eq!(m(" MailTo:Ana@X.io ", false), "ana@x.io");
        assert_eq!(m("J.Dupont+news@GoogleMail.com", true), "jdupont@gmail.com");
        assert_eq!(m("j.dupont+news@orange.fr", true), "j.dupont@orange.fr");
        assert_eq!(m("j.dupont+news@orange.fr", false), "j.dupont+news@orange.fr");
        assert_eq!(m("n/a", false), "");
        assert_eq!(m("ana@localhost", false), "");
    }

    #[test]
    fn names_in_any_order() {
        // first, last, full: "DUPONT Jean" alone is Jean + Dupont; a civility and accents change nothing.
        let t = table("first,last,full\nJean,Dupont,\n,,DUPONT Jean\nJéan,DUPONT,M. Jean Dupont\nDupont,Jean,\nJean,,\nJean,Martin,\nJean,,Dupond\n");
        let r = Rule::new(vec![names(&[0, 1, 2])], "same name");
        let d = find_r(&t, &[r]);
        assert_eq!(groups(&t, &d), vec![vec!["Jean", "", "Jéan", "Dupont"]]);
        assert_eq!(d.hits, vec![4]);
        assert_eq!(d.reasons(0), vec!["same name"]);
        // By sound: Dupond joins, flagged as such; spelled alike only.
        let snd = Rule::new(vec![Part::Name(Names { cols: vec![0, 1, 2], sound: Some(0.8), initials: false })], "same name");
        let d = find_r(&t, &[snd]);
        assert_eq!(groups(&t, &d), vec![vec!["Jean", "", "Jéan", "Dupont", "Jean"]]);
        assert_eq!(d.reasons(0), vec!["same name", "same name (name sounds alike)"]);
        assert!(d.confidence(0) < 60, "{}", d.confidence(0));
    }

    #[test]
    fn initials() {
        let t = table("name,city\nJean Dupont,Lyon\nJ. Dupont,Lyon\nJ Dupont,Lyon\nA. Martin,Lyon\nAnne Martin,Lyon\nAlain Martin,Lyon\nP. Durand,Lyon\nPaul Durand,Paris\n");
        let r = Rule::new(vec![Part::Name(Names { cols: vec![0], sound: None, initials: true }), Part::Field(Field { col: 1, clean: Clean::Slug })], "same name + city");
        let d = find_r(&t, &[r]);
        // A. Martin is Anne or Alain: not joined; P. Durand lives elsewhere.
        assert_eq!(groups(&t, &d), vec![vec!["Jean Dupont", "J. Dupont", "J Dupont"]]);
        assert_eq!(d.reasons(0), vec!["same name + city", "same name + city (first name as an initial)"]);
    }

    #[test]
    fn phones_across_columns() {
        let t = table("name,phone,mobile,country\nA,06 12 34 56 78,,France\nB,,+33612345678,FR\nC,(203) 893-3004,,United States\nD,+1 203 893 3004,,\nE,0612345678,,USA\nF,06 00 00 00 00,,\nG,06 00 00 00 00,,\nH,01 02 03 04 05,06 12 34 56 78,\n");
        let r = Rule::new(vec![Part::Any { cols: vec![1, 2], clean: Clean::E164 { cc: 33 }, country: Some(3) }], "same phone");
        let d = find_r(&t, &[r]);
        // E's 0612345678 read in the US is no number; F and G are placeholders.
        assert_eq!(groups(&t, &d), vec![vec!["A", "B", "H"], vec!["C", "D"]]);
        assert_eq!(d.hits, vec![5]);
    }

    #[test]
    fn confidence_and_reasons() {
        let t = table("first,last,email,phone,city\nJean,Dupont,jd@x.fr,0612345678,Lyon\nJean,Dupont,jd@x.fr,0612345678,Lyon\nAna,Lopez,ana@x.fr,,Paris\nLopez,Ana,ANA@x.fr ,0700000001,Paris\nLéo,Petit,,,Nice\nLeo,PETIT,,,Nice\nMax,Roux,m@x.fr,,Nice\nMax,Roux,,,Brest\n");
        let any_mail = Part::Any { cols: vec![2], clean: Clean::Mail { canon: false }, country: None };
        let any_phone = Part::Any { cols: vec![3], clean: Clean::E164 { cc: 33 }, country: None };
        let name = || names(&[0, 1]);
        let city = Part::Field(Field { col: 4, clean: Clean::Slug });
        let rules = vec![
            Rule { label: "same name + city".into(), ..Rule::new(vec![name(), city], "") },
            Rule { label: "same e-mail".into(), ..Rule::new(vec![any_mail], "") },
            Rule { label: "same phone".into(), ..Rule::new(vec![any_phone], "") },
        ];
        let d = find_r(&t, &rules);
        assert_eq!(groups(&t, &d), vec![vec!["Jean", "Jean"], vec!["Ana", "Lopez"], vec!["Léo", "Leo"]]);
        assert!(d.identical(0) && d.confidence(0) == 100);
        assert_eq!(d.reasons(0)[0], "identical rows");
        assert_eq!(d.reasons(1), vec!["same e-mail", "same name + city"]);
        assert!(d.confidence(1) > 95, "{}", d.confidence(1));
        assert_eq!(d.reasons(2), vec!["same name + city"]);
        assert_eq!(d.confidence(2), 80);
        assert_eq!(d.order(Order::LeastSure), vec![2, 1, 0]);
        assert_eq!(d.order(Order::SurestFirst), vec![0, 1, 2]);
        assert_eq!(d.side_by_side(Order::LeastSure).len(), 6);
        // A sort of the view keeps each group together, where its first row went.
        let mut v = d.side_by_side(Order::File);
        v.reverse();
        let (g0, g2) = (d.members(0).to_vec(), d.members(2).to_vec());
        v.swap(0, 3);
        d.keep_together(&mut v);
        let grp: Vec<u32> = v.iter().map(|&id| d.group(id).unwrap()).collect();
        assert!(grp.windows(2).filter(|w| w[0] != w[1]).count() == 2, "{grp:?}");
        assert_eq!(v.len(), 6);
        assert!(g0.iter().chain(&g2).all(|id| v.contains(id)));
        assert_eq!(d.count_scored(90, 101), 2);
        // A chain is as sure as its weakest link.
        let t = table("first,last,email,city\nJean,Dupont,jd@x.fr,Lyon\nJean,Dupont,,Lyon\nX,Y,jd@x.fr,Nice\n");
        let rules = vec![
            Rule { label: "same name + city".into(), ..Rule::new(vec![name(), Part::Field(Field { col: 3, clean: Clean::Slug })], "") },
            Rule::new(vec![Part::Any { cols: vec![2], clean: Clean::Mail { canon: false }, country: None }], "same e-mail"),
        ];
        let d = find_r(&t, &rules);
        assert_eq!(d.rows(), 3);
        assert_eq!(d.confidence(0), 80);
        assert_eq!(d.hits, vec![2, 2]);
    }

    #[test]
    fn detected_roles() {
        let t = table("Prénom,Nom,Nom complet,E-mail,Billing_email,Created_by,Téléphone,Portable,Société,Ville,Pays,record_id\n\
            Jean,Dupont,Jean DUPONT,jd@x.fr,compta@x.fr,admin@x.fr,01 02 03 04 05,06 12 34 56 78,Acme SAS,Lyon,France,recA1b2C3d4E5\n\
            Ana,Lopez,Ana LOPEZ,ana@y.fr,compta@y.fr,admin@x.fr,01 22 33 44 55,07 12 34 56 78,Bidule,Paris,France,recB1b2C3d4E5\n\
            Léo,Petit,Léo PETIT,leo@z.fr,,admin@x.fr,,06 99 88 77 66,Acme SAS,Nice,Belgique,recC1b2C3d4E5\n");
        let r = roles(&t.st);
        assert_eq!((r.first, r.last, r.full.clone()), (Some(0), Some(1), vec![2]));
        assert_eq!(r.emails, vec![3]);
        assert_eq!(r.phones, vec![6, 7]);
        assert_eq!((r.company, r.city, r.country, r.cc), (Some(8), Some(9), Some(10), 33));
        let t = table("id,firstName,lastName,fullName_fx,phoneNumber,Name,Email_perso\n1,Cedric,Durand,Cedric DURAND,+33681304083,SOMATICO,c@gmail.com\n2,Ana,Lopez,Ana LOPEZ,0622155942,Eiffage Energie,\n3,Léo,Petit,Léo PETIT,,SOMATICO,l@x.io\n");
        let r = roles(&t.st);
        assert_eq!((r.first, r.last, r.full.clone(), r.company), (Some(1), Some(2), vec![3], Some(5)));
        assert_eq!((r.phones.clone(), r.emails.clone()), (vec![4], vec![6]));
        let t = table("first_name,last_name,email,phone,country\nRobert,Taylor,r@x.com,+1 (203) 893-3004,United States\nAna,Moore,a@y.com,+1 (746) 177-1109,United States\n");
        let r = roles(&t.st);
        assert_eq!((r.first, r.last, r.phones.clone(), r.country, r.cc), (Some(0), Some(1), vec![3], Some(4), 1));
    }

    #[test]
    fn groups_cross_checked_on_names() {
        let t = table("first,last,phone,email\nJean,Dupont,0612345678,a@x.fr\nAnne,Martin,0612345678,b@x.fr\nJérémy,James,0700000002,j@x.fr\nJérémi,JAMES,0700000002,j@x.fr\nJ.,Petit,0700000003,\nJules,Petit,0700000003,\nLéo,Roux,0700000004,l@x.fr\nLeo,ROUX,,l@x.fr\n");
        let mail = || Part::Any { cols: vec![3], clean: Clean::Mail { canon: false }, country: None };
        let rules = [
            Rule::new(vec![Part::Any { cols: vec![2], clean: Clean::E164 { cc: 33 }, country: None }], "same phone"),
            Rule::new(vec![names(&[0, 1]), mail()], "same name + e-mail"),
            Rule::new(vec![mail()], "same e-mail"),
        ];
        let d = find_rules(&t.st, &all(&t), &rules, &[0, 1], &AtomicBool::new(false), None).unwrap();
        assert_eq!(groups(&t, &d), vec![vec!["Jean", "Anne"], vec!["Jérémy", "Jérémi"], vec!["J.", "Jules"], vec!["Léo", "Leo"]]);
        assert_eq!(d.reasons(0), vec!["same phone", "different names"]);
        assert_eq!(d.confidence(0), 30);
        assert_eq!(d.reasons(1), vec!["same e-mail", "same phone", "names spelled differently"]);
        assert_eq!(d.reasons(2), vec!["same phone", "names spelled differently"]);
        // Name + e-mail says the e-mail already.
        assert_eq!(d.reasons(3), vec!["same name + e-mail"]);
        assert_eq!(d.order(Order::LeastSure)[0], 0);
    }

    #[test]
    fn merges() {
        let t = table("a,b,c\n1,,x\n2,b,y\n,c,\n");
        let ids = all(&t);
        let m = merge(&t.st, ids[0], &ids[1..]);
        assert_eq!(m.row, vec!["1", "b", "x"]);
        assert_eq!((m.filled, m.kept), (1, 2));
    }

    #[test]
    fn edited_rows_count() {
        let mut t = table("a,b\n1,x\n2,y\n");
        let id = t.st.data_id(1);
        t.set_cells(&[(id, 1, "X".into())]);
        let spec = DupSpec { keys: vec![key(&[(1, Clean::Loose)])] };
        let d = find(&t.st, &all(&t), &spec, &AtomicBool::new(false), None).unwrap();
        assert_eq!(d.groups(), 1);
    }
}
