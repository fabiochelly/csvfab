"""Suppression des mots bruits (ui/js/39-noise-words.js), éprouvée dans la vraie page.

1. Les règles : mots entiers, casse et accents ignorés, et le trou laissé
   rangé (espaces doublés, virgule ou tiret orphelins, parenthèses vides).
2. Le dialogue, de bout en bout : une colonne ou toutes, les lignes
   affichées seulement ; l'écriture ne touche que les enregistrements
   modifiés, et l'annulation redonne les octets d'origine.

Sauté sans Chromium.
"""

import json
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

CASES = [
    # (valeur, mots, options, attendu)
    ("Dupont SARL, Paris", "SARL", {}, "Dupont, Paris"),
    ("SARL Dupont", "SARL", {}, "Dupont"),
    ("Dupont - SARL", "SARL", {}, "Dupont"),
    ("Dupont, SARL, Paris", "SARL", {}, "Dupont, Paris"),
    ("Dupont SARL ; Paris", "SARL", {}, "Dupont ; Paris"),
    ("(SARL) Dupont", "SARL", {}, "Dupont"),
    ("Les Etablissements Dupont", "les\nétablissements", {}, "Dupont"),
    ("M. Jean Dupont", "M.", {}, "Jean Dupont"),
    ("Dupont ET CIE", "et, et cie", {}, "Dupont"),
    ("Société d'Exploitation", "d'", {}, "Société Exploitation"),
    ("Sarlat", "SARL", {}, "Sarlat"),
    ("SARL", "SARL", {}, ""),
    ("garde  ses   espaces SARL", "SARL", {}, "garde  ses   espaces"),
    ("Le chat LE", "Le", {"caseless": False}, "chat LE"),
    ("Société", "societe", {"accents": False}, "Société"),
    ("Dupont® A-B", "®; -", {"whole": False}, "Dupont AB"),
    ("Dupont SARL, Paris", "SARL", {"tidy": False}, "Dupont , Paris"),
]

CSV = ("nom;ville\r\n"
       "Dupont SARL;Paris SARL\r\n"
       "Martin;Lyon\r\n"
       "Durand et Fils;Nantes\r\n").encode("utf-8")


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class NoiseWordsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def test_rules(self):
        for value, words, opts, want in CASES:
            with self.subTest(value=value, words=words, opts=opts):
                o = {"caseless": True, "accents": True, "whole": True, "tidy": True, **opts}
                got = self.chrome.eval(f"(() => {{ const o = {json.dumps(o)}; "
                                       f"return noiseStrip({json.dumps(value)}, noiseRegex(noiseWords({json.dumps(words)}), o), o.tidy); }})()")
                self.assertEqual(got, want)

    def run_dialog(self, path, words, col):
        self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({path!r});
            openNoise({'null' if col is None else col});
            document.getElementById('nz-words').value = {json.dumps(words)};
            document.getElementById('nz-col').value = {json.dumps('' if col is None else str(col))};
            noiseRefresh();
            window.__nzId = id;
            return id;
        }})()""")
        stats = self.chrome.eval("document.getElementById('nz-stats').textContent")
        self.chrome.eval("applyNoise()")
        out = self.b.tmp("out.csv")
        self.chrome.eval(f"__fx.write(__nzId, {out!r})")
        with open(out, "rb") as f:
            return stats, f.read()

    def test_one_column_then_undo(self):
        path = self.b.tmp("one.csv")
        with open(path, "wb") as f:
            f.write(CSV)
        stats, got = self.run_dialog(path, "SARL, et fils", 0)
        self.assertIn("2 cells will change", stats)
        self.assertEqual(got, CSV.replace(b"Dupont SARL;", b"Dupont;").replace(b"Durand et Fils", b"Durand"))
        self.chrome.eval("__fx.undo(__nzId)")
        out = self.b.tmp("undone.csv")
        self.chrome.eval(f"__fx.write(__nzId, {out!r})")
        with open(out, "rb") as f:
            self.assertEqual(f.read(), CSV)

    def test_every_column(self):
        path = self.b.tmp("all.csv")
        with open(path, "wb") as f:
            f.write(CSV)
        _, got = self.run_dialog(path, "sarl", None)
        self.assertEqual(got, CSV.replace(b" SARL", b""))


if __name__ == "__main__":
    unittest.main()
