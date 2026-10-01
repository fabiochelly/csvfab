"""Polices a chasse fixe : celles qui sont installees, et un catalogue de polices
libres a installer depuis la page."""

import io
import json
import os
import re
import shutil
import sys
import zipfile

_STYLE_RE = re.compile(r"\s+(Thin|ExtraLight|Light|Regular|Medium|SemiBold|Bold|ExtraBold|Black|Italic|Oblique|Condensed|Wide|SemiWide)(\s+\w+)*$", re.I)


def mono_families():
    """Les familles de polices a chasse fixe installees, pour le choix de police de la page :
    fc-list les connait (Linux, et macOS quand fontconfig y est) ; sans lui, une liste vide
    et la page passe par l'API du navigateur. Les familles nommees d'apres un style
    (« Aptos Mono Bold ») sont ramenees a leur famille de base quand elle existe."""
    exe = shutil.which("fc-list")
    if not exe:
        return []
    import subprocess
    try:
        out = subprocess.run([exe, ":spacing=100", "family"], capture_output=True, text=True, timeout=5).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    names = {line.split(",")[0].strip() for line in out.splitlines() if line.strip()}
    base = {n for n in names if not _STYLE_RE.search(n)}
    return sorted({n if n in base or _STYLE_RE.sub("", n) not in base else _STYLE_RE.sub("", n) for n in names}, key=str.lower)


# ---------------------------------------------------------------------------
# Polices libres a installer depuis la page (licence SIL OFL, sauf mention).
# Un catalogue fixe : la page ne donne qu'un identifiant, jamais une adresse.
# La derniere version de chaque projet est prise sur GitHub (API des releases),
# et l'archive n'est pas telechargee en entier : un zip garde son repertoire a
# la fin, on le lit par requetes Range (quelques Ko), puis seulement les fichiers
# de police retenus — 24 TTF de Cascadia Code dans une archive de 147 Mo.
# Installees pour l'utilisateur seul : ~/.local/share/fonts/csvfab/<id> + fc-cache
# (Linux), ~/Library/Fonts (macOS), %LOCALAPPDATA%\Microsoft\Windows\Fonts + la
# cle HKCU des polices (Windows). macOS et Windows : non essayes sur une vraie machine.
# ---------------------------------------------------------------------------
_STATIC = r"(Thin|ExtraLight|Light|Regular|Medium|SemiBold|Semibold|Bold|ExtraBold|Black)?(Italic|It)?"
FONT_CATALOG = [
    # id, nom affiche, famille, depot, archive (regex sur les fichiers de la derniere release, ou
    # une adresse fixe quand le projet n'en publie pas), fichiers retenus (regex), licence, note
    ("cascadia-code", "Cascadia Code", "Cascadia Code", "microsoft/cascadia-code", r"^CascadiaCode-.*\.zip$", r"^ttf/static/CascadiaCode-[A-Za-z]+\.ttf$", "OFL", "Microsoft, with ligatures"),
    ("cascadia-mono", "Cascadia Mono", "Cascadia Mono", "microsoft/cascadia-code", r"^CascadiaCode-.*\.zip$", r"^ttf/static/CascadiaMono-[A-Za-z]+\.ttf$", "OFL", "Cascadia without ligatures"),
] + [
    (f"monaspace-{n.lower()}", f"Monaspace {n}", f"Monaspace {n}", "githubnext/monaspace", r"^monaspace-static-.*\.zip$",
     rf"^Static Fonts/Monaspace {n}/Monaspace{n}-{_STATIC}\.otf$", "OFL", note) for n, note in (
        ("Neon", "GitHub, neo-grotesque"), ("Argon", "GitHub, humanist"), ("Xenon", "GitHub, slab serif"), ("Radon", "GitHub, handwriting"), ("Krypton", "GitHub, mechanical"))
] + [
    ("jetbrains-mono", "JetBrains Mono", "JetBrains Mono", "JetBrains/JetBrainsMono", r"\.zip$", r"^fonts/ttf/JetBrainsMono-[A-Za-z]+\.ttf$", "OFL", "JetBrains"),
    ("fira-code", "Fira Code", "Fira Code", "tonsky/FiraCode", r"\.zip$", r"^ttf/FiraCode-(Light|Regular|Medium|SemiBold|Bold)\.ttf$", "OFL", "Mozilla's Fira, with ligatures"),
    ("geist-mono", "Geist Mono", "Geist Mono", "vercel/geist-font", r"\.zip$", r"^geist-font/GeistMono/ttf/GeistMono-[A-Za-z]+\.ttf$", "OFL", "Vercel"),
    ("commit-mono", "Commit Mono", "CommitMono", "eigilnikolajsen/commit-mono", r"\.zip$", r"/ttfautohint/CommitMono-[0-9]+-[A-Za-z]+\.ttf$", "OFL", "neutral, Swiss-like"),
    ("maple-mono", "Maple Mono", "Maple Mono", "subframe7536/maple-font", r"^MapleMono-TTF\.zip$", r"^MapleMono-[A-Za-z]+\.ttf$", "OFL", "rounded"),
    ("intel-one-mono", "Intel One Mono", "Intel One Mono", "intel/intel-one-mono", r"^ttf\.zip$", r"^ttf/IntelOneMono-[A-Za-z]+\.ttf$", "OFL", "Intel, made for legibility"),
    ("0xproto", "0xProto", "0xProto", "0xType/0xProto", r"\.zip$", r"^fonts/0xProto-[A-Za-z]+\.ttf$", "OFL", "clear 0 / O, l / 1"),
    ("source-code-pro", "Source Code Pro", "Source Code Pro", "adobe-fonts/source-code-pro", r"^TTF-.*\.zip$", r"^TTF/SourceCodePro-[A-Za-z]+\.ttf$", "OFL", "Adobe"),
    ("hack", "Hack", "Hack", "source-foundry/Hack", r"-ttf\.zip$", r"^ttf/Hack-[A-Za-z]+\.ttf$", "MIT", "from Bitstream Vera / DejaVu"),
    # Monaco est une police d'Apple, non redistribuable : Meslo LG est la version libre de
    # Menlo, qui lui a succede sur Mac. Pas de release sur GitHub : l'archive du depot.
    ("meslo-lg", "Meslo LG", "Meslo LG M", "andreberg/Meslo-Font",
     "https://raw.githubusercontent.com/andreberg/Meslo-Font/master/dist/v1.2.1/Meslo%20LG%20v1.2.1.zip", r"/MesloLGM-[A-Za-z]+\.ttf$", "Apache 2.0", "free Menlo, the closest to Monaco"),
]
_UA = {"User-Agent": "csvfab"}


