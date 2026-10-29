//! The real app off screen (egui_kittest) driven by a seeded monkey: pointer moves, clicks,
//! double and right clicks, drags, the wheel, typing, pastes, shortcuts, resizes, on hostile
//! files. Nothing may panic on the UI thread, and the app stays sane: tab and cursor
//! in range, the edits all undone give the file as read.
//!
//! `TORTURE_SEED=<n>` replays a run (a failure prints it, with the last events),
//! `TORTURE_N=<x>` scales the number of events (1 by default; 0.1 for a quick look).
//! The long variants are `#[ignore]`d: `cargo test --release -p csvfab torture -- --ignored`.

use crate::app::App;
use eframe::egui::{self, pos2, vec2, Event, Key, Modifiers, PointerButton, Pos2};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
    fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

fn env_seed(tag: &str) -> u64 {
    let given = std::env::var("TORTURE_SEED").ok().and_then(|s| {
        let s = s.trim();
        s.strip_prefix("0x").map_or_else(|| s.parse().ok(), |h| u64::from_str_radix(h, 16).ok())
    });
    // A fixed seed per test by default: the suite is reproducible.
    given.unwrap_or_else(|| tag.bytes().fold(0x9E37_79B9_7F4A_7C15u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01B3))) | 1
}

fn scale() -> f64 {
    std::env::var("TORTURE_N").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0)
}

/// Once per process: no file dialog can open on the desktop (rfd asks the portal over D-Bus,
/// then runs `zenity`: no bus, a `zenity` that fails, no display). A panic is caught on the
/// test's own thread only (`catch_unwind` around each action): the other tests of this process
/// panic on purpose now and then (job.rs), and a job's panic ends the job as a failure (job.rs).
fn sandbox() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let bin = std::env::temp_dir().join(format!("csvfab-torture-bin-{}", std::process::id()));
        std::fs::create_dir_all(&bin).unwrap();
        let z = bin.join("zenity");
        std::fs::write(&z, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&z, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        // Once, before this test builds anything (other tests' threads only read it through std's lock).
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/csvfab-torture-bus");
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
    });
}

/// The files the monkey plays with: (tag, bytes).
fn files() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>)> = Vec::new();
    let mut normal = String::from("id,name,city,amount,date,email\n");
    for i in 0..300 {
        normal += &format!("{i},Name {},{},{}.{:02},2024-{:02}-{:02},user{i}@example.com\n", i % 37, ["Paris", "Lyon", "Nîmes", ""][i % 4], i * 7 % 1000, i % 100, i % 12 + 1, i % 28 + 1);
    }
    v.push(("normal", normal.into_bytes()));
    v.push(("ragged", b"a,b,c\n1\n1,2\n1,2,3,4,5\n,,,\n\n\nx,\"y\nz\",w,v,u,t\n7,8,9".to_vec()));
    v.push(("empty", Vec::new()));
    v.push(("header only", b"alpha;beta;gamma\n".to_vec()));
    v.push(("newlines", b"\n\n\r\n\n".to_vec()));
    v.push(("one column", (0..120).map(|i| format!("v{}\n", i % 9)).collect::<String>().into_bytes()));
    let uni = "名前,ﻣﺪﻳﻨﺔ,emoji,combining\n\
        東京,القاهرة,😀👩‍👩‍👧‍👦🏳️‍🌈,e\u{301}\u{302}\u{303}\u{304}\n\
        Ελλάδα,שלום עולם,\u{200b}\u{200d}\u{feff},Z\u{336}\u{337}a\u{338}\n\
        ,\u{202e}reversed\u{202c},\u{1F600}\u{1F600}\u{1F600},\u{0}\u{7}\u{1b}[31m\n";
    v.push(("unicode", uni.as_bytes().to_vec()));
    let mut wide = (0..320).map(|c| format!("c{c}")).collect::<Vec<_>>().join("\t") + "\n";
    for r in 0..40 { wide += &((0..320).map(|c| format!("{}", r * c)).collect::<Vec<_>>().join("\t") + "\n"); }
    v.push(("wide", wide.into_bytes()));
    let long = format!("k,text\n1,{}\n2,\"{}\"\n3,{}\n", "x".repeat(20_000), "line\n".repeat(400), "é".repeat(5000));
    v.push(("long cells", long.into_bytes()));
    v.push(("crlf quotes", b"a,b,c\r\n\"q,1\",\"multi\r\nline\",\"say \"\"hi\"\"\"\r\n\"unclosed,2,3\r\n4,5,6\r\n".to_vec()));
    v.push(("latin1", b"nom;ville;prix\r\nJos\xE9;N\xEEmes;12,5\r\nFran\xE7ois;S\xE8te;3\r\n\xC9lise;\xC0 voir;\x80\r\n".to_vec()));
    v.push(("raw text", "Lorem ipsum dolor sit amet, consectetur\nAnother line of prose here\n\n   indented\n".repeat(30).into_bytes()));
    let mut g = Rng(0xDEAD_BEEF);
    v.push(("garbage", (0..3000).map(|_| *g.pick(b"ab,;\t|\"\r\n\0\xff\xfe\xc3\xa9 ")).collect()));
    v
}

