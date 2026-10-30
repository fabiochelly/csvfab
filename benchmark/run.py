#!/usr/bin/env python3
"""Banc public : csvfab face à d'autres outils, sur des fichiers générés.

    cargo build --release -p csvfab --example bench_ui   # d'abord, une fois
    python3 benchmark/run.py                       # 100 k, 1 M et 5 M lignes ; csvfab et VisiData
    python3 benchmark/run.py --rows 1000000 --repeat 5
    python3 benchmark/run.py --tools csvfab --markdown results.md --json results.json

Pour chaque taille, le fichier est généré (benchmark/gen.py, même graine
partout : mêmes octets sur toutes les machines), lu une fois pour qu'il soit
dans le cache du système pour tous les outils, puis chaque outil y fait les
mêmes opérations :

  open          le fichier entier chargé : toutes les lignes connues et
                utilisables, pas seulement le premier écran (pour csvfab, le temps
                des premières lignes à l'écran est rapporté à part)
  filter        les lignes dont la colonne city contient « lyon », casse ignorée
  sort numbers  total_spent, décroissant
  sort text     last_name, croissant

Un temps n'est rapporté que si le résultat est juste : nombre de lignes,
lignes filtrées et plus gros montant comparés à ce que le générateur a compté.
La médiane de --repeat essais est rapportée, chaque essai dans un processus
neuf : un tri mesuré est toujours un premier tri, jamais un tri relu d'un cache.

csvfab tourne pour de vrai : l'app native dans son banc sans fenêtre
(app/examples/bench_ui.rs : egui_kittest, de vraies images, le temps arrêté
quand le résultat est dessiné et tessellé ; pas de GPU), dossier d'état
jetable. VisiData (s'il est installé, ou via `uv`) tourne dans son propre
processus, un par essai (vd_driver.py).

Bibliothèque standard seule.
"""

import argparse
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from gen import ensure  # noqa: E402

OPS = [("open", "Open"), ("filter", "Filter (column contains)"),
       ("sort_num", "Sort numbers"), ("sort_text", "Sort text")]

# Les opérations de bench_ui correspondantes, dans cet ordre, une fois chacune par processus.
BENCH_OPS = {"open": "open", "filter_col": "filter", "sort_num": "sort_num", "sort_text": "sort_text"}


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


def read_version():
    """La version de csvfab, celle du Cargo.toml de l'espace de travail."""
    try:
        with open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8") as f:
            m = re.search(r'^version\s*=\s*"([^"]+)"', f.read(), re.M)
        return m.group(1) if m else "?"
    except OSError:
        return "?"


def machine():
    ram = ram_bytes()
    return {"cpu": cpu_name(), "cores": os.cpu_count(), "ram_gb": round(ram / 2**30) if ram else None,
            "os": f"{platform.system()} {platform.release()}", "python": platform.python_version(),
            "csvfab": read_version()}


def load():
    """La charge du système (1 min, 5 min), notée avant et après chaque outil."""
    try:
        return [round(x, 2) for x in os.getloadavg()[:2]]
    except (OSError, AttributeError):
        return None


# --- mémoire (Linux : /proc) ----------------------------------------------------
# PSS (proportional set size) de tout l'arbre de processus de l'outil : une page
# partagée entre n processus compte pour 1/n. csvfab projette le fichier en mémoire :
# son PSS compte ces pages (Pss_File), que le système peut reprendre à tout moment.
# Anonymous, à côté, est la mémoire privée du processus (le tas).
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


def tree_mem(roots):
    """(PSS, anonyme, PSS des fichiers projetés) de l'arbre, en octets."""
    pss = anon = pss_file = 0
    for p in {q for r in roots for q in [r] + descendants(r)}:
        try:
            with open(f"/proc/{p}/smaps_rollup") as f:
                for line in f:
                    k = line.split(":")[0]
                    if k == "Pss":
                        pss += int(line.split()[1]) * 1024
                    elif k == "Anonymous":
                        anon += int(line.split()[1]) * 1024
                    elif k == "Pss_File":
                        pss_file += int(line.split()[1]) * 1024
        except OSError:
            pass
    return pss, anon, pss_file


