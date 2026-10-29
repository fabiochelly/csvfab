//! Free monospace fonts to install from the font dialog (bridge/fonts.py, 32-font.js
//! "Free fonts to install"): a fixed catalogue — the dialog only ever names an entry, never
//! an address. The latest release of each project is taken from GitHub, and its zip is not
//! downloaded whole: a zip keeps its directory at the end, read by HTTP ranges (a few KB),
//! then only the font files kept (24 TTF of Cascadia Code in an archive of 147 MB).
//! Installed for this user alone: ~/.local/share/fonts/csvfab/<id> and fc-cache (Linux),
//! ~/Library/Fonts (macOS). HTTP goes through curl (on every system csvfab runs on).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;

pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    /// The family the files install (what the picker then lists).
    pub family: &'static str,
    pub repo: &'static str,
    /// The release asset (a regex on its name), or a fixed address.
    asset: &'static str,
    /// The archive's files kept (a regex on their path).
    files: &'static str,
    pub license: &'static str,
    pub note: &'static str,
}

const STATIC: &str = "(Thin|ExtraLight|Light|Regular|Medium|SemiBold|Semibold|Bold|ExtraBold|Black)?(Italic|It)?";

pub fn catalog() -> &'static [Entry] {
    static C: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        let e = |id, name, family, repo, asset, files: String, license, note| Entry { id, name, family, repo, asset, files: Box::leak(files.into_boxed_str()), license, note };
        let mut v = vec![
            e("cascadia-code", "Cascadia Code", "Cascadia Code", "microsoft/cascadia-code", r"^CascadiaCode-.*\.zip$", r"^ttf/static/CascadiaCode-[A-Za-z]+\.ttf$".into(), "OFL", "Microsoft, with ligatures"),
            e("cascadia-mono", "Cascadia Mono", "Cascadia Mono", "microsoft/cascadia-code", r"^CascadiaCode-.*\.zip$", r"^ttf/static/CascadiaMono-[A-Za-z]+\.ttf$".into(), "OFL", "Cascadia without ligatures"),
        ];
        for (n, id, family, note) in [
            ("Neon", "monaspace-neon", "Monaspace Neon", "GitHub, neo-grotesque"),
            ("Argon", "monaspace-argon", "Monaspace Argon", "GitHub, humanist"),
            ("Xenon", "monaspace-xenon", "Monaspace Xenon", "GitHub, slab serif"),
            ("Radon", "monaspace-radon", "Monaspace Radon", "GitHub, handwriting"),
            ("Krypton", "monaspace-krypton", "Monaspace Krypton", "GitHub, mechanical"),
        ] {
            v.push(e(id, family, family, "githubnext/monaspace", r"^monaspace-static-.*\.zip$", format!("^Static Fonts/Monaspace {n}/Monaspace{n}-{STATIC}\\.otf$"), "OFL", note));
        }
        v.extend([
            e("jetbrains-mono", "JetBrains Mono", "JetBrains Mono", "JetBrains/JetBrainsMono", r"\.zip$", r"^fonts/ttf/JetBrainsMono-[A-Za-z]+\.ttf$".into(), "OFL", "JetBrains"),
            e("fira-code", "Fira Code", "Fira Code", "tonsky/FiraCode", r"\.zip$", r"^ttf/FiraCode-(Light|Regular|Medium|SemiBold|Bold)\.ttf$".into(), "OFL", "Mozilla's Fira, with ligatures"),
            e("geist-mono", "Geist Mono", "Geist Mono", "vercel/geist-font", r"\.zip$", r"^geist-font/GeistMono/ttf/GeistMono-[A-Za-z]+\.ttf$".into(), "OFL", "Vercel"),
            e("commit-mono", "Commit Mono", "CommitMono", "eigilnikolajsen/commit-mono", r"\.zip$", r"/ttfautohint/CommitMono-[0-9]+-[A-Za-z]+\.ttf$".into(), "OFL", "neutral, Swiss-like"),
            e("maple-mono", "Maple Mono", "Maple Mono", "subframe7536/maple-font", r"^MapleMono-TTF\.zip$", r"^MapleMono-[A-Za-z]+\.ttf$".into(), "OFL", "rounded"),
            e("intel-one-mono", "Intel One Mono", "Intel One Mono", "intel/intel-one-mono", r"^ttf\.zip$", r"^ttf/IntelOneMono-[A-Za-z]+\.ttf$".into(), "OFL", "Intel, made for legibility"),
            e("0xproto", "0xProto", "0xProto", "0xType/0xProto", r"\.zip$", r"^fonts/0xProto-[A-Za-z]+\.ttf$".into(), "OFL", "clear 0 / O, l / 1"),
            e("source-code-pro", "Source Code Pro", "Source Code Pro", "adobe-fonts/source-code-pro", r"^TTF-.*\.zip$", r"^TTF/SourceCodePro-[A-Za-z]+\.ttf$".into(), "OFL", "Adobe"),
            e("hack", "Hack", "Hack", "source-foundry/Hack", r"-ttf\.zip$", r"^ttf/Hack-[A-Za-z]+\.ttf$".into(), "MIT", "from Bitstream Vera / DejaVu"),
            // Monaco is Apple's, not to be shared: Meslo LG is the free Menlo, its successor.
            e("meslo-lg", "Meslo LG", "Meslo LG M", "andreberg/Meslo-Font", "https://raw.githubusercontent.com/andreberg/Meslo-Font/master/dist/v1.2.1/Meslo%20LG%20v1.2.1.zip", r"/MesloLGM-[A-Za-z]+\.ttf$".into(), "Apache 2.0", "free Menlo, the closest to Monaco"),
        ]);
        v
    })
}

