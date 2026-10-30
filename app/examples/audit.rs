//! Responsiveness audit, off screen (no window): the real app on frames driven like an
//! event loop drives them (a frame when egui or a job asks for one), every frame timed.
//! For each scenario: the time of the action itself (on the UI thread), the worst and
//! median frame until everything settled, the number of frames, the time until the last
//! frame, and how many frames are drawn while idle afterwards (must be 0).
//!
//!     audit <file.csv> [scenario,…]     (default: all)
//!
//! Frame costs are the UI thread's time on the CPU (`/proc/thread-self/schedstat`), with
//! the wall time beside: other processes cannot inflate them. `CSVFAB_SLOW=<ms>` makes the
//! app print where the time of each slower frame went.
//!
//! Scenarios: load, idle, menu, savemenu, palette, colpanel, cellmenu, totals, find,
//! type_filter, type_colfilter, sort, scroll_sort, scroll_insights, insert_col, delete_col, edit_cell,
//! undo, delete_rows, copy_all, type_grid, probe, registry, cmd:<name> / cmds (every command).

use csvfab::app::App;
use csvfab_engine::view::SortKey;
use eframe::egui::{self, Pos2, Rect};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

type H = egui_kittest::Harness<'static, App>;

#[derive(Default)]
struct Wake {
    at: Mutex<Option<Instant>>,
    /// The delay asked with the earliest wake-up.
    delay: Mutex<Duration>,
    cv: Condvar,
}

struct A {
    h: H,
    wake: Arc<Wake>,
    ctx: egui::Context,
    clock: Instant,
    file: PathBuf,
    /// Frame times of the current scenario (ms): wall clock, and on the CPU.
    frames: Vec<f64>,
    cpu: Vec<f64>,
    /// The process's CPU time when the scenario started.
    cpu0: f64,
    /// When the last frame started: frames are paced at 120 Hz as a vsync'd window's are.
    last: Instant,
}

/// This thread's time on a CPU so far (ns): frame costs unaffected by other processes.
fn cpu_ns() -> u64 {
    std::fs::read_to_string("/proc/thread-self/schedstat").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(0)
}

/// The process's CPU time so far, ms (every thread: the jobs' work).
fn proc_cpu_ms() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let f: Vec<&str> = s.rsplit_once(')').map_or("", |x| x.1).split_whitespace().collect();
    let t = |i: usize| f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    (t(11) + t(12)) * 10.0
}

