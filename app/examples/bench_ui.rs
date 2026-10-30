//! The real app, off screen, timed operation by operation: the native half of
//! `benchmark/compare.py` (which also samples this process's memory). No window:
//! `csvfab::app::App` runs in an egui_kittest harness on frames driven like an
//! event loop would drive them — a frame when egui or a background job asks for
//! one (`request_repaint`), none otherwise.
//!
//!     bench_ui <file.csv> [--ops open,filter_global,…|edits] [--repeat 3] [--size 1920x1080] [--ppp 1]
//!              [--out DIR] [--timeout SECS] [--probe]
//!     bench_ui <file.csv> --serve [--repeat N]   # one op per line on stdin (N: runs planned, for the hooks)
//!     bench_ui <file.csv> --startup        # process start → grid drawn, then exit
//!
//! Each operation is set up the way the interface sets it (the tab's filter fields
//! then `Tab::refresh`, as typing does; a command run as the menu runs it; a click on
//! a button found by its text on screen), then frames run until the result is on
//! screen; the time stops after that frame is drawn and tessellated (the CPU part of
//! painting; the GPU's is not included — there is none here). Clicks start the clock
//! on the button's release, the press already sent.
//!
//! stdout: one JSON object per line, `{"op", "run", "ms", "out"}` (or `"error"`), and
//! marks for the memory sampler: `@op <name>` (timed part starts), `@done <name>`,
//! `@reset` — flushed at once. `--probe` prints the texts on screen after each op (stderr).

use csvfab::app::App;

/// The benchmark's columns are those of benchmark/gen.py's files; `CSVFAB_BENCH_COLS` maps
/// them onto another file's (`city=ville,total_spent=montant,…`).
fn col_name(name: &str) -> String {
    std::env::var("CSVFAB_BENCH_COLS").ok().and_then(|m| m.split(',').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v.to_string()))).unwrap_or_else(|| name.to_string())
}

/// A formula with its `{column}` names mapped (`col_name`).
fn cols_in(expr: &str) -> String {
    let mut out = expr.to_string();
    for k in ["city", "total_spent", "last_name", "signup_date", "country"] {
        out = out.replace(&format!("{{{k}}}"), &format!("{{{}}}", col_name(k)));
    }
    out
}
use csvfab_engine::view::SortKey;
use eframe::egui::{self, Pos2, Rect};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

type H = egui_kittest::Harness<'static, App>;

const ALL_OPS: &[&str] = &[
    "open", "filter_global", "filter_col", "filter_num", "filter_expr", "sort_num", "sort_text", "sort_date", "sort_multi", "clear", "find",
    "totals", "col_values", "group_by", "pivot", "dupes", "insights", "profile", "formula", "save_csv", "export_xlsx",
];

/// Edits, timed with the longest frame they cost the UI thread (`frame_max`): not in the
/// default run (each leaves the table edited, so the next run opens the file again).
const EDIT_OPS: &[&str] = &["clear_all", "fill_all", "move_col", "insert_row", "dup_row", "paste", "totals_edit", "panel_id", "merge_layers"];

fn say(line: &str) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A JSON object from (key, already-encoded value) pairs.
fn obj(kv: &[(&str, String)]) -> String {
    format!("{{{}}}", kv.iter().map(|(k, v)| format!("{}:{v}", esc(k))).collect::<Vec<_>>().join(","))
}

fn list(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| esc(s)).collect::<Vec<_>>().join(","))
}

/// When egui (or a job's thread) last asked for a frame: the event loop's wake-up.
#[derive(Default)]
struct Wake {
    at: Mutex<Option<Instant>>,
    cv: Condvar,
}

struct Bench {
    h: H,
    wake: Arc<Wake>,
    ctx: egui::Context,
    ppp: f32,
    file: PathBuf,
    out_dir: PathBuf,
    timeout: Duration,
    probe: bool,
    /// The longest frame since `mark` (the UI thread's longest wait), while `timing`.
    worst: Duration,
    timing: bool,
}

/// Every text drawn in the last frame, with its rectangle, in painting order (topmost last).
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

/// The totals row's values as drawn (`…` while one is computing): the bold monospace
/// texts on the line of the row's Σ.
fn totals_drawn(h: &H) -> Vec<String> {
    fn walk(s: &egui::Shape, out: &mut Vec<(String, Rect, bool)>) {
        match s {
            egui::Shape::Text(t) => {
                let bold = t.galley.job.sections.first().is_some_and(|x| x.format.font_id.family == egui::FontFamily::Name(csvfab::fonts::MONO_BOLD.into()));
                out.push((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()), bold));
            }
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut all = Vec::new();
    for c in &h.output().shapes { walk(&c.shape, &mut all); }
    let Some(sig) = all.iter().find(|x| x.0 == "Σ").map(|x| x.1) else { return Vec::new() };
    let mut v: Vec<(f32, String)> = all.into_iter().filter(|x| x.2 && (x.1.center().y - sig.center().y).abs() < 6.0 && x.1.min.x > sig.max.x).map(|x| (x.1.min.x, x.0)).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|x| x.1).collect()
}

fn has_text(h: &H, f: impl Fn(&str) -> bool) -> bool {
    texts(h).iter().any(|(t, _)| f(t))
}

fn find_text(h: &H, f: impl Fn(&str) -> bool) -> Option<String> {
    texts(h).into_iter().rev().find(|(t, _)| f(t)).map(|x| x.0)
}

/// The whole number in a text written with any grouping ("1 234 567", "1,234").
fn digits(s: &str) -> Option<u64> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

