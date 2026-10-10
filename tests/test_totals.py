"""La ligne de totaux (ui/js/54-totals.js).

Collée en bas de la grille, une valeur par colonne calculée sur les lignes affichées :
somme par défaut sous les montants, rien ailleurs ; le filtre la recalcule (les anciennes
valeurs grisées en attendant) ; un clic choisit la fonction ; les dates ont leurs bornes ;
la colonne suit les insertions et suppressions (et leur annulation) ; la dernière ligne
reste visible au-dessus d'elle ; masquable.

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

STATUTS = ["actif", "vip", "prospect", "inactif"]
MONTANTS = ["3", "12,5", "1 250,00", "47,10"]


def ventes():
    lines = ["id;statut;montant;date;note"]
    for i in range(1, 121):
        m = MONTANTS[i % 4] if i != 60 else "n/a"          # une valeur qui n'est pas un nombre
        d = f"2024-{(i % 12) + 1:02d}-{(i % 27) + 1:02d}" if i % 10 else ""
        lines.append(f"{i};{STATUTS[i % 4]};{m};{d};texte {i}")
    return "\n".join(lines) + "\n"


def num(s):
    return float(s.replace(" ", "").replace(",", "."))


def expected(keep=lambda i: True):
    tot, n = 0.0, 0
    for i in range(1, 121):
        if i == 60 or not keep(i):
            continue
        tot += num(MONTANTS[i % 4]); n += 1
    return tot, n


# Attend que les totaux de l'onglet actif soient à jour (le calcul se fait au repos).
WAIT = """(async () => { for (let k = 0; k < 300 && (totMissing(T()) || !totFresh(T())); k++) await new Promise(r => setTimeout(r, 20));
  await new Promise(r => setTimeout(r, 30)); return !totMissing(T()); })()"""
CELLS = """(() => Object.fromEntries([...document.querySelectorAll('#tfoot .tf-c[data-c]')].map(c =>
  [c.dataset.c, [c.querySelector('.tf-l') ? c.querySelector('.tf-l').textContent : '', c.querySelector('.tf-v') ? c.querySelector('.tf-v').textContent : '']])))()"""


def fr(x, d=2):
    s = f"{x:,.{d}f}".replace(",", " ").replace(".", ",")
    return s


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class TotalsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p = cls.b.tmp("ventes.csv")
        with open(cls.p, "w", encoding="utf-8", newline="") as f:
            f.write(ventes())
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1300,900",))
        cls.chrome.wait_for("typeof totDraw === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def open(self):
        self.chrome.eval(f"(async () => {{ await __fx.open({self.p!r}); }})()", timeout=60)
        self.assertTrue(self.chrome.eval(WAIT, timeout=30), "totaux jamais calculés")

    def cells(self):
        # Les espaces du format français (fines insécables) ramenés à des espaces simples.
        return {k: [l, v.replace(" ", " ").replace("\xa0", " ")] for k, (l, v) in self.chrome.eval(CELLS).items()}

    def test_default_sum_under_amounts(self):
        self.open()
        c = self.cells()
        tot, n = expected()
        self.assertEqual(c["2"], ["Sum", fr(tot)])              # deux décimales : la colonne les écrit
        self.assertEqual(c["0"][1], "")                          # l'identifiant n'est pas sommé
        self.assertEqual(c["1"][1], "")
        tip = self.chrome.eval("document.querySelector('#tfoot .tf-c[data-c=\"2\"]').dataset.t || document.querySelector('#tfoot .tf-c[data-c=\"2\"]').title")
        self.assertIn(f"{n} numbers", tip.replace(" ", " "))
        self.assertIn("1 value not a number left out", tip.replace(" ", " "))

    def test_filter_recomputes(self):
        self.open()
        stale = self.chrome.eval("""(() => { const t = T(); t.colFilters[1] = 'vip'; applyFilters();
          return document.getElementById('tfoot').classList.contains('stale'); })()""")
        self.assertTrue(stale)                                   # les anciennes valeurs grisées en attendant
        self.assertTrue(self.chrome.eval(WAIT, timeout=30))
        tot, _ = expected(lambda i: STATUTS[i % 4] == "vip")
        self.assertEqual(self.cells()["2"], ["Sum", fr(tot, 1)])   # les lignes affichées écrivent 12,5 : une décimale
        self.assertFalse(self.chrome.eval("document.getElementById('tfoot').classList.contains('stale')"))
        self.chrome.eval("(() => { delete T().colFilters[1]; applyFilters(); })()")

    def test_pick_functions_from_the_menu(self):
        self.open()
        # Un clic sur la cellule ouvre le menu au-dessus, chaque fonction avec sa valeur.
        menu = self.chrome.eval("""(() => { document.querySelector('#tfoot .tf-c[data-c="2"]').click();
          const m = document.getElementById('tot-menu');
          return { open: m.classList.contains('open'), items: [...m.querySelectorAll('.dd-item')].map(x => x.textContent.trim()),
                   above: m.getBoundingClientRect().bottom <= document.querySelector('#tfoot').getBoundingClientRect().top + 1 }; })()""")
        self.assertTrue(menu["open"]); self.assertTrue(menu["above"])
        tot, n = expected()
        self.assertTrue(any(x.startswith("Average") for x in menu["items"]), menu["items"])
        avg = [x for x in menu["items"] if x.startswith("Average")][0].replace(" ", " ")
        self.assertIn(fr(tot / n), avg)
        self.chrome.eval("""[...document.querySelectorAll('#tot-menu .dd-item')].find(x => x.textContent.startsWith('Average')).click()""")
        self.assertEqual(self.cells()["2"], ["Avg", fr(tot / n)])
        # Valeurs distinctes : un calcul de plus, au repos.
        self.chrome.eval("totPick(1, 'distinct')")
        self.assertTrue(self.chrome.eval(WAIT, timeout=30))
        self.assertEqual(self.cells()["1"], ["Distinct", "4"])
        # Médiane.
        self.chrome.eval("totPick(2, 'median')")
        self.assertTrue(self.chrome.eval(WAIT, timeout=30))
        vals = sorted(num(MONTANTS[i % 4]) for i in range(1, 121) if i != 60)
        med = (vals[len(vals) // 2 - 1] + vals[len(vals) // 2]) / 2 if len(vals) % 2 == 0 else vals[len(vals) // 2]
        self.assertEqual(self.cells()["2"], ["Median", fr(med)])
        # Les bornes d'une colonne de dates, telles qu'écrites, et les cellules vides.
        self.chrome.eval("totPick(3, 'max')")
        self.assertTrue(self.chrome.eval(WAIT, timeout=30))
        dates = [f"2024-{(i % 12) + 1:02d}-{(i % 27) + 1:02d}" for i in range(1, 121) if i % 10]
        self.assertEqual(self.cells()["3"][1], max(dates))        # le nom cède la place : une date remplit sa colonne
        tip = self.chrome.eval("(e => e.dataset.t || e.title)([...document.querySelectorAll('#tfoot .tf-c')].find(x => x.dataset.c === '3'))")
        self.assertTrue(tip.startswith("Last: " + max(dates)), tip)
        self.chrome.eval("totPick(3, 'empty')")
        self.assertEqual(self.cells()["3"], ["Empty", "12"])
        self.chrome.eval("totPick(3, '')")
        self.assertEqual(self.cells()["3"][1], "")

    def test_follows_column_changes(self):
        self.open()
        tot, _ = expected()
        self.chrome.eval("(async () => { const ask = uiConfirm; uiConfirm = async () => true; try { await deleteColumn(0); } finally { uiConfirm = ask; } })()")
        self.assertTrue(self.chrome.eval(WAIT, timeout=30))
        heads = self.chrome.eval("T().headers")
        self.assertEqual(heads[1], "montant")
        self.assertEqual(self.cells()["1"], ["Sum", fr(tot)])    # la somme a suivi sa colonne
        self.chrome.eval("undo()")
        self.assertTrue(self.chrome.eval(WAIT, timeout=30))
        self.assertEqual(self.cells()["2"], ["Sum", fr(tot)])
        self.assertEqual(self.cells()["1"][1], "")

    def test_last_row_visible_above(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const pause = ms => new Promise(r => setTimeout(r, ms)), t = T(), n = t.filteredData.length;
          container.scrollTop = container.scrollHeight; renderOnScroll(); await pause(80);
          const foot = document.getElementById('tfoot').getBoundingClientRect();
          const last = tbody.querySelector(`.row[data-idx="${n - 1}"]`).getBoundingClientRect();
          container.scrollTop = 0; render(); await pause(50);
          setSel(t, n - 1, 1, n - 1, 1); revealCell(n - 1, 1); await pause(80);
          const last2 = tbody.querySelector(`.row[data-idx="${n - 1}"]`).getBoundingClientRect();
          const foot2 = document.getElementById('tfoot').getBoundingClientRect();
          return { lastBottom: last.bottom, footTop: foot.top, last2Bottom: last2.bottom, foot2Top: foot2.top, h: foot.height };
        })()""")
        self.assertEqual(r["h"], 35)
        self.assertLessEqual(r["lastBottom"], r["footTop"] + 0.5)
        self.assertLessEqual(r["last2Bottom"], r["foot2Top"] + 0.5)  # Ctrl+Fin ne la cache pas sous les totaux

    def test_toggle(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const h0 = container.scrollHeight; toggleTotals(); await new Promise(r => setTimeout(r, 50));
          const hidden = getComputedStyle(document.getElementById('tfoot')).display, h1 = container.scrollHeight;
          toggleTotals(); await new Promise(r => setTimeout(r, 50));
          return { hidden, shown: getComputedStyle(document.getElementById('tfoot')).display, d: h0 - h1, again: container.scrollHeight === h0 };
        })()""")
        self.assertEqual(r["hidden"], "none")
        self.assertEqual(r["shown"], "flex")
        self.assertEqual(r["d"], 35)                              # l'étendue la compte
        self.assertTrue(r["again"])


if __name__ == "__main__":
    unittest.main()
