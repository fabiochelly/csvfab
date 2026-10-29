//! The real app off screen (egui_kittest), driven by pointer and keys.

use crate::app::App;
use eframe::egui::{self, Event, PointerButton};
use std::time::Duration;

fn app_on(csv: &str, tag: &str) -> (egui_kittest::Harness<'static, App>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("csvfab-ui-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.csv");
    std::fs::write(&path, csv).unwrap();
    let p2 = path.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 700.0)).with_pixels_per_point(1.0).build_eframe(move |cc| App::new(cc, vec![p2]));
    for _ in 0..300 {
        h.step();
        if h.state().tabs.first().is_some_and(|t| t.table.is_some() && !t.busy()) { break; }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.run_steps(10);
    (h, dir)
}

fn values(h: &egui_kittest::Harness<'_, App>) -> String {
    let t = &h.state().tabs[0];
    let st = &t.table.as_ref().unwrap().st;
    (0..t.shown()).map(|p| st.cell(t.shown_id(p), 0)).collect::<Vec<_>>().join(",")
}

/// A row dragged by its number lands where the line showed, as one undo step.
#[test]
fn row_dragged_by_its_number() {
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n3,c\n4,d\n5,e\n", "drag");
    let (gx, rh) = (30.0, 35.0);
    // The first row's middle, as the renders show it at this size.
    let y1 = 230.0;
    h.hover_at(egui::pos2(gx, y1));
    h.run_steps(2);
    h.event(Event::PointerButton { pos: egui::pos2(gx, y1), button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(2);
    for k in 1..=10 {
        h.hover_at(egui::pos2(gx, y1 + k as f32 * rh * 0.25));
        h.run_steps(1);
    }
    // Lower half of the fourth row: after it.
    let y = y1 + 3.0 * rh + rh * 0.3;
    h.hover_at(egui::pos2(gx, y));
    h.run_steps(2);
    h.event(Event::PointerButton { pos: egui::pos2(gx, y), button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(4);
    assert_eq!(values(&h), "2,3,4,1,5");
    assert!(h.state().tabs[0].dirty());
    // Undone from its toast, once: a second Undo would undo another edit.
    let u = h.state().toasts.last().and_then(|t| t.undo).expect("the move's toast offers Undo");
    let ctx = h.ctx.clone();
    h.state_mut().undo_from_toast(u, &ctx);
    h.run_steps(3);
    assert_eq!(values(&h), "1,2,3,4,5");
    assert!(!h.state().tabs[0].dirty());
    h.state_mut().undo_from_toast(u, &ctx);
    h.run_steps(2);
    assert!(h.state().toasts.last().is_some_and(|t| t.text.starts_with("That edit is no longer")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A menu over the sparkline band hides the band from the pointer: no part lit, no tooltip
/// through it.
#[test]
fn band_not_hovered_through_a_menu() {
    let csv: String = std::iter::once("n,t,u\n".to_string()).chain((0..200).map(|i| format!("{},x{},{}\n", i % 37, i % 5, i % 23))).collect();
    let (mut h, dir) = app_on(&csv, "band");
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().tabs[0].spark.lit.is_some() { break; }
        h.hover_at(egui::pos2(150.0, 145.0));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().tabs[0].spark.lit.is_some(), "the band is lit when nothing covers it");
    h.hover_at(egui::pos2(380.0, 145.0));
    h.run_steps(3);
    assert!(h.state().tabs[0].spark.lit.is_some_and(|l| l.0 == 2), "the third column's band is lit");
    // The columns manager's button, top of the gutter: a click opens its menu.
    let at = egui::pos2(45.0, 105.0);
    h.hover_at(at);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "the columns menu opened");
    h.hover_at(egui::pos2(150.0, 146.0));
    h.run_steps(6);
    assert!(h.state().tabs[0].spark.lit.is_none(), "lit through the columns menu");
    // Beside the menu, on the band still in view: nothing either while it is open.
    h.hover_at(egui::pos2(380.0, 146.0));
    h.run_steps(4);
    assert!(h.state().tabs[0].spark.lit.is_none(), "lit beside the columns menu");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The selected column deleted, the last one or the only one: the selection follows, the grid
/// still draws (it panicked drawing the fill handle past the last column), and Ctrl+Z brings it back.
#[test]
fn selected_column_deleted() {
    use crate::grid::{Action, Cell};
    for (csv, col, left) in [("a,b,c\n1,2,3\n4,5,6\n", 2, 2), ("only\nx\ny\n", 0, 0)] {
        let (mut h, dir) = app_on(csv, &format!("delcol{col}"));
        let t = &mut h.state_mut().tabs[0];
        (t.grid.cursor, t.grid.anchor) = (Some(Cell { row: 1, col }), Some(Cell { row: 0, col }));
        h.run_steps(2);
        let ctx = h.ctx.clone();
        h.state_mut().apply(vec![Action::Sure(Box::new(Action::DeleteColumns(vec![col])))], &ctx);
        h.run_steps(4);
        let t = &h.state().tabs[0];
        assert_eq!(t.table.as_ref().unwrap().st.ncols, left);
        assert!(t.grid.cursor.into_iter().chain(t.grid.anchor).all(|c| c.col < left));
        let t = &mut h.state_mut().tabs[0];
        t.table.as_mut().unwrap().undo();
        t.after_edit(true, &ctx);
        h.run_steps(4);
        assert_eq!(h.state().tabs[0].table.as_ref().unwrap().st.ncols, left + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Another program rewrites the open file: told on coming back to the window, Reload
/// reads the new version. Then it cuts the file short under the grid: no crash, told,
/// and no save writes the zeros read in place of what was lost.
#[test]
fn file_changed_or_cut_by_another_program() {
    use crate::app::{Dialog, OnDisk};
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n3,c\n", "disk");
    let path = dir.join("t.csv");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, "n,v\n1,a\n2,b\n3,c\n4,d\n").unwrap();
    h.event(Event::WindowFocused(true));
    h.run_steps(2);
    assert!(matches!(h.state().dialog, Some(Dialog::OnDisk { what: OnDisk::Changed, .. })), "told the file changed");
    h.state_mut().dialog = None;
    // Not told again for the same change.
    h.event(Event::WindowFocused(true));
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
    let ctx = h.ctx.clone();
    h.state_mut().reload_tab(0, &ctx);
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().tabs[0].table.as_ref().is_some_and(|_| !h.state().tabs[0].busy()) && values(&h).split(',').count() == 4 { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(values(&h), "1,2,3,4");
    // Big enough for the rows shown to lie past the first page: cut to nothing under them.
    let body: String = std::iter::once("n,v\n".to_string()).chain((0..20_000).map(|i| format!("{i},row {i}\n"))).collect();
    std::fs::write(&path, &body).unwrap();
    h.state_mut().reload_tab(0, &ctx);
    for _ in 0..300 {
        h.run_steps(1);
        if h.state().tabs[0].table.is_some() && !h.state().tabs[0].busy() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.state_mut().dialog = None;
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(0).unwrap();
    // The grid reads its rows again (a scroll): zeros past the end, no crash.
    h.state_mut().tabs[0].grid.set_top(5000);
    h.event(Event::WindowFocused(true));
    h.run_steps(4);
    assert!(h.state().tabs[0].st().unwrap().base.cut_short());
    assert!(matches!(h.state().dialog, Some(Dialog::OnDisk { what: OnDisk::Cut, .. })), "told the file was cut short");
    h.state_mut().dialog = None;
    h.state_mut().save(None, true, &ctx);
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().dialog.is_some() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(&h.state().dialog, Some(Dialog::Message(m)) if m.contains("cut the file short")), "save refused");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0, "nothing written");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Our own saves are not "changed on disk", also when an edit came during the save (the
/// table then still stands on the read before it).
#[test]
fn own_save_not_told_as_changed() {
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n", "own");
    let ctx = h.ctx.clone();
    let edit = |h: &mut egui_kittest::Harness<'static, App>, v: &str| {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(0);
        t.table.as_mut().unwrap().set_cells(&[(id, 1, v.to_string())]);
        t.after_edit(false, &ctx);
    };
    edit(&mut h, "x");
    h.state_mut().save(None, false, &ctx);
    // An edit while the save runs: the table keeps its read, the file is the save's.
    edit(&mut h, "y");
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().saving() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    h.event(Event::WindowFocused(true));
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "told about our own save");
    // And the next save goes through without asking.
    h.state_mut().save(None, false, &ctx);
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().saving() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    assert!(h.state().dialog.is_none(), "the second save asked");
    assert_eq!(std::fs::read_to_string(dir.join("t.csv")).unwrap(), "n,v\ny,a\n2,b\n".replace("y,a", "1,y"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unsaved edits are copied aside a few seconds after the last one; a run that stopped
/// without asking leaves the copy, offered back at the next start; a save removes it.
#[test]
fn unsaved_edits_kept_aside() {
    use crate::app::Dialog;
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n", "recov");
    let rdir = dir.join("recovery");
    h.state_mut().recovery = crate::recovery::Recovery::at(rdir.clone());
    let ctx = h.ctx.clone();
    {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(1);
        t.table.as_mut().unwrap().set_cells(&[(id, 1, "edited".into())]);
        t.after_edit(false, &ctx);
    }
    // Not at once: after the quiet time (the harness's clock runs a frame at a time).
    h.run_steps(5);
    assert!(crate::recovery::left_in(&rdir).is_empty());
    let copy = || std::fs::read_dir(&rdir).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "csv"));
    for _ in 0..600 {
        h.run_steps(5);
        if copy().is_some_and(|p| p.with_extension("meta").exists()) { break; }
        std::thread::sleep(Duration::from_millis(2));
    }
    let data = copy().expect("copy written");
    assert_eq!(std::fs::read_to_string(&data).unwrap(), "n,v\n1,a\n2,edited\n");
    // As if left by a run no longer there: offered at start.
    let stem = data.file_stem().unwrap().to_str().unwrap().to_string();
    let other = stem.replacen(&std::process::id().to_string(), "999999999", 1);
    std::fs::rename(&data, rdir.join(format!("{other}.csv"))).unwrap();
    std::fs::rename(data.with_extension("meta"), rdir.join(format!("{other}.meta"))).unwrap();
    let left = crate::recovery::left_in(&rdir);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].name, "t.csv");
    assert_eq!(left[0].original.as_deref(), Some(dir.join("t.csv").as_path()));
    h.state_mut().recover_checked = false;
    h.run_steps(2);
    assert!(matches!(&h.state().dialog, Some(Dialog::Recover(l)) if l.len() == 1));
    h.state_mut().dialog = None;
    crate::recovery::remove(&left[0]);
    // Another edit: copied again; then saved, the copy goes.
    {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(0);
        t.table.as_mut().unwrap().set_cells(&[(id, 1, "again".into())]);
        t.after_edit(false, &ctx);
    }
    for _ in 0..600 {
        h.run_steps(5);
        if copy().is_some() { break; }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(copy().is_some());
    h.state_mut().save(None, false, &ctx);
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().saving() && !h.state().tabs[0].dirty() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    assert!(copy().is_none(), "the copy stays after the save");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Renaming a column of a file without a title line adds one: the first data line stays
/// data (it used to become the titles).
#[test]
fn rename_without_titles_keeps_the_data() {
    let (mut h, dir) = app_on("1,2\n3,4\n5,6\n", "rename");
    assert!(!h.state().tabs[0].st().unwrap().header);
    let ctx = h.ctx.clone();
    h.state_mut().tabs[0].rename_column(1, "amount".into(), &ctx);
    h.run_steps(3);
    let st = h.state().tabs[0].st().unwrap().clone();
    assert!(st.header);
    assert_eq!(st.headers(), vec!["Column 1", "amount"]);
    assert_eq!(values(&h), "1,3,5");
    let mut b = true;
    let out = dir.join("out.csv");
    csvfab_engine::save::save(&st, &out, &Default::default(), None, &mut b).unwrap();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "Column 1,amount\n1,2\n3,4\n5,6\n");
    // One undo step: back to the file as it was.
    let t = &mut h.state_mut().tabs[0];
    t.table.as_mut().unwrap().undo();
    t.after_edit(true, &ctx);
    assert!(!h.state().tabs[0].st().unwrap().header);
    assert_eq!(values(&h), "1,3,5");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ctrl+G goes to a row by its number in the file, whatever the sort; hidden by the
/// filters, it asks to clear them, then goes there; past the end, it says so.
#[test]
fn go_to_row_by_its_number() {
    use crate::app::Dialog;
    use crate::grid::Action;
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n3,c\n4,d\n5,e\n", "goto");
    let ctx = h.ctx.clone();
    let wait = |h: &mut egui_kittest::Harness<'static, App>| for _ in 0..200 {
        h.run_steps(1);
        if !h.state().tabs[0].busy() { break; }
        std::thread::sleep(Duration::from_millis(5));
    };
    // Sorted descending: row 2 of the file is shown 4th.
    h.state_mut().tabs[0].sort = vec![csvfab_engine::view::SortKey { col: 0, desc: true }];
    h.state_mut().tabs[0].refresh(&ctx);
    wait(&mut h);
    h.state_mut().go_to_number(2, &ctx);
    assert_eq!(h.state().tabs[0].grid.cursor.map(|c| c.row), Some(3));
    // Filtered out: asked, then shown once the filters are cleared.
    h.state_mut().tabs[0].filter.global = "e".into();
    h.state_mut().tabs[0].refresh(&ctx);
    wait(&mut h);
    h.state_mut().go_to_number(2, &ctx);
    let then = match h.state_mut().dialog.take() { Some(Dialog::Confirm { then, .. }) => then, _ => panic!("not asked") };
    assert!(matches!(then, Some(Action::GoToRow { clear: true, .. })));
    h.state_mut().apply(vec![Action::Sure(Box::new(then.unwrap()))], &ctx);
    wait(&mut h);
    h.run_steps(2);
    let t = &h.state().tabs[0];
    assert!(t.filter.global.is_empty());
    assert_eq!(t.grid.cursor.map(|c| t.shown_id(c.row)), Some(2));
    // Past the end.
    h.state_mut().go_to_number(9, &ctx);
    assert!(h.state().toasts.last().is_some_and(|t| t.text.starts_with("No row 9")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A click on a title sorts the view only; "Write the sorted order to the file" makes it
/// the rows' order — the rows a filter hides too — as one undo step, and the sort is off.
#[test]
fn sorted_order_written() {
    use crate::grid::Action;
    let (mut h, dir) = app_on("n,v\n2,b\n5,e\n1,a\n4,d\n3,c\n", "wsort");
    let ctx = h.ctx.clone();
    let wait = |h: &mut egui_kittest::Harness<'static, App>| for _ in 0..200 {
        h.run_steps(1);
        if !h.state().tabs[0].busy() && h.state().tools.is_empty() { break; }
        std::thread::sleep(Duration::from_millis(5));
    };
    h.state_mut().tabs[0].sort = vec![csvfab_engine::view::SortKey { col: 0, desc: false }];
    h.state_mut().tabs[0].filter.global = "e".into();
    h.state_mut().tabs[0].refresh(&ctx);
    wait(&mut h);
    assert!(!h.state().tabs[0].dirty());
    h.state_mut().apply(vec![Action::RunCommand("Write the sorted order to the file")], &ctx);
    wait(&mut h);
    h.run_steps(2);
    let t = &h.state().tabs[0];
    assert!(t.sort.is_empty() && t.dirty());
    assert_eq!(values(&h), "5");
    h.state_mut().tabs[0].filter.global.clear();
    h.state_mut().tabs[0].refresh(&ctx);
    wait(&mut h);
    assert_eq!(values(&h), "1,2,3,4,5");
    let toast = h.state().toasts.last().unwrap();
    assert!(toast.text.contains("rows sorted by \"n\""), "{}", toast.text);
    let u = toast.undo.expect("the toast offers Undo");
    h.state_mut().undo_from_toast(u, &ctx);
    wait(&mut h);
    assert_eq!(values(&h), "2,5,1,4,3");
    assert!(!h.state().tabs[0].dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A right click on a text field opens its menu and focuses it; the menu's choices reach
/// the field as keys (here "Clear": select all, delete).
#[test]
fn text_field_menu() {
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n", "fieldmenu");
    h.state_mut().tabs[0].filter.global = "abc".into();
    h.run_steps(2);
    let at = egui::pos2(230.0, 63.0);
    h.hover_at(at);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
    assert!(h.state().field_menu.is_some(), "the menu opened");
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "the field has the keyboard");
    // As the menu's "Clear" does: its events in the next frame's input.
    let cmd = egui::Modifiers::COMMAND;
    for (k, m) in [(egui::Key::A, cmd), (egui::Key::Delete, egui::Modifiers::NONE)] {
        h.event(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m });
    }
    h.run_steps(3);
    assert_eq!(h.state().tabs[0].filter.global, "");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A title being renamed is a field over it (what a double click on it opens): its text all
/// selected, typing replaces it, Enter writes the new name as one undo step.
#[test]
fn rename_in_the_title() {
    let (mut h, dir) = app_on("name,city\na,Lyon\nb,Paris\n", "rename-title");
    h.state_mut().tabs[0].grid.renaming = Some((0, "name".into(), false));
    h.run_steps(3);
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "the field has the keyboard");
    h.event(Event::Text("town".into()));
    h.run_steps(1);
    h.event(Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(4);
    let t = &h.state().tabs[0];
    assert!(t.grid.renaming.is_none());
    assert_eq!(t.st().unwrap().headers(), vec!["town", "city"]);
    // Escape drops what was typed.
    h.state_mut().tabs[0].grid.renaming = Some((1, "city".into(), false));
    h.run_steps(3);
    h.event(Event::Text("x".into()));
    h.run_steps(1);
    h.event(Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(h.state().tabs[0].grid.renaming.is_none());
    assert_eq!(h.state().tabs[0].st().unwrap().headers(), vec!["town", "city"]);
    let _ = std::fs::remove_dir_all(&dir);
}