fn cpu_ticks() -> u64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = s.rsplit_once(')').map_or("", |x| x.1);
    let f: Vec<&str> = rest.split_whitespace().collect();
    f.get(11).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) + f.get(12).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0)
}

impl Bench {
    fn new(file: PathBuf, size: (f32, f32), ppp: f32, open: bool) -> Bench {
        let files = if open { vec![file.clone()] } else { vec![] };
        let h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(size.0, size.1))
            .with_pixels_per_point(ppp)
            .with_accessibility_check(false)
            .build_eframe(move |cc| App::new(cc, files));
        let wake = Arc::new(Wake::default());
        let w = wake.clone();
        h.ctx.set_request_repaint_callback(move |info| {
            let t = Instant::now() + info.delay;
            let mut g = w.at.lock().unwrap();
            if g.is_none_or(|x| t < x) {
                *g = Some(t);
            }
            w.cv.notify_all();
        });
        let ctx = h.ctx.clone();
        Bench { h, wake, ctx, ppp, out_dir: file.parent().unwrap_or(Path::new(".")).join("out"), file, timeout: Duration::from_secs(900), probe: false, worst: Duration::ZERO, timing: false }
    }

    fn app(&mut self) -> &mut App {
        self.h.state_mut()
    }

    fn tab(&mut self) -> &mut csvfab::tab::Tab {
        let a = self.h.state_mut();
        let i = a.active;
        &mut a.tabs[i]
    }

    fn col(&mut self, name: &str) -> usize {
        let name = col_name(name);
        self.tab().st().and_then(|s| s.headers().iter().position(|h| *h == name)).unwrap_or_else(|| panic!("no column {name}"))
    }

    /// One frame (its queued input events each get one).
    fn frame(&mut self) {
        *self.wake.at.lock().unwrap() = None;
        let t0 = Instant::now();
        self.h.step();
        if self.timing { self.worst = self.worst.max(t0.elapsed()); }
    }

    /// The timed part starts: its mark said, the longest frame counted from here.
    fn mark(&mut self, op: &str) -> Instant {
        say(&format!("@op {op}"));
        self.worst = Duration::ZERO;
        self.timing = true;
        Instant::now()
    }

    /// Presses a key with modifiers on the grid (focused first by a click on its first cell).
    fn grid_key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.h.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
        self.h.event(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    }

    /// Clicks a cell of the first row shown (focuses the grid, selects it): the grid's body
    /// is the biggest widget that senses clicks and drags.
    fn click_cell(&mut self) -> Result<(), String> {
        let body = self.ctx.viewport(|v| {
            v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.sense.senses_click() && w.sense.senses_drag()).map(|w| w.rect).max_by(|a, b| a.area().total_cmp(&b.area()))
        }).ok_or("no grid body")?;
        let p = body.min + egui::vec2(body.width() * 0.3, 12.0);
        self.press(p);
        self.release(p);
        self.frame();
        Ok(())
    }

    /// `f` on the app, counted as a frame (an action applied between frames is the UI thread's time too).
    fn timed(&mut self, f: impl FnOnce(&mut Bench)) {
        let t0 = Instant::now();
        f(self);
        if self.timing { self.worst = self.worst.max(t0.elapsed()); }
    }

    /// The ▾ of column `name`'s title: the square button right in it, found among the
    /// widgets of the last frame (between its title and the next one's, `next`).
    fn panel_button(&self, name: &str, next: &str) -> Result<Pos2, String> {
        let (name, next) = (&col_name(name), &col_name(next));
        let ts = texts(&self.h);
        let hy = ts.iter().find(|(t, _)| t == name).map(|x| x.1).ok_or(format!("no {name} title"))?;
        let next = ts.iter().find(|(t, _)| t == next).map(|x| x.1.min.x).unwrap_or(f32::INFINITY);
        self.ctx.viewport(|v| {
            v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.sense.senses_click() && (w.rect.width() - 20.0).abs() < 1.5 && (w.rect.height() - 20.0).abs() < 1.5 && (w.rect.center().y - hy.center().y).abs() < 8.0 && w.rect.min.x > hy.max.x && w.rect.max.x < next).map(|w| w.rect.center()).next()
        }).ok_or(format!("no ▾ button in the {name} title"))
    }

    fn cell_at(&mut self, pos: usize, col: usize) -> String {
        let t = self.tab();
        let id = t.shown_id(pos);
        t.st().map(|s| s.cell(id, col)).unwrap_or_default()
    }

    fn edited(h: &mut H) -> bool {
        let a = h.state_mut();
        a.tabs.get(a.active).is_some_and(|t| !t.busy() && t.grid.idle())
    }

    /// Waits as an event loop does: until a frame is asked for (now or after a delay), at most `max`.
    fn idle_wait(&self, max: Duration) {
        let deadline = Instant::now() + max;
        let mut g = self.wake.at.lock().unwrap();
        loop {
            let now = Instant::now();
            if g.is_some_and(|t| t <= now) || now >= deadline {
                return;
            }
            let until = g.map_or(deadline, |t| t.min(deadline));
            g = self.wake.cv.wait_timeout(g, until - now).unwrap().0;
        }
    }

    /// The CPU side of painting the last frame.
    fn paint(&mut self) {
        let shapes = self.h.output().shapes.clone();
        let _ = self.ctx.tessellate(shapes, self.ppp);
    }

    /// Frames as the event loop would run them until `done` holds after one; that frame is painted.
    fn until(&mut self, what: &str, mut done: impl FnMut(&mut H) -> bool) -> Result<(), String> {
        let t0 = Instant::now();
        let trace = std::env::var_os("BENCH_TRACE").is_some();
        loop {
            let f0 = Instant::now();
            self.frame();
            if trace { eprintln!("  {what}: frame at {:7.1} ms took {:6.1} ms", (f0 - t0).as_secs_f64() * 1e3, f0.elapsed().as_secs_f64() * 1e3); }
            if done(&mut self.h) {
                self.paint();
                return Ok(());
            }
            if t0.elapsed() > self.timeout {
                if self.probe { self.dump(what); }
                return Err(format!("{what}: not done after {:?}", self.timeout));
            }
            self.idle_wait(Duration::from_millis(250));
        }
    }

    /// Frames as asked until the process has used (almost) no CPU for 300 ms: background
    /// work of the last operation (charts, look-ups) does not run into the next one.
    fn quiesce(&mut self) {
        let t0 = Instant::now();
        loop {
            let (c0, w0) = (cpu_ticks(), Instant::now());
            while w0.elapsed() < Duration::from_millis(300) {
                self.idle_wait(Duration::from_millis(300).saturating_sub(w0.elapsed()));
                if self.wake.at.lock().unwrap().is_some_and(|t| t <= Instant::now()) {
                    self.frame();
                }
            }
            if cpu_ticks() - c0 <= 2 || t0.elapsed() > Duration::from_secs(60) {
                return;
            }
        }
    }

    fn dump(&self, what: &str) {
        eprintln!("--- texts on screen after {what}:");
        for (t, r) in texts(&self.h) {
            if !t.trim().is_empty() {
                eprintln!("  [{:.0},{:.0}] {t:?}", r.min.x, r.min.y);
            }
        }
    }

    fn pointer(&self, p: Pos2, pressed: bool) {
        self.h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }

    /// Moves to `label` (the topmost text equal to it) and presses: the release is `release`.
    fn press_text(&mut self, label: &str) -> Result<Pos2, String> {
        let r = texts(&self.h).into_iter().rev().find(|(t, _)| t == label).map(|x| x.1).ok_or_else(|| {
            if self.probe { self.dump(label); }
            format!("no {label:?} on screen")
        })?;
        self.press(r.center());
        Ok(r.center())
    }

    fn press(&mut self, p: Pos2) {
        self.h.event(egui::Event::PointerMoved(p));
        self.pointer(p, true);
        self.frame();
    }

    fn release(&mut self, p: Pos2) {
        self.pointer(p, false);
        self.frame();
        self.h.remove_cursor();
    }

    fn command(&mut self, name: &str) -> Result<(), String> {
        let c = csvfab::tools::commands().into_iter().find(|c| c.name == name).ok_or_else(|| format!("no command {name:?}"))?;
        let ctx = self.ctx.clone();
        self.app().run_command(c.run, &ctx);
        // The menu click that runs it is input: the event loop draws the frames that follow.
        ctx.request_repaint();
        Ok(())
    }

    fn close_tools(&mut self) {
        self.app().tools.clear();
        self.frame();
    }

    fn loaded(h: &mut H) -> bool {
        let a = h.state_mut();
        a.tabs.get(a.active).is_some_and(|t| t.table.is_some() && !t.busy())
    }

    /// The file's tab, opened again from scratch (a new tab: nothing cached), untimed.
    fn reopen(&mut self) -> Result<(), String> {
        self.close_tools();
        while !self.app().tabs.is_empty() {
            self.app().close_tab(0, true);
        }
        self.frame();
        let (f, ctx) = (self.file.clone(), self.ctx.clone());
        self.app().open(f, &ctx);
        self.until("reopen", Self::loaded)?;
        self.quiesce();
        Ok(())
    }

    /// Back to the file as opened: no filter, sort, find, totals, tool.
    fn plain(&mut self) -> Result<(), String> {
        say("@reset");
        self.close_tools();
        let f = self.file.clone();
        let a = self.app();
        if a.tabs.len() != 1 || a.tabs[0].path.as_deref() != Some(f.as_path()) || a.tabs[0].dirty() {
            return self.reopen();
        }
        a.active = 0;
        let ctx = self.ctx.clone();
        let t = self.tab();
        let busy = !t.filter.global.is_empty() || t.col_filters.iter().any(|s| !s.is_empty()) || !t.sort.is_empty() || t.restrict.is_some() || !t.filter.hidden.is_empty();
        t.filter.global.clear();
        t.filter.hidden.clear();
        t.col_filters.iter_mut().for_each(|s| s.clear());
        t.sort.clear();
        t.restrict = None;
        t.find.open = false;
        t.find.text.clear();
        t.show_totals = false;
        t.grid.panel = None;
        if busy { t.refresh(&ctx); }
        self.h.key_press(egui::Key::Escape);
        self.until("reset", Self::loaded)?;
        self.quiesce();
        Ok(())
    }

    fn first(&mut self, col: usize) -> String {
        let t = self.tab();
        if t.shown() == 0 { return String::new(); }
        let id = t.shown_id(0);
        t.st().map(|s| s.cell(id, col)).unwrap_or_default()
    }

    fn shown(&mut self) -> usize {
        self.tab().shown()
    }

    /// The view recomputed after `set` changed the tab as the interface does; timed.
    fn view_op(&mut self, name: &str, set: impl FnOnce(&mut csvfab::tab::Tab)) -> Result<Duration, String> {
        let ctx = self.ctx.clone();
        say(&format!("@op {name}"));
        let t0 = Instant::now();
        let t = self.tab();
        set(t);
        t.refresh(&ctx);
        self.until(name, Self::loaded)?;
        Ok(t0.elapsed())
    }

    /// One operation, measured: (time, what it found).
    fn run(&mut self, op: &str) -> Result<(Duration, String), String> {
        let ctx = self.ctx.clone();
        match op {
            "open" => {
                self.close_tools();
                while !self.app().tabs.is_empty() { self.app().close_tab(0, true); }
                self.frame();
                self.quiesce();
                let f = self.file.clone();
                say("@op open");
                let t0 = Instant::now();
                self.app().open(f, &ctx);
                self.until("open", Self::loaded)?;
                let d = t0.elapsed();
                let st = self.tab().st().cloned().unwrap();
                Ok((d, obj(&[("rows", st.len().to_string()), ("cols", st.ncols.to_string())])))
            }
            "filter_global" => {
                let d = self.view_op(op, |t| t.filter.global = "lyon".into())?;
                Ok((d, obj(&[("n", self.shown().to_string())])))
            }
            "filter_col" => {
                let c = self.col("city");
                let d = self.view_op(op, |t| t.col_filters[c] = "lyon".into())?;
                Ok((d, obj(&[("n", self.shown().to_string())])))
            }
            "filter_num" => {
                let c = self.col("total_spent");
                let d = self.view_op(op, |t| t.col_filters[c] = ">1000".into())?;
                Ok((d, obj(&[("n", self.shown().to_string())])))
            }
            "filter_expr" => {
                // The search box in formula mode (the web's only numeric condition).
                let d = self.view_op(op, |t| { t.expr_mode = true; t.filter.global = cols_in("num({total_spent}) > 1000").into(); })?;
                let n = self.shown();
                self.tab().expr_mode = false;
                Ok((d, obj(&[("n", n.to_string())])))
            }
            "sort_num" | "sort_text" | "sort_date" => {
                let (name, desc) = match op { "sort_num" => ("total_spent", true), "sort_text" => ("last_name", false), _ => ("signup_date", false) };
                let c = self.col(name);
                let d = self.view_op(op, |t| t.sort = vec![SortKey { col: c, desc }])?;
                Ok((d, obj(&[("first", esc(&self.first(c)))])))
            }
            "sort_multi" => {
                let (a, b) = (self.col("country"), self.col("total_spent"));
                let d = self.view_op(op, |t| t.sort = vec![SortKey { col: a, desc: false }, SortKey { col: b, desc: true }])?;
                Ok((d, obj(&[("first", list(&[self.first(a), self.first(b)]))])))
            }
            "clear" => {
                let c = self.col("total_spent");
                let t = self.tab();
                t.filter.global = "lyon".into();
                t.sort = vec![SortKey { col: c, desc: true }];
                t.refresh(&ctx);
                self.until("clear setup", Self::loaded)?;
                self.quiesce();
                let d = self.view_op(op, |t| {
                    t.filter.global.clear();
                    t.col_filters.iter_mut().for_each(|s| s.clear());
                    t.sort.clear();
                })?;
                Ok((d, obj(&[("n", self.shown().to_string())])))
            }
            "find" => {
                say("@op find");
                let t0 = Instant::now();
                let t = self.tab();
                t.find.open = true;
                t.find.text = "Lyon".into();
                self.until(op, |h| {
                    let a = h.state_mut();
                    !a.tabs[a.active].find.matches.is_empty()
                })?;
                let d = t0.elapsed();
                let listed = self.tab().find.matches.len();
                let shown = find_text(&self.h, |s| s.ends_with(" matches") || s.ends_with(" match")).unwrap_or_default();
                Ok((d, obj(&[("listed", listed.to_string()), ("n", digits(&shown).map_or("null".into(), |n| n.to_string())), ("text", esc(&shown))])))
            }
            "totals" => {
                // A fresh tab: totals of a previous run are neither reused nor drawn dimmed meanwhile.
                self.reopen()?;
                // Scrolled to the last columns (untimed): the quantities, which get a sum by default, are there.
                self.tab().grid.xoff = 100_000.0;
                self.frame();
                self.quiesce();
                let t0 = self.mark("totals");
                self.tab().show_totals = true;
                self.until(op, |h| { let v = totals_drawn(h); !v.is_empty() && !v.iter().any(|t| t == "…") })?;
                let d = t0.elapsed();
                let vals = totals_drawn(&self.h);
                Ok((d, obj(&[("values", list(&vals))])))
            }
            "col_values" => {
                let m = self.panel_button("city", "country")?;
                self.press(m);
                let t0 = self.mark("col_values");
                self.release(m);
                self.until(op, |h| {
                    let a = h.state_mut();
                    a.tabs[a.active].grid.panel.is_some() && !has_text(h, |t| t == "Counting…")
                })?;
                let d = t0.elapsed();
                if self.probe { self.dump(op); }
                let distinct = find_text(&self.h, |s| s.contains("distinct values")).unwrap_or_default();
                // The first value listed after "(Select all)" and its count (drawn right of it, same line).
                let ts = texts(&self.h);
                let mut top = (String::new(), String::new());
                if let Some(i) = ts.iter().position(|(t, _)| t == "(Select all)") {
                    let y0 = ts[i].1.center().y;
                    if let Some((v, r)) = ts.iter().filter(|(_, r)| r.center().y > y0 + 10.0 && (r.min.x - ts[i].1.min.x).abs() < 4.0).min_by(|a, b| a.1.center().y.total_cmp(&b.1.center().y)) {
                        let c = ts.iter().filter(|(t, q)| (q.center().y - r.center().y).abs() < 4.0 && q.min.x > r.max.x && digits(t).is_some() && t.chars().all(|c| c.is_ascii_digit() || c.is_whitespace())).min_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x)).map(|x| x.0.clone()).unwrap_or_default();
                        top = (v.clone(), c);
                    }
                }
                Ok((d, obj(&[("distinct", digits(distinct.split(" distinct").next().unwrap_or("")).map_or("null".into(), |n| n.to_string())), ("text", esc(&distinct)),
                    ("top", list(&[top.0, top.1.clone()])), ("top_n", digits(&top.1).map_or("null".into(), |n| n.to_string()))])))
            }
            "group_by" => {
                let before = self.app().tabs.len();
                // The column panel's "Group by" on city: the dialog with city ticked; its preview drawn (untimed).
                let c = self.col("city");
                let tool = csvfab::tools::group::open(self.tab(), Some(c));
                self.app().open_tool(tool);
                self.ctx.request_repaint();
                self.until("group dialog", |h| has_text(h, |t| t == "Open the result in a new tab"))?;
                self.quiesce();
                let p = self.press_text("Open the result in a new tab")?;
                say("@op group_by");
                let t0 = Instant::now();
                self.release(p);
                self.until(op, |h| {
                    let a = h.state_mut();
                    a.tabs.len() > before && a.tabs.get(a.active).is_some_and(|t| t.table.is_some() && !t.busy())
                })?;
                let d = t0.elapsed();
                let st = self.tab().st().cloned().unwrap();
                let out = obj(&[("groups", st.len().to_string()), ("header", list(&st.headers()))]);
                let n = self.app().tabs.len();
                self.app().close_tab(n - 1, true);
                self.app().active = 0;
                Ok((d, out))
            }
            "pivot" => {
                say("@op pivot");
                let t0 = Instant::now();
                self.command("Pivot table…")?;
                self.until("pivot stats", |h| has_text(h, |t| t.contains(" × ") && t.contains(" from ")))?;
                if std::env::var_os("BENCH_TRACE").is_some() { eprintln!("  pivot: stats at {:.1} ms", t0.elapsed().as_secs_f64() * 1e3); }
                // Drawn: the table's total line is there.
                self.until(op, |h| h.state_mut().tools.len() == 1 && has_text(h, |t| t == "Total"))?;
                let d = t0.elapsed();
                if self.probe { self.dump(op); }
                let stats = find_text(&self.h, |s| s.contains(" × ") && s.contains(" from ")).unwrap_or_default();
                // The spec as the controls show it: the text right after each label.
                let ts = texts(&self.h);
                let after = |lab: &str| ts.iter().find(|(t, _)| t == lab).and_then(|(_, r)| ts.iter().filter(|(_, q)| (q.center().y - r.center().y).abs() < 8.0 && q.min.x > r.max.x).min_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x)).map(|x| x.0.clone())).unwrap_or_default();
                let spec = list(&[after("Rows"), after("Columns"), after("Show"), ts.iter().filter(|(_, q)| q.min.y < ts.iter().find(|(t, _)| t == "Colour scale").map_or(0.0, |x| x.1.max.y)).map(|x| x.0.clone()).filter(|t| !t.is_empty()).nth_back(1).unwrap_or_default()]);
                let dims: Vec<u64> = stats.split(" from ").next().unwrap_or("").split(" × ").filter_map(digits).collect();
                Ok((d, obj(&[("text", esc(&stats)), ("spec", spec), ("rows", dims.first().map_or("null".into(), |n| n.to_string())), ("cols", dims.get(1).map_or("null".into(), |n| n.to_string()))])))
            }
            "dupes" => {
                say("@op dupes");
                let t0 = Instant::now();
                self.command("Remove duplicates…")?;
                self.until(op, |h| has_text(h, |t| (t.contains(" groups") && t.contains("deleted")) || t.starts_with("No duplicate among")))?;
                let d = t0.elapsed();
                let note = find_text(&self.h, |t| (t.contains(" groups") && t.contains("deleted")) || t.starts_with("No duplicate among")).unwrap_or_default();
                // "28 270 rows in 9 171 groups — … : 19 099 deleted"
                let num = |s: &str| digits(s).map_or("null".to_string(), |n| n.to_string());
                let rows = note.split(" rows in ").next().unwrap_or("");
                let groups = note.split(" rows in ").nth(1).and_then(|x| x.split(" groups").next()).unwrap_or("");
                let del = note.rsplit(": ").next().and_then(|x| x.split(" deleted").next()).unwrap_or("");
                Ok((d, obj(&[("text", esc(&note)), ("rows", num(rows)), ("groups", num(groups)), ("removable", num(del))])))
            }
            "insights" => {
                self.reopen()?; // its report is cached per tab
                say("@op insights");
                let t0 = Instant::now();
                self.command("Insights")?;
                self.until(op, |h| has_text(h, |t| t.contains(" read · ")))?;
                let d = t0.elapsed();
                let sum = find_text(&self.h, |t| t.contains(" read · ")).unwrap_or_default();
                let items = sum.split(" · ").nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
                Ok((d, obj(&[("text", esc(&sum)), ("items", items.to_string())])))
            }
            "profile" => {
                self.reopen()?; // cached per tab too
                say("@op profile");
                let t0 = Instant::now();
                self.command("File profile…")?;
                // Done when the progress line ("Profiling… k of n columns") has given way.
                self.until(op, |h| {
                    let ts = texts(h);
                    h.state_mut().tools.len() == 1 && ts.iter().any(|(t, _)| t == "File profile") && ts.iter().any(|(t, _)| t == "Copy as text")
                        && !ts.iter().any(|(t, _)| t.starts_with("Profiling"))
                })?;
                let d = t0.elapsed();
                if self.probe { self.dump(op); }
                Ok((d, obj(&[("cols", self.tab().st().map_or(0, |s| s.ncols).to_string())])))
            }
            "formula" => {
                let n = self.tab().st().map_or(0, |s| s.ncols);
                let tool = csvfab::tools::formula::open_with(self.tab(), None, &cols_in("num({total_spent}) * 2"));
                self.app().open_tool(tool);
                self.ctx.request_repaint();
                self.until("formula dialog", |h| has_text(h, |t| t == "Apply"))?;
                self.quiesce();
                let p = self.press_text("Apply")?;
                let t0 = self.mark("formula");
                self.release(p);
                self.until(op, |h| {
                    let a = h.state_mut();
                    let t = &a.tabs[a.active];
                    a.tools.is_empty() && t.st().is_some_and(|s| s.ncols == n + 1) && !t.busy()
                })?;
                let d = t0.elapsed();
                let first = self.first(n);
                let header = self.tab().st().map(|s| s.headers()[n].clone()).unwrap_or_default();
                Ok((d, obj(&[("first", esc(&first)), ("header", esc(&header))])))
            }
            "save_csv" => {
                std::fs::create_dir_all(&self.out_dir).map_err(|e| e.to_string())?;
                let dest = self.out_dir.join(format!("native-{}", self.file.file_name().unwrap().to_string_lossy()));
                let _ = std::fs::remove_file(&dest);
                say("@op save_csv");
                let t0 = Instant::now();
                self.app().save(Some(dest.clone()), true, &ctx);
                self.until(op, |h| !h.state_mut().saving())?;
                let d = t0.elapsed();
                let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                // The tab now follows the new file (as after Save as): the next op reopens the source.
                Ok((d, obj(&[("bytes", bytes.to_string()), ("path", esc(&dest.display().to_string()))])))
            }
            "export_xlsx" => {
                // The dialog writes beside the tab's file: the tab is pointed into the output
                // folder for the export, never left writing beside the source.
                std::fs::create_dir_all(&self.out_dir).map_err(|e| e.to_string())?;
                let inside = self.out_dir.join(self.file.file_name().unwrap());
                let real = self.tab().path.replace(inside);
                let dest = self.out_dir.join(self.file.with_extension("xlsx").file_name().unwrap());
                let _ = std::fs::remove_file(&dest);
                self.app().toasts.clear(); // a previous run's "Workbook written" must not count
                self.command("Save as Excel workbook")?;
                self.until("save dialog", |h| has_text(h, |t| t.starts_with("Writes the workbook")))?;
                self.quiesce();
                // The dialog's button: the lowest "Save" on screen (the title and the toolbar's are above).
                let r = texts(&self.h).into_iter().filter(|(t, _)| t == "Save").max_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y)).map(|x| x.1).ok_or("no Save button")?;
                let p = r.center();
                self.press(p);
                say("@op export_xlsx");
                let t0 = Instant::now();
                self.release(p);
                let done = self.until(op, |h| has_text(h, |t| t.contains("Workbook written") || t.starts_with("Could not")));
                let d = t0.elapsed();
                self.tab().path = real;
                done?;
                let msg = find_text(&self.h, |t| t.contains("Workbook written") || t.starts_with("Could not")).unwrap_or_default();
                let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                Ok((d, obj(&[("text", esc(&msg)), ("bytes", bytes.to_string()), ("path", esc(&dest.display().to_string()))])))
            }
            "clear_all" | "fill_all" => {
                // Ctrl+A then Delete (or a value typed over the whole selection, Enter kept on the block).
                self.click_cell()?;
                self.grid_key(egui::Key::A, egui::Modifiers::COMMAND);
                self.frame();
                self.quiesce();
                let t0 = self.mark(op);
                if op == "clear_all" {
                    self.grid_key(egui::Key::Delete, egui::Modifiers::NONE);
                } else {
                    let ctx = self.ctx.clone();
                    self.timed(|b| b.app().apply(vec![csvfab::grid::Action::Paste("x".into())], &ctx));
                    ctx.request_repaint();
                }
                let n = self.shown();
                let want = if op == "clear_all" { "" } else { "x" };
                self.until(op, |h| {
                    if !Self::edited(h) { return false; }
                    let a = h.state_mut();
                    let t = &a.tabs[a.active];
                    let st = t.st().unwrap();
                    st.cell(t.shown_id(n - 1), 14) == want && st.cell(t.shown_id(0), 0) == want
                })?;
                let d = t0.elapsed();
                let toast = self.app().toasts.last().map(|t| t.text.clone()).unwrap_or_default();
                Ok((d, obj(&[("toast", esc(&toast)), ("mid", esc(&self.cell_at(n / 2, 7)))])))
            }
            "move_col" => {
                let t0 = self.mark(op);
                let ctx = self.ctx.clone();
                let n = self.tab().st().unwrap().ncols;
                self.timed(|b| csvfab::grid::move_column(b.tab(), 0, n - 1, &ctx));
                ctx.request_repaint();
                self.until(op, Self::edited)?;
                let d = t0.elapsed();
                Ok((d, obj(&[("last", esc(&self.tab().st().unwrap().headers()[n - 1]))])))
            }
            "insert_row" | "dup_row" => {
                // Under a sort (the rows shown are a view): the row goes in the file and the view.
                let c = self.col("total_spent");
                self.view_op("sort setup", |t| t.sort = vec![SortKey { col: c, desc: true }])?;
                self.quiesce();
                let n = self.shown();
                let t0 = self.mark(op);
                let ctx = self.ctx.clone();
                if op == "insert_row" {
                    self.timed(|b| b.app().apply(vec![csvfab::grid::Action::InsertRows { before: 10, count: 1 }], &ctx));
                } else {
                    self.timed(|b| csvfab::grid::duplicate_row(b.tab(), 10, &ctx));
                }
                ctx.request_repaint();
                self.until(op, |h| { let a = h.state_mut(); a.tabs[a.active].shown() == n + 1 } && Self::edited(h))?;
                let d = t0.elapsed();
                Ok((d, obj(&[("rows", self.shown().to_string()), ("at", esc(&self.cell_at(11, c)))])))
            }
            "paste" => {
                self.click_cell()?;
                self.quiesce();
                let t0 = self.mark(op);
                let ctx = self.ctx.clone();
                self.timed(|b| b.app().apply(vec![csvfab::grid::Action::Paste("a\tb\nc\td\n".into())], &ctx));
                ctx.request_repaint();
                self.until(op, Self::edited)?;
                let d = t0.elapsed();
                Ok((d, obj(&[("cell", esc(&self.cell_at(1, 2)))])))
            }
            "totals_edit" => {
                // The totals row on and counted; then one cell of a summed column typed in.
                self.reopen()?;
                self.tab().grid.xoff = 100_000.0;
                self.tab().show_totals = true;
                self.until("totals setup", |h| { let v = totals_drawn(h); !v.is_empty() && !v.iter().any(|t| t == "…") })?;
                self.quiesce();
                let before = totals_drawn(&self.h);
                let c = self.col("total_spent");
                let id = self.tab().shown_id(0);
                let ctx = self.ctx.clone();
                let t0 = self.mark(op);
                self.timed(|b| b.app().apply(vec![csvfab::grid::Action::SetCells(vec![(id, c, "1000000".into())])], &ctx));
                ctx.request_repaint();
                self.until(op, |h| { let a = h.state_mut(); !csvfab::totals_ui::pending(&a.tabs[a.active]) } && totals_drawn(h) != before)?;
                let d = t0.elapsed();
                Ok((d, obj(&[("before", list(&before)), ("after", list(&totals_drawn(&self.h)))])))
            }
            "panel_id" => {
                // The id column's panel (every value distinct): opened, searched, "(Select all)" unticked.
                let m = self.panel_button("id", "first_name")?;
                self.press(m);
                let t0 = self.mark(op);
                self.release(m);
                self.until(op, |h| { let a = h.state_mut(); a.tabs[a.active].grid.panel.as_ref().is_some_and(|p| p.counted()) } && has_text(h, |t| t == "(Select all)"))?;
                let open = t0.elapsed();
                let open_max = self.worst;
                self.quiesce();
                // Frames with every value listed (hovering the list): what each one costs.
                let sa = texts(&self.h).into_iter().find(|(t, _)| t == "(Select all)").map(|x| x.1).ok_or("no select all")?;
                self.worst = Duration::ZERO;
                for k in 0..5 { self.h.event(egui::Event::PointerMoved(sa.center() + egui::vec2(0.0, 40.0 + k as f32))); self.frame(); }
                let idle_max = self.worst;
                // A search typed, one letter a frame (the field has the keyboard), then erased.
                self.worst = Duration::ZERO;
                let t1 = Instant::now();
                for ch in ["1", "2", "3"] { self.h.event(egui::Event::Text(ch.into())); self.frame(); }
                self.until("panel search", |h| { let a = h.state_mut(); a.tabs[a.active].grid.panel.as_ref().is_some_and(|p| p.searched()) })?;
                let search = t1.elapsed();
                let search_max = self.worst;
                for _ in 0..3 { self.grid_key(egui::Key::Backspace, egui::Modifiers::NONE); self.frame(); }
                self.until("panel search erased", |h| { let a = h.state_mut(); a.tabs[a.active].grid.panel.as_ref().is_some_and(|p| p.searched()) })?;
                self.quiesce();
                // "(Select all)" unticked: every value at once.
                let sa = texts(&self.h).into_iter().find(|(t, _)| t == "(Select all)").map(|x| x.1).ok_or("no select all after the search")?;
                self.worst = Duration::ZERO;
                let t2 = Instant::now();
                self.press(sa.center());
                self.release(sa.center());
                for _ in 0..3 { self.frame(); }
                let toggle = t2.elapsed();
                let toggle_max = self.worst;
                self.worst = Duration::ZERO;
                for k in 0..5 { self.h.event(egui::Event::PointerMoved(sa.center() + egui::vec2(0.0, 40.0 + k as f32))); self.frame(); }
                let idle_off_max = self.worst;
                self.h.key_press(egui::Key::Escape);
                self.frame();
                let ms = |d: Duration| format!("{:.1}", d.as_secs_f64() * 1e3);
                Ok((open, obj(&[("open_max", ms(open_max)), ("search", ms(search)), ("search_max", ms(search_max)), ("toggle", ms(toggle)), ("toggle_max", ms(toggle_max)), ("idle_max", ms(idle_max)), ("idle_off_max", ms(idle_off_max))])))
            }
            "merge_layers" => {
                // A computed column, then 13 single-cell edits in it: the 13th merges its layers.
                let n = self.tab().st().map_or(0, |s| s.ncols);
                let tool = csvfab::tools::formula::open_with(self.tab(), None, &cols_in("num({total_spent}) * 2"));
                self.app().open_tool(tool);
                self.ctx.request_repaint();
                self.until("formula dialog", |h| has_text(h, |t| t == "Apply"))?;
                let p = self.press_text("Apply")?;
                self.release(p);
                self.until("formula", |h| { let a = h.state_mut(); let t = &a.tabs[a.active]; a.tools.is_empty() && t.st().is_some_and(|s| s.ncols == n + 1) && !t.busy() })?;
                self.quiesce();
                let ctx = self.ctx.clone();
                let t0 = self.mark(op);
                for k in 0..13 {
                    let id = self.tab().shown_id(k * 7);
                    self.timed(|b| b.app().apply(vec![csvfab::grid::Action::SetCells(vec![(id, n, format!("v{k}"))])], &ctx));
                    self.frame();
                }
                self.until(op, Self::edited)?;
                let d = t0.elapsed();
                Ok((d, obj(&[("cell", esc(&self.cell_at(7, n)))])))
            }
            _ => Err(format!("unknown op {op}")),
        }
    }
}

