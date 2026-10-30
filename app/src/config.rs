//! Settings and recent files, in a small text file: `key=value` per line.
//! Same directory as the web app used: ~/.local/state/csvfab (Linux),
//! ~/Library/Application Support/csvfab (macOS), %LOCALAPPDATA%\csvfab (Windows).

use std::path::PathBuf;

pub fn dir() -> PathBuf {
    // The app's own tests (off screen) never read nor write the user's settings and recent
    // files, nor each other's (a band one turns off would move the rows another clicks).
    if cfg!(test) { return std::env::temp_dir().join(format!("csvfab-test-state-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], "")); }
    let home = || std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(home).join("csvfab")
    } else if cfg!(target_os = "macos") {
        home().join("Library/Application Support/csvfab")
    } else {
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")).join("csvfab")
    }
}

const MAX_RECENT: usize = 12;

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Config {
    pub theme: Option<String>,
    pub mono_font: Option<String>,
    /// The screen's scale the window last ran at (the next start draws its first frame for it).
    pub scale: Option<f32>,
    /// The window was maximized when last seen (None: never run — it opens maximized).
    pub maximized: Option<bool>,
    /// The window's size when last not maximized (points).
    pub size: Option<(f32, f32)>,
    /// The grid's colour level (`theme::ALL`, `NO_TYPES`, `RAW`; None: all).
    pub look: Option<u8>,
    /// The bands around the grid, for every tab (None: shown, as in the web).
    pub show_filters: Option<bool>,
    pub show_charts: Option<bool>,
    pub show_totals: Option<bool>,
    /// The rows' density (an index of `theme::DENSITIES`; None: Normal).
    pub density: Option<u8>,
    /// The column inspector docked at the right (None: off).
    pub inspector: Option<bool>,
    pub recent: Vec<PathBuf>,
    /// When each recent file was last opened (seconds since 1970).
    pub opened: std::collections::HashMap<PathBuf, u64>,
}

impl Config {
    fn path() -> PathBuf {
        dir().join("native.conf")
    }

    pub fn load() -> Config {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            // A first start: the web app's (1.x) settings and recent files, if it ran here.
            if cfg!(test) { return c; }
            if let Some(v1) = v1::import(&dir()) { v1.save(); return v1; }
            return c;
        };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "theme" => c.theme = Some(v.to_string()),
                "mono_font" => c.mono_font = Some(v.to_string()),
                "scale" => c.scale = v.parse().ok(),
                "maximized" => c.maximized = v.parse().ok(),
                "size" => c.size = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))),
                "look" => c.look = v.parse().ok(),
                "show_filters" => c.show_filters = v.parse().ok(),
                "show_charts" => c.show_charts = v.parse().ok(),
                "show_totals" => c.show_totals = v.parse().ok(),
                "density" => c.density = v.parse().ok(),
                "inspector" => c.inspector = v.parse().ok(),
                "recent" => {
                    let (at, p) = match v.split_once('\t') { Some((t, p)) if t.bytes().all(|b| b.is_ascii_digit()) => (t.parse().ok(), p), _ => (None, v) };
                    let p = PathBuf::from(p);
                    if let Some(t) = at { c.opened.insert(p.clone(), t); }
                    c.recent.push(p);
                }
                _ => {}
            }
        }
        c
    }

    /// Written on a thread of its own, in the order asked (a start that opens files, a window
    /// resized, never waits on the disk); tests write at once.
    pub fn save(&self) {
        if cfg!(test) { return self.write(); }
        static TX: std::sync::Mutex<Option<std::sync::mpsc::Sender<Config>>> = std::sync::Mutex::new(None);
        let mut tx = TX.lock().unwrap_or_else(|e| e.into_inner());
        let tx = tx.get_or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Config>();
            let _ = std::thread::Builder::new().name("csvfab-config".into()).spawn(move || {
                // Only the last of a burst matters.
                while let Ok(mut c) = rx.recv() {
                    while let Ok(n) = rx.try_recv() { c = n; }
                    c.write();
                }
            });
            tx
        });
        let _ = tx.send(self.clone());
    }

    /// Written beside and renamed over: a crash mid-write never loses the settings.
    fn write(&self) {
        let mut out = String::new();
        if let Some(t) = &self.theme { out += &format!("theme={t}\n"); }
        if let Some(f) = &self.mono_font { out += &format!("mono_font={f}\n"); }
        if let Some(s) = self.scale { out += &format!("scale={s}\n"); }
        if let Some(m) = self.maximized { out += &format!("maximized={m}\n"); }
        if let Some((w, h)) = self.size { out += &format!("size={w}x{h}\n"); }
        if let Some(l) = self.look { out += &format!("look={l}\n"); }
        if let Some(b) = self.show_filters { out += &format!("show_filters={b}\n"); }
        if let Some(b) = self.show_charts { out += &format!("show_charts={b}\n"); }
        if let Some(b) = self.show_totals { out += &format!("show_totals={b}\n"); }
        if let Some(d) = self.density { out += &format!("density={d}\n"); }
        if let Some(b) = self.inspector { out += &format!("inspector={b}\n"); }
        // When each was last opened (the welcome screen's "ago"), then its path.
        for r in &self.recent {
            match self.opened.get(r) {
                Some(t) => out += &format!("recent={t}\t{}\n", r.display()),
                None => out += &format!("recent={}\n", r.display()),
            }
        }
        let p = Self::path();
        let _ = std::fs::create_dir_all(dir());
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, out).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    pub fn add_recent(&mut self, p: &std::path::Path) {
        self.recent.retain(|x| x != p);
        self.recent.insert(0, p.to_path_buf());
        self.recent.truncate(MAX_RECENT);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.opened.insert(p.to_path_buf(), now);
        let keep: std::collections::HashSet<&PathBuf> = self.recent.iter().collect();
        self.opened.retain(|k, _| keep.contains(k));
    }
}

