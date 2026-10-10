"""Aides de lecture de la grille, 2026-10-10.

Puces des filtres actifs (55-filter-chips.js), figer jusqu'à une colonne (07-…, freezeTo),
aperçu d'une cellule tronquée (56-cell-peek.js), valeurs riches — liens, e-mails, oui/non,
couleurs, JSON (51-grid-look.js, richCols) —, échelle de couleur d'une colonne (06-…, toggleHeat).

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

COULEURS = ["#e74c3c", "#3498db", "#2ecc71", "#f1c40f", "#9b59b6"]


def riche():
    rows = ["nom;site;email;actif;couleur;meta;montant;remarque"]
    for i in range(1, 61):
        coul = COULEURS[i % 5] if i != 7 else "rouge"      # une valeur qui n'est pas une couleur
        rows.append(f'Client {i};https://exemple{i}.fr/p;contact{i}@exemple.fr;{"oui" if i % 3 else "non"};{coul};'
                    f'"{{""id"": {i}, ""tags"": [""a""]}}";{i * 10 - 100};'
                    f"Une remarque assez longue pour être coupée par l'ellipse de sa cellule, numéro {i}")
    return "\n".join(rows) + "\n"


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class ReadingAidsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p = cls.b.tmp("riche.csv")
        with open(cls.p, "w", encoding="utf-8", newline="") as f:
            f.write(riche())
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1300,800",))
        cls.chrome.wait_for("typeof richCols === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def open(self):
        # Les colonnes riches sont lues au repos, après le premier rendu.
        self.chrome.eval(f"""(async () => {{ await __fx.open({self.p!r});
          for (let k = 0; k < 200 && !(T().rich && T().rich.map.size); k++) await new Promise(r => setTimeout(r, 20)); render(); }})()""", timeout=60)

    def test_rich_columns(self):
        self.open()
        r = self.chrome.eval("""(() => {
          const t = T(), m = richCols(t), cell = (i, c) => tbody.querySelector(`.row[data-idx="${i}"] .cell[data-c="${c}"]`);
          return { kinds: Object.fromEntries([...m].map(([c, v]) => [c, v.k])),
                   icons: [...thead.querySelectorAll('tr:first-child .ty-r b')].map(x => x.textContent),
                   link: cell(0, 1).classList.contains('lk'), mail: cell(0, 2).classList.contains('lk'),
                   yes: [cell(0, 3).className.includes('cbx on'), cell(0, 3).textContent], no: [cell(2, 3).classList.contains('cbx'), cell(2, 3).classList.contains('on')],
                   sw: [cell(0, 4).classList.contains('sw'), cell(0, 4).style.getPropertyValue('--sw')],
                   notColour: [cell(6, 4).classList.contains('sw'), cell(6, 4).style.getPropertyValue('--sw')],
                   json: cell(0, 5).classList.contains('js'), pills: [...gridLook(t).cat.keys()] };
        })()""")
        self.assertEqual(r["kinds"], {"1": "url", "2": "email", "3": "bool", "4": "color", "5": "json"})
        self.assertEqual(r["icons"], ["↗", "@", "✓", "◐", "{}"])
        self.assertTrue(r["link"]); self.assertTrue(r["mail"])
        self.assertEqual(r["yes"], [True, ""])                    # une case, sans texte : le fond de la cellule
        self.assertEqual(r["no"], [True, False])
        self.assertEqual(r["sw"], [True, "#3498db"])
        self.assertEqual(r["notColour"], [False, ""])             # « rouge » : du texte, pas de style venu des données
        self.assertTrue(r["json"])
        self.assertNotIn(3, r["pills"])                           # oui/non : des cases, pas des pastilles

    def test_ctrl_click_opens_a_link(self):
        self.open()
        r = self.chrome.eval("""(() => {
          const opened = [], keep = window.open; window.open = (u, w, f) => { opened.push([u, w, f]); return null; };
          const cell = tbody.querySelector('.row[data-idx="0"] .cell[data-c="1"]');
          cell.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
          cell.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, ctrlKey: true }));
          window.open = keep;
          return opened;
        })()""")
        self.assertEqual(r, [["https://exemple1.fr/p", "_blank", "noopener,noreferrer"]])   # le clic seul n'ouvre rien

    def test_peek_shows_the_whole_value(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const pause = ms => new Promise(r => setTimeout(r, ms)), box = document.getElementById('cell-peek');
          const over = el => { const b = el.getBoundingClientRect(); el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true, clientX: b.left + 5, clientY: b.top + 5 })); };
          const t = T(); t.colWidths[7] = 120; t.colWidths[5] = 90; applyColStyles(); render(); await pause(50);
          const long = tbody.querySelector('.row[data-idx="1"] .cell[data-c="7"]');
          over(long); await pause(100); const early = box.classList.contains('open');
          await pause(500); const text = box.classList.contains('open') ? box.textContent : null;
          tbody.dispatchEvent(new MouseEvent('mouseleave')); await pause(20);
          over(tbody.querySelector('.row[data-idx="1"] .cell[data-c="5"]')); await pause(600); const json = box.textContent;
          document.dispatchEvent(new MouseEvent('mousedown')); const gone = !box.classList.contains('open');
          over(tbody.querySelector('.row[data-idx="1"] .cell[data-c="0"]')); await pause(600); const short = box.classList.contains('open');
          delete t.colWidths[7]; delete t.colWidths[5]; render();
          return { early, text, json, gone, short };
        })()""")
        self.assertFalse(r["early"])                              # pas à chaque cellule traversée
        self.assertEqual(r["text"], "Une remarque assez longue pour être coupée par l'ellipse de sa cellule, numéro 2")
        self.assertEqual(r["json"], '{\n  "id": 2,\n  "tags": [\n    "a"\n  ]\n}')   # indenté
        self.assertTrue(r["gone"])
        self.assertFalse(r["short"])                              # une valeur entière à l'écran : rien

    def test_filter_chips(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const pause = ms => new Promise(r => setTimeout(r, ms)), t = T(), n = t.allData.length;
          const chips = () => [...document.querySelectorAll('#fchips .fchip')].map(x => x.querySelector('.fc-l').textContent);
          const before = chips();
          document.getElementById('global-search').value = 'client'; t.colFilters[0] = '1'; t.valFilters[3] = new Set(['non']);
          renderHeader(); applyFilters(); await pause(30);
          const three = chips(), all = !!document.querySelector('#fchips .fchip-all'), rows3 = t.filteredData.length;
          const el = document.querySelector('#fchips .fchip'); document.getElementById('global-search').value = 'clien'; applyFilters(); await pause(20);
          const kept = document.querySelector('#fchips .fchip') === el && el.textContent.includes('clien');   // typed: the same chip, its text changed (no pop-in again)
          document.getElementById('global-search').value = 'client'; applyFilters(); await pause(20);
          document.querySelectorAll('#fchips .fc-x')[2].click(); await pause(30);
          const two = chips(), rows2 = t.filteredData.length;
          document.querySelector('#fchips .fchip-all').click(); await pause(30);
          return { before, three, all, kept, two, rows: [rows3, rows2, t.filteredData.length, n], after: chips(), search: document.getElementById('global-search').value };
        })()""")
        self.assertEqual(r["before"], [])
        self.assertEqual(r["three"], ["Search client", "nom 1", "actif ≠ non"])
        self.assertTrue(r["all"]); self.assertTrue(r["kept"])
        self.assertEqual(r["two"], ["Search client", "nom 1"])   # le × retire ce filtre-là
        self.assertLess(r["rows"][0], r["rows"][1])
        self.assertEqual(r["rows"][2], r["rows"][3])             # Clear all : toutes les lignes
        self.assertEqual(r["after"], []); self.assertEqual(r["search"], "")

    def test_freeze_up_to_a_column(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const pause = ms => new Promise(r => setTimeout(r, ms)), t = T();
          freezeTo(2); await pause(30);
          const frz = [...tbody.querySelectorAll('.row[data-idx="0"] .cell.frz')].map(x => +x.dataset.c);
          container.scrollLeft = 600; renderOnScroll(); await pause(50);
          const c2 = tbody.querySelector('.row[data-idx="0"] .cell[data-c="2"]').getBoundingClientRect().left;
          const th2 = thead.querySelector('th[data-col="2"]').getBoundingClientRect().left;
          const out = { F: t.frozen, frz, stuck: Math.abs(c2 - th2) < 1 };
          freezeTo(2); out.off = t.frozen;
          t.colWidths[0] = 900; applyColStyles(); render(); freezeTo(1); out.refused = t.frozen; delete t.colWidths[0];
          container.scrollLeft = 0; render();
          return out;
        })()""")
        self.assertEqual(r["F"], 3)
        self.assertEqual(r["frz"], [0, 1, 2])
        self.assertTrue(r["stuck"])                               # la cellule et son titre restent ensemble au défilement
        self.assertEqual(r["off"], 0)                             # le même choix défige
        self.assertEqual(r["refused"], 0)                         # trop large : refusé

    def test_colour_scale(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const t = T(), bg = (i) => tbody.querySelector(`.row[data-idx="${i}"] .cell[data-c="6"]`).style.backgroundColor;
          toggleHeat(6); await new Promise(r => setTimeout(r, 30));
          const neg = heatStyle(t, 6, '-90'), zero = heatStyle(t, 6, '0'), top = heatStyle(t, 6, '500'), drawn = bg(0);
          const ask = uiConfirm; uiConfirm = async () => true; await deleteColumn(0); uiConfirm = ask;
          const moved = Object.keys(t.heat);
          undo(); const back = Object.keys(t.heat);
          toggleHeat(6); const off = tbody.querySelector('.row[data-idx="0"] .cell[data-c="6"]').style.backgroundColor;
          return { neg, zero, top, drawn, moved, back, off };
        })()""")
        self.assertIn("danger", r["neg"])                         # les négatifs dans la couleur du danger
        self.assertTrue(r["drawn"])                               # la cellule dessinée a sa teinte
        self.assertIn("accent", r["top"])
        self.assertIn(" 6%", r["zero"])                           # zéro : la teinte la plus faible
        self.assertIn(" 46%", r["top"])                           # le 98e centile et au-delà : la plus forte
        self.assertEqual(r["moved"], ["5"]); self.assertEqual(r["back"], ["6"])   # suit sa colonne, et l'annulation
        self.assertEqual(r["off"], "")


if __name__ == "__main__":
    unittest.main()