class Sampler:
    """Le pic de PSS (avec le fichier projeté à cet instant) et le pic de mémoire anonyme
    des arbres `roots`, relevés toutes les 0,05 s pendant le bloc `with` ; None hors
    Linux. Passe à part des chronos : relire /proc les perturberait."""

    def __init__(self, roots):
        self.roots, self.peak, self.anon, self.mapped, self.stop = roots, None, None, None, threading.Event()
        self.th = threading.Thread(target=self.run, daemon=True)

    def sample(self):
        pss, anon, mapped = tree_mem(self.roots())
        if pss >= (self.peak or 0):
            self.peak, self.mapped = pss, mapped
        self.anon = max(self.anon or 0, anon)

    def run(self):
        while True:
            self.sample()
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


# --- outils ----------------------------------------------------------------------
def warm(path):
    """Le fichier lu une fois : tous les outils le trouvent ensuite dans le cache du système."""
    with open(path, "rb") as f:
        while f.read(1 << 24):
            pass


def run_csvfab(path, repeat, timeout, bench, state_dir, memory=False):
    """Un processus neuf par essai ; avec memory, un seul, et le pic de mémoire."""
    # Un dossier d'état jetable : les réglages d'un utilisateur neuf, rien d'écrit chez vous.
    work = tempfile.mkdtemp(prefix="csvfab-bench-", dir=state_dir)
    env = dict(os.environ, XDG_STATE_HOME=work)

    def once(pids=None):
        cmd = [bench, path, "--ops", ",".join(BENCH_OPS), "--repeat", "1", "--size", "1920x1080",
               "--out", os.path.join(work, "out"), "--timeout", str(timeout)]
        p = subprocess.Popen(cmd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
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
        res = {}
        for line in so.splitlines():
            if line.startswith("{"):
                r = json.loads(line)
                if "error" in r:
                    raise RuntimeError(f"{r['op']}: {r['error']}")
                res[r["op"]] = r
        return res

    try:
        if memory:
            pids = []
            with Sampler(lambda: pids) as m:
                once(pids)
            return m
        out = {k: [] for k, _ in OPS}
        out["first_rows"] = []
        for _ in range(repeat):
            r = once()
            for op, key in BENCH_OPS.items():
                out[key].append(r[op]["ms"] / 1000)
            if "first_rows_ms" in r["open"]["out"]:      # un bench_ui d'avant cette mesure ne la donne pas
                out["first_rows"].append(r["open"]["out"]["first_rows_ms"] / 1000)
            out["rows"] = r["open"]["out"]["rows"]
            out["filter_n"] = r["filter_col"]["out"]["n"]
            out["sort_num_first"] = r["sort_num"]["out"]["first"]
            out["sort_text_first"] = r["sort_text"]["out"]["first"]
        out["version"] = read_version()
        return out
    finally:
        shutil.rmtree(work, ignore_errors=True)


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
        return m
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
             f"csvfab {info['csvfab']} (release build, `bench_ui`), Python {info['python']} — "
             f"`python3 benchmark/run.py`, median of {info['repeat']} runs, each in a fresh process.", ""]
    for entry in results:
        tools = [t for t in entry["tools"] if entry["tools"][t].get("ok")]
        others = [t for t in tools if t != "csvfab"]
        head = f"**{rows_label(entry['rows'])} rows × 15 columns · {size(entry['bytes'])}**"
        cols = ["csvfab" if t == "csvfab" else f"{TOOL_NAMES[t]} {entry['tools'][t].get('version', '')}".strip() for t in tools]
        lines.append(head)
        lines.append("")
        lines.append("| | " + " | ".join(cols) + (" | csvfab is" if "csvfab" in tools and others else "") + " |")
        lines.append("|---|" + "---:|" * len(cols) + ("---:|" if "csvfab" in tools and others else ""))
        rows = OPS[:1] + [("first_rows", "First rows on screen")] + OPS[1:] + [("peak_mem", "Peak memory (PSS)"), ("peak_anon", "of which private (heap)")]
        for key, label in rows:
            cells, vals = [], {}
            for t in tools:
                r = entry["tools"][t]
                mem = key.startswith("peak")
                v = r.get(key) if mem else statistics.median(r[key]) if r.get(key) else None
                vals[t] = v
                txt = (size(v) if v else "—") if mem else secs(v)
                cells.append(f"**{txt}**" if t == "csvfab" and v else txt)
            line = f"| {label} | " + " | ".join(cells)
            if "csvfab" in tools and others:
                o = vals[others[0]]
                line += f" | {ratio(o, vals['csvfab'], key.startswith('peak'))}" if o and vals["csvfab"] else " | "
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
    ap.add_argument("--bench-ui", default=None, help="the bench_ui binary (default: target/release/examples/bench_ui)")
    ap.add_argument("--state-dir", default=None, help="where csvfab's throwaway state folders go (default: the system's temp folder)")
    ap.add_argument("--visidata-python", default=None, help="a Python where `import visidata` works (default: this one, vd's, or `uv run --with visidata`)")
    ap.add_argument("--timeout", type=int, default=900, help="seconds allowed per tool and file (default: %(default)s)")
    ap.add_argument("--no-memory", dest="memory", action="store_false",
                    help="skip the extra run that samples each tool's peak memory (Linux only)")
    ap.add_argument("--markdown", help="also write the tables to this file")
    ap.add_argument("--json", help="also write every measure to this file")
    a = ap.parse_args()

    sizes = [int(x) for x in a.rows.split(",") if x.strip()]
    tools = [t.strip().lower() for t in a.tools.split(",") if t.strip()]
    bench = a.bench_ui or os.path.join(ROOT, "target", "release", "examples", "bench_ui" + (".exe" if os.name == "nt" else ""))
    if "csvfab" in tools and not os.path.exists(bench):
        sys.exit(f"{bench} not found: run `cargo build --release -p csvfab --example bench_ui` first, or pass --bench-ui.")
    if a.state_dir:
        os.makedirs(a.state_dir, exist_ok=True)
    vd_cmd = visidata_cmd(a.visidata_python) if "visidata" in tools else None
    if "visidata" in tools and not vd_cmd:
        print("VisiData not found (pip install visidata, or install uv): skipped.", file=sys.stderr)
        tools.remove("visidata")
    info = {**machine(), "repeat": a.repeat}
    os.makedirs(a.data_dir, exist_ok=True)
    print(f"{info['cpu']} · {info['os']} · csvfab {info['csvfab']}", file=sys.stderr)

    results = []
    for n in sizes:
        path = os.path.join(os.path.abspath(a.data_dir), f"customers-{rows_label(n).replace(' ', '').lower()}.csv")
        print(f"\n{n:,} rows: generating {os.path.basename(path)}…", file=sys.stderr, flush=True)
        expect = ensure(n, path)
        warm(path)
        entry = {"rows": n, "bytes": expect["bytes"], "file": os.path.basename(path), "tools": {}}
        for tool in tools:
            print(f"  {TOOL_NAMES.get(tool, tool)}…", end=" ", file=sys.stderr, flush=True)
            before = load()
            try:
                if tool == "csvfab":
                    r = run_csvfab(path, a.repeat, a.timeout, bench, a.state_dir)
                elif tool == "visidata":
                    r = run_visidata(path, a.repeat, a.timeout, vd_cmd)
                else:
                    raise RuntimeError("unknown tool")
                if a.memory:
                    m = (run_csvfab(path, 1, a.timeout, bench, a.state_dir, memory=True) if tool == "csvfab"
                         else run_visidata(path, 1, a.timeout, vd_cmd, memory=True))
                    r["peak_mem"], r["peak_anon"], r["peak_mapped"] = m.peak, m.anon, m.mapped
                bad = check(r, expect)
                r["ok"] = not bad
                if bad:
                    r["error"] = "wrong result (" + ", ".join(bad) + ")"
            except subprocess.TimeoutExpired:
                r = {"ok": False, "error": f"over {a.timeout} s"}
            except Exception as e:  # un outil qui plante ne doit pas emporter les autres mesures
                r = {"ok": False, "error": f"{type(e).__name__}: {e}"}
            r["load"] = {"before": before, "after": load()}
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
