"""La vidéo de démonstration : csvfab sur un fichier de 5 millions de lignes, filmé pour de vrai.

    cargo build --release -p csvfab --example bench_ui --example film
    python3 benchmark/run.py --json results.json        # d'abord les mesures
    python3 benchmark/video.py --results results.json   # → benchmark/data/csvfab-demo.mp4, sa miniature, le GIF

Trois parties, en 1920 × 1080 à 30 images/s :
  1. un carton de titre ;
  2. l'app elle-même (app/examples/film.rs : la vraie App sans fenêtre, rendue
     par wgpu) : ouverture du fichier généré, défilement, tris par un clic sur un
     titre, filtre tapé dans une colonne. L'horloge est l'horloge murale, chaque
     image garde son heure et le film est monté sur ces heures : une opération de
     40 ms dure 40 ms à l'écran, et les légendes sont les temps de cet essai-là.
     Les pauses de lecture sont entre les opérations, jamais dedans ;
  3. les résultats du banc (results.json) en barres qui avancent toutes à la même
     vitesse, au rythme des temps mesurés ; Modern CSV garde sa mesure datée (il
     ne se pilote pas par script) ; puis un carton de fin.
Les cartons sont des pages HTML dans un Chromium sans fenêtre, dont les animations
sont avancées pas à pas (image n = instant n/30), donc fluides quelle que soit la
vitesse de capture.

--check relit le film monté : l'image où l'écran change vraiment doit tomber
à l'heure mesurée, à une image près.

Il faut ffmpeg et un Chromium. Bibliothèque standard seule par ailleurs.
"""

import argparse
import base64
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from cdp import Chrome, find_chromium  # noqa: E402
from gen import ensure  # noqa: E402

W, H, FPS = 1920, 1080, 30
REPO = "github.com/fabiochelly/csvfab"

# Modern CSV 2.4.4 ne se pilote pas par script : ses temps sont ceux mesurés à la main,
# sur la même machine et le même fichier (version 1 des données), le 2026-10-03.
MODERN = {"date": "2026-10-03", "open": 4.6, "sort_num": 53.0, "sort_text": 24.0}

with open(os.path.join(ROOT, "icons", "csvfab.svg"), encoding="utf-8") as f:
    LOGO = f.read()


class Film:
    """Les images d'un carton, montées à la fin par ffmpeg (quasi sans perte)."""

    def __init__(self, workdir):
        self.dir, self.frames = workdir, []

    def add(self, b64):
        path = os.path.join(self.dir, f"f{len(self.frames):06d}.jpg")
        with open(path, "wb") as f:
            f.write(base64.b64decode(b64))
        self.frames.append(path)

    def render(self, out):
        subprocess.run(["nice", "-n", "19", "ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS),
                        "-i", os.path.join(self.dir, "f%06d.jpg"), "-threads", "4", "-c:v", "libx264rgb",
                        "-preset", "ultrafast", "-qp", "0", "-pix_fmt", "rgb24", out], check=True)
        for p in self.frames:
            os.remove(p)
        self.frames = []


def shot(chrome):
    return chrome.call("Page.captureScreenshot", format="jpeg", quality=96)["data"]


def size_metrics(chrome):
    chrome.call("Emulation.setDeviceMetricsOverride", width=W, height=H, deviceScaleFactor=1, mobile=False)


# --- 1 et 3 : les cartons -------------------------------------------------------
CARD_CSS = """
:root { --bg: #0d1117; --surf: #161b22; --brd: #30363d; --txt: #c9d1d9; --hi: #ffffff; --mut: #8b949e;
        --blue: #3b82f6; --violet: #7c3aed; --gold: #fbbf24; --ok: #3fb950; --bad: #f85149; --other: #6e7681; }
* { box-sizing: border-box; margin: 0; }
html, body { width: 1920px; height: 1080px; overflow: hidden; background: var(--bg); color: var(--txt);
             font-family: Inter, "Segoe UI", system-ui, -apple-system, "Helvetica Neue", Arial, sans-serif; }
.glow { position: absolute; inset: 0; background: radial-gradient(900px 600px at 50% 40%, rgba(59,130,246,.18), transparent 70%),
        radial-gradient(700px 500px at 75% 80%, rgba(124,58,237,.14), transparent 70%); }
.center { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; text-align: center; }
h1 { font-size: 112px; font-weight: 800; color: var(--hi); letter-spacing: -3px; }
h2 { font-size: 54px; font-weight: 700; color: var(--hi); letter-spacing: -1px; }
.sub { font-size: 40px; color: var(--mut); margin-top: 22px; }
.grad { background: linear-gradient(90deg, var(--blue), var(--violet)); -webkit-background-clip: text; background-clip: text; color: transparent; }
.in { opacity: 0; transform: translateY(24px); animation: in .7s cubic-bezier(.2,.8,.2,1) forwards; }
@keyframes in { to { opacity: 1; transform: none; } }
.logo { width: 180px; height: 180px; margin-bottom: 36px; }
"""

