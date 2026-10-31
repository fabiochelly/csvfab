//! The real app off screen (egui_kittest), driven by pointer and keys.

use crate::app::App;
use eframe::egui::{self, Event, PointerButton};
use std::time::Duration;

fn app_on(csv: impl AsRef<[u8]>, tag: &str) -> (egui_kittest::Harness<'static, App>, std::path::PathBuf) {
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

/// A value filter and a hidden column follow their column when one is inserted before them;
/// Undo puts them back where they were.
#[test]
fn column_settings_follow_inserts_and_undo() {
    use crate::grid::Action;
    let (mut h, dir) = app_on("a,b\n1,x\n2,y\n3,x\n", "colview");
    let ctx = h.ctx.clone();
    let wait = |h: &mut egui_kittest::Harness<'static, App>| for _ in 0..200 {
        h.run_steps(1);
        if !h.state().tabs[0].busy() { break; }
        std::thread::sleep(Duration::from_millis(5));
    };
    {
        let t = &mut h.state_mut().tabs[0];
        t.filter.hidden = vec![(1, std::sync::Arc::new(["x".to_string()].into_iter().collect()))];
        t.grid.hidden.insert(1);
        t.refresh(&ctx);
    }
    wait(&mut h);
    assert_eq!(values(&h), "2");
    h.state_mut().apply(vec![Action::Sure(Box::new(Action::InsertColumn(0)))], &ctx);
    wait(&mut h);
    let t = &h.state().tabs[0];
    assert_eq!(t.st().unwrap().ncols, 3);
    assert_eq!(t.filter.hidden.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2]);
    assert!(t.grid.hidden.contains(&2) && !t.grid.hidden.contains(&1));
    assert_eq!(t.shown(), 1);
    h.state_mut().undo(false, &ctx);
    wait(&mut h);
    let t = &h.state().tabs[0];
    assert_eq!(t.st().unwrap().ncols, 2);
    assert_eq!(t.filter.hidden.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1]);
    assert!(t.grid.hidden.contains(&1) && t.grid.hidden.len() == 1);
    assert_eq!(values(&h), "2");
    h.state_mut().undo(true, &ctx);
    wait(&mut h);
    assert!(h.state().tabs[0].grid.hidden.contains(&2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Dragged over the row numbers from a row not selected yet: the rows are selected, nothing moves.
#[test]
fn rows_selected_by_dragging_their_numbers() {
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n3,c\n4,d\n5,e\n", "dragsel");
    let (gx, rh, y1) = (30.0, 32.0, 159.0);
    h.hover_at(egui::pos2(gx, y1));
    h.run_steps(2);
    h.event(Event::PointerButton { pos: egui::pos2(gx, y1), button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(2);
    for k in 1..=8 {
        h.hover_at(egui::pos2(gx, y1 + k as f32 * rh * 0.25));
        h.run_steps(1);
    }
    let y = y1 + 2.0 * rh;
    h.event(Event::PointerButton { pos: egui::pos2(gx, y), button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert_eq!(values(&h), "1,2,3,4,5");
    assert!(!h.state().tabs[0].dirty());
    let (r, cs) = h.state().tabs[0].grid.range().expect("a selection");
    assert_eq!((*r.start(), *r.end(), *cs.start(), *cs.end()), (0, 2, 0, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A row dragged by its number lands where the line showed, as one undo step.
#[test]
fn row_dragged_by_its_number() {
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n3,c\n4,d\n5,e\n", "drag");
    let (gx, rh) = (30.0, 32.0);
    // The first row's middle, as the renders show it at this size.
    let y1 = 159.0;
    h.hover_at(egui::pos2(gx, y1));
    h.run_steps(2);
    // Selected first (a click on its number): a drag then moves it, as in Excel.
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: egui::pos2(gx, y1), button: PointerButton::Primary, pressed, modifiers: Default::default() });
        h.run_steps(2);
    }
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
        h.hover_at(egui::pos2(150.0, 95.0));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().tabs[0].spark.lit.is_some(), "the band is lit when nothing covers it");
    h.hover_at(egui::pos2(380.0, 95.0));
    h.run_steps(3);
    assert!(h.state().tabs[0].spark.lit.is_some_and(|l| l.0 == 2), "the third column's band is lit");
    // The columns manager's button, top of the gutter: a click opens its menu.
    let at = egui::pos2(45.0, 62.0);
    h.hover_at(at);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "the columns menu opened");
    h.hover_at(egui::pos2(150.0, 96.0));
    h.run_steps(6);
    assert!(h.state().tabs[0].spark.lit.is_none(), "lit through the columns menu");
    // Beside the menu, on the band still in view: nothing either while it is open.
    h.hover_at(egui::pos2(380.0, 96.0));
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

/// Unsaved edits are kept aside a few seconds after the last one — a journal of them while
/// the file on disk is the one read, a copy of the table once it changed; a run that stopped
/// without asking leaves them, offered back at the next start (the journal replayed over the
/// file, refused if the file changed since); a save removes them.
#[test]
fn unsaved_edits_kept_aside() {
    use crate::app::Dialog;
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n", "recov");
    let rdir = dir.join("recovery");
    h.state_mut().recovery = crate::recovery::Recovery::at(rdir.clone());
    let ctx = h.ctx.clone();
    let edit = |h: &mut egui_kittest::Harness<'static, App>, row: usize, v: &str| {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(row);
        t.table.as_mut().unwrap().set_cells(&[(id, 1, v.into())]);
        t.after_edit(false, &ctx);
    };
    let kept = |ext: &str| std::fs::read_dir(&rdir).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == ext) && p.with_extension("meta").exists());
    let wait = |h: &mut egui_kittest::Harness<'static, App>, ext: &str| {
        for _ in 0..600 {
            h.run_steps(5);
            if kept(ext).is_some() { break; }
            std::thread::sleep(Duration::from_millis(2));
        }
        kept(ext).unwrap_or_else(|| panic!("no .{ext} kept"))
    };
    edit(&mut h, 1, "edited");
    // Not at once: after the quiet time (the harness's clock runs a frame at a time).
    h.run_steps(5);
    assert!(crate::recovery::left_in(&rdir).is_empty());
    let data = wait(&mut h, "journal");
    assert!(kept("csv").is_none(), "a journal, not a copy, for a file unchanged on disk");
    // As if left by a run no longer there: offered at start.
    let stem = data.file_stem().unwrap().to_str().unwrap().to_string();
    let other = stem.replacen(&std::process::id().to_string(), "999999999", 1);
    std::fs::rename(&data, rdir.join(format!("{other}.journal"))).unwrap();
    std::fs::rename(data.with_extension("meta"), rdir.join(format!("{other}.meta"))).unwrap();
    let left = crate::recovery::left_in(&rdir);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].name, "t.csv");
    assert_eq!(left[0].original.as_deref(), Some(dir.join("t.csv").as_path()));
    h.state_mut().recover_checked = false;
    h.run_steps(2);
    assert!(matches!(&h.state().dialog, Some(Dialog::Recover(l)) if l.len() == 1));
    h.state_mut().dialog = None;
    // Restored: the file read, the edits replayed over it, unsaved.
    let bytes = std::fs::read(&left[0].data).unwrap();
    let mut t = crate::recovery::restore(&left[0], bytes.clone(), &ctx, &h.state().m).unwrap();
    for _ in 0..300 {
        t.poll(&ctx, &h.state().m);
        if t.table.is_some() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let st = t.st().unwrap();
    assert_eq!((st.cell(st.data_id(1), 1), t.dirty(), t.path.as_deref()), ("edited".into(), true, Some(dir.join("t.csv").as_path())));
    crate::recovery::remove(&left[0]);
    // Another edit: kept again; then saved, it goes.
    edit(&mut h, 0, "again");
    wait(&mut h, "journal");
    h.state_mut().save(None, false, &ctx);
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().saving() && !h.state().tabs[0].dirty() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    assert!(kept("journal").is_none() && kept("csv").is_none(), "kept after the save");
    // The file changed on disk under the table (another program): a journal could not be
    // replayed on it, the whole table is copied instead; the old journal is refused.
    let path = dir.join("t.csv");
    std::fs::write(dir.join("other.csv"), "n,v\nx,y\n").unwrap();
    std::fs::rename(dir.join("other.csv"), &path).unwrap();
    edit(&mut h, 1, "late");
    let copy = wait(&mut h, "csv");
    assert!(kept("journal").is_none());
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), "n,v\n1,again\n2,late\n");
    let e = crate::recovery::restore(&left[0], bytes, &ctx, &h.state().m).err().unwrap();
    assert!(e.contains("changed on disk"), "{e}");
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
    let at = egui::pos2(800.0, 23.0);
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

