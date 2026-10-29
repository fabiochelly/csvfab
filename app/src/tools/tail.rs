//! Follow the end of a growing file (tail -f, 36-tail.js), raw text only (a
//! log): checked every second, in a job; complete lines appended show as new
//! rows at the end — not edits: the tab stays clean. The view follows the end
//! when it was at the bottom; the filters apply to the new lines. Paused
//! while edits are pending, the file read again once they are saved or
//! discarded; read again too when it shrinks or is replaced.
//!
//! A tool without a window stays open on the tab while it is followed;
//! `state(uid)` tells the status bar what to show, `toggle` turns it on or off.

use super::{Cmd, Command, Cx, Tool};
use crate::job::Job;
use crate::tab::Tab;
use csvfab_engine::tail::{self, Follower, Tail};
use csvfab_engine::Table;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TAIL_MS: u64 = 1000;

/// What the status bar shows of a followed tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Follow {
    On,
    /// Edits pending: nothing is added under them.
    Paused,
    /// The last check failed (the file is gone…).
    Error,
}

/// Followed tabs: their state and the token of the tool following them (a
/// tool turned off and on again before it was dropped must not run twice).
static FOLLOWED: Mutex<Option<HashMap<u64, (Follow, u64)>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashMap<u64, (Follow, u64)>) -> R) -> R {
    let mut g = FOLLOWED.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// Whether the tab `uid` is followed, and how.
pub fn state(uid: u64) -> Option<Follow> {
    with(|m| m.get(&uid).map(|s| s.0))
}

fn can_follow(t: &Tab) -> bool {
    t.st().is_some_and(|st| tail::cannot_follow(&st.base).is_none())
}

pub fn commands() -> Vec<Command> {
    vec![
        Command { name: "Follow the end of the file (tail -f)", group: "View", keys: None, enabled: |t| can_follow(t) && state(t.uid).is_none(), run: |cx| { if let Some(t) = toggle(cx) { cx.out.push(Cmd::Tool(t)) } } },
        Command { name: "Stop following the file", group: "View", keys: None, enabled: |t| state(t.uid).is_some(), run: |cx| { toggle(cx); } },
    ]
}

/// Starts following the tab (the tool to open), or stops (None). Says why
/// when the tab cannot be followed.
pub fn toggle(cx: &mut Cx) -> Option<Box<dyn Tool>> {
    let uid = cx.tab.uid;
    if with(|m| m.remove(&uid)).is_some() {
        cx.toast(format!("{} | No longer following the file.", cx.tab.name));
        return None;
    }
    let st = cx.tab.st()?;
    if let Some(why) = tail::cannot_follow(&st.base) {
        let more = if !st.base.dialect.lines { "\n\nRead the file as Raw text first (the delimiter menu of the status bar)." } else { "" };
        cx.toast(format!("{why}{}", more.replace("\n\n", " ")));
        return None;
    }
    static TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let token = TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    with(|m| m.insert(uid, (Follow::On, token)));
    cx.toast(format!("{} | Following the end of the file: new lines appear as they are written.", cx.tab.name));
    // Starting at the end.
    cx.tab.grid.scroll = f64::MAX / 4.0;
    Some(Box::new(Tailer { uid, token, follower: Arc::new(Mutex::new(Follower::new())), job: None, next: Instant::now() }))
}

struct Tailer {
    uid: u64,
    token: u64,
    follower: Arc<Mutex<Follower>>,
    job: Option<Job<std::io::Result<Tail>>>,
    next: Instant,
}

impl Drop for Tailer {
    fn drop(&mut self) {
        let (uid, token) = (self.uid, self.token);
        with(|m| { if m.get(&uid).is_some_and(|s| s.1 == token) { m.remove(&uid); } });
    }
}

impl Tool for Tailer {
    fn show(&mut self, cx: &mut Cx) -> bool {
        let (uid, token) = (self.uid, self.token);
        if cx.tab.uid != uid || !with(|m| m.get(&uid).is_some_and(|s| s.1 == token)) { return false; }
        let set = |f: Follow| with(|m| { if let Some(s) = m.get_mut(&uid) { s.0 = f; } });
        if let Some(r) = self.job.as_mut().and_then(|j| j.poll()) {
            self.job = None;
            self.next = Instant::now() + Duration::from_millis(TAIL_MS);
            match r {
                Ok(Tail::Same) => set(Follow::On),
                Ok(Tail::Paused) => set(Follow::Paused),
                Ok(Tail::Appended { base, .. } | Tail::Reload(base)) => {
                    set(Follow::On);
                    let tab = &mut *cx.tab;
                    // At the bottom, the view stays there as the rows come.
                    let bottom = tab.grid.top + tab.grid.page + 2 >= tab.shown();
                    if !tab.dirty() {
                        tab.table = Some(Table::new(Arc::new(base), Some(false)));
                        tab.after_edit(true, cx.ctx);
                        if bottom { tab.grid.scroll = f64::MAX / 4.0; }
                    }
                }
                Err(e) => {
                    if state(self.uid) != Some(Follow::Error) { cx.toast(format!("{} | Follow: {e}", cx.tab.name)); }
                    set(Follow::Error);
                }
            }
        }
        if self.job.is_none() && Instant::now() >= self.next && !cx.tab.busy() {
            if let Some(st) = cx.tab.st() {
                let (base, dirty, f) = (st.base.clone(), cx.tab.dirty(), self.follower.clone());
                self.job = Some(Job::spawn(cx.ctx, move |_, _| f.lock().unwrap_or_else(|e| e.into_inner()).step(&base, dirty)));
            }
        }
        cx.ctx.request_repaint_after(self.next.saturating_duration_since(Instant::now()).max(Duration::from_millis(50)));
        true
    }
}