fn many_rows() -> Vec<u8> {
    let mut s = String::from("id,group,value,label\n");
    for i in 0..20_000 { s += &format!("{i},g{},{},\"lbl {}, {}\"\n", i % 17, (i * 7919) % 10007, i % 101, i % 3); }
    s.into_bytes()
}

type H = egui_kittest::Harness<'static, App>;

/// Tofu, as the app draws it, for the characters no font here has (CJK, rare emoji): the
/// harness would panic on them by default.
fn harness(path: &Path) -> H {
    let p = path.to_path_buf();
    let mut h = egui_kittest::Harness::builder().allow_missing_glyphs().with_size(vec2(1200.0, 700.0)).with_pixels_per_point(1.0).build_eframe(move |cc| App::new(cc, vec![p]));
    settle(&mut h);
    h
}

/// Frames until the tabs are read and their jobs done (a few seconds at most).
fn settle(h: &mut H) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        h.step();
        if h.state().tabs.iter().all(|t| !t.busy()) { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.step();
}

/// The monkey's run on one file, with what it did lately (printed on failure).
struct Monkey {
    r: Rng,
    seed: u64,
    tag: String,
    log: VecDeque<String>,
    n: usize,
    size: egui::Vec2,
    path: PathBuf,
}

const TEXTS: &[&str] = &["a", "x", "1", "-3.5", "2024-02-30", "=1+1", "\"", ",", ";", "\t", " ", "é", "😀", "👩‍👩‍👧", "e\u{301}", "\u{202e}", "\0", "\r", "a\nb", "{\"k\":1}", "%s%n", "<b>", "    ", "名前"];

const KEYS: &[Key] = &[
    Key::ArrowDown, Key::ArrowUp, Key::ArrowLeft, Key::ArrowRight, Key::PageUp, Key::PageDown, Key::Home, Key::End,
    Key::Enter, Key::Tab, Key::Escape, Key::Delete, Key::Backspace, Key::F2, Key::F3, Key::F1, Key::Space,
    Key::A, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H, Key::I, Key::M, Key::P, Key::V, Key::X, Key::Y, Key::Z, Key::K, Key::L, Key::R, Key::T, Key::B, Key::N, Key::U, Key::J,
    Key::S, Key::W, Key::Q, Key::Num1, Key::Num2, Key::Num0, Key::Minus, Key::Plus, Key::Equals,
];

impl Monkey {
    fn note(&mut self, s: String) {
        if self.log.len() >= 40 { self.log.pop_front(); }
        self.log.push_back(format!("#{} {s}", self.n));
    }

    fn report(&self, what: &str) -> String {
        let log: Vec<&str> = self.log.iter().map(|s| s.as_str()).collect();
        format!("monkey on {:?}: {what}\nreplay: TORTURE_SEED={:#x}\nlast events:\n  {}", self.tag, self.seed, log.join("\n  "))
    }

    /// Somewhere worth poking: the gutter, the bands above the grid, the scroll bars, the edges.
    fn spot(&mut self) -> Pos2 {
        let (w, h) = (self.size.x, self.size.y);
        let r = &mut self.r;
        match r.below(10) {
            0 => pos2(r.f(0.0, 70.0), r.f(0.0, h)),
            1 | 2 => pos2(r.f(0.0, w), r.f(0.0, 260.0)),
            3 => pos2(r.f(w - 24.0, w), r.f(0.0, h)),
            4 => pos2(r.f(0.0, w), r.f(h - 40.0, h)),
            5 => pos2(r.f(-20.0, w + 20.0), r.f(-20.0, h + 20.0)),
            _ => pos2(r.f(0.0, w), r.f(0.0, h)),
        }
    }

