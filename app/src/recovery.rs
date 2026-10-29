//! Edits not saved yet, kept on disk in case csvfab stops without asking (a crash, the
//! machine going down): each tab with unsaved edits is written in the background to
//! `<state dir>/recovery/` a few seconds after its last edit (a big file less often), and
//! removed once saved, discarded or closed. A start that finds copies left by a csvfab no
//! longer running offers them back, as new tabs.

use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::save::{self, SaveOptions};
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Quiet time after an edit before the copy is written; a file past `BIG` bytes waits
/// `BIG_EVERY` between copies (each writes the whole table).
const QUIET: f64 = 3.0;
const BIG: u64 = 100 << 20;
const BIG_EVERY: f64 = 30.0;

/// The unsaved tables as of the last edit, for the panic hook: a crash on the UI thread
/// writes them at once (nothing lost), the copies above being for a hard stop.
type Latest = HashMap<u64, (u64, String, Option<PathBuf>, csvfab_engine::State)>;
static LATEST: std::sync::Mutex<Option<Latest>> = std::sync::Mutex::new(None);

/// The panic hook: a report in `<state dir>/crash.log` (what, where, the backtrace), and
/// when the window's own thread panics — the app is going down — every unsaved table
/// written to the recovery folder. Panics in background jobs are caught there (job.rs):
/// they only get the report.
pub fn install_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ui = std::thread::current().name() == Some("main");
        let dir = crate::config::dir();
        let _ = std::fs::create_dir_all(&dir);
        let when = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let report = format!(
            "---- {when} csvfab {} thread {:?}{}\n{info}\n{}\n",
            crate::about::version(),
            std::thread::current().name().unwrap_or("?"),
            if ui { " — the app stopped" } else { " — caught, the app went on" },
            std::backtrace::Backtrace::force_capture()
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
            use std::io::Write;
            let _ = f.write_all(report.as_bytes());
        }
        if ui && !cfg!(test) && std::env::var_os("CSVFAB_SHOT").is_none() {
            // try_lock: the panic may have come while the lock was held.
            if let Ok(g) = LATEST.try_lock() {
                let rdir = dir.join("recovery");
                let _ = std::fs::create_dir_all(&rdir);
                for (uid, (_, name, path, st)) in g.iter().flat_map(|m| m.iter()) {
                    let (data, meta) = files(&rdir, *uid);
                    let mut b = true;
                    if save::save(st, &data, &SaveOptions { backup: false, ..Default::default() }, None, &mut b).is_ok() {
                        let _ = std::fs::write(&meta, format!("{name}\n{}\n", path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()));
                    }
                }
            }
        }
        prev(info);
    }));
}

pub struct Recovery {
    /// None: off (tests, screenshots).
    dir: Option<PathBuf>,
    /// Per tab (uid): the version last seen and when it was first seen.
    seen: HashMap<u64, (u64, f64)>,
    /// Per tab: the version written, and when.
    written: HashMap<u64, (u64, f64)>,
    jobs: HashMap<u64, Job<()>>,
}

/// A copy left by a csvfab that is not running any more.
#[derive(Clone, Debug)]
pub struct Left {
    pub data: PathBuf,
    pub name: String,
    /// The file the edits were made to.
    pub original: Option<PathBuf>,
    pub when: SystemTime,
}

fn files(dir: &Path, uid: u64) -> (PathBuf, PathBuf) {
    let stem = format!("{}-{uid}", std::process::id());
    (dir.join(format!("{stem}.csv")), dir.join(format!("{stem}.meta")))
}

impl Recovery {
    pub fn new() -> Recovery {
        let on = !cfg!(test) && std::env::var_os("CSVFAB_SHOT").is_none();
        Recovery { dir: on.then(|| crate::config::dir().join("recovery")), seen: HashMap::new(), written: HashMap::new(), jobs: HashMap::new() }
    }

    /// For tests: copies kept in `dir`.
    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Recovery {
        Recovery { dir: Some(dir), seen: HashMap::new(), written: HashMap::new(), jobs: HashMap::new() }
    }

