"""La lecture de la grille et le clic droit (ui/js/51-grid-look.js, 52-motion-links.js, 53-context-menu.js).

Pastilles de catégorie aux couleurs du ruban (un fond de cellule, pas un élément), ligne de séparation entre groupes d'une vue triée (décidée au
repos), ligne active teintée, filtre en mouvement, valeurs d'une colonne allumées au survol
de son graphique, menu du clic droit sur une cellule (garder ou cacher sa valeur) et routage
du clic droit vers le panneau de colonne et le menu de ligne, le menu du navigateur gardé
dans les champs de saisie.

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

STATUTS = ["actif", "vip", "prospect", "inactif", "perdu", "suspendu"]


def ventes():
    lines = ["id;statut;montant;note"]
    for i in range(1, 121):
        m = ["3", "12,5", "1 250,00", "47,10"][i % 4]
        lines.append(f"{i};{STATUTS[(i * 7) % 6] if i % 3 else 'actif'};{m};texte libre {i}")
    return "\n".join(lines) + "\n"


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class GridLookTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p = cls.b.tmp("ventes.csv")
        with open(cls.p, "w", encoding="utf-8", newline="") as f:
            f.write(ventes())
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1300,900",))
        cls.chrome.wait_for("typeof gridLook === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def open(self):
        self.chrome.eval(f"(async () => {{ await __fx.open({self.p!r}); }})()", timeout=60)
        # Les graphiques comptés : les pastilles prennent leurs couleurs.
        self.chrome.eval("""(async () => { for (let k = 0; k < 300 && !(T().spk && spkShown(T())); k++) await new Promise(r => setTimeout(r, 20)); render(); })()""")

    def test_pills_follow_the_ribbon(self):
        self.open()
        r = self.chrome.eval("""(() => {
          const t = T(), L = gridLook(t), cells = [...tbody.querySelectorAll('.row[data-idx] .cell[data-c="1"]')].slice(0, 6);
          return { cols: [...L.cat.keys()], order: [...L.cat.get(1)], segs: t.spk.cols[1].segs.map(x => x[0]),
                   pills: cells.map(c => [c.textContent, c.classList.contains('pc') ? [...c.classList].find(x => /^p[0-4o]$/.test(x)) : null,
                                          c.style.getPropertyValue('--pw'), pillWidth(c.textContent.trim()) + 'px']),
                   spans: tbody.querySelectorAll('.cell[data-c="1"] span').length,
                   note: !!tbody.querySelector('.cell[data-c="3"].pc') };
        })()""")
        self.assertEqual(r["cols"], [1])                        # statut seulement (la note est libre)
        self.assertEqual([v for v, _ in r["order"]], r["segs"])  # l'ordre du ruban
        self.assertEqual([c for _, c in r["order"]][:2], ["p0", "p1"])
        for text, cls, w, want in r["pills"]:
            self.assertTrue(cls, text)                           # le fond de la cellule, dans la couleur de sa valeur
            self.assertEqual(w, want)                            # aussi large que son texte
        self.assertEqual(r["spans"], 0)                          # pas d'élément en plus : la pastille est un fond
        self.assertFalse(r["note"])

    def test_groups_of_a_sorted_view(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const t = T(); sortBy(1, 1);
          for (let k = 0; k < 100 && !(t.groupLook && t.groupLook.rows === t.filteredData); k++) await new Promise(r => setTimeout(r, 20));
          const lines = [...tbody.querySelectorAll('.row.gs')].map(e => cellOf(t.filteredData[+e.dataset.idx], 1));
          container.scrollTop = 20 * ROW_H; renderOnScroll(); await new Promise(r => setTimeout(r, 50));
          const tag = document.getElementById('grp-tag');
          const out = { on: t.groupLook.on, lines, tag: tag && tag.style.display !== 'none' ? tag.textContent : null,
                        top: cellOf(t.filteredData[Math.floor((container.scrollTop - thead.offsetHeight) / ROW_H) + 1], 1) };
          undo();
          return out;
        })()""")
        self.assertTrue(r["on"])
        self.assertTrue(r["lines"])                              # une ligne à chaque changement de statut
        self.assertEqual(len(r["lines"]), len(set(r["lines"])))
        self.assertEqual(r["tag"], r["top"])                     # l'étiquette dit le groupe en haut de la vue

    def test_active_row(self):
        self.open()
        r = self.chrome.eval("""(() => {
          const t = T(); setSel(t, 3, 1, 3, 1);
          const rows = [...tbody.querySelectorAll('.row.xr')].map(e => +e.dataset.idx);
          setSel(t, 5, 2, 5, 2);
          return [rows, [...tbody.querySelectorAll('.row.xr')].map(e => +e.dataset.idx), tbody.querySelectorAll('.cell.xc').length];
        })()""")
        self.assertEqual(r, [[3], [5], 0])                      # la ligne seulement, plus la colonne

    def test_filter_moves_the_rows(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          document.getAnimations().forEach(a => a.cancel());
          document.getElementById('global-search').value = 'vip'; applyFilters();
          await new Promise(r => requestAnimationFrame(r));
          const rows = document.getAnimations().filter(a => a.effect && a.effect.target && a.effect.target.classList.contains('row')).length;
          document.getElementById('global-search').value = ''; applyFilters();
          return rows;
        })()""")
        self.assertGreater(r, 5)

    def test_chart_hover_lights_its_column_only(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const t = T(), c = t.spk.cols[1];
          glowOn(t, 1, { seg: 0, x0: 0, w: .3 });
          const lit = [...tbody.querySelectorAll('.cell.lit')];
          const out = { n: lit.length, cols: [...new Set(lit.map(e => e.dataset.c))], all: lit.every(e => e.textContent.trim() === c.segs[0][0]),
                        dim: tbody.classList.contains('glowing') };
          await new Promise(r => setTimeout(r, 100));
          out.ticks = document.querySelector('#scroll-strip .ss-glow').width > 0;
          glowOff();
          out.after = tbody.querySelectorAll('.cell.lit').length;
          return out;
        })()""")
        self.assertGreater(r["n"], 0)
        self.assertEqual(r["cols"], ["1"])
        self.assertTrue(r["all"])
        self.assertFalse(r["dim"])                               # plus de lignes atténuées
        self.assertTrue(r["ticks"])
        self.assertEqual(r["after"], 0)

    def test_right_click(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const t = T(), at = (el, x = 5, y = 5) => { const b = el.getBoundingClientRect(); const ev = new MouseEvent('contextmenu', { clientX: b.left + x, clientY: b.top + y, bubbles: true, cancelable: true }); el.dispatchEvent(ev); return ev.defaultPrevented; };
          const out = {};
          const td = tbody.querySelector('.row[data-idx="2"] .cell[data-c="1"]'), v = td.textContent.trim();
          out.cellPrevented = at(td, 20, 10);
          out.cellMenu = document.getElementById('cell-menu').classList.contains('open');
          out.items = [...document.querySelectorAll('#cell-menu .dd-item .lbl')].map(e => e.textContent);
          out.sel = [sel.fr, sel.fc];
          closeDDs();
          cellKeepValue(1, true);
          out.kept = [...new Set(t.filteredData.map(r => cellOf(r, 1)))];
          t.valFilters = {}; renderHeader(); applyColStyles(); applyFilters();
          at(thead.querySelector('th[data-col="2"] .col-name'));
          out.colItems = [...document.querySelectorAll('#cell-menu .dd-item .lbl')].map(e => e.textContent);
          out.colPanel = !!colPanel;
          closeDDs();
          at(tbody.querySelector('.row[data-idx="4"] .col-idx'));
          out.rowMenu = document.getElementById('row-menu').classList.contains('open') && document.getElementById('row-menu').dataset.id === String(t.filteredData[4].id);
          closeDDs();
          const g = document.getElementById('global-search');
          g.value = 'vip'; applyFilters();
          out.fieldPrevented = at(g);
          out.fieldItems = [...document.querySelectorAll('#cell-menu .dd-item')].map(e => e.textContent.replace(/Ctrl.*$/, '').trim() + (e.classList.contains('disabled') ? ' (off)' : ''));
          const filtered = t.filteredData.length;
          [...document.querySelectorAll('#cell-menu .dd-item')].find(e => e.textContent.includes('Clear the filter')).click();
          out.cleared = [g.value, filtered < t.allData.length];
          await new Promise(r => setTimeout(r, 300));
          out.afterClear = t.filteredData.length === t.allData.length;
          at(thead.querySelector('.f-box[data-col="1"]'));
          out.colFilterField = document.activeElement && document.activeElement.matches('input.f-in[data-col="1"]') && document.getElementById('cell-menu').classList.contains('open');
          closeDDs();
          out.v = v;
          return out;
        })()""")
        self.assertTrue(r["cellPrevented"])
        self.assertTrue(r["cellMenu"])
        self.assertIn("Copy", r["items"])
        self.assertIn("Paste", r["items"])
        self.assertTrue(any(x.startswith("Only the rows with") for x in r["items"]))
        self.assertEqual(r["sel"], [2, 1])
        self.assertEqual(r["kept"], [r["v"]])
        self.assertEqual(r["colItems"], ["Split the column…", "Merge columns…", "Insert a column before",
                                         "Insert a column after", "Colour the cells by value", "Freeze up to this column", "Delete the column"])
        self.assertFalse(r["colPanel"])                          # pas le panneau entier du ▾
        self.assertTrue(r["rowMenu"])
        self.assertTrue(r["fieldPrevented"])                     # plus de menu du navigateur, même dans un champ
        self.assertEqual(r["fieldItems"], ["Undo", "Cut (off)", "Copy (off)", "Paste", "Select all", "Clear the filter"])
        self.assertEqual(r["cleared"], ["", True])
        self.assertTrue(r["afterClear"])                         # l'effacement passe par l'événement input : le filtre suit
        self.assertTrue(r["colFilterField"])                     # le filtre d'une colonne devient un champ, avec le menu

    # La barre de filtres masquable : l'en-tête et l'étendue rétrécissent d'autant ; la masquer vide
    # ses champs (la règle de l'utilisateur : un filtre invisible cacherait pourquoi des lignes
    # manquent), dans l'onglet affiché comme dans les autres ; le choix est retenu.
    def test_filter_row_hides(self):
        self.open()
        r = self.chrome.eval("""(async () => {
          const pause = ms => new Promise(r => setTimeout(r, ms)), t = T(), n = t.allData.length;
          t.colFilters[1] = 'vip'; applyFilters(); renderHeader(); await pause(50);
          const h0 = thead.offsetHeight, s0 = container.scrollHeight, shown = t.filteredData.length;
          const other = { colFilters: { 0: 'x' }, refilter: false }; tabs.push(other);
          toggleFilterRow(); await pause(50);
          tabs.splice(tabs.indexOf(other), 1);
          const out = { dh: h0 - thead.offsetHeight, rows: [shown, t.filteredData.length, n], filters: Object.keys(t.colFilters).length,
                        other: [Object.keys(other.colFilters).length, other.refilter],
                        kept: localStorage.getItem('csvfab-filter-row'), pill: document.querySelector('.sb-filt').classList.contains('off') };
          toggleFilterRow(); await pause(50);
          out.back = thead.offsetHeight === h0; out.box = thead.querySelector('.filter-row .f-box[data-col="1"]').textContent;
          return out;
        })()""")
        self.assertEqual(r["dh"], 29)                            # la ligne de filtres, 28 px + sa bordure
        self.assertLess(r["rows"][0], r["rows"][2])
        self.assertEqual(r["rows"][1], r["rows"][2])             # le filtre vidé : toutes les lignes reviennent
        self.assertEqual(r["filters"], 0)
        self.assertEqual(r["other"], [0, True])                  # l'autre onglet aussi, refiltré à son retour
        self.assertEqual(r["kept"], "0"); self.assertTrue(r["pill"])
        self.assertTrue(r["back"]); self.assertEqual(r["box"], "Filter")

if __name__ == "__main__":
    unittest.main()