    fn mods(&mut self) -> Modifiers {
        match self.r.below(12) {
            0 | 1 => Modifiers::COMMAND,
            2 => Modifiers::SHIFT,
            3 => Modifiers::ALT,
            4 => Modifiers::COMMAND | Modifiers::SHIFT,
            _ => Modifiers::NONE,
        }
    }

    fn button(h: &H, pos: Pos2, b: PointerButton, pressed: bool, modifiers: Modifiers) {
        h.event(Event::PointerButton { pos, button: b, pressed, modifiers });
    }

    /// One random action, and the frames it needs.
    fn act(&mut self, h: &mut H) {
        self.n += 1;
        match self.r.below(100) {
            0..=2 => {
                // Any command of the menus and the palette (every tool), when it is enabled.
                let ctx = h.ctx.clone();
                let cmds: Vec<_> = crate::tools::commands().into_iter().filter(|c| h.state().tabs.get(h.state().active).is_some_and(|t| (c.enabled)(t))).collect();
                if cmds.is_empty() { return; }
                let c = &cmds[self.r.below(cmds.len())];
                self.note(format!("command {:?}", c.name));
                h.state_mut().run_command(c.run, &ctx);
            }
            3..=24 => {
                let p = self.spot();
                self.note(format!("move {p:?}"));
                h.hover_at(p);
            }
            25..=39 => {
                let (p, m) = (self.spot(), self.mods());
                self.note(format!("click {p:?} {m:?}"));
                h.hover_at(p);
                h.step();
                Self::button(h, p, PointerButton::Primary, true, m);
                h.step();
                Self::button(h, p, PointerButton::Primary, false, m);
            }
            40..=43 => {
                let p = self.spot();
                self.note(format!("double click {p:?}"));
                h.hover_at(p);
                for _ in 0..2 {
                    Self::button(h, p, PointerButton::Primary, true, Modifiers::NONE);
                    Self::button(h, p, PointerButton::Primary, false, Modifiers::NONE);
                }
            }
            44..=48 => {
                let p = self.spot();
                let b = if self.r.chance(80) { PointerButton::Secondary } else { PointerButton::Middle };
                self.note(format!("{b:?} click {p:?}"));
                h.hover_at(p);
                h.step();
                Self::button(h, p, b, true, Modifiers::NONE);
                h.step();
                Self::button(h, p, b, false, Modifiers::NONE);
            }
            49..=55 => {
                let (a, b, m) = (self.spot(), self.spot(), self.mods());
                let steps = 2 + self.r.below(8);
                self.note(format!("drag {a:?} → {b:?} in {steps} {m:?}"));
                h.hover_at(a);
                h.step();
                Self::button(h, a, PointerButton::Primary, true, m);
                h.step();
                for k in 1..=steps {
                    let t = k as f32 / steps as f32;
                    h.hover_at(a + (b - a) * t);
                    h.step();
                }
                Self::button(h, b, PointerButton::Primary, false, m);
            }
            56..=65 => {
                let p = self.spot();
                let big = self.r.chance(15);
                let k = if big { 1e6 } else { 300.0 };
                let delta = vec2(if self.r.chance(30) { self.r.f(-k, k) } else { 0.0 }, self.r.f(-k, k));
                let unit = *self.r.pick(&[egui::MouseWheelUnit::Point, egui::MouseWheelUnit::Line, egui::MouseWheelUnit::Page]);
                let m = self.mods();
                self.note(format!("wheel {delta:?} {unit:?} at {p:?} {m:?}"));
                h.hover_at(p);
                h.event(Event::MouseWheel { unit, delta, phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: m });
            }
            66..=73 => {
                let mut s = String::new();
                for _ in 0..1 + self.r.below(4) { s += self.r.pick(TEXTS); }
                // As a window sends typing (egui-winit): no control character; line breaks
                // and NULs come in pastes only (egui's word selection underflows on a line break
                // typed into a single-line field — not something a keyboard does).
                s.retain(|c| !c.is_control());
                if s.is_empty() { return; }
                self.note(format!("type {s:?}"));
                h.event(Event::Text(s));
            }
            74..=91 => {
                let (k, m) = (*self.r.pick(KEYS), self.mods());
                // Ctrl+O is the system's file dialog (sandboxed anyway): nothing to test there.
                if m.command && !m.shift && k == Key::O { return; }
                self.note(format!("key {m:?} {k:?}"));
                h.key_press_modifiers(m, k);
            }
            92 => {
                let s = vec2(self.r.f(320.0, 2200.0), self.r.f(200.0, 1400.0));
                self.note(format!("resize {s:?}"));
                self.size = s;
                h.set_size(s);
            }
            93 => {
                let ppp = *self.r.pick(&[0.75f32, 1.0, 1.25, 1.5, 2.0]);
                self.note(format!("pixels per point {ppp}"));
                h.set_pixels_per_point(ppp);
            }
            94 => {
                let e = match self.r.below(4) {
                    0 => Event::Copy,
                    1 => Event::Cut,
                    _ => {
                        let mut s = String::new();
                        for r in 0..1 + self.r.below(5) {
                            if r > 0 { s.push('\n'); }
                            let cells: Vec<&str> = (0..1 + self.r.below(6)).map(|_| *self.r.pick(TEXTS)).collect();
                            s += &cells.join("\t");
                        }
                        Event::Paste(s)
                    }
                };
                self.note(format!("{e:?}"));
                h.event(e);
            }
            95 => {
                self.note("pointer gone".into());
                h.event(Event::PointerGone);
            }
            96 => {
                self.note("zoom".into());
                h.event(Event::Zoom(self.r.f(0.5, 2.0)));
            }
            97 => {
                // Let the jobs (filters, sorts, tools) finish and show.
                self.note("wait".into());
                std::thread::sleep(Duration::from_millis(15));
            }
            _ => {
                let ctx = h.ctx.clone();
                if h.state().tabs.len() < 3 {
                    self.note("open the file again in a tab / switch".into());
                    let p = self.path.clone();
                    h.state_mut().open(p, &ctx);
                }
            }
        }
        h.step();
    }

