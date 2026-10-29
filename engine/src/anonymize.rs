//! Anonymise: a copy of a CRM export can be shared for a test or a demo once
//! the people in it cannot be recognised. Each column gets a treatment,
//! guessed from its title and values: names become other names with the
//! same initial, taken from short lists; an e-mail keeps its shape (user part
//! treated like a name, domain swapped for a reserved one); a phone number
//! keeps its country code and first two digits; a postal code keeps its
//! department; a date is shifted by up to a year; anything else can be
//! scrambled letter for letter, its shape kept. Every replacement is derived
//! from a hash of the value, so the same value gives the same result in
//! every row, column and file — duplicates stay duplicates, joins still hold.
//! The hash and generator are the JS ones (FNV-1a over UTF-16 units,
//! mulberry32): files anonymised by either version agree.

use crate::split::{remove_accents, slugify};
use crate::table::{RowId, State};
use rayon::prelude::*;
use regex::Regex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::LazyLock;

static FIRST_NAMES: &[&str] = &["Alice", "Antoine", "Aurélie", "Adrien", "Anaïs", "Alain", "Benoît", "Brigitte", "Bastien", "Béatrice", "Bruno", "Camille", "Clément", "Céline", "Christophe", "Chloé",
    "David", "Delphine", "Damien", "Dominique", "Émilie", "Éric", "Elsa", "Étienne", "Florence", "Fabien", "François", "Fanny", "Gaëlle", "Guillaume", "Gérard", "Gabrielle", "Hélène", "Hugo", "Henri",
    "Inès", "Isabelle", "Ismaël", "Julie", "Julien", "Jeanne", "Jérôme", "Karine", "Kevin", "Kamel", "Laura", "Lucas", "Laurent", "Léa", "Marie", "Mathieu", "Manon", "Maxime", "Nathalie", "Nicolas", "Nadia", "Noé",
    "Olivier", "Océane", "Omar", "Pauline", "Pierre", "Philippe", "Patricia", "Quentin", "Quitterie", "Romain", "Rachel", "Rémi", "Roxane", "Sophie", "Samuel", "Sandrine", "Simon", "Thomas", "Théa", "Thierry",
    "Ulysse", "Ursula", "Valérie", "Vincent", "Victor", "Véronique", "William", "Wendy", "Xavier", "Xénia", "Yann", "Yasmine", "Yves", "Zoé", "Zacharie"];
static LAST_NAMES: &[&str] = &["Andrieu", "Aubert", "Arnaud", "Allard", "Bernard", "Blanc", "Bonnet", "Brun", "Chevalier", "Colin", "Caron", "Clément", "Dubois", "Durand", "Dumont", "Delorme", "Étienne", "Evrard",
    "Fontaine", "Faure", "Fournier", "Garnier", "Girard", "Guérin", "Henry", "Hubert", "Hamon", "Imbert", "Israël", "Joly", "Jacquet", "Klein", "Keller", "Lambert", "Lefèvre", "Leroy", "Lemoine",
    "Martin", "Morel", "Mercier", "Masson", "Noël", "Nguyen", "Nicolas", "Olivier", "Ollivier", "Perrin", "Petit", "Picard", "Pons", "Quéré", "Quintin", "Roux", "Renard", "Rolland", "Rey",
    "Simon", "Schmitt", "Sauvage", "Thomas", "Tessier", "Toussaint", "Urbain", "Vidal", "Vasseur", "Vincent", "Weber", "Wagner", "Xavier", "Yvon", "Zimmermann"];
static COMPANIES: &[&str] = &["Alpha Conseil", "Atelier Nord", "Bleu Horizon", "Boréal Services", "Cap Ouest", "Corail Industrie", "Delta Logistique", "Dune Digital",
    "Écume Studio", "Élan Distribution", "Fabrique du Sud", "Forge Numérique", "Granit Bâtiment", "Galaxie Media", "Horizon Immo", "Héliotrope",
    "Indigo Formation", "Ilot Vert", "Jade Solutions", "Juniper Tech", "Kaolin Design", "Korrigan Transport", "Lumen Énergie", "Lagune Traiteur",
    "Méridien Assurances", "Mosaïque Web", "Nacre Cosmétique", "Nordik Outillage", "Onyx Sécurité", "Orée Paysages", "Prisme Audit", "Pollen Agri",
    "Quartz Optique", "Quai des Arts", "Rivage Voyages", "Racine Bio", "Sillage Marine", "Sextant Ingénierie", "Tilleul Santé", "Topaze Finance",
    "Ubac Montagne", "Unisson Événements", "Vallon Menuiserie", "Vertige Sport", "Wagon Éditions", "Westline Textile", "Xénon Éclairage", "Yucca Jardins",
    "Zénith Consulting", "Zéphyr Aéro"];
