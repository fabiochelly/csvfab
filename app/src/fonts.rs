//! Fonts: the grid's monospace and the chrome's UI font, found on the system
//! the way the web app's CSS stacks are (`--mono-default`, `--ui`), and
//! installed in egui with egui's own fonts behind them for missing glyphs.
//!
//! Where they are: fontconfig knows (`fc-list`, one call, ~20 ms for 800
//! faces) — honouring the user's aliases as Chromium did; without it (macOS,
//! Windows) the usual font folders are scanned and each file's tables read.
//! Either costs more than a startup can spend, so what was picked is kept in
//! a small file beside the settings, valid while the font folders are unchanged (their
//! mtimes and their subfolders', a few dozen stats). A cached start only
//! reads the font files.
//!
//! Weights: `Monospace` and `Proportional` are the regular faces; the named
//! families `MONO_BOLD` (600, for values in bold) and `UI_MEDIUM` (500),
//! `UI_SEMIBOLD` (600, titles, chips) take the closest face of the same
//! family (CSS matching rules), or the variable font at that `wght`. They
//! always exist — the regular face when there is no other.

use eframe::egui;
use egui::epaint::text::{Blob, FontData, FontDefinitions, FontFamily, FontTweak, VariationCoords};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// `--mono-default` of app.css, first found wins.
pub const MONO_STACK: &[&str] = &[
    "MonoLisa", "MonolisaNerd", "MonoLisa Nerd Font", "Cascadia Code", "Monaco", "JetBrains Mono",
    "JetBrainsMono Nerd Font", "Consolas",
];
/// `--ui` of app.css: Inter, then the system's own UI font (`system-ui`), then the usual ones.
pub const UI_STACK: &[&str] = &[
    "Inter", "SF Pro Text", "SF Pro", "System Font", ".SF NS", "Segoe UI Variable Text", "Segoe UI", "Roboto",
    "Helvetica Neue", "Noto Sans", "Liberation Sans", "Arial", "DejaVu Sans",
];
pub const MONO_BOLD: &str = "mono-bold";
pub const UI_MEDIUM: &str = "ui-medium";
pub const UI_SEMIBOLD: &str = "ui-semibold";

/// A face picked for a role.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub family: String,
    pub path: PathBuf,
    /// The face in a collection (.ttc).
    pub index: u32,
    /// The `wght` set on a variable font, `None` for a static face.
    pub wght: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct FontsInfo {
    pub mono: Option<Pick>,
    pub mono_bold: Option<Pick>,
    pub ui: Option<Pick>,
    pub ui_medium: Option<Pick>,
    pub ui_semibold: Option<Pick>,
    /// The picks came from the cache (no fc-list, no scan).
    pub cached: bool,
    /// Time spent in `install`: resolving, reading the files, handing them to egui.
    pub ms: f32,
}

const ROLES: [(&str, Role); 5] = [
    ("mono", Role::Mono(400)),
    ("mono_bold", Role::Mono(600)),
    ("ui", Role::Ui(400)),
    ("ui_medium", Role::Ui(500)),
    ("ui_semibold", Role::Ui(600)),
];

#[derive(Clone, Copy)]
enum Role {
    Mono(u16),
    Ui(u16),
}

