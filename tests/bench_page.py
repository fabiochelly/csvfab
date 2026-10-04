#!/usr/bin/env python3
"""Banc A/B de la page : ouverture, filtres, tri, panneau de colonne, doublons, profil, défilement…

Le pendant de bench.py pour le JavaScript, avec la même méthode : la version de
référence (extraite de git) et la copie de travail tournent chacune dans son
Chromium sans fenêtre, sur son propre server.py, et chaque mesure alterne entre
les deux, essai par essai ; elle n'est dite plus lente que si sa médiane dépasse
celle de la référence de plus de --tolerance % ET qu'un test de signe sur les
duels l'exclut du hasard (p < 0,05).

    python3 tests/bench_page.py                 # copie de travail contre HEAD
    python3 tests/bench_page.py --ref v1.17.0   # … contre une étiquette ou un commit
    python3 tests/bench_page.py --quick         # moins d'essais, gros fichier plus petit
    python3 tests/bench_page.py --only filter   # les mesures dont le nom contient « filter »

Les fichiers (un petit, un gros façon export CRM : 20 colonnes, accents, dates,
montants à la française, doublons) sont générés ici, de façon déterministe. Le
temps est pris dans la page (performance.now()), une mise en page forcée comprise
quand l'opération redessine la grille ; la peinture n'y est pas — elle demanderait
le traçage de Chromium. Chaque mesure est d'abord jouée une fois sans être
comptée (caches de la page, JIT), et un ramasse-miettes forcé précède chaque essai.

Les opérations passent par les fonctions internes de la page : une mesure qu'une
des deux versions ne sait pas faire (fonction renommée, absente) est écartée et
signalée, pas comptée. Leur résultat (lignes retenues, groupes…) est comparé entre
les deux versions : un écart est signalé, car on ne compare alors plus la même chose.
Ce n'est pas un test unitaire (le nom ne commence pas par test_) : unittest ne le lance pas.
"""

import argparse
import json
import os
import random
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests.bench import Results, extract, fast_cpus, pin, snapshot  # noqa: E402
from tests.cdp import Chrome, JsError, find_chromium  # noqa: E402
from tests.support import Bridge  # noqa: E402

# --- fichiers ------------------------------------------------------------------
PRENOMS = ["Jean", "Marie", "Pierre", "Élodie", "François", "Hélène", "Luc", "Chloé", "Noël", "Zoé",
           "Gaëlle", "Jérôme", "Anaïs", "Benoît", "Cécile", "Raphaël", "Inès", "Théo", "Léa", "Maël"]
NOMS = ["Martin", "Bernard", "Dubois", "Thomas", "Robert", "Richard", "Petit", "Durand", "Leroy", "Moreau",
        "Simon", "Laurent", "Lefèvre", "Michel", "Garcia", "David", "Bertrand", "Roux", "Vincent", "Fournier"]
VILLES = ["Paris", "Lyon", "Marseille", "Toulouse", "Nice", "Nantes", "Strasbourg", "Montpellier",
          "Bordeaux", "Lille", "Rennes", "Reims", "Saint-Étienne", "Le Havre", "Grenoble", "Dijon"]
STATUTS = ["prospect", "client", "ancien client", "partenaire", "à relancer"]
SOURCES = ["salon", "site web", "recommandation", "appel entrant", "campagne e-mail", "LinkedIn"]
MOTS = ["rappeler", "devis", "envoyé", "réunion", "contrat", "signé", "relance", "budget", "validé",
        "intéressé", "à voir", "après l'été", "urgent", "décideur", "absent", "société"]
HEADER = ["Id", "Prénom", "Nom", "Email", "Téléphone", "Société", "Adresse", "Code postal", "Ville",
          "Pays", "Créé le", "Montant", "Statut", "Source", "Score", "Actif", "Tags", "Modifié le",
          "Note", "Commentaire"]


