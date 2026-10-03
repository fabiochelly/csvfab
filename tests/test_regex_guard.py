"""Garde des regex (ui/js/40-regex-guard.js), éprouvée dans la vraie page.

Une regex à retour arrière catastrophique, sur les valeurs qui la font
exploser, doit être refusée sans que la page se fige : filtre global, filtre
de colonne, barre de recherche, séparation de colonne, formule. Une regex
ordinaire passe, après sa vérification ou tout de suite si elle n'a pas de
répétition.

Sauté sans Chromium.
"""

import json
import os
import time
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

BAD = "^(a+)+$"            # exponentiel sur « aaa…ab »
ROWS = ["id;mot"] + [f"{i};{'a' * 30 + 'b' if i % 2 else 'abc' + str(i)}" for i in range(1, 401)]


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class RegexGuardTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.path = cls.b.tmp("motifs.csv")
        with open(cls.path, "w", encoding="utf-8", newline="") as f:
            f.write("\r\n".join(ROWS) + "\r\n")
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof regexGate === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)
        cls.chrome.eval(f"__fx.open({cls.path!r})")

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def filt(self, js_setup):
        """Applique un filtre ; rend (état juste après, état une fois la vérification finie)."""
        t0 = time.time()
        first = self.chrome.eval(f"(() => {{ {js_setup}; applyFilters(); const t = T(); return {{ err: t.reErr || t.exprErr, n: t.filteredData.length }}; }})()")
        self.assertLess(time.time() - t0, 2, "la page s'est figée")
        self.chrome.wait_for("!T().reErr.startsWith('checking') && !(T().exprErr || '').startsWith('checking')", timeout=10)
        last = self.chrome.eval("({ err: T().reErr || T().exprErr, n: T().filteredData.length })")
        return first, last

    def reset(self):
        self.chrome.eval("""(() => { const t = T(); t.colFilters = {};
            document.getElementById('global-search').value = ''; document.getElementById('use-regex').checked = false;
            document.getElementById('use-expr').checked = false; applyFilters(); })()""")

    def test_global_filter_refuses_catastrophic_regex(self):
        self.reset()
        first, last = self.filt(f"document.getElementById('use-regex').checked = true; document.getElementById('global-search').value = {json.dumps(BAD)}")
        self.assertIn("checking", first["err"])
        self.assertIn("too long", last["err"])
        self.assertEqual(last["n"], 400, "la vue doit rester telle quelle")

    def test_column_filter_refuses_catastrophic_regex(self):
        self.reset()
        _, last = self.filt(f"document.getElementById('use-regex').checked = true; T().colFilters = {{ 1: {json.dumps(BAD + ' ')} }}")
        self.assertIn("too long", last["err"])

    def test_ordinary_regex_filters_after_check(self):
        self.reset()
        first, last = self.filt("document.getElementById('use-regex').checked = true; document.getElementById('global-search').value = 'a{30}b'")
        self.assertEqual(last["err"], "")
        self.assertEqual(last["n"], 200)

    def test_regex_without_repetition_is_not_held(self):
        self.reset()
        first, _ = self.filt("document.getElementById('use-regex').checked = true; document.getElementById('global-search').value = 'abc1'")
        self.assertEqual(first["err"], "")
        self.assertGreater(first["n"], 0)

    def test_formula_filter_refuses_catastrophic_pattern(self):
        self.reset()
        _, last = self.filt(f"document.getElementById('use-expr').checked = true; document.getElementById('global-search').value = {json.dumps('matches({mot}, ' + json.dumps(BAD + '  ') + ')')}")
        self.assertIn("too long", last["err"])

    def test_find_bar_and_split_refuse_it_too(self):
        self.reset()
        res = self.chrome.eval(f"""(async () => {{
            const t = T();
            const start = performance.now();
            const g1 = sepFinder({json.dumps(BAD + '   ')}, true, t, 1, null);
            await new Promise(r => setTimeout(r, 1800));
            const g2 = sepFinder({json.dumps(BAD + '   ')}, true, t, 1, null);
            return {{ first: g1.error, then: g2.error, ms: performance.now() - start }};
        }})()""", timeout=20)
        self.assertIn("checking", res["first"])
        self.assertIn("too long", res["then"])
        res = self.chrome.eval(f"""(async () => {{
            if (!findBarOpen()) toggleSRBar();
            document.getElementById('sr-regex').checked = true;
            document.getElementById('sr-find').value = {json.dumps(BAD + '    ')};
            findSpec(T());
            await new Promise(r => setTimeout(r, 1800));
            const S = findSpec(T());
            return {{ error: S.error, guard: !!S.guard, pos: (findPos(T()), document.getElementById('sr-pos').textContent) }};
        }})()""", timeout=20)
        self.assertTrue(res["guard"])
        self.assertIn("too long", res["error"])
        self.assertEqual(res["pos"], "too slow")