/// Install the fonts in `ctx`. `mono_pick`: a family from `list_monospace()`
/// (the font dialog's choice), `"monospace"` for the system's own; `None` or
/// a family no longer installed: the stack.
pub fn install(ctx: &egui::Context, mono_pick: Option<&str>) -> FontsInfo {
    let t0 = Instant::now();
    let pick = mono_pick.filter(|s| !s.is_empty()).unwrap_or("");
    let fp = fingerprint();
    let (picks, cached) = match read_cache(fp, pick) {
        Some(p) => (p, true),
        None => {
            let p = resolve(&catalogue(), pick);
            write_cache(fp, pick, &p);
            (p, false)
        }
    };
    let mut info = FontsInfo { cached, ..Default::default() };
    let mut defs = FontDefinitions::default();
    let mut blobs: HashMap<PathBuf, Option<Blob>> = HashMap::new();
    for ((role, _), p) in ROLES.iter().zip(picks) {
        let Some(p) = p else { continue };
        // A variable font or a collection serves several roles from one read.
        let blob = blobs.entry(p.path.clone()).or_insert_with(|| map(&p.path));
        let Some(blob) = blob.clone() else { continue };
        let mut data = FontData::from_blob(blob, p.index);
        if let Some(w) = p.wght {
            data = data.tweak(FontTweak { coords: VariationCoords::new([(b"wght", w)]), ..Default::default() });
        }
        defs.font_data.insert(format!("csvfab-{role}"), Arc::new(data));
        *match *role {
            "mono" => &mut info.mono,
            "mono_bold" => &mut info.mono_bold,
            "ui" => &mut info.ui,
            "ui_medium" => &mut info.ui_medium,
            _ => &mut info.ui_semibold,
        } = Some(p);
    }
    // Each family: its face, the regular one of its kind, then the other kind (as a browser falls
    // back to the next font for a glyph), then egui's own fonts (Hack, Ubuntu, its icons).
    let has = |k: &str| defs.font_data.contains_key(k);
    let chain = |first: &str, regular: &str, other: &str, base: &[String]| -> Vec<String> {
        let mut v: Vec<String> = [first, regular, other].iter().filter(|k| has(k)).map(|k| k.to_string()).collect();
        v.dedup();
        v.extend(base.iter().cloned());
        v
    };
    let mono_base = defs.families[&FontFamily::Monospace].clone();
    let ui_base = defs.families[&FontFamily::Proportional].clone();
    let fams = [
        (FontFamily::Monospace, chain("csvfab-mono", "csvfab-mono", "csvfab-ui", &mono_base)),
        (FontFamily::Name(MONO_BOLD.into()), chain("csvfab-mono_bold", "csvfab-mono", "csvfab-ui", &mono_base)),
        (FontFamily::Proportional, chain("csvfab-ui", "csvfab-ui", "csvfab-mono", &ui_base)),
        (FontFamily::Name(UI_MEDIUM.into()), chain("csvfab-ui_medium", "csvfab-ui", "csvfab-mono", &ui_base)),
        (FontFamily::Name(UI_SEMIBOLD.into()), chain("csvfab-ui_semibold", "csvfab-ui", "csvfab-mono", &ui_base)),
    ];
    for (f, list) in fams {
        defs.families.insert(f, list);
    }
    // The font dialog's samples: each family in its own face (mapped, as above), the mono and
    // ui fonts behind it for what it lacks.
    let samples = SAMPLES.lock().map(|v| v.clone()).unwrap_or_default();
    let behind: Vec<String> = ["csvfab-mono", "csvfab-ui"].iter().filter(|k| defs.font_data.contains_key(**k)).map(|k| k.to_string()).chain(mono_base.iter().cloned()).collect();
    for p in samples {
        let Some(blob) = blobs.entry(p.path.clone()).or_insert_with(|| map(&p.path)).clone() else { continue };
        let mut data = FontData::from_blob(blob, p.index);
        if let Some(w) = p.wght {
            data = data.tweak(FontTweak { coords: VariationCoords::new([(b"wght", w)]), ..Default::default() });
        }
        let key = format!("{SAMPLE}{}", p.family);
        defs.font_data.insert(key.clone(), Arc::new(data));
        defs.families.insert(FontFamily::Name(key.clone().into()), std::iter::once(key).chain(behind.iter().cloned()).collect());
    }
    ctx.set_fonts(defs);
    info.ms = t0.elapsed().as_secs_f32() * 1000.0;
    info
}

/// A font file, mapped rather than read: no copy on the heap (a few MB per face, more than
/// the rest of the app's own memory at rest), only the glyphs' pages ever read from disk,
/// and shared with every other process using the font. Font files are replaced, not
/// rewritten, when updated (as browsers rely on when they map them too). Read if mapping fails.
fn map(path: &Path) -> Option<Blob> {
    let f = std::fs::File::open(path).ok()?;
    // SAFETY: read-only map of a font file; see above for files changing underneath.
    match unsafe { memmap2::Mmap::map(&f) } {
        Ok(m) => Some(Arc::new(m) as Blob),
        Err(_) => std::fs::read(path).ok().map(|v| Arc::new(v) as Blob),
    }
}

