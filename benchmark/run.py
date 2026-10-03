#!/usr/bin/env python3
"""Banc public : csvfab face à d'autres outils, sur des fichiers générés.

    python3 benchmark/run.py                       # 100 k, 1 M et 5 M lignes ; csvfab et VisiData
    python3 benchmark/run.py --rows 1000000 --repeat 5
    python3 benchmark/run.py --tools csvfab --markdown results.md --json results.json

Pour chaque taille, le fichier est généré (benchmark/gen.py, même graine
partout : mêmes octets sur toutes les machines), lu une fois pour qu'il soit
dans le cache du système pour tous les outils, puis chaque outil y fait les
mêmes opérations :

  open          le fichier entier chargé, et pour csvfab la première image de
                la grille peinte (1920 × 1080)
  filter        les lignes dont la colonne city contient « lyon », casse ignorée
  sort numbers  total_spent, décroissant
  sort text     last_name, croissant

Un temps n'est rapporté que si le résultat est juste : nombre de lignes,
lignes filtrées et plus gros montant comparés à ce que le générateur a compté.
La médiane de --repeat essais est rapportée.

csvfab tourne pour de vrai : server.py et la page dans un Chromium sans
fenêtre (profil et dossier d'état jetables), pilotés par le protocole DevTools.
VisiData (s'il est installé, ou via `uv`) tourne dans son propre processus,
un par essai (vd_driver.py).

Bibliothèque standard seule. Le dépôt cloné suffit (tests/cdp.py et
tests/support.py y sont).
"""

import argparse
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
sys.path.insert(0, HERE)
from gen import ensure  # noqa: E402
from tests.cdp import Chrome, find_chromium  # noqa: E402
from tests.support import Bridge, read_version  # noqa: E402

OPS = [("open", "Open"), ("filter", "Filter (column contains)"),
       ("sort_num", "Sort numbers"), ("sort_text", "Sort text")]

# Mesures dans la page : chaque opération appelle ce que l'interface appelle
# (le clic sur un titre, la saisie dans le filtre d'une colonne) et attend
# l'image suivante, peinte.
PAGE = r"""
window.__bench = {
  paint() { return new Promise(r => requestAnimationFrame(() => setTimeout(r, 0))); },
  col(name) { return T().headers.indexOf(name); },
  async open(path) {
    // À froid à chaque essai : onglets fermés, cache de réouverture vidé (sinon un
    // fichier déjà vu s'ouvre sans être relu, ~2× plus vite).
    for (const t of [...tabs]) await closeTab(t.id);
    await idxClear();
    await new Promise(r => setTimeout(r, 300));
    const t0 = performance.now();
    await addPathTabs([path]);
    const t = tabs.find(x => x.path === path);
    await tabRows(t);
    activateTab(t.id);
    await this.paint();
    return { ms: performance.now() - t0, rows: t.allData.length };
  },
  async filter(col, q) {
    const t = T();
    const t0 = performance.now();
    t.colFilters = { [this.col(col)]: q };
    applyFilters();
    await this.paint();
    const ms = performance.now() - t0, n = t.filteredData.length;
    t.colFilters = {}; applyFilters(); await this.paint();
    return { ms, n };
  },
  async sort(col, dir) {
    const t = T(), c = this.col(col);
    const t0 = performance.now();
    sortBy(c, dir);
    await this.paint();
    const ms = performance.now() - t0, first = cellOf(t.filteredData[0], c);
    undo(); await this.paint();          // l'ordre du fichier, pour l'essai suivant
    return { ms, first };
  },
};
true
"""


# --- machine -----------------------------------------------------------------
def cpu_name():
    try:
        if sys.platform.startswith("linux"):
            with open("/proc/cpuinfo") as f:
                for line in f:
                    if line.startswith("model name"):
                        return line.split(":", 1)[1].strip()
        if sys.platform == "darwin":
            return subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip()
    except OSError:
        pass
    return platform.processor() or platform.machine()


def ram_bytes():
    try:
        return os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None


def browser_version(binary):
    try:
        return subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "?"


def machine(browser):
    ram = ram_bytes()
    return {"cpu": cpu_name(), "cores": os.cpu_count(), "ram_gb": round(ram / 2**30) if ram else None,
            "os": f"{platform.system()} {platform.release()}", "python": platform.python_version(),
            "browser": browser_version(browser), "csvfab": read_version()}


