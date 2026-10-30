//! The demo film: the real app on a big file, off screen, in real time. No window.
//!
//!     film <file.csv> --out DIR [--size 1280x720] [--ppp 1.5] [--theme ID] [--whole-word]
//!     film <file.csv> --still out.png [--size 1067x640] [--ppp 1.5]
//!
//! `csvfab::app::App` runs in an egui_kittest harness on real frames: egui's clock is
//! the wall clock, each frame is rendered through wgpu and kept with the moment it
//! shows. DIR/app.mkv (lossless, 30 fps) is built from those moments: output frame n
//! is the last frame stamped at or before n / 30 s, so a frame slower than 30 fps holds
//! longer and nothing is compressed or stretched. DIR/timeline.json gives every frame's
//! stamp and every operation's start, end and duration.
//!
//! A timed operation runs as in `bench_ui`: the clock starts at the event (the click's
//! release, the letter typed), frames run as an event loop would run them, and it stops
//! when the result is drawn and tessellated. No frame is rendered meanwhile, so rendering
//! and capturing cost the operation nothing: the frame before it stays on screen for
//! exactly the time measured, and the result, rendered once the clock has stopped, is
//! stamped with the moment the clock stopped.
//!
//! The pointer and the captions are drawn over the app in a layer of their own. Captions
//! show the times measured a moment before, in this very run. `--whole-word` enters
//! "lyon" as one text instead of letter by letter, to compare with `bench_ui`'s filter.

use csvfab::app::App;
use csvfab_engine::view::SortKey;
use eframe::egui::{self, Color32, FontFamily, FontId, Pos2, Rect, Stroke, pos2, vec2};
use egui::text::{LayoutJob, TextFormat};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const FPS: f64 = 30.0;

/// What is drawn over the app, set by the script.
#[derive(Default)]
struct Overlay {
    pointer: Option<Pos2>,
    down: bool,
    ripple: Option<(Pos2, Instant)>,
    caption: Option<Caption>,
    leaving: Option<(Caption, Instant)>,
}

#[derive(Clone)]
struct Caption {
    /// The line's pieces; true: a measured figure.
    head: Vec<(String, bool)>,
    sub: String,
    born: Instant,
}

/// The app, and the overlay in the topmost layer.
struct FilmApp {
    app: App,
    overlay: Arc<Mutex<Overlay>>,
}

impl eframe::App for FilmApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, ui, frame);
        paint_overlay(ui.ctx(), &self.overlay.lock().unwrap());
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        eframe::App::clear_color(&self.app, visuals)
    }
}

fn paint_caption(p: &egui::Painter, screen: Rect, c: &Caption, a: f32) {
    let col = |c: Color32| c.gamma_multiply(a);
    let semibold = FontFamily::Name(csvfab::fonts::UI_SEMIBOLD.into());
    let mut job = LayoutJob::default();
    for (s, figure) in &c.head {
        job.append(s, 0.0, TextFormat::simple(FontId::new(23.0, semibold.clone()), col(if *figure { Color32::from_rgb(0xfb, 0xbf, 0x24) } else { Color32::WHITE })));
    }
    let head = p.layout_job(job);
    let sub = p.layout_job(LayoutJob::simple_singleline(c.sub.clone(), FontId::proportional(13.5), col(Color32::from_rgb(0xa8, 0xb3, 0xc2))));
    let w = head.size().x.max(sub.size().x) + 44.0;
    let h = head.size().y + if c.sub.is_empty() { 0.0 } else { sub.size().y + 3.0 } + 22.0;
    let r = Rect::from_center_size(pos2(screen.center().x, screen.max.y - 66.0 - h / 2.0), vec2(w.max(300.0), h));
    p.rect_filled(r.translate(vec2(0.0, 4.0)).expand(3.0), 13.0, Color32::from_black_alpha(90).gamma_multiply(a));
    p.rect(r, 11.0, col(Color32::from_rgb(0x0d, 0x11, 0x17)), Stroke::new(1.0, col(Color32::from_rgb(0x3a, 0x42, 0x4d))), egui::StrokeKind::Inside);
    p.galley(pos2(r.center().x - head.size().x / 2.0, r.min.y + 10.0), head.clone(), Color32::WHITE);
    p.galley(pos2(r.center().x - sub.size().x / 2.0, r.min.y + 13.0 + head.size().y), sub, Color32::WHITE);
}