/// The prefix of a sample's family (`sample_family`).
const SAMPLE: &str = "sample:";

/// The faces the font dialog shows its families in, while it is open (`set_samples`).
static SAMPLES: std::sync::Mutex<Vec<Pick>> = std::sync::Mutex::new(Vec::new());

/// The font dialog's families in their own faces (none once it closes): the fonts installed
/// again with them, `mono_pick` the font in use.
pub fn set_samples(ctx: &egui::Context, faces: Vec<Pick>, mono_pick: Option<&str>) {
    if let Ok(mut v) = SAMPLES.lock() {
        if v.is_empty() && faces.is_empty() { return; }
        *v = faces;
    }
    install(ctx, mono_pick);
}

/// The family a sample of `family` is drawn in, once installed (fonts change at the next frame).
pub fn sample_family(ctx: &egui::Context, family: &str) -> Option<FontFamily> {
    let f = FontFamily::Name(format!("{SAMPLE}{family}").into());
    ctx.fonts(|x| x.definitions().families.contains_key(&f)).then_some(f)
}

/// `list_monospace`, with the regular face of each family (for its sample).
pub fn list_monospace_faces() -> Vec<(String, Option<Pick>)> {
    let cat = catalogue();
    mono_names(&cat).into_iter().map(|n| { let p = best(&cat, &n, 400); (n, p) }).collect()
}

/// The installed monospace families, for the font dialog: fixed-width for Latin
/// letters (as `fontUsable()` of 32-font.js: emoji and icon fonts that call
/// themselves monospace are left out), a family named after a style folded into
/// its base ("Aptos Mono Bold" → "Aptos Mono"), sorted without case.
pub fn list_monospace() -> Vec<String> {
    mono_names(&catalogue())
}

fn mono_names(cat: &[Face]) -> Vec<String> {
    let mut seen: HashMap<String, bool> = HashMap::new();
    for f in cat.iter().filter(|f| f.mono && !f.italic) {
        let name = f.families[0].clone();
        if seen.contains_key(&name) {
            continue;
        }
        let ok = sfnt::latin_fixed(&f.path, f.index).unwrap_or(false);
        seen.insert(name, ok);
    }
    let names: Vec<String> = seen.into_iter().filter(|(_, ok)| *ok).map(|(n, _)| n).collect();
    let base: std::collections::HashSet<&str> = names.iter().map(|n| n.as_str()).filter(|n| strip_style(n).is_none()).collect();
    let mut out: Vec<String> =
        names.iter().map(|n| strip_style(n).filter(|b| base.contains(b)).unwrap_or(n).to_string()).collect();
    out.sort_by_key(|n| n.to_lowercase());
    out.dedup();
    out
}

/// "Aptos Mono Bold Italic" → Some("Aptos Mono").
fn strip_style(name: &str) -> Option<&str> {
    const STYLES: &[&str] = &[
        "thin", "extralight", "light", "regular", "medium", "semibold", "bold", "extrabold", "black", "italic",
        "oblique", "condensed", "wide", "semiwide",
    ];
    let mut end = name.len();
    for w in name.split(' ').rev() {
        if !STYLES.contains(&w.to_lowercase().as_str()) {
            break;
        }
        // A name made of style words only ("Bold") has no base to fold into.
        end = end.checked_sub(w.len() + 1)?;
    }
    (end < name.len() && end > 0).then(|| &name[..end])
}

// ---------------------------------------------------------------------------
// The installed faces

#[derive(Clone, Debug)]
struct Face {
    families: Vec<String>,
    path: PathBuf,
    index: u32,
    /// CSS weight; for a variable font, its `wght` range.
    weight: (u16, u16),
    italic: bool,
    mono: bool,
    variable: bool,
}

