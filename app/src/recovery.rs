//! Edits not saved yet, kept on disk in case csvfab stops without asking (a crash, the
//! machine going down): each tab with unsaved edits is written in the background to
//! `<state dir>/recovery/` a few seconds after its last edit, and removed once saved,
//! discarded or closed. A table standing on its file as it still is on disk keeps a journal
//! of its edits (`csvfab_engine::journal`: a few bytes per edited cell), replayed over the
//! file; any other (no file behind it, or the file changed since it was read) a copy of the
//! whole table (a big file then less often). A start that finds what a csvfab no longer
//! running left offers it back: a journal as the file with its edits, a copy as a new tab.

use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::save::{self, SaveOptions};
use csvfab_engine::{journal, OpenOptions, Stamp, State};
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Quiet time after an edit before the copy is written; a copy of a file past `BIG` bytes
/// waits `BIG_EVERY` between copies (each writes the whole table; a journal is small).
const QUIET: f64 = 3.0;
const BIG: u64 = 100 << 20;
const BIG_EVERY: f64 = 30.0;

/// The unsaved tables as of the last edit, for the panic hook: a crash on the UI thread
/// writes them at once (nothing lost), the copies above being for a hard stop.
type Latest = HashMap<u64, (u64, String, Option<PathBuf>, OpenOptions, State)>;
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
                for (uid, (_, name, path, opts, st)) in g.iter().flat_map(|m| m.iter()) {
                    keep(&rdir, *uid, name, path.as_deref(), *opts, st);
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

/// What a csvfab that is not running any more left: a journal or a copy.
#[derive(Clone, Debug)]
pub struct Left {
    /// The journal (`.journal`) or the copy (`.csv`).
    pub data: PathBuf,
    pub name: String,
    /// The file the edits were made to.
    pub original: Option<PathBuf>,
    pub when: SystemTime,
}

/// Tab `uid`'s copy, journal and note (its name, its file) in `dir`.
fn files(dir: &Path, uid: u64) -> (PathBuf, PathBuf, PathBuf) {
    let stem = format!("{}-{uid}", std::process::id());
    (dir.join(format!("{stem}.csv")), dir.join(format!("{stem}.journal")), dir.join(format!("{stem}.meta")))
}

/// Whether a journal can stand for the table: it stands on `path` as read whole, and the file
/// is still that on disk (else the edits' rows would be other rows).
fn journaled(st: &State, path: Option<&Path>) -> bool {
    let b = &st.base;
    path.is_some_and(|p| b.path.as_deref() == Some(p) && b.mapped() && !b.partial && !b.cut_short() && b.stamp.is_some() && Stamp::of(p) == b.stamp)
}

/// Writes tab `uid`'s unsaved edits to `dir`: its journal, or a copy of the table (the other
/// removed), then the note that offers them back. A journal is written aside, then renamed:
/// a stop meanwhile leaves the one before.
fn keep(dir: &Path, uid: u64, name: &str, path: Option<&Path>, opts: OpenOptions, st: &State) {
    let (copy, jpath, meta) = files(dir, uid);
    let (ok, other) = if journaled(st, path) {
        let tmp = jpath.with_extension("journal.tmp");
        let res = (|| -> std::io::Result<()> {
            let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
            journal::write(st, opts, &mut w)?;
            w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            std::fs::rename(&tmp, &jpath)
        })();
        if res.is_err() { let _ = std::fs::remove_file(&tmp); }
        (res.is_ok(), copy)
    } else {
        (save::save(st, &copy, &SaveOptions { backup: false, ..Default::default() }, None, &mut true).is_ok(), jpath)
    };
    if ok {
        let _ = std::fs::remove_file(other);
        let _ = std::fs::write(&meta, format!("{name}\n{}\n", path.map(|p| p.display().to_string()).unwrap_or_default()));
    }
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
                    m.insert(t.uid, (table.version(), t.name.clone(), t.path.clone(), t.open_opts, table.st.clone()));
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
            // A whole copy of a big file is not rewritten every few seconds (a journal is small).
            let b = &table.st.base;
            let big = b.size > BIG && !(b.mapped() && !b.partial && b.path.is_some() && b.path == t.path);
            let last = self.written.get(&t.uid).map_or(f64::NEG_INFINITY, |w| w.1);
            let due = (since + QUIET).max(if big { last + BIG_EVERY } else { 0.0 });
            if now < due {
                ctx.request_repaint_after(Duration::from_secs_f64(due - now));
                continue;
            }
            let (st, uid, name, path, opts, dir2) = (table.st.clone(), t.uid, t.name.clone(), t.path.clone(), t.open_opts, dir.clone());
            self.jobs.insert(t.uid, Job::spawn(ctx, move |_, _| {
                let _ = std::fs::create_dir_all(&dir2);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&dir2, std::fs::Permissions::from_mode(0o700));
                }
                keep(&dir2, uid, &name, path.as_deref(), opts, &st);
            }));
            self.written.insert(t.uid, (v, now));
        }
    }

    /// Tab `uid`'s copy is no longer needed.
    pub fn forget(&mut self, uid: u64) {
        self.written.remove(&uid);
        self.seen.remove(&uid);
        if let Some(dir) = &self.dir {
            let (copy, jpath, meta) = files(dir, uid);
            for p in [meta, copy, jpath] { let _ = std::fs::remove_file(p); }
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
        let data = Some(p.with_extension("journal")).filter(|j| j.exists()).unwrap_or_else(|| p.with_extension("csv"));
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

/// What `l` gives back as a tab: a journal replayed over its file (when that file is still
/// the one the edits were made on), a copy as a new tab. Unsaved: closing or quitting asks,
/// and the edits are kept aside again.
pub fn restore(l: &Left, bytes: Vec<u8>, ctx: &egui::Context, m: &crate::theme::Metrics) -> Result<Tab, String> {
    if l.data.extension().is_some_and(|e| e == "journal") {
        let (opts, stamp) = journal::header(&bytes).ok_or("the recovery file is damaged: its edits are lost")?;
        let path = l.original.clone().ok_or("no file behind these edits")?;
        if Stamp::of(&path) != Some(stamp) {
            return Err(format!("{} changed on disk (or is gone) since these edits were made: they cannot be put back on it", path.display()));
        }
        return Ok(Tab::recover(path, opts, bytes, ctx));
    }
    let mut t = Tab::memory(format!("{} (recovered)", l.name), bytes, ctx, m);
    if let Some(x) = &mut t.table { x.mark_saved_at(u64::MAX); }
    Ok(t)
}

/// A copy dropped (restored, or refused).
pub fn remove(l: &Left) {
    let _ = std::fs::remove_file(l.data.with_extension("meta"));
    let _ = std::fs::remove_file(&l.data);
}
