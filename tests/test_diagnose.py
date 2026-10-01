"""« Why does this row look wrong? » : diagnoseRow() (ui/js/37-diagnose.js) sur le corpus, dans la vraie page."""

import unittest

from tests import corpus
from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

DIAG = r"""
window.__dg = async (path, id) => {
  const tab = await __fx.open(path), t = __fx.tab(tab), r = t.allData.find(x => x.id === id);
  const d = diagnoseRow(t, r);
  return { expected: d.expected, observed: d.observed, cause: d.cause || null,
           kinds: d.findings.map(f => f.kind), titles: d.findings.map(f => f.title),
           near: d.findings.map(f => f.near && f.near.hex.filter(x => x.hit).map(x => x.h).join(' ')),
           repair: d.repair && { kind: d.repair.kind, fields: d.repair.fields } };
};
window.__fix = async (path, id) => {
  const tab = await __fx.open(path), t = __fx.tab(tab), r = t.allData.find(x => x.id === id);
  setSel(t, t.filteredData.indexOf(r), 0, t.filteredData.indexOf(r), 0); toggleRowCard(true);
  rowCard.key = ''; rowCardSync(true); applyRepair();
  return { rows: t.allData.length, data: t.allData.find(x => x.id === id).data, irregular: t.allData.filter(x => x.len !== t.headers.length).length };
};
true
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class DiagnoseTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.paths = dict(zip(corpus.NAMES, corpus.write(cls.b.tmp("corpus"))))
        cls.chrome = Chrome(cls.b.origin + "/")
        try:
            cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
            cls.chrome.eval(HELPER)
            cls.chrome.eval(DIAG)
        except Exception:
            cls.chrome.close()
            cls.b.stop()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def dg(self, name, row):
        return self.chrome.eval(f"__dg({self.paths[name]!r}, {row})")

    def test_clean_row_has_no_problem(self):
        d = self.dg("unquoted_delimiter.csv", 1)
        self.assertEqual((d["expected"], d["observed"], d["kinds"]), (5, 5, []))

    def test_unquoted_delimiter_is_located(self):
        d = self.dg("unquoted_delimiter.csv", 3)
        self.assertEqual((d["expected"], d["observed"]), (5, 6))
        self.assertIn("extra-fields", d["kinds"])
        self.assertIn("adresse", d["cause"])
        self.assertEqual(d["near"][d["kinds"].index("extra-fields")], "3B")        # le « ; » fautif
        self.assertEqual(d["repair"], {"kind": "merge", "fields": ["3", "Cy", "7; rue Haute", "Lyon", "69002"]})

    def test_split_line_is_joined(self):
        d = self.dg("unquoted_newline.csv", 2)
        self.assertEqual((d["expected"], d["observed"], d["kinds"][0]), (3, 2, "split-line"))
        self.assertEqual(d["repair"]["fields"], ["2", "5 av. Foch\nbat. B", "Paris"])
        fixed = self.chrome.eval(f"__fix({self.paths['unquoted_newline.csv']!r}, 2)")
        self.assertEqual((fixed["rows"], fixed["irregular"]), (3, 0))
        self.assertEqual(fixed["data"], ["2", "5 av. Foch\nbat. B", "Paris"])

    def test_missing_and_extra_fields(self):
        d = self.dg("ragged_rows.csv", 2)                       # « 4;5 » sous « a;b;c »
        self.assertEqual((d["observed"], d["kinds"][0], d["repair"]["fields"]), (2, "missing-fields", ["4", "5", ""]))
        d = self.dg("trailing_delimiter.csv", 1)
        self.assertEqual(d["kinds"], [])                        # même forme que l'en-tête : rien à redire

    def test_quotes(self):
        self.assertIn("unclosed-quote", self.dg("malformed_quote.csv", 1)["kinds"])
        self.assertEqual(self.dg("stray_quote.csv", 1)["kinds"], ["text-after-quote"])   # « "ok"z "w" », sur sa ligne
        self.assertEqual(self.dg("stray_quote.csv", 2)["kinds"], ["quote-inside"])       # « x "quoted" y »
        # « "ok"z » : le champ court jusqu'au guillemet suivant — il n'y en a plus, il avale la fin du fichier.
        self.assertEqual(self.dg("stray_quote.csv", 3)["kinds"], ["runaway-quote", "unclosed-quote"])
        self.assertIn("space-before-quote", self.dg("spaces_around_quotes.csv", 1)["kinds"])
        self.assertIn("multiline", self.dg("quoted_newline.csv", 1)["kinds"])

    def test_characters(self):
        d = self.dg("nul_character.csv", 1)
        self.assertEqual(d["kinds"], ["nul"])
        self.assertEqual(d["near"][0], "00")
        self.assertIn("invisible", self.dg("bom_in_middle.csv", 1)["kinds"])
        self.assertIn("formula", self.dg("formula_injection.csv", 1)["kinds"])
        self.assertEqual(self.dg("semicolon_decimal_comma.csv", 2)["kinds"], [])  # « -0,5 » n'est pas une formule

    def test_encodings(self):
        d = self.dg("mixed_encodings.csv", 1)                    # lu en 1252, mais cette ligne est de l'UTF-8
        self.assertEqual(d["kinds"], ["utf8-line"])
        self.assertEqual(d["near"][0], "C3 A9")                 # « é » en UTF-8, la séquence entière
        self.assertEqual(d["repair"], {"kind": "reencode", "fields": ["Zoé", "Lyon"]})
        fixed = self.chrome.eval(f"__fix({self.paths['mixed_encodings.csv']!r}, 1)")
        self.assertEqual(fixed["data"], ["Zoé", "Lyon"])
        self.assertEqual(self.dg("mixed_encodings.csv", 2)["kinds"], ["cp1252"])
        self.assertIn("replacement", self.dg("utf16le_lone_surrogate.csv", 1)["kinds"])
        self.assertEqual(self.dg("emoji.csv", 2)["kinds"], [])                # le ZWJ de 👨‍👩‍👧‍👦 est normal


if __name__ == "__main__":
    unittest.main()
