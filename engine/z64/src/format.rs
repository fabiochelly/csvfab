//! On-disk layout: frames, seek table, and the scan used when the table is missing.
//!
//! ```text
//! [zstd frame]... [line index]? [seek table]
//! ```
//!
//! The line index is a skippable frame, which zstd tools ignore. It is listed in the
//! seek table with a decompressed size of 0, as the seekable format requires.
//!
//! A file can hold several such segments in a row. That is what `cat a.z64 b.z64` gives,
//! and what appending does. Each seek table covers its own segment, and they are read
//! from the end of the file, one after the other.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use zstd::zstd_safe;

use crate::frame::decompress_into;

/// Maximum uncompressed size of a block.
pub const BLOCK_SIZE: usize = 64 * 1024;
/// Default zstd level. Level 1 is the fastest of the regular levels.
pub const DEFAULT_LEVEL: i32 = 1;

const ZSTD_MAGIC: u32 = 0xFD2F_B528;
const SKIPPABLE_MAGIC: u32 = 0x184D_2A50;
const SKIPPABLE_MASK: u32 = 0xFFFF_FFF0;
const LINES_MAGIC: u32 = 0x184D_2A5A;
pub(crate) const SEEK_TABLE_MAGIC: u32 = 0x184D_2A5E;
pub(crate) const SEEKABLE_FOOTER_MAGIC: u32 = 0x8F92_EAB1;
const FOOTER_SIZE: u64 = 9;
const SKIPPABLE_HEADER_SIZE: u64 = 8;
/// Starts the payload of the line index.
const TAG: [u8; 4] = *b"Z64\x01";
const TAGGED_HEADER_SIZE: usize = SKIPPABLE_HEADER_SIZE as usize + TAG.len();
/// Limit from the seekable format specification.
pub(crate) const MAX_ENTRIES: usize = 0x800_0000;

/// Largest frame a block can produce (frame header and checksum included).
pub(crate) fn max_frame_size() -> usize {
    zstd_safe::compress_bound(BLOCK_SIZE) + 64
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// One frame of the file. `raw == 0` marks a skippable frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub comp: u32,
    pub raw: u32,
}

/// What a file contains, whether read from its seek table or found by scanning.
pub(crate) struct Layout {
    /// Every frame up to `data_end`.
    pub entries: Vec<Entry>,
    /// End of the last complete frame, line index and seek table excluded.
    pub data_end: u64,
    /// The file stops in the middle of a frame.
    pub truncated: bool,
    pub has_table: bool,
    /// The file ends with a seek table: its writer finished. Without one, nothing tells
    /// a file cut exactly between two frames from a complete file.
    pub finished: bool,
    /// Number of `\n` per data block, when the file has a valid index.
    pub lines: Option<Vec<u32>>,
}

/// A file that can be shortened, which appending and repairing both need.
pub trait Truncate {
    fn truncate_to(&mut self, len: u64) -> io::Result<()>;
}

impl Truncate for std::fs::File {
    fn truncate_to(&mut self, len: u64) -> io::Result<()> {
        self.set_len(len)
    }
}

impl Truncate for Cursor<Vec<u8>> {
    fn truncate_to(&mut self, len: u64) -> io::Result<()> {
        self.get_mut().truncate(len as usize);
        Ok(())
    }
}

/// FNV-1a, enough to reject a damaged line index without another dependency.
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811C_9DC5, |hash: u32, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

