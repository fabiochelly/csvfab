#!/usr/bin/env python3
"""Banc A/B de l'ouverture d'un fichier, jusqu'à la première image, par le chemin du lanceur.

Ce que bench_page.py ne mesure pas : le fichier arrive comme avec `csvfab x.csv` (POST
/api/open, puis le sondage long de la page) et le chronomètre court jusqu'à la première
image qui montre ses lignes (un MutationObserver sur la grille, puis requestAnimationFrame
+ setTimeout), peinture comprise. La version de référence (extraite de git) et la copie de
travail tournent chacune sur son server.py ; même verdict que bench.py : plus lente si la
médiane dépasse celle de la référence de plus de --tolerance % ET qu'un test de signe sur
les duels l'exclut du hasard.

    python3 tests/bench_open.py                    # copie de travail contre HEAD, ~12 min
    python3 tests/bench_open.py --quick            # moins de duels, sans le fichier de 147 Mo, ~3 min
    python3 tests/bench_open.py --only fresh       # les mesures dont le nom contient « fresh »
    python3 tests/bench_open.py --all-cpus         # sans épingler : les workers ont tous les cœurs

Les mesures :
  fresh:X   page chargée avec le fichier déjà en file (le lancement à froid, une fois Chromium
            là) : de la navigation à la première image. Un seul Chromium, qui va d'un serveur à
            l'autre : avec deux navigateurs qui se relaient, le second de chaque duel gagnait
            20 à 50 ms quelle que soit sa version.
  warm:X    page ouverte, aucun fichier : de la réponse du sondage à la première image.
  warm2:X   pareil, un autre fichier déjà ouvert dans un onglet.
  cold:B    147 Mo, le cache de réouverture vidé avant.
  (XS 1,2 Mo, S 2,4 Mo, M 24 Mo, B 147 Mo, façon export CRM.) Les mesures warm passent par deux
  Chromium qui échangent leurs versions à mi-parcours, comme bench_page.py.

**Le repos avant chaque mesure (--rest, 1,5 s) n'est pas du confort.** Sur ce portable (profil
d'énergie « balanced », EPP balance_power), une ouverture qui suit d'autres activités prend
~40 ms pour 2,4 Mo, mais ~120 ms après une seconde d'inactivité : chaque étape 2 à 3 fois plus
lente à la fois, la chaîne d'échanges courts entre processus réveillant des cœurs endormis.
Sans repos, les duels étaient bimodaux (le navigateur qui venait de servir gagnait de ~40 ms) ;
avec, la dispersion tombe à ±3 % et un A/A ne signale rien. C'est aussi le cas réel : un fichier
s'ouvre après une pause. Et l'attente se fait en une seule évaluation dans la page : la sonder
toutes les 10 ms pendant l'ouverture la gardait éveillée et faussait tout.
Ce n'est pas un test unitaire (le nom ne commence pas par test_) : unittest ne le lance pas.
"""

import argparse
import json
import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests.bench import Results, extract, fast_cpus, pin, snapshot  # noqa: E402
from tests.bench_page import make_crm  # noqa: E402
from tests.cdp import Chrome, find_chromium  # noqa: E402
from tests.support import Bridge  # noqa: E402

# Avant tout script de la page : l'instant où le sondage répond, celui où la grille reçoit ses
# premières lignes, et l'image qui suit ; __frame() les attend en une évaluation.
PRE = r"""
(() => {
  window.__m = { pend: [], rows: null, frame: null };
  const of = window.fetch;
  window.fetch = function (u) { const p = of.apply(this, arguments); if (String(u).startsWith('/api/pending')) p.then(() => __m.pend.push(performance.now()), () => {}); return p; };
  window.__arm = () => { __m.rows = null; __m.frame = null; __m.pend = []; };
  const watch = () => {
    const tb = document.getElementById('tbody');
    new MutationObserver(() => {
      if (__m.rows != null || !tb.querySelector('.row')) return;
      __m.rows = performance.now();
      requestAnimationFrame(() => setTimeout(() => { __m.frame = performance.now(); }, 0));
    }).observe(tb, { childList: true });
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', watch); else watch();
  window.__frame = () => new Promise(res => { const f = () => __m.frame != null && typeof T === 'function' && T() && T().loaded ? res(__m) : setTimeout(f, 5); f(); });
})();
"""