const STREETS: &[&str] = &["rue des Lilas", "avenue de la Gare", "boulevard Victor Hugo", "chemin des Vignes", "allée des Peupliers", "place du Marché", "impasse des Roses",
    "rue de la Paix", "rue Pasteur", "avenue Jean Jaurès", "rue des Écoles", "route de Lyon", "rue du Moulin", "cours Mirabeau", "quai des Brumes",
    "rue Émile Zola", "rue Saint-Michel", "avenue des Champs", "chemin du Lavoir", "rue de l'Église", "rue des Fontaines", "boulevard de la République",
    "allée du Stade", "rue du Port"];
/// Reserved names: they never deliver.
const DOMAINS: &[&str] = &["example.com", "example.org", "example.net", "mail.example", "courrier.example", "contact.example", "demo.example", "test.example"];
const PARTICLES: &[&str] = &["de", "du", "des", "la", "le", "les", "van", "von", "der", "den", "el", "al", "di", "da", "y", "et", "and", "of", "the", "ben", "bin", "ibn", "mc", "mac", "d'", "l'"];
/// Country codes of one or two digits; the others take three.
const CC2: &[&str] = &["20", "27", "30", "31", "32", "33", "34", "36", "39", "40", "41", "43", "44", "45", "46", "47", "48", "49", "51", "52", "53", "54", "55", "56", "57", "58",
    "60", "61", "62", "63", "64", "65", "66", "81", "82", "84", "86", "90", "91", "92", "93", "94", "95", "98"];

fn re(s: &str) -> Regex {
    Regex::new(s).unwrap()
}

// `(?-u:\b)` where the JS had a non-Unicode \b: "allée" is not followed by a
// word boundary there, and "12é" has one — kept as it was.
static STREET_TYPE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(?:rue|avenue|av\.?|boulevard|bd\.?|chemin|allée|allee|place|impasse|route|cours|quai|square|passage|voie|résidence|residence|lotissement|hameau|clos|sentier|promenade|esplanade|traverse|villa|mail)(?-u:\b)\.?"));
static LEGAL_FORM: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\s*[-,]?\s*(?-u:\b)(?:SARL|SASU|SAS|SA|EURL|SCI|SCOP|SNC|SELARL|GIE|GmbH|AG|Ltd\.?|LLC|Inc\.?|Corp\.?|SRL|BV|NV|SL|SpA|Oy|AB|AS)\s*$"));
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^([0-9]+)\s*(?:bis|ter|[a-z])?(?-u:\b)\s*(.*)$"));
static WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\p{L}[\p{L}'’]*"));
static LETTERS_OR_DIGITS: LazyLock<Regex> = LazyLock::new(|| re(r"\p{L}+|[0-9]+"));
static LETTER_OR_DIGIT: LazyLock<Regex> = LazyLock::new(|| re(r"[\p{L}0-9]"));
static EMAIL: LazyLock<Regex> = LazyLock::new(|| re(r"^([^@\s]+)@([^@\s]+)$"));
static EMAIL_LIKE: LazyLock<Regex> = LazyLock::new(|| re(r"^[^@\s]+@[^@\s]+\.[^@\s]+$"));
static DATE: LazyLock<Regex> = LazyLock::new(|| re(r"^([0-9]{1,4})[-/.]([0-9]{1,2})[-/.]([0-9]{1,4})((?:[ T][0-9]{1,2}:[0-9]{2}.*)?)$"));
static FORMAT_CHARS: LazyLock<Regex> = LazyLock::new(|| re(r"\p{Cf}"));

/// FNV-1a over the UTF-16 units, as the JS `charCodeAt` loop.
pub fn hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for u in s.encode_utf16() {
        h ^= u as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// mulberry32 seeded with a hash: "random" draws that are the same for the same value.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Rng {
        Rng(if seed == 0 { 1 } else { seed })
    }

    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6D2B79F5);
        let a = self.0;
        let mut x = (a ^ (a >> 15)).wrapping_mul(1 | a);
        x = x.wrapping_add((x ^ (x >> 7)).wrapping_mul(61 | x)) ^ x;
        (x ^ (x >> 14)) as f64 / 4294967296.0
    }

    fn digit(&mut self) -> char {
        (b'0' + (self.next() * 10.0) as u8) as char
    }

    fn letter(&mut self) -> char {
        (b'a' + (self.next() * 26.0) as u8) as char
    }
}

