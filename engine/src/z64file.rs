//! `.z64` files (zstd in independent blocks of 64 KiB, a table of them at the end) opened and
//! saved as they are: no CSV beside them, the tab stands on the `.z64`. A table needs its bytes
//! end to end and an index of every record, so the whole content is unpacked once, on every core,
//! and everything after that reads it as it reads a file mapped.
//!
//! Where the content goes: anonymous memory, when it fits; else an unnamed file, whose pages the
//! system can drop, as it drops a mapped file's. Nothing is left behind either way.
//!
//! The blocks give what a `.gz` cannot: the size before anything is unpacked, and the first rows
//! at once (`head`).

use crate::base::Data;
use crate::mapguard::Guarded;
use memmap2::{Mmap, MmapMut};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// A share of the file unpacked at a time: for the progress shown, and because the blocks of one
/// read are held compressed in memory first. A whole number of blocks: no copy between two.
const STEP: usize = 1024 * z64::BLOCK_SIZE;

/// Where it does not know the free memory: as much as any machine has to spare.
const MEMORY_UNKNOWN: u64 = 4 << 30;

const SUFFIX: &str = ".z64";

/// `name` without its `.z64`, if it ends in it: `data.csv.z64` → `data.csv`.
pub fn strip(name: &str) -> Option<&str> {
    let k = name.len().checked_sub(SUFFIX.len()).filter(|&k| k > 0 && name.is_char_boundary(k))?;
    name[k..].eq_ignore_ascii_case(SUFFIX).then(|| &name[..k])
}

/// The name of what the file holds: `data.csv.z64` → `data.csv`, any other name as it is.
pub fn inner_name(name: &str) -> &str {
    strip(name).unwrap_or(name)
}

pub fn is_z64(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| strip(n).is_some())
}

/// Every core: a table is unpacked or written whole, in one go.
fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// The size of what the file holds, read from its table.
pub fn content_len(path: &Path) -> io::Result<u64> {
    Ok(z64::Reader::new(File::open(path)?, 1)?.len())
}

/// The first `want` bytes of what the file holds, one thread, only the blocks they lie in. None
/// when that is all there is: the file is then read whole at once.
pub fn head(path: &Path, want: usize) -> io::Result<Option<Vec<u8>>> {
    let mut r = z64::Reader::new(File::open(path)?, 1)?;
    if r.len() <= want as u64 { return Ok(None); }
    let mut buf = vec![0; want];
    r.read_into(&mut buf)?;
    Ok(Some(buf))
}

/// A file's content, unpacked.
pub struct Unpacked {
    pub data: Data,
    pub size: u64,
    /// The file has no end marker: its writer stopped before the end. Every complete block is here.
    pub unfinished: bool,
}

/// Unpacks `path`. Progress: bytes unpacked. A block that does not match its checksum is an
/// error: nothing is shown that the file does not hold.
pub fn read(path: &Path, progress: Option<&AtomicUsize>) -> io::Result<Unpacked> {
    let spill = |size| !fits_in_memory(size);
    unpack(path, progress, spill)
}

fn unpack(path: &Path, progress: Option<&AtomicUsize>, spill: impl FnOnce(u64) -> bool) -> io::Result<Unpacked> {
    let f = File::open(path)?;
    let packed = f.metadata()?.len();
    let mut r = z64::Reader::new(f, threads())?;
    let size = r.len();
    let unfinished = r.is_truncated();
    if size == 0 { return Ok(Unpacked { data: Data::Owned(Arc::new(Vec::new())), size, unfinished }); }
    if size > crate::convert::packed::RATIO_FROM && size > crate::convert::packed::MAX_RATIO * packed.max(1) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("it decompresses more than {} times over: a booby-trapped archive", crate::convert::packed::MAX_RATIO)));
    }
    let map = if spill(size) { to_file(&mut r, path, size, progress)? } else { to_memory(&mut r, size, progress)? };
    let map = Guarded::of(map);
    Ok(Unpacked { data: Data::Map(Arc::new(map)), size, unfinished })
}

/// Whether `size` bytes can be held without pushing the rest of the machine out: the system
/// would swap, and a table's pages cannot be dropped once written to anonymous memory.
fn fits_in_memory(size: u64) -> bool {
    size <= crate::colcache::meminfo().map_or(MEMORY_UNKNOWN, |(_, available)| available / 2)
}