/// The web app's settings (its Chromium profile's localStorage) and recent files (its
/// IndexedDB), read from the LevelDB logs those are kept in — no LevelDB library: a log is
/// records of write batches, the last write of a key wins. Values compacted into a table file
/// (`.ldb`, compressed) are not read: the logs hold what was written since the last compaction,
/// which for these few keys is nearly always all of it.
pub mod v1 {
    use super::Config;
    use std::path::{Path, PathBuf};

    /// The writes (key, value; None: deleted) of a LevelDB log, in order.
    pub fn puts(log: &[u8]) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
        const BLOCK: usize = 32768;
        let (mut out, mut rec, mut i) = (Vec::new(), Vec::new(), 0usize);
        let mut batches: Vec<Vec<u8>> = Vec::new();
        while i + 7 <= log.len() {
            let left = BLOCK - i % BLOCK;
            if left < 7 { i += left; continue; }
            let len = u16::from_le_bytes([log[i + 4], log[i + 5]]) as usize;
            let ty = log[i + 6];
            i += 7;
            let Some(frag) = log.get(i..i + len) else { break };
            i += len;
            match ty {
                1 => batches.push(frag.to_vec()),
                2 => rec = frag.to_vec(),
                3 => rec.extend_from_slice(frag),
                4 => { rec.extend_from_slice(frag); batches.push(std::mem::take(&mut rec)); }
                _ => i += (BLOCK - i % BLOCK) % BLOCK,
            }
        }
        let varint = |b: &[u8], i: &mut usize| -> Option<usize> {
            let (mut r, mut s) = (0usize, 0);
            loop {
                let x = *b.get(*i)?;
                *i += 1;
                r |= ((x & 0x7f) as usize) << s;
                s += 7;
                if x < 0x80 || s > 35 { return Some(r); }
            }
        };
        for b in batches {
            let Some(n) = b.get(8..12).map(|x| u32::from_le_bytes(x.try_into().unwrap())) else { continue };
            let mut i = 12;
            for _ in 0..n {
                let Some(&t) = b.get(i) else { break };
                i += 1;
                let Some(kl) = varint(&b, &mut i) else { break };
                let Some(k) = b.get(i..i + kl) else { break };
                i += kl;
                if t == 1 {
                    let Some(vl) = varint(&b, &mut i) else { break };
                    let Some(v) = b.get(i..i + vl) else { break };
                    i += vl;
                    out.push((k.to_vec(), Some(v.to_vec())));
                } else {
                    out.push((k.to_vec(), None));
                }
            }
        }
        out
    }

    /// The `.log` files of a LevelDB directory, oldest first.
    fn logs(dir: &Path) -> Vec<Vec<u8>> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "log")).collect();
        files.sort();
        files.iter().filter_map(|p| std::fs::read(p).ok()).collect()
    }

    /// localStorage's value bytes: a first byte 1 for Latin-1, 0 for UTF-16LE.
    fn text(v: &[u8]) -> Option<String> {
        match v.split_first()? {
            (1, rest) => Some(rest.iter().map(|&b| b as char).collect()),
            (0, rest) => String::from_utf16(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()).ok(),
            _ => None,
        }
    }

    /// The settings and recent files the web app kept under `state` (its profile there), if any.
    pub fn import(state: &Path) -> Option<Config> {
        let profile = state.join("profile").join("Default");
        let mut c = Config::default();
        let mut found = false;
        for log in logs(&profile.join("Local Storage").join("leveldb")) {
            for (k, v) in puts(&log) {
                // `_http://127.0.0.1:PORT\0\1csvfab-theme`: the web app's own origin.
                let Some(name) = k.windows(9).position(|w| w == b"\0\x01csvfab-").map(|p| String::from_utf8_lossy(&k[p + 2..]).into_owned()) else { continue };
                if !k.starts_with(b"_http://127.0.0.1:") { continue; }
                let v = v.as_deref().and_then(text);
                found = true;
                let on = v.as_deref().map(|v| v != "0");
                match name.as_str() {
                    "csvfab-theme" => c.theme = v.filter(|t| !t.is_empty()),
                    "csvfab-mono" => c.mono_font = v.filter(|t| !t.is_empty()),
                    "csvfab-filter-row" => c.show_filters = on,
                    "csvfab-sparklines" => c.show_charts = on,
                    "csvfab-totals" => c.show_totals = on,
                    "csvfab-type-colors" => c.look = on.map(|on| if on { crate::theme::ALL } else { crate::theme::NO_TYPES }),
                    _ => {}
                }
            }
        }
        // IndexedDB: the last value written under the key "recents" (in UTF-16BE in the key),
        // the paths inside it, newest first.
        let key: Vec<u8> = "recents".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let dbs = std::fs::read_dir(profile.join("IndexedDB")).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("http_127.0.0.1_") && n.to_string_lossy().ends_with(".indexeddb.leveldb")));
        let mut last: Option<Vec<u8>> = None;
        for db in dbs {
            for log in logs(&db) {
                for (k, v) in puts(&log) {
                    if k.windows(key.len()).any(|w| w == key) { if let Some(v) = v { last = Some(v); } }
                }
            }
        }
        if let Some(v) = last {
            for p in paths_in(&v) {
                if !c.recent.contains(&p) && c.recent.len() < super::MAX_RECENT { c.recent.push(p); }
            }
            found |= !c.recent.is_empty();
        }
        found.then_some(c)
    }

    /// The absolute paths written in a value, in order: a length byte, then the path (as both
    /// V8's serialization and Chromium's own records put them).
    pub fn paths_in(v: &[u8]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut i = 1;
        while i < v.len() {
            let n = v[i - 1] as usize;
            let abs = v[i] == b'/' || (i + 2 < v.len() && v[i].is_ascii_alphabetic() && v[i + 1] == b':' && v[i + 2] == b'\\');
            if abs && n > 2 && i + n <= v.len() {
                if let Ok(s) = std::str::from_utf8(&v[i..i + n]) {
                    if !s.contains(|c: char| c.is_control()) && Path::new(s).extension().is_some() {
                        out.push(PathBuf::from(s));
                        i += n;
                        continue;
                    }
                }
            }
            i += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A LevelDB log of one write batch, as Chromium writes them.
    fn log(puts: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut b = vec![0u8; 8];
        b.extend_from_slice(&(puts.len() as u32).to_le_bytes());
        for (k, v) in puts {
            b.push(1);
            b.push(k.len() as u8);
            b.extend_from_slice(k);
            b.push(v.len() as u8);
            b.extend_from_slice(v);
        }
        let mut out = vec![0, 0, 0, 0];
        out.extend_from_slice(&(b.len() as u16).to_le_bytes());
        out.push(1);
        out.extend(b);
        out
    }

    /// The density and the inspector are remembered; a config without them means Normal, off.
    /// (Each test thread has its own settings directory: `dir`.)
    #[test]
    fn density_and_inspector_round_trip() {
        let c = Config::load();
        assert_eq!((c.density, c.inspector), (None, None));
        Config { density: Some(2), inspector: Some(true), ..Default::default() }.save();
        let c = Config::load();
        assert_eq!((c.density, c.inspector), (Some(2), Some(true)));
    }

    #[test]
    fn web_settings_and_recents_imported() {
        let state = std::env::temp_dir().join(format!("csvfab-v1-{}", std::process::id()));
        let ls = state.join("profile/Default/Local Storage/leveldb");
        let idb = state.join("profile/Default/IndexedDB/http_127.0.0.1_8787.indexeddb.leveldb");
        std::fs::create_dir_all(&ls).unwrap();
        std::fs::create_dir_all(&idb).unwrap();
        let k = |n: &str| [b"_http://127.0.0.1:8787\0\x01".as_slice(), n.as_bytes()].concat();
        let (theme, totals, mono) = (k("csvfab-theme"), k("csvfab-totals"), k("csvfab-mono"));
        std::fs::write(ls.join("000003.log"), log(&[(&theme, b"\x01dark"), (&totals, b"\x011"), (&theme, b"\x01latte"), (&mono, b"\x01Iosevka")])).unwrap();
        let key: Vec<u8> = [&[0u8, 1, 1, 3, 1, 7][..], &"recents".encode_utf16().flat_map(|u| u.to_be_bytes()).collect::<Vec<_>>()].concat();
        let val = b"\x028\x08\x00\x1a4\n\x0d/home/a/x.csv\x12\x00\x1a\x05x.csv\x02N\x08\x00\x1aJ\n\x10/home/b/data.tsv\x12\x00".to_vec();
        std::fs::write(idb.join("000007.log"), log(&[(&key, &val)])).unwrap();
        let c = v1::import(&state).unwrap();
        assert_eq!(c.theme.as_deref(), Some("latte"));
        assert_eq!(c.mono_font.as_deref(), Some("Iosevka"));
        assert_eq!(c.show_totals, Some(true));
        assert_eq!(c.recent, [PathBuf::from("/home/a/x.csv"), PathBuf::from("/home/b/data.tsv")]);
        let _ = std::fs::remove_dir_all(&state);
        assert!(v1::import(&state).is_none());
    }
}