    /// What must hold between any two frames.
    fn check(&mut self, h: &H) {
        let app = h.state();
        assert!(app.tabs.is_empty() || app.active < app.tabs.len(), "{}", self.report(&format!("active tab {} of {}", app.active, app.tabs.len())));
        for t in &app.tabs {
            let Some(table) = &t.table else { continue };
            if t.busy() { continue; }
            let st = &table.st;
            if t.view.is_none() {
                assert!(t.shown() == st.len(), "{}", self.report("rows shown without a view"));
            }
            for c in t.grid.cursor.iter().chain(t.grid.anchor.iter()) {
                assert!(c.row < t.shown().max(1) && c.col < st.ncols.max(1), "{}", self.report(&format!("cursor {c:?} past {} rows × {} columns", t.shown(), st.ncols)));
            }
        }
    }
}

/// Every edit undone: the table reads as the file did (when nothing was saved over it).
fn undo_all(h: &mut H, m: &Monkey, original: &[u8]) {
    let ctx = h.ctx.clone();
    // The file's own tab (tools open others: a group-by's result, a split's parts).
    let real = std::fs::canonicalize(&m.path).unwrap_or(m.path.clone());
    let Some(i) = h.state().tabs.iter().position(|t| t.path.as_ref() == Some(&real)) else { return };
    h.state_mut().active = i;
    for _ in 0..2 {
        for redo in [false, true, false] {
            for _ in 0..10_000 {
                let app = h.state_mut();
                let Some(t) = app.tabs.get(app.active).and_then(|t| t.table.as_ref()) else { break };
                if if redo { !t.can_redo() } else { !t.can_undo() } { break; }
                app.undo(redo, &ctx);
            }
            h.step();
        }
    }
    settle(h);
    let app = h.state();
    let Some(tab) = app.tabs.get(app.active) else { return };
    let Some(table) = &tab.table else { return };
    // Saved, or read again another way (encoding, delimiter): the file as read is another.
    if std::fs::read(&m.path).ok().as_deref() != Some(original) || tab.open_opts.enc.is_some() || tab.open_opts.delim.is_some() || table.st.base.partial {
        return;
    }
    let want = csvfab_engine::Table::new(std::sync::Arc::new(csvfab_engine::Base::from_bytes(original.to_vec(), Default::default())), Some(table.st.header));
    let (a, b) = (&table.st, &want.st);
    assert_eq!((a.ncols, a.all_len()), (b.ncols, b.all_len()), "{}", m.report("shape after undoing everything"));
    for p in 0..a.all_len() {
        for c in 0..a.ncols {
            assert_eq!(a.cell(a.all_id(p), c), b.cell(b.all_id(p), c), "{}", m.report(&format!("cell {p}.{c} after undoing everything")));
        }
    }
}

