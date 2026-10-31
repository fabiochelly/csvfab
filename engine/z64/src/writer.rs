use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, mpsc};

use rayon::prelude::*;

use crate::format::{
    BLOCK_SIZE, DEFAULT_LEVEL, Entry, MAX_ENTRIES, Truncate, read_layout, write_trailer,
};
use crate::frame::{compress_into, compress_to_vec};
use crate::{BLOCKS_PER_THREAD, LazyPool, resolve_threads};

/// How a [`Writer`] compresses.
#[derive(Clone)]
pub struct Options {
    /// zstd level.
    pub level: i32,
    /// 0 uses all cores (capped at 8), 1 never starts a thread.
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            level: DEFAULT_LEVEL,
            threads: 0,
        }
    }
}

fn count_lines(block: &[u8]) -> u32 {
    memchr::memchr_iter(b'\n', block).count() as u32
}

/// Compressed frames of a batch, with the number of lines of each block.
type Compressed = Vec<(Vec<u8>, u32)>;

fn compress_blocks(level: i32, data: &[u8]) -> io::Result<Compressed> {
    data.par_chunks(BLOCK_SIZE)
        .map(|block| Ok((compress_to_vec(level, block)?, count_lines(block))))
        .collect()
}

/// A full batch being compressed in the background while the caller goes on.
struct PendingBatch {
    /// Threads do not survive `fork()`: a child must not wait for this batch.
    pid: u32,
    data: Arc<Vec<u8>>,
    done: mpsc::Receiver<io::Result<Compressed>>,
}

/// Compresses what is written to it into a z64 stream.
///
/// With several threads, a full batch is compressed in the background and reaches the
/// file when the next one is ready, in order. A failure can therefore be reported by a
/// later call than the one that caused it, and at the latest by `finish`.
///
/// [`finish`](Writer::finish) writes the seek table. It runs on drop as a last
/// resort, but only an explicit call reports errors.
///
/// A [`segmented`](Writer::segmented) writer instead hands `inner` one complete segment
/// per `write`, cut on a line end, for files shared between processes.
pub struct Writer<W: Write> {
    inner: W,
    level: i32,
    threads: usize,
    pool: LazyPool,
    buf: Vec<u8>,
    batch_size: usize,
    frame: Vec<u8>,
    pending: Option<PendingBatch>,
    spare: Vec<u8>,
    entries: Vec<Entry>,
    /// `None` after appending to a file whose index was lost.
    lines: Option<Vec<u32>>,
    written: u64,
    segmented: bool,
    /// Bytes of `buf` already known to hold no line end.
    line_checked: usize,
    committed: bool,
    pid: u32,
    finished: bool,
    /// A block could not be written. A table written now would hide that it is missing,
    /// so the file stays without one and every reader will report it as incomplete.
    broken: bool,
}

impl<W: Write> Writer<W> {
    pub fn new(inner: W, options: Options) -> Self {
        let threads = resolve_threads(options.threads);
        let batch_size = if threads > 1 {
            BLOCK_SIZE * threads * BLOCKS_PER_THREAD
        } else {
            BLOCK_SIZE
        };
        Self {
            inner,
            level: options.level,
            threads,
            pool: LazyPool::new(threads),
            buf: Vec::new(),
            batch_size,
            frame: Vec::new(),
            pending: None,
            spare: Vec::new(),
            entries: Vec::new(),
            lines: Some(Vec::new()),
            written: 0,
            segmented: false,
            line_checked: 0,
            committed: false,
            pid: std::process::id(),
            finished: false,
            broken: false,
        }
    }

    /// Each `write` to `inner` will be a whole segment, which leaves the other writers of
    /// a shared file free to add theirs in between. See [`SharedLog`](crate::SharedLog).
    pub fn segmented(inner: W, options: Options) -> Self {
        let mut writer = Self::new(inner, options);
        writer.segmented = true;
        writer
    }

    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Uncompressed position in the stream.
    pub fn position(&self) -> u64 {
        let pending = self.pending.as_ref().map_or(0, |batch| batch.data.len());
        self.written + (pending + self.buf.len()) as u64
    }

    /// Copies `data` if that does not fill the buffer, so no compression runs.
    pub fn write_buffered(&mut self, data: &[u8]) -> bool {
        if self.finished
            || self.broken
            || self.buf.len() + data.len() >= self.batch_size
            || (self.segmented && self.pid != std::process::id())
        {
            return false;
        }
        self.buf.extend_from_slice(data);
        true
    }

