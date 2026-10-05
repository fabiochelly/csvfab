"""Les correspondances de la recherche sur la bande de défilement (stripFind, 28-scroll-strip.js).

Ouvrir la barre de recherche et chercher dessine un repère par groupe de lignes trouvées, sur
le bord droit de la bande, à la hauteur de ces lignes ; une recherche sans résultat n'en
dessine aucun, fermer la barre les efface, une fenêtre redimensionnée les redessine à sa taille.

Sauté sans Chromium.
"""

import os
import shutil
import tempfile
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

# Ce qu'il faut pour lire la bande : les ordonnées (en px CSS) des pixels peints.
PAINTED = r"""
window.__painted = () => {
  const x = document.querySelector('#scroll-strip .ss-find');
  if (!x || !x.width) return [];
  const d = x.getContext('2d').getImageData(0, 0, x.width, x.height).data, k = x.height / parseFloat(x.style.height), out = [];
  for (let y = 0; y < x.height; y++) if (d[y * x.width * 4 + 3]) { const v = Math.floor(y / k); if (out[out.length - 1] !== v) out.push(v); }
  return out;
};
window.__find = async q => {
  document.getElementById('sr-find').value = q; findChanged();
  await new Promise(r => setTimeout(r, 250));
  const t0 = performance.now(); while (!find.done && performance.now() - t0 < 10000) await new Promise(r => setTimeout(r, 20));
  return find.n;
};
true
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class FindStripTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="csvfab-fstrip-")
        cls.path = os.path.join(cls.dir, "lignes.csv")
        with open(cls.path, "w", encoding="utf-8", newline="") as f:
            f.write("n;texte\n")
            for i in range(1, 5001):
                f.write(f"{i};{'cible' if i in (100, 2500, 4900) else 'autre'} {i}\n")
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1200,800",))
        cls.chrome.wait_for("typeof stripFind === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)
        cls.chrome.eval(PAINTED)
        cls.chrome.eval(f"__fx.open({cls.path!r})", timeout=60)
        cls.chrome.eval("toggleSRBar(); true")

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def test_ticks_where_the_matches_are(self):
        self.assertEqual(self.chrome.eval("__find('cible')"), 3)
        ys = self.chrome.eval("__painted()")
        want = self.chrome.eval("[99, 2499, 4899].map(i => Math.floor(rowY(stripGeom(T()), i)))")
        groups = []
        for y in ys:
            if groups and y - groups[-1][-1] <= 1:
                groups[-1].append(y)
            else:
                groups.append([y])
        self.assertEqual(len(groups), 3, ys)
        for g, w in zip(groups, want):
            self.assertLessEqual(abs(g[0] - w), 1, (groups, want))
        self.assertGreater(len(self.chrome.eval("__find('autre').then(() => __painted())")), 100)   # partout

    def test_none_closed_resized(self):
        self.assertEqual(self.chrome.eval("__find('introuvable')"), 0)
        self.assertEqual(self.chrome.eval("__painted()"), [])
        self.chrome.eval("__find('cible')")
        before = self.chrome.eval("__painted()")
        self.assertTrue(before)
        self.chrome.call("Emulation.setDeviceMetricsOverride", width=1200, height=500, deviceScaleFactor=1, mobile=False)
        try:
            self.chrome.eval("new Promise(r => setTimeout(r, 300))")
            after = self.chrome.eval("__painted()")
            self.assertTrue(after)
            self.assertLess(max(after), max(before))         # la bande a raccourci, les repères aussi
        finally:
            self.chrome.call("Emulation.clearDeviceMetricsOverride")
        self.chrome.eval("toggleSRBar(); true")
        try:
            self.assertEqual(self.chrome.eval("__painted()"), [])
        finally:
            self.chrome.eval("toggleSRBar(); true")


if __name__ == "__main__":
    unittest.main()
