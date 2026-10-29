//! Renders the real app off screen into a PNG — no window, nothing on the desktop:
//!
//!     cargo run --release -p csvfab --example shot -- out.png file.csv [spec] [hover x,y]
//!
//! `spec` is a `CSVFAB_SHOT` list (`menu`, `col:3`, `cmd:Insights`, `search:lyon`,
//! `scroll:12345`…, see `shot.rs` and the tools). The app runs on simulated frames
//! until its background jobs are done, then one frame is rendered through wgpu.
//! Size and scale: `SHOT_SIZE=1400x860`, `SHOT_PPP=2`.

use eframe::egui;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(out), Some(file)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: shot <out.png> <file> [spec] [hover x,y]");
        std::process::exit(2);
    };
    let spec = args.get(3).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "none".into());
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("CSVFAB_SHOT", &spec) };
    let size = std::env::var("SHOT_SIZE").ok().and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((1400.0f32, 860.0f32));
    let ppp: f32 = std::env::var("SHOT_PPP").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    // Several files: joined by "::", each in a tab (the last one shown).
    let paths: Vec<std::path::PathBuf> = file.split("::").map(std::path::PathBuf::from).collect();
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(size.0, size.1))
        .with_pixels_per_point(ppp)
        .with_accessibility_check(false)
        .build_eframe(move |cc| csvfab::app::App::new(cc, paths));
    if let Some((x, y)) = args.get(4).and_then(|s| s.split_once(',')).and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?))) {
        h.hover_at(egui::pos2(x, y));
        // SHOT_PRESS=1: the primary button pressed there, and held (after the file is shown).
        if std::env::var_os("SHOT_PRESS").is_some() {
            for _ in 0..90 { h.step(); std::thread::sleep(Duration::from_millis(16)); }
            h.event(egui::Event::PointerButton { pos: egui::pos2(x, y), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
            // SHOT_DRAG=X,Y: then the pointer moved there, the button still held.
            if let Some((dx, dy)) = std::env::var("SHOT_DRAG").ok().and_then(|s| s.split_once(',').and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))) {
                for _ in 0..10 { h.step(); std::thread::sleep(Duration::from_millis(16)); }
                h.hover_at(egui::pos2(dx, dy));
            }
        }
    }
    // Frames until the file is read, the view and charts computed and the animations over.
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(std::env::var("SHOT_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(2500)) {
        h.step();
        std::thread::sleep(Duration::from_millis(16));
    }
    match h.render() {
        Ok(img) => img.save(out).expect("write the PNG"),
        Err(e) => { eprintln!("render failed: {e}"); std::process::exit(1); }
    }
}
