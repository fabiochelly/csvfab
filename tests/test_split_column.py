"""Séparer une colonne (« Split a column », ui/js/07-columns-structure.js) : l'en-tête suit les lignes.

Le 2026-10-07 l'utilisateur a séparé une adresse sur un code postal (regex \\b\\d{5}\\b) : le
premier clic sur Split ne faisait rien, le second ajoutait les champs aux lignes mais pas les
titres à l'en-tête — tout était décalé, et enregistré ainsi. La cause : « Split into files »
(42-…) déclarait une autre fonction splitRefresh(), qui dans le script unique (/app.js, les
fichiers bout à bout) remplaçait celle de la boîte, dont l'aperçu ne se faisait donc plus.
D'où aussi le test qui refuse deux déclarations de même nom au niveau supérieur du script.

Sauté sans Chromium (sauf le test du script).
"""

import glob
import json
import os
import re
import tempfile
import shutil
import unittest
from collections import Counter

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

SPLIT = """(async (sep, regex, keep, n) => {
  const $ = id => document.getElementById(id);
  openSplit(1);
  $('split-sep').value = sep; $('split-re').checked = regex; $('split-keep').checked = keep; $('split-trim').checked = true;
  $('split-sep').dispatchEvent(new Event('input'));
  if (n) { $('split-n').value = n; $('split-n').dispatchEvent(new Event('input')); }
  /* Le bouton reste désactivé tant que le garde-fou des regex (40-…) vérifie le motif : on attend l'aperçu, comme l'utilisateur. */
  for (let k = 0; k < 100 && $('split-go').disabled; k++) await new Promise(r => setTimeout(r, 20));
  $('split-go').click();
  for (let k = 0; k < 100 && $('modal-split').style.display === 'block'; k++) await new Promise(r => setTimeout(r, 20));
  const t = T();
  return { open: $('modal-split').style.display, headers: t.headers, rows: t.allData.map(r => r.data.slice()) };
})"""


class ScriptTest(unittest.TestCase):
    def test_no_name_declared_twice(self):
        """Les fichiers ui/js/ forment un seul script : un nom déclaré deux fois, la dernière déclaration gagne."""
        names = Counter()
        for path in sorted(glob.glob(os.path.join(ROOT, "ui", "js", "*.js"))):
            with open(path, encoding="utf-8") as f:
                for line in f:
                    m = re.match(r"(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)|(?:const|let|var|class)\s+([A-Za-z_$][\w$]*)", line)
                    if m:
                        names[m.group(1) or m.group(2)] += 1
        self.assertEqual([n for n, k in names.items() if k > 1], [])


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class SplitColumnTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="csvfab-split-")
        cls.path = os.path.join(cls.dir, "clients.csv")
        with open(cls.path, "w", encoding="utf-8", newline="") as f:
            f.write("Nom;Adresse;Ville\n")
            f.write("Tardy;22 ALLEE ZEPHIRIN 45160 OLIVET;Rouen\n")
            f.write("Koch;;Nancy\n")
            f.write("Gallois;5 Rue du Pré;Lyon\n")
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1300,900",))
        cls.chrome.wait_for("typeof openSplit === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def split(self, sep, regex, keep, n=None):
        self.chrome.eval(f"__fx.open({self.path!r})", timeout=60)
        return self.chrome.eval(f"({SPLIT})({json.dumps(sep)}, {json.dumps(regex)}, {json.dumps(keep)}, {json.dumps(n)})", timeout=30)

    def test_regex_split_keeps_header_in_step(self):
        r = self.split(r"\b\d{5}\b", True, False, "1")      # 1 tapé : il en faut 2, comme l'utilisateur
        self.assertEqual(r["open"], "none")
        self.assertEqual(r["headers"], ["Nom", "Adresse 1", "Adresse 2", "Ville"])
        self.assertEqual(r["rows"][0], ["Tardy", "22 ALLEE ZEPHIRIN", "OLIVET", "Rouen"])
        self.assertEqual(r["rows"][2], ["Gallois", "5 Rue du Pré", "", "Lyon"])
        for row in r["rows"]:
            self.assertEqual(len(row), len(r["headers"]))

    def test_keep_original_column(self):
        r = self.split(r"\b\d{5}\b", True, True)
        self.assertEqual(r["headers"], ["Nom", "Adresse", "Adresse 1", "Adresse 2", "Ville"])
        self.assertEqual(r["rows"][0], ["Tardy", "22 ALLEE ZEPHIRIN 45160 OLIVET", "22 ALLEE ZEPHIRIN", "OLIVET", "Rouen"])

    def test_saved_header_matches(self):
        self.split(" ", False, False, "2")
        out = self.b.tmp("split-out.csv")
        self.chrome.eval(f"(async () => {{ await srvWrite(T(), {out!r}, false); return true; }})()", timeout=30)
        with open(out, encoding="utf-8") as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[0], "Nom;Adresse 1;Adresse 2;Ville")
        self.assertEqual(lines[1], "Tardy;22;ALLEE ZEPHIRIN 45160 OLIVET;Rouen")
        self.assertTrue(all(len(x.split(";")) == 4 for x in lines))


if __name__ == "__main__":
    unittest.main()
