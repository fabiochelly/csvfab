"""Les graphiques de colonnes (ui/js/47-sparklines.js) : ce qu'ils comptent et ce qu'un clic filtre.

Un petit fichier aux répartitions connues — des entiers 0 à 9 cent fois chacun, une catégorie
A / B / C à 60 / 30 / 10 %, une colonne vide à partir de la ligne 801 — est compté exactement
(moins de SPK_ROWS lignes) : une barre par entier, les trois valeurs du ruban, le remplissage.
Un clic sur une barre ou une valeur pose le filtre par valeur qui ne garde qu'elles, un second
clic l'enlève ; sous un filtre, les lignes montrées sont comptées à part. Un tri ne change pas
l'image (même échantillon, par enregistrement) ; une modification, si. L'interrupteur cache la
bande. Un fichier plus grand est échantillonné, et le même après un tri.

Sauté sans Chromium.
"""

import os
import shutil
import tempfile
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

# Attend que les graphiques soient à jour : la base, puis les lignes filtrées s'il y a un filtre.
WAIT = """(async () => {
  const t = T();
  for (let k = 0; k < 1000; k++) {
    if (!spkJob && t.spk && sameStamp(t.spk.stamp, dataStamp(t))
        && (t.filteredData === t.allData ? !t.spk.f : t.spk.f && t.spk.fData === t.filteredData)) return true;
    await new Promise(r => setTimeout(r, 20));
  }
  return false;
})()"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class SparklinesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="csvfab-spk-")
        cls.path = os.path.join(cls.dir, "petit.csv")
        with open(cls.path, "w", encoding="utf-8", newline="") as f:
            f.write("n;cat;quand;vide\n")
            for i in range(1000):
                cat = "A" if i % 10 < 6 else "B" if i % 10 < 9 else "C"
                f.write(f"{i % 10};{cat};2024-{1 + i % 12:02d}-{1 + i % 28:02d};{'' if i >= 800 else 'x'}\n")
        cls.big = os.path.join(cls.dir, "grand.csv")
        with open(cls.big, "w", encoding="utf-8", newline="") as f:
            f.write("montant;nom\n")
            for i in range(60000):
                f.write(f"{(i * 7919) % 100003},{i % 100:02d};nom {i % 37}\n")
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1300,900",))
        cls.chrome.wait_for("typeof spkSoon === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def open(self, path):
        self.chrome.eval(f"__fx.open({path!r})", timeout=60)
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))

    def test_counts(self):
        self.open(self.path)
        r = self.chrome.eval("""(() => {
          const s = T().spk, [n, cat, quand, vide] = s.cols;
          return { sampled: s.sampled, S: s.S, kinds: s.cols.map(c => c.k).join(','),
                   bins: Array.from(n.cnt), first: n.first.map(x => x[1]), segs: cat.segs, other: cat.other,
                   dB: quand.B, dSum: quand.cnt.reduce((a, x) => a + x, 0), vide: [vide.fill, vide.segs],
                   label: document.querySelector('#spk-1 .lb').textContent,
                   css: document.getElementById('spk-style').textContent.includes('#spk-0::before{--m:url(') };
        })()""")
        self.assertFalse(r["sampled"])
        self.assertEqual(r["S"], 1000)
        self.assertEqual(r["kinds"], "n,t,d,t")
        self.assertEqual(r["bins"], [100] * 10)               # un entier par barre
        self.assertEqual(r["first"], [str(i) for i in range(10)])
        self.assertEqual(r["segs"], [["A", 600], ["B", 300], ["C", 100]])
        self.assertEqual(r["other"], 0)
        self.assertEqual((r["dB"], r["dSum"]), (32, 1000))
        self.assertEqual(r["vide"], [800, [["x", 800]]])
        self.assertIn("A", r["label"])
        self.assertIn("60 %", r["label"])
        self.assertTrue(r["css"])

    def test_click_filters_then_clears(self):
        self.open(self.path)
        rows = "T().filteredData.length"
        self.chrome.eval("spkFilter(T(), 0, { bar: 3 }); true")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertEqual(self.chrome.eval(rows), 100)
        self.assertEqual(self.chrome.eval("T().filteredData.every(r => cellOf(r, 0) === '3')"), True)
        r = self.chrome.eval("""(() => { const f = T().spk.f; return { n: Array.from(f[0].cnt), cat: Array.from(f[1].seg),
                                          picked: /#spk-0\{[^}]*--pl:/.test(document.getElementById('spk-style').textContent) }; })()""")
        self.assertEqual(r["n"], [0, 0, 0, 100, 0, 0, 0, 0, 0, 0])
        self.assertEqual(r["cat"], [100, 0, 0])                # les lignes 3, 13, 23… sont toutes « A »
        self.assertTrue(r["picked"])
        # Une valeur d'une autre colonne : les deux filtres se cumulent ; puis on retire celle-ci.
        self.chrome.eval("spkFilter(T(), 1, { seg: 0 }); true")
        self.assertEqual(self.chrome.eval(rows), 100)
        self.chrome.eval("spkFilter(T(), 1, { seg: 0 }); spkFilter(T(), 0, { bar: 3 }); true")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertEqual(self.chrome.eval(rows), 1000)
        self.assertEqual(self.chrome.eval("Object.keys(T().valFilters).length"), 0)
        self.assertIsNone(self.chrome.eval("T().spk.f"))
        # Les autres valeurs du ruban : ni A ni B, ni vide.
        self.chrome.eval("spkFilter(T(), 1, { seg: 1 }); true")
        self.assertEqual(self.chrome.eval(rows), 300)
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertIn("100 %", self.chrome.eval("document.querySelector('#spk-1 .lb').textContent"))
        self.chrome.eval("spkFilter(T(), 1, { seg: 1 }); true")

    def test_gauge_filters_filled_or_empty(self):
        self.open(self.path)
        rows = "T().filteredData.length"
        self.chrome.eval("spkFilter(T(), 3, { fill: 'e' }); true")
        self.assertEqual(self.chrome.eval(rows), 200)          # « vide » est vide à partir de la ligne 801
        self.assertTrue(self.chrome.eval("T().filteredData.every(r => !cellOf(r, 3))"))
        self.chrome.eval("spkFilter(T(), 3, { fill: 'f' }); true")   # l'autre partie remplace la première
        self.assertEqual(self.chrome.eval(rows), 800)
        self.chrome.eval("spkFilter(T(), 3, { fill: 'f' }); true")   # la même : tout revient
        self.assertEqual(self.chrome.eval(rows), 1000)
        # Une colonne de texte n'a pas de jauge : la fin de son ruban est sa part vide, qu'on pointe
        # en haut comme en bas ; sous un histogramme, le bas de la case est la jauge, le haut une barre.
        r = self.chrome.eval("""(() => { const at = (i, x, dy) => { const box = document.getElementById('spk-' + i), b = box.getBoundingClientRect();
            const h = spkAt({ target: box, clientX: b.left + b.width * x, clientY: dy < 0 ? b.bottom + dy : b.top + dy }); return [h.gauge, h.part.fill || null, h.part.bar ?? null]; };
          return { texte: [at(3, .9, -3), at(3, .9, 30)], gg: !!document.querySelector('#spk-3 .gg'),
                   nombre: [at(0, .05, -3), at(0, .05, 5)] }; })()""")
        self.assertEqual(r["texte"], [[False, "e", None], [False, "e", None]])
        self.assertFalse(r["gg"])
        self.assertEqual(r["nombre"], [[True, "f", None], [False, None, 0]])

    def test_hover_light_never_takes_the_label(self):
        # La lumière du survol (spkHl) est le dernier enfant de la case survolée : après un filtre
        # puis son retrait, le texte réécrit allait dedans et suivait le pointeur comme une
        # infobulle en italique ; il doit rester dans .lb, la lumière vide.
        self.open(self.path)
        self.chrome.eval("""(() => { const box = document.getElementById('spk-1'), r = box.getBoundingClientRect();
          box.dispatchEvent(new MouseEvent('mousemove', { clientX: r.left + 5, clientY: r.top + 22, bubbles: true }));
          spkFilter(T(), 1, { seg: 1, x0: 0, w: .3 }); return true; })()""")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.chrome.eval("spkFilter(T(), 1, { seg: 1 }); true")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        r = self.chrome.eval("""(() => { const box = document.getElementById('spk-1'), r = box.getBoundingClientRect();
          box.dispatchEvent(new MouseEvent('mousemove', { clientX: r.left + 5, clientY: r.top + 22, bubbles: true }));
          return { lit: spkHl.parentElement === box, text: spkHl.textContent, label: box.querySelector('.lb').textContent }; })()""")
        self.assertTrue(r["lit"])
        self.assertEqual(r["text"], "")
        self.assertIn("60 %", r["label"])

    def test_band_row_kept_across_header_rebuilds(self):
        self.open(self.path)
        same = self.chrome.eval("""(() => { const a = thead.querySelector('.spk-row'); renderHeader(); renderHeader();
          return a === thead.querySelector('.spk-row') && thead.rows.length === 3 && thead.rows[2].classList.contains('filter-row'); })()""")
        self.assertTrue(same)
        # Une colonne ajoutée sur place : plus d'image jusqu'au calcul suivant, jamais celle d'une autre colonne.
        self.chrome.eval("window.uiPrompt = async () => 'nouvelle'; addColumn(-1)", timeout=30)
        self.chrome.wait_for("T().headers[0] === 'nouvelle'")
        self.assertEqual(self.chrome.eval("document.querySelectorAll('.spk-row .spk.k-n').length"), 0)
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertEqual(self.chrome.eval("[T().spk.cols[0].k, T().spk.cols[1].k, document.querySelectorAll('.spk-row th').length]"), ["", "n", 6])
        self.chrome.eval("undo(); true")

    def test_panel_inserts_before_or_after(self):
        self.open(self.path)
        self.chrome.eval("window.uiPrompt = async () => 'X'; true")
        names = """(async (col, which) => {
          openColPanel({ stopPropagation() {}, currentTarget: document.querySelectorAll('thead tr:first-child .col-menu')[col] }, col);
          const items = document.querySelectorAll('#col-panel .cp-add-menu .dd-item');
          items[which].click(); await new Promise(r => setTimeout(r, 50));
          const h = T().headers.slice(); undo(); return h;
        })"""
        self.assertEqual(self.chrome.eval(f"({names})(1, 0)"), ["n", "X", "cat", "quand", "vide"])
        self.assertEqual(self.chrome.eval(f"({names})(1, 1)"), ["n", "cat", "X", "quand", "vide"])
        self.assertEqual(self.chrome.eval("[...document.querySelectorAll('thead .col-actions, thead .c-btn')].length"), 0)

    def test_sort_keeps_edit_changes(self):
        self.open(self.path)
        css = "document.getElementById('spk-style').textContent"
        before = self.chrome.eval(css)
        self.chrome.eval("sortBy(1, -1); true")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertEqual(self.chrome.eval(css), before)
        self.chrome.eval("const t = T(); setCells(t, [[t.allData.find(r => cellOf(r, 0) === '3'), 0, '9']], 'test'); true")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertEqual(self.chrome.eval("[T().spk.cols[0].cnt[3], T().spk.cols[0].cnt[9]]"), [99, 101])
        self.assertNotEqual(self.chrome.eval(css), before)

    def test_switch(self):
        self.open(self.path)
        h = "document.getElementById('mainTable').offsetHeight"
        tall = self.chrome.eval(h)
        self.chrome.eval("toggleSparklines(); true")
        try:
            self.assertEqual(self.chrome.eval("getComputedStyle(document.querySelector('.spk-row')).display"), "none")
            self.assertLess(self.chrome.eval(h), tall)
            self.assertEqual(self.chrome.eval("localStorage.getItem('csvfab-sparklines')"), "0")
        finally:
            self.chrome.eval("toggleSparklines(); true")
        self.assertEqual(self.chrome.eval(h), tall)

    def test_big_file_sampled_by_record(self):
        self.open(self.big)
        r = self.chrome.eval("(() => { const s = T().spk; return { sampled: s.sampled, S: s.S, n: s.n, sum: s.cols[0].cnt.reduce((a, x) => a + x, 0) }; })()")
        self.assertTrue(r["sampled"])
        self.assertEqual(r["n"], 60000)
        self.assertLessEqual(r["S"], 20000)
        self.assertEqual(r["sum"], r["S"])
        css = "document.getElementById('spk-style').textContent"
        before = self.chrome.eval(css)
        self.chrome.eval("sortBy(0, 1); true")
        self.assertTrue(self.chrome.eval(WAIT, timeout=60))
        self.assertEqual(self.chrome.eval(css), before)       # les mêmes enregistrements, dans un autre ordre


if __name__ == "__main__":
    unittest.main()