def make_crm(path, rows, wide=1, seed=42):
    """Un export de CRM : ~215 octets la ligne, 5 % de contacts en double (même
    e-mail, nom écrit autrement), quelques champs entre guillemets. wide = k : les
    20 colonnes k fois côte à côte (le cas d'un fichier qui défile de côté)."""
    rng = random.Random(seed)
    ch = rng.choice
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(";".join(h + (f" {k + 1}" if k else "") for k in range(wide) for h in HEADER) + "\r\n")
        prev = []
        for i in range(rows):
            if prev and rng.random() < 0.05:
                p, n, mail = ch(prev)
                n = n.upper()
            else:
                p, n = ch(PRENOMS), ch(NOMS)
                mail = f"{p.lower()}.{n.lower()}{rng.randrange(1000)}@exemple.fr"
                if len(prev) < 5000:
                    prev.append((p, n, mail))
                else:
                    prev[rng.randrange(5000)] = (p, n, mail)
            note = " ".join(rng.choices(MOTS, k=rng.randrange(2, 7)))
            com = f'"{ch(MOTS)}; {ch(MOTS)}"' if rng.random() < 0.1 else ch(MOTS)
            line = (f"{i + 1};{p};{n};{mail};0{rng.randrange(1, 8)} {rng.randrange(10, 99)} {rng.randrange(10, 99)} "
                    f"{rng.randrange(10, 99)} {rng.randrange(10, 99)};{ch(NOMS)} {ch(['SA', 'SARL', 'SAS', '& fils'])};"
                    f"{rng.randrange(1, 200)} rue {ch(NOMS)};{rng.randrange(1000, 96000):05d};{ch(VILLES)};France;"
                    f"{rng.randrange(1, 29):02d}/{rng.randrange(1, 13):02d}/{rng.randrange(2010, 2026)};"
                    f"{rng.randrange(0, 50000)},{rng.randrange(100):02d};{ch(STATUTS)};{ch(SOURCES)};"
                    f"{rng.randrange(101)};{ch(['oui', 'non'])};{ch(MOTS)},{ch(MOTS)};"
                    f"20{rng.randrange(18, 26)}-{rng.randrange(1, 13):02d}-{rng.randrange(1, 29):02d}T10:{rng.randrange(60):02d}:00Z;"
                    f"{note};{com}")
            f.write(";".join([line] * wide) + "\r\n")


