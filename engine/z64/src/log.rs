//! Several processes appending to the same file.
//!
//! Each commit is one self-contained segment, `[frames][line index][seek table]`, added
//! at the end of the file under a lock held only for that write. Compression happens
//! before, without the lock, so writers do not wait for each other's work, and the cost
//! of a commit does not depend on the size of the file. Segments are never modified: the
//! file is valid after each commit, and readers never need to exclude writers for long.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::format::{SEEKABLE_FOOTER_MAGIC, invalid};

/// How long a commit waits for the others. Far longer than any commit: only a writer in
/// mode `w`, which keeps the lock until it closes, can reach it.
pub(crate) const COMMIT_WAIT: Duration = Duration::from_secs(60);
/// Commits are short, so this only has to survive a brief collision.
#[cfg_attr(not(feature = "python"), allow(dead_code))]
pub(crate) const OPEN_WAIT: Duration = Duration::from_millis(200);
const FIRST_PAUSE: Duration = Duration::from_micros(20);
const LONGEST_PAUSE: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockKind {
    #[cfg_attr(not(feature = "python"), allow(dead_code))]
    Shared,
    Exclusive,
}

/// Waits for the lock by polling: the standard library has no timeout on `File::lock`,
/// and waiting forever would hang a process that holds the same file open in mode `w`.
pub(crate) fn lock_file(file: &File, kind: LockKind, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    let mut pause = FIRST_PAUSE;
    loop {
        let attempt = match kind {
            LockKind::Shared => file.try_lock_shared(),
            LockKind::Exclusive => file.try_lock(),
        };
        match attempt {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
            Err(std::fs::TryLockError::WouldBlock) if start.elapsed() >= timeout => {
                return Err(io::Error::new(
                    ErrorKind::WouldBlock,
                    "the file is locked by another writer",
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                sleep(pause);
                pause = (pause * 2).min(LONGEST_PAUSE);
            }
        }
    }
}

fn open_for_append(path: &PathBuf) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Returns the length of a file that can be appended to.
fn checked_length(file: &mut File) -> io::Result<u64> {
    let length = file.seek(SeekFrom::End(0))?;
    if length == 0 {
        return Ok(0);
    }
    let mut tail = [0u8; 4];
    if length >= 4 {
        file.seek(SeekFrom::End(-4))?;
        file.read_exact(&mut tail)?;
    }
    if u32::from_le_bytes(tail) != SEEKABLE_FOOTER_MAGIC {
        return Err(invalid(
            "the file does not end with a complete z64 segment, repair it first",
        ));
    }
    Ok(length)
}

fn append(file: &mut File, data: &[u8]) -> io::Result<()> {
    let length = checked_length(file)?;
    file.seek(SeekFrom::Start(length))?;
    file.write_all(data).inspect_err(|_| {
        // A disk that is full must not leave a half-written segment for everyone else.
        let _ = file.set_len(length);
    })
}

/// A file where each `write` is one atomic commit. Use it through [`Writer::segmented`].
///
/// [`Writer::segmented`]: crate::Writer::segmented
pub struct SharedLog {
    file: File,
    path: PathBuf,
    pid: u32,
}

impl SharedLog {
    /// Opens or creates a log. A file that does not end cleanly is refused.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        let mut file = open_for_append(&path)?;
        lock_file(&file, LockKind::Exclusive, COMMIT_WAIT)?;
        let checked = checked_length(&mut file);
        file.unlock()?;
        checked?;
        Ok(Self {
            file,
            path,
            pid: std::process::id(),
        })
    }

    fn commit(&mut self, data: &[u8]) -> io::Result<()> {
        let pid = std::process::id();
        if pid != self.pid {
            // A lock belongs to the open file, which `fork` shares: without a new one,
            // a parent and its child would not exclude each other.
            self.file = open_for_append(&self.path)?;
            self.pid = pid;
        }
        lock_file(&self.file, LockKind::Exclusive, COMMIT_WAIT)?;
        let result = append(&mut self.file, data);
        let released = self.file.unlock();
        result.and(released)
    }
}

impl Write for SharedLog {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.commit(data)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