fn initial_of(w: &str) -> Option<char> {
    remove_accents(&w.to_lowercase()).chars().find(|c| c.is_ascii_lowercase())
}

/// The words of a list starting with a letter (the whole list when none does).
fn bucket(list: &'static [&'static str], letter: Option<char>) -> &'static [&'static str] {
    static BUCKETS: LazyLock<HashMap<usize, Vec<Vec<&'static str>>>> = LazyLock::new(|| {
        [FIRST_NAMES, LAST_NAMES, COMPANIES].iter().map(|l| {
            let mut b = vec![Vec::new(); 26];
            for w in l.iter() {
                if let Some(c) = initial_of(w) { b[(c as u8 - b'a') as usize].push(*w); }
            }
            (l.as_ptr() as usize, b)
        }).collect()
    });
    let b = letter.and_then(|c| BUCKETS.get(&(list.as_ptr() as usize)).map(|b| &b[(c as u8 - b'a') as usize]));
    match b {
        Some(v) if !v.is_empty() => v,
        _ => list,
    }
}

/// The case of the original word: DUPONT → DURAND, Dupont → Durand, dupont → durand.
fn case_like(src: &str, w: &str) -> String {
    if src == src.to_uppercase() && src.chars().any(char::is_uppercase) {
        return w.to_uppercase();
    }
    if src.chars().next().is_some_and(|c| c.to_lowercase().eq(std::iter::once(c))) {
        return w.to_lowercase();
    }
    w.to_string()
}

fn anon_word(w: &str, list: &'static [&'static str]) -> String {
    let c = bucket(list, initial_of(w));
    case_like(w, c[hash(&remove_accents(&w.to_lowercase())) as usize % c.len()])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameKind {
    First,
    Last,
    /// The first word a first name, the rest a last name.
    Full,
}

/// Names: each word replaced by one with the same initial — particles (de, van…) kept.
fn anon_name(v: &str, kind: NameKind) -> String {
    let mut out = String::with_capacity(v.len());
    let (mut at, mut k) = (0, 0);
    let mut last: Option<&'static [&'static str]> = None;
    for m in WORD.find_iter(v) {
        out.push_str(&v[at..m.start()]);
        at = m.end();
        let w = m.as_str();
        let low = w.to_lowercase();
        // Words of two letters after the first are kept too (initials, "Jr").
        if PARTICLES.contains(&low.as_str()) || PARTICLES.contains(&low.replacen('’', "'", 1).as_str()) || (w.encode_utf16().count() <= 2 && k > 0) {
            out.push_str(w);
            continue;
        }
        // Jean-Pierre: both halves are first names.
        let hyphen = if m.start() > 0 && v.as_bytes()[m.start() - 1] == b'-' { last } else { None };
        let list = hyphen.unwrap_or(match kind {
            NameKind::First => FIRST_NAMES,
            NameKind::Last => LAST_NAMES,
            NameKind::Full => if k == 0 { FIRST_NAMES } else { LAST_NAMES },
        });
        if hyphen.is_none() { k += 1; }
        last = Some(list);
        out.push_str(&anon_word(w.strip_suffix(['’', '\'']).unwrap_or(w), list));
    }
    out.push_str(&v[at..]);
    out
}

fn anon_digits(s: &str, rnd: &mut Rng) -> String {
    s.chars().map(|c| if c.is_ascii_digit() { rnd.digit() } else { c }).collect()
}

fn anon_email(v: &str) -> String {
    let Some(m) = EMAIL.captures(v.trim()) else { return anon_text(v) };
    let mut rnd = Rng::new(hash(&v.to_lowercase()));
    let local = LETTERS_OR_DIGITS.replace_all(&m[1], |p: &regex::Captures| {
        let p = &p[0];
        if p.bytes().any(|b| b.is_ascii_digit()) { anon_digits(p, &mut rnd) } else { anon_word(p, FIRST_NAMES).to_lowercase() }
    });
    format!("{}@{}", remove_accents(&local), DOMAINS[hash(&m[2].to_lowercase()) as usize % DOMAINS.len()])
}

fn cc_len(d: &str) -> usize {
    if d.starts_with('1') || d.starts_with('7') { 1 } else if d.len() >= 2 && CC2.contains(&&d[..2]) { 2 } else { 3 }
}

