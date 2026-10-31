use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::sync::mpsc;

use crate::format::{BLOCK_SIZE, Layout, invalid, read_layout};
use crate::frame::{decompress_all, decompress_into};
use crate::{BLOCKS_PER_THREAD, LazyPool, resolve_threads};

#[derive(Clone, Copy)]
struct Block {
    comp_start: u64,
    raw_start: u64,
    comp: u32,
    raw: u32,
}

impl Block {
    fn raw_end(&self) -> u64 {
        self.raw_start + u64::from(self.raw)
    }

    fn comp_end(&self) -> u64 {
        self.comp_start + u64::from(self.comp)
    }
}

/// `comp` and `raw` must cover exactly `blocks`.
fn decode_blocks(blocks: &[Block], comp: &[u8], raw: &mut [u8], parallel: bool) -> io::Result<()> {
    let mut jobs = Vec::with_capacity(blocks.len());
    let (mut comp, mut raw) = (comp, raw);
    for block in blocks {
        let (frame, comp_rest) = comp.split_at(block.comp as usize);
        let (out, raw_rest) = raw.split_at_mut(block.raw as usize);
        (comp, raw) = (comp_rest, raw_rest);
        if block.raw > 0 {
            jobs.push((frame, out));
        }
    }
    decompress_all(jobs, parallel)
}

/// A batch being decoded in the background, ahead of the reader.
struct Prefetch {
    /// Threads do not survive `fork()`: a child must not wait for this batch.
    pid: u32,
    first: usize,
    last: usize,
    raw_len: usize,
    done: mpsc::Receiver<Decoded>,
}

struct Decoded {
    result: io::Result<()>,
    comp: Vec<u8>,
    raw: Vec<u8>,
}

/// Reads a z64 stream as plain data, with `Read`, `BufRead` and `Seek`.
pub struct Reader<R> {
    inner: R,
    blocks: Vec<Block>,
    /// Number of `\n` before each block, then in the whole file.
    line_starts: Option<Vec<u64>>,
    total: u64,
    incomplete: bool,
    threads: usize,
    pool: LazyPool,
    /// Blocks to decode on the next load. Starts at one and doubles while reads are
    /// sequential, so that a seek or a short read only pays for a single block.
    ahead: usize,
    max_ahead: usize,
    next_block: usize,
    pos: u64,
    comp: Vec<u8>,
    raw: Vec<u8>,
    loaded_start: u64,
    loaded_len: usize,
    prefetch: Option<Prefetch>,
    spare_comp: Vec<u8>,
    spare_raw: Vec<u8>,
}

impl<R: Read + Seek> Reader<R> {
    /// `threads == 0` uses all cores (capped at 8), `threads == 1` never starts a thread.
    pub fn new(mut inner: R, threads: usize) -> io::Result<Self> {
        let Layout {
            entries,
            truncated,
            finished,
            lines,
            ..
        } = read_layout(&mut inner)?;
        let mut blocks = Vec::with_capacity(entries.len());
        let (mut comp_start, mut raw_start) = (0u64, 0u64);
        for entry in entries {
            blocks.push(Block {
                comp_start,
                raw_start,
                comp: entry.comp,
                raw: entry.raw,
            });
            comp_start += u64::from(entry.comp);
            raw_start += u64::from(entry.raw);
        }
        let line_starts = lines.map(|counts| {
            let mut counts = counts.into_iter();
            let mut total = 0u64;
            let mut starts = Vec::with_capacity(blocks.len() + 1);
            starts.push(0);
            for block in &blocks {
                if block.raw > 0 {
                    total += u64::from(counts.next().unwrap_or(0));
                }
                starts.push(total);
            }
            starts
        });
        let threads = resolve_threads(threads);
        Ok(Self {
            inner,
            blocks,
            line_starts,
            total: raw_start,
            incomplete: truncated || !finished,
            threads,
            pool: LazyPool::new(threads),
            ahead: 0,
            max_ahead: if threads > 1 {
                threads * BLOCKS_PER_THREAD
            } else {
                1
            },
            next_block: 0,
            pos: 0,
            comp: Vec::new(),
            raw: Vec::new(),
            loaded_start: 0,
            loaded_len: 0,
            prefetch: None,
            spare_comp: Vec::new(),
            spare_raw: Vec::new(),
        })
    }

    /// Total uncompressed size.
    pub fn len(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Number of data blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.iter().filter(|block| block.raw > 0).count()
    }

    /// Size of the frames, seek table and line index excluded.
    pub fn compressed_len(&self) -> u64 {
        self.blocks.last().map_or(0, Block::comp_end)
    }

    /// The file has no end marker: it was cut, or its writer never finished.
    pub fn is_truncated(&self) -> bool {
        self.incomplete
    }

