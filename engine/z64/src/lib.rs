//! z64: zstd-compressed files cut into independent 64 KiB blocks.
//!
//! A z64 file is a valid [zstd seekable] file: one zstd frame per block, then a
//! seek table stored in a skippable frame. `zstd -d` can therefore read it. An index of
//! the lines is stored in a skippable frame too.
//!
//! [zstd seekable]: https://github.com/facebook/zstd/tree/dev/contrib/seekable_format

mod format;
mod frame;
mod log;
#[cfg(feature = "python")]
mod python;
mod reader;
mod repair;
mod stream;
mod writer;

use std::io::{self, BufRead, Cursor};

pub use format::{BLOCK_SIZE, DEFAULT_LEVEL, Truncate};
pub use log::SharedLog;
pub use reader::Reader;
pub use repair::{RepairReport, repair};
pub use stream::StreamReader;
pub use writer::{Options, Writer};

/// Blocks handled together per thread. Larger batches keep the threads busy longer,
/// at the price of more data held in memory.
pub(crate) const BLOCKS_PER_THREAD: usize = 4;

/// Thread count for work on a whole buffer: `threads == 0` means all the cores here. The
/// cap of `resolve_threads` is for line by line use, where more threads only wait.
pub(crate) fn whole_buffer_threads(threads: usize) -> usize {
    match threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    }
}

/// Compresses a whole buffer into a z64 stream.
pub fn compress(data: &[u8], mut options: Options) -> io::Result<Vec<u8>> {
    use std::io::Write;
    options.threads = whole_buffer_threads(options.threads);
    let mut writer = Writer::new(Vec::with_capacity(data.len() / 2), options);
    writer.write_all(data)?;
    writer.finish()?;
    Ok(std::mem::take(writer.get_mut()))
}

/// Decompresses a whole z64 stream.
pub fn decompress(data: &[u8], threads: usize) -> io::Result<Vec<u8>> {
    let threads = whole_buffer_threads(threads);
    let mut reader = Reader::new(Cursor::new(data), threads)?;
    let mut out = vec![0; reader.len() as usize];
    reader.read_into(&mut out)?;
    reader.fill_buf()?; // reports a file without an end marker
    Ok(out)
}

/// Moves up to `limit` bytes from a reader to `out`, without an intermediate buffer.
pub(crate) fn drain<R: BufRead>(
    reader: &mut R,
    out: &mut Vec<u8>,
    limit: u64,
) -> io::Result<usize> {
    let start = out.len();
    loop {
        let missing = limit - (out.len() - start) as u64;
        if missing == 0 {
            return Ok(out.len() - start);
        }
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(out.len() - start);
        }
        let taken = available.len().min(missing.min(usize::MAX as u64) as usize);
        out.extend_from_slice(&available[..taken]);
        reader.consume(taken);
    }
}

/// Resolves a `threads` setting: 0 means all cores, capped at 8.
pub(crate) fn resolve_threads(threads: usize) -> usize {
    match threads {
        0 => std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(8),
        n => n,
    }
}

/// A thread pool built on first use, so that small files never start any thread.
pub(crate) struct LazyPool {
    threads: usize,
    built: Option<(u32, rayon::ThreadPool)>,
}

impl LazyPool {
    pub(crate) fn new(threads: usize) -> Self {
        Self {
            threads,
            built: None,
        }
    }

    /// Threads do not survive `fork()`: a pool inherited from a parent process would
    /// wait forever, so it is abandoned and replaced.
    pub(crate) fn get(&mut self) -> std::io::Result<&rayon::ThreadPool> {
        let pid = std::process::id();
        if self.built.as_ref().is_some_and(|(owner, _)| *owner != pid) {
            std::mem::forget(self.built.take());
        }
        if self.built.is_none() {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(self.threads)
                .build()
                .map_err(std::io::Error::other)?;
            self.built = Some((pid, pool));
        }
        Ok(&self.built.as_ref().expect("pool was just built").1)
    }
}

impl Drop for LazyPool {
    fn drop(&mut self) {
        if self
            .built
            .as_ref()
            .is_some_and(|(owner, _)| *owner != std::process::id())
        {
            std::mem::forget(self.built.take());
        }
    }
}