/// The monkey on one file for about `events` actions.
fn run_on(tag: &str, bytes: &[u8], events: usize, seed: u64) {
    sandbox();
    let dir = std::env::temp_dir().join(format!("csvfab-torture-ui-{}-{}", tag.replace(' ', "-"), std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.csv");
    std::fs::write(&path, bytes).unwrap();
    let mut h = harness(&path);
    let mut m = Monkey {
        r: Rng(seed ^ tag.bytes().fold(0u64, |a, b| a.rotate_left(7) ^ b as u64) | 1),
        seed,
        tag: tag.to_string(),
        log: VecDeque::new(),
        n: 0,
        size: vec2(1200.0, 700.0),
        path: path.clone(),
    };
    for _ in 0..events {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.act(&mut h)));
        if let Err(e) = r {
            eprintln!("{}", m.report("panicked (above)"));
            std::panic::resume_unwind(e);
        }
        m.check(&h);
        // The welcome screen (every tab closed): the file again.
        if h.state().tabs.is_empty() && m.r.chance(20) {
            let ctx = h.ctx.clone();
            h.state_mut().open(path.clone(), &ctx);
        }
    }
    settle(&mut h);
    m.check(&h);
    {
        let app = h.state();
        let t = app.tabs.get(app.active);
        eprintln!("monkey {tag}: {} actions, {} tabs, {} tools, dialog {}, undo {}, dirty {}, sort {:?}, filter {:?}", m.n, app.tabs.len(), app.tools.len(), app.dialog.is_some(),
            t.and_then(|t| t.table.as_ref()).is_some_and(|t| t.can_undo()), t.is_some_and(|t| t.dirty()), t.map(|t| t.sort.clone()), t.map(|t| t.filter.global.clone()));
    }
    // Whatever dialog or tool is up: closed, then every edit undone.
    for _ in 0..4 { h.key_press(Key::Escape); h.step(); }
    let original = bytes.to_vec();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| undo_all(&mut h, &m, &original)));
    if let Err(e) = r {
        eprintln!("{}", m.report("undoing everything panicked (above)"));
        std::panic::resume_unwind(e);
    }
    m.check(&h);
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

fn events(base: usize) -> usize {
    ((base as f64 * scale()) as usize).max(1)
}

/// Every small hostile file, a few thousand actions in all.
#[test]
fn torture_monkey_small_files() {
    let seed = env_seed("small");
    for (tag, bytes) in files() {
        let t = Instant::now();
        run_on(tag, &bytes, events(400), seed);
        eprintln!("monkey {tag}: {:?}", t.elapsed());
    }
}

/// 20,000 rows: scrolling, sorting and filtering for real.
#[test]
fn torture_monkey_many_rows() {
    run_on("many rows", &many_rows(), events(1500), env_seed("many"));
}

/// Hours of it: every file, ten times the actions, several seeds.
#[test]
#[ignore]
fn torture_monkey_long() {
    let seed = env_seed("long");
    for k in 0..4u64 {
        for (tag, bytes) in files() {
            run_on(tag, &bytes, events(4000), seed.wrapping_add(k.wrapping_mul(0x9E37_79B9)));
        }
        run_on("many rows", &many_rows(), events(8000), seed.wrapping_add(k));
    }
}

// ---- What the monkey found ---------------------------------------------------------------

