//! What a start can do before the window exists. Creating the window and its GL
//! context takes ~25 ms of the main thread, mostly in the system's GL driver; the
//! app's own start-up work runs beside it instead of after it:
//! - the files given are opened at once (their tabs read them on background threads),
//! - the fonts are found, read and handed to egui, the theme applied, on another thread.
//!
//! All of it goes through the egui context created here and handed to eframe.
//!
//! Tried and dropped: loading the GL driver's libraries (Mesa's pulls in LLVM, ~10 ms) on
//! a thread at start. glibc holds its loader lock through a whole `dlopen`, and creating
//! a thread needs it too: the file and font threads waited for the driver instead.

use crate::app::App;
use crate::config::Config;
use eframe::egui;
use std::path::PathBuf;
use std::thread::JoinHandle;

pub struct Early {
    pub ctx: egui::Context,
    pub app: App,
    /// Fonts, icons and theme being installed in `ctx`.
    style: JoinHandle<()>,
}

impl Early {
    /// The app, once its fonts and theme are in the context (before the first frame).
    pub fn finish(self) -> App {
        let _ = self.style.join();
        self.app
    }
}

/// Starts everything that does not need the window.
/// `config`: the settings, read once by the caller (the window's size came from them).
pub fn start(files: Vec<PathBuf>, config: Config) -> Early {
    let ctx = context();
    crate::app::expect_scale(config.scale);
    // The chrome's icons for the scale the window last had, on their own thread (they need no
    // font): the first frame drawn then finds them ready.
    if let Some(s) = config.scale {
        let _ = std::thread::Builder::new().name("csvfab-icons".into()).spawn(move || crate::icons::rasterize_ahead(s));
    }
    let style = {
        let (ctx, mono) = (ctx.clone(), config.mono_font.clone());
        let theme = config.theme.clone();
        std::thread::Builder::new()
            .name("csvfab-style".into())
            .spawn(move || App::style(&ctx, theme.as_deref(), mono.as_deref()))
            .expect("thread")
    };
    let app = App::build(&ctx, config, files);
    crate::profile::mark("files opening, fonts loading");
    Early { ctx, app, style }
}

/// The context eframe would have made itself (`create_egui_context`, without persistence).
fn context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(cfg!(not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "macos", target_os = "windows"))));
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(2).unwrap());
    ctx
}