# --- mémoire (Linux : /proc) ----------------------------------------------------
# PSS (proportional set size) de tout l'arbre de processus de l'outil : une page
# partagée entre n processus compte pour 1/n. Additionner les RSS compterait les
# bibliothèques de Chromium une fois par processus (~100 Mo chacun, mesuré).
def descendants(pid):
    kids, todo = [], [pid]
    while todo:
        p = todo.pop()
        c = []
        try:
            for tid in os.listdir(f"/proc/{p}/task"):     # chaque fil a ses enfants : uv et Chromium lancent les leurs depuis d'autres fils
                with open(f"/proc/{p}/task/{tid}/children") as f:
                    c += [int(x) for x in f.read().split()]
        except OSError:
            pass
        kids += c
        todo += c
    return kids


def tree_pss(roots):
    total = 0
    for p in {q for r in roots for q in [r] + descendants(r)}:
        try:
            with open(f"/proc/{p}/smaps_rollup") as f:
                for line in f:
                    if line.startswith("Pss:"):
                        total += int(line.split()[1]) * 1024
                        break
        except OSError:
            pass
    return total


class Sampler:
    """Le pic de PSS des arbres `roots`, relevé toutes les 0,05 s pendant le bloc
    `with` ; None hors Linux. Passe à part des chronos : relire /proc les perturberait."""

    def __init__(self, roots):
        self.roots, self.peak, self.stop = roots, None, threading.Event()
        self.th = threading.Thread(target=self.run, daemon=True)

    def run(self):
        while True:
            self.peak = max(self.peak or 0, tree_pss(self.roots()))
            if self.stop.wait(0.05):
                break

    def __enter__(self):
        if os.path.exists("/proc/self/smaps_rollup"):
            self.th.start()
        return self

    def __exit__(self, *exc):
        self.stop.set()
        if self.th.is_alive():
            self.th.join(5)
            self.peak = max(self.peak or 0, tree_pss(self.roots()))


# --- outils ----------------------------------------------------------------------
def warm(path):
    """Le fichier lu une fois : tous les outils le trouvent ensuite dans le cache du système."""
    with open(path, "rb") as f:
        while f.read(1 << 24):
            pass


def run_csvfab(path, repeat, timeout, browser, memory=False):
    """Les chronos ; avec memory, un seul essai de chaque opération et le pic de mémoire."""
    b = Bridge().start()
    chrome = None
    try:
        chrome = Chrome(b.origin + "/", binary=browser, args=["--window-size=1920,1080"])
        chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
        chrome.eval(PAGE)
        time.sleep(2)       # l'app ouverte depuis un instant : son worker de lecture est prêt, comme pour un utilisateur
        if memory:
            with Sampler(lambda: [b.proc.pid, chrome.proc.pid]) as m:
                chrome.eval(f"__bench.open({path!r})", timeout=timeout)
                chrome.eval("__bench.filter('city', 'lyon')", timeout=timeout)
                chrome.eval("__bench.sort('total_spent', -1)", timeout=timeout)
                chrome.eval("__bench.sort('last_name', 1)", timeout=timeout)
            return m.peak
        out = {k: [] for k, _ in OPS}
        for _ in range(repeat):
            r = chrome.eval(f"__bench.open({path!r})", timeout=timeout)
            out["open"].append(r["ms"] / 1000)
            out["rows"] = r["rows"]
        for _ in range(repeat):
            r = chrome.eval("__bench.filter('city', 'lyon')", timeout=timeout)
            out["filter"].append(r["ms"] / 1000)
            out["filter_n"] = r["n"]
            r = chrome.eval("__bench.sort('total_spent', -1)", timeout=timeout)
            out["sort_num"].append(r["ms"] / 1000)
            out["sort_num_first"] = r["first"]
            r = chrome.eval("__bench.sort('last_name', 1)", timeout=timeout)
            out["sort_text"].append(r["ms"] / 1000)
            out["sort_text_first"] = r["first"]
        out["version"] = read_version()
        return out
    finally:
        if chrome:
            chrome.close()
        b.stop()


def visidata_cmd(explicit):
    """La commande qui lance un Python où VisiData s'importe, ou None."""
    if explicit:
        return [explicit]
    if subprocess.run([sys.executable, "-c", "import visidata"], capture_output=True).returncode == 0:
        return [sys.executable]
    vd = shutil.which("vd") or shutil.which("visidata")
    if vd:
        try:
            with open(vd, "rb") as f:
                first = f.readline().decode(errors="replace").strip()
            if first.startswith("#!") and "python" in first:
                return first[2:].split()
        except OSError:
            pass
    if shutil.which("uv"):
        return ["uv", "run", "--quiet", "--no-project", "--with", "visidata", "python"]
    return None