fn paint_overlay(ctx: &egui::Context, o: &Overlay) {
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::unique("film-overlay")));
    let screen = ctx.content_rect();
    if let Some(c) = &o.caption {
        paint_caption(&p, screen, c, (c.born.elapsed().as_secs_f32() / 0.18).min(1.0));
    }
    if let Some((c, t)) = &o.leaving {
        let a = 1.0 - t.elapsed().as_secs_f32() / 0.18;
        if a > 0.0 { paint_caption(&p, screen, c, a); }
    }
    if let Some((at, t)) = o.ripple {
        let k = t.elapsed().as_secs_f32() / 0.4;
        if k < 1.0 { p.circle_stroke(at, 7.0 + k * 20.0, Stroke::new(2.5, Color32::from_rgb(0xfb, 0xbf, 0x24).gamma_multiply(1.0 - k))); }
    }
    if let Some(at) = o.pointer {
        // An arrow: a head and a tail, each convex, then its outline.
        let s = if o.down { 0.86 } else { 1.0 };
        let q = |x: f32, y: f32| at + vec2(x, y) * s;
        let outline = [q(0.0, 0.0), q(0.0, 17.0), q(4.4, 13.2), q(7.2, 19.8), q(10.2, 18.5), q(7.4, 12.0), q(12.8, 12.0)];
        let fill = Color32::WHITE;
        p.add(egui::Shape::convex_polygon(vec![q(0.0, 0.0), q(0.0, 17.0), q(4.4, 13.2), q(7.4, 12.0), q(12.8, 12.0)], fill, Stroke::NONE));
        p.add(egui::Shape::convex_polygon(vec![q(4.4, 13.2), q(7.2, 19.8), q(10.2, 18.5), q(7.4, 12.0)], fill, Stroke::NONE));
        p.add(egui::Shape::closed_line(outline.to_vec(), Stroke::new(1.1, Color32::BLACK)));
    }
}

type H = egui_kittest::Harness<'static, FilmApp>;

/// When egui (or a job's thread) last asked for a frame: the event loop's wake-up.
#[derive(Default)]
struct Wake {
    at: Mutex<Option<Instant>>,
    cv: Condvar,
}

struct Op {
    name: String,
    t0: f64,
    t1: f64,
    ms: f64,
    note: serde_json::Value,
}

struct Film {
    h: H,
    wake: Arc<Wake>,
    ctx: egui::Context,
    ppp: f32,
    overlay: Arc<Mutex<Overlay>>,
    start: Instant,
    last: Instant,
    at: Pos2,
    frames: Option<SyncSender<(f64, Vec<u8>)>>,
    /// (the moment shown, how long the render took), in seconds.
    stamps: Vec<(f64, f64)>,
    ops: Vec<Op>,
}

/// Every text drawn in the last frame, with its rectangle.
fn texts(h: &H) -> Vec<(String, Rect)> {
    fn walk(s: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match s {
            egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.output().shapes {
        walk(&c.shape, &mut out);
    }
    out
}

fn tab(h: &mut H) -> &mut csvfab::tab::Tab {
    let a = &mut h.state_mut().app;
    let i = a.active;
    &mut a.tabs[i]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// A time as a caption says it.
fn say_ms(d: Duration) -> String {
    let v = ms(d);
    if v < 1000.0 { format!("{v:.0} ms") } else { format!("{:.2} s", v / 1e3) }
}

/// 5000000 → 5,000,000 (captions are in English; the app's status bar keeps its own format).
fn commas(n: usize) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); }
        o.push(c);
    }
    o
}

fn ease(p: f64) -> f64 {
    1.0 - (1.0 - p).powi(3)
}