    fn record(&mut self, block: &[u8], comp: usize, lines: u32) {
        self.entries.push(Entry {
            comp: comp as u32,
            raw: block.len() as u32,
        });
        if let Some(counts) = &mut self.lines {
            counts.push(lines);
        }
    }

    fn check_capacity(&self, data: &[u8]) -> io::Result<()> {
        if self.entries.len() + data.len().div_ceil(BLOCK_SIZE) > MAX_ENTRIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many blocks in a z64 file",
            ));
        }
        Ok(())
    }

    /// What a process inherits through `fork` is its parent's to write.
    fn forget_inherited_data(&mut self) {
        let pid = std::process::id();
        if self.segmented && self.pid != pid {
            self.pid = pid;
            self.buf.clear();
            self.pending = None;
        }
    }

    fn store_segment(&mut self, data: &[u8], compressed: &Compressed) -> io::Result<()> {
        let size: usize = compressed.iter().map(|(frame, _)| frame.len()).sum();
        let mut segment = Vec::with_capacity(size + compressed.len() * 16 + 64);
        let mut entries = Vec::with_capacity(compressed.len());
        let mut lines = Vec::with_capacity(compressed.len());
        for (block, (frame, count)) in data.chunks(BLOCK_SIZE).zip(compressed) {
            segment.extend_from_slice(frame);
            entries.push(Entry {
                comp: frame.len() as u32,
                raw: block.len() as u32,
            });
            lines.push(*count);
        }
        write_trailer(&mut segment, &entries, Some(&lines))?;
        self.inner.write_all(&segment)?;
        self.committed = true;
        self.written += data.len() as u64;
        Ok(())
    }

    fn store(&mut self, data: &[u8], compressed: &Compressed) -> io::Result<()> {
        if self.segmented {
            return self.store_segment(data, compressed);
        }
        self.check_capacity(data)?;
        for (block, (frame, lines)) in data.chunks(BLOCK_SIZE).zip(compressed) {
            self.inner.write_all(frame)?;
            self.record(block, frame.len(), *lines);
        }
        self.written += data.len() as u64;
        Ok(())
    }

    fn store_pending(&mut self) -> io::Result<()> {
        let Some(batch) = self.pending.take() else {
            return Ok(());
        };
        let received = (batch.pid == std::process::id())
            .then(|| batch.done.recv().ok())
            .flatten();
        let compressed = match received {
            Some(compressed) => compressed?,
            None => {
                let level = self.level;
                let data = &batch.data;
                self.pool.get()?.install(|| compress_blocks(level, data))?
            }
        };
        self.store(&batch.data, &compressed)?;
        if let Ok(mut data) = Arc::try_unwrap(batch.data) {
            data.clear();
            self.spare = data;
        }
        Ok(())
    }

    fn collect_pending(&mut self) -> io::Result<()> {
        let result = self.store_pending();
        self.broken |= result.is_err();
        result
    }

    /// Writes the previous batch, then starts compressing the buffer in the background.
    fn submit_buffer(&mut self) -> io::Result<()> {
        self.collect_pending()?;
        let spare = std::mem::take(&mut self.spare);
        let data = Arc::new(std::mem::replace(&mut self.buf, spare));
        let job_data = Arc::clone(&data);
        let level = self.level;
        let (sender, done) = mpsc::channel();
        let pool = match self.pool.get() {
            Ok(pool) => pool,
            Err(error) => {
                self.broken = true;
                return Err(error);
            }
        };
        pool.spawn(move || {
            let result = compress_blocks(level, &job_data);
            drop(job_data); // lets the caller reuse the buffer
            let _ = sender.send(result);
        });
        self.pending = Some(PendingBatch {
            pid: std::process::id(),
            data,
            done,
        });
        Ok(())
    }

    fn emit(&mut self, data: &[u8]) -> io::Result<()> {
        self.collect_pending()?;
        let result = self.emit_blocks(data);
        self.broken |= result.is_err();
        result
    }

    fn emit_blocks(&mut self, data: &[u8]) -> io::Result<()> {
        let level = self.level;
        if self.threads > 1 && data.len() > BLOCK_SIZE {
            let compressed = self.pool.get()?.install(|| compress_blocks(level, data))?;
            return self.store(data, &compressed);
        }
        if self.segmented {
            let compressed = data
                .chunks(BLOCK_SIZE)
                .map(|block| Ok((compress_to_vec(level, block)?, count_lines(block))))
                .collect::<io::Result<Compressed>>()?;
            return self.store(data, &compressed);
        }
        self.check_capacity(data)?;
        for block in data.chunks(BLOCK_SIZE) {
            let mut frame = std::mem::take(&mut self.frame);
            compress_into(level, block, &mut frame)?;
            self.inner.write_all(&frame)?;
            self.record(block, frame.len(), count_lines(block));
            self.frame = frame;
        }
        self.written += data.len() as u64;
        Ok(())
    }

    /// Commits the buffer up to its last line end. Two writers cutting their lines at
    /// the block size would interleave pieces of lines. A line longer than a batch stays
    /// in the buffer until it ends.
    fn commit_lines(&mut self) -> io::Result<()> {
        let checked = self.line_checked.min(self.buf.len());
        let Some(end) = memchr::memrchr(b'\n', &self.buf[checked..]) else {
            self.line_checked = self.buf.len();
            return Ok(());
        };
        let rest = self.buf.split_off(checked + end + 1);
        if self.threads > 1 {
            self.submit_buffer()?;
        } else {
            self.emit_buffer()?;
        }
        self.buf.extend_from_slice(&rest);
        self.line_checked = 0;
        Ok(())
    }

    /// Writes everything accepted so far, the current partial block included.
    fn emit_buffer(&mut self) -> io::Result<()> {
        self.collect_pending()?;
        if self.buf.is_empty() {
            return Ok(());
        }
        let buf = std::mem::take(&mut self.buf);
        let result = self.emit(&buf);
        self.buf = buf;
        self.buf.clear();
        result
    }

    /// Seals the current block and writes the seek table. Calling it twice is harmless.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        if self.broken {
            return Ok(()); // the failure itself was already reported
        }
        self.forget_inherited_data();
        self.emit_buffer()?;
        if self.segmented {
            if !self.committed {
                // A log that was created and left empty must still be a valid file.
                let mut segment = Vec::new();
                write_trailer(&mut segment, &[], Some(&[]))?;
                self.inner.write_all(&segment)?;
            }
            return self.inner.flush();
        }
        write_trailer(&mut self.inner, &self.entries, self.lines.as_deref())?;
        self.inner.flush()
    }
}