# Une ouverture « page ouverte » : onglets fermés, éventuellement un autre fichier ouvert, le cache
# de réouverture éventuellement vidé, le repos, puis le chemin mis en file par la page elle-même.
WARM = """(async () => {
  for (const t of tabs.slice()) await closeTab(t.id);
  if (%(cold)s && typeof idxDb === 'function') {
    const db = await idxDb();
    if (db) await new Promise(r => { const tx = db.transaction(['data', 'meta'], 'readwrite'); tx.objectStore('data').clear(); tx.objectStore('meta').clear(); tx.oncomplete = tx.onerror = r; });
  }
  const open = p => srvFetch('/api/open', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ paths: [p] }) });
  if (%(other)s) { __arm(); await open(%(other)s); await __frame(); await new Promise(r => setTimeout(r, 300)); }
  if (typeof gc === 'function') gc();
  await new Promise(r => setTimeout(r, %(rest)d));
  __arm(); await open(%(path)s);
  const m = await __frame();
  return m.frame - m.pend[m.pend.length - 1];
})()"""


class Page:
    """Un Chromium sans fenêtre sur la page d'un serveur. Pas de cache « aller-retour » : une page
    quittée garderait son sondage, qui prendrait le chemin mis en file pour la suivante."""

    def __init__(self, bridge):
        self.c = Chrome("about:blank", args=("--js-flags=--expose-gc", "--window-size=1600,1000",
                                             "--disable-background-timer-throttling", "--disable-renderer-backgrounding",
                                             "--disable-features=BackForwardCache"))
        self.c.call("Page.enable")
        self.c.call("Page.addScriptToEvaluateOnNewDocument", source=PRE)
        self.goto(bridge)

    def goto(self, bridge):
        self.b = bridge
        self.c.call("Page.navigate", url=bridge.origin + "/")
        self.c.wait_for("typeof T === 'function' && document.readyState === 'complete' && window.__m")

    def fresh(self, bridge, path, rest):
        """La page de `bridge` chargée avec `path` déjà en file : ms de la navigation à l'image."""
        self.c.call("Page.navigate", url="about:blank")
        self.c.wait_for("location.href === 'about:blank'")
        time.sleep(max(rest, 0.6))                       # le serveur quitté remarque le sondage mort (0,25 s)
        self.b = bridge
        r = bridge.post("/api/open", json.dumps({"paths": [path]}).encode())
        assert r.status == 200, r.status
        self.c.call("Page.navigate", url=bridge.origin + "/")
        deadline = time.time() + 30
        while True:                                     # rare : quelques évaluations pendant le chargement
            try:
                if self.c.eval("location.href !== 'about:blank' && typeof window.__frame === 'function'"):
                    break
            except Exception:
                pass
            if time.time() > deadline:
                raise TimeoutError("la page ne s'est pas chargée")
            time.sleep(0.1)
        m = self.c.eval("__frame()", timeout=120)
        self.c.wait_for("typeof T === 'function' && document.readyState === 'complete'")
        return m["frame"]

    def warm(self, path, other, cold, rest):
        return self.c.eval(WARM % {"cold": "true" if cold else "false", "other": json.dumps(other),
                                   "rest": int(rest * 1000), "path": json.dumps(path)}, timeout=120)

    def close(self):
        self.c.close()