/// The country code (after + or 00) and the first two national digits stay;
/// the other digits are drawn from the hash, the spacing and punctuation kept.
fn anon_phone(v: &str) -> String {
    let digits: String = v.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 4 { return v.to_string(); }
    let mut rnd = Rng::new(hash(&digits));
    let t = v.trim_start();
    let intl = if t.starts_with('+') {
        cc_len(&digits)
    } else if t.starts_with("00") && t.as_bytes().get(2).is_some_and(u8::is_ascii_digit) {
        2 + cc_len(&digits[2..])
    } else {
        0
    };
    let keep = if intl > 0 { intl + 1 } else { 2 };
    let mut k = 0;
    v.chars().map(|c| {
        if !c.is_ascii_digit() { return c; }
        k += 1;
        if k <= keep { c } else { rnd.digit() }
    }).collect()
}

/// Letters for letters, digits for digits, the first `keep` kept.
fn redraw(v: &str, rnd: &mut Rng, mut keep: usize) -> String {
    LETTER_OR_DIGIT.replace_all(v, |m: &regex::Captures| {
        let ch = &m[0];
        if keep > 0 { keep -= 1; return ch.to_string(); }
        if ch.as_bytes()[0].is_ascii_digit() { rnd.digit().to_string() } else { case_like(ch, &rnd.letter().to_string()) }
    }).into_owned()
}

/// Postal codes: the department (first two characters, three for the
/// overseas 97x / 98x) stays, the rest is redrawn.
fn anon_postal(v: &str) -> String {
    let s = v.trim();
    if s.is_empty() { return v.to_string(); }
    let b = s.as_bytes();
    let overseas = b.len() == 5 && b[0] == b'9' && (b[1] == b'7' || b[1] == b'8') && b[2..].iter().all(u8::is_ascii_digit);
    redraw(v, &mut Rng::new(hash(s)), if overseas { 3 } else { 2 })
}

fn anon_company(v: &str) -> String {
    let cut = LEGAL_FORM.find(v).map_or(v.len(), |m| m.start());
    let (base, form) = v.split_at(cut);
    let w = base.trim();
    if w.is_empty() { return v.to_string(); }
    let c = bucket(COMPANIES, initial_of(w));
    case_like(w, c[hash(&remove_accents(&w.to_lowercase())) as usize % c.len()]) + form
}

fn anon_address(v: &str) -> String {
    let s = v.trim();
    if s.is_empty() { return v.to_string(); }
    let h = hash(&remove_accents(&s.to_lowercase())) as usize;
    let m = NUMBERED.captures(s);
    let rest = m.as_ref().map_or(s, |m| m.get(2).unwrap().as_str());
    let mut street = STREETS[h % STREETS.len()].to_string();
    // The original kind of way (avenue, chemin…) kept.
    if let Some(t) = STREET_TYPE.find(rest) {
        street = format!("{}{}", t.as_str(), &street[street.find(' ').unwrap()..]);
    }
    // 12 RUE DES LILAS stays in capitals.
    if rest == rest.to_uppercase() && rest.chars().any(char::is_uppercase) {
        street = street.to_uppercase();
    }
    if m.is_some() { format!("{} {street}", 1 + h % 150) } else { street }
}

/// Days since 1970-01-01 of a date (proleptic Gregorian; Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

fn this_year() -> i64 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_from_days((secs / 86400) as i64).0
}

/// A date as `y-m-d`, `d/m/y` (or `m/d/y`), two-digit years included, and
/// what follows it (a time). As convert's date reader.
fn parse_date_cell(s: &str, month_first: bool) -> Option<(i64, u32, u32, String)> {
    let m = DATE.captures(s)?;
    let (a, b, c) = (&m[1], &m[2], &m[3]);
    let (y, mo, d);
    if a.len() == 4 {
        if c.len() > 2 { return None; }
        (y, mo, d) = (a, b, c);
    } else {
        if a.len() > 2 || (c.len() != 2 && c.len() != 4) { return None; }
        y = c;
        (d, mo) = if month_first { (b, a) } else { (a, b) };
    }
    let (mut y, mo, d): (i64, u32, u32) = (y.parse().ok()?, mo.parse().ok()?, d.parse().ok()?);
    // Two-digit years: up to ten years ahead is this century, the rest the last one (birth dates).
    if a.len() != 4 && c.len() == 2 {
        y += if y <= this_year() % 100 + 10 { 2000 } else { 1900 };
    }
    if !(1..=12).contains(&mo) || d < 1 { return None; }
    let next = if mo == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, mo + 1, 1) };
    if d as i64 > next - days_from_civil(y, mo, 1) { return None; }
    Some((y, mo, d, m[4].to_string()))
}