fn texts(h: &H) -> Vec<(String, Rect)> {
    fn walk(s: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match s {
            egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &h.output().shapes { walk(&c.shape, &mut out); }
    out
}

impl A {
    fn new(file: PathBuf) -> A {
        let f = file.clone();
        let h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1920.0, 1080.0))
            .with_pixels_per_point(1.5)
            .with_accessibility_check(false)
            .build_eframe(move |cc| App::new(cc, vec![f]));
        let wake = Arc::new(Wake::default());
        let w = wake.clone();
        h.ctx.set_request_repaint_callback(move |info| {
            let t = Instant::now() + info.delay;
            let mut g = w.at.lock().unwrap();
            if g.is_none_or(|x| t < x) { *g = Some(t); *w.delay.lock().unwrap() = info.delay; }
            w.cv.notify_all();
        });
        let ctx = h.ctx.clone();
        A { h, wake, ctx, clock: Instant::now(), file, frames: Vec::new(), cpu: Vec::new(), cpu0: 0.0, last: Instant::now() }
    }

    fn app(&mut self) -> &mut App { self.h.state_mut() }
    fn tab(&mut self) -> &mut csvfab::tab::Tab { let a = self.h.state_mut(); let i = a.active; &mut a.tabs[i] }

    fn frame(&mut self) {
        let vsync = Duration::from_micros(8333);
        if self.last.elapsed() < vsync { std::thread::sleep(vsync - self.last.elapsed()); }
        self.last = Instant::now();
        *self.wake.at.lock().unwrap() = None;
        self.h.input_mut().time = Some(self.clock.elapsed().as_secs_f64());
        let (t, c) = (Instant::now(), cpu_ns());
        self.h.step();
        self.frames.push(t.elapsed().as_secs_f64() * 1e3);
        self.cpu.push((cpu_ns() - c) as f64 / 1e6);
    }

    /// Waits as an event loop does, at most `max`; true when a frame is due.
    fn wait(&self, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        let mut g = self.wake.at.lock().unwrap();
        loop {
            let now = Instant::now();
            if g.is_some_and(|t| t <= now) { return true; }
            if now >= deadline { return false; }
            let until = g.map_or(deadline, |t| t.min(deadline));
            g = self.wake.cv.wait_timeout(g, until - now).unwrap().0;
        }
    }

    /// Frames as asked until none is asked for `quiet`; returns the time of the last frame.
    fn settle(&mut self, quiet: Duration, max: Duration) -> Duration {
        let t0 = Instant::now();
        let mut last = Duration::ZERO;
        while t0.elapsed() < max {
            if !self.wait(quiet) { break; }
            // A frame asked for 400 ms or more after the last one: a periodic one (the text
            // cursor's blink), not work going on.
            if *self.wake.delay.lock().unwrap() >= Duration::from_millis(400) { break; }
            self.frame();
            last = t0.elapsed();
        }
        last
    }

    /// Frames asked for during `d` with nothing happening.
    fn idle_frames(&mut self, d: Duration) -> usize {
        let n0 = self.frames.len();
        let t0 = Instant::now();
        while t0.elapsed() < d {
            if self.wait(d.saturating_sub(t0.elapsed())) { self.frame(); }
        }
        let n = self.frames.len() - n0;
        self.frames.truncate(n0);
        self.cpu.truncate(n0);
        n
    }

    fn report(&mut self, name: &str, action_ms: f64, last: Duration, extra: &str) {
        let mut v = self.frames.clone();
        v.sort_by(f64::total_cmp);
        let (med, worst) = if v.is_empty() { (0.0, 0.0) } else { (v[v.len() / 2], v[v.len() - 1]) };
        let p95 = if v.is_empty() { 0.0 } else { v[v.len() * 95 / 100] };
        let mut c = self.cpu.clone();
        c.sort_by(f64::total_cmp);
        let (cmed, cworst) = if c.is_empty() { (0.0, 0.0) } else { (c[c.len() / 2], c[c.len() - 1]) };
        let focus = self.ctx.memory(|m| m.focused().is_some());
        let idle = self.idle_frames(Duration::from_millis(1500));
        let work = proc_cpu_ms() - self.cpu0;
        let blink = if focus { " (text focus: blink)" } else { "" };
        println!("{name:<40} action {action_ms:7.2} ms | worst frame cpu {cworst:7.2} ms (wall {worst:7.2}, p95 {p95:6.2}) | median cpu {cmed:5.2} (wall {med:5.2}) | frames {:5} | settled {:8.1} ms | process cpu {work:7.0} ms | idle frames {idle}{blink}{extra}", v.len(), last.as_secs_f64() * 1e3);
        self.frames.clear();
        self.cpu.clear();
        self.cpu.clear();
    }

    fn pointer(&self, p: Pos2, pressed: bool, button: egui::PointerButton) {
        self.h.event(egui::Event::PointerButton { pos: p, button, pressed, modifiers: Default::default() });
    }

    fn click(&mut self, p: Pos2, button: egui::PointerButton) {
        self.h.event(egui::Event::PointerMoved(p));
        self.frame();
        self.pointer(p, true, button);
        self.frame();
        self.pointer(p, false, button);
    }

    fn escape(&mut self) {
        self.h.key_press(egui::Key::Escape);
        self.frame();
        self.h.remove_cursor();
        self.frame();
    }

    fn loaded(&mut self) -> bool {
        let a = self.app();
        a.tabs.get(a.active).is_some_and(|t| t.table.is_some() && !t.busy())
    }

    fn col(&mut self, name: &str) -> usize {
        self.tab().st().and_then(|s| s.headers().iter().position(|h| h == name)).unwrap_or(0)
    }

    /// Back to the file as opened, untimed.
    fn plain(&mut self) {
        let ctx = self.ctx.clone();
        self.app().tools.clear();
        self.app().palette = None;
        self.app().dialog = None;
        self.app().toasts.clear();
        let f = self.file.clone();
        let reopen = { let a = self.app(); a.tabs.len() != 1 || a.tabs[0].dirty() || a.tabs[0].path.as_deref() != Some(f.as_path()) };
        if reopen {
            while !self.app().tabs.is_empty() { self.app().close_tab(0, true); }
            self.app().open(f, &ctx);
        }
        self.app().active = 0;
        let t = self.tab();
        t.filter = Default::default();
        t.col_filters.iter_mut().for_each(|s| s.clear());
        t.sort.clear();
        t.restrict = None;
        t.expr_mode = false;
        t.marks = Default::default();
        t.find.open = false;
        t.find.text.clear();
        t.show_totals = false;
        t.grid.panel = None;
        t.grid.scroll = 0.0;
        t.refresh(&ctx);
        self.h.remove_cursor();
        ctx.request_repaint();
        let t0 = Instant::now();
        while !self.loaded() && t0.elapsed() < Duration::from_secs(60) { self.frame(); self.wait(Duration::from_millis(50)); }
        self.settle(Duration::from_millis(600), Duration::from_secs(60));
        self.frames.clear();
        self.cpu.clear();
    }

    fn type_text(&mut self, s: &str, gap: Duration) {
        for ch in s.chars() {
            self.h.event(egui::Event::Text(ch.to_string()));
            self.frame();
            let t = Instant::now();
            while t.elapsed() < gap { if self.wait(gap.saturating_sub(t.elapsed())) { self.frame(); } }
        }
    }

    fn wheel(&mut self, notches: usize, gap: Duration) {
        for _ in 0..notches {
            self.h.event(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: egui::vec2(0.0, -3.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: egui::Modifiers::NONE });
            self.frame();
            let t = Instant::now();
            while t.elapsed() < gap { if self.wait(gap.saturating_sub(t.elapsed())) { self.frame(); } }
        }
    }

    fn find_text(&self, f: impl Fn(&str) -> bool) -> Option<Rect> {
        texts(&self.h).into_iter().rev().find(|(t, _)| f(t)).map(|x| x.1)
    }

    /// The x where column `name`'s first value is drawn (titles are not text shapes).
    fn col_x(&mut self, name: &str) -> Option<f32> {
        let c = self.col(name);
        let v = { let t = self.tab(); let id = t.shown_id(0); t.st()?.cell(id, c) };
        texts(&self.h).into_iter().filter(|(t, r)| *t == v && r.min.y > 150.0).min_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y)).map(|x| x.1.min.x)
    }

    /// The ▾ in a column's title: the first 20 × 20 button right of its values' start.
    fn col_button(&mut self, name: &str) -> Option<Pos2> {
        let x = self.col_x(name)?;
        self.ctx.viewport(|v| {
            v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.sense.senses_click() && (w.rect.width() - 20.0).abs() < 1.5 && (w.rect.height() - 20.0).abs() < 1.5 && w.rect.min.x > x && w.rect.min.y < 140.0).min_by(|a, b| a.rect.min.x.total_cmp(&b.rect.min.x)).map(|w| w.rect.center())
        })
    }

    /// A column's filter field: the text field under its title.
    fn col_filter_field(&mut self, name: &str) -> Option<Pos2> {
        let x = self.col_x(name)? + 30.0;
        self.ctx.viewport(|v| {
            v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.sense.is_focusable() && w.rect.height() < 26.0 && w.rect.width() > 40.0 && w.rect.x_range().contains(x) && w.rect.min.y > 150.0 && w.rect.max.y < 215.0).map(|w| w.rect.center()).next()
        })
    }

    fn run(&mut self, sc: &str) {
        eprintln!("== {sc}");
        self.cpu0 = proc_cpu_ms();
        let ctx = self.ctx.clone();
        let quiet = Duration::from_millis(700);
        let max = Duration::from_secs(120);
        match sc {
            "load" => {
                self.app().tools.clear();
                while !self.app().tabs.is_empty() { self.app().close_tab(0, true); }
                self.frame();
                self.settle(quiet, max);
                self.frames.clear();
        self.cpu.clear();
                let f = self.file.clone();
                let t = Instant::now();
                self.app().open(f, &ctx);
                let a = t.elapsed().as_secs_f64() * 1e3;
                ctx.request_repaint();
                let last = self.settle(Duration::from_millis(800), max);
                self.report(sc, a, last, "");
            }
            "registry" => {
                let t = Instant::now();
                let mut n = 0;
                for _ in 0..1000 { n += csvfab::tools::commands().len(); }
                println!("tools::commands(): {:.1} µs a call ({} commands)", t.elapsed().as_secs_f64() * 1e3, n / 1000);
            }
            "probe" => {
                for (t, r) in texts(&self.h) { if !t.trim().is_empty() && r.min.y < 240.0 { println!("[{:.0},{:.0} {:.0}x{:.0}] {t:?}", r.min.x, r.min.y, r.width(), r.height()); } }
                self.ctx.viewport(|v| for (_, ws) in v.prev_pass.widgets.layers() { for w in ws.iter() { if w.rect.min.y < 220.0 && w.rect.min.y > 70.0 && w.rect.min.x > 1600.0 { println!("widget {:?} {:?} click={} focus={}", w.rect, w.id, w.sense.senses_click(), w.sense.is_focusable()); } } });
            }
            "idle" => {
                let n = self.idle_frames(Duration::from_secs(3));
                println!("{sc:<48} idle frames in 3 s: {n}");
            }
            "menu" => {
                let y = self.find_text(|t| t == "Filter...").map(|r| r.center().y).unwrap_or(60.0);
                let x = self.app().m.band_pad_x + 16.0;
                self.click(Pos2::new(x, y), egui::PointerButton::Primary);
                self.frames.clear();
        self.cpu.clear();
                self.frame();
                let a = *self.cpu.last().unwrap();
                let last = self.settle(quiet, max);
                let open = self.find_text(|t| t == "Insights").is_some();
                self.report(sc, a, last, if open { "" } else { "  (menu NOT open)" });
                self.escape();
            }
            "savemenu" => {
                let r = self.find_text(|t| t == "Save").unwrap_or(Rect::NOTHING);
                // the ▾ is 30 px right of the Save segment's end; approximate from the window's right edge
                let p = Pos2::new(1920.0 - self.app().m.band_pad_x - 15.0, r.center().y);
                self.click(p, egui::PointerButton::Primary);
                self.frames.clear();
        self.cpu.clear();
                self.frame();
                let a = *self.cpu.last().unwrap();
                let last = self.settle(quiet, max);
                let open = self.find_text(|t| t == "Save as…").is_some();
                self.report(sc, a, last, if open { "" } else { "  (menu NOT open)" });
                self.escape();
            }
            "palette" => {
                let t = Instant::now();
                self.app().palette = Some(Default::default());
                ctx.request_repaint();
                self.frame();
                let a = t.elapsed().as_secs_f64() * 1e3;
                self.type_text("piv", Duration::from_millis(120));
                let last = self.settle(quiet, max);
                self.report(sc, a, last, "");
                self.app().palette = None;
                self.frame();
            }
            "colpanel" | "colpanel_sorted" => {
                if sc == "colpanel_sorted" {
                    let c = self.col("total_spent");
                    self.tab().sort = vec![SortKey { col: c, desc: true }];
                    self.tab().refresh(&ctx);
                    ctx.request_repaint();
                    self.settle(quiet, max);
                    self.frames.clear();
        self.cpu.clear();
                }
                let Some(p) = self.col_button("city") else { println!("{sc}: no ▾"); return };
                self.h.event(egui::Event::PointerMoved(p));
                self.frame();
                self.pointer(p, true, egui::PointerButton::Primary);
                self.frame();
                self.frames.clear();
        self.cpu.clear();
                self.pointer(p, false, egui::PointerButton::Primary);
                self.frame();
                let a = *self.cpu.last().unwrap();
                let last = self.settle(quiet, max);
                let open = self.tab().grid.panel.is_some();
                let counting = self.find_text(|t| t == "Counting…").is_some();
                self.report(sc, a, last, &format!("{}{}", if open { "" } else { "  (panel NOT open)" }, if counting { "  (still counting)" } else { "" }));
                self.escape();
            }
            "type_grid" | "type_nan" => {
                // A click on a cell then typing: the cell's editor opens with the text.
                let p = if sc == "type_nan" { Pos2::new(f32::NAN, f32::NAN) } else { Pos2::new(300.0, 400.0) };
                self.click(p, egui::PointerButton::Primary);
                self.frame();
                self.frames.clear();
        self.cpu.clear();
                self.type_text("lyon", Duration::from_millis(110));
                let last = self.settle(quiet, Duration::from_secs(10));
                let editing = self.tab().grid.editing.is_some();
                self.report(sc, 0.0, last, if editing { "  (editing)" } else { "" });
                self.escape();
                self.escape();
                let n = self.idle_frames(Duration::from_millis(1500));
                println!("{sc}: idle frames after Escape: {n}");
            }
            "insert_col" | "delete_col" | "edit_cell" | "undo" | "insert_col_sorted" | "edit_cell_sorted" | "delete_rows" | "copy_all" => {
                use csvfab::grid::Action;
                if sc.ends_with("_sorted") {
                    let c = self.col("total_spent");
                    self.tab().sort = vec![SortKey { col: c, desc: true }];
                    self.tab().refresh(&ctx);
                    ctx.request_repaint();
                    self.settle(quiet, max);
                    self.frames.clear();
                    self.cpu.clear();
                    self.cpu0 = proc_cpu_ms();
                }
                let id = { let t = self.tab(); t.shown_id(0) };
                let acts = match sc {
                    "insert_col" | "insert_col_sorted" => vec![Action::InsertColumn(3)],
                    "delete_col" => vec![Action::DeleteColumns(vec![3])],
                    "edit_cell" | "edit_cell_sorted" => vec![Action::SetCells(vec![(id, 1, "Zoé".into())])],
                    "delete_rows" => vec![Action::DeleteRows((0..1000).map(|p| self.tab().shown_id(p)).collect())],
                    "copy_all" => { let t = self.tab(); let n = t.shown(); t.grid.anchor = Some(csvfab::grid::Cell { row: 0, col: 0 }); t.grid.cursor = Some(csvfab::grid::Cell { row: n - 1, col: 2 }); vec![Action::Copy] }
                    _ => vec![],
                };
                let (c0, w0) = (cpu_ns(), Instant::now());
                if sc == "undo" {
                    self.app().apply(vec![Action::SetCells(vec![(id, 1, "Zoé".into())])], &ctx);
                    self.settle(quiet, max);
                    self.frames.clear();
                    self.cpu.clear();
                    self.cpu0 = proc_cpu_ms();
                    let c1 = cpu_ns();
                    self.app().undo(false, &ctx);
                    let a = (cpu_ns() - c1) as f64 / 1e6;
                    ctx.request_repaint();
                    let last = self.settle(quiet, max);
                    self.report(sc, a, last, "");
                    return;
                }
                self.app().apply(acts, &ctx);
                let a = (cpu_ns() - c0) as f64 / 1e6;
                println!("  {sc}: action wall {:.2} ms", w0.elapsed().as_secs_f64() * 1e3);
                ctx.request_repaint();
                let last = self.settle(quiet, max);
                self.report(sc, a, last, "");
            }
            "cellmenu" => {
                self.click(Pos2::new(300.0, 400.0), egui::PointerButton::Secondary);
                self.frames.clear();
        self.cpu.clear();
                self.frame();
                let a = *self.cpu.last().unwrap();
                let last = self.settle(quiet, max);
                self.report(sc, a, last, "");
                self.escape();
            }
            "totals" => {
                let t = Instant::now();
                self.tab().show_totals = true;
                ctx.request_repaint();
                self.frame();
                let a = t.elapsed().as_secs_f64() * 1e3;
                let last = self.settle(quiet, max);
                self.report(sc, a, last, "");
                self.tab().show_totals = false;
            }
            "find" => {
                let t = Instant::now();
                { let tb = self.tab(); tb.find.open = true; tb.find.text = "Lyon".into(); }
                ctx.request_repaint();
                self.frame();
                let a = t.elapsed().as_secs_f64() * 1e3;
                let last = self.settle(quiet, max);
                let n = self.tab().find.matches.len();
                self.report(sc, a, last, &format!("  ({n} matches listed)"));
            }
            "type_filter" | "type_colfilter" => {
                if sc == "type_filter" {
                    self.app().focus_search = true;
                } else {
                    let Some(p) = self.col_filter_field("city") else { println!("{sc}: no filter field"); return };
                    self.click(p, egui::PointerButton::Primary);
                }
                ctx.request_repaint();
                self.frame();
                self.frames.clear();
        self.cpu.clear();
                let t = Instant::now();
                self.type_text("lyon", Duration::from_millis(110));
                let last = self.settle(quiet, max);
                let shown = self.tab().shown();
                self.report(sc, 0.0, last + t.elapsed().saturating_sub(last), &format!("  ({shown} rows shown)"));
            }
            "sort" => {
                let c = self.col("last_name");
                let t = Instant::now();
                self.tab().sort = vec![SortKey { col: c, desc: false }];
                self.tab().refresh(&ctx);
                let a = t.elapsed().as_secs_f64() * 1e3;
                ctx.request_repaint();
                let last = self.settle(quiet, max);
                self.report(sc, a, last, "");
            }
            "scroll_sort" => {
                let c = self.col("total_spent");
                self.h.event(egui::Event::PointerMoved(Pos2::new(700.0, 500.0)));
                self.frame();
                self.frames.clear();
        self.cpu.clear();
                self.tab().sort = vec![SortKey { col: c, desc: true }];
                self.tab().refresh(&ctx);
                self.wheel(30, Duration::from_millis(16));
                let last = self.settle(quiet, max);
                self.report(sc, 0.0, last, "  (wheel during the sort)");
            }
            "scroll_insights" => {
                self.h.event(egui::Event::PointerMoved(Pos2::new(300.0, 700.0)));
                self.frame();
                self.frames.clear();
        self.cpu.clear();
                self.command("Insights");
                self.wheel(60, Duration::from_millis(16));
                let last = self.settle(quiet, max);
                self.report(sc, 0.0, last, "  (wheel while Insights computes)");
            }
            "cmds" => {
                let names: Vec<&str> = csvfab::tools::commands().iter().map(|c| c.name).collect();
                for n in names {
                    if SKIP.contains(&n) { continue; }
                    self.plain();
                    self.run(&format!("cmd:{n}"));
                }
            }
            s if s.starts_with("cmd:") => {
                let name = &s[4..];
                let c0 = cpu_ns();
                if !self.command(name) { println!("{s}: no such command or disabled"); return; }
                let a = (cpu_ns() - c0) as f64 / 1e6;
                let last = self.settle(quiet, max);
                let tools = self.app().tools.len();
                self.report(s, a, last, &format!("  ({tools} tool open)"));
            }
            _ => println!("unknown scenario {sc}"),
        }
    }

    fn command(&mut self, name: &str) -> bool {
        let Some(c) = csvfab::tools::commands().into_iter().find(|c| c.name == name) else { return false };
        if !(c.enabled)(self.tab()) { return false; }
        let ctx = self.ctx.clone();
        self.app().run_command(c.run, &ctx);
        ctx.request_repaint();
        true
    }
}

/// Commands that change the file for good or need another file: not run by `cmds`.
const SKIP: &[&str] = &["Discard all edits and reload from disk", "Delete hidden rows", "Stop following the file"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let file = PathBuf::from(args.get(1).expect("usage: audit <file> [scenarios]"));
    let all = "load,idle,menu,savemenu,palette,colpanel,colpanel_sorted,cellmenu,totals,find,type_filter,type_colfilter,sort,scroll_sort,scroll_insights,cmds";
    let list = args.get(2).cloned().unwrap_or_else(|| all.into());
    let mut a = A::new(file);
    // First load: until shown and settled.
    let t0 = Instant::now();
    while !a.loaded() && t0.elapsed() < Duration::from_secs(60) { a.frame(); a.wait(Duration::from_millis(50)); }
    a.settle(Duration::from_millis(800), Duration::from_secs(60));
    a.frames.clear();
    for sc in list.split(',') {
        if sc.starts_with("cmd:") || sc == "cmds" || sc != "load" { a.plain(); }
        a.run(sc);
    }
}