# (nom, sorte, fichier) — fresh dans un seul Chromium, les autres dans deux.
MEASURES = [
    ("fresh:S", "fresh", "S"), ("fresh:M", "fresh", "M"),
    ("warm:XS", "warm", "XS"), ("warm:S", "warm", "S"), ("warm:M", "warm", "M"), ("warm:B", "warm", "B"),
    ("warm2:S", "warm2", "S"), ("warm2:M", "warm2", "M"),
    ("cold:B", "cold", "B"),
]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--ref", default="HEAD", help="révision de référence (défaut : HEAD)")
    ap.add_argument("--tolerance", type=float, default=3.0, help="écart de médiane en %% au-delà duquel une mesure peut être dite plus lente")
    ap.add_argument("--runs", type=int, help="duels par mesure (défaut : 16, 8 en --quick)")
    ap.add_argument("--quick", action="store_true", help="moins de duels, sans le fichier de 147 Mo")
    ap.add_argument("--only", action="append", help="ne mesurer que les noms contenant ce texte (répétable)")
    ap.add_argument("--rest", type=float, default=1.5, help="secondes de repos avant chaque ouverture mesurée (défaut : 1,5)")
    ap.add_argument("--all-cpus", action="store_true", help="ne pas épingler sur les cœurs rapides (les workers ont alors tous les cœurs)")
    ap.add_argument("--save", help="écrire tous les échantillons dans ce fichier JSON")
    a = ap.parse_args()
    if not find_chromium():
        print("Chromium introuvable", file=sys.stderr)
        return 2
    runs = a.runs or (8 if a.quick else 16)
    todo = [m for m in MEASURES if (not a.only or any(o in m[0] for o in a.only)) and not (a.quick and m[2] == "B")]
    tmp = tempfile.mkdtemp(prefix="csvfab-benchopen-")
    res, servers, pages = Results(), [], []
    try:
        ref = extract(a.ref, os.path.join(tmp, "ref"))
        new = snapshot(os.path.join(tmp, "new"))
        print("fichiers…", file=sys.stderr)
        files = {}
        for key, n, seed in (("XS", 5000, 42), ("S", 10000, 42), ("M", 100000, 42), ("B", 600000, 42), ("other", 9000, 7)):
            if key == "other" and not any(m[1] == "warm2" for m in todo):
                continue
            if key != "other" and not any(m[2] == key for m in todo):
                continue
            files[key] = os.path.join(tmp, f"{key}.csv")
            make_crm(files[key], n, seed=seed)
        if not a.all_cpus:
            pin(fast_cpus())
        servers = [Bridge(ref).start(), Bridge(new).start()]
        fresh = [m for m in todo if m[1] == "fresh"]
        if fresh:
            page = Page(servers[0])
            pages.append(page)
            for name, _, key in fresh:
                print(f"  {name}", file=sys.stderr)
                for k in (0, 1, 0, 1):                  # premiers chargements : caches, pas comptés
                    page.fresh(servers[k], files[key], a.rest)
                for i in range(runs):
                    order = (0, 1) if i % 2 == 0 else (1, 0)
                    t = {k: page.fresh(servers[k], files[key], a.rest) for k in order}
                    res.add(name, t[0] / 1000, t[1] / 1000)
            page.close()
            pages.remove(page)
        warm = [m for m in todo if m[1] != "fresh"]
        if warm:
            pages = [Page(servers[0]), Page(servers[1])]
            for half in (0, 1):
                if half:
                    pages[0].goto(servers[1])
                    pages[1].goto(servers[0])
                pa, pb = (pages[1], pages[0]) if half else (pages[0], pages[1])   # pa : la référence
                print(f"moitié {half + 1}/2", file=sys.stderr)
                for name, kind, key in warm:
                    print(f"  {name}", file=sys.stderr)
                    args = (files[key], files.get("other") if kind == "warm2" else None, kind == "cold", a.rest)
                    pa.warm(*args)
                    pb.warm(*args)
                    for i in range(runs // 2):
                        order = (pa, pb) if i % 2 == 0 else (pb, pa)
                        t = {p: p.warm(*args) for p in order}
                        res.add(name, t[pa] / 1000, t[pb] / 1000)
        rows, worse = res.table(a.tolerance)
        if rows:
            w = max(len(r[0]) for r in rows)
            print(f"\nréférence = {a.ref}, nouvelle = copie de travail ; médianes en ms, jusqu'à la première image\n")
            print(f"{'mesure':<{w}}  {'réf.':>9}  {'nouv.':>9}  {'écart':>8}  {'perdus':>7}  {'n':>4}")
            for name, x, y, d, lost, n, slow in rows:
                print(f"{name:<{w}}  {x:>9.2f}  {y:>9.2f}  {d:>+7.1f}%  {lost * 100:>6.0f}%  {n:>4}" + ("  ← plus lent" if slow else ""))
        if a.save:
            with open(a.save, "w") as f:
                json.dump(res.as_json(), f)
        print("\nplus lent : " + ", ".join(worse) if worse else "\nrien de plus lent.")
        return 1 if worse else 0
    finally:
        for p in pages:
            p.close()
        for s in servers:
            s.stop()
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