/// The app on `csv`, read, with the dir to remove.
fn app_with(csv: &[u8], tag: &str) -> (H, PathBuf) {
    sandbox();
    let dir = std::env::temp_dir().join(format!("csvfab-torture-reg-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.csv");
    std::fs::write(&path, csv).unwrap();
    (harness(&path), dir)
}

/// A selection of two cells kept while a filter leaves no row: the status bar summed the
/// selection's rows past the rows shown.
#[test]
fn selection_over_no_row() {
    use crate::grid::Cell;
    let (mut h, dir) = app_with(b"a,b\n1,2\n3,4\n", "norow");
    let ctx = h.ctx.clone();
    let t = &mut h.state_mut().tabs[0];
    (t.grid.cursor, t.grid.anchor) = (Some(Cell { row: 0, col: 1 }), Some(Cell { row: 1, col: 0 }));
    t.filter.global = "no such text".into();
    t.refresh(&ctx);
    settle(&mut h);
    h.run_steps(3);
    assert_eq!(h.state().tabs[0].shown(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A window too small for the scroll bars' thumbs, the column tooltip or a row dragged:
/// their `clamp`s got a minimum above the maximum (grid.rs strips, spark_ui part_at).
#[test]
fn tiny_window() {
    let csv: String = std::iter::once("a,b,c\n".to_string()).chain((0..500).map(|i| format!("{i},{},{}\n", i % 7, "w".repeat(i % 50)))).collect();
    let (mut h, dir) = app_with(csv.as_bytes(), "tiny");
    for (w, ht) in [(1200.0, 700.0), (420.0, 260.0), (300.0, 120.0), (160.0, 60.0), (40.0, 30.0)] {
        h.set_size(vec2(w, ht));
        h.run_steps(3);
        for k in 0..12 {
            let p = pos2(w * (k % 4) as f32 / 4.0 + 3.0, ht * (k / 4) as f32 / 3.0 + 3.0);
            h.hover_at(p);
            h.step();
            h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
            h.step();
            h.hover_at(p + vec2(30.0, 40.0));
            h.step();
            h.event(Event::PointerButton { pos: p + vec2(30.0, 40.0), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
            h.step();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A block pasted whose last column is empty adds no column, but the selection was set to
/// cover it: one past the last column, and the fill handle indexed the widths with it.
#[test]
fn paste_with_empty_last_column() {
    use crate::grid::{Action, Cell};
    let (mut h, dir) = app_with(b"a\nx\ny\nz\n", "pastecol");
    let ctx = h.ctx.clone();
    let t = &mut h.state_mut().tabs[0];
    (t.grid.cursor, t.grid.anchor) = (Some(Cell { row: 0, col: 0 }), Some(Cell { row: 0, col: 0 }));
    h.state_mut().apply(vec![Action::Paste("1\t\n2\t".into())], &ctx);
    settle(&mut h);
    h.run_steps(3);
    let t = &h.state().tabs[0];
    let n = t.table.as_ref().unwrap().st.ncols;
    assert!(t.grid.cursor.into_iter().chain(t.grid.anchor).all(|c| c.col < n), "{:?} with {n} columns", t.grid.cursor);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The analysis dialogs (the file map, the profile, the insights) in a window smaller than
/// their padding: they asked egui for rooms of negative height.
#[test]
fn analysis_dialogs_in_a_tiny_window() {
    let csv: String = std::iter::once("a,b,c\n".to_string()).chain((0..300).map(|i| format!("{i},{},x{}\n", i % 7, i % 3))).collect();
    for name in ["Map", "File profile…", "Insights"] {
        let (mut h, dir) = app_with(csv.as_bytes(), &format!("tinydlg{}", name.len()));
        let ctx = h.ctx.clone();
        let c = crate::tools::commands().into_iter().find(|c| c.name == name).unwrap_or_else(|| panic!("no command {name}"));
        h.state_mut().run_command(c.run, &ctx);
        for (w, ht) in [(900.0, 600.0), (400.0, 160.0), (200.0, 60.0), (60.0, 40.0)] {
            h.set_size(vec2(w, ht));
            h.run_steps(4);
            std::thread::sleep(Duration::from_millis(20));
            h.run_steps(2);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The file map in a tall window at a high density: its picture was made at the screen's
/// pixels up to 4096 a side, past what the GPU takes (2048 here).
#[test]
fn file_map_in_a_tall_window() {
    let csv: String = std::iter::once("a,b,c\n".to_string()).chain((0..3000).map(|i| format!("{i},{},x{}\n", i % 7, i % 3))).collect();
    let (mut h, dir) = app_with(csv.as_bytes(), "tallmap");
    h.set_size(vec2(900.0, 1500.0));
    h.set_pixels_per_point(2.0);
    h.run_steps(3);
    let ctx = h.ctx.clone();
    let c = crate::tools::commands().into_iter().find(|c| c.name == "Map").unwrap();
    h.state_mut().run_command(c.run, &ctx);
    for _ in 0..40 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Not the app's: egui 0.37's double click on a word of a single-line field holding Hebrew
/// drawn as tofu (no font here has it) underflows a character index
/// (`text_cursor_state::ccursor_previous_word`, a debug-build panic; release wraps). Kept to
/// see when an egui upgrade fixes it; the monkey types no Hebrew meanwhile.
#[test]
#[ignore]
fn egui_word_select_on_tofu_hebrew() {
    let mut s = "שלום".to_string();
    let mut h = egui_kittest::Harness::builder().allow_missing_glyphs().with_accessibility_check(false).with_size(vec2(300.0, 60.0)).build_ui(move |ui| { ui.add(egui::TextEdit::singleline(&mut s).desired_width(280.0)); });
    h.run_steps(2);
    let p = pos2(100.0, 18.0);
    h.hover_at(p);
    h.step();
    for _ in 0..2 {
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.event(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    }
    h.run_steps(2);
}