class _RangeFile(io.RawIOBase):
    """Un fichier distant lu par plages HTTP, que zipfile parcourt comme un fichier local."""

    def __init__(self, url):
        import urllib.request
        self._u = urllib.request
        with urllib.request.urlopen(urllib.request.Request(url, method="HEAD", headers=_UA), timeout=30) as r:
            self.url, self.size = r.url, int(r.headers["Content-Length"])   # l'adresse apres redirection (stockage GitHub)
        self.pos = 0

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = off if whence == 0 else self.pos + off if whence == 1 else self.size + off
        return self.pos

    def read(self, n=-1):
        if n is None or n < 0:
            n = self.size - self.pos
        if n <= 0 or self.pos >= self.size:
            return b""
        end = min(self.size, self.pos + n) - 1
        req = self._u.Request(self.url, headers={**_UA, "Range": f"bytes={self.pos}-{end}"})
        with self._u.urlopen(req, timeout=60) as r:
            data = r.read()
        self.pos += len(data)
        return data

    def readinto(self, b):
        d = self.read(len(b))
        b[:len(d)] = d
        return len(d)


def _font_dir():
    if os.name == "nt":
        return os.path.join(os.environ.get("LOCALAPPDATA") or os.path.expanduser("~\\AppData\\Local"), "Microsoft", "Windows", "Fonts")
    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/Fonts")
    return os.path.join(os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share"), "fonts", "csvfab")


def catalog():
    have = {f.lower() for f in mono_families()}
    return [{"id": i, "name": n, "family": fam, "repo": repo, "license": lic, "note": note, "installed": fam.lower() in have}
            for i, n, fam, repo, _a, _f, lic, note in FONT_CATALOG]


def install(fid):
    """Telecharge et installe une police du catalogue ; renvoie un bilan."""
    import urllib.request
    entry = next((e for e in FONT_CATALOG if e[0] == fid), None)
    if not entry:
        raise ValueError("police inconnue")
    _i, name, family, repo, asset_re, file_re, _lic, _note = entry
    if assetre.startswith("https://"):
        url, version = asset_re, None
    else:
        with urllib.request.urlopen(urllib.request.Request(f"https://api.github.com/repos/{repo}/releases/latest", headers=_UA), timeout=30) as r:
            rel = json.loads(r.read().decode("utf-8"), strict=False)
        asset = next((a for a in rel.get("assets", []) if re.search(asset_re, a["name"])), None)
        if not asset:
            raise ValueError(f"pas d'archive dans la derniere version de {repo}")
        url, version = asset["browser_download_url"], rel.get("tag_name")
    zf = zipfile.ZipFile(_RangeFile(url))
    members = [m for m in zf.namelist() if re.search(file_re, m)]
    if not members:
        raise ValueError("aucun fichier de police reconnu dans l'archive")
    dest = _font_dir() if os.name == "nt" or sys.platform == "darwin" else os.path.join(_font_dir(), fid)
    os.makedirs(dest, exist_ok=True)
    written = []
    for m in members:
        base = os.path.basename(m)
        if not base or not re.fullmatch(r"[\w.\-\[\] ]+\.(ttf|otf)", base, re.I):   # jamais de chemin venu de l'archive
            continue
        path = os.path.join(dest, base)
        tmp = path + ".part"
        with zf.open(m) as src, open(tmp, "wb") as out:
            shutil.copyfileobj(src, out)
        os.replace(tmp, path)
        written.append(path)
    if os.name == "nt":
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Windows NT\CurrentVersion\Fonts", 0, winreg.KEY_SET_VALUE) as k:
            for p in written:
                kind = "OpenType" if p.lower().endswith(".otf") else "TrueType"
                winreg.SetValueEx(k, f"{os.path.splitext(os.path.basename(p))[0]} ({kind})", 0, winreg.REG_SZ, p)
    elif shutil.which("fc-cache"):
        import subprocess
        subprocess.run(["fc-cache", "-f", dest], capture_output=True, timeout=120)
    return {"ok": True, "id": fid, "name": name, "family": family, "version": version, "files": len(written), "dir": dest}
