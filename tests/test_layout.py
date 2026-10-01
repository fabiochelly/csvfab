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


if __name__ == "__main__":
    unittest.main()
