//! Compressed files: `.gz`, `.zst` and a `.zip` of tables are unpacked beside themselves, and
//! what comes out opens as any file does (a CSV as it is, a workbook or a JSON through its own
//! import). A zip with several tables is listed like a workbook's sheets.

use super::{check, io_err, tick, Imported, Sheet};
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};

// As for a workbook (xlsx_read.rs): past this size, a member compressing better than tables
// ever do is an archive made to fill the disk.
const MAX_RATIO: u64 = 500;
const RATIO_FROM: u64 = 256 << 20;

/// What a zip can hold that opens as a table.
const TABLES: &[&str] = &["csv", "tsv", "txt", "tab", "psv", "dat", "json", "jsonl", "ndjson", "xlsx", "xlsm", "xls", "xlsb", "ods", "sqlite", "sqlite3", "db", "db3", "parquet", "parq", "pqt"];

/// Where `name.csv.gz` is unpacked: `name.csv`. A name with nothing left to tell its kind
/// (`data.gz`): `data.csv`.
pub fn out_path(src: &Path) -> PathBuf {
    let stem = src.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    src.with_file_name(if Path::new(&stem).extension().is_some() { stem } else { format!("{stem}.csv") })
}

/// Copies what `r` unpacks to, `packed` bytes in all. Progress: megabytes written.
fn copy<W: Write>(mut r: impl Read, packed: u64, out: &mut W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<u64, String> {
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        check(cancel)?;
        let n = r.read(&mut buf).map_err(|e| format!("not a readable compressed file ({e})"))?;
        if n == 0 { return Ok(total); }
        total += n as u64;
        if total > RATIO_FROM && total > MAX_RATIO * packed.max(1) {
            return Err(format!("it decompresses more than {MAX_RATIO} times over: a booby-trapped archive"));
        }
        out.write_all(&buf[..n]).map_err(io_err)?;
        if total >> 20 != (total - n as u64) >> 20 { tick(progress, 1); }
    }
}

/// `src`, gzip or zstd by its extension, unpacked into `out`.
pub fn unpack<W: Write>(src: &Path, mut out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let f = File::open(src).map_err(io_err)?;
    let packed = f.metadata().map_err(io_err)?.len();
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let size = if matches!(ext.as_str(), "zst" | "zstd") {
        let d = zstd::stream::read::Decoder::new(f).map_err(|e| format!("not a readable zstd file ({e})"))?;
        copy(d, packed, &mut out, progress, cancel)?
    } else {
        // Several gzip members end to end are one file (`cat a.gz b.gz`).
        copy(flate2::read::MultiGzDecoder::new(BufReader::with_capacity(1 << 16, f)), packed, &mut out, progress, cancel)?
    };
    out.flush().map_err(io_err)?;
    Ok(Imported { size, ..Default::default() })
}

fn archive(src: &Path) -> Result<zip::ZipArchive<BufReader<File>>, String> {
    let f = File::open(src).map_err(io_err)?;
    zip::ZipArchive::new(BufReader::with_capacity(1 << 16, f)).map_err(|e| format!("not a readable zip ({e})"))
}

/// The base name of a member: never a path, whatever the archive says (`../../x`).
fn base_name(name: &str) -> Option<&str> {
    name.rsplit(['/', '\\']).next().filter(|n| !n.is_empty() && *n != ".." && !n.starts_with('.'))
}

/// The tables a zip holds: its members with a table's extension, as sheets. `dim`: their size is
/// no row count, so none.
pub fn members(src: &Path) -> Result<Vec<Sheet>, String> {
    let mut z = archive(src)?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let m = z.by_index_raw(i).map_err(|e| e.to_string())?;
        let name = m.name().map(|n| n.into_owned()).unwrap_or_default();
        if m.is_dir() || name.starts_with("__MACOSX/") { continue; }
        let Some(base) = base_name(&name) else { continue };
        let ext = Path::new(base).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if TABLES.contains(&ext.as_str()) {
            out.push(Sheet { name, hidden: false, filled: true, view: false, dim: None, approx: false });
        }
    }
    Ok(out)
}

/// Where a zip's member is unpacked: beside the zip, its own base name.
pub fn member_path(src: &Path, member: &str) -> PathBuf {
    src.with_file_name(base_name(member).unwrap_or("table.csv"))
}

/// One member of the zip into `out`.
pub fn unzip<W: Write>(src: &Path, member: &str, mut out: W, progress: Option<&AtomicUsize>, cancel: &AtomicBool) -> Result<Imported, String> {
    let mut z = archive(src)?;
    let m = z.by_name(member).map_err(|e| format!("cannot read \"{member}\" in this zip ({e})"))?;
    let packed = m.compressed_size();
    let size = copy(m, packed, &mut out, progress, cancel)?;
    out.flush().map_err(io_err)?;
    Ok(Imported { sheet: Some(member.to_string()), size, ..Default::default() })
}
