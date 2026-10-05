"""La carte du fichier (ui/js/46-file-map.js) : ce qu'elle compte et où elle mène.

Un petit fichier aux défauts connus — 20 « N/A » dans une colonne de nombres, 5 lignes
d'un champ de trop, une colonne vide à partir de la ligne 2 000, une cellule modifiée —
est compté exactement (moins de MAP_EXACT lignes) : chaque défaut à sa place et en son
nombre. Un clic sur un paquet de lignes mène à la première valeur hors type de la colonne ;
une colonne masquée n'a pas de bande ; Ctrl+M ouvre et ferme.

Sauté sans Chromium.
"""

import os
import shutil
import tempfile
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

WAIT = "(async () => { openFileMap(); while (!fmap.done) await new Promise(r => setTimeout(r, 20)); return true; })()"


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class FileMapTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="csvfab-map-")
        cls.path = os.path.join(cls.dir, "carte.csv")
        with open(cls.path, "w", encoding="utf-8", newline="") as f:
            f.write("montant;date;nom;note\n")
            for i in range(1, 3001):
                m = "N/A" if 500 <= i < 520 else f"{i},50"
                note = "" if i > 2000 else f"note {i}"
                extra = ";de trop" if 1000 <= i < 1005 else ""
                f.write(f"{m};{1 + i % 28:02d}/03/2024;nom {i};{note}{extra}\n")
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1300,900",))
        cls.chrome.wait_for("typeof openFileMap === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def setUp(self):
        self.chrome.eval(f"__fx.open({self.path!r})", timeout=60)

    def tearDown(self):
        self.chrome.eval("closeAllModals(); true")

    def test_counts(self):
        self.chrome.eval("const t = T(); setCells(t, [[t.allData[2500], 2, 'changé']], 'test'); true")
        self.chrome.eval(WAIT, timeout=60)
        r = self.chrome.eval("""(() => {
          const m = fmap, nc = m.g.cols.length, nb = m.g.nb, sum = (a, k) => { let s = 0; for (let b = 0; b < nb; b++) s += a[b * nc + k]; return s; };
          const bin = i => { let b = 0; while (m.lo[b + 1] <= i) b++; return b; };
          let emptyAfter = 0, filledBefore = 0, before = 0;
          for (let b = 0; b < nb; b++) { if (m.lo[b] >= 2000) emptyAfter += m.fill[b * nc + 3]; if (m.lo[b + 1] <= 2000) { filledBefore += m.fill[b * nc + 3]; before = m.lo[b + 1]; } }
          let first = -1; for (let b = 0; b < nb; b++) { const f = m.firstBad[b * nc]; if (f >= 0 && (first < 0 || f < first)) first = f; }
          return { kinds: m.kinds.join(''), seen: m.seen.reduce((a, x) => a + x, 0), bad: sum(m.bad, 0), badDates: sum(m.bad, 1), first,
                   irr: m.irr.reduce((a, x) => a + x, 0), edit: m.edit.reduce((a, x) => a + x, 0), editBin: m.edit[bin(2500)],
                   emptyAfter, filledBefore, before, exact: !m.sampled, sum: document.getElementById('mp-sum').textContent };
        })()""")
        self.assertEqual(r["kinds"], "ndtt")
        self.assertEqual(r["seen"], 3000)                 # chaque ligne comptée une fois
        self.assertEqual(r["bad"], 20)
        self.assertEqual(r["badDates"], 0)
        self.assertEqual(r["first"], 499)                 # la ligne 500 du fichier, la 499e de la vue (0-based)
        self.assertEqual((r["irr"], r["edit"], r["editBin"]), (5, 1, 1))
        self.assertEqual(r["emptyAfter"], 0)
        self.assertEqual(r["filledBefore"], r["before"])        # tous les paquets entièrement avant la ligne 2 000
        self.assertGreater(r["before"], 1900)
        self.assertTrue(r["exact"])
        self.assertIn("3", r["sum"])

    def test_click_goes_to_the_first_misfit(self):
        self.chrome.eval(WAIT, timeout=60)
        r = self.chrome.eval("""(() => {
          const m = fmap, cv = document.getElementById('mp-cv').getBoundingClientRect();
          let b = 0; while (m.lo[b + 1] <= 510) b++;
          const y = cv.top + (b + .5) * m.g.H / m.g.nb, x = cv.left + MAP_GUT + 4 + m.g.cw / 2;
          document.getElementById('mp-cv').dispatchEvent(new MouseEvent('mousemove', { clientX: x, clientY: y, bubbles: true }));
          const info = document.getElementById('mp-info').textContent;
          document.getElementById('mp-cv').dispatchEvent(new MouseEvent('click', { clientX: x, clientY: y, bubbles: true }));
          return { info, open: document.getElementById('modal-map').style.display, sel: sel && [sel.fr, sel.fc], first: m.firstBad[b * m.g.cols.length] };
        })()""")
        self.assertIn("montant", r["info"])
        self.assertIn("not numbers", r["info"])
        self.assertEqual(r["open"], "none")
        self.assertEqual(r["sel"], [r["first"], 0])
        self.assertGreaterEqual(r["first"], 499)

    def test_hidden_column_and_shortcut(self):
        self.chrome.eval("T().hiddenCols.add(2); true")
        try:
            self.chrome.eval(WAIT, timeout=60)
            self.assertEqual(self.chrome.eval("fmap.g.cols"), [0, 1, 3])
        finally:
            self.chrome.eval("T().hiddenCols.delete(2); closeAllModals(); true")
        key = "window.dispatchEvent(new KeyboardEvent('keydown', { key: 'm', ctrlKey: true, bubbles: true })); document.getElementById('modal-map').style.display"
        self.assertEqual(self.chrome.eval(key), "block")
        self.assertEqual(self.chrome.eval(key), "none")


if __name__ == "__main__":
    unittest.main()