# --- la page -------------------------------------------------------------------
# Injecté dans chaque page. Toute opération rend {ms, out} : out sert à vérifier que
# les deux versions ont fait la même chose. Les colonnes : 2 Nom, 3 Email, 8 Ville,
# 10 Créé le (dates), 11 Montant (nombres), 12 Statut.
HARNESS = r"""
window.__bp = (() => {
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const layout = () => void tbody.offsetHeight;
  const timed = async (fn, out) => { const t0 = performance.now(); const r = await fn(); layout(); return { ms: performance.now() - t0, out: out ? out(r) : r }; };
  const closeAll = () => {
    for (const t of tabs.slice()) { t.allData = []; t.filteredData = []; t.base = null; }
    tabs.length = 0; activeTabId = null;
  };
  const setQuery = (q, o = {}) => {
    document.getElementById('global-search').value = q;
    document.getElementById('use-regex').checked = !!o.regex;
    document.getElementById('use-slug').checked = !!o.slug;
    document.getElementById('use-reverse').checked = false;
    document.getElementById('use-expr').checked = !!o.expr;
  };
  const resetView = () => {
    const t = T(); setQuery(''); t.colFilters = {}; t.valFilters = {}; t.lastFilter = null;
    applyFilters(); container.scrollTop = 0; container.scrollLeft = 0; render();
  };
  // A version that filters in workers (44-par-filter.js) answers later: wait for it.
  const settled = async () => { if (typeof whenFiltered === 'function') await whenFiltered(); };
  // …and forget its answers: typing asks a new query each time, a measure must not repeat one it kept.
  const apply = async () => { if (typeof par !== 'undefined' && par) par.cache.clear(); applyFilters(); await settled(); };
  const filter = (q, o) => timed(async () => { const t = T(); t.lastFilter = null; setQuery(q, o); await apply(); }, () => T().filteredData.length);
  const ops = {
    async boot() {
      const n = performance.getEntriesByType('navigation')[0];
      return { ms: n.loadEventEnd - n.startTime, out: typeof T };
    },
    async open({ path, cold }) {
      closeAll();
      if (cold && typeof idxDb === 'function') {
        const db = await idxDb();
        if (db) await new Promise(r => { const tx = db.transaction(['data', 'meta'], 'readwrite'); tx.objectStore('data').clear(); tx.objectStore('meta').clear(); tx.oncomplete = tx.onerror = r; });
      }
      const r = await timed(async () => {
        await addPathTabs([path]);
        const t = tabs.find(x => x.path === path);
        await tabRows(t);
        if (t.error) throw new Error(String(t.error.message || t.error));
        activateTab(t.id);
      }, () => [T().allData.length, T().headers.length]);
      await sleep(cold ? 1500 : 300);   // le cache de réouverture s'écrit après l'ouverture : pas pendant la mesure suivante
      return r;
    },
    filter: ({ q, ...o }) => filter(q, o),
    async narrow({ q1, q2 }) {
      const t = T(); t.lastFilter = null; setQuery(q1); await apply();
      return timed(async () => { setQuery(q2); await apply(); }, () => T().filteredData.length);
    },
    colFilter: ({ col, q }) => timed(async () => { const t = T(); t.lastFilter = null; setQuery(''); t.colFilters = { [col]: q }; await apply(); }, () => T().filteredData.length),
    async valueFilter({ col, values }) {
      return timed(async () => { const t = T(); t.lastFilter = null; t.valFilters = { [col]: new Set(values) }; await apply(); }, () => T().filteredData.length);
    },
    async sort({ col }) {
      const r = await timed(() => sortBy(col, 1), () => cellStr(cellOf(T().allData[0], col)));
      undo();
      return r;
    },
    async colPanel({ col }) {
      T().cpCache = null;
      const btn = document.querySelectorAll('thead tr:first-child .col-menu')[col];
      const r = await timed(() => openColPanel({ stopPropagation() { }, currentTarget: btn }, col),
                            () => document.querySelectorAll('#col-panel input[type=checkbox]').length);
      closeColPanel();
      return r;
    },
    dedupe: ({ cols, fn }) => timed(() => {
      const t = T(), spec = { keys: cols.length ? [{ fields: cols.map(c => ({ col: t.headers[c], fn })) }] : [] };
      return dedupeKeep(t, spec, false);
    }, r => r.keep.filter(x => !x).length),
    profile: () => timed(() => profileTab(T(), () => { }), r => r && r.cols.map(c => c.distinct).join(',')),
    groupBy: ({ col, sum }) => timed(() => {
      const t = T();
      return groupCompute(t, t.allData, [col], { [sum]: new Set(['sum']) }, columnKinds(t), LK_NORM.loose);
    }, r => (r.groups || r).size),
    async review() {
      const t = T(), rows = [];
      for (let i = 0; i < t.allData.length; i += Math.max(1, Math.floor(t.allData.length / 1000))) rows.push([t.allData[i], 19, 'modifié']);
      setCells(t, rows, 'bench');
      const r = await timed(() => computeChanges(t), r => [r.changed.length, r.cells]);
      undo();
      return r;
    },
    async save({ path }) {
      return timed(() => srvWrite(T(), path, false), j => j && j.size);
    },
    async scroll({ steps, dx, dy }) {
      resetView();
      const r = await timed(() => {
        for (let i = 1; i <= steps; i++) { container.scrollTop = i * dy; container.scrollLeft = i * dx; renderOnScroll(); layout(); }
      }, () => document.querySelectorAll('#tbody .row').length);
      resetView();
      return r;
    },
    async jumps({ n, sort }) {
      resetView();
      if (sort != null) sortBy(sort, 1);   // a sorted view: its rows come from all over the file
      const span = container.scrollHeight - container.clientHeight;
      const r = await timed(() => {
        for (let i = 1; i <= n; i++) { container.scrollTop = Math.floor(span * ((i * 0.618034) % 1)); render(true); layout(); }
      }, () => document.querySelectorAll('#tbody .row').length);
      if (sort != null) undo();
      resetView();
      return r;
    },
  };
  return {
    async run(name, args) {
      if (typeof gc === 'function') gc();
      await sleep(30);
      const r = await ops[name](args || {});
      if (name !== 'open' && name !== 'boot') resetView();
      return r;
    },
  };
})();
true
"""


