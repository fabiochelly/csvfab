//! Work off the UI thread: a closure on its own thread, its result picked up
//! by polling, progress and cancellation shared through atomics. The window
//! is repainted while the job runs and when it ends, so nobody waits.

use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Job<T> {
    rx: Receiver<T>,
    cancel: Arc<AtomicBool>,
    /// Units of work done, as the job counts them (bytes, rows…).
    pub progress: Arc<AtomicUsize>,
    pub started: Instant,
}

/// What a job gives back when it panicked: its way of saying it found nothing or failed
/// (None, an error…), so whoever waits carries on as after a failure — never a progress bar
/// turning forever. The panic itself is told by a toast (`take_failures`).
pub trait Failed {
    fn failed(msg: &str) -> Self;
}

impl<X> Failed for Option<X> {
    fn failed(_: &str) -> Self { None }
}
impl<X> Failed for Vec<X> {
    fn failed(_: &str) -> Self { Vec::new() }
}
impl Failed for () {
    fn failed(_: &str) -> Self {}
}
impl Failed for bool {
    fn failed(_: &str) -> Self { false }
}
impl<X> Failed for Result<X, String> {
    fn failed(msg: &str) -> Self { Err(msg.to_string()) }
}
impl<X> Failed for std::io::Result<X> {
    fn failed(msg: &str) -> Self { Err(std::io::Error::other(msg.to_string())) }
}
impl<X> Failed for Result<X, (bool, String)> {
    fn failed(msg: &str) -> Self { Err((false, msg.to_string())) }
}
impl<A, B> Failed for (Vec<A>, Vec<B>) {
    fn failed(_: &str) -> Self { (Vec::new(), Vec::new()) }
}
impl Failed for csvfab_engine::diagnose::Diagnosis {
    fn failed(_: &str) -> Self { Default::default() }
}
impl<X> Failed for Result<X, csvfab_engine::pivot::PivotError> {
    fn failed(_: &str) -> Self { Err(csvfab_engine::pivot::PivotError::Cancelled) }
}
impl Failed for csvfab_engine::split::Split {
    fn failed(msg: &str) -> Self { csvfab_engine::split::Split { folder: None, written: 0, error: Some(std::io::Error::other(msg.to_string())) } }
}

/// The panics caught in jobs since last asked, for the app to tell.
static FAILURES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn take_failures() -> Vec<String> {
    FAILURES.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

/// Runs `f`, a panic in it caught: Err(its message), also kept for the app to tell.
pub fn guarded<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| {
        let msg = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown error".into());
        if let Ok(mut v) = FAILURES.lock() { v.push(msg.clone()); }
        msg
    })
}

impl<T: Send + Failed + 'static> Job<T> {
    pub fn spawn(ctx: &egui::Context, f: impl FnOnce(&AtomicBool, &AtomicUsize) -> T + Send + 'static) -> Job<T> {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicUsize::new(0));
        let (c, p, ctx) = (cancel.clone(), progress.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = guarded(|| f(&c, &p)).unwrap_or_else(|m| T::failed(&format!("internal error: {m}")));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        Job { rx, cancel, progress, started: Instant::now() }
    }
}

impl<T> Job<T> {
    /// The result, once.
    pub fn poll(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

impl<T> Drop for Job<T> {
    /// A job nobody waits for anymore stops at its next check.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job that panics ends as a failure for whoever waits (never a wait forever) and is
    /// kept to be told.
    #[test]
    fn panic_ends_the_job() {
        let ctx = egui::Context::default();
        let mut j: Job<Result<u32, String>> = Job::spawn(&ctx, |_, _| -> Result<u32, String> { panic!("boom in a job") });
        let t0 = Instant::now();
        let r = loop {
            if let Some(r) = j.poll() { break r; }
            assert!(t0.elapsed() < Duration::from_secs(5), "the job never ended");
            std::thread::sleep(Duration::from_millis(2));
        };
        // (Its message also goes to `take_failures`, which parallel App tests drain: not checked here.)
        assert!(r.is_err_and(|e| e.contains("boom in a job")));
        let mut k: Job<Option<u8>> = Job::spawn(&ctx, |_, _| -> Option<u8> { panic!("again") });
        while k.poll().is_none() && t0.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(2)); }
        assert!(t0.elapsed() < Duration::from_secs(5));
    }
}