fn line_index_frame(lines: &[u32]) -> io::Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(lines.len() * 4 + 4);
    for count in lines {
        payload.extend_from_slice(&count.to_le_bytes());
    }
    payload.extend_from_slice(&checksum(&payload).to_le_bytes());
    let size = u32::try_from(TAG.len() + payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut frame = Vec::with_capacity(TAGGED_HEADER_SIZE + payload.len());
    frame.extend_from_slice(&LINES_MAGIC.to_le_bytes());
    frame.extend_from_slice(&size.to_le_bytes());
    frame.extend_from_slice(&TAG);
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Writes the optional line index, then the seek table.
pub(crate) fn write_trailer<W: Write>(
    out: &mut W,
    entries: &[Entry],
    lines: Option<&[u32]>,
) -> io::Result<()> {
    let mut trailer = Vec::new();
    let mut all_entries = entries.to_vec();
    if let Some(lines) = lines {
        let frame = line_index_frame(lines)?;
        all_entries.push(Entry {
            comp: frame.len() as u32,
            raw: 0,
        });
        trailer = frame;
    }
    let body_size = all_entries.len() as u64 * 8 + FOOTER_SIZE;
    let body_size = u32::try_from(body_size)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many blocks"))?;
    trailer.extend_from_slice(&SEEK_TABLE_MAGIC.to_le_bytes());
    trailer.extend_from_slice(&body_size.to_le_bytes());
    for entry in &all_entries {
        trailer.extend_from_slice(&entry.comp.to_le_bytes());
        trailer.extend_from_slice(&entry.raw.to_le_bytes());
    }
    trailer.extend_from_slice(&(all_entries.len() as u32).to_le_bytes());
    trailer.push(0); // descriptor: per-frame zstd checksums, none in the table
    trailer.extend_from_slice(&SEEKABLE_FOOTER_MAGIC.to_le_bytes());
    out.write_all(&trailer)
}

/// Largest trailer frame that gets checked. One this size already describes 2 TiB of data.
const MAX_CHECKED_FRAME: u64 = 256 * 1024 * 1024;

/// Skipping a frame trusts its size. One flipped bit there can jump over the segment that
/// follows, so the frames z64 writes are checked before being skipped.
pub(crate) fn must_check_frame(magic: u32, total: u64) -> bool {
    (magic == LINES_MAGIC || magic == SEEK_TABLE_MAGIC) && total <= MAX_CHECKED_FRAME
}

/// Says if a complete frame selected by `must_check_frame` has the shape its size implies.
pub(crate) fn frame_is_sound(magic: u32, frame: &[u8]) -> bool {
    if magic == LINES_MAGIC {
        let Some(payload) = frame.get(TAGGED_HEADER_SIZE..) else {
            return false;
        };
        let Some(split) = payload.len().checked_sub(4) else {
            return false;
        };
        let (counts, stored) = payload.split_at(split);
        return frame[8..12] == TAG
            && counts.len() % 4 == 0
            && u32_at(stored, 0) == checksum(counts);
    }
    let footer_at = frame.len().saturating_sub(FOOTER_SIZE as usize);
    if footer_at < SKIPPABLE_HEADER_SIZE as usize {
        return false;
    }
    let descriptor = frame[footer_at + 4];
    let entry_size = if descriptor & 0x80 != 0 { 12 } else { 8 };
    let entries = u32_at(frame, footer_at) as usize * entry_size;
    u32_at(frame, footer_at + 5) == SEEKABLE_FOOTER_MAGIC
        && entries + SKIPPABLE_HEADER_SIZE as usize == footer_at
}

/// What the next frame of a buffer turned out to be.
pub(crate) enum Parsed {
    Data {
        comp: u32,
        raw: u32,
    },
    Skippable {
        magic: u32,
        total: u64,
    },
    /// More bytes are needed. At the end of the input, the file is truncated.
    Incomplete,
}

/// `chunk` must hold `max_frame_size()` bytes, or everything up to the end of the input.
pub(crate) fn parse_frame(chunk: &[u8], offset: u64) -> io::Result<Parsed> {
    if chunk.len() < 4 {
        return Ok(Parsed::Incomplete);
    }
    let magic = u32_at(chunk, 0);
    if magic & SKIPPABLE_MASK == SKIPPABLE_MAGIC {
        if chunk.len() < SKIPPABLE_HEADER_SIZE as usize {
            return Ok(Parsed::Incomplete);
        }
        let total = u64::from(u32_at(chunk, 4)) + SKIPPABLE_HEADER_SIZE;
        return Ok(Parsed::Skippable { magic, total });
    }
    if magic != ZSTD_MAGIC {
        return Err(invalid(format!(
            "not a z64 file (bad magic at offset {offset})"
        )));
    }
    let comp = match zstd_safe::find_frame_compressed_size(chunk) {
        Ok(size) => size,
        Err(_) if chunk.len() < max_frame_size() => return Ok(Parsed::Incomplete),
        Err(_) => {
            return Err(invalid(format!(
                "frame at offset {offset} exceeds a z64 block"
            )));
        }
    };
    match zstd_safe::get_frame_content_size(chunk) {
        Ok(Some(raw)) if raw <= BLOCK_SIZE as u64 => {
            if raw == 0 {
                // Frames announcing no data are skipped, so this size must be checked now.
                decompress_into(&chunk[..comp], &mut [])?;
            }
            Ok(Parsed::Data {
                comp: comp as u32,
                raw: raw as u32,
            })
        }
        _ => Err(invalid(format!(
            "frame at offset {offset} is not a z64 block"
        ))),
    }
}

/// Reads the layout of a file, scanning its frames when there is no usable table.
pub(crate) fn read_layout<R: Read + Seek>(file: &mut R) -> io::Result<Layout> {
    let file_len = file.seek(SeekFrom::End(0))?;
    if let Some(layout) = read_segments(file, file_len)? {
        return Ok(layout);
    }
    let mut layout = scan(file, file_len)?;
    read_line_index(file, &mut layout)?;
    Ok(layout)
}

/// Follows the seek tables back to the start of the file. Anything that does not chain
/// up perfectly gives `None`, and `scan` then walks the frames.
fn read_segments<R: Read + Seek>(file: &mut R, file_len: u64) -> io::Result<Option<Layout>> {
    let mut segments = Vec::new();
    let mut end = file_len;
    while end > 0 {
        let Some((layout, start)) = read_seek_table(file, end)? else {
            return Ok(None);
        };
        segments.push((layout, end));
        end = start;
    }
    if segments.is_empty() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    let mut lines = Some(Vec::new());
    let mut data_end = 0;
    let mut remaining = segments.into_iter().rev().peekable();
    while let Some((mut segment, end)) = remaining.next() {
        let table_start = segment.data_end;
        read_line_index(file, &mut segment)?;
        match (&mut lines, segment.lines.take()) {
            (Some(all), Some(counts)) => all.extend(counts),
            _ => lines = None,
        }
        data_end = segment.data_end;
        entries.append(&mut segment.entries);
        if remaining.peek().is_some() {
            // Earlier trailers stay in the file, so they stay in the list of frames.
            if data_end < table_start {
                entries.push(trailer_entry(table_start - data_end)?);
            }
            entries.push(trailer_entry(end - table_start)?);
            data_end = end;
        }
    }
    let mut layout = new_layout(entries, data_end, false, true, true);
    layout.lines = lines;
    Ok(Some(layout))
}

fn trailer_entry(size: u64) -> io::Result<Entry> {
    let comp = u32::try_from(size).map_err(|_| invalid("trailer too large"))?;
    Ok(Entry { comp, raw: 0 })
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

fn new_layout(
    entries: Vec<Entry>,
    data_end: u64,
    truncated: bool,
    has_table: bool,
    finished: bool,
) -> Layout {
    Layout {
        entries,
        data_end,
        truncated,
        has_table,
        finished,
        lines: None,
    }
}

/// Reads the table of the segment that stops at `end`, and returns where it starts.
///
/// Returns `None`, never an error, for a missing or inconsistent table: the frames
/// themselves are the source of truth and `scan` will rebuild it.
fn read_seek_table<R: Read + Seek>(file: &mut R, end: u64) -> io::Result<Option<(Layout, u64)>> {
    if end < SKIPPABLE_HEADER_SIZE + FOOTER_SIZE {
        return Ok(None);
    }
    let mut footer = [0u8; FOOTER_SIZE as usize];
    file.seek(SeekFrom::Start(end - FOOTER_SIZE))?;
    file.read_exact(&mut footer)?;
    let count = u64::from(u32_at(&footer, 0));
    let descriptor = footer[4];
    if u32_at(&footer, 5) != SEEKABLE_FOOTER_MAGIC || descriptor & 0x7C != 0 {
        return Ok(None);
    }
    let entry_size: u64 = if descriptor & 0x80 != 0 { 12 } else { 8 };
    let table_size = count * entry_size + FOOTER_SIZE + SKIPPABLE_HEADER_SIZE;
    if table_size > end {
        return Ok(None);
    }
    let data_end = end - table_size;
    let mut table = vec![0u8; (table_size - FOOTER_SIZE) as usize];
    file.seek(SeekFrom::Start(data_end))?;
    file.read_exact(&mut table)?;
    if u32_at(&table, 0) != SEEK_TABLE_MAGIC
        || u64::from(u32_at(&table, 4)) != table_size - SKIPPABLE_HEADER_SIZE
    {
        return Ok(None);
    }
    let mut entries = Vec::with_capacity(count as usize);
    let mut comp_total = 0u64;
    for row in table[SKIPPABLE_HEADER_SIZE as usize..].chunks_exact(entry_size as usize) {
        let entry = Entry {
            comp: u32_at(row, 0),
            raw: u32_at(row, 4),
        };
        if entry.comp == 0 || entry.raw as usize > BLOCK_SIZE {
            return Ok(None);
        }
        comp_total += u64::from(entry.comp);
        entries.push(entry);
    }
    let Some(start) = data_end.checked_sub(comp_total) else {
        return Ok(None);
    };
    if !empty_entries_are_empty(file, start, &entries)? {
        return Ok(None);
    }
    Ok(Some((
        new_layout(entries, data_end, false, true, true),
        start,
    )))
}

/// An entry without data is skipped when reading. One flipped bit in a size can make a
/// real block look like that, and nothing else in the table would reveal it.
fn empty_entries_are_empty<R: Read + Seek>(
    file: &mut R,
    start: u64,
    entries: &[Entry],
) -> io::Result<bool> {
    const HEADER_SIZE: usize = 64;
    let mut buf = [0u8; HEADER_SIZE];
    let mut offset = start;
    for entry in entries {
        if entry.raw == 0 {
            let available = (entry.comp as usize).min(HEADER_SIZE);
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut buf[..available])?;
            let empty = match parse_frame(&buf[..available], offset) {
                Ok(Parsed::Skippable { total, .. }) => total == u64::from(entry.comp),
                Ok(Parsed::Data { comp, raw: 0 }) => comp == entry.comp,
                _ => false,
            };
            if !empty {
                return Ok(false);
            }
        }
        offset += u64::from(entry.comp);
    }
    Ok(true)
}

/// Walks the frames one by one. This is what lets a file survive a crash of its
/// writer, or be built with `cat a.z64 b.z64`.
fn scan<R: Read + Seek>(file: &mut R, file_len: u64) -> io::Result<Layout> {
    let window = max_frame_size();
    let mut buf = vec![0u8; window];
    let mut entries = Vec::new();
    let mut pos = 0u64;
    let mut truncated = false;
    let mut finished = false;
    while pos < file_len {
        let available = (file_len - pos).min(window as u64) as usize;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..available])?;
        match parse_frame(&buf[..available], pos)? {
            Parsed::Data { comp, raw } => {
                entries.push(Entry { comp, raw });
                pos += u64::from(comp);
            }
            Parsed::Skippable { magic, total } => {
                if pos + total > file_len {
                    truncated = true;
                    break;
                }
                if magic == SEEK_TABLE_MAGIC && pos + total == file_len {
                    // It did not validate, or covers only the end of a `cat`. Either way
                    // the frames are all there, and appending will replace it.
                    finished = true;
                    break;
                }
                if must_check_frame(magic, total) {
                    let mut frame = vec![0u8; total as usize];
                    file.seek(SeekFrom::Start(pos))?;
                    file.read_exact(&mut frame)?;
                    if !frame_is_sound(magic, &frame) {
                        return Err(invalid(format!("damaged trailer at offset {pos}")));
                    }
                }
                let comp =
                    u32::try_from(total).map_err(|_| invalid("skippable frame too large"))?;
                entries.push(Entry { comp, raw: 0 });
                pos += total;
            }
            Parsed::Incomplete => {
                truncated = true;
                break;
            }
        }
    }
    Ok(new_layout(entries, pos, truncated, false, finished))
}