fn step(r: &mut z64::Reader<File>, out: &mut [u8], progress: Option<&AtomicUsize>) -> io::Result<()> {
    if r.read_into(out)? != out.len() { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the z64 file ends before its table says")); }
    if let Some(p) = progress { p.fetch_add(out.len(), Ordering::Relaxed); }
    Ok(())
}

/// The blocks decoded right into the pages (no copy, nothing zeroed first).
fn to_memory(r: &mut z64::Reader<File>, size: u64, progress: Option<&AtomicUsize>) -> io::Result<Mmap> {
    let mut m = MmapMut::map_anon(size as usize)?;
    #[cfg(target_os = "linux")]
    let _ = m.advise(memmap2::Advice::HugePage);
    for chunk in m.chunks_mut(STEP) { step(r, chunk, progress)?; }
    m.make_read_only()
}

/// Written, then mapped: a disk that fills up is an error here, where a write through a map
/// would be a SIGBUS.
fn to_file(r: &mut z64::Reader<File>, path: &Path, size: u64, progress: Option<&AtomicUsize>) -> io::Result<Mmap> {
    let mut f = unnamed_file(path)?;
    let mut buf = vec![0; STEP];
    let mut left = size;
    while left > 0 {
        let n = left.min(STEP as u64) as usize;
        step(r, &mut buf[..n], progress)?;
        f.write_all(&buf[..n])?;
        left -= n as u64;
    }
    // SAFETY: read-only, and no other program can reach a file without a name.
    unsafe { Mmap::map(&f) }
}

/// Where the unnamed file goes. Beside the file on Linux, where the temporary folder is often
/// memory itself, which is what this avoids.
fn spill_dirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "linux") {
        dirs.extend(path.parent().map(Path::to_path_buf).filter(|d| crate::convert::dir_writable(d)));
    }
    dirs.push(std::env::temp_dir());
    dirs
}

/// A file only this process holds, gone with its last handle (a crash leaves nothing).
fn unnamed_file(path: &Path) -> io::Result<File> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut last = io::Error::other("no folder to unpack to");
    for dir in spill_dirs(path) {
        let tmp = dir.join(format!(".csvfab-{}-{}.z64-unpacked", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
            const SHARE_ALL: u32 = 7;
            o.custom_flags(FILE_FLAG_DELETE_ON_CLOSE).share_mode(SHARE_ALL);
        }
        match o.open(&tmp) {
            Ok(f) => {
                #[cfg(unix)]
                std::fs::remove_file(&tmp)?;
                return Ok(f);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// What a save writes through, on every core, at zstd's fastest regular level: the format's own
/// default, the one its author measured.
pub fn writer(f: File) -> z64::Writer<File> {
    z64::Writer::new(f, z64::Options { level: z64::DEFAULT_LEVEL, threads: threads() })
}

/// Seals what `writer` wrote and puts it on the disk.
pub fn finish(mut w: z64::Writer<File>) -> io::Result<()> {
    w.finish()?;
    w.get_ref().sync_all()
}

#[cfg(test)]
pub(crate) fn unpack_to_file(path: &Path) -> io::Result<Unpacked> {
    unpack(path, None, |_| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(strip("data.csv.z64"), Some("data.csv"));
        assert_eq!(strip("DATA.Z64"), Some("DATA"));
        assert_eq!(strip("data.csv"), None);
        assert_eq!(strip(".z64"), None);
        assert_eq!(strip("é.z64"), Some("é"));
        assert_eq!(inner_name("a.tsv.z64"), "a.tsv");
        assert_eq!(inner_name("a.tsv"), "a.tsv");
        assert!(is_z64(Path::new("/x/a.csv.Z64")));
        assert!(!is_z64(Path::new("/x/z64")));
    }

    /// In memory or in a file, the same bytes; the file is gone from its folder at once.
    #[test]
    fn both_ways_give_the_same_bytes() {
        let dir = std::env::temp_dir().join(format!("csvfab-z64file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.csv.z64");
        let body: Vec<u8> = (0..400_000u32).flat_map(|i| format!("{i},row {}\n", i * 7).into_bytes()).collect();
        std::fs::write(&path, z64::compress(&body, z64::Options::default()).unwrap()).unwrap();
        let (a, b) = (read(&path, None).unwrap(), unpack_to_file(&path).unwrap());
        assert_eq!(&*a.data, &body[..]);
        assert_eq!(&*b.data, &body[..]);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing beside the file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
