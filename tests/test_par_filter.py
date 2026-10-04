"""Filtres en parallèle (ui/js/44-par-filter.js) : mêmes lignes que sur le fil de la page.

Sur un fichier assez gros pour passer par les workers (> PAR_MIN), chaque filtre est
appliqué deux fois dans la même page : par les workers, puis avec les workers coupés
(parUsable rendu faux) ; les deux vues doivent être identiques, ligne pour ligne et
dans le même ordre. Les cas qui piègent un résultat calculé par enregistrement :
cellules modifiées après coup (la réponse gardée vaut encore), lignes triées, ligne
insérée (sans enregistrement), colonne supprimée puis colonne ajoutée (la table des
colonnes change), champs entre guillemets contenant le séparateur, rétrécissement.

Sauté sans Chromium.
"""

import os
import shutil
import tempfile
import unittest

from tests.bench_page import make_crm
from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

CHECK = r"""
window.__par = {
  async both(setup) {
    const t = T();
    let used = false;
    const req = parRequest, cached = parCached;
    parRequest = (...a) => { used = true; return req(...a); };
    parCached = (...a) => { const r = cached(...a); if (r) used = true; return r; };
    try { (0, eval)(setup); applyFilters(); await whenFiltered(); }
    finally { parRequest = req; parCached = cached; }
    const a = T().filteredData.map(r => r.id);
    const keep = parUsable; parUsable = () => false;
    try { T().lastFilter = null; applyFilters(); } finally { parUsable = keep; }
    const b = T().filteredData.map(r => r.id);
    let diff = -1;
    for (let i = 0; i < Math.max(a.length, b.length); i++) if (a[i] !== b[i]) { diff = i; break; }
    return { used, broken: !!(par && par.broken), n: a.length, m: b.length, diff, total: T().allData.length };
  },
  query(q, o = {}) {
    return `T().lastFilter = null; document.getElementById('global-search').value = ${JSON.stringify(q)};
      document.getElementById('use-regex').checked = ${!!o.regex}; document.getElementById('use-slug').checked = ${!!o.slug};
      document.getElementById('use-reverse').checked = ${!!o.reverse}; document.getElementById('use-expr').checked = ${!!o.expr};`;
  },
};
true
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class ParFilterTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="csvfab-par-")
        cls.path = os.path.join(cls.dir, "crm.csv")
        make_crm(cls.path, 90_000)                 # ~19 Mo : au-dessus de PAR_MIN (16 Mo)
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof parRequest === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)
        cls.chrome.eval(CHECK)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()
        shutil.rmtree(cls.dir, ignore_errors=True)

    def setUp(self):
        self.chrome.eval(f"__fx.open({self.path!r})", timeout=60)

    def same(self, setup, workers=True):
        r = self.chrome.eval(f"__par.both({setup!r})", timeout=120)
        self.assertEqual(r["diff"], -1, f"{setup!r}: vues différentes ({r})")
        self.assertEqual(r["n"], r["m"])
        self.assertEqual(r["used"], workers, f"{setup!r}: workers {'non ' if workers else ''}utilisés ({r})")
        self.assertFalse(r["broken"], f"{setup!r}: un worker a échoué — la page a filtré seule ({r})")
        return r

    def q(self, text, **o):
        return self.chrome.eval(f"__par.query({text!r}, {{regex: {str(o.get('regex', False)).lower()}, "
                                f"slug: {str(o.get('slug', False)).lower()}, reverse: {str(o.get('reverse', False)).lower()}, "
                                f"expr: {str(o.get('expr', False)).lower()}}})")

    def test_text_filters(self):
        cases = [self.q("lyon"), self.q("devis urgent"), self.q('"à voir"'), self.q("elodie", slug=True),
                 self.q("ÉLODIE"), self.q("lyon|nice", regex=True), self.q("lyon", reverse=True),
                 self.q("introuvable"), self.q("relance", slug=True, reverse=True)]
        for setup in cases:
            r = self.same(setup)
            self.assertGreater(r["total"], 0)

    def test_column_and_value_filters(self):
        clear = self.q("")
        self.same(clear + "T().colFilters = {8: 'saint'};")
        self.same(clear + "T().colFilters = {8: 'saint', 12: 'client'};")
        self.same(clear + "T().colFilters = {19: '; '};")                       # le séparateur dans un champ entre guillemets
        self.same(self.q("", regex=True) + "T().colFilters = {1: '^[ÉE]l'};")
        self.same(clear + "T().colFilters = {}; T().valFilters = {12: new Set(['prospect', 'client'])};")
        self.same(self.q("lyon") + "T().valFilters = {12: new Set(['prospect'])};")
        self.chrome.eval("T().valFilters = {}; T().colFilters = {}; true")

    def test_answer_kept_through_edits_sort_insert(self):
        self.same(self.q("lyon"))
        # Des villes changées après coup : la réponse gardée pour « lyon » sert encore.
        self.same("const t = T(); setCells(t, t.allData.slice(0, 3000).filter((_, i) => i % 7 === 0).map(r => [r, 8, 'Lyon']), 'test');"
                  "setCells(t, t.allData.filter(r => cellOf(r, 8) === 'Lyon').slice(0, 50).map(r => [r, 8, 'Paris']), 'test');")
        self.same("sortBy(2, 1);")
        self.same("insertRow(T().allData[10].id, true); const t = T(); setCells(t, [[t.allData[11], 8, 'Lyon']], 'test');")

    def test_columns_remapped(self):
        self.same(self.q("lyon"))
        self.same("const t = T(); remapRows(t, t.headers.map((_, i) => i).filter(i => i !== 3)); t.headers.splice(3, 1);"
                  "t.colFilters = {}; t.modificationsLog.push({ what: 'test', undo() { } });", workers=False)
        # Les lignes n'ont plus bougé depuis la vue : la requête suivante va aux workers, avec la nouvelle table.
        self.same(self.q("devis") + "T().colFilters = {7: 'lyon'};")
        self.same("const t = T(), order = t.headers.map((_, i) => i); order.splice(2, 0, -1); remapRows(t, order);"
                  "t.headers.splice(2, 0, 'Nouvelle'); setCells(t, [[t.allData[5], 2, 'cible']], 'test');", workers=False)
        self.same(self.q("") + "T().colFilters = {2: 'cible'};")

    def test_narrowing_and_expression(self):
        self.same(self.q("réun"))
        self.same("document.getElementById('global-search').value = 'réunion';")   # rétrécit, mais pas à peu de lignes : workers
        self.same("document.getElementById('global-search').value = 'réunion urgent décideur';")
        self.same(self.q('{Montant}.num() > 40000 && {Ville}.contains("lyon")', expr=True), workers=False)


if __name__ == "__main__":
    unittest.main()
