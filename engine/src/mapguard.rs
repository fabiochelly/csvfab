//! A file read through a memory map, safe from another program cutting it short.
//!
//! The map is what makes opening instant: the bytes are read where they lie, never
//! copied. But when another program truncates the file while it is open (a script's
//! `>`, an editor writing in place, a log rotated by truncation, a network share
//! dropping), every read past the new end raises SIGBUS, and the process dies with the
//! edits not yet saved. Here a SIGBUS handler catches the faults that land in a map of
//! ours: the pages past the end are replaced by zero pages, the read carries on (it
//! sees NUL bytes) and the map is marked `cut_short`, for the app to tell the user and
//! refuse to write the damaged bytes back. Faults elsewhere go to the handler that was
//! there before (Rust's own, then the default: the crash, as without this).
//!
//! Windows does not let a mapped file shrink: nothing to catch there.

use memmap2::Mmap;
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The maps alive, for the handler: lock-free slots it can read from a signal.
struct Slot {
    used: AtomicBool,
    start: AtomicUsize,
    end: AtomicUsize,
    cut: AtomicBool,
}

const SLOTS: usize = 1024;
#[allow(clippy::declare_interior_mutable_const)]
const FREE: Slot = Slot { used: AtomicBool::new(false), start: AtomicUsize::new(0), end: AtomicUsize::new(0), cut: AtomicBool::new(false) };
static TABLE: [Slot; SLOTS] = [FREE; SLOTS];
/// How many maps were found cut short since the start: the app polls it each frame.
static CUTS: AtomicUsize = AtomicUsize::new(0);

/// A read-only map of a whole file, its pages replaced by zeros past the end if the
/// file shrinks under it.
pub struct Guarded {
    /// Dropped by hand, out of the watch first, then the files waiting for it deleted.
    map: std::mem::ManuallyDrop<Mmap>,
    slot: Option<usize>,
}

impl Guarded {
    /// Maps `f` whole (`len` > 0) and puts the map under watch.
    pub fn map(f: &File) -> std::io::Result<Guarded> {
        // SAFETY: read-only; the file shrinking under it is what the handler catches.
        let map = unsafe { Mmap::map(f)? };
        let slot = watch(map.as_ptr() as usize, map.len());
        Ok(Guarded { map: std::mem::ManuallyDrop::new(map), slot })
    }

    /// Whether another program cut the file short while it was mapped: the bytes past
    /// its new end read as zeros, not as they were.
    pub fn cut_short(&self) -> bool {
        self.slot.is_some_and(|s| TABLE[s].cut.load(Ordering::Acquire))
    }

    #[cfg(unix)]
    pub fn advise(&self, a: memmap2::Advice) -> std::io::Result<()> {
        self.map.advise(a)
    }
}

impl std::ops::Deref for Guarded {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.map
    }
}

impl Drop for Guarded {
    /// Out of the table before the map goes; then the files left for it to let go of.
    fn drop(&mut self) {
        if let Some(s) = self.slot {
            let t = &TABLE[s];
            t.start.store(0, Ordering::Release);
            t.end.store(0, Ordering::Release);
            t.cut.store(false, Ordering::Release);
            t.used.store(false, Ordering::Release);
        }
        // SAFETY: dropped once, here; the field is not used after.
        unsafe { std::mem::ManuallyDrop::drop(&mut self.map) };
        delete_waiting();
    }
}

/// Files to delete once no map holds them: on Windows, a file another took the place of
/// while this app still mapped it (save.rs `replace`).
static WAITING: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// `path` is deleted as soon as it can be: now if nothing maps it, else when its map goes
/// (or at the latest by `delete_waiting` when the app quits).
pub fn delete_later(path: std::path::PathBuf) {
    if std::fs::remove_file(&path).is_ok() { return; }
    if let Ok(mut w) = WAITING.lock() { w.push(path); }
}

/// Deletes the files waiting that nothing holds any more.
pub fn delete_waiting() {
    let Ok(mut w) = WAITING.try_lock() else { return };
    w.retain(|p| p.exists() && std::fs::remove_file(p).is_err());
}

