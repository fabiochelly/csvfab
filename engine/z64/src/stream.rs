use std::io::{self, BufRead, Read};

use crate::format::{
    BLOCK_SIZE, Parsed, SEEK_TABLE_MAGIC, frame_is_sound, invalid, max_frame_size,
    must_check_frame, parse_frame,
};
use crate::frame::decompress_all;
use crate::{BLOCKS_PER_THREAD, LazyPool, resolve_threads};

const READ_SIZE: usize = 1024 * 1024;

/// Reads a z64 stream from a source that cannot seek, such as a pipe or a socket.
///
/// Frames are decoded as soon as they arrive: a slow source is never waited for
/// longer than it takes to receive one complete block.
pub struct StreamReader<R> {
    inner: R,
    threads: usize,
    pool: LazyPool,
    max_batch: usize,
    input: Vec<u8>,
    consumed: usize,
    /// Bytes of a skippable frame that are still to come and to ignore.
    skip_left: u64,
    input_offset: u64,
    eof: bool,
    /// The last frame seen was a seek table, which only a finished file ends with.
    saw_table: bool,
    truncated: bool,
    raw: Vec<u8>,
    raw_pos: usize,
    raw_len: usize,
    pos: u64,
}

struct PendingFrame {
    start: usize,
    comp: usize,
    raw: usize,
}

impl<R: Read> StreamReader<R> {
    /// `threads == 0` uses all cores (capped at 8), `threads == 1` never starts a thread.
    pub fn new(inner: R, threads: usize) -> Self {
        let threads = resolve_threads(threads);
        Self {
            inner,
            threads,
            pool: LazyPool::new(threads),
            max_batch: if threads > 1 {
                threads * BLOCKS_PER_THREAD
            } else {
                1
            },
            input: Vec::new(),
            consumed: 0,
            skip_left: 0,
            input_offset: 0,
            eof: false,
            saw_table: false,
            truncated: false,
            raw: Vec::new(),
            raw_pos: 0,
            raw_len: 0,
            pos: 0,
        }
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Decoded bytes available at the current position without any work.
    pub fn buffered(&self) -> &[u8] {
        &self.raw[self.raw_pos..self.raw_len]
    }

    fn read_more(&mut self) -> io::Result<()> {
        self.input_offset += self.consumed as u64;
        self.input.drain(..self.consumed);
        self.consumed = 0;
        let length = self.input.len();
        self.input.resize(length + READ_SIZE, 0);
        let received = self.inner.read(&mut self.input[length..]);
        self.input
            .truncate(length + *received.as_ref().unwrap_or(&0));
        self.eof = received? == 0;
        Ok(())
    }

    /// Collects the frames at hand, without blocking as soon as one is complete.
    fn next_frames(&mut self) -> io::Result<Vec<PendingFrame>> {
        loop {
            while self.skip_left > 0 {
                let available = (self.input.len() - self.consumed) as u64;
                let skipped = available.min(self.skip_left);
                self.consumed += skipped as usize;
                self.skip_left -= skipped;
                if self.skip_left > 0 {
                    if self.eof {
                        self.truncated = true;
                        return Ok(Vec::new());
                    }
                    self.read_more()?;
                }
            }
            let mut frames = Vec::new();
            let mut at = self.consumed;
            while frames.len() < self.max_batch {
                let window = at..self.input.len().min(at + max_frame_size());
                let offset = self.input_offset + at as u64;
                match parse_frame(&self.input[window], offset)? {
                    Parsed::Data { comp, raw } => {
                        let (comp, raw) = (comp as usize, raw as usize);
                        self.saw_table = false;
                        frames.push(PendingFrame {
                            start: at,
                            comp,
                            raw,
                        });
                        at += comp;
                    }
                    Parsed::Skippable { magic, total } => {
                        let available = (self.input.len() - at) as u64;
                        self.saw_table = magic == SEEK_TABLE_MAGIC;
                        if must_check_frame(magic, total) {
                            if available < total {
                                break; // kept whole in the input, to be checked first
                            }
                            let end = at + total as usize;
                            if !frame_is_sound(magic, &self.input[at..end]) {
                                return Err(invalid(format!("damaged trailer at offset {offset}")));
                            }
                            at = end;
                        } else if available >= total {
                            at += total as usize;
                        } else if frames.is_empty() {
                            self.consumed = self.input.len();
                            self.skip_left = total - available;
                            at = self.consumed;
                            break;
                        } else {
                            break;
                        }
                    }
                    Parsed::Incomplete => break,
                }
            }
            if !frames.is_empty() {
                return Ok(frames);
            }
            self.consumed = at;
            if self.skip_left > 0 {
                continue;
            }
            if self.eof {
                self.truncated = self.consumed < self.input.len() || !self.saw_table;
                return Ok(Vec::new());
            }
            self.read_more()?;
        }
    }

    /// Returns false at the end of the input. A frame may hold no data, so an empty
    /// result does not mean that the input is over.
    fn load(&mut self) -> io::Result<bool> {
        let frames = self.next_frames()?;
        let raw_len: usize = frames.iter().map(|frame| frame.raw).sum();
        debug_assert!(raw_len <= self.max_batch * BLOCK_SIZE);
        self.raw.resize(raw_len.max(self.raw.len()), 0);
        let mut jobs = Vec::with_capacity(frames.len());
        let mut raw = &mut self.raw[..raw_len];
        for frame in &frames {
            let (out, rest) = raw.split_at_mut(frame.raw);
            raw = rest;
            jobs.push((&self.input[frame.start..frame.start + frame.comp], out));
        }
        if self.threads > 1 && jobs.len() > 1 {
            self.pool.get()?.install(|| decompress_all(jobs, true))?;
        } else {
            decompress_all(jobs, false)?;
        }
        if let Some(frame) = frames.last() {
            self.consumed = frame.start + frame.comp;
        }
        self.raw_pos = 0;
        self.raw_len = raw_len;
        Ok(!frames.is_empty())
    }
}

impl<R: Read> BufRead for StreamReader<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.raw_pos == self.raw_len {
            if self.truncated {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "z64 stream has no end marker: it was cut or not closed",
                ));
            }
            if !self.load()? && !self.truncated {
                return Ok(&[]);
            }
        }
        Ok(self.buffered())
    }

    fn consume(&mut self, amount: usize) {
        self.raw_pos += amount;
        self.pos += amount as u64;
    }
}

impl<R: Read> Read for StreamReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let n = available.len().min(out.len());
        out[..n].copy_from_slice(&available[..n]);
        self.consume(n);
        Ok(n)
    }

    /// The default implementation zeroes its buffer and copies in small steps.
    fn read_to_end(&mut self, out: &mut Vec<u8>) -> io::Result<usize> {
        crate::drain(self, out, u64::MAX)
    }
}
