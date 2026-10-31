//! Compression of a single block. zstd contexts are kept per thread and reused:
//! creating one for each 64 KiB block would dominate the running time.

use std::cell::RefCell;
use std::io;

use rayon::prelude::*;
use zstd::bulk::{Compressor, Decompressor};
use zstd::zstd_safe::CParameter;

use crate::format::{invalid, max_frame_size};

thread_local! {
    static COMPRESSOR: RefCell<Option<(i32, Compressor<'static>)>> = const { RefCell::new(None) };
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static DECOMPRESSOR: RefCell<Option<Decompressor<'static>>> = const { RefCell::new(None) };
}

/// Replaces the content of `out` with one complete zstd frame holding `src`.
pub(crate) fn compress_into(level: i32, src: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
    COMPRESSOR.with_borrow_mut(|slot| {
        if !matches!(slot, Some((current, _)) if *current == level) {
            let mut compressor = Compressor::new(level)?;
            compressor.set_parameter(CParameter::ChecksumFlag(true))?;
            *slot = Some((level, compressor));
        }
        let (_, compressor) = slot.as_mut().expect("compressor was just set");
        out.clear();
        out.reserve(max_frame_size());
        compressor.compress_to_buffer(src, out)?;
        Ok(())
    })
}

/// Same as `compress_into` for parallel code, where each frame must be kept. A frame is
/// far smaller than the buffer needed to produce it, so that buffer is reused.
pub(crate) fn compress_to_vec(level: i32, src: &[u8]) -> io::Result<Vec<u8>> {
    SCRATCH.with_borrow_mut(|frame| {
        compress_into(level, src, frame)?;
        Ok(frame.clone())
    })
}

/// Decodes one frame into `out`, which must have exactly the announced size.
pub(crate) fn decompress_into(src: &[u8], out: &mut [u8]) -> io::Result<()> {
    if src.len() > max_frame_size() {
        return Err(invalid("compressed block too large"));
    }
    DECOMPRESSOR.with_borrow_mut(|slot| {
        if slot.is_none() {
            *slot = Some(Decompressor::new()?);
        }
        let decompressor = slot.as_mut().expect("decompressor was just set");
        let written = decompressor
            .decompress_to_buffer(src, out)
            .map_err(|e| invalid(format!("corrupt block: {e}")))?;
        if written != out.len() {
            return Err(invalid("block size does not match the seek table"));
        }
        Ok(())
    })
}

/// Decodes several frames, in parallel on the current rayon pool if asked to.
pub(crate) fn decompress_all(jobs: Vec<(&[u8], &mut [u8])>, parallel: bool) -> io::Result<()> {
    if parallel && jobs.len() > 1 {
        jobs.into_par_iter()
            .try_for_each(|(frame, out)| decompress_into(frame, out))
    } else {
        jobs.into_iter()
            .try_for_each(|(frame, out)| decompress_into(frame, out))
    }
}