fn main() {
    let t_main = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let Some(file) = args.get(1).filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: bench_ui <file.csv> [--ops a,b] [--repeat N] [--size WxH] [--ppp P] [--out DIR] [--timeout S] [--probe] [--startup]");
        std::process::exit(2);
    };
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let has = |name: &str| args.iter().any(|a| a == name);
    let repeat: usize = flag("--repeat").and_then(|s| s.parse().ok()).unwrap_or(3);
    let size = flag("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))).unwrap_or((1920.0f32, 1080.0f32));
    let ppp: f32 = flag("--ppp").and_then(|s| s.parse().ok()).unwrap_or(1.0);
    // The screenshot hook the duplicates dialog reads when it opens (its suggested keys, as
    // the "Suggest keys" button), one per run; also keeps the app from listening for a
    // second launch (no single-instance socket).
    let hooks: Vec<String> = (0..repeat + 2).map(|_| "dupes-suggest".to_string()).collect();
    // SAFETY: before any other thread exists.
    unsafe { std::env::set_var("CSVFAB_SHOT", hooks.join(",")) };
    let path = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));

    if has("--startup") {
        // Process start → the first frame showing the file's rows drawn, as `csvfab file.csv`.
        let mut b = Bench::new(path, size, ppp, true);
        let built = t_main.elapsed();
        if let Err(e) = b.until("startup", Bench::loaded) {
            say(&obj(&[("op", esc("startup")), ("error", esc(&e))]));
            std::process::exit(1);
        }
        say("@ready");
        let st = b.tab().st().cloned().unwrap();
        say(&obj(&[("op", esc("startup")), ("ms_in_process", format!("{:.3}", t_main.elapsed().as_secs_f64() * 1e3)),
            ("ms_app_built", format!("{:.3}", built.as_secs_f64() * 1e3)), ("out", obj(&[("rows", st.len().to_string())]))]));
        std::process::exit(0);
    }

    let mut b = Bench::new(path.clone(), size, ppp, false);
    b.probe = has("--probe");
    if let Some(d) = flag("--out") { b.out_dir = PathBuf::from(d); }
    if let Some(s) = flag("--timeout").and_then(|s| s.parse().ok()) { b.timeout = Duration::from_secs(s); }
    b.frame();
    b.quiesce();
    let mut opened = false;
    let mut runs: std::collections::HashMap<String, usize> = Default::default();
    let mut one = |b: &mut Bench, op: &str| {
        let run = runs.entry(op.to_string()).or_default();
        if op != "open" && (!opened || b.plain().is_err()) {
            if let Err(e) = b.reopen() {
                say(&obj(&[("op", esc(op)), ("run", run.to_string()), ("error", esc(&format!("reopen: {e}")))]));
                *run += 1;
                return;
            }
        }
        opened = true;
        b.timing = false;
        let r = b.run(op);
        let worst = std::mem::take(&mut b.worst);
        let timed = std::mem::take(&mut b.timing);
        say(&format!("@done {op}"));
        if b.probe { b.dump(op); }
        match r {
            Ok((d, out)) => {
                let mut kv = vec![("op", esc(op)), ("run", run.to_string()), ("ms", format!("{:.3}", d.as_secs_f64() * 1e3))];
                if timed { kv.push(("frame_max", format!("{:.3}", worst.as_secs_f64() * 1e3))); }
                kv.push(("out", out));
                say(&obj(&kv))
            }
            Err(e) => say(&obj(&[("op", esc(op)), ("run", run.to_string()), ("error", esc(&e))])),
        }
        *run += 1;
        b.quiesce();
        say(&format!("@steady {op}"));
        std::thread::sleep(Duration::from_millis(400));
        say("@idle");
    };
    if has("--serve") {
        // One operation per line on stdin (the orchestrator interleaves this app with the web app).
        let stdin = std::io::stdin();
        let mut line = String::new();
        while { line.clear(); stdin.read_line(&mut line).unwrap_or(0) > 0 } {
            let op = line.trim();
            if op == "quit" { break; }
            if !op.is_empty() { one(&mut b, op); }
        }
        return;
    }
    let ops: Vec<String> = flag("--ops").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_else(|| ALL_OPS.iter().map(|s| s.to_string()).collect());
    let ops: Vec<String> = ops.into_iter().flat_map(|o| if o == "edits" { EDIT_OPS.iter().map(|s| s.to_string()).collect() } else { vec![o] }).collect();
    for op in &ops {
        for _ in 0..repeat {
            one(&mut b, op);
        }
    }
    say("@end");
    std::thread::sleep(Duration::from_millis(1500));
}