class Browser:
    """Un Chromium sans fenêtre, que l'on fait passer d'une version à l'autre."""

    def __init__(self, url):
        self.c = Chrome(url, args=("--js-flags=--expose-gc", "--window-size=1600,1000",
                                   "--disable-background-timer-throttling", "--disable-renderer-backgrounding"))
        self.ready()

    def ready(self):
        # __old : marque de l'ancien document, qu'une navigation pas encore engagée laisserait passer pour prêt.
        self.c.wait_for("!window.__old && typeof T === 'function' && document.readyState === 'complete'"
                        " && performance.getEntriesByType('navigation')[0].loadEventEnd > 0")
        self.c.eval(HARNESS)

    def goto(self, url):
        self.c.eval("window.__old = true")
        self.c.call("Page.navigate", url=url)
        self.ready()

    def reload(self):
        self.c.eval("window.__old = true")
        self.c.call("Page.reload", ignoreCache=False)
        self.ready()

    def run(self, op, args=None, timeout=600):
        return self.c.eval(f"__bp.run({json.dumps(op)}, {json.dumps(args or {})})", timeout=timeout)

    def close(self):
        self.c.close()


# --- les mesures ---------------------------------------------------------------
# (nom, opération, arguments, fichiers) — S petit, B gros, W large.
MEASURES = [
    ("open_cold", "open", {"cold": True}, "SBW"),
    ("open", "open", {}, "SBW"),
    ("filter_word", "filter", {"q": "lyon"}, "SBW"),
    ("filter_two_words", "filter", {"q": "devis urgent"}, "SB"),
    ("filter_none_found", "filter", {"q": "introuvable"}, "SB"),
    ("filter_accents", "filter", {"q": "elodie", "slug": True}, "SB"),
    ("filter_regex", "filter", {"q": "^1[0-9]5;", "regex": True}, "SB"),
    ("filter_expr", "filter", {"q": '{Montant}.num() > 40000 && {Ville}.contains("lyon")', "expr": True}, "SB"),
    ("filter_narrow", "narrow", {"q1": "réun", "q2": "réunion"}, "SB"),
    ("col_filter", "colFilter", {"col": 8, "q": "saint"}, "SB"),
    ("value_filter", "valueFilter", {"col": 12, "values": ["prospect", "client"]}, "SB"),
    ("sort_text", "sort", {"col": 2}, "SBW"),
    ("sort_number", "sort", {"col": 11}, "SB"),
    ("sort_date", "sort", {"col": 10}, "SB"),
    ("col_panel", "colPanel", {"col": 8}, "SB"),
    ("col_panel_distinct", "colPanel", {"col": 3}, "SB"),
    ("dedupe_email", "dedupe", {"cols": [3], "fn": "email"}, "SB"),
    ("dedupe_3cols", "dedupe", {"cols": [1, 2, 8], "fn": "loose"}, "SB"),
    ("dedupe_whole_row", "dedupe", {"cols": [], "fn": "loose"}, "SB"),
    ("profile", "profile", {}, "SB"),
    ("group_by", "groupBy", {"col": 8, "sum": 11}, "SB"),
    ("review", "review", {}, "SB"),
    ("save_untouched", "save", {}, "SB"),
    ("scroll_down", "scroll", {"steps": 120, "dx": 0, "dy": 35 * 3}, "SBW"),
    ("scroll_sideways", "scroll", {"steps": 80, "dx": 120, "dy": 0}, "W"),
    ("scroll_jumps", "jumps", {"n": 40}, "SBW"),
    ("scroll_jumps_sorted", "jumps", {"n": 40, "sort": 2}, "SBW"),
]


