#!/usr/bin/env python3
"""La vidéo de démonstration : csvfab sur un fichier de 5 millions de lignes, filmé pour de vrai.

    python3 benchmark/run.py --json results.json        # d'abord les mesures
    python3 benchmark/video.py --results results.json   # → benchmark/data/csvfab-5m.mp4 et sa miniature

Trois parties, en 1920 × 1080 à 30 images/s :
  1. un carton de titre ;
  2. l'app elle-même (server.py et la page, dans un Chromium sans fenêtre) :
     ouverture du fichier généré, défilement, tri par un clic sur un titre,
     filtre tapé dans une colonne. Capturée en temps réel, image par image,
     avec l'heure de chaque image : une opération qui prend 2 s dure 2 s à
     l'écran, et les temps affichés sont ceux que la page vient de mesurer ;
  3. les résultats du banc (results.json) en barres qui avancent au rythme des
     temps mesurés, la limite de lignes des tableurs, et un carton de fin.
Les cartons sont des pages HTML dont les animations sont avancées pas à pas
(image n = instant n/30), donc fluides quelle que soit la vitesse de capture.

Il faut ffmpeg. Bibliothèque standard seule par ailleurs.
"""

import argparse
import base64
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
sys.path.insert(0, HERE)
from gen import ensure  # noqa: E402
from tests.cdp import Chrome, find_chromium  # noqa: E402
from tests.support import Bridge, read_version  # noqa: E402

W, H, FPS = 1920, 1080, 30
REPO = "github.com/fabiochelly/csvfab"

with open(os.path.join(ROOT, "icons", "csvfab.svg"), encoding="utf-8") as f:
    LOGO = f.read()


class Film:
    """Les images capturées et la durée de chacune, montées à la fin par ffmpeg."""

    def __init__(self, workdir):
        self.dir, self.frames = workdir, []      # [chemin, durée]

    def add(self, png_b64, duration):
        path = os.path.join(self.dir, f"f{len(self.frames):06d}.jpg")
        with open(path, "wb") as f:
            f.write(base64.b64decode(png_b64))
        self.frames.append([path, duration])

    def hold(self, seconds):
        if self.frames:
            self.frames[-1][1] += seconds

    def render(self, out):
        lst = os.path.join(self.dir, "list.txt")
        with open(lst, "w") as f:
            f.write("ffconcat version 1.0\n")
            for p, d in self.frames:
                f.write(f"file '{p}'\nduration {max(d, 0.001):.4f}\n")
            f.write(f"file '{self.frames[-1][0]}'\n")      # le concat ignore la durée de la dernière entrée sinon
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", lst,
                        "-vf", f"fps={FPS},format=yuv420p", "-c:v", "libx264", "-preset", "slow", "-crf", "16",
                        "-movflags", "+faststart", out], check=True)


def shot(chrome):
    return chrome.call("Page.captureScreenshot", format="jpeg", quality=92)["data"]


