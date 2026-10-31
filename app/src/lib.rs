//! csvfab — native, as a library: the binary (`main.rs`) runs it in a window,
//! the `shot` example renders it off screen. The window and its shell (app,
//! chrome), the grid, the tools; everything heavy runs in the engine crate on
//! background threads.

pub mod about;
pub mod app;
pub mod bidi;
pub mod chrome;
pub mod colpanel;
pub mod config;
pub mod early;
pub mod fieldmenu;
pub mod fontpick;
pub mod fontshop;
pub mod fonts;
pub mod grid;
pub mod icons;
pub mod inspector;
pub mod instance;
pub mod job;
pub mod profile;
pub mod recovery;
pub mod rowcard;
pub mod shot;
pub mod spark_ui;
pub mod syntax;
pub mod tab;
pub mod theme;
pub mod tools;
pub mod totals_ui;
pub mod widgets;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod refresh_ui_tests;
#[cfg(test)]
mod torture_ui_tests;

use eframe::egui;
use std::path::PathBuf;

impl eframe::App for app::App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let drawn = self.frame(ui);
        if profile::on() && ui.ctx().cumulative_frame_nr() < 3 {
            profile::mark(&format!("frame {} at scale {}", ui.ctx().cumulative_frame_nr(), ui.ctx().pixels_per_point()));
        }
        if drawn {
            profile::mark("first frame drawn");
            if self.tabs.get(self.active).is_some_and(|t| t.table.is_some()) { profile::mark("first frame with the grid"); }
        } else {
            profile::mark("first frame left empty (scale not known yet)");
        }
    }

    /// egui's glyph atlas is as wide as the largest texture the GPU takes (16384 px here) and
    /// grows in height by doubling: 2 MB on the heap and as much on the GPU before the first
    /// glyph, uploaded whole on the first frame. 4096 px wide holds the app's glyphs in half
    /// that (it is cleared only past 80% of 4096 × 4096: some 17000 glyphs at once).
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.max_texture_side = raw_input.max_texture_side.map(|s| s.min(4096));
        // A text field's right-click menu speaks to the field as keys would (fieldmenu.rs).
        raw_input.events.append(&mut self.inject);
    }

    /// The theme's background, not eframe's grey: what shows before and around the first frame.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.p.bg.to_normalized_gamma_f32()
    }
}

/// The threads every parallel pass runs on (scans, filters, sorts, statistics), a notch
/// below the UI thread's priority: however heavy a job, the window gets the CPU first, so
/// scrolling and typing stay at the screen's rate while millions of rows are crunched.
fn compute_pool() {
    let _ = rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("csvfab-work-{i}"))
        .start_handler(|_| {
            // SAFETY: on Linux, setpriority(PRIO_PROCESS, 0, …) sets the calling thread's nice value.
            #[cfg(target_os = "linux")]
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 4) };
        })
        .build_global();
}

/// glibc's allocator tuned, first thing in `main`, before any other thread exists: left to
/// itself it raises both thresholds as big blocks are freed, up to 32 MB, and the blocks of a
/// big job then stay in the heap after it. 4 MB and up go back to the system when freed.
pub fn tune_malloc() {
    // SAFETY: mallopt only sets the allocator's parameters; no other thread is using it yet.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 4 << 20);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 4 << 20);
    }
}

/// Opens the window on `files` (or hands them to a window already open).
pub fn run() -> eframe::Result {
    // csvfab --version, --help: answered on the terminal, no window.
    if let Some(a) = std::env::args().nth(1) {
        if a == "--version" || a == "-V" {
            println!("csvfab {}", about::version());
            return Ok(());
        }
        if a == "--help" || a == "-h" {
            println!("csvfab {} — a CSV editor that rewrites the file in place\n\nUsage: csvfab [FILE or FOLDER]…\n\n  Each file opens in a tab (a folder: the tables it holds);\n  with a csvfab window already open, they open in it.\n\n  -V, --version   print the version\n  -h, --help      print this help", about::version());
            return Ok(());
        }
    }
    profile::start();
    recovery::install_hook();
    compute_pool();
    let files: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    // A window is already open: it takes the files (not for screenshots, which need their own).
    if std::env::var_os("CSVFAB_SHOT").is_none() && instance::hand_over(&files) {
        instance::raise();
        return Ok(());
    }
    // As the user left it last time (maximized or not), as the web app's Chromium window
    // remembered; a first start leaves the placement to the window manager.
    let cfg = config::Config::load();
    let maximized = cfg.maximized.unwrap_or(false);
    let size = cfg.size.map_or([1400.0, 900.0], |(w, h)| [w, h]);
    let mut viewport = egui::ViewportBuilder::default().with_title("csvfab").with_inner_size(size).with_maximized(maximized).with_app_id("csvfab").with_drag_and_drop(true);
    // Wayland takes the icon from the desktop entry (app id): no need for eframe to decode its own.
    // Elsewhere (Windows' taskbar, X11) the window carries csvfab's, not eframe's own.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        viewport = viewport.with_icon(egui::IconData::default());
    } else if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/csvfab-256.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let early = early::start(files, cfg);
    profile::mark("eframe starting");
    eframe::run_native_ext("csvfab", options, Some(early.ctx.clone()), Box::new(move |_cc| {
        profile::mark("window and GL ready");
        let app = early.finish();
        profile::mark("app ready (fonts, icons, theme)");
        Ok(Box::new(app))
    }))
}