class Bench:
    def __init__(self, res, only):
        self.res, self.only = res, only
        self.ra = self.rb = None           # le navigateur qui fait tourner chaque version, échangés à mi-parcours
        self.skipped, self.differ = {}, {}

    def wanted(self, name):
        return not self.only or any(o in name for o in self.only)

    def measure(self, name, op, args, runs, warm=True):
        """runs duels, l'ordre alterné ; une version qui échoue écarte la mesure."""
        if name in self.skipped:
            return
        print(f"  {name}", file=sys.stderr)
        try:
            if warm:
                self.ra.run(op, args)
                self.rb.run(op, args)
            for i in range(runs):
                if i % 2:
                    b = self.rb.run(op, args)
                    a = self.ra.run(op, args)
                else:
                    a = self.ra.run(op, args)
                    b = self.rb.run(op, args)
                if a["out"] != b["out"]:
                    self.differ.setdefault(name, (a["out"], b["out"]))
                self.res.add(name, a["ms"] / 1000, b["ms"] / 1000)
        except JsError as e:
            self.res.pairs.pop(name, None)
            self.skipped[name] = str(e).split("\n")[0]

    def boot(self, runs):
        for p in (self.ra, self.rb):      # le premier chargement compile app.js : pas compté
            p.reload()
        for i in range(runs):
            for p in ((self.ra, self.rb) if i % 2 == 0 else (self.rb, self.ra)):
                p.reload()
            a, b = self.ra.run("boot"), self.rb.run("boot")
            self.res.add("boot", a["ms"] / 1000, b["ms"] / 1000)

    def suite(self, files, runs, out_dir):
        """Une moitié des duels : files = [(lettre, étiquette, chemin)]."""
        if self.wanted("boot"):
            print("chargement de la page…", file=sys.stderr)
            self.boot(runs * 2)
        for letter, label, path in files:
            todo = [m for m in MEASURES if letter in m[3] and self.wanted(f"{label}:{m[0]}")]
            if not todo:
                continue
            print(f"fichier {label}…", file=sys.stderr)
            n = runs * 2 if letter == "S" else runs    # le petit fichier, plus bruité, a deux fois plus de duels
            opened = False
            for name, op, args, _ in todo:
                args = dict(args)
                if op == "open":
                    args["path"] = path
                elif not opened:
                    for p in (self.ra, self.rb):
                        p.run("open", {"path": path})
                    opened = True
                if op == "save":
                    args["path"] = os.path.join(out_dir, f"out-{letter}.csv")
                self.measure(f"{label}:{name}", op, args, n, warm=not args.get("cold"))
                if op == "open":
                    opened = True


