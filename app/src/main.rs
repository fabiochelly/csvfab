//! csvfab — the native binary: everything is in the library.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    let r = csvfab::run();
    // Started from a menu or a file manager there is no console: a failure to open the
    // window is said the way the desktop says things (as the web launcher's `die`).
    if let Err(e) = &r {
        notify(&format!("csvfab could not start: {e}"));
    }
    r
}

fn notify(msg: &str) {
    use std::process::Command;
    eprintln!("{msg}");
    let _ = if cfg!(target_os = "macos") {
        Command::new("osascript").args(["-e", &format!("display notification {:?} with title \"csvfab\"", msg)]).output()
    } else if cfg!(windows) {
        Command::new("mshta").arg(format!("javascript:alert({:?});close()", msg)).output()
    } else {
        Command::new("notify-send").args(["-a", "csvfab", "-i", "csvfab", "csvfab", msg]).output()
    };
}