/// What an install did.
pub struct Installed {
    pub family: &'static str,
    pub version: Option<String>,
    pub files: usize,
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    // curl ships with Linux, macOS and Windows 10+; without it, say what is missing.
    if Command::new("curl").arg("--version").output().is_err() {
        return Err("curl is needed to download fonts and was not found".into());
    }
    let out = Command::new("curl").args(["-sSfL", "--max-time", "120", "-A", "csvfab"]).args(args).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(out.stdout)
}

/// A remote file read by HTTP ranges, in blocks kept once read: what `zip` reads as a file.
struct Remote {
    url: String,
    size: u64,
    pos: u64,
    blocks: HashMap<u64, Vec<u8>>,
}

const BLOCK: u64 = 1 << 20;

impl Remote {
    fn open(url: &str) -> Result<Remote, String> {
        // The address after redirects (GitHub's storage) and the size, from one HEAD.
        let out = curl(&["-I", "-o", if cfg!(windows) { "NUL" } else { "/dev/null" }, "-w", "%{url_effective}\n", url])?;
        let effective = String::from_utf8_lossy(&out).trim().to_string();
        let head = curl(&["-I", &effective])?;
        let size = String::from_utf8_lossy(&head).lines().rev().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<u64>().ok()).flatten()
        }).ok_or("the archive's size is unknown")?;
        Ok(Remote { url: effective, size, pos: 0, blocks: HashMap::new() })
    }

    fn block(&mut self, k: u64) -> std::io::Result<&[u8]> {
        if !self.blocks.contains_key(&k) {
            let (a, b) = (k * BLOCK, ((k + 1) * BLOCK).min(self.size) - 1);
            let data = curl(&["-r", &format!("{a}-{b}"), &self.url]).map_err(std::io::Error::other)?;
            self.blocks.insert(k, data);
        }
        Ok(&self.blocks[&k])
    }
}

impl Read for Remote {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.size || buf.is_empty() { return Ok(0); }
        let (k, off) = (self.pos / BLOCK, (self.pos % BLOCK) as usize);
        let b = self.block(k)?;
        let n = buf.len().min(b.len().saturating_sub(off));
        buf[..n].copy_from_slice(&b[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Remote {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(x) => x,
            SeekFrom::Current(d) => (self.pos as i64 + d).max(0) as u64,
            SeekFrom::End(d) => (self.size as i64 + d).max(0) as u64,
        };
        Ok(self.pos)
    }
}

fn font_dir(id: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if cfg!(windows) {
        // This user's fonts folder (no administrator needed), as the web's server used.
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("AppData/Local"));
        return local.join("Microsoft").join("Windows").join("Fonts");
    }
    if cfg!(target_os = "macos") {
        home.join("Library/Fonts")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("fonts/csvfab").join(id)
    }
}

