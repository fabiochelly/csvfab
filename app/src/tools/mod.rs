//! Tools: the dialogs and panels built on the engine's features (find &
//! replace, duplicates, pivot, Insights…). Each lives in its own file, owns
//! its state, and talks to the app through `Cx`: the active tab (its table,
//! view and edits) and a list of requests the app carries out after the frame.
//!
//! A tool is opened by a `Command` (main menu, command palette, shortcut) and
//! drawn every frame until `show` returns false. Long work goes through
//! `crate::job::Job`, never on the UI thread.

pub mod fill_example;
pub mod find;
pub mod bulk;
pub mod clean;
pub mod columns;
pub mod convert;
pub mod edit_kit;
pub mod lookup;
pub mod noise;
pub mod rows;
pub mod formula;
pub mod fx_picker;
pub mod import;
pub mod form;
pub mod export;
pub mod split;
pub mod combine;
pub mod anonymize;
pub mod review;
pub mod compare;
pub mod tail;
pub mod analysis_kit;
pub mod dupes;
pub mod profile;
pub mod map;
pub mod schema;
pub mod insights;
pub mod diagnose;
pub mod group;
pub mod pivot;

use crate::tab::Tab;
use crate::theme::{Metrics, Palette};
use eframe::egui;
use std::path::PathBuf;

/// What a tool sees of the app while it is drawn.
pub struct Cx<'a> {
    pub ctx: &'a egui::Context,
    /// The tab the tool was opened on.
    pub tab: &'a mut Tab,
    pub p: &'a Palette,
    pub m: &'a Metrics,
    /// Requests the app carries out after the frame.
    pub out: &'a mut Vec<Cmd>,
}

impl Cx<'_> {
    pub fn toast(&mut self, msg: impl Into<String>) {
        self.out.push(Cmd::Toast(msg.into()));
    }
}

/// What a tool asks of the app.
pub enum Cmd {
    /// Open a file in a new tab (or switch to it).
    Open(PathBuf),
    /// Open a file in a tab behind the active one (it stays active).
    OpenBehind(PathBuf),
    /// A new tab from bytes in memory (a group-by result…): `name` is its title;
    /// it has no file until saved.
    OpenBytes { name: String, bytes: Vec<u8> },
    Toast(String),
    /// Tab `uid`'s file was written by a save of ours: (its stamp as read, the one written, the
    /// sample of what was written: `base::file_sample`).
    Wrote(u64, Option<csvfab_engine::Stamp>, csvfab_engine::Stamp, Option<u64>),
    /// Open another tool on the active tab.
    Tool(Box<dyn Tool>),
}

pub trait Tool {
    /// Draws the tool (usually an `egui::Window` or `egui::Modal` with
    /// `theme::dialog_frame`). Returns false once it is closed.
    fn show(&mut self, cx: &mut Cx) -> bool;

    /// A tool open at most once per tab: opening it again replaces the one open.
    fn single(&self) -> Option<&'static str> {
        None
    }
}

/// An entry of the main menu and the command palette.
pub struct Command {
    pub name: &'static str,
    /// The menu section it belongs to ("File", "Rows", "Columns", "Look", "Transform"…).
    pub group: &'static str,
    /// Shown beside the name, e.g. "Ctrl+H".
    pub keys: Option<&'static str>,
    /// Whether it applies to this tab (most need a loaded table).
    pub enabled: fn(&Tab) -> bool,
    pub run: fn(&mut Cx),
}

/// A table to work on: read whole (while a big file loads, its first rows show and can be
/// edited, but a tool or a save would only see those).
pub fn has_table(t: &Tab) -> bool {
    t.table.is_some() && !t.partial()
}

/// Every tool's commands, in menu order: made once (the menu, the palette and the shortcuts
/// read them every frame).
pub fn commands() -> &'static [Command] {
    static ALL: std::sync::OnceLock<Vec<Command>> = std::sync::OnceLock::new();
    ALL.get_or_init(all_commands)
}

fn all_commands() -> Vec<Command> {
    let mut v = Vec::new();
    // Each tool module adds its own: `v.extend(module::commands());`
    v.extend(find::commands());
    v.extend(bulk::commands());
    v.extend(clean::commands());
    v.extend(noise::commands());
    v.extend(columns::commands());
    v.extend(lookup::commands());
    v.extend(convert::commands());
    v.extend(dupes::commands());
    v.extend(profile::commands());
    v.extend(map::commands());
    v.extend(schema::commands());
    v.extend(insights::commands());
    v.extend(diagnose::commands());
    v.extend(group::commands());
    v.extend(pivot::commands());
    v.extend(formula::commands());
    v.extend(fill_example::commands());
    v.extend(fx_picker::commands());
    v.extend(export::commands());
    v.extend(review::commands());
    v.extend(anonymize::commands());
    v.extend(split::commands());
    v.extend(combine::commands());
    v.extend(compare::commands());
    v.extend(tail::commands());
    v.extend(rows::commands());
    v.extend(builtin());
    v
}

fn builtin() -> Vec<Command> {
    Vec::new()
}

/// A dialog in the app's look: a centred window with the dialog frame, closed by
/// Escape or its ×. Returns the window's inner result and whether it is still open.
pub fn dialog<R>(cx: &mut Cx, title: &str, width: f32, open: &mut bool, add: impl FnOnce(&mut egui::Ui, &mut Cx) -> R) -> Option<R> {
    let frame = crate::theme::dialog_frame(cx.p, cx.m);
    let mut keep = *open;
    let ctx = cx.ctx.clone();
    let r = egui::Window::new(egui::RichText::new(title).font(crate::grid::semibold(16.0)))
        .id(egui::Id::unique(("tool", title)))
        .frame(frame)
        .collapsible(false)
        .resizable(true)
        .default_width(width)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
        .open(&mut keep)
        .show(&ctx, |ui| add(ui, cx));
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && r.as_ref().is_some_and(|r| r.response.contains_pointer() || ctx.memory(|m| m.focused().is_none())) {
        keep = false;
    }
    *open = keep;
    r.and_then(|r| r.inner)
}