/// The grid's body (the biggest widget that senses clicks and drags), clicked near its top
/// left: the grid has the keyboard, the cell there is selected.
fn click_grid(h: &mut egui_kittest::Harness<'static, App>) {
    let body = h.ctx.viewport(|v| v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.sense.senses_click() && w.sense.senses_drag()).map(|w| w.rect).max_by(|a, b| a.area().total_cmp(&b.area()))).unwrap();
    let at = body.min + egui::vec2(body.width() * 0.3, 12.0);
    h.hover_at(at);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

fn key(h: &mut egui_kittest::Harness<'static, App>, key: egui::Key, modifiers: egui::Modifiers) {
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
}

/// Ctrl+A then Delete on more cells than are cleared at once: the patch comes from a job, the
/// cells that held something are emptied (and counted), the empty ones stay as read; one
/// undo step brings it all back.
#[test]
fn big_selection_cleared_in_a_job() {
    let mut csv = String::from("a,b,c\n");
    for i in 0..30_000 { csv += &format!("{i},{},z\n", if i % 3 == 0 { String::new() } else { format!("b{i}") }); }
    let (mut h, dir) = app_on(&csv, "bigclear");
    click_grid(&mut h);
    key(&mut h, egui::Key::A, egui::Modifiers::COMMAND);
    h.run_steps(1);
    key(&mut h, egui::Key::Delete, egui::Modifiers::NONE);
    h.run_steps(1);
    assert!(!h.state().tabs[0].grid.idle(), "a selection this big is cleared in a job");
    for _ in 0..500 {
        h.run_steps(1);
        if h.state().tabs[0].grid.idle() && !h.state().tabs[0].busy() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let t = &h.state().tabs[0];
    let st = t.st().unwrap();
    assert!((0..st.len()).all(|p| (0..3).all(|c| st.cell(st.data_id(p), c).is_empty())));
    assert_eq!(st.headers(), vec!["a", "b", "c"]);
    assert_eq!(t.table.as_ref().unwrap().undo_depth(), 1);
    let filled = 30_000 * 2 + 20_000;
    assert!(h.state().toasts.iter().any(|x| x.text == format!("{} cells cleared", crate::grid::fmt_int(filled))), "{:?}", h.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
    let ctx = h.ctx.clone();
    h.state_mut().undo(false, &ctx);
    h.run_steps(2);
    let st = h.state().tabs[0].st().unwrap().clone();
    assert_eq!((st.cell(st.data_id(4), 1), st.cell(st.data_id(3), 1), st.cell(st.data_id(29_999), 2)), ("b4".into(), String::new(), "z".into()));
    assert!(!h.state().tabs[0].dirty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cell typed under the totals row: its sum follows at once, without a pass over the rows;
/// the other columns' totals are not counted again either.
#[test]
fn totals_follow_a_typed_cell() {
    use crate::grid::Action;
    let mut csv = String::from("id,amount,qty\n");
    for i in 0..2000 { csv += &format!("{i},{}.50,{}\n", i % 10, i % 7); }
    let (mut h, dir) = app_on(&csv, "totals-edit");
    h.state_mut().tabs[0].show_totals = true;
    for _ in 0..400 {
        h.run_steps(1);
        if !crate::totals_ui::pending(&h.state().tabs[0]) { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let sum = |h: &egui_kittest::Harness<'static, App>, c: usize| crate::totals_ui::value(&h.state().tabs[0], c);
    let before = sum(&h, 1).expect("a sum under amount");
    assert_eq!(before, crate::totals_ui::value(&h.state().tabs[0], 1).unwrap());
    let ctx = h.ctx.clone();
    let id = h.state().tabs[0].shown_id(3);
    h.state_mut().apply(vec![Action::SetCells(vec![(id, 1, "1003.75".into())])], &ctx);
    // Right away: no pass to wait for.
    assert!(!crate::totals_ui::pending(&h.state().tabs[0]));
    let want: f64 = (0..2000).map(|i| if i == 3 { 1003.75 } else { (i % 10) as f64 + 0.5 }).sum();
    assert_eq!(sum(&h, 1).unwrap(), csvfab_engine::stats::fmt_num(want, 2, 2, true));
    // And what a count over the rows gives, once one is made.
    h.state_mut().tabs[0].totals.set_choices(Default::default());
    for _ in 0..400 {
        h.run_steps(1);
        if !crate::totals_ui::pending(&h.state().tabs[0]) { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(sum(&h, 1).unwrap(), csvfab_engine::stats::fmt_num(want, 2, 2, true));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A column moved: its kind and reading aids go with it, the rows shown stay (no recount of
/// the view, the sort kept on its column).
#[test]
fn moved_column_keeps_its_analysis() {
    use csvfab_engine::view::{Kind, SortKey};
    let (mut h, dir) = app_on("n,mail,when\n3,a@b.fr,2024-01-02\n1,c@d.fr,2024-02-03\n2,e@f.fr,2024-03-04\n", "movecol");
    let ctx = h.ctx.clone();
    {
        let t = &mut h.state_mut().tabs[0];
        t.sort = vec![SortKey { col: 0, desc: false }];
        t.refresh(&ctx);
    }
    for _ in 0..200 { h.run_steps(1); if !h.state().tabs[0].busy() { break; } std::thread::sleep(Duration::from_millis(5)); }
    let (kinds, rich) = (h.state().tabs[0].kinds.clone(), h.state().tabs[0].rich.clone());
    assert_eq!(kinds, vec![Kind::Number, Kind::Text, Kind::Date]);
    let view = h.state().tabs[0].view.clone().unwrap();
    crate::grid::move_column(&mut h.state_mut().tabs[0], 0, 2, &ctx);
    let t = &h.state().tabs[0];
    assert!(!t.busy(), "no view to compute again");
    assert!(std::sync::Arc::ptr_eq(t.view.as_ref().unwrap(), &view));
    assert_eq!(t.kinds, vec![Kind::Text, Kind::Date, Kind::Number]);
    assert_eq!(t.rich, vec![rich[1].clone(), rich[2].clone(), rich[0].clone()]);
    assert_eq!(t.sort, vec![SortKey { col: 2, desc: false }]);
    let st = h.state().tabs[0].st().unwrap().clone();
    let t = &h.state().tabs[0];
    assert_eq!((0..3).map(|p| st.cell(t.shown_id(p), 2)).collect::<Vec<_>>(), ["1", "2", "3"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A save in place of rows that keep their place: the table now stands on the saved file and
/// the rows shown are kept as they were (not filtered and sorted again); the cells are the
/// file's. Then a sort and a filter on it see the saved values.
#[test]
fn save_keeps_the_view() {
    let mut csv = String::from("city,n\n");
    for i in 0..400 { csv.push_str(&format!("{},{}\n", ["Lyon", "Paris", "Lyonnais"][i % 3], (i * 37) % 101)); }
    let (mut h, dir) = app_on(&csv, "keepview");
    let ctx = h.ctx.clone();
    let wait = |h: &mut egui_kittest::Harness<'static, App>| for _ in 0..400 {
        h.run_steps(1);
        if !h.state().tabs[0].busy() && !h.state().saving() { break; }
        std::thread::sleep(Duration::from_millis(5));
    };
    {
        let t = &mut h.state_mut().tabs[0];
        t.filter.global = "lyon".into();
        t.sort = vec![csvfab_engine::view::SortKey { col: 1, desc: true }];
        t.refresh(&ctx);
    }
    wait(&mut h);
    {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(3);
        t.table.as_mut().unwrap().set_cells(&[(id, 0, "Lyon-Est".into())]);
        t.after_edit(false, &ctx);
    }
    wait(&mut h);
    let before = h.state().tabs[0].view.clone().unwrap();
    let base = h.state().tabs[0].st().unwrap().base.clone();
    h.state_mut().save(None, false, &ctx);
    wait(&mut h);
    h.run_steps(3);
    let t = &h.state().tabs[0];
    assert!(!t.dirty() && !std::sync::Arc::ptr_eq(&t.st().unwrap().base, &base), "the saved file adopted");
    assert!(std::sync::Arc::ptr_eq(t.view.as_ref().unwrap(), &before), "the view kept, not computed again");
    let st = t.st().unwrap();
    assert_eq!(st.cell(t.shown_id(3), 0), "Lyon-Est");
    assert!(st.cmap.is_none() && st.cols.iter().all(|l| l.is_empty()));
    // A filter typed after it: computed on the saved file.
    h.state_mut().tabs[0].filter.global = "lyon-est".into();
    h.state_mut().tabs[0].refresh(&ctx);
    wait(&mut h);
    assert_eq!(h.state().tabs[0].shown(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Waits until the active tab's jobs and the save are done.
fn settle(h: &mut egui_kittest::Harness<'static, App>) {
    for _ in 0..400 {
        h.run_steps(1);
        if !h.state().tabs[0].busy() && !h.state().saving() && h.state().tabs[0].dups_job.is_none() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

/// After a save that renumbered the rows (one deleted above), the marks and the rows a tool
/// showed are still those rows, not the ones now at their old ids.
#[test]
fn marks_follow_their_rows_through_a_save() {
    use crate::grid::Action;
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n3,c\n4,d\n", "renumber");
    let ctx = h.ctx.clone();
    let id3 = h.state().tabs[0].shown_id(2);
    {
        let t = &mut h.state_mut().tabs[0];
        let mut m = crate::tab::Marks::default();
        m.rows.insert(id3);
        m.cells.insert((id3, 1), "was c".into());
        t.marks = std::sync::Arc::new(m);
        t.restrict_to("marked rows", vec![id3], &ctx);
    }
    settle(&mut h);
    assert_eq!(values(&h), "3");
    // The first row deleted (outside the restriction: by its id), then saved.
    let id1 = h.state().tabs[0].st().unwrap().data_id(0);
    h.state_mut().apply(vec![Action::Sure(Box::new(Action::DeleteRows(vec![id1])))], &ctx);
    h.state_mut().save(None, false, &ctx);
    settle(&mut h);
    let t = &h.state().tabs[0];
    assert!(!t.dirty());
    assert_eq!(std::fs::read_to_string(dir.join("t.csv")).unwrap(), "n,v\n2,b\n3,c\n4,d\n");
    assert_eq!(values(&h), "3", "the restriction followed its row");
    let st = t.st().unwrap();
    let marked: Vec<String> = t.marks.rows.iter().map(|&id| st.cell(id, 0)).collect();
    assert_eq!(marked, ["3"]);
    assert!(t.marks.cells.keys().all(|&(id, _)| st.cell(id, 0) == "3"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sync client writing our file back (same bytes, another inode and time) is not "changed on
/// disk"; another program's edit of the same size is.
#[test]
fn same_content_rewritten_is_not_a_change() {
    use crate::app::{Dialog, OnDisk};
    let (mut h, dir) = app_on("n,v\n1,a\n2,b\n", "sync");
    let path = dir.join("t.csv");
    let tmp = dir.join("t.tmp");
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&tmp, "n,v\n1,a\n2,b\n").unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    for _ in 0..20 {
        h.event(Event::WindowFocused(true));
        h.run_steps(2);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state().dialog.is_none(), "our own content told as changed");
    std::fs::write(&tmp, "n,v\n1,a\n2,X\n").unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    for _ in 0..40 {
        h.event(Event::WindowFocused(true));
        h.run_steps(2);
        if h.state().dialog.is_some() { break; }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(matches!(h.state().dialog, Some(Dialog::OnDisk { what: OnDisk::Changed, .. })), "another program's edit of the same size");
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Only the rows with …" is an exact value filter (case and spaces count), kept apart from
/// the column's box, and Invert leaves it alone.
#[test]
fn only_rows_with_a_value_is_exact() {
    use crate::grid::Action;
    let (mut h, dir) = app_on("n,city\n1,Paris\n2,paris\n3,Paris \n4,Lyon\n", "onlyval");
    let ctx = h.ctx.clone();
    h.state_mut().tabs[0].col_filters[0] = "!4".into();
    h.state_mut().apply(vec![Action::FilterValue { col: 1, value: "Paris".into(), keep: true }], &ctx);
    settle(&mut h);
    assert_eq!(values(&h), "1");
    assert_eq!(h.state().tabs[0].col_filters[1], "", "the column's box is left as typed");
    h.state_mut().tabs[0].filter.invert = true;
    h.state_mut().tabs[0].refresh(&ctx);
    settle(&mut h);
    assert_eq!(values(&h), "", "Invert flips the box (!4), not the value kept");
    h.state_mut().tabs[0].filter.invert = false;
    h.state_mut().tabs[0].col_filters[0].clear();
    h.state_mut().apply(vec![Action::FilterValue { col: 1, value: "Paris".into(), keep: false }], &ctx);
    settle(&mut h);
    assert_eq!(values(&h), "2,3,4", "hidden: \"Paris\" only, not \"paris\" nor \"Paris \"");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key typed over a block keeps the block: Enter then writes the value in every cell.
#[test]
fn typing_over_a_block_fills_it() {
    let (mut h, dir) = app_on("a,b\n1,x\n2,y\n3,z\n", "typeblock");
    click_grid(&mut h);
    {
        let g = &mut h.state_mut().tabs[0].grid;
        g.anchor = Some(crate::grid::Cell { row: 0, col: 1 });
        g.cursor = Some(crate::grid::Cell { row: 2, col: 1 });
    }
    h.run_steps(1);
    h.event(Event::Text("q".into()));
    h.run_steps(2);
    assert!(h.state().tabs[0].grid.editing.is_some());
    key(&mut h, egui::Key::Enter, egui::Modifiers::NONE);
    settle(&mut h);
    let t = &h.state().tabs[0];
    let st = t.st().unwrap();
    assert_eq!((0..3).map(|p| st.cell(t.shown_id(p), 1)).collect::<Vec<_>>(), ["q", "q", "q"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Text sorts read numbers in it ("Lot 2" before "Lot 10"), accents and case aside, as 1.x's
/// collator; the status line says what the sort did.
#[test]
fn text_sort_is_natural() {
    use crate::grid::Action;
    let (mut h, dir) = app_on("name\nLot 10\nlot 2\nÉmile\nLot 1\nemma\n", "natsort");
    let ctx = h.ctx.clone();
    h.state_mut().apply(vec![Action::Sort(vec![csvfab_engine::view::SortKey { col: 0, desc: false }])], &ctx);
    settle(&mut h);
    assert_eq!(values(&h), "Émile,emma,Lot 1,lot 2,Lot 10");
    assert!(h.state().status.as_ref().is_some_and(|s| s.1.starts_with("Sorted by name ↑ (text)")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A filter that does not compile: the box it is in is marked, the reason is in the status
/// line, and the rows shown are not said to be filtered.
#[test]
fn bad_column_filter_is_told() {
    let (mut h, dir) = app_on("n,total\n1,5\n2,2000\n", "badfilter");
    let ctx = h.ctx.clone();
    h.state_mut().tabs[0].col_filters[1] = ">abc".into();
    h.state_mut().tabs[0].refresh(&ctx);
    settle(&mut h);
    let t = &h.state().tabs[0];
    assert_eq!(t.filter_error_col, Some(1));
    assert!(t.filter_error.as_deref().is_some_and(|e| e.contains("total") && e.contains("not a number")), "{:?}", t.filter_error);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The duplicate marks are counted again after any edit, once, a moment after the last.
#[test]
fn duplicate_marks_follow_edits() {
    let (mut h, dir) = app_on("name,city\nAnn,Lyon\nAnn,Lyon\nBob,Nice\nCid,Nice\n", "dupmarks");
    let ctx = h.ctx.clone();
    let st = h.state().tabs[0].st().unwrap().clone();
    let keys = vec![crate::tools::dupes::KeyDraft::column("name")];
    let rules = crate::tools::dupes::compile(&st.headers(), &keys).unwrap().unwrap();
    let ids: Vec<csvfab_engine::RowId> = (0..st.len()).map(|p| st.data_id(p)).collect();
    let d = csvfab_engine::dupes::find_rules(&st, &ids, &rules, &[], &std::sync::atomic::AtomicBool::new(false), None).unwrap();
    assert_eq!(d.groups(), 1);
    h.state_mut().tabs[0].marks_mut_dups(Some(std::sync::Arc::new(crate::tools::dupes::DupMarks::new(keys, rules, d))));
    // Bob renamed Ann: a group of three; typed twice quickly, counted once.
    for v in ["An", "Ann"] {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(2);
        t.apply_changes(&[(id, 0, v.into())], &ctx);
        h.run_steps(1);
    }
    std::thread::sleep(Duration::from_millis(350));
    settle(&mut h);
    let t = &h.state().tabs[0];
    let m = t.marks.dups.as_ref().expect("still marked");
    assert_eq!(m.group(t.shown_id(2)), m.group(t.shown_id(0)), "the renamed row joined the group");
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Delete duplicates" in the dialog: made ready in a job, one undo step, the rows back after.
#[test]
fn duplicates_deleted_in_a_job_then_undone() {
    let (mut h, dir) = app_on("name,city\nAnn,Lyon\nAnn,Lyon\nBob,Nice\nann, LYON\nCid,Nice\n", "dupdelete");
    let ctx = h.ctx.clone();
    let before = h.state().tabs[0].table.as_ref().unwrap().undo_depth();
    crate::shot::push("dupes-delete");
    let c = crate::tools::commands().into_iter().find(|c| c.name == "Remove duplicates…").unwrap();
    h.state_mut().run_command(c.run, &ctx);
    for _ in 0..600 {
        h.run_steps(1);
        if h.state().tabs[0].shown() == 3 { break }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.state().tabs[0].shown(), 3, "Ann's twins gone");
    let t = &mut h.state_mut().tabs[0];
    assert_eq!(t.table.as_ref().unwrap().undo_depth(), before + 1);
    t.table.as_mut().unwrap().undo();
    t.after_edit(true, &ctx);
    settle(&mut h);
    assert_eq!(h.state().tabs[0].shown(), 5, "all back");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save as Parquet from the dialog, then the file opened as the other imports: its CSV beside
/// it, in a tab of its own, the toast of a conversion, a damaged file told.
#[test]
fn parquet_saved_and_opened() {
    let (mut h, dir) = app_on("a;b\n1;x\n2;y\n", "parquet");
    let ctx = h.ctx.clone();
    let settle = |h: &mut egui_kittest::Harness<'static, App>, done: &dyn Fn(&App) -> bool| for _ in 0..400 {
        h.run_steps(1);
        if done(h.state()) { break; }
        std::thread::sleep(Duration::from_millis(5));
    };
    let c = crate::tools::commands().into_iter().find(|c| c.name == "Save as Parquet file").unwrap();
    h.state_mut().run_command(c.run, &ctx);
    h.run_steps(4);
    key(&mut h, egui::Key::Enter, Default::default());
    let saved = dir.join("t.parquet");
    settle(&mut h, &|a| a.toasts.iter().any(|t| t.text.contains("Parquet file written")));
    assert!(h.state().toasts.iter().any(|t| t.text.contains("Parquet file written: t.parquet — 2 rows")), "{:?}", h.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
    // A second artefact: the tab stays on its CSV.
    assert_eq!(h.state().tabs[0].name, "t.csv");

    let copy = dir.join("p.parquet");
    std::fs::copy(&saved, &copy).unwrap();
    h.state_mut().open(copy, &ctx);
    settle(&mut h, &|a| a.tabs.len() == 2 && a.tabs[1].table.is_some() && !a.tabs[1].busy());
    assert_eq!(h.state().tabs[1].name, "p.csv");
    assert!(h.state().toasts.iter().any(|t| t.text.contains("p.parquet → p.csv: 2 rows, 2 columns.")));
    assert_eq!(std::fs::read_to_string(dir.join("p.csv")).unwrap(), "a;b\n1;x\n2;y\n");

    let bad = dir.join("bad.parquet");
    std::fs::write(&bad, "not parquet at all").unwrap();
    h.state_mut().open(bad, &ctx);
    h.run_steps(30);
    assert_eq!(h.state().tabs.len(), 2);
    assert!(!dir.join("bad.csv").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A tab hiding `secret`, its rows filtered and sorted: `a` hidden by value, 3 before 1.
fn tab_with_hidden(tag: &str) -> (egui_kittest::Harness<'static, App>, std::path::PathBuf, egui::Context) {
    let (mut h, dir) = app_on("id;name;secret\n1;a;x\n2;b;y\n3;c;z\n", tag);
    let ctx = h.ctx.clone();
    {
        let t = &mut h.state_mut().tabs[0];
        t.grid.hidden.insert(2);
        t.filter.hidden = vec![(1, std::sync::Arc::new(["b".to_string()].into_iter().collect()))];
        t.sort = vec![csvfab_engine::view::SortKey { col: 0, desc: true }];
        t.refresh(&ctx);
    }
    until(&mut h, &|a| !a.tabs[0].busy());
    assert_eq!(values(&h), "3,1");
    (h, dir, ctx)
}

fn until(h: &mut egui_kittest::Harness<'static, App>, done: &dyn Fn(&App) -> bool) {
    for _ in 0..600 {
        h.run_steps(1);
        if done(h.state()) { return; }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("never done: {:?}", h.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
}

/// A file made by an export, as CSV.
fn as_csv(f: csvfab_engine::convert::Format, p: &std::path::Path) -> String {
    let dest = p.with_extension("back.csv");
    csvfab_engine::convert::import(f, p, None, b';', &dest, false, None, &Default::default()).unwrap();
    std::fs::read_to_string(dest).unwrap()
}

/// Excel, SQLite, Parquet: no hidden column, and by default only the rows shown, in the view's order.
#[test]
fn exports_leave_out_what_is_hidden() {
    use csvfab_engine::convert::Format;
    for (name, file, format) in [("Save as Excel workbook", "t.xlsx", Format::Xlsx), ("Save as SQLite database", "t.sqlite", Format::Sqlite), ("Save as Parquet file", "t.parquet", Format::Parquet)] {
        let (mut h, dir, ctx) = tab_with_hidden(&format!("hide-{file}"));
        let c = crate::tools::commands().into_iter().find(|c| c.name == name).unwrap();
        h.state_mut().run_command(c.run, &ctx);
        h.run_steps(4);
        key(&mut h, egui::Key::Enter, Default::default());
        until(&mut h, &|a| a.toasts.iter().any(|t| t.text.contains("written:")));
        assert_eq!(as_csv(format, &dir.join(file)), "id;name\n3;c\n1;a\n", "{name}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The CSV of the rows shown and Extract are exports too.
#[test]
fn csv_exports_leave_out_what_is_hidden() {
    let (mut h, dir, ctx) = tab_with_hidden("hide-csv");
    h.state_mut().run_command(|cx| { let t = crate::tools::export::shown_csv(cx.tab, "shown.csv"); cx.out.push(crate::tools::Cmd::Tool(t)) }, &ctx);
    h.run_steps(4);
    key(&mut h, egui::Key::Enter, Default::default());
    until(&mut h, &|a| a.tabs.len() == 2 && a.tabs[1].table.is_some() && !a.tabs[1].busy());
    assert_eq!(std::fs::read_to_string(dir.join("shown.csv")).unwrap(), "id;name\n3;c\n1;a\n");
    h.state_mut().active = 0;
    let c = crate::tools::commands().into_iter().find(|c| c.name == "Extract the rows shown to a new file").unwrap();
    h.state_mut().run_command(c.run, &ctx);
    until(&mut h, &|a| a.tabs.len() == 3 && a.tabs[2].table.is_some() && !a.tabs[2].busy());
    assert_eq!(std::fs::read_to_string(dir.join("t-2.csv")).unwrap(), "id;name\n3;c\n1;a\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Split into files writes the table without the hidden column (what it writes: engine test
/// `a_table_without_columns_is_written_by_every_export`; its buttons have no label to click).
#[test]
fn split_leaves_out_what_is_hidden() {
    let (mut h, dir, ctx) = tab_with_hidden("hide-split");
    h.state_mut().run_command(|cx| assert_eq!(crate::tools::split::columns_written(cx), Some(2)), &ctx);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save and Save as keep the hidden column and the filtered rows, in the file's order: the
/// tab goes on from that file.
#[test]
fn saves_keep_what_is_hidden() {
    let (mut h, dir, ctx) = tab_with_hidden("hide-save");
    {
        let t = &mut h.state_mut().tabs[0];
        let id = t.shown_id(0);
        t.table.as_mut().unwrap().set_cells(&[(id, 1, "C".into())]);
        t.after_edit(false, &ctx);
    }
    until(&mut h, &|a| !a.tabs[0].busy());
    h.state_mut().save(None, false, &ctx);
    until(&mut h, &|a| !a.saving());
    assert_eq!(std::fs::read_to_string(dir.join("t.csv")).unwrap(), "id;name;secret\n1;a;x\n2;b;y\n3;C;z\n");
    // Ctrl+S with nothing to write, then Save as under a new name, the box left as it starts.
    h.state_mut().save_as(&ctx);
    h.run_steps(4);
    h.event(Event::Text("copy".into()));
    h.run_steps(2);
    key(&mut h, egui::Key::Enter, Default::default());
    until(&mut h, &|a| a.toasts.iter().any(|t| t.text.contains("Saved as a new file")));
    assert_eq!(std::fs::read_to_string(dir.join("copy.csv")).unwrap(), "id;name;secret\n1;a;x\n2;b;y\n3;C;z\n");
    // The tab moved onto it and still hides the column.
    assert_eq!(h.state().tabs[0].name, "copy.csv");
    assert!(h.state().tabs[0].grid.hidden.contains(&2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The toasts saying a new file left its table's encoding.
fn switches(h: &egui_kittest::Harness<'static, App>) -> Vec<String> {
    h.state().toasts.iter().filter(|t| t.text.contains("Written in UTF-8")).map(|t| t.text.clone()).collect()
}

/// A table in Windows-1251, its one tab active, nothing told yet.
fn cyrillic(tag: &str) -> (egui_kittest::Harness<'static, App>, std::path::PathBuf, egui::Context) {
    let text = "имя;город\n".to_string() + &"Иван Петров;Москва, улица Ленина\nАнна Иванова;Самара, улица Гагарина\n".repeat(20);
    let (h, dir) = app_on(encoding_rs_bytes(&text), tag);
    let ctx = h.ctx.clone();
    assert_eq!(h.state().tabs[0].st().unwrap().base.enc, csvfab_engine::Enc::Windows1251);
    (h, dir, ctx)
}

/// `text` typed in the second column of a row.
fn type_in(h: &mut egui_kittest::Harness<'static, App>, ctx: &egui::Context, row: usize, text: &str) {
    let t = &mut h.state_mut().tabs[0];
    let id = t.shown_id(row);
    t.table.as_mut().unwrap().set_cells(&[(id, 1, text.into())]);
    t.after_edit(false, ctx);
    until(h, &|a| !a.tabs[0].busy());
}

/// Extract goes to UTF-8 when an edit needs it, and says so, naming the file; not otherwise.
#[test]
fn extract_tells_of_a_switch_to_utf8() {
    let (mut h, dir, ctx) = cyrillic("sw-extract");
    let run = |h: &mut egui_kittest::Harness<'static, App>, want: &str| {
        h.state_mut().active = 0;
        h.state_mut().toasts.clear();
        let c = crate::tools::commands().into_iter().find(|c| c.name == "Extract the rows shown to a new file").unwrap();
        h.state_mut().run_command(c.run, &ctx);
        until(h, &|a| a.toasts.iter().any(|t| t.text.contains(&format!("exported to {want}"))));
    };
    run(&mut h, "t-2.csv");
    assert_eq!(switches(&h), Vec::<String>::new());
    assert_eq!(std::fs::read(dir.join("t-2.csv")).unwrap(), std::fs::read(dir.join("t.csv")).unwrap());
    // Typed and in the encoding: still nothing to say.
    type_in(&mut h, &ctx, 3, "Киев");
    run(&mut h, "t-3.csv");
    assert_eq!(switches(&h), Vec::<String>::new());
    assert!(std::str::from_utf8(&std::fs::read(dir.join("t-3.csv")).unwrap()).is_err());
    type_in(&mut h, &ctx, 3, "東京é");
    run(&mut h, "t-4.csv");
    assert_eq!(switches(&h), ["t-4.csv | Written in UTF-8: 3 characters don't exist in Windows-1251"]);
    let out = std::fs::read_to_string(dir.join("t-4.csv")).unwrap();
    assert!(out.contains("東京é") && out.contains("Москва"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Split says so too, naming the folder, every file of which is in UTF-8.
#[test]
fn split_tells_of_a_switch_to_utf8() {
    let (mut h, dir, ctx) = cyrillic("sw-split");
    h.state_mut().run_command(|cx| {
        let parent = cx.tab.path.as_ref().unwrap().parent().unwrap().to_path_buf();
        let t = crate::tools::split::written_in(cx, 1, parent).unwrap();
        cx.out.push(crate::tools::Cmd::Tool(t));
    }, &ctx);
    until(&mut h, &|a| a.toasts.iter().any(|t| t.text.contains("2 files written")));
    assert_eq!(switches(&h), Vec::<String>::new());
    type_in(&mut h, &ctx, 3, "東");
    h.state_mut().toasts.clear();
    h.state_mut().run_command(|cx| {
        let parent = cx.tab.path.as_ref().unwrap().parent().unwrap().to_path_buf();
        let t = crate::tools::split::written_in(cx, 0, parent).unwrap();
        cx.out.push(crate::tools::Cmd::Tool(t));
    }, &ctx);
    until(&mut h, &|a| a.toasts.iter().any(|t| t.text.contains("2 files written")));
    let folder = csvfab_engine::split::folder_name("t.csv", "имя", 0);
    assert_eq!(switches(&h), [format!("{folder} | Written in UTF-8: 1 character doesn't exist in Windows-1251")]);
    for f in std::fs::read_dir(dir.join(&folder)).unwrap().flatten() {
        assert!(std::fs::read_to_string(f.path()).is_ok(), "{:?} is not in UTF-8", f.path());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Combine says so when an edit needs it, and when the first file's encoding is not shared.
#[test]
fn combine_tells_of_a_switch_to_utf8() {
    let (mut h, dir, ctx) = cyrillic("sw-combine");
    std::fs::copy(dir.join("t.csv"), dir.join("same.csv")).unwrap();
    std::fs::write(dir.join("utf8.csv"), "имя;город\nЁлка;Тула\n").unwrap();
    let run = |h: &mut egui_kittest::Harness<'static, App>, file: &'static str, dest: &'static str| {
        h.state_mut().active = 0;
        h.state_mut().toasts.clear();
        CASE.with_borrow_mut(|c| *c = (file, dest));
        h.state_mut().run_command(|cx| {
            let (file, dest) = CASE.with_borrow(|c| *c);
            let dir = cx.tab.path.as_ref().unwrap().parent().unwrap().to_path_buf();
            let t = crate::tools::combine::written_to(cx, &dir.join(file), dir.join(dest)).unwrap();
            cx.out.push(crate::tools::Cmd::Tool(t));
        }, &ctx);
        until(h, &|a| a.toasts.iter().any(|t| t.text.contains(&format!("combined into {dest}"))));
    };
    thread_local! { static CASE: std::cell::RefCell<(&'static str, &'static str)> = const { std::cell::RefCell::new(("", "")) }; }
    run(&mut h, "same.csv", "a.csv");
    assert_eq!(switches(&h), Vec::<String>::new());
    assert!(std::str::from_utf8(&std::fs::read(dir.join("a.csv")).unwrap()).is_err());
    run(&mut h, "utf8.csv", "b.csv");
    assert_eq!(switches(&h), ["b.csv | Written in UTF-8: the files aren't all in Windows-1251"]);
    assert!(std::fs::read_to_string(dir.join("b.csv")).unwrap().contains("Ёлка"));
    type_in(&mut h, &ctx, 3, "東京");
    run(&mut h, "same.csv", "c.csv");
    assert_eq!(switches(&h), ["c.csv | Written in UTF-8: 2 characters don't exist in Windows-1251"]);
    assert!(std::fs::read_to_string(dir.join("c.csv")).unwrap().contains("東京"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A recovery copy is in UTF-8: nothing said at each autosave, once when it comes back, and
/// why it truly is. A journal, on the file itself, has nothing to say.
#[test]
fn a_recovery_copy_tells_of_utf8_once() {
    let (mut h, dir, ctx) = cyrillic("sw-recov");
    let rdir = dir.join("recovery");
    h.state_mut().recovery = crate::recovery::Recovery::at(rdir.clone());
    let meta = || std::fs::read_dir(&rdir).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "meta"));
    let wait_for = |h: &mut egui_kittest::Harness<'static, App>, ext: &str, note: &str| {
        for _ in 0..600 {
            h.run_steps(5);
            if let Some(m) = meta() {
                if m.with_extension(ext).exists() && std::fs::read_to_string(&m).unwrap().lines().nth(2).unwrap_or("") == note { return m; }
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("no .{ext} kept with {note:?}");
    };
    type_in(&mut h, &ctx, 1, "Киев");
    wait_for(&mut h, "journal", "");
    // The file changed under the table: a copy, in UTF-8 even though the edit fits.
    let bytes = std::fs::read(dir.join("t.csv")).unwrap();
    std::fs::write(dir.join("other.csv"), bytes).unwrap();
    std::fs::rename(dir.join("other.csv"), dir.join("t.csv")).unwrap();
    type_in(&mut h, &ctx, 2, "Омск");
    wait_for(&mut h, "csv", "Written in UTF-8: recovery copies don't keep Windows-1251");
    type_in(&mut h, &ctx, 3, "東京");
    let m = wait_for(&mut h, "csv", "Written in UTF-8: 2 characters don't exist in Windows-1251");
    assert_eq!(switches(&h), Vec::<String>::new());
    assert!(std::fs::read_to_string(m.with_extension("csv")).unwrap().contains("東京"));
    // As if left by a run no longer there: told when it is taken back, then gone.
    let stem = m.file_stem().unwrap().to_str().unwrap().to_string();
    let other = stem.replacen(&std::process::id().to_string(), "999999999", 1);
    std::fs::rename(m.with_extension("csv"), rdir.join(format!("{other}.csv"))).unwrap();
    std::fs::rename(&m, rdir.join(format!("{other}.meta"))).unwrap();
    let left = crate::recovery::left_in(&rdir);
    assert_eq!(left.len(), 1);
    assert_eq!(switches(&h), Vec::<String>::new());
    h.state_mut().take_back(&left, true, &ctx);
    assert_eq!(switches(&h), ["t.csv (recovered) | Written in UTF-8: 2 characters don't exist in Windows-1251"]);
    assert!(crate::recovery::left_in(&rdir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ctrl+S with a character the file's encoding cannot hold: it asks, and "Save as UTF-8" keeps it.
#[test]
fn a_character_the_encoding_lacks_is_asked_about() {
    use egui_kittest::kittest::Queryable;
    let text = "имя;город\n".to_string() + &"Иван Петров;Москва, улица Ленина\n".repeat(40);
    let (mut h, dir) = app_on(encoding_rs_bytes(&text), "lossy");
    let ctx = h.ctx.clone();
    assert_eq!(h.state().tabs[0].st().unwrap().base.enc, csvfab_engine::Enc::Windows1251);
    {
        let t = h.state_mut().tabs[0].table.as_mut().unwrap();
        let id = t.st.data_id(3);
        t.set_cells(&[(id, 1, "東京".into())]);
    }
    h.state_mut().save(None, false, &ctx);
    for _ in 0..100 {
        h.run_steps(1);
        if !h.state().saving() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h.get_by_label("Save as UTF-8").click();
    for _ in 0..100 {
        h.run_steps(1);
        if !h.state().saving() && !h.state().tabs[0].busy() && !h.state().tabs[0].dirty() { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let b = std::fs::read(dir.join("t.csv")).unwrap();
    assert!(b.starts_with(b"\xEF\xBB\xBF"));
    let s = std::str::from_utf8(&b[3..]).unwrap();
    assert!(s.contains("東京") && s.contains("Москва") && !s.contains('?'), "{s}");
    assert_eq!(h.state().tabs[0].open_opts.enc, Some(csvfab_engine::Enc::Utf8));
    let _ = std::fs::remove_dir_all(&dir);
}

fn encoding_rs_bytes(s: &str) -> Vec<u8> {
    let mut o = Vec::new();
    for c in s.chars() {
        o.push(match c as u32 {
            0x410..=0x44F => (c as u32 - 0x410 + 0xC0) as u8,
            u => u as u8,
        });
    }
    o
}

/// A zip of one table: unpacked beside it, what comes out opens in a tab.
#[test]
fn a_zip_opens_through_its_table() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("csvfab-ui-zip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pack.zip");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    z.start_file("in/table.csv", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all("name;n\nZoé;1\n東京;2\n".as_bytes()).unwrap();
    z.finish().unwrap();
    let p2 = path.clone();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000.0, 600.0)).with_pixels_per_point(1.0).build_eframe(move |cc| App::new(cc, vec![p2]));
    for _ in 0..400 {
        h.step();
        if h.state().tabs.first().is_some_and(|t| t.table.is_some() && !t.busy()) { break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    let t = &h.state().tabs[0];
    assert_eq!(t.name, "table.csv");
    assert_eq!(t.st().unwrap().cell(t.st().unwrap().data_id(1), 0), "東京");
    assert!(dir.join("table.csv").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}
