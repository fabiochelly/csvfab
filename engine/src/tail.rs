//! Live tail (tail -f) of a raw text file (a log): checked now and then,
//! what was appended since is looked at alone, and when it holds complete
//! lines the file is taken again with them — new rows at the end, not
//! edits: the table stays clean, Save has nothing to write. Paused while
//! edits are pending (the rows would mix with them), and the file re-read
//! whole once they are saved or discarded; re-read too when it shrinks
//! (truncated), was replaced (rotated), or did not end with a line break.
//!
//! `Base` is immutable: the file is mapped again and only the bytes appended
//! are scanned (`Base::append`), the index of the lines known is kept. The
//! appended bytes alone decide whether there is anything to take.

use crate::base::{Base, OpenOptions, Stamp};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// What one check found.
pub enum Tail {
    /// Nothing new (or no complete line yet).
    Same,
    /// Edits pending: nothing added under them.
    Paused,
    /// Complete lines were appended: the file as it is now, `added` more rows at its end.
    Appended { base: Base, added: usize },
    /// The file as it is now, to show anew (it shrank, was replaced, ended
    /// mid-line, or edits were saved or discarded meanwhile).
    Reload(Base),
}

/// Why a table cannot be followed; None: it can.
pub fn cannot_follow(base: &Base) -> Option<&'static str> {
    if !base.dialect.lines {
        Some("Follow works on raw text files.")
    } else if base.path.is_none() {
        Some("This table is not a file on disk.")
    } else if base.enc.is_utf16() {
        Some("A UTF-16 file cannot be followed.")
    } else {
        None
    }
}

/// Follows one table: remembers whether edits paused it.
#[derive(Default)]
pub struct Follower {
    paused: bool,
}

impl Follower {
    pub fn new() -> Follower {
        Follower::default()
    }

    /// One check of `base`'s file. `dirty`: the table holds edits not saved.
    /// The caller shows the new base in a fresh, clean table (raw text: no
    /// header) and keeps the view at the bottom if it was there.
    pub fn step(&mut self, base: &Base, dirty: bool) -> io::Result<Tail> {
        let Some(path) = base.path.as_deref() else { return Ok(Tail::Same) };
        let reopen = || Base::open(path, OpenOptions { enc: Some(base.enc), delim: Some(0) }, None);
        if dirty {
            self.paused = true;
            return Ok(Tail::Paused);
        }
        if self.paused {
            // Saved or discarded: the bytes held may no longer be the file's.
            self.paused = false;
            return Ok(Tail::Reload(reopen()?));
        }
        let Some(now) = Stamp::of(path) else { return Err(io::Error::new(io::ErrorKind::NotFound, "the file is gone")) };
        let have = base.size;
        if now.len == have && base.stamp.is_none_or(|s| s.ino == now.ino) { return Ok(Tail::Same); }
        let nl = if base.eol == b"\r" { b'\r' } else { b'\n' };
        let last = base.bytes().last().copied().unwrap_or(nl);
        let replaced = base.stamp.is_some_and(|s| s.ino != now.ino);
        if now.len < have || replaced || (last != b'\n' && last != b'\r') {
            return Ok(Tail::Reload(reopen()?));
        }
        // Only what was appended is read.
        let mut f = File::open(path)?;
        f.seek(SeekFrom::Start(have))?;
        let mut chunk = Vec::with_capacity((now.len - have) as usize);
        f.take(now.len - have).read_to_end(&mut chunk)?;
        if memchr::memchr(nl, &chunk).is_none() { return Ok(Tail::Same); }   // no complete line yet
        // The lines appended indexed after the ones known; the whole file only if that cannot be.
        let new = match base.append()? { Some(b) => b, None => reopen()? };
        // The file may have grown again meanwhile: the new count is the truth.
        let added = new.n.saturating_sub(base.n);
        Ok(Tail::Appended { base: new, added })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(p: &std::path::Path, b: &[u8]) {
        std::fs::OpenOptions::new().append(true).open(p).unwrap().write_all(b).unwrap();
    }

    #[test]
    #[cfg_attr(windows, ignore = "rotates the log in place while it is mapped: Windows forbids it to anyone")]
    fn follows_a_log() {
        let dir = std::env::temp_dir().join(format!("csvfab-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("app.log");
        std::fs::write(&p, b"start\nsecond line, with a comma\n").unwrap();
        let open = || Base::open(&p, OpenOptions { enc: None, delim: Some(0) }, None).unwrap();
        let mut base = open();
        assert!(cannot_follow(&base).is_none());
        assert_eq!(base.n, 2);
        let mut f = Follower::new();
        assert!(matches!(f.step(&base, false).unwrap(), Tail::Same));

        append(&p, b"third\nfourth\n");
        match f.step(&base, false).unwrap() {
            Tail::Appended { base: b, added } => { assert_eq!(added, 2); assert_eq!(b.n, 4); assert_eq!(b.record(3), b"fourth"); base = b; }
            _ => panic!("expected lines appended"),
        }
        append(&p, b"unfinis");
        assert!(matches!(f.step(&base, false).unwrap(), Tail::Same));    // waits for its line break
        append(&p, b"hed\n");
        match f.step(&base, false).unwrap() {
            Tail::Appended { base: b, added } => { assert_eq!(added, 1); assert_eq!(b.record(4), b"unfinished"); base = b; }
            _ => panic!("expected a line appended"),
        }
        // Edits pending: paused; once saved or discarded, read again.
        append(&p, b"x\n");
        assert!(matches!(f.step(&base, true).unwrap(), Tail::Paused));
        match f.step(&base, false).unwrap() { Tail::Reload(b) => { assert_eq!(b.n, 6); base = b; } _ => panic!("expected a reload") }
        // Truncated: read again.
        std::fs::write(&p, b"new\n").unwrap();
        match f.step(&base, false).unwrap() { Tail::Reload(b) => assert_eq!(b.n, 1), _ => panic!("expected a reload") }
        let csv = Base::from_bytes(b"a,b\n1,2\n".to_vec(), OpenOptions::default());
        assert!(cannot_follow(&csv).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