TITLE = """<div class="glow"></div><div class="center">
<div class="logo in">{logo}</div>
<h1 class="in" style="animation-delay:.25s">csv<span class="grad">fab</span></h1>
<div class="sub in" style="animation-delay:.6s">{rows} rows · {size} · filmed in real time</div>
</div>"""

END = """<div class="glow"></div><div class="center">
<div class="logo in" style="width:140px;height:140px">{logo}</div>
<h2 class="in" style="animation-delay:.2s;font-size:72px">The CSV editor for files<br><span class="grad">spreadsheets can't open.</span></h2>
<div class="sub in" style="animation-delay:.7s">Free &amp; open source · Linux · macOS · Windows</div>
<div class="sub in" style="animation-delay:1.1s;color:var(--hi);font-size:46px;margin-top:48px;font-weight:600">{repo}</div>
<div class="sub in" style="animation-delay:1.5s;font-size:30px;margin-top:30px;font-family:ui-monospace,Menlo,Consolas,monospace">
brew install --cask fabiochelly/csvfab/csvfab · winget install FabioChelly.csvfab</div>
<div class="sub in" style="animation-delay:1.9s;font-size:26px;margin-top:40px">The csvfab and VisiData bars: <span style="font-family:ui-monospace,Menlo,Consolas,monospace">python3 benchmark/run.py</span> — run it yourself.</div>
</div>"""

# Les barres : la largeur de chacune avance au rythme du temps mesuré (mis à l'échelle
# `scale`), jusqu'à son terme, où le temps s'affiche. Pilotées par __seek(t), pas par CSS.
RACE = """<div class="glow"></div>
<div style="position:absolute;left:140px;right:140px;top:56px">
<h2 class="in">{title}</h2>
<div class="sub in" style="animation-delay:.2s;font-size:32px;margin-top:12px">{subtitle}</div>
<div id="ops" style="margin-top:34px"></div>
<div class="sub in" style="animation-delay:.4s;font-size:24px;position:fixed;left:140px;bottom:56px">{foot}</div>
</div>
<script>
const OPS = {ops}, SCALE = {scale}, MAXW = 1110;
const root = document.getElementById('ops');
const slowest = Math.max(...OPS.flatMap(o => o.bars.map(b => b.s || 0)));
for (const o of OPS) {{
  const g = document.createElement('div'); g.style.marginBottom = '20px';
  g.innerHTML = `<div style="font-size:27px;color:var(--hi);font-weight:600;margin-bottom:4px">${{o.label}}</div>`;
  for (const b of o.bars) {{
    const row = document.createElement('div');
    row.style.cssText = 'display:flex;align-items:center;height:38px;margin:3px 0';
    row.innerHTML = `<div style="width:370px;white-space:nowrap;font-size:24px;color:${{b.us ? 'var(--hi)' : 'var(--mut)'}};font-weight:${{b.us ? 700 : 400}}">${{b.name}}</div>
      <div class="bar" style="height:28px;border-radius:7px;width:0;background:${{b.us ? 'linear-gradient(90deg,var(--blue),var(--violet))' : 'var(--other)'}}"></div>
      <div class="val" style="margin-left:18px;font-size:25px;font-variant-numeric:tabular-nums;color:${{b.us ? 'var(--gold)' : 'var(--txt)'}};font-weight:700"></div>`;
    row._b = b; g.appendChild(row);
  }}
  root.appendChild(g);
}}
function fmt(s) {{ return s < 1 ? Math.round(s * 1000) + ' ms' : s < 10 ? s.toFixed(1) + ' s' : Math.round(s) + ' s'; }}
window.__seek = t => {{
  document.getAnimations().forEach(a => {{ a.pause(); a.currentTime = t * 1000; }});
  const run = Math.max(0, t - 0.8) * SCALE;     // les barres partent après l'apparition des titres
  for (const row of root.querySelectorAll(':scope > div > div')) {{
    const b = row._b; if (!b) continue;
    const bar = row.querySelector('.bar'), val = row.querySelector('.val');
    if (b.limit) {{
      bar.style.width = '0'; val.innerHTML = t > 1.2 ? `<span style="color:var(--mut);font-weight:400">${{b.limit}}</span>` : '';
      continue;
    }}
    const done = run >= b.s, s = Math.min(run, b.s);
    bar.style.width = Math.max(4, s / slowest * MAXW) + 'px';
    val.textContent = run > 0 ? fmt(s) + (done ? '' : '…') : '';
    if (done && b.ratio) val.innerHTML = fmt(b.s) + ` <span style="color:var(--ok);font-size:24px;margin-left:10px">${{b.ratio}}</span>`;
  }}
}};
</script>"""