    /// Once a frame: writes what is due, removes what is no longer needed. Asks for one
    /// frame when a copy falls due (never more while idle).
    pub fn tick(&mut self, tabs: &[Tab], ctx: &egui::Context) {
        let Some(dir) = self.dir.clone() else { return };
        let now = ctx.input(|i| i.time);
        self.jobs.retain(|_, j| j.poll().is_none());
        let alive: Vec<u64> = tabs.iter().map(|t| t.uid).collect();
        // Closed tabs, saved or discarded edits: their copy goes.
        let gone: Vec<u64> = self.written.keys().copied().filter(|u| !alive.contains(u) || tabs.iter().any(|t| t.uid == *u && !t.dirty())).collect();
        for u in gone { self.forget(u); }
        self.seen.retain(|u, _| alive.contains(u));
        // The tables for the panic hook: refreshed when one changes (a State is a few Arcs).
        if let Ok(mut g) = LATEST.lock() {
            let m = g.get_or_insert_with(HashMap::new);
            m.retain(|u, _| tabs.iter().any(|t| t.uid == *u && t.dirty()));
            for t in tabs.iter().filter(|t| t.dirty()) {
                let Some(table) = &t.table else { continue };
                if m.get(&t.uid).is_none_or(|e| e.0 != table.version()) {
                    m.insert(t.uid, (table.version(), t.name.clone(), t.path.clone(), table.st.clone()));
                }
            }
        }
        for t in tabs {
            let Some(table) = t.table.as_ref().filter(|_| t.dirty()) else { continue };
            let v = table.version();
            if self.written.get(&t.uid).is_some_and(|w| w.0 == v) || self.jobs.contains_key(&t.uid) { continue; }
            let since = match self.seen.get(&t.uid) {
                Some(&(sv, at)) if sv == v => at,
                _ => {
                    self.seen.insert(t.uid, (v, now));
                    now
                }
            };
            let big = table.st.base.size > BIG;
            let last = self.written.get(&t.uid).map_or(f64::NEG_INFINITY, |w| w.1);
            let due = (since + QUIET).max(if big { last + BIG_EVERY } else { 0.0 });
            if now < due {
                ctx.request_repaint_after(Duration::from_secs_f64(due - now));
                continue;
            }
            let (data, meta) = files(&dir, t.uid);
            let st = table.st.clone();
            let info = format!("{}\n{}\n", t.name, t.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
            let dir2 = dir.clone();
            self.jobs.insert(t.uid, Job::spawn(ctx, move |_, _| {
                let _ = std::fs::create_dir_all(&dir2);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&dir2, std::fs::Permissions::from_mode(0o700));
                }
                let mut b = true;
                if save::save(&st, &data, &SaveOptions { backup: false, ..Default::default() }, None, &mut b).is_ok() {
                    let _ = std::fs::write(&meta, info);
                }
            }));
            self.written.insert(t.uid, (v, now));
        }
    }

    /// Tab `uid`'s copy is no longer needed.
    pub fn forget(&mut self, uid: u64) {
        self.written.remove(&uid);
        self.seen.remove(&uid);
        if let Some(dir) = &self.dir {
            let (data, meta) = files(dir, uid);
            let _ = std::fs::remove_file(meta);
            let _ = std::fs::remove_file(data);
        }
    }

    /// A clean quit: every copy of this run goes.
    pub fn clear(&mut self) {
        let uids: Vec<u64> = self.written.keys().copied().collect();
        for u in uids { self.forget(u); }
    }

    /// The copies left by csvfab runs that are over (not this one, nor one still open).
    pub fn leftovers(&self) -> Vec<Left> {
        let Some(dir) = &self.dir else { return Vec::new() };
        left_in(dir)
    }
}

pub fn left_in(dir: &Path) -> Vec<Left> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Left> = rd.flatten().filter_map(|e| {
        let p = e.path();
        if p.extension()? != "meta" { return None; }
        let pid: u32 = p.file_stem()?.to_str()?.split('-').next()?.parse().ok()?;
        if pid == std::process::id() || running(pid) { return None; }
        let data = p.with_extension("csv");
        let when = std::fs::metadata(&data).ok()?.modified().ok()?;
        let info = std::fs::read_to_string(&p).ok()?;
        let mut l = info.lines();
        let name = l.next().unwrap_or("").to_string();
        let original = l.next().filter(|s| !s.is_empty()).map(PathBuf::from);
        Some(Left { data, name, original, when })
    }).collect();
    out.sort_by_key(|l| std::cmp::Reverse(l.when));
    out
}

/// Whether process `pid` is a csvfab still running.
fn running(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim().starts_with("csvfab"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// A copy dropped (restored, or refused).
pub fn remove(l: &Left) {
    let _ = std::fs::remove_file(l.data.with_extension("meta"));
    let _ = std::fs::remove_file(&l.data);
}