    /// The file carries an index of its lines, which makes `seek_line` immediate.
    pub fn has_line_index(&self) -> bool {
        self.line_starts.is_some()
    }

    /// Decoded bytes available at the current position without any work.
    pub fn buffered(&self) -> &[u8] {
        match self.pos.checked_sub(self.loaded_start) {
            Some(offset) if offset < self.loaded_len as u64 => {
                &self.raw[offset as usize..self.loaded_len]
            }
            _ => &[],
        }
    }

    fn finish_prefetch(&mut self, first: usize) -> Option<(Prefetch, Decoded)> {
        let prefetch = self.prefetch.take()?;
        if prefetch.first != first || prefetch.pid != std::process::id() {
            return None;
        }
        let decoded = prefetch.done.recv().ok()?;
        Some((prefetch, decoded))
    }

    /// Reads the next batch and decodes it on the pool while the caller goes on. Any
    /// failure here is left to the regular load, which will report it in its turn.
    fn start_prefetch(&mut self) {
        let first = self.next_block;
        let last = (first + self.ahead).min(self.blocks.len());
        if self.threads <= 1 || self.ahead < self.max_ahead || first >= last {
            return;
        }
        let blocks = self.blocks[first..last].to_vec();
        let (head, tail) = (blocks[0], blocks[blocks.len() - 1]);
        let raw_len = (tail.raw_end() - head.raw_start) as usize;
        let mut comp = std::mem::take(&mut self.spare_comp);
        let mut raw = std::mem::take(&mut self.spare_raw);
        comp.resize((tail.comp_end() - head.comp_start) as usize, 0);
        raw.resize(raw_len.max(raw.len()), 0);
        let loaded = self.inner.seek(SeekFrom::Start(head.comp_start)).is_ok()
            && self.inner.read_exact(&mut comp).is_ok();
        if !loaded {
            return;
        }
        let Ok(pool) = self.pool.get() else { return };
        let (sender, done) = mpsc::channel();
        pool.spawn(move || {
            let result = decode_blocks(&blocks, &comp, &mut raw[..raw_len], true);
            let _ = sender.send(Decoded { result, comp, raw });
        });
        let pid = std::process::id();
        self.prefetch = Some(Prefetch {
            pid,
            first,
            last,
            raw_len,
            done,
        });
    }

    fn load(&mut self) -> io::Result<()> {
        self.loaded_len = 0;
        let first = self.blocks.partition_point(|b| b.raw_end() <= self.pos);
        let (last, raw_len) = if let Some((prefetch, decoded)) = self.finish_prefetch(first) {
            self.spare_comp = decoded.comp;
            self.spare_raw = std::mem::replace(&mut self.raw, decoded.raw);
            decoded.result?;
            (prefetch.last, prefetch.raw_len)
        } else {
            let sequential = first == self.next_block;
            self.ahead = if sequential {
                (self.ahead * 2).clamp(1, self.max_ahead)
            } else {
                1
            };
            self.next_block = first;
            let last = (first + self.ahead).min(self.blocks.len());
            let (head, tail) = (self.blocks[first], self.blocks[last - 1]);
            let raw_len = (tail.raw_end() - head.raw_start) as usize;
            self.comp
                .resize((tail.comp_end() - head.comp_start) as usize, 0);
            self.raw.resize(raw_len.max(self.raw.len()), 0);
            self.inner.seek(SeekFrom::Start(head.comp_start))?;
            self.inner.read_exact(&mut self.comp)?;
            let blocks = &self.blocks[first..last];
            let (comp, raw) = (&self.comp, &mut self.raw[..raw_len]);
            if self.threads > 1 && last - first > 1 {
                self.pool
                    .get()?
                    .install(|| decode_blocks(blocks, comp, raw, true))?;
            } else {
                decode_blocks(blocks, comp, raw, false)?;
            }
            (last, raw_len)
        };
        self.next_block = last;
        self.loaded_start = self.blocks[first].raw_start;
        self.loaded_len = raw_len;
        self.start_prefetch();
        Ok(())
    }

    /// Fills `out` as far as the file allows and returns the number of bytes written.
    ///
    /// Whole blocks are decoded straight into `out`, all in parallel, with no read-ahead
    /// and no intermediate copy. Use it for large reads whose size is known.
    pub fn read_into(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < out.len() {
            let rest = &mut out[filled..];
            if self.buffered().is_empty()
                && let Some(length) = self.decode_whole_blocks(rest)?
            {
                filled += length;
                continue;
            }
            let available = self.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let length = available.len().min(rest.len());
            rest[..length].copy_from_slice(&available[..length]);
            self.consume(length);
            filled += length;
        }
        Ok(filled)
    }

