//! A table's edits as a file of their own, for crash recovery: what `Table` holds over its
//! file — the columns' layers, the column map, the row order, line 1's reading, the line
//! break — with the file's stamp. Replayed over the very file it was made on (same stamp,
//! same records), it gives the table back; a file changed since is refused. A few bytes per
//! edited cell, where a copy of the table would rewrite the whole file.

use crate::base::{Base, OpenOptions, Stamp};
use crate::enc::Enc;
use crate::table::{Layer, Layers, State, Table};
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const MAGIC: &[u8; 8] = b"CSVFABJ\0";
const VERSION: u32 = 1;

/// The line breaks a table can choose (`State::eol`).
const EOLS: [&[u8]; 3] = [b"\n", b"\r\n", b"\r"];

/// Writes `st`'s edits; `opts`: how its file was read (to read it the same way again).
pub fn write(st: &State, opts: OpenOptions, mut w: impl Write) -> io::Result<()> {
    let b = &st.base;
    let stamp = b.stamp.ok_or_else(|| io::Error::other("no file behind the table"))?;
    let mut o = Vec::with_capacity(1 << 16);
    o.extend_from_slice(MAGIC);
    put32(&mut o, VERSION);
    o.push(opts.enc.and_then(|e| Enc::ALL.iter().position(|&x| x == e)).map_or(0, |k| k as u8 + 1));
    o.extend_from_slice(&opts.delim.map_or([0, 0], |d| [1, d]));
    put64(&mut o, stamp.len);
    let t = stamp.mtime.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok());
    o.push(t.is_some() as u8);
    put64(&mut o, t.map_or(0, |d| d.as_secs()));
    put32(&mut o, t.map_or(0, |d| d.subsec_nanos()));
    put64(&mut o, stamp.ino);
    put64(&mut o, b.n as u64);
    o.push(st.header as u8);
    put64(&mut o, st.ncols as u64);
    o.push(st.eol.and_then(|e| EOLS.iter().position(|&x| x == e)).map_or(0, |k| k as u8 + 1));
    match &st.cmap {
        None => o.push(0),
        Some(m) => {
            o.push(1);
            put64(&mut o, m.len() as u64);
            for &x in m.iter() { put32(&mut o, x as u32); }
        }
    }
    match &st.order {
        None => o.push(0),
        Some(ord) => {
            o.push(1);
            put64(&mut o, ord.len() as u64);
            for &x in ord.iter() { put32(&mut o, x); }
        }
    }
    put64(&mut o, st.cols.len() as u64);
    for layers in st.cols.iter() {
        put64(&mut o, layers.len() as u64);
        for l in layers.iter() {
            let (ids, offs, arena) = l.parts();
            put64(&mut o, ids.len() as u64);
            for &x in ids { put32(&mut o, x); }
            put64(&mut o, offs.len() as u64);
            for &x in offs { put32(&mut o, x); }
            put64(&mut o, arena.len() as u64);
            // Big layers go straight out: the buffer is for the many small fields.
            w.write_all(&o)?;
            o.clear();
            w.write_all(arena)?;
        }
    }
    w.write_all(&o)?;
    w.flush()
}

/// How the journal's file was read and the stamp it had (None: not a journal of this version).
pub fn header(bytes: &[u8]) -> Option<(OpenOptions, Stamp)> {
    let mut r = Reader(bytes);
    if r.take(8)? != MAGIC || r.u32()? != VERSION { return None; }
    let enc = match r.u8()? { 0 => None, k => Some(*Enc::ALL.get(k as usize - 1)?) };
    let delim = match (r.u8()?, r.u8()?) { (0, _) => None, (_, d) => Some(d) };
    let len = r.u64()?;
    let has = r.u8()? == 1;
    let (secs, nanos) = (r.u64()?, r.u32()?);
    let mtime = has.then(|| SystemTime::UNIX_EPOCH + Duration::new(secs, nanos));
    Some((OpenOptions { enc, delim }, Stamp { len, mtime, ino: r.u64()? }))
}