/// Dates move by up to a year either way, in their own format.
fn anon_date(v: &str) -> Option<String> {
    let s = v.trim();
    let (y, mo, d, rest) = parse_date_cell(s, false)?;
    let shift = (hash(s) % 731) as i64 - 365;
    let (y, mo, d) = civil_from_days(days_from_civil(y, mo, d) + shift);
    let b = s.as_bytes();
    if b.len() >= 4 && b[..4].iter().all(u8::is_ascii_digit) {
        return Some(format!("{y}-{mo:02}-{d:02}{rest}"));
    }
    let lead = b.iter().take_while(|c| c.is_ascii_digit()).count();
    let sep = if (1..=2).contains(&lead) && matches!(b.get(lead), Some(b'-' | b'/' | b'.')) { b[lead] as char } else { '/' };
    // d/m/yy keeps two digits.
    static SHORT: LazyLock<Regex> = LazyLock::new(|| re(r"^[0-9]{1,2}[-/.][0-9]{1,2}[-/.][0-9]{2}"));
    let short = SHORT.find(s).is_some_and(|m| !b.get(m.end()).is_some_and(u8::is_ascii_digit));
    let ys = y.to_string();
    let yy = if short { &ys[ys.len().saturating_sub(2)..] } else { &ys };
    Some(format!("{d:02}{sep}{mo:02}{sep}{yy}{rest}"))
}

/// Letters for letters, digits for digits: the shape stays, the meaning goes.
fn anon_text(v: &str) -> String {
    redraw(v, &mut Rng::new(hash(v)), 0)
}

/// A phone number in E.164 (+33612345678), a national one taking `cc`; None if it is not one.
fn parse_phone_cell(s: &str, cc: &str) -> Option<String> {
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mut x: String = s.replace("(0)", "").chars().filter(|c| !(c.is_whitespace() || matches!(c, '.' | '-' | '(' | ')' | '/'))).collect();
    if let Some(r) = x.strip_prefix("00") { x = format!("+{r}"); }
    if !x.starts_with('+') {
        if cc.is_empty() { return None; }
        let b = x.as_bytes();
        if b.first() == Some(&b'0') && (7..=15).contains(&b.len()) && digits(&x) {
            x = format!("+{cc}{}", &x[1..]);
        } else if cc == "33" && b.len() == 9 && b[0] != b'0' && digits(&x) {
            x = format!("+33{x}");
        } else if cc == "33" && digits(&x) && x.starts_with("33") && ((x.len() == 11 && b[2] != b'0') || (x.len() == 12 && b[2] == b'0' && b[3] != b'0')) {
            x = format!("+33{}", &x[x.len() - 9..]);   // the + lost, sometimes the 0 kept
        } else {
            return None;
        }
    }
    // +33 06…: the national 0 kept after the code.
    if x.len() == 13 && x.starts_with("+330") && x.as_bytes()[4] != b'0' && digits(&x[1..]) {
        x = format!("+33{}", &x[x.len() - 9..]);
    }
    if !(x.len() >= 9 && x.len() <= 16 && x.as_bytes()[1] != b'0' && digits(&x[1..])) { return None; }
    if x.starts_with("+33") && x.len() != 12 { return None; }
    Some(x)
}

/// A column's treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Keep,
    First,
    Last,
    Full,
    Email,
    Phone,
    Postal,
    Address,
    Company,
    Date,
    Text,
    Blank,
}

impl Mode {
    pub const ALL: [Mode; 12] = [Mode::Keep, Mode::First, Mode::Last, Mode::Full, Mode::Email, Mode::Phone, Mode::Postal, Mode::Address, Mode::Company, Mode::Date, Mode::Text, Mode::Blank];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Keep => "Keep as is",
            Mode::First => "First name",
            Mode::Last => "Last name",
            Mode::Full => "Full name",
            Mode::Email => "E-mail",
            Mode::Phone => "Phone number",
            Mode::Postal => "Postal code",
            Mode::Address => "Address",
            Mode::Company => "Company",
            Mode::Date => "Date, shifted",
            Mode::Text => "Scramble, same shape",
            Mode::Blank => "Clear",
        }
    }

    /// A value's fake; None: not recognised (a date that is none), kept as it is.
    pub fn apply(self, v: &str) -> Option<String> {
        Some(match self {
            Mode::Keep => v.to_string(),
            Mode::First => anon_name(v, NameKind::First),
            Mode::Last => anon_name(v, NameKind::Last),
            Mode::Full => anon_name(v, NameKind::Full),
            Mode::Email => anon_email(v),
            Mode::Phone => anon_phone(v),
            Mode::Postal => anon_postal(v),
            Mode::Address => anon_address(v),
            Mode::Company => anon_company(v),
            Mode::Date => return anon_date(v),
            Mode::Text => anon_text(v),
            Mode::Blank => String::new(),
        })
    }
}