def report(res, tolerance, ref, bench):
    rows, worse = res.table(tolerance)
    if rows:
        w = max(len(r[0]) for r in rows)
        print(f"\nréférence = {ref}, nouvelle = copie de travail ; médianes en ms\n")
        print(f"{'mesure':<{w}}  {'réf.':>10}  {'nouv.':>10}  {'écart':>8}  {'perdus':>7}  {'n':>4}")
        for name, a, b, d, lost, n, slow in rows:
            print(f"{name:<{w}}  {a:>10.2f}  {b:>10.2f}  {d:>+7.1f}%  {lost * 100:>6.0f}%  {n:>4}"
                  + ("  ← plus lent" if slow else ""))
    for name, why in bench.skipped.items():
        print(f"écartée : {name} — {why}")
    for name, (a, b) in bench.differ.items():
        print(f"résultat différent : {name} — réf. {json.dumps(a, ensure_ascii=False)[:80]}, "
              f"nouv. {json.dumps(b, ensure_ascii=False)[:80]}")
    return worse


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--ref", default="HEAD", help="révision de référence (défaut : HEAD)")
    ap.add_argument("--tolerance", type=float, default=3.0,
                    help="écart de médiane en %% au-delà duquel une mesure peut être dite plus lente (défaut : 3)")
    ap.add_argument("--quick", action="store_true", help="moins d'essais, gros fichier de 100 000 lignes")
    ap.add_argument("--rows", type=int, help="lignes du gros fichier (défaut : 600 000, ~147 Mo)")
    ap.add_argument("--only", action="append", help="ne mesurer que les noms contenant ce texte (répétable)")
    ap.add_argument("--all-cpus", action="store_true",
                    help="ne pas épingler sur les cœurs rapides : les workers ont alors tous les cœurs, comme en usage")
    ap.add_argument("--save", help="écrire tous les échantillons dans ce fichier JSON")
    a = ap.parse_args()
    if not find_chromium():
        print("Chromium introuvable", file=sys.stderr)
        return 2

    rows = a.rows or (100_000 if a.quick else 600_000)
    half = 3 if a.quick else 5            # duels par moitié (le petit fichier en a deux fois plus)
    tmp = tempfile.mkdtemp(prefix="csvfab-benchpage-")
    servers, browsers = [], []
    res = Results()
    bench = Bench(res, a.only)
    try:
        ref = extract(a.ref, os.path.join(tmp, "ref"))
        new = snapshot(os.path.join(tmp, "new"))
        data = os.path.join(tmp, "data")
        os.makedirs(data)
        print("fichiers…", file=sys.stderr)
        files = []
        for letter, label, n, wide in (("S", "small", 10_000, 1), ("B", None, rows, 1), ("W", "wide", 5_000, 5)):
            path = os.path.join(data, f"{letter}.csv")
            if label and not any(bench.wanted(f"{label}:{m[0]}") for m in MEASURES if letter in m[3]):
                continue
            make_crm(path, n, wide)
            files.append((letter, label or f"{os.path.getsize(path) / 1e6:.0f}MB", path))
        # Tout sur les cœurs rapides (Chromium et serveurs héritent du masque) :
        # un processus posé sur un cœur efficace y reste, 10 à 27 % plus lent (bench.py).
        # Mais les filtres en parallèle n'y ont que 4 cœurs sur 16 : --all-cpus mesure leur gain réel.
        if not a.all_cpus:
            pin(fast_cpus())
        servers = [Bridge(ref).start(), Bridge(new).start()]
        browsers = [Browser(servers[0].origin + "/"), Browser(servers[1].origin + "/")]
        # Deux instances de Chromium ne vont pas exactement aussi vite (mesuré : une
        # version comparée à elle-même perdait 7 duels sur 8) : chacune fait tourner
        # chaque version la moitié du temps, et leur écart s'annule.
        for phase in (0, 1):
            print(f"moitié {phase + 1}/2", file=sys.stderr)
            if phase:
                browsers[0].goto(servers[1].origin + "/")
                browsers[1].goto(servers[0].origin + "/")
                bench.ra, bench.rb = browsers[1], browsers[0]
            else:
                bench.ra, bench.rb = browsers
            bench.suite(files, half, data)
    finally:
        for p in browsers:
            p.close()
        for s in servers:
            s.stop()
        shutil.rmtree(tmp, ignore_errors=True)

    worse = report(res, a.tolerance, a.ref, bench)
    if a.save:
        with open(a.save, "w") as f:
            json.dump({"ref": a.ref, "rows": rows, "samples_ms": res.as_json()}, f)
    if worse:
        print(f"\nplus lent : {', '.join(worse)}")
        return 1
    print("\nrien de plus lent.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
