//! The regular expressions of the search and column filters, find & replace and the grid's
//! highlights. A pattern the `regex` crate takes runs there (linear time, nothing changed for
//! it); look-around and back-references, which it refuses, run on fancy-regex as formulas do,
//! with a bound on its backtracking: a cell that would take minutes stops there, counted as not
//! matching (and left as it is by a replace).

/// Steps fancy-regex may backtrack on one search before giving up (as formulas and schemas).
pub const BACKTRACK_LIMIT: usize = 1_000_000;

#[derive(Clone, Debug)]
pub enum Re {
    Fast(regex::Regex),
    /// With the pattern relaxed (`relax`) for the `regex` crate, when it takes it: a cell it
    /// does not match is not searched by the backtracking engine.
    Fancy(fancy_regex::Regex, Option<(regex::Regex, String)>),
}

impl Re {
    /// `pattern` in the `regex` crate's syntax; Err: the message to show, on one line.
    pub fn new(pattern: &str, case_insensitive: bool, size_limit: usize) -> Result<Re, String> {
        match regex::RegexBuilder::new(pattern).case_insensitive(case_insensitive).size_limit(size_limit).build() {
            Ok(r) => Ok(Re::Fast(r)),
            Err(e) if needs_backtracking(&e) => {
                let re = fancy_regex::RegexBuilder::new(pattern)
                    .case_insensitive(case_insensitive)
                    .backtrack_limit(BACKTRACK_LIMIT)
                    .delegate_size_limit(size_limit)
                    .build()
                    .map_err(|e| e.to_string())?;
                let loose = relax(pattern).and_then(|p| Some((regex::RegexBuilder::new(&p).case_insensitive(case_insensitive).size_limit(size_limit).build().ok()?, p)));
                Ok(Re::Fancy(re, loose))
            }
            Err(e) => Err(error_line(&e)),
        }
    }

    #[inline]
    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Re::Fast(r) => r.is_match(s),
            Re::Fancy(r, loose) => loose.as_ref().is_none_or(|l| l.0.is_match(s)) && r.is_match(s).unwrap_or(false),
        }
    }

    /// False only where no match can be (a necessary test, in linear time): a whole record.
    #[inline]
    pub fn maybe(&self, s: &str) -> bool {
        match self {
            Re::Fast(r) => r.is_match(s),
            Re::Fancy(_, loose) => loose.as_ref().is_none_or(|l| l.0.is_match(s)),
        }
    }

    /// The pattern in the `regex` crate's syntax that every match also matches (its literals
    /// tell the records to skip): the pattern itself, or relaxed.
    pub fn plain_pattern(&self) -> Option<&str> {
        match self {
            Re::Fast(r) => Some(r.as_str()),
            Re::Fancy(_, loose) => loose.as_ref().map(|l| l.1.as_str()),
        }
    }

    /// Each match's (start, end), left to right, not overlapping.
    pub fn for_each_match(&self, s: &str, mut f: impl FnMut(usize, usize)) {
        match self {
            Re::Fast(r) => r.find_iter(s).for_each(|m| f(m.start(), m.end())),
            Re::Fancy(r, loose) => {
                if loose.as_ref().is_some_and(|l| !l.0.is_match(s)) { return; }
                for m in r.find_iter(s) {
                    let Ok(m) = m else { break };
                    f(m.start(), m.end());
                }
            }
        }
    }

    /// Groups of the pattern, the whole match included.
    pub fn captures_len(&self) -> usize {
        match self {
            Re::Fast(r) => r.captures_len(),
            Re::Fancy(r, _) => r.captures_len(),
        }
    }

    pub fn has_names(&self) -> bool {
        match self {
            Re::Fast(r) => r.capture_names().any(|n| n.is_some()),
            Re::Fancy(r, _) => r.capture_names().any(|n| n.is_some()),
        }
    }

    pub fn fancy(&self) -> bool {
        matches!(self, Re::Fancy(..))
    }
}

