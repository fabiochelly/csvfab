//! phonetic() and similarity(): the duplicates' "Sounds like" (31-duplicates.js).
//!
//! A value's sound is Metaphone's principle on French spelling, per word of
//! its slug: first a French transcription (ph→f, qu→k, c and g softened before
//! e/i/y, nasals as one sound each, eau/au→o, silent final letters dropped…),
//! then Metaphone's merges on it (d=t, v=f, g=k, every nasal N, vowels dropped
//! but an initial one). Dupont / Dupond, Lefebvre / Lefèvre, Philippe /
//! Filippe share their code.

use super::fx::slugify;

/// The code of every word, space-separated (dupSound(v).code).
pub fn phonetic(v: &str) -> String {
    sound(v).1
}

/// (transcription, code) of every word.
pub fn sound(v: &str) -> (String, String) {
    // ç is s: the slug would make it a c, read k
    let v: String = v.chars().map(|c| if c == 'ç' || c == 'Ç' { 's' } else { c }).collect();
    let (mut trans, mut code) = (String::new(), String::new());
    for w in slugify(&v).split(' ').filter(|w| !w.is_empty()) {
        let (t, c) = word(w);
        if !trans.is_empty() || !code.is_empty() {
            trans.push(' ');
            code.push(' ');
        }
        trans.push_str(&t);
        code.push_str(&c);
    }
    (trans, code)
}

/// Replaces, left to right, the first of `alts` that matches at each place
/// and whose next character passes `next` (a regex look-ahead).
fn rep(s: &[char], alts: &[&str], by: &str, next: impl Fn(Option<char>) -> bool) -> Vec<char> {
    let alts: Vec<Vec<char>> = alts.iter().map(|a| a.chars().collect()).collect();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    'scan: while i < s.len() {
        for a in &alts {
            if s[i..].starts_with(a) && next(s.get(i + a.len()).copied()) {
                out.extend(by.chars());
                i += a.len();
                continue 'scan;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

fn any(_: Option<char>) -> bool {
    true
}

fn in_set(set: &'static str) -> impl Fn(Option<char>) -> bool {
    move |c| c.is_some_and(|c| set.contains(c))
}

/// (?=[^aeioumn]|$)
fn nasal_end(c: Option<char>) -> bool {
    c.is_none_or(|c| !"aeioumn".contains(c))
}

/// (.)\1+ → $1
fn dedup(s: &mut Vec<char>) {
    s.dedup();
}

fn word(w: &str) -> (String, String) {
    let mut s: Vec<char> = w.chars().collect();
    s = rep(&s, &["ph"], "f", any);
    s = rep(&s, &["th"], "t", any);
    s = rep(&s, &["ch"], "k", in_set("rl")); // chr, chl: Christophe, Chloé
    s = rep(&s, &["sch", "ch", "sh"], "S", any);
    s = rep(&s, &["ck", "cq", "qu", "q"], "k", any);
    s = rep(&s, &["gu"], "G", in_set("eiy")); // gu before e/i stays hard: Guillaume
    s = rep(&s, &["ge"], "j", in_set("aou"));
    s = rep(&s, &["g"], "j", in_set("eiy"));
    s = rep(&s, &["G"], "g", any);
    s = rep(&s, &["c"], "s", in_set("eiy"));
    s = rep(&s, &["c"], "k", any);
    s = rep(&s, &["w"], "v", any);
    s = rep(&s, &["z"], "s", any);
    s = rep(&s, &["y"], "i", any);
    if s.last() == Some(&'x') {
        s.pop();
    }
    s = rep(&s, &["x"], "ks", any);
    s.retain(|&c| c != 'h');
    s = rep(&s, &["ain", "ein", "aim", "eim"], "1", nasal_end);
    s = rep(&s, &["in", "im", "un", "um"], "1", nasal_end);
    s = rep(&s, &["an", "am", "en", "em"], "2", nasal_end);
    s = rep(&s, &["on", "om"], "3", nasal_end);
    s = rep(&s, &["aul"], "o", in_set("tdx")); // Thibault, Gaultier
    s = rep(&s, &["eau", "au"], "o", any);
    s = rep(&s, &["ou"], "u", any);
    s = rep(&s, &["ai", "ei"], "e", any);
    s = rep(&s, &["oeu", "eu"], "e", any);
    s = rep(&s, &["bv"], "v", any);
    dedup(&mut s);
    for _ in 0..2 {
        // silent final letters
        if s.len() > 2 && s.last().is_some_and(|c| "dtsxzp".contains(*c)) { s.pop(); } else { break }
    }
    if s.len() > 2 && s.last() == Some(&'e') {
        s.pop(); // final mute e
    }
    let trans: String = s.iter().collect();
    // Metaphone's merges: the key casts wide, the spelling distance narrows it.
    let mut m: Vec<char> = s.iter().map(|&c| match c {
        '1' | '2' | '3' => 'N',
        'S' => 'X',
        'd' => 't',
        'v' => 'f',
        'g' => 'k',
        'z' => 's',
        c => c,
    }).collect();
    // (An empty word made "undefined" in the web app — m[0] of "" — a slip not kept.)
    if let Some(&first) = m.first() {
        let head = if "aeiou".contains(first) { 'A' } else { first };
        let mut k: Vec<char> = vec![head];
        k.extend(m[1..].iter().filter(|c| !"aeiou".contains(**c)));
        m = k;
    }
    dedup(&mut m);
    (trans, m.iter().collect::<String>().to_uppercase())
}

/// How alike two texts are, 0 to 1: 1 − edit distance / the longer length (Levenshtein).
pub fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 {
        return 0.0;
    }
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

    #[test]
    fn sounds_alike() {
        let same = |a: &str, b: &str| assert_eq!(phonetic(a), phonetic(b), "{a} / {b}");
        same("Dupont", "Dupond");
        same("Dupont", "Dupon");
        same("Lefebvre", "Lefèvre");
        same("Philippe", "Filippe");
        same("Christophe", "Kristof");
        same("Schmitt", "Schmidt");
        same("Meyer", "Mayer");
        assert_eq!(phonetic("Dupont"), "TPN");
        assert_eq!(phonetic("Jean-Pierre Martin"), phonetic("jean pierre martine"));
        assert_ne!(sound("Martin").0, sound("Martine").0);
        assert_eq!(similarity("abc", "abc"), 1.0);
        assert_eq!(similarity("abcd", "abce"), 0.75);
        assert_eq!(similarity("", "x"), 0.0);
    }
}