impl Film {
    fn clock(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// One frame of the app, on the wall clock.
    fn step(&mut self) {
        *self.wake.at.lock().unwrap() = None;
        let now = self.clock();
        self.h.input_mut().time = Some(now);
        self.h.step();
    }

    /// The last frame rendered and handed to the film, showing the moment `t`.
    fn shot(&mut self, t: f64) {
        let r0 = Instant::now();
        let img = self.h.render().expect("render");
        self.stamps.push((t, r0.elapsed().as_secs_f64()));
        if let Some(tx) = &self.frames {
            let _ = tx.send((t, img.into_raw()));
        }
    }

    /// One filmed frame, paced at 30 a second (later if rendering takes longer).
    fn tick(&mut self) {
        let next = self.last + Duration::from_secs_f64(1.0 / FPS);
        let now = Instant::now();
        if next > now { std::thread::sleep(next - now); }
        self.last = Instant::now();
        let t = self.clock();
        self.step();
        self.shot(t);
    }

    /// Filmed frames for `secs`; `each` gets the progress (0 to 1) before each one.
    fn live(&mut self, secs: f64, mut each: impl FnMut(&mut Film, f64)) {
        let t0 = Instant::now();
        loop {
            let p = (t0.elapsed().as_secs_f64() / secs).min(1.0);
            each(self, p);
            self.tick();
            if p >= 1.0 { return; }
        }
    }

    fn hold(&mut self, secs: f64) {
        self.live(secs, |_, _| {});
    }

    fn say(&mut self, head: &[(&str, bool)], sub: &str) {
        let mut o = self.overlay.lock().unwrap();
        if let Some(c) = o.caption.take() { o.leaving = Some((c, Instant::now())); }
        o.caption = Some(Caption { head: head.iter().map(|(s, f)| (s.to_string(), *f)).collect(), sub: sub.to_string(), born: Instant::now() });
    }

    fn hush(&mut self) {
        let mut o = self.overlay.lock().unwrap();
        if let Some(c) = o.caption.take() { o.leaving = Some((c, Instant::now())); }
    }

    /// The pointer glides to `to`, as a hand would.
    fn point(&mut self, to: Pos2, secs: f64) {
        let from = self.at;
        self.live(secs, |f, p| {
            let at = from + (to - from) * ease(p) as f32;
            f.at = at;
            f.overlay.lock().unwrap().pointer = Some(at);
            f.h.event(egui::Event::PointerMoved(at));
        });
    }

    fn button(&self, pressed: bool) {
        self.h.event(egui::Event::PointerButton { pos: self.at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        self.overlay.lock().unwrap().down = pressed;
    }

    fn press(&mut self) {
        self.button(true);
        self.tick();
        self.overlay.lock().unwrap().ripple = Some((self.at, Instant::now()));
        self.hold(0.07);
    }

    /// Waits as an event loop does: until a frame is asked for, at most `max`.
    fn idle_wait(&self, max: Duration) {
        let deadline = Instant::now() + max;
        let mut g = self.wake.at.lock().unwrap();
        loop {
            let now = Instant::now();
            if g.is_some_and(|t| t <= now) || now >= deadline { return; }
            let until = g.map_or(deadline, |t| t.min(deadline));
            g = self.wake.cv.wait_timeout(g, until - now).unwrap().0;
        }
    }

    /// An operation: `start` then frames until `done`, as `bench_ui`'s `until`. See the file's head.
    fn timed(&mut self, name: &str, start: impl FnOnce(&mut Film), mut done: impl FnMut(&mut H) -> bool) -> Duration {
        let t0 = Instant::now();
        let film_t0 = self.clock();
        start(self);
        loop {
            self.step();
            if done(&mut self.h) {
                let shapes = self.h.output().shapes.clone();
                let _ = self.ctx.tessellate(shapes, self.ppp);
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(120), "{name}: not done");
            self.idle_wait(Duration::from_millis(250));
        }
        let d = t0.elapsed();
        let t1 = film_t0 + d.as_secs_f64();
        self.shot(t1);
        self.last = Instant::now();
        self.ops.push(Op { name: name.to_string(), t0: film_t0, t1, ms: ms(d), note: serde_json::Value::Null });
        eprintln!("{name}: {:.1} ms", ms(d));
        d
    }

    fn note(&mut self, v: serde_json::Value) {
        if let Some(o) = self.ops.last_mut() { o.note = v; }
    }

    /// Where a column's title is, its filter box below it.
    fn title(&self, name: &str) -> Rect {
        texts(&self.h).into_iter().filter(|(t, _)| t == name).map(|x| x.1).min_by(|a, b| a.min.y.total_cmp(&b.min.y)).unwrap_or_else(|| panic!("no title {name} on screen"))
    }

    fn filter_box(&self, name: &str) -> Rect {
        let t = self.title(name);
        texts(&self.h).into_iter().filter(|(s, r)| s == "Filter" && r.min.y > t.max.y).map(|x| x.1).min_by(|a, b| (a.min.x - t.min.x).abs().total_cmp(&(b.min.x - t.min.x).abs())).expect("no filter box")
    }

    fn col(&mut self, name: &str) -> usize {
        tab(&mut self.h).st().and_then(|s| s.headers().iter().position(|h| h == name)).unwrap_or_else(|| panic!("no column {name}"))
    }

    /// The columns slide until `name` is on screen, `left` px from the grid's left edge.
    fn reveal(&mut self, name: &str, left: f32) {
        let c = self.col(name);
        let t = tab(&mut self.h);
        let to = (t.grid.widths[..c].iter().sum::<f32>() - left).max(0.0);
        let from = t.grid.xoff;
        self.live(0.7, |f, p| tab(&mut f.h).grid.xoff = from + (to - from) * ease(p) as f32);
    }

    fn cell(&mut self, pos: usize, col: usize) -> String {
        let t = tab(&mut self.h);
        let id = t.shown_id(pos);
        t.st().map(|s| s.cell(id, col)).unwrap_or_default()
    }

    /// Clicks the title of `name`: sorted as `want` once the result is drawn.
    fn sort_click(&mut self, name: &str, op: &str, want: Vec<SortKey>) -> Duration {
        let p = self.title(name).center();
        self.point(p, 0.75);
        self.press();
        self.timed(op, |f| f.button(false), move |h| {
            let t = tab(h);
            t.sort == want && !t.busy()
        })
    }
}

fn filmed(f: &mut Film, file: PathBuf, whole_word: bool) {
    let bytes = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    let size = format!("{:.0} MB", bytes as f64 / 1e6);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    f.say(&[(&name, false)], &format!("{size} · nothing in this film is sped up"));
    f.point(pos2(640.0, 330.0), 1.0);
    f.hold(1.6);
    // Open: what a double click in the file manager, or a file dropped, calls.
    let ctx = f.ctx.clone();
    let mut first = None;
    let t0 = Instant::now();
    let open = f.timed("open", |f| f.h.state_mut().app.open(file, &ctx), |h| {
        let a = &h.state_mut().app;
        let t = a.tabs.get(a.active);
        if first.is_none() && t.is_some_and(|t| t.table.is_some()) { first = Some(t0.elapsed()); }
        t.is_some_and(|t| t.table.is_some() && !t.busy())
    });
    let first = first.unwrap_or(open);
    let rows = tab(&mut f.h).st().map_or(0, |s| s.len());
    f.note(serde_json::json!({"rows": rows, "first_rows_ms": ms(first)}));
    f.say(&[("Opened in ", false), (&say_ms(open), true)], &format!("all {} rows known · first rows on screen at {}", commas(rows), say_ms(first)));
    f.hold(3.8);

    f.say(&[("Scroll anywhere", false)], "rows are read only when they are shown");
    let body_end = (rows as f64) * 32.0;
    f.live(3.4, |f, p| {
        let g = &mut tab(&mut f.h).grid;
        // Faster and faster: from a page at a time to most of the file.
        g.scroll = (p * p * p * p) * body_end * 0.82;
    });
    tab(&mut f.h).grid.set_top(rows.saturating_sub(1));
    f.tick();
    f.say(&[(&format!("Row {}", commas(rows)), false)], "the last line of the file, at once");
    f.hold(2.4);
    tab(&mut f.h).grid.set_top(0);
    f.hush();
    f.hold(0.5);

    f.say(&[("Sort: click a title", false)], &format!("{} rows by total_spent", commas(rows)));
    f.reveal("total_spent", 300.0);
    let c = f.col("total_spent");
    let asc = f.sort_click("total_spent", "sort_num_asc", vec![SortKey { col: c, desc: false }]);
    let v = f.cell(0, c);
    f.note(serde_json::json!({"first": v}));
    f.say(&[("Sorted in ", false), (&say_ms(asc), true)], "smallest first · first sort of this column since the file opened");
    f.hold(2.8);
    let desc = f.sort_click("total_spent", "sort_num_desc", vec![SortKey { col: c, desc: true }]);
    let v = f.cell(0, c);
    f.note(serde_json::json!({"first": v}));
    f.say(&[("Largest first, ", false), (&say_ms(desc), true)], &format!("second click · the top amount: {v}"));
    f.hold(3.0);

    f.say(&[("Now by name", false)], "one click on last_name");
    f.reveal("id", 0.0);
    let c = f.col("last_name");
    let text = f.sort_click("last_name", "sort_text", vec![SortKey { col: c, desc: false }]);
    let v = f.cell(0, c);
    f.note(serde_json::json!({"first": v}));
    f.say(&[("Sorted by name in ", false), (&say_ms(text), true)], &format!("{} rows, A → Z · first sort of this column", commas(rows)));
    f.hold(3.2);

    f.say(&[("Filter as you type", false)], "city contains “lyon”");
    f.reveal("city", 300.0);
    let b = f.filter_box("city").center();
    f.point(b, 0.75);
    f.press();
    f.button(false);
    f.tick();
    // The hand leaves the box: a filter opens a band that moves the grid down, and the
    // charts answer a pointer resting on them.
    let rest = b + vec2(150.0, 210.0);
    f.point(rest, 0.45);
    let c = f.col("city");
    let word: Vec<String> = if whole_word { vec!["lyon".to_string()] } else { "lyon".chars().map(|c| c.to_string()).collect() };
    let mut typed = String::new();
    let mut each = Vec::new();
    for (i, s) in word.iter().enumerate() {
        typed.push_str(s);
        let want = typed.clone();
        let d = f.timed(&format!("filter_{want}"), |f| f.h.event(egui::Event::Text(s.clone())), move |h| {
            let t = tab(h);
            t.filter.cols.iter().any(|(k, v)| *k == c && *v == want) && !t.busy()
        });
        let n = tab(&mut f.h).shown();
        f.note(serde_json::json!({"n": n}));
        each.push(format!("{typed} {}", say_ms(d)));
        if i + 1 < word.len() { f.hold(0.17); }
    }
    let n = tab(&mut f.h).shown();
    f.say(&[(&commas(n), true), (" rows out of ", false), (&commas(rows), false)], &format!("as you type: {}", each.join(" · ")));
    f.hold(4.4);
    f.hush();
    f.hold(0.4);
}

/// A still of the app on the file: sorted, filtered, its charts drawn.
fn still(f: &mut Film, file: PathBuf, out: &str) {
    let ctx = f.ctx.clone();
    f.h.state_mut().app.open(file, &ctx);
    let settled = |f: &mut Film, secs: f64| {
        let t0 = Instant::now();
        while t0.elapsed().as_secs_f64() < secs { f.step(); std::thread::sleep(Duration::from_millis(16)); }
    };
    settled(f, 1.5);
    let (city, spent) = (f.col("city"), f.col("total_spent"));
    let t = tab(&mut f.h);
    t.col_filters[city] = "lyon".into();
    t.sort = vec![SortKey { col: spent, desc: true }];
    t.refresh(&ctx);
    settled(f, 2.5);
    let x: f32 = tab(&mut f.h).grid.widths[..city].iter().sum();
    tab(&mut f.h).grid.xoff = x;
    settled(f, 1.0);
    f.h.render().expect("render").save(out).expect("write the still");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(file) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: film <file.csv> --out DIR [--size 1280x720] [--ppp 1.5] [--theme ID] [--whole-word] | --still out.png");
        std::process::exit(2);
    };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let size = flag("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((1280.0f32, 720.0f32));
    let ppp: f32 = flag("--ppp").and_then(|s| s.parse().ok()).unwrap_or(1.5);
    // The hooks of `shot.rs`: nothing asked for but a theme; also keeps the app from
    // listening for a second launch (no single-instance socket) and from writing settings.
    let hooks = flag("--theme").map_or("none".to_string(), |t| format!("theme:{t}"));
    // SAFETY: before any other thread exists.
    unsafe { std::env::set_var("CSVFAB_SHOT", hooks) };
    let path = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));

    let overlay = Arc::new(Mutex::new(Overlay::default()));
    let o2 = overlay.clone();
    let h = egui_kittest::Harness::builder()
        .with_size(egui::vec2(size.0, size.1))
        .with_pixels_per_point(ppp)
        .with_step_dt(1.0 / FPS as f32)
        .with_render_cursor(false)
        .with_accessibility_check(false)
        .build_eframe(move |cc| FilmApp { app: App::new(cc, Vec::new()), overlay: o2 });
    let wake = Arc::new(Wake::default());
    let w = wake.clone();
    h.ctx.set_request_repaint_callback(move |info| {
        let t = Instant::now() + info.delay;
        let mut g = w.at.lock().unwrap();
        if g.is_none_or(|x| t < x) { *g = Some(t); }
        w.cv.notify_all();
    });
    let ctx = h.ctx.clone();
    let now = Instant::now();
    let mut f = Film { h, wake, ctx, ppp, overlay, start: now, last: now, at: pos2(900.0, 560.0), frames: None, stamps: Vec::new(), ops: Vec::new() };

    if let Some(out) = flag("--still") {
        still(&mut f, path, &out);
        return;
    }
    let Some(dir) = flag("--out").map(PathBuf::from) else {
        eprintln!("film: --out DIR is needed");
        std::process::exit(2);
    };
    std::fs::create_dir_all(&dir).expect("create the output folder");

    // The first frames and the GPU's start-up, before the film's clock.
    for _ in 0..3 { f.step(); }
    f.h.render().expect("render");
    let (w, h) = ((size.0 * ppp).round() as u32, (size.1 * ppp).round() as u32);

    // Frames go through a short queue to a thread that feeds ffmpeg, at nice 19: output
    // frame n is the last frame stamped at or before n / 30 s.
    let mut enc = Command::new("nice")
        .args(["-n", "19", "ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-video_size", &format!("{w}x{h}"), "-framerate", "30", "-i", "pipe:0", "-an",
            "-threads", "4", "-c:v", "libx264rgb", "-preset", "ultrafast", "-qp", "0", "-pix_fmt", "rgb24"])
        .arg(dir.join("app.mkv"))
        .stdin(Stdio::piped())
        .spawn()
        .expect("run ffmpeg");
    let mut stdin = enc.stdin.take().unwrap();
    let (tx, rx) = sync_channel::<(f64, Vec<u8>)>(12);
    let writer = std::thread::spawn(move || {
        let mut prev: Option<Vec<u8>> = None;
        let mut n = 0u64;
        for (t, img) in rx {
            if let Some(p) = &prev {
                while (n as f64) / FPS < t {
                    stdin.write_all(p).expect("write to ffmpeg");
                    n += 1;
                }
            }
            prev = Some(img);
        }
        if let Some(p) = &prev { stdin.write_all(p).expect("write to ffmpeg"); n += 1; }
        n
    });
    f.frames = Some(tx);
    f.start = Instant::now();
    f.last = f.start;
    f.tick();

    filmed(&mut f, path, args.iter().any(|a| a == "--whole-word"));

    let end = f.clock();
    f.shot(end);
    drop(f.frames.take());
    let written = writer.join().expect("the writer");
    enc.wait().expect("ffmpeg");

    let mut renders: Vec<f64> = f.stamps.iter().map(|s| s.1 * 1e3).collect();
    renders.sort_by(f64::total_cmp);
    let gaps: Vec<f64> = f.stamps.windows(2).map(|w| (w[1].0 - w[0].0) * 1e3).collect();
    let ops: Vec<serde_json::Value> = f.ops.iter().map(|o| serde_json::json!({"op": o.name, "t0": o.t0, "t1": o.t1, "ms": o.ms, "note": o.note})).collect();
    let tl = serde_json::json!({
        "seconds": end, "frames_rendered": f.stamps.len(), "frames_written": written, "fps": FPS,
        "render_ms_median": renders[renders.len() / 2], "render_ms_max": renders[renders.len() - 1],
        "frame_gap_ms_max": gaps.iter().copied().fold(0.0, f64::max),
        "frames_later_than_40ms": gaps.iter().filter(|g| **g > 40.0).count(),
        "ops": ops, "stamps": f.stamps.iter().map(|s| (s.0 * 1e4).round() / 1e4).collect::<Vec<f64>>(),
    });
    std::fs::write(dir.join("timeline.json"), serde_json::to_string_pretty(&tl).unwrap()).expect("write the timeline");
    eprintln!("{end:.1} s, {} frames rendered, {written} written, render median {:.1} ms", f.stamps.len(), renders[renders.len() / 2]);
}