def card(chrome, body, seconds, film, out, extra_css=""):
    html = f"<!doctype html><meta charset=utf-8><style>{CARD_CSS}{extra_css}</style><body>{body}</body>"
    if "__seek" not in body:
        html += "<script>window.__seek = t => document.getAnimations().forEach(a => { a.pause(); a.currentTime = t * 1000; });</script>"
    chrome.call("Page.navigate", url="data:text/html;base64," + base64.b64encode(html.encode()).decode())
    chrome.wait_for("typeof window.__seek === 'function' && document.readyState === 'complete'")
    for i in range(int(seconds * FPS)):
        chrome.eval(f"__seek({i / FPS})")
        film.add(shot(chrome))
    film.render(out)


# --- 2 : l'app, filmée en temps réel (app/examples/film.rs) ----------------------------
def film_app(binary, path, work, env, theme=None):
    cmd = [binary, path, "--out", work] + (["--theme", theme] if theme else [])
    subprocess.run(cmd, env=env, check=True)
    with open(os.path.join(work, "timeline.json"), encoding="utf-8") as f:
        return json.load(f)


def gray_frames(path, w=320, h=180):
    """Les images d'un film, réduites et en gris (octets par image : w × h)."""
    p = subprocess.Popen(["ffmpeg", "-loglevel", "error", "-i", path, "-vf", f"scale={w}:{h}:flags=area", "-f", "rawvideo",
                          "-pix_fmt", "gray", "-"], stdout=subprocess.PIPE)
    while True:
        b = p.stdout.read(w * h)
        if len(b) < w * h:
            break
        yield b
    p.wait()


def check(mkv, timeline, offset=0.0):
    """Pour chaque opération : l'image où l'écran change vraiment, contre l'heure mesurée.

    Le changement cherché touche plus de 1 % de l'image (le pointeur et son onde, non).
    `offset` : où commence la partie app dans un film monté."""
    frames = list(gray_frames(mkv))
    changed = [0.0] + [sum(1 for x, y in zip(a[::2], b[::2]) if abs(x - y) > 16) / (len(a) / 2) for a, b in zip(frames, frames[1:])]
    out = []
    for o in timeline["ops"]:
        first = int(o["t0"] * FPS)
        want = next((i for i in range(int((o["t0"] + offset) * FPS), len(frames)) if i / FPS >= o["t1"] + offset), None)
        seen = next((i for i in range(first + int(offset * FPS), min(len(frames), first + int(offset * FPS) + 3 * FPS)) if changed[i] > 0.01), None)
        out.append({"op": o["op"], "measured_ms": round(o["ms"], 1), "frames_expected_at": want, "changed_at": seen,
                    "on_screen_ms": None if seen is None else round((seen / FPS - o["t0"] - offset) * 1e3, 1)})
    return out


# --- les résultats ----------------------------------------------------------------------
def pick(results, rows):
    for e in results["results"]:
        if e["rows"] == rows:
            return e
    return None


def race_ops(entry):
    labels = [("open", "Open the file (all rows known)"), ("sort_num", "Sort numbers"), ("sort_text", "Sort text"), ("filter", "Filter a column")]
    ops = []
    for key, label in labels:
        bars = []
        for tool, name in (("csvfab", "csvfab"), ("visidata", "VisiData")):
            r = entry["tools"].get(tool)
            if r and r.get("ok"):
                bars.append({"name": name, "s": statistics.median(r[key]), "us": tool == "csvfab"})
        if key in MODERN:
            bars.append({"name": f"Modern CSV ({MODERN['date']})", "s": MODERN[key]})
        else:
            bars.append({"name": f"Modern CSV ({MODERN['date']})", "limit": "Premium version only: not measured"})
        ops.append({"label": label, "bars": bars})
    return ops


