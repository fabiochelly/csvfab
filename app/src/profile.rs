//! `CSVFAB_PROFILE=1`: when each step of a start happened, from the process start,
//! on stderr — the numbers the start-up work is judged by.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ON: OnceLock<bool> = OnceLock::new();

pub fn start() {
    START.get_or_init(Instant::now);
    ON.get_or_init(|| std::env::var_os("CSVFAB_PROFILE").is_some());
}

pub fn on() -> bool {
    *ON.get().unwrap_or(&false)
}

/// Prints `step` with the time since start, once per distinct step.
pub fn mark(step: &str) {
    if !on() { return; }
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
    if seen.iter().any(|s| s == step) { return; }
    seen.push(step.to_string());
    let ms = START.get().map_or(0.0, |s| s.elapsed().as_secs_f64() * 1e3);
    eprintln!("[{ms:8.1} ms] {step}");
}