/// The first non-empty values of a column (trimmed), in file order.
fn sample(st: &State, col: usize, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut tmp = Vec::new();
    for p in 0..st.len() {
        let v = st.with_cell(st.data_id(p), col, &mut tmp, |s| s.trim().to_string());
        if !v.is_empty() {
            out.push(v);
            if out.len() >= n { break; }
        }
    }
    out
}

/// What a column most likely holds, from its title and its first 300 values.
pub fn guess(st: &State, col: usize) -> Mode {
    static RES: LazyLock<Vec<Regex>> = LazyLock::new(|| [
        r"\b(tel|telephone|phone|mobile|portable|fax|gsm|cell|cellulaire)\b",
        r"\b(code postal|codepostal|cp|zip|zipcode|postcode|postal)\b",
        r"\b(adresse|address|rue|street|voie|adr)\b",
        r"\b(mail|email|e mail|ip|web|site|url)\b",
        r"\b(societe|company|entreprise|organisation|organization|raison sociale|employeur|employer|firme|compte|account|client)\b",
        r"\b(id|numero|number|no|code|type)\b",
        r"\b(prenom|prenoms|firstname|first name|givenname|given)\b",
        r"\b(nom complet|fullname|full name|contact|interlocuteur|dirigeant|responsable|signataire|commercial|manager)\b",
        r"\b(nom|noms|lastname|last name|surname|family name|name|patronyme)\b",
        r"\b(fichier|file|societe|company|produit|product|projet|project|domaine|domain|user|utilisateur|login|campagne|rue|etablissement)\b",
        r"\b(iban|bic|rib|siret|siren|nir|secu|securite sociale|passeport|passport|cni|permis|licence|vat|tva|numero de compte)\b",
        r"\b(naissance|birth|birthday|dob|ne le|nee le)\b",
        r"\b(login|user|username|identifiant|pseudo|nickname)\b",
    ].iter().map(|s| re(s)).collect());
    let h = slugify(st.headers().get(col).map_or("", |s| s.as_str()));
    let vals = sample(st, col, 300);
    let share = |f: &dyn Fn(&str) -> bool| if vals.is_empty() { 0.0 } else { vals.iter().filter(|v| f(v)).count() as f64 / vals.len() as f64 };
    let has = |k: usize| RES[k].is_match(&h);
    if share(&|v| EMAIL_LIKE.is_match(v)) >= 0.6 { return Mode::Email; }
    if has(0) && share(&|v| parse_phone_cell(&FORMAT_CHARS.replace_all(v, ""), "33").is_some()) >= 0.4 { return Mode::Phone; }
    if has(1) { return Mode::Postal; }
    if has(2) && !has(3) { return Mode::Address; }
    if has(4) && !has(5) { return Mode::Company; }
    if has(6) { return Mode::First; }
    if has(7) { return Mode::Full; }
    if has(8) && !has(9) { return Mode::Last; }
    if has(10) { return Mode::Text; }
    if has(11) && share(&|v| parse_date_cell(v, false).is_some()) >= 0.6 { return Mode::Date; }
    if has(12) { return Mode::Text; }
    Mode::Keep
}

/// Every column's guess.
pub fn guess_all(st: &State) -> Vec<Mode> {
    (0..st.ncols).into_par_iter().map(|c| guess(st, c)).collect()
}

/// The column's first value and what the treatment makes of it (None: not
/// recognised, kept); None for an empty column.
pub fn example(st: &State, col: usize, mode: Mode) -> Option<(String, Option<String>)> {
    let v = sample(st, col, 1).pop()?;
    let nv = mode.apply(&v);
    Some((v, nv))
}

