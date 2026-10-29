//! Screenshot hooks for checking the look without a pointer: `CSVFAB_SHOT=menu`
//! (main menu open), `col:3` (column 3's panel), `cell` (a cell's menu),
//! `scroll:123456` (scrolled to that pixel), several joined by commas. Each
//! fires once, after the file is shown.

use std::sync::{Mutex, OnceLock};

#[cfg_attr(test, allow(dead_code))]
fn pending() -> &'static Mutex<Vec<String>> {
    static P: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(std::env::var("CSVFAB_SHOT").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default()))
}

/// The argument of hook `kind` the first time it is asked for ("" when it has none).
pub fn take(kind: &str) -> Option<String> {
    #[cfg(test)]
    return MINE.with(|m| pick(&mut m.borrow_mut(), kind));
    #[cfg(not(test))]
    pick(&mut *pending().lock().ok()?, kind)
}

fn pick(p: &mut Vec<String>, kind: &str) -> Option<String> {
    let i = p.iter().position(|s| s == kind || s.starts_with(&format!("{kind}:")))?;
    let s = p.remove(i);
    Some(s.split_once(':').map_or(String::new(), |x| x.1.to_string()))
}

#[cfg(test)]
thread_local! {
    /// In tests, each test thread's own hooks: the apps of the tests running beside it
    /// (the frames of another harness) never take them.
    static MINE: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Asks for hook `kind` from a test, as `CSVFAB_SHOT` would.
#[cfg(test)]
pub fn push(kind: &str) {
    MINE.with(|m| m.borrow_mut().push(kind.to_string()));
}
