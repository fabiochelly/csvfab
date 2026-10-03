"""Tableau croisé dynamique (ui/js/43-pivot.js), éprouvé dans la vraie page.

Sur un petit fichier aux résultats connus : somme, nombre, moyenne, valeurs distinctes,
totaux de ligne, de colonne et général, valeur vide classée en dernier, valeur non
numérique écartée et comptée, dates regroupées par mois et par trimestre, tri par total,
une case cliquée qui n'affiche que ses lignes, le résultat ouvert dans un nouvel onglet.

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

VENTES = ("id;date;ville;statut;montant\r\n"
          "1;15/01/2024;Paris;vip;10,50\r\n"
          "2;20/01/2024;Paris;actif;5\r\n"
          "3;03/02/2024;Lyon;vip;2\r\n"
          "4;04/02/2024;Lyon;vip;abc\r\n"
          "5;05/04/2024;;actif;1\r\n")


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class PivotTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p = cls.b.tmp("ventes.csv")
        with open(cls.p, "w", encoding="utf-8", newline="") as f:
            f.write(VENTES)
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof openPivot === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def pivot(self, r, c, fn, v=4, rd="m", then=""):
        return self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({self.p!r}); activateTab(id);
            openPivot();
            const g = i => document.getElementById(i);
            g('pt-r').value = {r}; g('pt-c').value = {c}; g('pt-fn').value = '{fn}'; g('pt-v').value = {v}; g('pt-rd').value = '{rd}';
            pivotChange();
            for (let i = 0; i < 100 && !(pivot.res && pivot.res.n); i++) await new Promise(r => setTimeout(r, 20));
            {then}
            const out = {{ m: pivotMatrix(x => String(x)), skipped: pivot.res.skipped, html: document.getElementById('pt-table').textContent }};
            closeAllModals();
            return out;
        }})()""", timeout=30)

    def test_sum_by_city_and_status(self):
        r = self.pivot(2, 3, "sum")
        self.assertEqual(r["m"], [["ville", "actif", "vip", "Total"],
                                  ["Lyon", "", "2", "2"],
                                  ["Paris", "5", "10.5", "15.5"],
                                  ["(empty)", "1", "", "1"],
                                  ["Total", "6", "12.5", "18.5"]])
        self.assertEqual(r["skipped"], 1)                          # « abc » n'est pas un nombre
        self.assertIn("10,50", r["html"])                          # deux décimales à l'écran

    def test_count_average_distinct(self):
        self.assertEqual(self.pivot(2, 3, "count")["m"][1], ["Lyon", "", "2", "2"])     # les lignes, nombre ou non
        self.assertEqual(self.pivot(2, -1, "avg")["m"][2], ["Paris", "7.75"])
        self.assertEqual(self.pivot(3, -1, "distinct", v=2)["m"], [["statut", "Distinct values of ville"], ["actif", "1"], ["vip", "2"], ["Total", "2"]])

    def test_dates_and_sort(self):
        self.assertEqual([x[0] for x in self.pivot(1, -1, "count")["m"]], ["date", "2024-01", "2024-02", "2024-04", "Total"])
        self.assertEqual([x[0] for x in self.pivot(1, -1, "count", rd="q")["m"]], ["date", "2024-Q1", "2024-Q2", "Total"])
        r = self.pivot(2, -1, "sum", then="pivotSort('total');")
        self.assertEqual([x[0] for x in r["m"]], ["ville", "Paris", "Lyon", "(empty)", "Total"])

    def test_drill_and_new_tab(self):
        r = self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({self.p!r}); activateTab(id);
            openPivot();
            const g = i => document.getElementById(i);
            g('pt-r').value = 2; g('pt-c').value = 3; g('pt-fn').value = 'sum'; g('pt-v').value = 4;
            pivotChange();
            for (let i = 0; i < 100 && !(pivot.res && pivot.res.n); i++) await new Promise(r => setTimeout(r, 20));
            pivotDrill(pivot.view.rk.indexOf('Paris'), pivot.view.ck.indexOf('vip'));
            const t = T(), shown = t.filteredData.map(r => cellStr(cellOf(r, 0))), label = t.rowMark.label;
            clearRowMark();
            openPivot();
            for (let i = 0; i < 100 && !(pivot.res && pivot.res.n === 5); i++) await new Promise(r => setTimeout(r, 20));
            pivotTab();
            for (let i = 0; i < 100 && !(T() !== t && T().loaded); i++) await new Promise(r => setTimeout(r, 30));
            return {{ shown, label, tab: T().name, headers: T().headers, rows: T().allData.length }};
        }})()""", timeout=30)
        self.assertEqual(r["shown"], ["1"])
        self.assertEqual(r["label"], "ville = Paris · statut = vip")
        self.assertEqual(r["tab"], "ventes — pivot ville × statut.csv")
        self.assertEqual(r["headers"], ["ville", "actif", "vip", "Total"])
        self.assertEqual(r["rows"], 4)