def run_visidata(path, repeat, timeout, cmd, memory=False):
    def once(pids=None):
        p = subprocess.Popen(cmd + [os.path.join(HERE, "vd_driver.py"), path],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if pids is not None:
            pids.append(p.pid)
        try:
            so, se = p.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            p.kill()
            p.communicate()
            raise
        if p.returncode:
            raise RuntimeError(se.strip().splitlines()[-1] if se.strip() else f"exit {p.returncode}")
        return json.loads(so.strip().splitlines()[-1])

    if memory:
        pids = []
        with Sampler(lambda: pids) as m:
            once(pids)
        return m.peak
    out = {k: [] for k, _ in OPS}
    for _ in range(repeat):
        r = once()
        for k, _ in OPS:
            out[k].append(r[k])
        for k in ("rows", "filter_n", "sort_num_first", "sort_text_first", "version"):
            out[k] = r[k]
    return out


# --- vérification et rapport -------------------------------------------------------
def check(res, expect):
    """Ce qui ne correspond pas à ce que le générateur a compté (liste vide : tout est juste)."""
    bad = []
    if res.get("rows") != expect["rows"]:
        bad.append(f"rows {res.get('rows')} ≠ {expect['rows']}")
    if res.get("filter_n") != expect["city_lyon"]:
        bad.append(f"filter {res.get('filter_n')} ≠ {expect['city_lyon']}")
    try:
        if abs(float(res.get("sort_num_first")) - expect["max_total_spent"]) > 0.005:
            bad.append(f"sort {res.get('sort_num_first')} ≠ {expect['max_total_spent']}")
    except (TypeError, ValueError):
        bad.append(f"sort {res.get('sort_num_first')!r}")
    return bad


def secs(x):
    if x is None:
        return "—"
    return f"{x * 1000:.0f} ms" if x < 1 else f"{x:.1f} s" if x < 10 else f"{x:.0f} s"


def size(n):
    return f"{n / 2**30:.1f} GB" if n >= 2**30 else f"{n / 2**20:.0f} MB"


def rows_label(n):
    return f"{n // 1000000} M" if n % 1000000 == 0 else f"{n // 1000} k" if n % 1000 == 0 else f"{n:,}"


def ratio(other, ours, memory):
    if other >= ours:
        return f"{other / ours:.1f}× {'less memory' if memory else 'faster'}"
    return f"{ours / other:.1f}× {'more memory' if memory else 'slower'}"


TOOL_NAMES = {"csvfab": "csvfab", "visidata": "VisiData"}


def report(results, info):
    lines = [f"Measured on {info['cpu']} ({info['cores']} threads, {info['ram_gb']} GB RAM), {info['os']}, "
             f"{info['browser']}, Python {info['python']} — `python3 benchmark/run.py`, median of {info['repeat']} runs.", ""]
    for entry in results:
        tools = [t for t in entry["tools"] if entry["tools"][t].get("ok")]
        others = [t for t in tools if t != "csvfab"]
        head = f"**{rows_label(entry['rows'])} rows × 15 columns · {size(entry['bytes'])}**"
        cols = ["csvfab" if t == "csvfab" else f"{TOOL_NAMES[t]} {entry['tools'][t].get('version', '')}".strip() for t in tools]
        lines.append(head)
        lines.append("")
        lines.append("| | " + " | ".join(cols) + (" | csvfab is" if "csvfab" in tools and others else "") + " |")
        lines.append("|---|" + "---:|" * len(cols) + ("---:|" if "csvfab" in tools and others else ""))
        rows = OPS + [("peak_mem", "Peak memory")]
        for key, label in rows:
            cells, vals = [], {}
            for t in tools:
                r = entry["tools"][t]
                v = r.get("peak_mem") if key == "peak_mem" else statistics.median(r[key])
                vals[t] = v
                txt = (size(v) if v else "—") if key == "peak_mem" else secs(v)
                cells.append(f"**{txt}**" if t == "csvfab" else txt)
            line = f"| {label} | " + " | ".join(cells)
            if "csvfab" in tools and others:
                o = vals[others[0]]
                line += f" | {ratio(o, vals['csvfab'], key == 'peak_mem')}" if o and vals["csvfab"] else " | "
            lines.append(line + " |")
        failed = [f"{TOOL_NAMES[t]}: {entry['tools'][t].get('error')}" for t in entry["tools"] if not entry["tools"][t].get("ok")]
        if failed:
            lines += ["", "Not reported: " + "; ".join(failed) + "."]
        lines.append("")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description="csvfab benchmark: open, filter and sort large generated CSV files.")
    ap.add_argument("--rows", default="100000,1000000,5000000", help="comma-separated row counts (default: %(default)s)")
    ap.add_argument("--repeat", type=int, default=3, help="runs per measure, the median is reported (default: %(default)s)")
    ap.add_argument("--tools", default="csvfab,visidata", help="comma-separated: csvfab, visidata (default: %(default)s)")
    ap.add_argument("--data-dir", default=os.path.join(HERE, "data"), help="where the generated files go (default: benchmark/data)")
    ap.add_argument("--browser", default=None, help="Chromium-based browser binary (default: found on PATH)")
    ap.add_argument("--visidata-python", default=None, help="a Python where `import visidata` works (default: this one, vd's, or `uv run --with visidata`)")
    ap.add_argument("--timeout", type=int, default=900, help="seconds allowed per tool and file (default: %(default)s)")
    ap.add_argument("--no-memory", dest="memory", action="store_false",
                    help="skip the extra run that samples each tool's peak memory (Linux only)")
    ap.add_argument("--markdown", help="also write the tables to this file")
    ap.add_argument("--json", help="also write every measure to this file")
    a = ap.parse_args()

    sizes = [int(x) for x in a.rows.split(",") if x.strip()]
    tools = [t.strip().lower() for t in a.tools.split(",") if t.strip()]
    browser = a.browser or find_chromium()
    if "csvfab" in tools and not browser:
        sys.exit("No Chromium-based browser found: pass --browser.")
    vd_cmd = visidata_cmd(a.visidata_python) if "visidata" in tools else None
    if "visidata" in tools and not vd_cmd:
        print("VisiData not found (pip install visidata, or install uv): skipped.", file=sys.stderr)
        tools.remove("visidata")
    info = {**machine(browser), "repeat": a.repeat}
    os.makedirs(a.data_dir, exist_ok=True)
    print(f"{info['cpu']} · {info['os']} · {info['browser']} · csvfab {info['csvfab']}", file=sys.stderr)

    results = []
    for n in sizes:
        path = os.path.join(os.path.abspath(a.data_dir), f"customers-{rows_label(n).replace(' ', '').lower()}.csv")
        print(f"\n{n:,} rows: generating {os.path.basename(path)}…", file=sys.stderr, flush=True)
        expect = ensure(n, path)
        warm(path)
        entry = {"rows": n, "bytes": expect["bytes"], "file": os.path.basename(path), "tools": {}}
        for tool in tools:
            print(f"  {TOOL_NAMES.get(tool, tool)}…", end=" ", file=sys.stderr, flush=True)
            try:
                if tool == "csvfab":
                    r = run_csvfab(path, a.repeat, a.timeout, browser)
                elif tool == "visidata":
                    r = run_visidata(path, a.repeat, a.timeout, vd_cmd)
                else:
                    raise RuntimeError("unknown tool")
                if a.memory:
                    r["peak_mem"] = (run_csvfab(path, 1, a.timeout, browser, memory=True) if tool == "csvfab"
                                     else run_visidata(path, 1, a.timeout, vd_cmd, memory=True))
                bad = check(r, expect)
                r["ok"] = not bad
                if bad:
                    r["error"] = "wrong result (" + ", ".join(bad) + ")"
            except subprocess.TimeoutExpired:
                r = {"ok": False, "error": f"over {a.timeout} s"}
            except Exception as e:  # un outil qui plante ne doit pas emporter les autres mesures
                r = {"ok": False, "error": f"{type(e).__name__}: {e}"}
            entry["tools"][tool] = r
            print(" · ".join(f"{k} {secs(statistics.median(r[k]))}" for k, _ in OPS) if r["ok"] else r["error"],
                  file=sys.stderr, flush=True)
        results.append(entry)

    md = report(results, info)
    print("\n" + md)
    if a.markdown:
        with open(a.markdown, "w", encoding="utf-8") as f:
            f.write(md)
    if a.json:
        with open(a.json, "w", encoding="utf-8") as f:
            json.dump({"machine": info, "results": results}, f, indent=1, ensure_ascii=False)


if __name__ == "__main__":
    main()