def size_metrics(chrome, zoom=1):
    """Une image de W × H pixels ; zoom > 1 grossit la page (l'app à 1080p native a un texte trop fin pour une vidéo)."""
    chrome.call("Emulation.setDeviceMetricsOverride", width=round(W / zoom), height=round(H / zoom),
                deviceScaleFactor=zoom, mobile=False)


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
<div class="sub in" style="animation-delay:.6s">{rows} rows · {size}. Watch it open.</div>
</div>"""

END = """<div class="glow"></div><div class="center">
<div class="logo in" style="width:140px;height:140px">{logo}</div>
<h2 class="in" style="animation-delay:.2s;font-size:72px">The CSV editor for files<br><span class="grad">spreadsheets can't open.</span></h2>
<div class="sub in" style="animation-delay:.7s">Free &amp; open source · Linux · macOS · Windows</div>
<div class="sub in" style="animation-delay:1.1s;color:var(--hi);font-size:46px;margin-top:48px;font-weight:600">{repo}</div>
<div class="sub in" style="animation-delay:1.5s;font-size:30px;margin-top:30px;font-family:ui-monospace,Menlo,Consolas,monospace">
brew install --cask fabiochelly/csvfab/csvfab · winget install FabioChelly.csvfab</div>
<div class="sub in" style="animation-delay:1.9s;font-size:26px;margin-top:40px">Every number in this video: <span style="font-family:ui-monospace,Menlo,Consolas,monospace">python3 benchmark/run.py</span> — run it yourself.</div>
</div>"""

# Les barres : la largeur de chacune avance au rythme du temps mesuré (mis à l'échelle
# `scale`), jusqu'à son terme, où le temps s'affiche. Pilotées par __seek(t), pas par CSS.
RACE = """<div class="glow"></div>
<div style="position:absolute;left:140px;right:140px;top:90px">
<h2 class="in">{title}</h2>
<div class="sub in" style="animation-delay:.2s;font-size:32px;margin-top:12px">{subtitle}</div>
<div id="ops" style="margin-top:56px"></div>
<div class="sub in" style="animation-delay:.4s;font-size:24px;position:fixed;left:140px;bottom:56px">{foot}</div>
</div>
<script>
const OPS = {ops}, SCALE = {scale}, MAXW = 1300;
const root = document.getElementById('ops');
const slowest = Math.max(...OPS.flatMap(o => o.bars.map(b => b.s || 0)));
for (const o of OPS) {{
  const g = document.createElement('div'); g.style.marginBottom = '34px';
  g.innerHTML = `<div style="font-size:30px;color:var(--hi);font-weight:600;margin-bottom:12px">${{o.label}}</div>`;
  for (const b of o.bars) {{
    const row = document.createElement('div');
    row.style.cssText = 'display:flex;align-items:center;height:46px;margin:6px 0';
    row.innerHTML = `<div style="width:230px;font-size:26px;color:${{b.us ? 'var(--hi)' : 'var(--mut)'}};font-weight:${{b.us ? 700 : 400}}">${{b.name}}</div>
      <div class="bar" style="height:34px;border-radius:8px;width:0;background:${{b.us ? 'linear-gradient(90deg,var(--blue),var(--violet))' : 'var(--other)'}}"></div>
      <div class="val" style="margin-left:18px;font-size:28px;font-variant-numeric:tabular-nums;color:${{b.us ? 'var(--gold)' : 'var(--txt)'}};font-weight:700"></div>`;
    row._b = b; g.appendChild(row);
  }}
  root.appendChild(g);
}}
function fmt(s) {{ return s < 1 ? Math.round(s * 1000) + ' ms' : s.toFixed(1) + ' s'; }}
window.__seek = t => {{
  document.getAnimations().forEach(a => {{ a.pause(); a.currentTime = t * 1000; }});
  const run = Math.max(0, t - 0.8) * SCALE;     // les barres partent après l'apparition des titres
  for (const row of root.querySelectorAll(':scope > div > div')) {{
    const b = row._b; if (!b) continue;
    const bar = row.querySelector('.bar'), val = row.querySelector('.val');
    if (b.limit) {{
      bar.style.width = '0'; val.innerHTML = t > 1.2 ? `<span style="color:var(--bad)">✕ ${{b.limit}}</span>` : '';
      continue;
    }}
    const done = run >= b.s, s = Math.min(run, b.s);
    bar.style.width = Math.max(4, s / slowest * MAXW) + 'px';
    val.textContent = run > 0 ? fmt(s) + (done ? '' : '…') : '';
    if (done && b.ratio) val.innerHTML = fmt(b.s) + ` <span style="color:var(--ok);font-size:24px;margin-left:10px">${{b.ratio}}</span>`;
  }}
}};
</script>"""


def card(chrome, body, seconds, film, extra_css=""):
    html = f"<!doctype html><meta charset=utf-8><style>{CARD_CSS}{extra_css}</style><body>{body}</body>"
    if "__seek" not in body:
        html += "<script>window.__seek = t => document.getAnimations().forEach(a => { a.pause(); a.currentTime = t * 1000; });</script>"
    chrome.call("Page.navigate", url="data:text/html;base64," + base64.b64encode(html.encode()).decode())
    chrome.wait_for("typeof window.__seek === 'function' && document.readyState === 'complete'")
    for i in range(int(seconds * FPS)):
        chrome.eval(f"__seek({i / FPS})")
        film.add(shot(chrome), 1 / FPS)


# --- 2 : l'app, filmée en temps réel -------------------------------------------------
# Un curseur dessiné (une capture sans fenêtre n'en a pas), des légendes, et la
# suite des gestes. Chaque geste passe par ce que l'interface appelle : le clic
# sur un titre (titleClick), la saisie dans le filtre d'une colonne (editColFilter
# puis input), le défilement du conteneur. Les temps des légendes sont mesurés là.
SCRIPT = r"""
(() => {
const css = document.createElement('style');
css.textContent = `
#__cap { position: fixed; left: 50%; bottom: 50px; transform: translateX(-50%); z-index: 99999; pointer-events: none;
  background: rgba(var(--bg-deep-rgb), .92); border: 1px solid var(--brd-2); border-radius: 12px; padding: 13px 26px;
  box-shadow: 0 12px 40px rgba(0,0,0,.55); text-align: center; min-width: 460px; transition: opacity .25s; font-family: var(--ui); }
#__cap b { display: block; font-size: 27px; color: var(--txt-strong); font-weight: 700; letter-spacing: -.5px; }
#__cap i { display: block; font-style: normal; font-size: 16px; color: var(--txt-mut); margin-top: 4px; }
#__cap em { font-style: normal; color: var(--warn); font-variant-numeric: tabular-nums; }
#__cur { position: fixed; z-index: 100000; left: 640px; top: 470px; width: 22px; height: 22px; pointer-events: none;
  transition: left .7s cubic-bezier(.3,.7,.2,1), top .7s cubic-bezier(.3,.7,.2,1); filter: drop-shadow(0 2px 4px rgba(0,0,0,.6)); }
#__cur.down svg { transform: scale(.85); }`;
document.head.appendChild(css);
const cap = document.createElement('div'); cap.id = '__cap'; cap.style.opacity = 0; document.body.appendChild(cap);
const cur = document.createElement('div'); cur.id = '__cur';
cur.innerHTML = '<svg viewBox="0 0 24 24" width="22" height="22"><path d="M4 2l15 11-6.5 1.2L9 21z" fill="white" stroke="black" stroke-width="1.4" stroke-linejoin="round"/></svg>';
document.body.appendChild(cur);
const sleep = ms => new Promise(r => setTimeout(r, ms));
const paint = () => new Promise(r => requestAnimationFrame(() => setTimeout(r, 0)));
const say = (b, i) => { cap.innerHTML = `<b>${b}</b>` + (i ? `<i>${i}</i>` : ''); cap.style.opacity = 1; };
const hush = () => { cap.style.opacity = 0; };
const num = x => x.toLocaleString('en-US');      // les légendes en anglais ; la barre d'état de l'app garde son format
const secs = ms => ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
async function point(el) {
  const r = el.getBoundingClientRect();
  cur.style.left = (r.left + Math.min(r.width / 2, 60)) + 'px'; cur.style.top = (r.top + r.height / 2) + 'px';
  await sleep(800);
}
async function click(el) {
  await point(el);
  cur.classList.add('down'); await sleep(90); cur.classList.remove('down');
  const t0 = performance.now();
  el.dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1 }));
  await paint();
  return performance.now() - t0;
}
const title = name => thead.querySelector(`th[data-col="${T().headers.indexOf(name)}"] .col-name`);
async function reveal(name) {           // la colonne visée amenée à l'écran, en glissant
  const th = thead.querySelector(`th[data-col="${T().headers.indexOf(name)}"]`);
  const to = Math.max(0, Math.min(th.offsetLeft - 260, container.scrollWidth - container.clientWidth)), from = container.scrollLeft;
  for (let i = 1; i <= 20; i++) { container.scrollLeft = from + (to - from) * (1 - Math.pow(1 - i / 20, 3)); await paint(); }
}

