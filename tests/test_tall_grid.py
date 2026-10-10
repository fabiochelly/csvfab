"""Une grille plus haute que ce que Chromium sait mettre en page.

Chromium met la page en page en pixels physiques, plafonnés à 2^25 : à l'échelle 2 (l'écran de
l'utilisateur), au-delà de 2^24 px CSS — 479 349 lignes de 35 px — l'étendue était coupée, les
dernières lignes inaccessibles et la ligne de totaux sous le bas de la vue (2026-10-10, un fichier
de 540 000 lignes). Au-delà du plafond, la barre de défilement est tenue au plafond et projetée
sur la grille (vscroll, 01-… / 12-…). L'échelle est forcée au lancement (--force-device-scale-factor) :
l'émulation par CDP ne change pas les unités de mise en page et ne reproduit pas le bug."""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

N = 540_000

# Chaque ligne dessinée dans la vue est à l'endroit que sa position dans la grille lui donne,
# et chaque ligne que la vue doit montrer est dessinée.
CHECK = r"""
window.__tall = () => {
  const t = T(), vc = document.getElementById('view-container'), box = vc.getBoundingClientRect();
  const th = thead.offsetHeight, top = vc.vTop, [v0, v1] = viewRows(t), bad = [], have = new Set();
  const vh = container._vh || vc.clientHeight;
  for (const el of tbody.querySelectorAll('.row[data-idx]')) {
    const i = +el.dataset.idx, y = el.getBoundingClientRect().top - box.top, want = th + i * 35 - top;
    have.add(i);
    if (want > -35 && want < vh && Math.abs(y - want) > 1) bad.push(`ligne ${i} à ${Math.round(y)} au lieu de ${Math.round(want)}`);
  }
  for (let i = v0; i <= Math.min(v1, t.filteredData.length - 1); i++) if (!have.has(i)) bad.push(`ligne ${i} pas dessinée`);
  return { bad, top, v0, on: vscroll.on, base: vscroll.base, vmax: vscroll.vmax, smax: vscroll.smax, st: vc.scrollTop };
};
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class TallGridTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/", args=["--force-device-scale-factor=2", "--window-size=1200,800"])
        try:
            cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
            cls.chrome.eval(HELPER)
            cls.chrome.eval(CHECK)
            cls.path = cls.b.tmp("haut.csv")
            with open(cls.path, "w", encoding="utf-8") as f:
                f.write("id;nom;montant\n")
                f.writelines(f"{i};nom {i};{i % 1000},50\n" for i in range(1, N + 1))
        except Exception:
            cls.chrome.close()
            cls.b.stop()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def run_js(self, body):
        return self.chrome.eval(r"""(async (path) => {
          const pause = ms => new Promise(r => setTimeout(r, ms)), vc = document.getElementById('view-container');
          const settle = async () => { await pause(60); renderOnScroll(); await pause(30); };
          await __fx.open(path); await pause(300);
          """ + body + "})" + f"({self.path!r})", timeout=120)

    def test_the_end_is_reachable_with_its_totals(self):
        r = self.run_js(r"""
          vc.scrollTop = 1e9; await settle();
          const t = T(), n = t.filteredData.length, box = vc.getBoundingClientRect(), c = __tall();
          const last = document.querySelector(`#tbody .row[data-idx="${n - 1}"]`), tf = document.getElementById('tfoot');
          const lr = last && last.getBoundingClientRect(), fr = tf.getBoundingClientRect();
          return { c, n, last: lr && [lr.top, lr.bottom], foot: [fr.top, fr.bottom, tf.style.display], bottom: box.bottom - (box.height - vc.clientHeight),
                   end: vc.vTop === vscroll.vmax, ext: vc.scrollHeight };
        """)
        self.assertTrue(r["c"]["on"], "le plafond devait s'appliquer à 540 000 lignes à l'échelle 2")
        self.assertLessEqual(r["ext"], 2 ** 24, "l'étendue du conteneur dépasse le plafond")
        self.assertEqual(r["c"]["bad"], [])
        self.assertIsNotNone(r["last"], "la dernière ligne n'est pas dessinée")
        self.assertEqual(r["foot"][2], "flex", "la ligne de totaux n'est pas affichée")
        self.assertAlmostEqual(r["last"][1], r["foot"][0], delta=1, msg="les totaux ne suivent pas la dernière ligne")
        self.assertLessEqual(r["foot"][1], r["bottom"] + 1, "la ligne de totaux sort du bas de la vue")

    def test_small_steps_are_one_to_one_across_blocks(self):
        # Blocs de 64 lignes : les pas franchissent la base plusieurs fois.
        r = self.run_js(r"""
          vscroll.block = 64; vc.vTop = 300000 * 35; render(); await pause(50);
          const out = [], start = vc.vTop;
          for (let k = 0; k < 40; k++) { vc.scrollTop = vc.scrollTop + 100; await settle(); const c = __tall(); if (c.bad.length) out.push(...c.bad); }
          const moved = vc.vTop - start, base = vscroll.base;
          for (let k = 0; k < 40; k++) { vc.scrollTop = vc.scrollTop - 100; await settle(); const c = __tall(); if (c.bad.length) out.push(...c.bad); }
          const back = vc.vTop - start;
          vscroll.block = 32768; render();
          return { out, moved, back, base };
        """)
        self.assertEqual(r["out"], [])
        self.assertAlmostEqual(r["moved"], 4000, delta=2, msg="un pas de molette doit déplacer la grille d'autant")
        self.assertAlmostEqual(r["back"], 0, delta=2)
        self.assertGreater(r["base"], 0)

    def test_thumb_jump_lands_at_the_same_share(self):
        r = self.run_js(r"""
          const out = [];
          for (const f of [0.5, 0.97, 0.02, 1, 0]) {
            vc.scrollTop = Math.round(vscroll.smax * f); await settle();
            const c = __tall(); out.push([f, c.top / vscroll.vmax, c.bad]);
          }
          return out;
        """)
        for f, share, bad in r:
            self.assertEqual(bad, [], f"saut à {f}")
            self.assertAlmostEqual(share, f, delta=0.001, msg=f"saut à {f} : arrivé à {share}")

    def test_scrollbar_end_after_drift_still_reaches_the_last_row(self):
        # Des pas 1:1 laissent un écart entre la barre et la grille : la fin du défilement le
        # reprend (scrollend), et la molette au bout de la barre fait encore avancer la grille.
        r = self.run_js(r"""
          const at = () => Math.round(vc.vTop * vscroll.smax / vscroll.vmax);
          vc.vTop = vscroll.vmax / 2; await settle();
          vscroll.v -= 5000 * 35; render(); await pause(30);
          const drift = Math.abs(vc.scrollTop - at());
          vc.dispatchEvent(new Event('scrollend')); await settle();
          const after = Math.abs(vc.scrollTop - at()), bad1 = __tall().bad;
          vc.scrollTop = vscroll.smax; await settle();
          vscroll.v = vscroll.vmax - 3000 * 35; render(); await pause(30);
          let guard = 0;
          while (vc.vTop < vscroll.vmax && guard++ < 3000) {
            if (vc.scrollTop < vscroll.smax - 1) { vc.scrollTop = vscroll.smax; await pause(0); }
            vc.dispatchEvent(new WheelEvent('wheel', { deltaY: 400 })); await pause(0);
          }
          await settle();
          return { drift, after, bad1, end: vc.vTop === vscroll.vmax, bad: __tall().bad };
        """)
        self.assertGreater(r["drift"], 1)
        self.assertLessEqual(r["after"], 1, "la fin du défilement n'a pas repris l'écart")
        self.assertEqual(r["bad1"], [])
        self.assertTrue(r["end"], "la molette au bout de la barre n'atteint pas la fin")
        self.assertEqual(r["bad"], [])

    def test_reveal_and_go_to_the_last_row(self):
        r = self.run_js(r"""
          const t = T(), n = t.filteredData.length;
          vc.vTop = 0; await settle();
          setSel(t, n - 1, 0, n - 1, 0); revealCell(n - 1, 0); await settle();
          const last = document.querySelector(`#tbody .row[data-idx="${n - 1}"]`), tf = document.getElementById('tfoot').getBoundingClientRect();
          const a = __tall(), lr = last && last.getBoundingClientRect();
          setSel(t, 0, 0, 0, 0); revealCell(0, 0); await settle();
          const first = document.querySelector('#tbody .row[data-idx="0"]'), b = __tall();
          return { a: a.bad, b: b.bad, lastAboveFoot: !!lr && lr.bottom <= tf.top + 1, first: !!first, top: b.top };
        """)
        self.assertEqual(r["a"], [])
        self.assertTrue(r["lastAboveFoot"], "la dernière ligne révélée doit être au-dessus des totaux")
        self.assertEqual(r["b"], [])
        self.assertTrue(r["first"])
        self.assertEqual(r["top"], 0)

    def test_under_the_cap_nothing_changes(self):
        p = self.b.tmp("court-haut.csv")
        with open(p, "w", encoding="utf-8") as f:
            f.write("a;b\n" + "".join(f"{i};x\n" for i in range(5000)))
        r = self.chrome.eval(r"""(async (path) => {
          await __fx.open(path); await new Promise(r => setTimeout(r, 200));
          const vc = document.getElementById('view-container');
          vc.scrollTop = 1234; await new Promise(r => setTimeout(r, 80));
          return { on: vscroll.on, base: vscroll.base, same: vc.vTop === vc.scrollTop, layer: document.getElementById('grid-layer').scrollTop, st: vc.scrollTop };
        })""" + f"({p!r})")
        self.assertFalse(r["on"])
        self.assertEqual(r["base"], 0)
        self.assertTrue(r["same"])
        self.assertEqual(r["layer"], r["st"])


if __name__ == "__main__":
    unittest.main()