def human_rows(n):
    return f"{n:,}"


def human_size(b):
    return f"{b / 1e9:.1f} GB" if b >= 1e9 else f"{b / 1e6:.0f} MB"


def ms_text(ms):
    return f"{ms:.0f} ms" if ms < 1000 else f"{ms / 1000:.2f} s"


def encode(parts, out):
    """Le film final : H.264 High, yuv420p, 30 i/s, CRF 18, faststart, une piste AAC silencieuse."""
    inputs = [x for p in parts for x in ("-i", p)]
    n = len(parts)
    chain = "".join(f"[{i}:v]" for i in range(n)) + f"concat=n={n}:v=1:a=0,scale=out_color_matrix=bt709:out_range=tv,format=yuv420p[v]"
    subprocess.run(["nice", "-n", "19", "ffmpeg", "-y", "-loglevel", "error", *inputs,
                    "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-filter_complex", chain,
                    "-map", "[v]", "-map", f"{n}:a", "-shortest", "-threads", "8", "-c:v", "libx264", "-profile:v", "high", "-preset", "slow", "-crf", "18",
                    "-pix_fmt", "yuv420p", "-r", str(FPS), "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv",
                    "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart", out], check=True)


def make_gif(mkv, timeline, out):
    """Le GIF du README, 960 × 540, palette faite pour lui : l'ouverture, le défilement, deux tris et le filtre,
    coupés dans les pauses entre les opérations, jamais dedans."""
    ops = {o["op"]: o for o in timeline["ops"]}
    parts = [(ops["open"]["t0"] - 1.2, ops["open"]["t1"] + 5.3), (ops["sort_num_asc"]["t0"] - 1.0, ops["sort_num_asc"]["t1"] + 2.0),
             (ops["sort_text"]["t0"] - 1.0, ops["sort_text"]["t1"] + 2.2), (ops["filter_l"]["t0"] - 0.4, ops["filter_lyon"]["t1"] + 3.0)]
    keep = "+".join(f"between(t,{a:.3f},{b:.3f})" for a, b in parts)
    chain = (f"select='{keep}',setpts=N/{FPS}/TB,fps=12,scale=960:540:flags=lanczos,split[a][b];"
             "[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle")
    subprocess.run(["nice", "-n", "19", "ffmpeg", "-y", "-loglevel", "error", "-i", mkv, "-threads", "4",
                    "-filter_complex", chain, "-loop", "0", out], check=True)


