use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};

use crate::Truncate;
use crate::format::{read_layout, write_trailer};
use crate::frame::decompress_into;

/// What [`repair`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepairReport {
    /// Data blocks left in the file.
    pub blocks: u64,
    /// Uncompressed size left in the file.
    pub size: u64,
    /// Bytes of the file that cannot be recovered.
    pub removed_bytes: u64,
    /// The file needed work. With `dry_run`, it was not touched.
    pub needed_repair: bool,
}

/// Restores a file whose writer crashed: cuts the incomplete end and rewrites the table.
///
/// A complete but damaged block is removed only at the very end of the file, where a
/// crash can leave one. Damage further inside is not repaired: the frames after it are
/// intact, and cutting them would destroy valid data.
pub fn repair<F>(file: &mut F, dry_run: bool) -> io::Result<RepairReport>
where
    F: Read + Write + Seek + Truncate,
{
    let file_len = file.seek(SeekFrom::End(0))?;
    let mut layout = read_layout(file)?;
    let mut dropped = 0u64;
    let mut comp = Vec::new();
    let mut raw = Vec::new();
    while let Some(last) = layout.entries.last().copied().filter(|entry| entry.raw > 0) {
        comp.resize(last.comp as usize, 0);
        raw.resize(last.raw as usize, 0);
        file.seek(SeekFrom::Start(layout.data_end - u64::from(last.comp)))?;
        file.read_exact(&mut comp)?;
        match decompress_into(&comp, &mut raw) {
            Ok(()) => break,
            Err(error) if error.kind() == ErrorKind::InvalidData => {
                layout.entries.pop();
                layout.data_end -= u64::from(last.comp);
                dropped += u64::from(last.comp);
                if let Some(lines) = &mut layout.lines {
                    lines.pop();
                }
            }
            Err(error) => return Err(error),
        }
    }
    let needed_repair = !layout.has_table || layout.truncated || dropped > 0;
    let removed_bytes = if layout.finished {
        dropped
    } else {
        file_len - layout.data_end
    };
    if needed_repair && !dry_run {
        file.truncate_to(layout.data_end)?;
        file.seek(SeekFrom::Start(layout.data_end))?;
        write_trailer(file, &layout.entries, layout.lines.as_deref())?;
        file.flush()?;
    }
    let data = layout.entries.iter().filter(|entry| entry.raw > 0);
    Ok(RepairReport {
        blocks: data.clone().count() as u64,
        size: data.map(|entry| u64::from(entry.raw)).sum(),
        removed_bytes,
        needed_repair,
    })
}