/// Every face fontconfig knows, else every face of the usual folders.
fn catalogue() -> Vec<Face> {
    fc_list().unwrap_or_else(scan_dirs)
}

fn fc_list() -> Option<Vec<Face>> {
    let out = std::process::Command::new("fc-list")
        .args(["--format", "%{family}\t%{weight}\t%{spacing}\t%{index}\t%{variable}\t%{slant}\t%{file}\n"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let faces: Vec<Face> = text.lines().filter_map(fc_line).collect();
    (!faces.is_empty()).then_some(faces)
}

fn fc_line(line: &str) -> Option<Face> {
    let f: Vec<&str> = line.splitn(7, '\t').collect();
    let [family, weight, spacing, index, variable, slant, file] = f[..] else { return None };
    let index: u32 = index.parse().ok()?;
    // Named instances of a variable font (index ≥ 2^16): the font itself is listed too, with its range.
    if index >= 1 << 16 || family.is_empty() {
        return None;
    }
    let w: Vec<f32> = weight.trim_matches(['[', ']']).split(' ').filter_map(|x| x.parse().ok()).collect();
    let weight = match w[..] {
        [a] => (fc_weight(a), fc_weight(a)),
        [a, b] => (fc_weight(a), fc_weight(b)),
        _ => (400, 400),
    };
    Some(Face {
        families: family.split(',').map(str::to_string).collect(),
        path: file.into(),
        index,
        weight,
        italic: slant.parse::<u32>().is_ok_and(|s| s > 0),
        mono: spacing.parse::<u32>().is_ok_and(|s| s >= 100),
        variable: variable == "True",
    })
}

/// fontconfig's weight scale → CSS weights (FcWeightToOpenType's table).
fn fc_weight(w: f32) -> u16 {
    const MAP: [(f32, f32); 11] = [
        (0.0, 100.0), (40.0, 200.0), (50.0, 300.0), (55.0, 350.0), (75.0, 380.0), (80.0, 400.0),
        (100.0, 500.0), (180.0, 600.0), (200.0, 700.0), (205.0, 800.0), (210.0, 900.0),
    ];
    let k = MAP.iter().position(|&(f, _)| w <= f).unwrap_or(MAP.len() - 1);
    if k == 0 || w >= MAP[k].0 {
        return MAP[k].1 as u16;
    }
    let ((f0, c0), (f1, c1)) = (MAP[k - 1], MAP[k]);
    (c0 + (w - f0) / (f1 - f0) * (c1 - c0)).round() as u16
}

fn font_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from);
    let mut d: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Some(w) = std::env::var_os("WINDIR") {
            d.push(Path::new(&w).join("Fonts"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            d.push(Path::new(&l).join("Microsoft").join("Windows").join("Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        d.extend(["/System/Library/Fonts", "/System/Library/Fonts/Supplemental", "/Library/Fonts"].map(PathBuf::from));
        d.extend(home.iter().map(|h| h.join("Library/Fonts")));
    } else {
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        d.extend(data.map(|x| x.join("fonts")));
        d.extend(home.iter().map(|h| h.join(".fonts")));
        d.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
    }
    d
}

/// Without fontconfig: every .ttf / .otf / .ttc of the font folders, read by their tables.
fn scan_dirs() -> Vec<Face> {
    let mut out = Vec::new();
    let mut stack = font_dirs();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(path);
                continue;
            }
            let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                continue;
            }
            for (index, i) in sfnt::faces(&path).unwrap_or_default() {
                out.push(Face {
                    families: i.families,
                    path: path.clone(),
                    index,
                    weight: i.wght.map_or((i.weight, i.weight), |(a, b)| (a as u16, b as u16)),
                    italic: i.italic,
                    mono: i.fixed,
                    variable: i.wght.is_some(),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Picking

fn resolve(cat: &[Face], mono_pick: &str) -> [Option<Pick>; 5] {
    let fam = |names: &[&str]| -> Option<String> {
        names.iter().find(|n| cat.iter().any(|f| has_family(f, n))).map(|n| n.to_string())
    };
    let system = |generic: &str| fc_match(generic).filter(|n| cat.iter().any(|f| has_family(f, n)));
    let mono = match mono_pick {
        "" => None,
        "monospace" => system("monospace"),
        p => fam(&[p]),
    }
    .or_else(|| fam(MONO_STACK))
    .or_else(|| system("monospace"))
    .or_else(|| fam(&["Menlo", "Courier New", "DejaVu Sans Mono", "Liberation Mono"]));
    let ui = fam(UI_STACK).or_else(|| system("sans-serif"));
    ROLES.map(|(_, role)| match role {
        Role::Mono(w) => mono.as_deref().and_then(|f| best(cat, f, w)),
        Role::Ui(w) => ui.as_deref().and_then(|f| best(cat, f, w)),
    })
}

fn has_family(f: &Face, name: &str) -> bool {
    f.families.iter().any(|x| x.eq_ignore_ascii_case(name))
}

/// The face of `family` (or of "`family` Variable", Inter's variable build) a browser draws
/// weight `w` with: CSS font matching, upright faces first; among equals, the smallest file.
fn best(cat: &[Face], family: &str, w: u16) -> Option<Pick> {
    let var_name = format!("{family} Variable");
    let faces: Vec<&Face> = cat.iter().filter(|f| has_family(f, family) || has_family(f, &var_name)).collect();
    let upright: Vec<&Face> = faces.iter().copied().filter(|f| !f.italic).collect();
    let pool = if upright.is_empty() { faces } else { upright };
    // CSS: 400 looks up to 500 first, 500 down to 400; below 400 lighter first, above 500 heavier first.
    let dist = |f: &Face| -> u32 {
        let (lo, hi) = f.weight;
        if (lo..=hi).contains(&w) {
            return 0;
        }
        let (near, w) = ((if hi < w { hi } else { lo }) as i32, w as i32);
        let d = (near - w).unsigned_abs();
        let preferred = match w {
            400 => near > w && near <= 500,
            500 => near < w && near >= 400,
            x if x < 400 => near < x,
            x => near > x,
        };
        if preferred { d } else { d + 1000 }
    };
    let size = |f: &Face| std::fs::metadata(&f.path).map_or(u64::MAX, |m| m.len());
    let d0 = pool.iter().map(|f| dist(f)).min()?;
    let f = pool.into_iter().filter(|f| dist(f) == d0).min_by_key(|f| (size(f), !f.variable))?;
    let wght = (f.variable && f.weight.0 != f.weight.1).then(|| w.clamp(f.weight.0, f.weight.1) as f32);
    Some(Pick { family: family.to_string(), path: f.path.clone(), index: f.index, wght })
}

/// The family fontconfig gives a generic name (monospace, sans-serif), when it is installed.
fn fc_match(generic: &str) -> Option<String> {
    let o = std::process::Command::new("fc-match").args(["-f", "%{family}", generic]).output().ok()?;
    let s = String::from_utf8_lossy(&o.stdout);
    s.split(',').next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

// ---------------------------------------------------------------------------
// The cache: the picks, valid while the font folders keep their mtimes

/// Beside the settings (config.rs): one file, a few lines.
fn cache_file() -> Option<PathBuf> {
    Some(crate::config::dir().join("fonts.cache"))
}

/// The font folders, their subfolders (a font installed in a new folder changes its parent;
/// in an existing one, that folder) and fontconfig's own settings (aliases change the picks).
fn fingerprint() -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut stamp = |p: &Path| {
        p.hash(&mut h);
        if let Ok(t) = std::fs::metadata(p).and_then(|m| m.modified()) {
            t.hash(&mut h);
        }
    };
    for d in font_dirs() {
        stamp(&d);
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut subs: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
        subs.sort();
        subs.iter().for_each(|s| stamp(s));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| Path::new(&home).join(".config"));
        stamp(&cfg.join("fontconfig/fonts.conf"));
        stamp(&cfg.join("fontconfig/conf.d"));
    }
    stamp(Path::new("/etc/fonts/conf.d"));
    h.finish()
}

fn read_cache(fp: u64, pick: &str) -> Option<[Option<Pick>; 5]> {
    let text = std::fs::read_to_string(cache_file()?).ok()?;
    let mut lines = text.lines();
    if lines.next()? != format!("{fp:016x}\t{pick}") {
        return None;
    }
    let mut out: [Option<Pick>; 5] = Default::default();
    for l in lines {
        let [role, index, wght, family, path] = l.splitn(5, '\t').collect::<Vec<_>>()[..] else { return None };
        let k = ROLES.iter().position(|(r, _)| *r == role)?;
        let path = PathBuf::from(path);
        if !path.exists() {
            return None;
        }
        out[k] = Some(Pick { family: family.into(), path, index: index.parse().ok()?, wght: wght.parse().ok() });
    }
    Some(out)
}

fn write_cache(fp: u64, pick: &str, picks: &[Option<Pick>; 5]) {
    let Some(file) = cache_file() else { return };
    let mut s = format!("{fp:016x}\t{pick}\n");
    for ((role, _), p) in ROLES.iter().zip(picks) {
        if let Some(p) = p {
            let w = p.wght.map_or(String::from("-"), |w| w.to_string());
            s += &format!("{role}\t{}\t{w}\t{}\t{}\n", p.index, p.family, p.path.display());
        }
    }
    // Best effort: without it the next start resolves again.
    let _ = std::fs::create_dir_all(file.parent().unwrap_or(Path::new("."))).and_then(|_| std::fs::write(file, s));
}

// ---------------------------------------------------------------------------
// The few OpenType tables the scan and the dialog need, read in place

mod sfnt {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::Path;

    pub struct Info {
        pub families: Vec<String>,
        pub weight: u16,
        pub italic: bool,
        /// `post.isFixedPitch`.
        pub fixed: bool,
        /// The `wght` axis of a variable font.
        pub wght: Option<(f32, f32)>,
    }

    struct Font {
        f: File,
        tables: Vec<([u8; 4], u32, u32)>,
    }

    fn be16(b: &[u8], o: usize) -> u16 {
        b.get(o..o + 2).map_or(0, |x| u16::from_be_bytes([x[0], x[1]]))
    }
    fn be32(b: &[u8], o: usize) -> u32 {
        b.get(o..o + 4).map_or(0, |x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
    }

    fn read_at(f: &mut File, off: u32, len: u32) -> std::io::Result<Vec<u8>> {
        let mut v = vec![0; len.min(1 << 24) as usize];
        f.seek(SeekFrom::Start(off as u64))?;
        f.read_exact(&mut v)?;
        Ok(v)
    }

    impl Font {
        fn open(f: File, offset: u32) -> std::io::Result<Font> {
            let mut f = f;
            let head = read_at(&mut f, offset, 12)?;
            let n = be16(&head, 4) as u32;
            let dir = read_at(&mut f, offset + 12, n * 16)?;
            let tables = dir.as_chunks::<16>().0.iter().map(|r| ([r[0], r[1], r[2], r[3]], be32(r, 8), be32(r, 12))).collect();
            Ok(Font { f, tables })
        }

        fn table(&mut self, tag: &[u8; 4]) -> Option<Vec<u8>> {
            let &(_, off, len) = self.tables.iter().find(|t| &t.0 == tag)?;
            read_at(&mut self.f, off, len).ok()
        }
    }

    /// The offsets of the faces of a file (one, or a collection's).
    fn offsets(f: &mut File) -> std::io::Result<Vec<u32>> {
        let head = read_at(f, 0, 12)?;
        if &head[..4] != b"ttcf" {
            return Ok(vec![0]);
        }
        let n = be32(&head, 8).min(256);
        let v = read_at(f, 12, n * 4)?;
        Ok((0..n as usize).map(|i| be32(&v, i * 4)).collect())
    }

    pub fn faces(path: &Path) -> std::io::Result<Vec<(u32, Info)>> {
        let mut file = File::open(path)?;
        let offs = offsets(&mut file)?;
        let mut out = Vec::new();
        for (i, off) in offs.into_iter().enumerate() {
            let Ok(mut font) = Font::open(file.try_clone()?, off) else { continue };
            let families = font.table(b"name").map(|n| names(&n)).unwrap_or_default();
            if families.is_empty() {
                continue;
            }
            let os2 = font.table(b"OS/2").unwrap_or_default();
            let post = font.table(b"post").unwrap_or_default();
            let wght = font.table(b"fvar").and_then(|v| {
                let (start, count, size) = (be16(&v, 4) as usize, be16(&v, 8) as usize, be16(&v, 10) as usize);
                (0..count).map(|k| start + k * size).find(|&a| v.get(a..a + 4) == Some(b"wght")).map(|a| {
                    let fixed = |o: usize| be32(&v, o) as i32 as f32 / 65536.0;
                    (fixed(a + 4), fixed(a + 12))
                })
            });
            out.push((
                i as u32,
                Info {
                    families,
                    weight: be16(&os2, 4).clamp(1, 1000),
                    italic: be16(&os2, 62) & 1 != 0,
                    fixed: be32(&post, 12) != 0,
                    wght,
                },
            ));
        }
        Ok(out)
    }

    /// The family names (typographic and legacy, English or Mac Roman).
    fn names(n: &[u8]) -> Vec<String> {
        let (count, store) = (be16(n, 2) as usize, be16(n, 4) as usize);
        let mut out: Vec<String> = Vec::new();
        for want in [16u16, 1] {
            for k in 0..count {
                let r = 6 + k * 12;
                let (plat, lang, id, len, off) = (be16(n, r), be16(n, r + 4), be16(n, r + 6), be16(n, r + 8) as usize, be16(n, r + 10) as usize);
                let Some(b) = n.get(store + off..store + off + len) else { continue };
                let s = match (plat, id == want) {
                    (3, true) if lang == 0x409 || lang & 0xff == 9 => {
                        String::from_utf16_lossy(&b.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect::<Vec<_>>())
                    }
                    (1, true) if lang == 0 => b.iter().map(|&c| c as char).collect(),
                    _ => continue,
                };
                if !s.is_empty() && !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        out
    }

    /// Whether the face draws i, W and 0 with one advance (a monospace text font, not an emoji
    /// or icon font that claims to be one): cmap + hmtx.
    pub fn latin_fixed(path: &Path, index: u32) -> std::io::Result<bool> {
        let mut file = File::open(path)?;
        let off = offsets(&mut file)?.get(index as usize).copied().unwrap_or(0);
        let mut font = Font::open(file, off)?;
        let (Some(cmap), Some(hhea), Some(hmtx)) = (font.table(b"cmap"), font.table(b"hhea"), font.table(b"hmtx")) else {
            return Ok(false);
        };
        let n_metrics = be16(&hhea, 34) as usize;
        let adv = |g: u16| be16(&hmtx, 4 * (g as usize).min(n_metrics.saturating_sub(1)));
        let glyphs: Vec<u16> = ['i', 'W', '0'].iter().map(|&c| glyph(&cmap, c as u32)).collect();
        Ok(glyphs.iter().all(|&g| g != 0) && glyphs.iter().all(|&g| adv(g) == adv(glyphs[0])))
    }

    fn glyph(cmap: &[u8], c: u32) -> u16 {
        let n = be16(cmap, 2) as usize;
        let subs: Vec<(u16, u16, usize)> = (0..n).map(|k| (be16(cmap, 4 + k * 8), be16(cmap, 6 + k * 8), be32(cmap, 8 + k * 8) as usize)).collect();
        let pick = subs.iter().find(|s| (s.0, s.1) == (3, 10)).or_else(|| subs.iter().find(|s| (s.0, s.1) == (3, 1))).or_else(|| subs.iter().find(|s| s.0 == 0));
        let Some(&(_, _, o)) = pick else { return 0 };
        match be16(cmap, o) {
            4 => {
                let seg = be16(cmap, o + 6) as usize / 2;
                let (ends, starts, deltas, ranges) = (o + 14, o + 16 + seg * 2, o + 16 + seg * 4, o + 16 + seg * 6);
                let Some(k) = (0..seg).find(|&k| be16(cmap, ends + k * 2) as u32 >= c) else { return 0 };
                let start = be16(cmap, starts + k * 2) as u32;
                if start > c {
                    return 0;
                }
                let (delta, ro) = (be16(cmap, deltas + k * 2), be16(cmap, ranges + k * 2) as usize);
                if ro == 0 {
                    return (c as u16).wrapping_add(delta);
                }
                let g = be16(cmap, ranges + k * 2 + ro + 2 * (c - start) as usize);
                if g == 0 { 0 } else { g.wrapping_add(delta) }
            }
            12 => {
                let groups = be32(cmap, o + 12) as usize;
                (0..groups)
                    .map(|k| o + 16 + k * 12)
                    .find(|&g| be32(cmap, g) <= c && c <= be32(cmap, g + 4))
                    .map_or(0, |g| (be32(cmap, g + 8) + c - be32(cmap, g)) as u16)
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights() {
        assert_eq!(fc_weight(80.0), 400);
        assert_eq!(fc_weight(180.0), 600);
        assert_eq!(fc_weight(200.0), 700);
        assert_eq!(fc_weight(0.0), 100);
        assert_eq!(fc_weight(90.0), 450);
    }

    #[test]
    fn style_names() {
        assert_eq!(strip_style("Aptos Mono Bold Italic"), Some("Aptos Mono"));
        assert_eq!(strip_style("Cascadia Code"), None);
        assert_eq!(strip_style("Bold"), None);
    }

    #[test]
    fn fc_lines() {
        let f = fc_line("Inter Variable\t[0 210]\t\t0\tTrue\t0\t/x/InterVariable.ttf").unwrap();
        assert_eq!((f.weight, f.variable, f.mono), ((100, 900), true, false));
        assert!(fc_line("Inter Variable\t50\t\t196608\tFalse\t100\t/x/I.ttf").is_none());
        let m = fc_line("MonolisaNerd,MonolisaNerd SemiBold\t180\t100\t0\tFalse\t0\t/x/M.ttf").unwrap();
        assert_eq!((m.families.len(), m.weight.0, m.mono), (2, 600, true));
    }

    #[test]
    fn picks_follow_css_matching() {
        let face = |w: u16, italic| Face {
            families: vec!["F".into()],
            path: PathBuf::from(format!("/nonexistent/{w}{italic}")),
            index: 0,
            weight: (w, w),
            italic,
            mono: true,
            variable: false,
        };
        let cat = [face(300, false), face(400, true), face(500, false), face(700, false)];
        let w = |want| best(&cat, "F", want).unwrap().path;
        assert_eq!(w(400), PathBuf::from("/nonexistent/500false")); // upright first, then 400 → 500
        assert_eq!(w(600), PathBuf::from("/nonexistent/700false")); // heavier first
        assert_eq!(w(200), PathBuf::from("/nonexistent/300false"));
    }

    /// The table reader agrees with fontconfig on this machine's fonts (skipped without fc-list).
    #[test]
    fn scan_matches_fontconfig() {
        let Some(fc) = fc_list() else { return };
        let mut checked = 0;
        for f in fc.iter().filter(|f| !f.variable) {
            let Ok(faces) = sfnt::faces(&f.path) else { continue };
            let Some((_, i)) = faces.iter().find(|(k, _)| *k == f.index) else { continue };
            // fontconfig may list only the legacy name with its style ("Aptos Narrow Bold Italic").
            let same = |n: &String| f.families.iter().any(|x| x.eq_ignore_ascii_case(n) || strip_style(x).is_some_and(|b| b.eq_ignore_ascii_case(n)));
            assert!(i.families.iter().any(same), "{:?} vs {:?}", i.families, f.families);
            checked += 1;
        }
        assert!(checked > 0);
    }
}