window.__film = async (path, rows, size, small) => {
  window.__done = false;
  await sleep(700);
  // A file of a common size first, so the big one's time is not taken for the usual one.
  say(`First, ${small.size}`, `${small.rows} rows — a typical customer export, 15 columns`);
  await sleep(2400);
  let t0 = performance.now();
  await addPathTabs([small.path]);
  const s = tabs.find(x => x.path === small.path);
  await tabRows(s); activateTab(s.id); await paint();
  say(`Opened in <em>${secs(performance.now() - t0)}</em>`, `${num(s.allData.length)} rows`);
  await sleep(2800);
  say(`Now ${rows} rows · ${size}`, `the same export, ${small.ratio} times bigger`);
  await sleep(2600);
  say('Open', 'every row, not a preview');
  await sleep(900);
  t0 = performance.now();
  await addPathTabs([path]);
  const t = tabs.find(x => x.path === path);
  await tabRows(t); activateTab(t.id); await paint();
  const open = performance.now() - t0;
  say(`Opened in <em>${secs(open)}</em>`, `${num(t.allData.length)} rows ready to filter, sort and edit`);
  await sleep(3200);

  say('Scroll anywhere', 'rows are decoded only when they are shown');
  for (let i = 0; i < 70; i++) { container.scrollTop += 140 + i * 30; await paint(); }
  await sleep(300);
  container.scrollTop = container.scrollHeight; await paint();
  say(`Row ${num(t.allData.length)}`, 'the last line of the file, instantly');
  await sleep(2400);
  container.scrollTop = 0; await paint();

  say('Sort: click a column title', `${num(t.allData.length)} rows by total spent`);
  await reveal('total_spent');
  await sleep(600);
  let ms = await click(title('total_spent'));
  say(`Sorted in <em>${secs(ms)}</em>`, 'smallest first — click again for the biggest spenders');
  await sleep(2000);
  ms = await click(title('total_spent'));
  say(`Biggest spenders first · <em>${secs(ms)}</em>`, `${num(t.allData.length)} rows, numbers recognised by themselves`);
  await sleep(2800);
  say('Now by name', 'one click on last_name');
  await reveal('id');
  await sleep(500);
  ms = await click(title('last_name'));
  say(`Sorted by name in <em>${secs(ms)}</em>`, 'text compared like a dictionary, accents included');
  await sleep(2800);

  say('Filter as you type', 'city contains “lyon”');
  const box = thead.querySelector(`.f-box[data-col="${t.headers.indexOf('city')}"]`);
  await reveal('city');
  await point(box);
  box.focus(); await sleep(250);
  const input = thead.querySelector(`input.f-in[data-col="${t.headers.indexOf('city')}"]`);
  for (const ch of 'lyon') { input.value += ch; input.dispatchEvent(new Event('input', { bubbles: true })); await sleep(260); }
  while (t.filteredData.length === t.allData.length) await sleep(20);
  await sleep(500);
  say(`${num(t.filteredData.length)} rows found · <em>${secs(lastFilterMs)}</em>`, `out of ${num(t.allData.length)} — still sorted by name`);
  await sleep(3400);
  cur.style.left = '1000px'; cur.style.top = '600px';
  hush();
  await sleep(800);
  window.__done = true;
  return { open };
};
return true;
})()
"""


def film_app(chrome, film, path, rows, size, small, tail=0.4):
    chrome.eval(SCRIPT)
    chrome.call("Runtime.evaluate", expression=f"__film({json.dumps(path)}, {json.dumps(rows)}, {json.dumps(size)}, {json.dumps(small)}).then(r => window.__res = r)")
    last = None
    while True:
        img = shot(chrome)
        now = time.monotonic()
        if last is not None:
            film.frames[-1][1] = now - last
        film.add(img, 1 / FPS)
        last = now
        if len(film.frames) % 4 == 0 and chrome.eval("window.__done === true"):
            break
    film.hold(tail)
    return chrome.eval("window.__res")


# --- les résultats ----------------------------------------------------------------------
def pick(results, rows):
    for e in results["results"]:
        if e["rows"] == rows:
            return e
    return None


def race_ops(entry):
    import statistics
    labels = [("open", "Open the file"), ("sort_num", "Sort by a number column"), ("filter", "Filter a column")]
    ops = []
    for key, label in labels:
        bars = []
        for tool, name in (("csvfab", "csvfab"), ("visidata", "VisiData")):
            r = entry["tools"].get(tool)
            if r and r.get("ok"):
                bars.append({"name": name, "s": statistics.median(r[key]), "us": tool == "csvfab"})
        if len(bars) == 2 and bars[0]["s"]:
            bars[0]["ratio"] = f"{bars[1]['s'] / bars[0]['s']:.1f}× faster"
        if key == "open":
            bars.append({"name": "Excel", "limit": "stops at 1,048,576 rows"})
            bars.append({"name": "LibreOffice", "limit": "stops at 1,048,576 rows"})
        ops.append({"label": label, "bars": bars})
    return ops


def human_rows(n):
    return f"{n:,}"


def human_size(b):
    return f"{b / 2**30:.1f} GB" if b >= 2**30 else f"{b / 2**20:.0f} MB"


def main():
    ap = argparse.ArgumentParser(description="Record the csvfab demo video.")
    ap.add_argument("--results", required=True, help="the JSON written by benchmark/run.py --json")
    ap.add_argument("--rows", type=int, default=5000000, help="rows of the file filmed (default: %(default)s)")
    ap.add_argument("--small-rows", type=int, default=650000, help="rows of the file opened first, ~100 MB (default: %(default)s)")
    ap.add_argument("--data-dir", default=os.path.join(HERE, "data"))
    ap.add_argument("--out", default=os.path.join(HERE, "data", "csvfab-5m.mp4"))
    ap.add_argument("--thumbnail", default=os.path.join(HERE, "data", "csvfab-thumbnail.png"))
    ap.add_argument("--browser", default=None)
    a = ap.parse_args()
    if not shutil.which("ffmpeg"):
        sys.exit("ffmpeg is needed.")
    with open(a.results, encoding="utf-8") as f:
        results = json.load(f)
    entry = pick(results, a.rows)
    if not entry:
        sys.exit(f"No measure for {a.rows:,} rows in {a.results}.")
    path = os.path.abspath(os.path.join(a.data_dir, entry["file"]))
    small_path = os.path.abspath(os.path.join(a.data_dir, f"customers-{a.small_rows // 1000}k.csv"))
    small_info = ensure(a.small_rows, small_path)
    small = {"path": small_path, "rows": human_rows(a.small_rows), "size": human_size(small_info["bytes"]),
             "ratio": f"{entry['bytes'] / small_info['bytes']:.1f}".rstrip("0").rstrip(".")}
    ensure(a.rows, path)
    rows, size = human_rows(a.rows), human_size(entry["bytes"])
    work = tempfile.mkdtemp(prefix="csvfab-video-")
    film = Film(work)
    browser = a.browser or find_chromium()
    try:
        cards = Chrome("about:blank", binary=browser, args=[f"--window-size={W},{H}", "--hide-scrollbars"])
        size_metrics(cards)
        card(cards, TITLE.format(logo=LOGO, rows=rows, size=size), 3.2, film)

        b = Bridge().start()
        app = Chrome(b.origin + "/", binary=browser, args=[f"--window-size={W * 2 // 3},{H * 2 // 3}"])
        try:
            size_metrics(app, 1.5)
            app.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
            time.sleep(2)
            res = film_app(app, film, path, rows, size, small)
            print(f"filmed: open {res['open'] / 1000:.2f} s, {len(film.frames)} frames", file=sys.stderr)
        finally:
            app.close()
            b.stop()

        m = results["machine"]
        foot = (f"Medians of {m['repeat']} runs on {m['cpu']}, {m['os'].split()[0]} — measured by benchmark/run.py, "
                f"csvfab {m['csvfab']}, VisiData {entry['tools'].get('visidata', {}).get('version', '')} "
                "(on the previous version of the generated files). "
                "Excel and LibreOffice Calc: documented sheet limit.")
        ops = race_ops(entry)
        slowest = max(b["s"] for o in ops for b in o["bars"] if "s" in b)
        scale = max(1.0, slowest / 7)      # la plus longue barre en 7 s au plus
        race_s = 0.8 + slowest / scale + 3.2
        subtitle = f"{rows} rows · {size}" + (f" · bars {scale:.1f}× real time" if scale > 1.05 else " · real time")
        card(cards, RACE.format(title=f"{rows} rows: csvfab against the others", subtitle=subtitle, foot=foot,
                                ops=json.dumps(ops), scale=scale), race_s, film)
        card(cards, END.format(logo=LOGO, repo=REPO), 6.0, film)

        # La miniature YouTube, 1280 × 720.
        thumb = f"""<div class="glow"></div><div class="center" style="transform:scale(1)">
<div class="logo" style="width:200px;height:200px">{LOGO}</div>
<h1 style="font-size:150px">{rows}</h1>
<div class="sub" style="font-size:64px;color:var(--hi);font-weight:700">rows · {size} · <span class="grad">opened in {res['open'] / 1000:.1f} s</span></div>
<div class="sub" style="font-size:44px;margin-top:30px">Excel stops at 1,048,576.</div></div>"""
        html = f"<!doctype html><meta charset=utf-8><style>{CARD_CSS}</style><body>{thumb}</body>"
        cards.call("Page.navigate", url="data:text/html;base64," + base64.b64encode(html.encode()).decode())
        cards.wait_for("document.readyState === 'complete'")
        png = cards.call("Page.captureScreenshot", format="png")["data"]
        cards.close()
        tmp_png = os.path.join(work, "thumb-full.png")
        with open(tmp_png, "wb") as f:
            f.write(base64.b64decode(png))
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", tmp_png, "-vf", "scale=1280:720", a.thumbnail], check=True)

        film.render(a.out)
        total = sum(d for _, d in film.frames)
        print(f"{a.out}: {total:.0f} s · {a.thumbnail}", file=sys.stderr)
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
