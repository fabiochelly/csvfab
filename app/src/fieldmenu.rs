//! The right-click menu of text fields (53-context-menu.js `fieldMenu`): Undo, Cut, Copy,
//! Paste, Select all, Clear. egui's fields have none: a right click on a field (any of
//! them — found among the widgets under the pointer by its text state) focuses it and
//! opens this menu; each item is sent to the field as the keys or clipboard events it
//! would get from the keyboard, in the next frame's input (`App::raw_input_hook`).

use crate::app::App;
use crate::icons::Icon;
use crate::widgets::{sep, width, Item};
use eframe::egui::{self, Event, Id, Key, Modifiers, Pos2};

pub struct FieldMenu {
    field: Id,
    at: Pos2,
}

/// A right click on a text field: focus it, open the menu where the pointer is. Before the
/// frame's widgets (they would not see a secondary click as theirs).
pub fn detect(app: &mut App, ctx: &egui::Context) {
    if !ctx.input(|i| i.pointer.secondary_pressed()) { return; }
    let Some(at) = ctx.input(|i| i.pointer.interact_pos()) else { return };
    // The ids first: reading a field's state inside the snapshot would lock the context twice.
    let under: Vec<Id> = ctx.interaction_snapshot(|s| s.contains_pointer.iter().copied().collect());
    let field = under.into_iter().find(|&id| egui::TextEdit::load_state(ctx, id).is_some());
    if let Some(field) = field {
        ctx.memory_mut(|m| m.request_focus(field));
        app.field_menu = Some(FieldMenu { field, at });
    }
}

/// The menu, drawn over everything; its choice goes to the field as input events.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(fm) = &app.field_menu else { return };
    let (field, at) = (fm.field, fm.at);
    let p = app.theme.p.clone();
    let has_sel = egui::TextEdit::load_state(ctx, field).and_then(|s| s.cursor.char_range()).is_some_and(|r| r.primary.index != r.secondary.index);
    let mut pick: Option<&str> = None;
    let r = egui::Area::new(Id::unique("field-menu")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        crate::theme::menu_frame(&p, &app.m).show(ui, |ui| {
            width(ui, 200.0);
            for (label, icon, keys, enabled) in [
                ("Undo", Icon::Undo, "Ctrl+Z", true),
                ("Cut", Icon::Cut, "Ctrl+X", has_sel),
                ("Copy", Icon::Copy, "Ctrl+C", has_sel),
                ("Paste", Icon::Paste, "Ctrl+V", true),
            ] {
                if Item::new(label).icon(icon).keys(keys).enabled(enabled).show(ui, &p).clicked() { pick = Some(label); }
            }
            sep(ui, &p);
            if Item::new("Select all").keys("Ctrl+A").show(ui, &p).clicked() { pick = Some("Select all"); }
            if Item::new("Clear").icon(Icon::Cross).show(ui, &p).clicked() { pick = Some("Clear"); }
        });
    });
    let outside = ctx.input(|i| i.pointer.any_pressed()) && !r.response.contains_pointer() && !ctx.input(|i| i.pointer.secondary_pressed());
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    if let Some(what) = pick {
        let key = |k: Key, m: Modifiers| Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m };
        let cmd = Modifiers::COMMAND;
        match what {
            "Undo" => app.inject.push(key(Key::Z, cmd)),
            "Cut" => app.inject.push(Event::Cut),
            "Copy" => app.inject.push(Event::Copy),
            "Paste" => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            "Select all" => app.inject.push(key(Key::A, cmd)),
            _ => app.inject.extend([key(Key::A, cmd), key(Key::Delete, Modifiers::NONE)]),
        }
        // The field keeps the keyboard: what was sent lands in it.
        ctx.memory_mut(|m| m.request_focus(field));
        ctx.request_repaint();
    }
    if pick.is_some() || outside || esc { app.field_menu = None; }
}
