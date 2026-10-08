"""Remplissage par l'exemple (ui/js/48-fill-example.js), éprouvé dans la vraie page.

Des règles trouvées à partir d'un ou deux exemples tapés : un mot remis en casse,
une adresse e-mail bâtie de deux colonnes sans accents, une date retournée, des
initiales, des chiffres seuls ; une colonne déjà en partie remplie qui se complète
comme le reste ; l'offre qui vient seule au deuxième exemple tapé, ses fantômes
dans les cellules vides, Tab qui écrit (une seule annulation qui rend les octets),
Échap qui oublie, une modification ailleurs qui retire l'offre.

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

CLIENTS = ("civilite;prenom;nom;client;naissance;tel;ville;cible\r\n"
           "M.;Jean;DUPONT;M. Jean DUPONT;1984-03-15;06 12 34 56 78;Paris;\r\n"
           "Mme;Élodie;Martin;Mme Élodie Martin;1990-11-02;07 98 76 54 32;Lyon;\r\n"
           "M.;Jean-Pierre;LE GALL;M. Jean-Pierre LE GALL;1975-07-30;01 23 45 67 89;Brest;\r\n"
           "Mme;Anne;ROUX;Mme Anne ROUX;2001-01-09;06 00 00 00 01;Nantes;\r\n"
           "M.;Paul;Petit;M. Paul Petit;1968-12-24;;Paris;\r\n")


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class FillByExampleTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p = cls.b.tmp("clients.csv")
        with open(cls.p, "w", encoding="utf-8", newline="") as f:
            f.write(CLIENTS)
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof ffSynth === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def rule(self, examples, col=7):
        """Les valeurs que la règle trouvée donne pour chaque ligne (exemples = {ligne: valeur})."""
        return self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({self.p!r}), t = __fx.tab(id);
            const ex = Object.entries({examples!r}).map(([i, v]) => ({{ r: t.allData[+i], out: v }}));
            const prog = ffSynth(t, {col}, ex);
            return prog ? t.allData.map(r => ffRun(prog, r)) : null;
        }})()""")

    def test_word_in_another_case(self):
        self.assertEqual(self.rule({0: "Dupont", 1: "Martin"}, 7),
                         ["Dupont", "Martin", "Le Gall", "Roux", "Petit"])

    def test_email_from_two_columns(self):
        self.assertEqual(self.rule({0: "jean.dupont@acme.fr", 1: "elodie.martin@acme.fr"}),
                         ["jean.dupont@acme.fr", "elodie.martin@acme.fr", "jean-pierre.le gall@acme.fr",
                          "anne.roux@acme.fr", "paul.petit@acme.fr"])

    def test_date_turned_around(self):
        self.assertEqual(self.rule({0: "15/03/1984"}),
                         ["15/03/1984", "02/11/1990", "30/07/1975", "09/01/2001", "24/12/1968"])

    def test_initials_and_digits(self):
        self.assertEqual(self.rule({0: "J. DUPONT", 1: "É. MARTIN"}),
                         ["J. DUPONT", "É. MARTIN", "J. LE GALL", "A. ROUX", "P. PETIT"])
        self.assertEqual(self.rule({0: "0612345678", 1: "0798765432"}),
                         ["0612345678", "0798765432", "0123456789", "0600000001", None])

    def test_no_rule_for_unrelated_values(self):
        self.assertIsNone(self.rule({0: "rouge", 1: "vert"}))

    def test_offer_ghosts_tab_and_undo(self):
        r = self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({self.p!r}), t = __fx.tab(id);
            const type = (i, v) => {{
                setSel(t, i, 7, i, 7); render();
                startEdit(tbody.querySelector(`.row[data-idx="${{i}}"] .cell[data-c="7"]`), v);
                document.querySelector('textarea.cell-editor').dispatchEvent(new KeyboardEvent('keydown', {{ key: 'Enter', bubbles: true }}));
            }};
            type(0, 'Dupont');
            await new Promise(r => setTimeout(r, 50));
            const afterOne = !!ffx;
            type(1, 'Martin');
            for (let i = 0; i < 50 && !(ffx && ffx.n != null); i++) await new Promise(r => setTimeout(r, 20));
            const ghosts = [...tbody.querySelectorAll('.cell[data-c="7"] .ffg')].map(e => e.textContent);
            const bar = document.getElementById('ff-bar').textContent;
            const n = ffx && ffx.n;
            document.body.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'Tab', bubbles: true }}));
            const filled = t.allData.map(r => cellOf(r, 7));
            const log = t.modificationsLog.length;
            undo();
            const undone = t.allData.map(r => cellOf(r, 7));
            return {{ afterOne, ghosts, bar, n, filled, log, undone, gone: !ffx && !document.getElementById('ff-bar').classList.contains('on') }};
        }})()""", timeout=30)
        self.assertFalse(r["afterOne"])                            # un seul exemple : rien d'offert
        self.assertEqual(r["ghosts"], ["Le Gall", "Roux", "Petit"])
        self.assertIn("Fill cible by example", r["bar"])
        self.assertIn("Capitalised", r["bar"])                    # la règle dite : nom, en casse de titre
        self.assertEqual(r["n"], 3)
        self.assertEqual(r["filled"], ["Dupont", "Martin", "Le Gall", "Roux", "Petit"])
        self.assertEqual(r["log"], 3)                              # deux saisies + un remplissage
        self.assertEqual(r["undone"], ["Dupont", "Martin", "", "", ""])
        self.assertTrue(r["gone"])

    def test_escape_and_other_edit_drop_the_offer(self):
        r = self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({self.p!r}), t = __fx.tab(id);
            setCells(t, [[t.allData[0], 7, 'DUPONT']], 'set');
            setSel(t, 0, 7, 0, 7);
            document.body.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'e', ctrlKey: true, bubbles: true }}));
            const offered = !!ffx && ffx.prog.length;
            document.body.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'Escape', bubbles: true }}));
            const afterEsc = !!ffx;
            document.body.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'e', ctrlKey: true, bubbles: true }}));
            const again = !!ffx;
            setCells(t, [[t.allData[3], 0, 'Dr']], 'set');
            return {{ offered, afterEsc, again, afterEdit: !!ffx, ghostsLeft: tbody.querySelectorAll('.ffg').length }};
        }})()""", timeout=30)
        self.assertTrue(r["offered"])
        self.assertFalse(r["afterEsc"])
        self.assertTrue(r["again"])                               # Ctrl+E sur la colonne remplie en partie
        self.assertFalse(r["afterEdit"])
        self.assertEqual(r["ghostsLeft"], 0)

    def test_ctrl_e_with_typed_examples(self):
        r = self.chrome.eval(f"""(async () => {{
            const id = await __fx.open({self.p!r}), t = __fx.tab(id);
            setCells(t, [[t.allData[0], 7, 'J.D.'], [t.allData[1], 7, 'É.M.']], 'set');
            setSel(t, 0, 7, 0, 7);
            document.body.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'e', ctrlKey: true, bubbles: true }}));
            for (let i = 0; i < 50 && !(ffx && ffx.n != null); i++) await new Promise(r => setTimeout(r, 20));
            ffAccept();
            return t.allData.map(r => cellOf(r, 7));
        }})()""", timeout=30)
        self.assertEqual(r, ["J.D.", "É.M.", "J.L.", "A.R.", "P.P."])


if __name__ == "__main__":
    unittest.main()