#[derive(Debug, Default)]
pub struct Anonymized {
    /// The cells to set (one undo step): only those that change.
    pub changes: Vec<(RowId, usize, String)>,
    /// Values the treatment did not recognise, kept.
    pub kept: usize,
}

/// The cells of `rows` (e.g. all data rows, or those shown) anonymised per
/// column treatment (`modes[c]`; missing: Keep). Empty cells stay empty. A
/// value's fake is the same everywhere, so it is computed once per distinct
/// value per column and thread (capped, for a column of unique e-mails).
/// None if cancelled; `progress` counts rows.
pub fn anonymize(st: &State, rows: &[RowId], modes: &[Mode], cancel: &AtomicBool, progress: Option<&AtomicUsize>) -> Option<Anonymized> {
    const CH: usize = 1 << 13;
    const MEMO: usize = 100_000;
    let cols: Vec<(usize, Mode)> = modes.iter().enumerate().filter(|(_, m)| **m != Mode::Keep).map(|(c, m)| (c, *m)).collect();
    if cols.is_empty() { return Some(Anonymized::default()); }
    let parts: Vec<Anonymized> = rows
        .par_chunks(CH)
        .map_init(
            || (vec![HashMap::<String, Option<String>>::new(); cols.len()], Vec::new()),
            |(memo, tmp), chunk| {
                let mut out = Vec::new();
                let mut kept = 0;
                if cancel.load(Ordering::Relaxed) { return Anonymized { changes: out, kept }; }
                for &id in chunk {
                    for (k, &(c, mode)) in cols.iter().enumerate() {
                        let m = &mut memo[k];
                        st.with_cell(id, c, tmp, |old| {
                            if old.trim().is_empty() { return; }
                            let nv = match m.get(old) {
                                Some(v) => v.clone(),
                                None => {
                                    let v = mode.apply(old);
                                    if m.len() < MEMO { m.insert(old.to_string(), v.clone()); }
                                    v
                                }
                            };
                            match nv {
                                None => kept += 1,
                                Some(nv) if nv != old => out.push((id, c, nv)),
                                _ => {}
                            }
                        });
                    }
                }
                if let Some(p) = progress { p.fetch_add(chunk.len(), Ordering::Relaxed); }
                Anonymized { changes: out, kept }
            },
        )
        .collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let kept = parts.iter().map(|p| p.kept).sum();
    let mut changes = Vec::with_capacity(parts.iter().map(|p| p.changes.len()).sum());
    for p in parts { changes.extend(p.changes); }
    Some(Anonymized { changes, kept })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::{Base, OpenOptions};
    use crate::table::Table;
    use std::sync::Arc;

    #[test]
    fn hash_and_rng_match_js() {
        // Values from the JS: anonHash('abc'), anonRng(1)() ×2.
        assert_eq!(hash("abc"), 440920331);
        assert_eq!(hash("é"), 1812687940);
        assert_eq!(hash("Dupont€😀"), 2938928010);
        let mut r = Rng::new(1);
        assert_eq!(r.next(), 0.6270739405881613);
        assert_eq!(r.next(), 0.002735721180215478);
        let mut r = Rng::new(hash("x"));
        assert_eq!([r.next(), r.next(), r.next()], [0.4801359330303967, 0.10925848642364144, 0.8231615843251348]);
    }

    #[test]
    fn names_keep_initial_and_case() {
        let v = Mode::Full.apply("Jean-Pierre de LA FONTAINE").unwrap();
        let w: Vec<&str> = v.split([' ', '-']).collect();
        assert_eq!(w.len(), 5);
        assert!(w[0].starts_with('J') && w[1].starts_with('P'), "{v}");
        assert!(FIRST_NAMES.contains(&w[0]) && FIRST_NAMES.contains(&w[1]), "{v}");
        assert_eq!(w[2], "de");
        assert_eq!(w[3], "LA");    // a particle
        assert!(w[4].starts_with('F') && w[4] == w[4].to_uppercase(), "{v}");
        // Same value, same fake; another value, maybe another.
        assert_eq!(Mode::Last.apply("Dupont"), Mode::Last.apply("Dupont"));
        assert_eq!(Mode::Last.apply("dupont").unwrap(), Mode::Last.apply("Dupont").unwrap().to_lowercase());
    }

    #[test]
    fn shapes_kept() {
        let e = Mode::Email.apply("Jean.Dupont42@acme.fr").unwrap();
        let (l, d) = e.split_once('@').unwrap();
        assert!(DOMAINS.contains(&d));
        assert!(l.starts_with('j') && l.contains('.') && l.ends_with(|c: char| c.is_ascii_digit()), "{e}");
        let p = Mode::Phone.apply("+33 6 12 34 56 78").unwrap();
        assert!(p.starts_with("+33 6 ") && p.len() == "+33 6 12 34 56 78".len(), "{p}");
        let p = Mode::Phone.apply("06.12.34.56.78").unwrap();
        assert!(p.starts_with("06.") && p.matches('.').count() == 4, "{p}");
        let z = Mode::Postal.apply("75011").unwrap();
        assert!(z.starts_with("75") && z.len() == 5);
        assert!(Mode::Postal.apply("97411").unwrap().starts_with("974"));
        let a = Mode::Address.apply("12 bis AVENUE DES CHAMPS").unwrap();
        assert!(a.contains("AVENUE ") && a == a.to_uppercase(), "{a}");
        assert!(Mode::Address.apply("3 chemin du bois").unwrap().contains(" chemin "));
        let c = Mode::Company.apply("Dupont et Fils SARL").unwrap();
        assert!(c.ends_with(" SARL") && c.starts_with('D'), "{c}");
        let t = Mode::Text.apply("AB-12x").unwrap();
        assert!(t.len() == 6 && t.as_bytes()[2] == b'-' && t[..2] == t[..2].to_uppercase() && t.as_bytes()[3].is_ascii_digit(), "{t}");
        assert_eq!(Mode::Blank.apply("x").unwrap(), "");
    }

    #[test]
    fn dates_shift_in_their_format() {
        assert_eq!(Mode::Date.apply("not a date"), None);
        for s in ["1984-03-15", "15/03/1984", "15.03.84", "1984-03-15 10:20"] {
            let v = Mode::Date.apply(s).unwrap();
            assert_eq!(v.len(), s.len(), "{s} → {v}");
            let shift = (hash(s) % 731) as i64 - 365;
            let p = parse_date_cell(s, false).unwrap();
            let q = parse_date_cell(&v, false).unwrap();
            assert_eq!(days_from_civil(q.0, q.1, q.2) - days_from_civil(p.0, p.1, p.2), shift, "{s} → {v}");
            assert_eq!(p.3, q.3);
        }
        assert_eq!(parse_date_cell("31/02/2024", false), None);
        assert!(parse_date_cell("29/02/2024", false).is_some());
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn phones_read() {
        assert_eq!(parse_phone_cell("06 12 34 56 78", "33").as_deref(), Some("+33612345678"));
        assert_eq!(parse_phone_cell("612345678", "33").as_deref(), Some("+33612345678"));
        assert_eq!(parse_phone_cell("+33 (0)6 12 34 56 78", "33").as_deref(), Some("+33612345678"));
        assert_eq!(parse_phone_cell("0033612345678", "33").as_deref(), Some("+33612345678"));
        assert_eq!(parse_phone_cell("hello", "33"), None);
    }

    #[test]
    fn guesses_and_applies() {
        let csv = "Prénom;Nom;E-mail;Téléphone;Code postal;Adresse;Société;Date de naissance;Ville\n\
            Jean;Dupont;jean@acme.fr;06 12 34 56 78;75011;12 rue des Lilas;Acme SARL;15/03/1984;Paris\n\
            Anne;Roux;anne@acme.fr;07 98 76 54 32;69003;3 avenue Foch;Bidule SA;not a date;Lyon\n\
            Zoé;Dupont;;;;;;01/02/1990;Paris\n";
        let t = Table::new(Arc::new(Base::from_bytes(csv.as_bytes().to_vec(), OpenOptions::default())), None);
        let modes = guess_all(&t.st);
        assert_eq!(modes, [Mode::First, Mode::Last, Mode::Email, Mode::Phone, Mode::Postal, Mode::Address, Mode::Company, Mode::Date, Mode::Keep]);
        let rows: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        let r = anonymize(&t.st, &rows, &modes, &AtomicBool::new(false), None).unwrap();
        assert_eq!(r.kept, 1);    // "not a date"
        assert!(r.changes.iter().all(|c| c.1 != 8 && !c.2.is_empty()));
        let fake = |p: usize| r.changes.iter().find(|c| c.0 == rows[p] && c.1 == 1).map(|c| c.2.clone());
        assert!(fake(0).is_some());
        assert_eq!(fake(0), fake(2));    // Dupont twice: one fake
        assert_eq!(example(&t.st, 0, Mode::First).unwrap().0, "Jean");
    }
}
