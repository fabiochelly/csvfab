"""La grille dans la vraie page : ce qui doit rester visible, quel que soit le défilement."""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

# Mesure, une fois tout défilé à droite (ou la dernière colonne révélée au clavier),
# où finit la dernière colonne par rapport à la bande de défilement de droite.
MEASURE = r"""
(async (path, how) => {
  const tab = await __fx.open(path), t = __fx.tab(tab);
  await new Promise(r => setTimeout(r, 200));
  const vc = document.getElementById('view-container');
  if (how === 'scroll') vc.scrollLeft = vc.scrollWidth;
  else { setSel(t, 0, t.headers.length - 1, 0, t.headers.length - 1); revealCell(0, t.headers.length - 1); }
  await new Promise(r => setTimeout(r, 200));
  const ths = document.querySelectorAll('#mainTable thead tr:first-child th');
  const strip = document.getElementById('scroll-strip').getBoundingClientRect();
  return { lastRight: Math.round(ths[ths.length - 1].getBoundingClientRect().right), stripLeft: Math.round(strip.left),
           native: vc.offsetWidth - vc.clientWidth };
})"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class LayoutTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        try:
            cls.chrome.call("Emulation.setDeviceMetricsOverride", width=1000, height=700, deviceScaleFactor=1, mobile=False)
            cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
            cls.chrome.eval(HELPER)
        except Exception:
            cls.chrome.close()
            cls.b.stop()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def wide_file(self, name, rows):
        head = ";".join(f"colonne_{i}" for i in range(12))
        body = "".join(";".join(f"valeur {r}-{i} " + "x" * 20 for i in range(12)) + "\n" for r in range(rows))
        p = self.b.tmp(name)
        with open(p, "w", encoding="utf-8") as f:
            f.write(head + "\n" + body)
        return p

    def check(self, rows, how):
        p = self.wide_file(f"wide-{rows}-{how}.csv", rows)
        m = self.chrome.eval(f"({MEASURE})({p!r}, {how!r})")
        self.assertLessEqual(m["lastRight"], m["stripLeft"],
                             f"la dernière colonne finit à {m['lastRight']} px, sous la bande qui commence à {m['stripLeft']} px "
                             f"(barre native : {m['native']} px)")

    # Toutes les lignes tiennent à l'écran : pas de barre verticale native, la bande
    # prend sa place — la grille doit pouvoir défiler 24 px plus loin (bug du 2026-10-02).
    def test_few_rows_scrolled_right(self):
        self.check(5, "scroll")

    def test_few_rows_last_column_revealed(self):
        self.check(5, "reveal")

    def test_many_rows_scrolled_right(self):
        self.check(400, "scroll")

    def test_many_rows_last_column_revealed(self):
        self.check(400, "reveal")


    # Un fichier court dans une fenêtre basse : arrivée en bas pas à pas, la fenêtre de
    # lignes dessinées ne pouvait plus avancer d'un pas entier (ROW_STEP) et les dernières
    # lignes restaient vides, le fond rayé à leur place (bug du 2026-10-07, 31 lignes).
    def test_all_rows_drawn_at_the_end(self):
        p = self.wide_file("court.csv", 31)
        r = self.chrome.eval(r"""(async (path) => {
          const pause = ms => new Promise(r => setTimeout(r, ms));
          await __fx.open(path); await pause(200);
          const vc = document.getElementById('view-container'), out = [];
          vc.style.height = '420px'; render(); await pause(100);
          for (let y = 0; y <= vc.scrollHeight; y += 60) { vc.scrollTop = y; renderOnScroll(); }
          await pause(100);
          const [v0, v1] = viewRows(T()), have = new Set([...document.querySelectorAll('#tbody .row')].map(x => +x.dataset.idx));
          for (let i = v0; i <= Math.min(v1, T().filteredData.length - 1); i++) if (!have.has(i)) out.push(i);
          /* Un saut puis de petits pas, sur un écran court : la fenêtre maigre doit couvrir l'écran. */
          vc.scrollTop = 0; render(true); await pause(50);
          for (let y = 0; y < 200; y += 25) { vc.scrollTop = y; renderOnScroll(); }
          const [w0, w1] = viewRows(T()), have2 = new Set([...document.querySelectorAll('#tbody .row')].map(x => +x.dataset.idx));
          for (let i = w0; i <= Math.min(w1, T().filteredData.length - 1); i++) if (!have2.has(i)) out.push('saut ' + i);
          vc.style.height = ''; resizeContainer();
          return out;
        })""" + f"({p!r})")
        self.assertEqual(r, [], "lignes à l'écran mais pas dessinées")


if __name__ == "__main__":
    unittest.main()