    /// Decodes the blocks that start at the current position and fit in `out`.
    fn decode_whole_blocks(&mut self, out: &mut [u8]) -> io::Result<Option<usize>> {
        let first = self.blocks.partition_point(|b| b.raw_end() <= self.pos);
        let end = self.pos + out.len() as u64;
        let last = self.blocks.partition_point(|b| b.raw_end() <= end);
        if first >= last || self.blocks[first].raw_start != self.pos {
            return Ok(None);
        }
        let (head, tail) = (self.blocks[first], self.blocks[last - 1]);
        let raw_len = (tail.raw_end() - head.raw_start) as usize;
        self.comp
            .resize((tail.comp_end() - head.comp_start) as usize, 0);
        self.inner.seek(SeekFrom::Start(head.comp_start))?;
        self.inner.read_exact(&mut self.comp)?;
        let blocks = &self.blocks[first..last];
        let (comp, raw) = (&self.comp, &mut out[..raw_len]);
        if self.threads > 1 && last - first > 1 {
            self.pool
                .get()?
                .install(|| decode_blocks(blocks, comp, raw, true))?;
        } else {
            decode_blocks(blocks, comp, raw, false)?;
        }
        self.prefetch = None;
        self.next_block = last;
        self.pos = tail.raw_end();
        Ok(Some(raw_len))
    }

    /// Counts the lines of the whole file when it has no index, as after a crash.
    fn ensure_line_starts(&mut self) -> io::Result<()> {
        if self.line_starts.is_some() {
            return Ok(());
        }
        let mut starts = Vec::with_capacity(self.blocks.len() + 1);
        let (mut comp, mut raw) = (Vec::new(), vec![0u8; BLOCK_SIZE]);
        let mut total = 0u64;
        starts.push(0);
        for block in &self.blocks {
            if block.raw > 0 {
                comp.resize(block.comp as usize, 0);
                self.inner.seek(SeekFrom::Start(block.comp_start))?;
                self.inner.read_exact(&mut comp)?;
                let out = &mut raw[..block.raw as usize];
                decompress_into(&comp, out)?;
                total += memchr::memchr_iter(b'\n', out).count() as u64;
            }
            starts.push(total);
        }
        self.line_starts = Some(starts);
        Ok(())
    }

    /// Moves to the start of line `line`, counting from 0, and returns the position.
    ///
    /// Past the last line, moves to the end of the file. Only one block is decoded
    /// when the file has a line index. Without one, the first call reads everything.
    pub fn seek_line(&mut self, line: u64) -> io::Result<u64> {
        if line == 0 {
            self.pos = 0;
            return Ok(0);
        }
        self.ensure_line_starts()?;
        let starts = self
            .line_starts
            .as_ref()
            .expect("line index was just built");
        // The line starts right after the `line`-th newline.
        let index = starts.partition_point(|count| *count < line);
        if index == starts.len() {
            self.pos = self.total;
            return Ok(self.pos);
        }
        let nth = (line - starts[index - 1] - 1) as usize;
        let start = self.blocks[index - 1].raw_start;
        self.pos = start;
        let offset = memchr::memchr_iter(b'\n', self.fill_buf()?)
            .nth(nth)
            .ok_or_else(|| invalid("the line index does not match the data"))?;
        self.pos = start + offset as u64 + 1;
        Ok(self.pos)
    }

    /// Number of lines, a last line without `\n` included.
    pub fn line_count(&mut self) -> io::Result<u64> {
        self.ensure_line_starts()?;
        let newlines = self
            .line_starts
            .as_ref()
            .and_then(|s| s.last().copied())
            .unwrap_or(0);
        if self.total == 0 {
            return Ok(0);
        }
        let saved = self.pos;
        self.pos = self.total - 1;
        let unterminated = self.fill_buf()?.first() != Some(&b'\n');
        self.pos = saved;
        Ok(newlines + u64::from(unterminated))
    }
}

impl<R: Read + Seek> BufRead for Reader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.buffered().is_empty() {
            if self.pos >= self.total {
                if self.incomplete {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "z64 file has no end marker: it was cut or not closed",
                    ));
                }
                return Ok(&[]);
            }
            self.load()?;
        }
        Ok(self.buffered())
    }

    fn consume(&mut self, amount: usize) {
        self.pos += amount as u64;
    }
}

impl<R: Read + Seek> Read for Reader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }

    /// The default implementation zeroes its buffer and copies in small steps.
    fn read_to_end(&mut self, out: &mut Vec<u8>) -> io::Result<usize> {
        out.reserve(self.total.saturating_sub(self.pos) as usize);
        crate::drain(self, out, u64::MAX)
    }
}

impl<R: Read + Seek> Seek for Reader<R> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let pos = match target {
            SeekFrom::Start(pos) => Some(pos),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
            SeekFrom::End(delta) => self.total.checked_add_signed(delta),
        };
        self.pos = pos.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start of the file",
            )
        })?;
        Ok(self.pos)
    }
}
