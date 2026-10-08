"""« Ce que j'ai remarqué » (ui/js/49-insights.js), éprouvé dans la vraie page.

Un fichier de 200 lignes aux défauts posés à la main : un code postal qui décide
de la ville sauf sur une ligne, deux codes postaux sans leur zéro, deux noms en
capitales, deux dates écrites autrement, un montant à virgule décimale de point,
TTC = HT × 1,2 sauf sur une ligne, une quantité hors de toute mesure et une
négative, deux lignes copiées (dont l'identifiant revient), une colonne vide,
une colonne constante. Chaque défaut doit donner sa carte, et « Show the rows »
marquer exactement les bonnes lignes. Un fichier propre ne donne rien à voir.

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

VILLES = [("75011", "Paris"), ("69003", "Lyon"), ("13001", "Marseille"), ("33000", "Bordeaux"),
          ("31000", "Toulouse"), ("44000", "Nantes")]
NOMS = ["Martin", "Bernard", "Thomas", "Petit", "Robert", "Richard", "Durand", "Dubois", "Moreau", "Laurent"]


def fr(x):
    return f"{x:.2f}".replace(".", ",")


def piege():
    lines = ["id;cp;ville;nom;date;ht;tva;ttc;pays;fax;qte"]
    for i in range(198):
        cp, ville = VILLES[i % 6]
        if i == 12:
            ville = "Pariss"                                       # cp → ville, une ligne en désaccord
        if i in (54, 66):
            cp = "7501"                                            # le zéro perdu (deux lignes de Paris)
        nom = NOMS[i % 10].upper() if i in (20, 21) else NOMS[i % 10]
        d, m = 1 + i % 28, 1 + i % 12
        date = f"2024-{m:02d}-{d:02d}" if i in (30, 31) else f"{d:02d}/{m:02d}/2024"
        ht = (i * 7) % 300 + 10.5
        tva, ttc = round(ht * .2, 2), round(ht * 1.2, 2)
        if i == 40:
            ttc += 5                                               # la règle cassée
        hts = f"{ht:.2f}" if i == 70 else fr(ht)                   # un point décimal parmi les virgules
        qte = 5000 if i == 60 else -3 if i == 61 else 1 + (i * 3) % 7
        lines.append(f"{i + 1};{cp};{ville};{nom};{date};{hts};{fr(tva)};{fr(ttc)};FR;;{qte}")
    lines += [lines[1], lines[2]]                                  # deux copies exactes (id compris)
    return "\r\n".join(lines) + "\r\n"


PROPRE = "id;nom;ville\r\n" + "".join(f"{i};{NOMS[i % 10]};{VILLES[i % 6][1]}\r\n" for i in range(1, 60))


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class InsightsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p = cls.b.tmp("piege.csv")
        with open(cls.p, "w", encoding="utf-8", newline="") as f:
            f.write(piege())
        cls.q = cls.b.tmp("propre.csv")
        with open(cls.q, "w", encoding="utf-8", newline="") as f:
            f.write(PROPRE)
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof openInsights === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def cards(self, path):
        return self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({path!r});
            openInsights();
            for (let i = 0; i < 300 && !(insights && insights.done); i++) await new Promise(r => setTimeout(r, 20));
            const out = [...document.querySelectorAll('#in-list .in-card')].map(e => ({{ id: +e.dataset.id, sev: e.classList.contains('warn') ? 'warn' : 'info', text: e.textContent }}));
            return {{ out, sum: document.getElementById('in-sum').textContent, none: !!document.querySelector('.in-none') }};
        }})()""", timeout=60)

    def find(self, cards, *words):
        hit = [c for c in cards if all(w in c["text"] for w in words)]
        self.assertTrue(hit, f"aucune carte avec {words} parmi : " + " | ".join(c["text"] for c in cards))
        return hit[0]

    def shown(self, card_id):
        """Les lignes (leur id) et les infobulles marquées par « Show the rows » d'une carte."""
        return self.chrome.eval(f"""(async () => {{
            await inShow({card_id});
            const t = T(), m = t.rowMark;
            const cells = [...m.cells].map(([r, cs]) => [cellOf(r, 0), ...[...cs].flat()]);
            const out = {{ rows: [...m.rows].map(r => cellOf(r, 0)).sort((a, b) => a - b), cells, shown: t.filteredData.length }};
            clearRowMark(); openInsights();
            return out;
        }})()""", timeout=60)

    def test_cards(self):
        r = self.cards(self.p)
        c = r["out"]
        self.find(c, "cp", "decides", "ville", "except on 1 row")
        self.find(c, "cp", "written otherwise", "7501", "Leading zeros")
        self.find(c, "nom", "another case", "MARTIN")
        self.find(c, "date", "written otherwise", "2024-")
        self.find(c, "ht", "point", "comma")
        self.find(c, "ttc = ht + tva", "except on 1 row")
        self.find(c, "tva = ht × 0,2", "every row")
        self.find(c, "qte", "far from all the others", "5000")
        self.find(c, "qte", "negative")
        self.find(c, "2 rows are exact copies")
        self.find(c, "id", "identifies every row but 2 values")
        self.find(c, "fax", "empty on every row")
        self.find(c, "pays", "FR", "every row")
        self.assertEqual(c[0]["sev"], "warn")                     # les défauts d'abord
        self.assertIn("200 rows read", r["sum"])

    def test_show_marks_the_right_rows(self):
        c = self.cards(self.p)["out"]
        dep = self.shown(self.find(c, "decides", "ville")["id"])
        self.assertEqual(dep["rows"], ["13"])
        self.assertEqual(dep["cells"], [["13", "ville", "Paris"]])  # l'attendu en infobulle
        self.assertEqual(dep["shown"], 1)
        self.assertEqual(self.shown(self.find(c, "ttc = ht + tva")["id"])["rows"], ["41"])
        self.assertEqual(self.shown(self.find(c, "written otherwise", "7501")["id"])["rows"], ["55", "67"])
        self.assertEqual(self.shown(self.find(c, "exact copies")["id"])["rows"], ["1", "2"])
        self.assertEqual(self.shown(self.find(c, "far from")["id"])["rows"], ["61"])

    def test_clean_file_says_nothing_stands_out(self):
        r = self.cards(self.q)
        self.assertEqual([x["text"] for x in r["out"] if x["sev"] == "warn"], [])


if __name__ == "__main__":
    unittest.main()