/// How many maps were found cut short so far (it only grows): a change tells the app
/// to look at its tabs.
pub fn cuts() -> usize {
    CUTS.load(Ordering::Acquire)
}

fn watch(start: usize, len: usize) -> Option<usize> {
    if len == 0 || !install() {
        return None;
    }
    let s = TABLE.iter().position(|t| t.used.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok())?;
    let t = &TABLE[s];
    t.cut.store(false, Ordering::Release);
    t.end.store(start + len, Ordering::Release);
    t.start.store(start, Ordering::Release);
    Some(s)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod handler {
    use super::{CUTS, TABLE};
    use std::sync::atomic::Ordering;
    use std::sync::Once;

    static mut PREV: std::mem::MaybeUninit<libc::sigaction> = std::mem::MaybeUninit::uninit();
    static mut PAGE: usize = 4096;
    static ONCE: Once = Once::new();
    static mut OK: bool = false;

    pub fn install() -> bool {
        ONCE.call_once(|| unsafe {
            let p = libc::sysconf(libc::_SC_PAGESIZE);
            if p > 0 { PAGE = p as usize; }
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_bus as *const () as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigemptyset(&mut sa.sa_mask);
            OK = libc::sigaction(libc::SIGBUS, &sa, (&raw mut PREV).cast()) == 0;
        });
        // SAFETY: written once, inside call_once, before any reader.
        unsafe { OK }
    }

    /// Runs on the thread that read past the end. Only async-signal-safe work: atomics
    /// and mmap.
    extern "C" fn on_bus(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut libc::c_void) {
        // SAFETY: the kernel hands a valid siginfo for SA_SIGINFO handlers.
        let addr = unsafe { (*info).si_addr() } as usize;
        let page = unsafe { PAGE };
        for t in TABLE.iter() {
            let start = t.start.load(Ordering::Acquire);
            let end = t.end.load(Ordering::Acquire);
            if start == 0 || addr < start || addr >= end {
                continue;
            }
            // From the faulting page to the map's end: zero pages, read-only, in place.
            let lo = addr & !(page - 1);
            let hi = (end + page - 1) & !(page - 1);
            // SAFETY: the range lies inside a live map of ours (in the table until it is
            // unmapped); MAP_FIXED replaces its pages, munmap later frees them alike.
            let r = unsafe { libc::mmap(lo as *mut libc::c_void, hi - lo, libc::PROT_READ, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED, -1, 0) };
            if r != libc::MAP_FAILED {
                if !t.cut.swap(true, Ordering::AcqRel) {
                    CUTS.fetch_add(1, Ordering::AcqRel);
                }
                return;
            }
        }
        // Not ours: the handler from before, or back to it and fault again.
        unsafe {
            let prev = &*(&raw const PREV).cast::<libc::sigaction>();
            if prev.sa_flags & libc::SA_SIGINFO != 0 && prev.sa_sigaction > libc::SIG_IGN {
                let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) = std::mem::transmute(prev.sa_sigaction);
                f(sig, info, ctx);
            } else {
                libc::sigaction(libc::SIGBUS, prev, std::ptr::null_mut());
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
use handler::install;

/// Elsewhere: macOS reports these faults differently, Windows never lets a mapped file shrink.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn install() -> bool {
    false
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::Write;

    /// A file truncated by another program while mapped: the read goes on (zeros past the
    /// new end), the map says it was cut short, and the process lives.
    #[test]
    fn cut_short_reads_zeros() {
        let dir = std::env::temp_dir().join(format!("csvfab-mapguard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.csv");
        let mut f = File::create(&path).unwrap();
        let line = b"abcdefghij,0123456789\n";
        for _ in 0..20_000 { f.write_all(line).unwrap(); }
        drop(f);
        let g = Guarded::map(&File::open(&path).unwrap()).unwrap();
        assert_eq!(&g[..line.len()], line);
        let before = cuts();
        // Another program: cut to 1000 bytes.
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(1000).unwrap();
        let tail: u64 = g[300_000..].iter().map(|&b| b as u64).sum();
        assert_eq!(tail, 0);
        assert!(g.cut_short());
        assert!(cuts() > before);
        assert_eq!(&g[..line.len()], line, "the bytes still in the file read as they are");
        drop(g);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