/// The pattern without its look-arounds, its back-references made "any text": it matches
/// wherever the pattern does (assertions only narrow a match; a reference repeats some text).
/// None if it cannot be read that way.
pub fn relax(p: &str) -> Option<String> {
    let c: Vec<char> = p.chars().collect();
    let mut out = String::with_capacity(p.len());
    let mut i = 0;
    let mut class = false;
    while i < c.len() {
        match c[i] {
            '\\' => {
                let n = *c.get(i + 1)?;
                if !class && n.is_ascii_digit() && n != '0' {
                    i += 2;
                    while c.get(i).is_some_and(|d| d.is_ascii_digit()) { i += 1; }
                    out.push_str("(?s:.*?)");
                    continue;
                }
                if !class && n == 'k' && c.get(i + 2) == Some(&'<') {
                    i += c[i..].iter().position(|&x| x == '>')? + 1;
                    out.push_str("(?s:.*?)");
                    continue;
                }
                out.push(c[i]);
                out.push(n);
                i += 2;
                continue;
            }
            '[' if !class => {
                class = true;
                out.push('[');
                i += 1;
                // A ] (or ^]) first is a member, not the end.
                if c.get(i) == Some(&'^') { out.push('^'); i += 1; }
                if c.get(i) == Some(&']') { out.push(']'); i += 1; }
                continue;
            }
            ']' if class => class = false,
            '(' if !class && c.get(i + 1) == Some(&'?') => {
                let look = matches!((c.get(i + 2), c.get(i + 3)), (Some('=' | '!'), _) | (Some('<'), Some('=' | '!')));
                if look {
                    i = close(&c, i)? + 1;
                    out.push_str("(?:)");
                    continue;
                }
            }
            _ => {}
        }
        out.push(c[i]);
        i += 1;
    }
    (!class).then_some(out)
}

/// The index of the parenthesis closing the group opened at `open`.
fn close(c: &[char], open: usize) -> Option<usize> {
    let (mut depth, mut class, mut i) = (0, false, open);
    while i < c.len() {
        match c[i] {
            '\\' => i += 1,
            '[' if !class => {
                class = true;
                if c.get(i + 1) == Some(&'^') { i += 1; }
                if c.get(i + 1) == Some(&']') { i += 1; }
            }
            ']' if class => class = false,
            '(' if !class => depth += 1,
            ')' if !class => {
                depth -= 1;
                if depth == 0 { return Some(i); }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// What the `regex` crate refuses for want of backtracking.
fn needs_backtracking(e: &regex::Error) -> bool {
    let s = e.to_string();
    s.contains("look-around") || s.contains("backreferences")
}

/// A `regex` error on one line (the crate draws a caret under the pattern).
pub fn error_line(e: &regex::Error) -> String {
    let s = e.to_string();
    s.lines().rev().find_map(|l| l.strip_prefix("error: ")).unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backtracking_patterns_run_on_fancy() {
        let re = |p: &str| Re::new(p, true, 1 << 20).unwrap();
        assert!(!re("lyon|paris").fancy() && re("Lyon").is_match("à lyon"));
        let look = re(r"\d+(?= €)");
        assert!(look.fancy() && look.is_match("12 €") && !look.is_match("12 $"));
        let back = re(r"(\w)\1");
        assert!(back.is_match("Ann") && !back.is_match("Ana"));
        let mut spans = Vec::new();
        re(r"(?<=x)a").for_each_match("xa ya xa", |a, b| spans.push((a, b)));
        assert_eq!(spans, [(1, 2), (7, 8)]);
        // Catastrophic backtracking stops at the bound: no match, no hang.
        let slow = re(r"^(a+)+\1$");
        assert!(!slow.is_match(&format!("{}b", "a".repeat(40))));
        assert!(Re::new("(", false, 1 << 20).is_err());
        assert_eq!(relax(r"(?<=@)gmail\.(?!fr)").as_deref(), Some(r"(?:)gmail\.(?:)"));
        assert_eq!(relax(r"(\w)\1[\1(?=]x\k<n>(?<n>a)").as_deref(), Some(r"(\w)(?s:.*?)[\1(?=]x(?s:.*?)(?<n>a)"));
        assert_eq!(relax(r"(?=a(b)[)])c").as_deref(), Some("(?:)c"));
        assert!(relax("(?=a").is_none());
    }
}
