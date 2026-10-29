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

#[derive(Default, Clone, PartialEq)]
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
    pub recent: Vec<PathBuf>,
}

impl Config {
    fn path() -> PathBuf {
        dir().join("native.conf")
    }

    pub fn load() -> Config {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(Self::path()) else { return c };
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
                "recent" => c.recent.push(PathBuf::from(v)),
                _ => {}
            }
        }
        c
    }

    /// Written beside and renamed over: a crash mid-write never loses the settings.
    pub fn save(&self) {
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
        for r in &self.recent { out += &format!("recent={}\n", r.display()); }
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
    }
}