/// Downloads and installs catalogue entry `id` (off the UI thread: it takes seconds).
pub fn install(id: &str) -> Result<Installed, String> {
    let e = catalog().iter().find(|e| e.id == id).ok_or("unknown font")?;
    let (url, version) = if e.asset.starts_with("https://") {
        (e.asset.to_string(), None)
    } else {
        let rel = curl(&[&format!("https://api.github.com/repos/{}/releases/latest", e.repo)])?;
        let rel: serde_json::Value = serde_json::from_slice(&rel).map_err(|x| x.to_string())?;
        let re = regex::Regex::new(e.asset).map_err(|x| x.to_string())?;
        let asset = rel["assets"].as_array().and_then(|a| a.iter().find(|a| a["name"].as_str().is_some_and(|n| re.is_match(n))))
            .ok_or_else(|| format!("no archive in the latest release of {}", e.repo))?;
        (asset["browser_download_url"].as_str().unwrap_or_default().to_string(), rel["tag_name"].as_str().map(str::to_string))
    };
    let mut zip = zip::ZipArchive::new(Remote::open(&url)?).map_err(|x| x.to_string())?;
    let keep = regex::Regex::new(e.files).map_err(|x| x.to_string())?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).filter(|n| keep.is_match(n)).collect();
    if names.is_empty() { return Err("no font file found in the archive".into()); }
    let dest = font_dir(e.id);
    std::fs::create_dir_all(&dest).map_err(|x| x.to_string())?;
    let safe = regex::Regex::new(r"(?i)^[\w.\-\[\] ]+\.(ttf|otf)$").unwrap();
    let mut files = 0;
    for n in names {
        // Never a path from the archive: its file name alone, checked.
        let base = n.rsplit('/').next().unwrap_or_default();
        if !safe.is_match(base) { continue; }
        let mut f = zip.by_name(&n).map_err(|x| x.to_string())?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(|x| x.to_string())?;
        let path = dest.join(base);
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|x| x.to_string())?;
        // Windows finds a user's font through its entry under HKCU (`reg`: no extra crate).
        if cfg!(windows) {
            let kind = if base.to_ascii_lowercase().ends_with(".otf") { "OpenType" } else { "TrueType" };
            let stem = base.rsplit_once('.').map_or(base, |x| x.0);
            let _ = Command::new("reg").args(["add", r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts", "/v", &format!("{stem} ({kind})"), "/t", "REG_SZ", "/d", &path.to_string_lossy(), "/f"]).output();
        }
        files += 1;
    }
    if cfg!(target_os = "linux") {
        let _ = Command::new("fc-cache").arg("-f").arg(&dest).output();
    }
    Ok(Installed { family: e.family, version, files })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry's rules compile and take the files the projects publish (names as in their
    /// archives), not the others.
    #[test]
    fn catalogue_rules() {
        for e in catalog() {
            assert!(e.asset.starts_with("https://") || regex::Regex::new(e.asset).is_ok(), "{}", e.id);
            assert!(regex::Regex::new(e.files).is_ok(), "{}", e.id);
        }
        let files = |id: &str| regex::Regex::new(catalog().iter().find(|e| e.id == id).unwrap().files).unwrap();
        assert!(files("cascadia-code").is_match("ttf/static/CascadiaCode-Bold.ttf"));
        assert!(!files("cascadia-code").is_match("ttf/static/CascadiaMono-Bold.ttf"));
        assert!(files("monaspace-neon").is_match("Static Fonts/Monaspace Neon/MonaspaceNeon-SemiBoldItalic.otf"));
        assert!(files("jetbrains-mono").is_match("fonts/ttf/JetBrainsMono-Regular.ttf"));
        assert!(files("meslo-lg").is_match("Meslo LG v1.2.1/MesloLGM-Regular.ttf"));
        assert_eq!(catalog().len(), 17);
    }
}