/// The table the journal describes, over `base`: its file read again as the journal says
/// (`header`). Err when the file is no longer the one the edits were made on, or the
/// journal is damaged. The table is unsaved, without undo history.
pub fn replay(base: Arc<Base>, bytes: &[u8]) -> Result<Table, String> {
    let damaged = || "the recovery file is damaged".to_string();
    let (_, stamp) = header(bytes).ok_or_else(damaged)?;
    if base.stamp != Some(stamp) { return Err("the file changed on disk since these edits were made".into()); }
    let mut r = Reader(&bytes[8 + 4 + 1 + 2 + 8 + 1 + 8 + 4 + 8..]);
    (|| -> Option<Table> {
        if r.u64()? != base.n as u64 { return None; }
        let header = r.u8()? == 1;
        let ncols = r.u64()? as usize;
        let eol = match r.u8()? { 0 => None, k => Some(*EOLS.get(k as usize - 1)?) };
        let cmap = match r.u8()? {
            0 => None,
            _ => Some(Arc::new(r.vec32()?.into_iter().map(|x| x as i32).collect::<Vec<i32>>())),
        };
        let order = match r.u8()? { 0 => None, _ => Some(Arc::new(r.vec32()?)) };
        let ncl = r.u64()? as usize;
        if ncl != ncols || cmap.as_ref().is_some_and(|m| m.len() != ncols || m.iter().any(|&f| f < -1)) { return None; }
        let mut cols = Vec::with_capacity(ncols.min(1 << 20));
        for _ in 0..ncl {
            let k = r.u64()? as usize;
            let mut layers = Vec::with_capacity(k.min(64));
            for _ in 0..k {
                let (ids, offs) = (r.vec32()?, r.vec32()?);
                let n = r.u64()? as usize;
                layers.push(Arc::new(Layer::from_parts(ids, offs, r.take(n)?.to_vec())?));
            }
            cols.push(Layers::new(layers));
        }
        if !r.0.is_empty() { return None; }
        let st = State { base, cols: Arc::new(cols), order, cmap, header, ncols, eol };
        Some(Table::restored(st))
    })().ok_or_else(damaged)
}

fn put32(o: &mut Vec<u8>, x: u32) {
    o.extend_from_slice(&x.to_le_bytes());
}

fn put64(o: &mut Vec<u8>, x: u64) {
    o.extend_from_slice(&x.to_le_bytes());
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.0.len() { return None; }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn vec32(&mut self) -> Option<Vec<u32>> {
        let n = self.u64()? as usize;
        let b = self.take(n.checked_mul(4)?)?;
        Some(b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Cells, RowId};

    fn grid(t: &Table) -> Vec<Vec<String>> {
        (0..t.st.all_len()).map(|p| (0..t.st.ncols).map(|c| t.st.cell(t.st.all_id(p), c)).collect()).collect()
    }

    /// Every kind of edit written and replayed over the file: the same table, the same bytes
    /// saved; a file changed since, or a journal cut short, refused.
    #[test]
    #[cfg_attr(windows, ignore = "rewrites the file while it is mapped, to be refused: Windows forbids it to anyone")]
    fn round_trip_and_refusals() {
        let dir = std::env::temp_dir().join(format!("csvfab-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        let body: String = std::iter::once("id;name;note\r\n".to_string()).chain((0..3000).map(|i| format!("{i};n{i};\"x;{i}\"\r\n"))).collect();
        std::fs::write(&p, &body).unwrap();
        let opts = OpenOptions { enc: None, delim: Some(b';') };
        let open = || Arc::new(Base::open(&p, opts, None).unwrap());
        let mut t = Table::new(open(), None);
        let ids: Vec<RowId> = (0..t.st.len()).map(|p| t.st.data_id(p)).collect();
        t.set_cells(&[(ids[3], 1, "édité".into()), (ids[7], 2, String::new())]);
        let mut c = Cells::default();
        for &id in ids.iter().step_by(3) { c.push(id, &format!("v{id}")); }
        t.apply(crate::table::Patch::column(2, vec![c]));
        let no = std::sync::atomic::AtomicBool::new(false);
        t.apply(crate::table::Patch::fill(&t.st, &ids[100..200], &[0], "z", &no).unwrap());
        t.move_column(0, 2);
        t.insert_column(1, "Nouvelle");
        let new = t.insert_rows(5, 2);
        t.set_cells(&[(new[1], 1, "ligne neuve".into())]);
        t.delete_rows(&[ids[9]].into());
        t.set_eol(b"\n");
        t.set_header(false);
        let mut j = Vec::new();
        write(&t.st, opts, &mut j).unwrap();
        assert_eq!(header(&j).map(|h| h.0.delim), Some(Some(b';')));
        let r = replay(open(), &j).unwrap();
        assert!(r.dirty() && !r.can_undo());
        assert_eq!(grid(&r), grid(&t));
        assert_eq!((r.st.header, r.st.eol, r.st.ncols), (t.st.header, t.st.eol, t.st.ncols));
        let saved = |t: &Table| crate::save::write_to(&t.st, &Default::default(), Vec::new()).unwrap();
        assert_eq!(saved(&r), saved(&t));
        // Rows made after replaying get ids of their own.
        let mut r = r;
        let more = r.insert_rows(0, 1);
        assert!(more[0] > new[1]);
        // Damaged anywhere: refused, never a wrong table.
        for cut in [0, 10, j.len() / 2, j.len() - 1] {
            assert!(replay(open(), &j[..cut]).is_err(), "cut at {cut}");
        }
        let mut bad = j.clone();
        let k = bad.len() - 3;
        bad[k] = 0xFF; // in the last layer's text: not UTF-8
        assert!(replay(open(), &bad).is_err());
        // The file changed on disk since: refused.
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&p, body.replace("n5;", "N5;")).unwrap();
        let e = replay(open(), &j).err().unwrap();
        assert!(e.contains("changed"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
