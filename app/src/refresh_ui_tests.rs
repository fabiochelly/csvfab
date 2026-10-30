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
        let y0 = 52.0 + 68.0 + 29.0;
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
    assert_eq!(y0, 52.0 + 68.0 + 29.0);
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

/// The inspector's histogram: x in the window of bar `i` of 28 (`right`: the last one, set apart).
const HIST_Y: f32 = 350.0;
fn hist_x(i: f32) -> f32 {
    896.0 + 288.0 * (i + 0.5) / 28.0
}

/// A click on a bar and a drag across bars keep what the bars say; the end bar keeps the far
/// values, however far; again: off. The histogram stays whole under its own filter.
#[test]
fn the_inspector_histogram_filters_what_it_shows() {
    let csv: String = std::iter::once("v,x\n".to_string()).chain((0..400).map(|i| format!("{},a\n", i % 100 + 1))).chain(["900000,b\n".to_string(), "22039,b\n".to_string(), "1500000,b\n".to_string()]).collect();
    let (mut h, dir) = app_on(&csv, "insp-hist");
    h.state_mut().tabs[0].grid.focus_col = Some(0);
    h.state_mut().toggle_inspector();
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    h.run_steps(2);
    let c = h.state().tabs[0].inspector.as_ref().unwrap().counted().unwrap();
    let last = c.prof.hist.last().unwrap().clone();
    assert!(last.tail && last.count == 3, "{last:?}");
    let body = c.prof.hist[..27].to_vec();
    assert!(body.iter().all(|b| b.count > 0));
    // The end bar.
    click(&mut h, egui::pos2(1181.0, HIST_Y));
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() == 3);
    assert!(matches!(&h.state().tabs[0].filter.picks[..], [p] if matches!(p.keep, Keeps::Range { lo: 22039.0, hi: 1500000.0, .. })));
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    assert_eq!(h.state().tabs[0].inspector.as_ref().unwrap().counted().unwrap().prof.hist, c.prof.hist, "its own filter aside");
    h.run_steps(3);
    // Again: off.
    click(&mut h, egui::pos2(1181.0, HIST_Y));
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() == 403);
    assert!(h.state().tabs[0].filter.picks.is_empty());
    h.run_steps(3);
    // A bar, a drag.
    click(&mut h, egui::pos2(hist_x(4.0), HIST_Y));
    let want: u64 = body[4].count;
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() as u64 == want);
    h.run_steps(3);
    click(&mut h, egui::pos2(hist_x(4.0), HIST_Y));
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() == 403);
    h.run_steps(3);
    let (a, z) = (egui::pos2(hist_x(2.0), HIST_Y), egui::pos2(hist_x(9.0), HIST_Y));
    h.hover_at(a);
    h.run_steps(1);
    h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    for k in 1..=4 { h.hover_at(a + (z - a) * (k as f32 / 4.0)); h.run_steps(1); }
    h.event(Event::PointerButton { pos: z, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    let want: u64 = body[2..=9].iter().map(|b| b.count).sum();
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() as u64 == want);
    assert!(matches!(&h.state().tabs[0].filter.picks[..], [p] if matches!(p.keep, Keeps::Range { lo, hi, .. } if lo == body[2].lo && hi == body[9].hi)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ctrl+Shift+I is the inspector's, Ctrl+I still the row card's; the palette finds it.
#[test]
fn the_inspector_has_its_shortcut() {
    use egui::{Key, Modifiers};
    let (mut h, dir) = app_on(CSV, "insp-key");
    let press = |h: &mut H, modifiers: Modifiers| {
        for pressed in [true, false] { h.event(Event::Key { key: Key::I, physical_key: None, pressed, repeat: false, modifiers }); }
        h.run_steps(3);
    };
    let card = h.state().card.open;
    press(&mut h, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!((h.state().config.inspector, h.state().card.open), (Some(true), card));
    press(&mut h, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!((h.state().config.inspector, h.state().card.open), (Some(false), card));
    assert!(h.state().tabs[0].inspector.is_none());
    click(&mut h, egui::pos2(200.0, 52.0 + 68.0 + 29.0 + 16.0));
    press(&mut h, Modifiers::COMMAND);
    assert_eq!((h.state().config.inspector, h.state().card.open), (Some(false), !card));
    assert!(crate::tools::commands().iter().any(|c| c.name == "Column inspector" && c.keys == Some("Ctrl+Shift+I")));
    assert!(crate::chrome::SHORTCUTS.iter().any(|k| k.0 == "Ctrl+Shift+I"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The figures keep their places whatever the values; every state draws, narrow or not.
#[test]
fn the_inspector_on_every_kind_of_column() {
    let csv = "n,empty,one,long,day\n".to_string() + &(0..30).map(|i| format!("{},,42,{}.25,2024-01-{:02}\n", i, 9_000_000_000_000u64 + i * 1_000_000_007, i % 28 + 1)).collect::<String>();
    let (mut h, dir) = app_on(&csv, "insp-kinds");
    h.state_mut().toggle_inspector();
    for c in 0..5 {
        h.state_mut().tabs[0].grid.focus_col = Some(c);
        h.run_steps(1);
        wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
        h.run_steps(2);
    }
    // No room: it stands aside and says so, once; back when there is.
    h.set_size(egui::vec2(560.0, 400.0));
    h.run_steps(3);
    assert!(h.state().tabs[0].inspector.as_ref().is_some_and(|i| i.is_aside()));
    assert!(h.state().toasts.iter().any(|t| t.text.contains("No room")));
    h.set_size(egui::vec2(1000.0, 300.0));
    h.run_steps(3);
    assert!(h.state().tabs[0].inspector.as_ref().is_some_and(|i| !i.is_aside()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Worth a look says what the histogram sets apart, and Show keeps the same rows as the end
/// bar; a column without far values has no such line.
#[test]
fn worth_a_look_agrees_with_the_histogram() {
    // 3 % far out: too many for an Insights card, and the histogram gathers them all the same.
    let csv: String = std::iter::once("v,w\n".to_string()).chain((0..970).map(|i| format!("{},{}\n", i % 100 + 1, i % 100 + 1))).chain((0..30).map(|i| format!("{},{}\n", 50_000 + i * 100, i % 100 + 1))).collect();
    let (mut h, dir) = app_on(&csv, "insp-far");
    h.state_mut().tabs[0].grid.focus_col = Some(0);
    h.state_mut().toggle_inspector();
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now() && i.settled()));
    h.run_steps(2);
    let c = h.state().tabs[0].inspector.as_ref().unwrap().counted().unwrap();
    assert_eq!(c.prof.hist.last().map(|b| (b.tail, b.count)), Some((true, 30)));
    let far = crate::colpanel::far_bars(&c.prof);
    assert_eq!(far, vec![(27, "30 values far out, over 100".to_string())]);
    // The cards leave it out: the same fences, but a card is for a handful.
    let tab = &h.state().tabs[0];
    let st = tab.st().unwrap().clone();
    let rep = csvfab_engine::insights::insights(&st, &std::sync::atomic::AtomicBool::new(false), None).unwrap();
    assert!(!rep.items.iter().any(|i| i.is_far()), "over 1 % of the values: no card");
    // Its line: the first under the head, under the chart and its note. Show keeps the end bar's rows.
    click(&mut h, egui::pos2(1170.0, 456.0));
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() == 30);
    assert!(matches!(&h.state().tabs[0].filter.picks[..], [p] if matches!(p.keep, Keeps::Range { lo: 50000.0, hi: 52900.0, .. })));
    // Without far values: the bars and the line are gone together.
    h.state_mut().tabs[0].grid.focus_col = Some(1);
    h.run_steps(1);
    wait(&mut h, &|a| a.tabs[0].inspector.as_ref().is_some_and(|i| i.counted_now()));
    let c = h.state().tabs[0].inspector.as_ref().unwrap().counted().unwrap();
    assert!(c.prof.hist.iter().all(|b| !b.tail) && crate::colpanel::far_bars(&c.prof).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A drag in a column's chart, end bar included, keeps what the bars hold, on screen.
#[test]
fn a_chart_drag_keeps_its_end_bar() {
    let csv: String = std::iter::once("a,v\n".to_string()).chain((0..400).map(|i| format!("x,{}\n", 100 + i % 100))).chain(["x,900000\n".to_string(), "x,22039\n".to_string(), "x,1500000\n".to_string()]).collect();
    let (mut h, dir) = app_on(&csv, "chart-end");
    wait(&mut h, &|a| a.tabs[0].spark.base.is_some());
    h.run_steps(3);
    let t = &h.state().tabs[0];
    let spark = t.spark.base.clone().unwrap();
    let csvfab_engine::sparkline::Col::Curve(c) = &spark.cols[1] else { panic!() };
    assert_eq!((c.bins.tails(), c.cnt[csvfab_engine::sparkline::BINS - 1]), ((false, true), 3));
    // The chart of the second column: its box, as the band places it.
    let (x0, w) = (92.0 + t.grid.widths[0], t.grid.widths[1]);
    let (l, r) = (x0 + 10.0, x0 + w - 11.0);
    let n = c.bins.n as f32;
    let y = 52.0 + 33.0 + 10.0;
    let (a, z) = (egui::pos2(l + (r - l) * 27.5 / n, y), egui::pos2(r - 0.5, y));
    h.hover_at(a);
    h.run_steps(2);
    h.event(Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.run_steps(1);
    for k in 1..=4 { h.hover_at(a + (z - a) * (k as f32 / 4.0)); h.run_steps(1); }
    h.event(Event::PointerButton { pos: z, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
    wait(&mut h, &|a| !a.tabs[0].busy() && a.tabs[0].shown() < 403);
    // The range starts where the drag took off: the bars from there on, whole, far values with them.
    let t = &h.state().tabs[0];
    let shown: Vec<String> = (0..t.shown()).map(|k| t.st().unwrap().cell(t.shown_id(k), 1)).collect();
    assert!(["22039", "900000", "1500000"].iter().all(|v| shown.iter().any(|x| x == v)), "{shown:?}");
    assert!((27..32).any(|b| c.cnt[b..].iter().sum::<u32>() as usize == shown.len()), "{}", shown.len());
    let _ = std::fs::remove_dir_all(&dir);
}