impl<W: Read + Write + Seek + Truncate> Writer<W> {
    /// Continues an existing z64 file, which must be open for reading and writing.
    ///
    /// New data starts a new block: the last block of the file is left untouched.
    /// A file that stops in the middle of a frame is refused rather than cut.
    pub fn append(mut file: W, options: Options) -> io::Result<Self> {
        let layout = read_layout(&mut file)?;
        if layout.truncated {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cannot append to a truncated z64 file, repair it first",
            ));
        }
        file.truncate_to(layout.data_end)?;
        file.seek(SeekFrom::Start(layout.data_end))?;
        let mut writer = Self::new(file, options);
        writer.written = layout.entries.iter().map(|e| u64::from(e.raw)).sum();
        writer.entries = layout.entries;
        writer.lines = layout
            .lines
            .or_else(|| writer.entries.is_empty().then(Vec::new));
        Ok(writer)
    }
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        if self.finished {
            return Err(io::Error::other("write after finish"));
        }
        if self.broken {
            return Err(io::Error::other("a previous write failed"));
        }
        let total = data.len();
        if self.segmented {
            self.forget_inherited_data();
            self.buf.extend_from_slice(data);
            if self.buf.len() >= self.batch_size {
                self.commit_lines()?;
            }
            return Ok(total);
        }
        while !data.is_empty() {
            if self.buf.is_empty() && data.len() >= self.batch_size {
                // No copy, and one single parallel run however large the data is.
                let whole = data.len() - data.len() % self.batch_size;
                let (batches, rest) = data.split_at(whole);
                self.emit(batches)?;
                data = rest;
                continue;
            }
            let room = self.batch_size - self.buf.len();
            let (head, rest) = data.split_at(room.min(data.len()));
            self.buf.extend_from_slice(head);
            data = rest;
            if self.buf.len() == self.batch_size {
                if self.threads > 1 {
                    self.submit_buffer()?;
                } else {
                    self.emit_buffer()?;
                }
            }
        }
        Ok(total)
    }

    /// Seals the current block so that everything written so far reaches the file.
    /// Frequent calls create many small blocks and hurt the compression ratio.
    ///
    /// A segmented writer holds back a line that has not ended until it does, or until
    /// `finish`: committed alone, it would let another writer land in the middle of it.
    fn flush(&mut self) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::other("a previous write failed"));
        }
        if !self.finished {
            self.forget_inherited_data();
            if self.segmented {
                self.commit_lines()?;
                self.collect_pending()?;
            } else {
                self.emit_buffer()?;
            }
        }
        self.inner.flush()
    }
}

impl<W: Write> Drop for Writer<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