/// Removes the line index from `entries`: it is rewritten whenever the file changes.
/// An index that does not match the blocks is dropped and not fatal.
fn read_line_index<R: Read + Seek>(file: &mut R, layout: &mut Layout) -> io::Result<()> {
    let Some(last) = layout
        .entries
        .last()
        .copied()
        .filter(|entry| entry.raw == 0)
    else {
        return Ok(());
    };
    let start = layout.data_end - u64::from(last.comp);
    if (last.comp as usize) < TAGGED_HEADER_SIZE + 4 {
        return Ok(());
    }
    let mut header = [0u8; TAGGED_HEADER_SIZE];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut header)?;
    if u32_at(&header, 0) != LINES_MAGIC || header[8..12] != TAG {
        return Ok(());
    }
    let mut payload = vec![0u8; last.comp as usize - TAGGED_HEADER_SIZE];
    file.read_exact(&mut payload)?;
    layout.entries.pop();
    layout.data_end = start;

    let (counts, stored) = payload.split_at(payload.len() - 4);
    let blocks = layout.entries.iter().filter(|entry| entry.raw > 0).count();
    if u32_at(stored, 0) == checksum(counts) && counts.len() == blocks * 4 {
        layout.lines = Some(
            counts
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect(),
        );
    }
    Ok(())
}
