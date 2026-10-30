//! The real app off screen on what the top band, the density and the inspector change.

use crate::app::App;
use crate::theme::DENSITIES;
use csvfab_engine::view::Keeps;
use eframe::egui::{self, Event, PointerButton};
use std::time::Duration;

type H = egui_kittest::Harness<'static, App>;

fn app_on(csv: &str, tag: &str) -> (H, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("csvfab-look-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.csv");
    std::fs::write(&path, csv).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1200.0, 700.0)).with_pixels_per_point(1.0).with_accessibility_check(false).build_eframe(move |cc| App::new(cc, vec![path]));
    wait(&mut h, &|a| a.tabs.first().is_some_and(|t| t.table.is_some() && !t.busy()));
    h.run_steps(10);
    (h, dir)
}

fn wait(h: &mut H, done: &dyn Fn(&App) -> bool) {
    for _ in 0..800 {
        h.run_steps(1);
        if done(h.state()) { return; }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("never done");
}

fn click(h: &mut H, at: egui::Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
}

const CSV: &str = "city,amount\nLyon,1\nParis,2\nLyon,3\n,x\nLyon,5\nNice,6\nParis,7\nLyon,8\n";

/// The rows' density moves the rows and what is hit there; the row on top stays on top.
#[test]
fn density_changes_the_rows_and_keeps_the_top_one() {
    let csv: String = std::iter::once("n,v\n".to_string()).chain((0..2000).map(|i| format!("{i},x{i}\n"))).collect();
    let (mut h, dir) = app_on(&csv, "density");
    assert_eq!((h.state().m.row_h, h.state().m.totals_h), (32.0, 32.0));
    h.state_mut().tabs[0].grid.set_top(500);
    h.run_steps(2);
    for (d, (_, rh)) in DENSITIES.iter().enumerate() {
        h.state_mut().set_density(d);
        h.run_steps(2);
        let t = &h.state().tabs[0];
        assert_eq!((t.grid.top, h.state().m.row_h), (500, *rh));
        // The first row's first cell, then the third's.
        let y0 = 46.0 + 68.0 + 29.0;
        for k in [0.0, 2.0] {
            click(&mut h, egui::pos2(200.0, y0 + (k + 0.5) * rh));
            assert_eq!(h.state().tabs[0].grid.cursor.map(|c| c.row), Some(500 + k as usize), "density {d}");
        }
    }
    assert_eq!(h.state().config.density, Some(2));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The chips' bar is there while a filter is in force, and pushes the grid down.
#[test]
fn chips_bar_only_with_a_filter() {
    let (mut h, dir) = app_on(CSV, "chips");
    let ctx = h.ctx.clone();
    let body_top = |h: &H| h.ctx.viewport(|v| v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.sense.senses_click() && w.sense.senses_drag()).map(|w| w.rect).max_by(|a, b| a.area().total_cmp(&b.area())).unwrap().min.y);
    let y0 = body_top(&h);
    assert_eq!(y0, 46.0 + 68.0 + 29.0);
    h.state_mut().tabs[0].filter.global = "lyon".into();
    h.state_mut().tabs[0].refresh(&ctx);
    wait(&mut h, &|a| !a.tabs[0].busy());
    h.run_steps(3);
    assert_eq!(body_top(&h), y0 + 34.0);
    assert_eq!(h.state().tabs[0].shown(), 4);
    // Its ×, 12 px from the left, the chip 75 or so wide.
    let at = h.ctx.viewport(|v| v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.rect.width() == 18.0 && w.sense.senses_click()).map(|w| w.rect.center()).next()).unwrap();
    click(&mut h, at);
    wait(&mut h, &|a| !a.tabs[0].busy());
    h.run_steps(3);
    assert_eq!((h.state().tabs[0].shown(), body_top(&h)), (8, y0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A click on a title still sorts, and takes the inspector there: no "first click selects".
#[test]
fn a_title_click_sorts_and_the_inspector_follows() {
    let (mut h, dir) = app_on(CSV, "insp-title");
    assert!(h.state().tabs[0].inspector.is_none(), "off by default: nothing kept");
    h.state_mut().toggle_inspector();
    h.run_steps(2);
    assert_eq!(h.state().config.inspector, Some(true));
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    assert_eq!(h.state().tabs[0].grid.focus_col, Some(0));
    // The second title's text, in the middle of the title line.
    let w0 = h.state().tabs[0].grid.widths[0];
    click(&mut h, egui::pos2(92.0 + w0 + 40.0, 62.0));
    assert_eq!(h.state().tabs[0].sort.first().map(|k| (k.col, k.desc)), Some((1, false)));
    assert_eq!(h.state().tabs[0].grid.focus_col, Some(1));
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    h.state_mut().toggle_inspector();
    h.run_steps(2);
    assert!(h.state().tabs[0].inspector.is_none(), "off: nothing kept, nothing polled");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A top value's click is a value filter on the column, as a chart's part; a second click shows all again.
#[test]
fn a_top_value_filters() {
    let (mut h, dir) = app_on(CSV, "insp-top");
    h.state_mut().toggle_inspector();
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    h.run_steps(3);
    // "Lyon" is the top value: its row, among the panel's widgets, the first 30 tall and wide.
    let row = |h: &H| h.ctx.viewport(|v| v.prev_pass.widgets.layers().flat_map(|(_, ws)| ws.iter()).filter(|w| w.rect.height() == 30.0 && w.rect.min.x > 880.0 && w.rect.width() > 200.0 && w.sense.senses_click()).map(|w| w.rect).min_by(|a, b| a.min.y.total_cmp(&b.min.y))).expect("the top values");
    let at = row(&h).center();
    click(&mut h, at);
    wait(&mut h, &|a| !a.tabs[0].busy());
    let t = &h.state().tabs[0];
    assert_eq!(t.shown(), 4);
    assert!(matches!(&t.filter.picks[..], [p] if p.col == 0 && matches!(&p.keep, Keeps::Values(v) if v.contains("Lyon"))));
    h.run_steps(3);
    // Under the chips' bar now.
    let at = row(&h).center();
    click(&mut h, at);
    wait(&mut h, &|a| !a.tabs[0].busy());
    assert_eq!(h.state().tabs[0].shown(), 8);
    assert!(h.state().tabs[0].filter.picks.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Numbers and dates get their four figures, wrong-typed values are counted.
#[test]
fn inspector_on_a_number_and_a_date_column() {
    let csv: String = std::iter::once("n,d\n".to_string()).chain((1..=40).map(|i| format!("{},2024-01-{:02}\n", if i == 7 { "n/a".to_string() } else { i.to_string() }, i % 28 + 1))).collect();
    let (mut h, dir) = app_on(&csv, "insp-num");
    h.state_mut().tabs[0].grid.focus_col = Some(0);
    h.state_mut().toggle_inspector();
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    h.run_steps(2);
    let c = h.state().tabs[0].inspector.as_ref().unwrap().counted().unwrap();
    assert_eq!((c.prof.filled, c.prof.count, c.odd.len()), (40, 39, 1));
    assert!(c.prof.median.is_some() && c.prof.avg().is_some());
    h.state_mut().tabs[0].grid.focus_col = Some(1);
    h.run_steps(1);
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    let c = h.state().tabs[0].inspector.as_ref().unwrap().counted().unwrap();
    assert!(c.prof.date_median.is_some() && c.prof.date_mean.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
