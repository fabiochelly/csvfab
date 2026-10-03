"""Découper un fichier en un fichier par valeur, assembler des onglets en un seul
(ui/js/42-split-combine.js), éprouvés dans la vraie page.

1. Découpage : un dossier neuf à côté du fichier (jamais un dossier existant), un fichier
   par valeur avec la ligne de titres, les lignes copiées octet pour octet (séparateur,
   fins de ligne, guillemets d'origine), une ligne modifiée écrite modifiée, les noms
   rendus sûrs et sans collision ; la correspondance « slug » regroupe Paris et PARIS.
2. Assemblage : colonnes rapprochées par titre (casse, accents) quel que soit leur ordre,
   fichiers d'encodages et de séparateurs différents, colonne absente laissée vide,
   colonne d'origine, « seulement les colonnes communes », correspondance exacte.
3. La route POST /api/mkdir : un seul niveau, 409 si le dossier existe.

Sauté sans Chromium.
"""

import os
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge, q
from tests.test_forensic import HELPER

VENTES = (b"id;ville;note\r\n"
          b"1;Paris;\"a;b\"\r\n"
          b"2;Lyon;x\r\n"
          b"3;;vide\r\n"
          b"4;A/B;slash\r\n"
          b"5;PARIS;caps\r\n"
          b"6;Paris;  espaces  \r\n")


class MkdirTest(unittest.TestCase):
    def test_mkdir(self):
        b = Bridge().start()
        try:
            p = b.tmp("neuf")
            self.assertEqual(b.post("/api/mkdir" + q(path=p), b"").status, 200)
            self.assertTrue(os.path.isdir(p))
            self.assertEqual(b.post("/api/mkdir" + q(path=p), b"").status, 409)
            self.assertEqual(b.post("/api/mkdir" + q(path=os.path.join(p, "a", "b")), b"").status, 400)
        finally:
            b.stop()


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class SplitCombineTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof openSplitFiles === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def make(self, name, data):
        p = self.b.tmp(name)
        with open(p, "wb") as f:
            f.write(data)
        return p

    def split(self, tab, col, match="exact", edit=None):
        return self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({tab});
            {edit or ''}
            splitDialog();
            document.querySelector('input[name="sf-match"][value="{match}"]').checked = true;
            openSplitFiles({col});
            for (let i = 0; i < 100 && !(splitF && splitF.groups); i++) await new Promise(r => setTimeout(r, 30));
            const names = splitF.groups.map(g => g.name);
            await applySplitFiles();
            return names;
        }})()""", timeout=30)

    def files(self, folder):
        out = {}
        for n in sorted(os.listdir(folder)):
            with open(os.path.join(folder, n), "rb") as f:
                out[n] = f.read()
        return out

    def test_split_by_value(self):
        p = self.make("ventes.csv", VENTES)
        tab = self.chrome.eval(f"__fx.open({p!r})")
        names = self.split(tab, 1)
        self.assertEqual(names, ["ventes - Paris.csv", "ventes - Lyon.csv", "ventes - (empty).csv", "ventes - A_B.csv", "ventes - PARIS (2).csv"])
        folder = os.path.join(os.path.dirname(p), "ventes - by ville")
        got = self.files(folder)
        head = b"id;ville;note\r\n"
        self.assertEqual(got["ventes - Paris.csv"], head + b"1;Paris;\"a;b\"\r\n6;Paris;  espaces  \r\n")   # octets d'origine
        self.assertEqual(got["ventes - Lyon.csv"], head + b"2;Lyon;x\r\n")
        self.assertEqual(got["ventes - (empty).csv"], head + b"3;;vide\r\n")
        self.assertEqual(got["ventes - A_B.csv"], head + b"4;A/B;slash\r\n")
        self.assertEqual(got["ventes - PARIS (2).csv"], head + b"5;PARIS;caps\r\n")
        with open(p, "rb") as f:
            self.assertEqual(f.read(), VENTES, "le fichier découpé n'est pas touché")
        # Une seconde fois, slug, une ligne modifiée : un autre dossier, Paris et PARIS ensemble.
        names = self.split(tab, 1, "slug", "setCells(t, [[t.allData[1], 2, 'modifié']], 'set');")
        self.assertEqual(names, ["ventes - Paris.csv", "ventes - Lyon.csv", "ventes - (empty).csv", "ventes - A_B.csv"])
        got = self.files(os.path.join(os.path.dirname(p), "ventes - by ville (2)"))
        self.assertEqual(got["ventes - Paris.csv"], head + b"1;Paris;\"a;b\"\r\n5;PARIS;caps\r\n6;Paris;  espaces  \r\n")
        self.assertEqual(got["ventes - Lyon.csv"], head + "2;Lyon;modifié\r\n".encode())
        self.assertEqual(set(got), set(names))

    def combine(self, first, others, match="slug", common=False, source=False):
        return self.chrome.eval(f"""(async () => {{
            const ids = [];
            for (const p of {others!r}) ids.push(await __fx.open(p));
            const id = await __fx.open({first!r}); activateTab(id);
            combineDialog();
            document.querySelector('input[name="cb-match"][value="{match}"]').checked = true;
            document.getElementById('cb-common').checked = {str(common).lower()};
            document.getElementById('cb-source').checked = {str(source).lower()};
            openCombine();
            combine.on = new Set([id, ...ids]);
            await combineRefresh();
            const cols = combine.plan.out.map(o => o.name), enc = combine.plan.we.enc;
            await applyCombine();
            return {{ cols, enc, active: T().name }};
        }})()""", timeout=30)

    def test_combine(self):
        a = self.make("clients-a.csv", "id;Nom;Ville\r\n1;Dupont;Paris\r\n2;Martin;\"Le Havre\"\r\n".encode())
        b = self.make("clients-b.csv", "ville,nom,code\r\nLyon,Élodie,7\r\nNantes,\"Durand, fils\",8\r\n".encode("cp1252"))
        r = self.combine(a, [b], source=True)
        self.assertEqual(r["cols"], ["id", "Nom", "Ville", "code"])
        self.assertEqual(r["enc"], "utf-8")                     # deux encodages : UTF-8 les tient tous
        self.assertEqual(r["active"], "clients-a - combined.csv")
        with open(self.b.tmp("clients-a - combined.csv"), "rb") as f:
            self.assertEqual(f.read().decode("utf-8"),
                             "id;Nom;Ville;code;source file\r\n1;Dupont;Paris;;clients-a.csv\r\n2;Martin;Le Havre;;clients-a.csv\r\n"
                             ";Élodie;Lyon;7;clients-b.csv\r\n;Durand, fils;Nantes;8;clients-b.csv\r\n")
        r = self.combine(a, [b], common=True)
        self.assertEqual(r["cols"], ["Nom", "Ville"])
        with open(self.b.tmp("clients-a - combined (2).csv"), "rb") as f:
            self.assertEqual(f.read().decode("utf-8"), "Nom;Ville\r\nDupont;Paris\r\nMartin;Le Havre\r\nÉlodie;Lyon\r\nDurand, fils;Nantes\r\n")
        r = self.combine(a, [b], match="exact")
        self.assertEqual(r["cols"], ["id", "Nom", "Ville", "ville", "nom", "code"])
