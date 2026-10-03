"""Tri de texte (textRanks, ui/js/12-bulk-edits.js), éprouvé dans la vraie page.

Les rangs viennent de clés numériques triées par base puis vérifiées par le
collator : ils doivent être exactement ceux d'un tri par le collator seul,
sur chacun des trois chemins (valeurs toutes distinctes, ligne par ligne ;
valeurs répétées, par la Map ; peu de valeurs ou clés inutiles, tri simple)
et sur des valeurs que les clés ne voient pas (accents, ligatures, ß, nombres
longs, zéros en tête, écritures non latines, caractères ignorés, préfixes
communs plus longs que les clés).

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge

CHECK = r"""
(() => {
  // La référence : le tri simple de l'ancienne version, collator seul.
  function reference(col, coll) {
    const rank = new Map();
    for (const v of col) if (v !== null && !rank.has(v)) rank.set(v, 0);
    const uniq = [...rank.keys()].sort(coll.compare);
    let r = 0;
    for (let i = 0; i < uniq.length; i++) { if (i && coll.compare(uniq[i - 1], uniq[i]) !== 0) r++; rank.set(uniq[i], r); }
    return col.map(v => v === null ? NaN : rank.get(v));
  }
  let seed = 12345;
  const rnd = k => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed % k; };
  const TRICKY = ['Dupont', 'Émile', 'émile', 'Zoé', 'zoe', 'Ångström', "O'Brien", 'de la Tour', 'Müller', 'Mueller',
                  'Œuvre', 'oeuvre', 'œuvre', 'straße', 'strasse', 'STRASSE', '', '  x', '-a', '_b', '10', '9', '09', '0',
                  '00', 'item 10', 'item 9', 'Item 010', 'a1b', 'a01b', 'ǅ', 'Ʃ', 'ж', '中', 'æ', 'ae', 'Æsop', 'ß', 'ss',
                  'ı', 'i', 'İ', 'ﬁ', 'fi', 'x́', '­', 'a­b', 'ab', '½', '1/2', '٣', '３', 'Ⅻ', 'è',
                  'è', '\t', '12345678901234567890', '12345678901234567891', '0000000000000000000001'];
  const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789éèàçœ';
  const word = L => { let s = ''; for (let j = 0; j < L; j++) s += A[rnd(A.length)]; return s; };
  const sets = {
    // toutes distinctes : le chemin ligne par ligne
    distinct: Array.from({ length: 30000 }, (_, i) => word(6 + rnd(10)) + i),
    // distinctes, mais pareilles au-delà des clés : les groupes de clés égales
    longPrefix: Array.from({ length: 30000 }, (_, i) => 'client.dupont.martin.' + i + '@' + ['b.fr', 'a.fr', 'c.com'][i % 3]),
    // répétées, nombreuses : la Map et les clés
    repeated: Array.from({ length: 40000 }, () => word(3) + ' ' + rnd(5000)),
    // pièges, répétés : la Map ; mêlés de valeurs vides
    tricky: Array.from({ length: 20000 }, (_, i) => i % 11 ? TRICKY[rnd(TRICKY.length)] + (i % 3 ? '' : ' ' + rnd(100)) + (i % 7 ? '' : TRICKY[rnd(TRICKY.length)]) : null),
    // pièges, tous distincts : le chemin ligne par ligne avec le tri de secours
    trickyDistinct: Array.from({ length: 20000 }, (_, i) => TRICKY[rnd(TRICKY.length)] + TRICKY[rnd(TRICKY.length)] + String.fromCharCode(0x20 + rnd(0x2D0)) + i),
    // préfixe commun plus long que les clés : le tri simple
    urls: Array.from({ length: 20000 }, () => 'https://www.example.com/products/category/item-' + rnd(1e7)),
    // peu de valeurs : le tri simple
    few: Array.from({ length: 20000 }, () => ['Paris', 'Lyon', 'Le Havre', 'Évry', 'evry', null][rnd(6)]),
  };
  const coll = new Intl.Collator('fr', { numeric: true, sensitivity: 'base' });
  const bad = {};
  for (const [name, col] of Object.entries(sets)) {
    const a = reference(col, coll), b = textRanks(col, coll);
    for (let i = 0; i < col.length; i++)
      if (!(a[i] === b[i] || (a[i] !== a[i] && b[i] !== b[i]))) { bad[name] = [col[i], a[i], b[i]]; break; }
  }
  return bad;
})()
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class SortRanksTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof textRanks === 'function' && document.readyState === 'complete'")

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def test_ranks_are_the_collators(self):
        self.assertEqual(self.chrome.eval(CHECK, timeout=120), {})