def main():
    ap = argparse.ArgumentParser(description="Record the csvfab demo video.")
    ap.add_argument("--results", required=True, help="the JSON written by benchmark/run.py --json")
    ap.add_argument("--rows", type=int, default=5000000, help="rows of the file filmed (default: %(default)s)")
    ap.add_argument("--data-dir", default=os.path.join(HERE, "data"))
    ap.add_argument("--film", default=None, help="the film binary (default: target/release/examples/film)")
    ap.add_argument("--film-dir", default=None, help="keep the app's film here, and reuse one that is already there")
    ap.add_argument("--theme", default=None, help="the app's theme (default: its default theme)")
    ap.add_argument("--out", default=os.path.join(HERE, "data", "csvfab-demo.mp4"))
    ap.add_argument("--thumbnail", default=os.path.join(HERE, "data", "csvfab-thumbnail.png"))
    ap.add_argument("--gif", default=os.path.join(HERE, "data", "demo.gif"))
    ap.add_argument("--browser", default=None)
    ap.add_argument("--check", action="store_true", help="check the app's film against its measured times, and print the table")
    a = ap.parse_args()
    if not shutil.which("ffmpeg"):
        sys.exit("ffmpeg is needed.")
    with open(a.results, encoding="utf-8") as f:
        results = json.load(f)
    entry = pick(results, a.rows)
    if not entry:
        sys.exit(f"No measure for {a.rows:,} rows in {a.results}.")
    path = os.path.abspath(os.path.join(a.data_dir, entry["file"]))
    ensure(a.rows, path)
    rows, size = human_rows(a.rows), human_size(entry["bytes"])
    binary = a.film or os.path.join(ROOT, "target", "release", "examples", "film" + (".exe" if os.name == "nt" else ""))
    work = tempfile.mkdtemp(prefix="csvfab-video-")
    film_dir = a.film_dir or os.path.join(work, "app")
    os.makedirs(film_dir, exist_ok=True)
    mkv = os.path.join(film_dir, "app.mkv")
    browser = a.browser or find_chromium()
    film = Film(work)
    try:
        if os.path.exists(os.path.join(film_dir, "timeline.json")):
            with open(os.path.join(film_dir, "timeline.json"), encoding="utf-8") as f:
                tl = json.load(f)
        else:
            # Un dossier d'état jetable : les réglages et la mine d'une première ouverture.
            env = dict(os.environ, XDG_STATE_HOME=os.path.join(work, "state"))
            tl = film_app(binary, path, film_dir, env, a.theme)
        opened = next(o for o in tl["ops"] if o["op"] == "open")
        print(f"app: {tl['seconds']:.1f} s, {tl['frames_rendered']} frames, open {opened['ms']:.0f} ms", file=sys.stderr)
        if a.check:
            table = check(mkv, tl)
            for r in table:
                print(f"  {r['op']:<18} measured {r['measured_ms']:>7.1f} ms · screen changes {r['on_screen_ms']} ms after the start "
                      f"(frame {r['changed_at']}, expected {r['frames_expected_at']})", file=sys.stderr)
            with open(os.path.join(film_dir, "timing-check.json"), "w", encoding="utf-8") as f:
                json.dump(table, f, indent=1)

        cards = Chrome("about:blank", binary=browser, args=[f"--window-size={W},{H}", "--hide-scrollbars"])
        size_metrics(cards)
        title, race, end = (os.path.join(work, n + ".mkv") for n in ("title", "race", "end"))
        card(cards, TITLE.format(logo=LOGO, rows=rows, size=size), 3.2, film, title)

        m = results["machine"]
        vd = entry["tools"].get("visidata", {}).get("version", "")
        foot = (f"Medians of {m['repeat']} runs, {m['cpu']}, {m['os'].split()[0]}, 2026-10-30: csvfab {m['csvfab']}, VisiData {vd} (benchmark/run.py). "
                f"Modern CSV 2.4.4, free version, in edit mode: timed by hand on the same machine, {MODERN['date']}, on the previous version of the generated file. "
                "The film's captions were one take, its CPU already awake: these bars start from an idle CPU, which is slower.")
        ops = race_ops(entry)
        slowest = max(b["s"] for o in ops for b in o["bars"] if "s" in b)
        scale = max(1.0, slowest / 9)      # la plus longue barre en 9 s au plus
        race_s = 0.8 + slowest / scale + 3.2
        subtitle = f"{rows} rows · {size} · every bar advances at the same speed, {scale:.1f}× real time" if scale > 1.05 else f"{rows} rows · {size} · real time"
        card(cards, RACE.format(title=f"{rows} rows: how long each tool takes", subtitle=subtitle, foot=foot,
                                ops=json.dumps(ops), scale=scale), race_s, film, race)
        card(cards, END.format(logo=LOGO, repo=REPO), 6.0, film, end)

        # La miniature YouTube, 1280 × 720 : les médianes du banc, pas le meilleur essai.
        def median_ms(key):
            return statistics.median(entry["tools"]["csvfab"][key]) * 1000

        thumb = f"""<div class="glow"></div><div class="center">
<div class="logo" style="width:170px;height:170px;margin-bottom:10px">{LOGO}</div>
<h1 style="font-size:190px;letter-spacing:-6px">5 million rows</h1>
<div class="sub" style="font-size:96px;color:var(--hi);font-weight:800;margin-top:6px"><span class="grad">opened in {ms_text(median_ms('open'))}</span></div>
<div class="sub" style="font-size:52px;margin-top:34px">sorted in {ms_text(median_ms('sort_num'))} · filmed in real time</div></div>"""
        html = f"<!doctype html><meta charset=utf-8><style>{CARD_CSS}</style><body>{thumb}</body>"
        cards.call("Page.navigate", url="data:text/html;base64," + base64.b64encode(html.encode()).decode())
        cards.wait_for("document.readyState === 'complete'")
        png = cards.call("Page.captureScreenshot", format="png")["data"]
        cards.close()
        tmp_png = os.path.join(work, "thumb-full.png")
        with open(tmp_png, "wb") as f:
            f.write(base64.b64decode(png))
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", tmp_png, "-vf", "scale=1280:720:flags=lanczos", a.thumbnail], check=True)

        encode([title, mkv, race, end], a.out)
        make_gif(mkv, tl, a.gif)
        print(f"{a.out} · {a.thumbnail} · {a.gif}", file=sys.stderr)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
