//! Times the grid off screen — no window: runs the real app on simulated frames with a
//! `CSVFAB_SHOT` spec (usually `autoscroll:PX`), then reports, over the measured frames,
//! the median and worst whole-frame CPU time (egui run, all of the app), the tessellation
//! time of what it drew, and the number of shapes / vertices.
//!
//!     cargo run --release -p csvfab --example gridbench -- file.csv "autoscroll:9000" [frames]
//!
//! `SHOT_SIZE` / `SHOT_PPP` as `shot`. Warm-up: until the file and its jobs are done.

use eframe::egui;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(file) = args.get(1) else {
        eprintln!("usage: gridbench <file> [spec] [frames]");
        std::process::exit(2);
    };
    let spec = args.get(2).cloned().unwrap_or_else(|| "autoscroll:9000".into());
    let frames: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(600);
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("CSVFAB_SHOT", &spec) };
    let size = std::env::var("SHOT_SIZE").ok().and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((1400.0f32, 860.0f32));
    let ppp: f32 = std::env::var("SHOT_PPP").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let path = std::path::PathBuf::from(file);
    let mut h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(size.0, size.1))
        .with_pixels_per_point(ppp)
        .with_accessibility_check(false)
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| csvfab::app::App::new(cc, vec![path]));
    h.hover_at(egui::pos2(size.0 * 0.5, size.1 * 0.55));
    // Real time, as a window's frames get it (the easings and the wheel's speed-up read it).
    let clock = Instant::now();
    let tick = |h: &mut egui_kittest::Harness<'_, csvfab::app::App>| { h.input_mut().time = Some(clock.elapsed().as_secs_f64()); h.step(); };
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(std::env::var("SHOT_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(1500)) {
        tick(&mut h);
        std::thread::sleep(Duration::from_millis(5));
    }
    // WHEEL=1: one notch, then a flick of 12 notches 25 ms apart: the scroll frame by frame.
    if std::env::var_os("WHEEL").is_some() {
        let scroll = |h: &egui_kittest::Harness<'_, csvfab::app::App>| { let a = h.state(); a.tabs[a.active].grid.scroll };
        let notch = |h: &egui_kittest::Harness<'_, csvfab::app::App>| h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, -1.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: egui::Modifiers::NONE });
        let s0 = scroll(&h);
        notch(&h);
        let mut line = String::new();
        for _ in 0..12 { std::thread::sleep(Duration::from_millis(16)); tick(&mut h); line += &format!(" {:.0}", scroll(&h) - s0); }
        println!("one notch, px moved after each frame (16 ms apart):{line}");
        let s1 = scroll(&h);
        let (mut worst, t) = (0.0f64, Instant::now());
        for k in 0..40 {
            if k % 2 == 0 && k < 24 { notch(&h); }
            std::thread::sleep(Duration::from_millis(12));
            let a = Instant::now();
            tick(&mut h);
            worst = worst.max(a.elapsed().as_secs_f64() * 1e3);
        }
        println!("flick of 12 notches: {:.0} px ({:.0} rows) in {:.0} ms, worst frame {worst:.2} ms", scroll(&h) - s1, (scroll(&h) - s1) / 35.0, t.elapsed().as_secs_f64() * 1e3);
        return;
    }
    let (mut run, mut tess, mut shapes, mut verts) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..frames {
        let a = Instant::now();
        tick(&mut h);
        run.push(a.elapsed().as_secs_f64() * 1e3);
        let out = h.output().shapes.clone();
        shapes.push(out.len());
        let b = Instant::now();
        let prim = h.ctx.tessellate(out, ppp);
        tess.push(b.elapsed().as_secs_f64() * 1e3);
        verts.push(prim.iter().map(|p| match &p.primitive { egui::epaint::Primitive::Mesh(m) => m.vertices.len(), _ => 0 }).sum::<usize>());
    }
    let med = |v: &mut Vec<f64>| { v.sort_by(f64::total_cmp); (v[v.len() / 2], v[v.len() * 99 / 100], v[v.len() - 1]) };
    let (rm, r99, rx) = med(&mut run);
    let (tm, t99, tx) = med(&mut tess);
    shapes.sort();
    verts.sort();
    println!("frame (egui run, whole app): median {rm:.3} ms, p99 {r99:.3}, max {rx:.3}");
    println!("tessellate:                  median {tm:.3} ms, p99 {t99:.3}, max {tx:.3}");
    // Idle: after the frames, with nothing moving, does the app still ask for frames?
    if std::env::var_os("IDLE").is_some() {
        h.remove_cursor();
        for _ in 0..200 { tick(&mut h); std::thread::sleep(Duration::from_millis(5)); }
        let d = h.output().viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay);
        println!("idle: repaint asked in {d:?} (MAX = none)");
    }
    let smaps = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    let anon = smaps.lines().find(|l| l.starts_with("Anonymous:")).unwrap_or("").split_whitespace().nth(1).unwrap_or("?").to_string();
    println!("anonymous memory: {anon} kB");
    println!("shapes: median {}  vertices: median {}", shapes[shapes.len() / 2], verts[verts.len() / 2]);
}
