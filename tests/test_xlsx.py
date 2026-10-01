"""Excel dans les deux sens : CSV → classeur (/api/xlsx), classeur → CSV (/api/xlsx2csv, /api/xlsx-sheets).

Les classeurs lus sont fabriqués à la main (support.make_workbook) et les
classeurs écrits sont relus avec zipfile + ElementTree, sans le code du serveur.
"""

import io
import json
import os
import unittest
import xml.etree.ElementTree as ET
import zipfile

from tests.support import Bridge, make_workbook, q

NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}

CSV = ("Code;Montant;Date;Pct;Nom;Quand\n"
       "01000;1 234,50;15/06/2023;12%;Dupont;2023-06-15T10:30:00Z\n"
       "02000;-5,25;01/01/2024;7,5%;Lefèvre;2024-01-01\n"
       "03000;7;31/12/2024;100%;\"A;B\";2024-02-29 08:00\n").encode("utf-8")


def cells(xlsx_bytes):
    """{référence: (style, type, valeur)} de la première feuille, et le classeur entier."""
    z = zipfile.ZipFile(io.BytesIO(xlsx_bytes))
    sheet = ET.fromstring(z.read("xl/worksheets/sheet1.xml"))
    out = {}
    for c in sheet.iter("{%s}c" % NS["m"]):
        v = c.find("m:v", NS)
        t = c.find("m:is/m:t", NS)
        out[c.get("r")] = (c.get("s"), c.get("t"), v.text if v is not None else (t.text if t is not None else None))
    return out, z, sheet


def num_formats(z):
    """Code de format de chaque style de cellule (index s → formatCode ou id intégré)."""
    st = ET.fromstring(z.read("xl/styles.xml"))
    custom = {f.get("numFmtId"): f.get("formatCode") for f in st.iterfind("m:numFmts/m:numFmt", NS)}
    return [custom.get(x.get("numFmtId"), x.get("numFmtId")) for x in st.iterfind("m:cellXfs/m:xf", NS)]


class XlsxWriteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    def make(self, body=CSV, **params):
        params.setdefault("delim", ";")
        params.setdefault("header", 1)
        return self.b.post("/api/xlsx" + q(**params), body)

    def test_returns_a_workbook_and_its_summary(self):
        r = self.make()
        self.assertEqual(r.status, 200)
        self.assertEqual(r.header("Content-Type"),
                         "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        self.assertEqual(json.loads(r.header("X-Xlsx-Info")),
                         {"rows": 4, "cols": 6, "truncated_rows": False, "truncated_cols": False})
        z = zipfile.ZipFile(io.BytesIO(r.body))
        self.assertIsNone(z.testzip())
        self.assertEqual(sorted(z.namelist()), sorted([
            "[Content_Types].xml", "_rels/.rels", "xl/workbook.xml", "xl/_rels/workbook.xml.rels",
            "xl/styles.xml", "xl/worksheets/sheet1.xml"]))

    def test_column_typing(self):
        c, z, _ = cells(self.make().body)
        fmt = num_formats(z)
        # Code à zéro initial : texte tel quel.
        self.assertEqual(c["A2"], (None, "inlineStr", "01000"))
        # Montants français → nombres, format à deux décimales.
        self.assertEqual([c[f"B{i}"][2] for i in (2, 3, 4)], ["1234.5", "-5.25", "7"])
        self.assertEqual(fmt[int(c["B2"][0])], "#,##0.00")
        # Dates jj/mm/aaaa → numéros de série, affichées dans l'ordre du fichier.
        self.assertEqual([c[f"C{i}"][2] for i in (2, 3, 4)], ["45092", "45292", "45657"])
        self.assertEqual(fmt[int(c["C2"][0])], "dd/mm/yyyy")
        # Pourcentages, une décimale puisque 7,5 % en a une.
        self.assertEqual([c[f"D{i}"][2] for i in (2, 3, 4)], ["0.12", "0.075", "1"])
        self.assertEqual(fmt[int(c["D2"][0])], "0.0%")
        # Texte, délimiteur cité compris.
        self.assertEqual(c["E4"], (None, "inlineStr", "A;B"))
        # Dates ISO avec heures → date + heure.
        self.assertEqual(c["F2"][2], "45092.4375")
        self.assertEqual(fmt[int(c["F2"][0])], "yyyy-mm-dd hh:mm")

    def test_header_look(self):
        c, z, sheet = cells(self.make().body)
        self.assertEqual(c["A1"], ("1", "inlineStr", "Code"))
        pane = sheet.find("m:sheetViews/m:sheetView/m:pane", NS)
        self.assertEqual((pane.get("ySplit"), pane.get("state")), ("1", "frozen"))
        self.assertEqual(sheet.find("m:autoFilter", NS).get("ref"), "A1:F4")
        wb = ET.fromstring(z.read("xl/workbook.xml"))
        dn = wb.find("m:definedNames/m:definedName", NS)
        self.assertEqual((dn.get("name"), dn.text), ("_xlnm._FilterDatabase", "'Sheet1'!$A$1:$F$4"))
        st = z.read("xl/styles.xml").decode()
        self.assertIn('rgb="FF1E3A8A"', st)
        self.assertIn('<name val="Aptos"/>', st)

    def test_without_header(self):
        c, z, sheet = cells(self.make(header=0).body)
        self.assertIsNone(sheet.find("m:sheetViews", NS))
        self.assertIsNone(sheet.find("m:autoFilter", NS))
        self.assertEqual(c["A1"][0], None)

    def test_long_integers_stay_text(self):
        body = "Id;N\n1234567890123456;1\n2234567890123456;2\n".encode()
        c, _, _ = cells(self.make(body).body)
        self.assertEqual(c["A2"], (None, "inlineStr", "1234567890123456"))
        self.assertEqual(c["B2"], (None, None, "1"))      # entier : format Général

    def test_english_numbers_and_comma_delimiter(self):
        body = b'Amount,Who\n"1,234.50",a\n2.5,b\n'
        c, _, _ = cells(self.make(body, delim=",").body)
        self.assertEqual((c["A2"][2], c["A3"][2]), ("1234.5", "2.5"))

    def test_written_to_a_path(self):
        dest = self.b.tmp("out.xlsx")
        r = self.make(path=dest)
        d = r.json()
        self.assertEqual((d["ok"], d["path"], d["rows"]), (True, os.path.realpath(dest), 4))
        with open(dest, "rb") as f:
            c, z, _ = cells(f.read())
        wb = ET.fromstring(z.read("xl/workbook.xml"))
        self.assertEqual(wb.find("m:sheets/m:sheet", NS).get("name"), "out")   # nom de la feuille = du fichier
        self.assertEqual(sorted(f for f in os.listdir(os.path.dirname(dest)) if f.startswith(".csvfab")), [])

    def test_sheet_name_parameter(self):
        _, z, _ = cells(self.make(sheet="Ventes 2024").body)
        wb = ET.fromstring(z.read("xl/workbook.xml"))
        self.assertEqual(wb.find("m:sheets/m:sheet", NS).get("name"), "Ventes 2024")

    def test_errors(self):
        self.assertEqual(self.make(delim=";;").status, 400)
        r = self.make(path=self.b.tmp("nodir", "x.xlsx"))
        self.assertEqual(r.status, 400)

    def test_round_trip(self):
        dest = self.b.tmp("rt.xlsx")
        self.make(path=dest)
        r = self.b.post("/api/xlsx2csv" + q(src=dest, delim=";"))
        self.assertEqual(r.text.replace("\r\n", "\n"),
                         "Code;Montant;Date;Pct;Nom;Quand\n"
                         "01000;1234,5;2023-06-15;0,12;Dupont;2023-06-15 10:30\n"
                         "02000;-5,25;2024-01-01;0,075;Lefèvre;2024-01-01\n"
                         "03000;7;2024-12-31;1;\"A;B\";2024-02-29 08:00\n")
        self.assertEqual(json.loads(r.header("X-Xlsx-Info")),
                         {"sheet": "rt", "others": 0, "rows": 4, "cols": 6})


# Feuilles du classeur de lecture : une vide (cellules seulement mises en forme),
# deux remplies et visibles, une masquée.
SHARED = ["Nom", "Ville", "Dupont", "Lyon", "Ana &amp; Bo", "Lefèvre"]
DATA = ('<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="inlineStr"><is><t>Né</t></is></c>'
        '<c r="D1" t="inlineStr"><is><t>Montant</t></is></c><c r="E1" t="inlineStr"><is><t>OK</t></is></c>'
        '<c r="F1" t="inlineStr"><is><t>Vu</t></is></c></row>'
        '<row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" t="s"><v>3</v></c><c r="C2" s="1"><v>45092</v></c>'
        '<c r="D2"><v>1234.5</v></c><c r="E2" t="b"><v>1</v></c><c r="F2" s="2"><v>45092.4375</v></c></row>'
        '<row r="3"><c r="A3" s="1"/></row>'                                   # ligne vide : sautée
        '<row r="5"><c r="A5" t="s"><v>4</v></c><c r="D5"><v>0.1</v></c><c r="E5" t="b"><v>0</v></c></row>'
        '<row r="6"><c r="B6" t="s"><v>5</v></c><c r="D6"><v>3</v></c><c r="F6" t="str"><v>formule</v></c></row>')


class XlsxReadTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.book = cls.b.tmp("book.xlsx")
        make_workbook(cls.book, [
            ("Vide", "", '<row r="1"><c r="A1" s="1"/></row>'),
            ("Données", "", DATA),
            ("Cachée", "hidden", '<row r="1"><c r="A1" t="inlineStr"><is><t>x</t></is></c></row>'),
            ("Autre", "", '<row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c></row>'
                          '<row r="2"><c r="A2"><v>2</v></c></row>'),
        ], shared=SHARED)
        cls.single = cls.b.tmp("single.xlsx")
        make_workbook(cls.single, [("Feuil1", "", DATA), ("Feuil2", "", "")], shared=SHARED)

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    EXPECTED = ("Nom;Ville;Né;Montant;OK;Vu\n"
                "Dupont;Lyon;2023-06-15;1234,5;TRUE;2023-06-15 10:30\n"
                "Ana & Bo;;;0,1;FALSE;\n"
                ";Lefèvre;;3;;formule\n")

    def test_sheet_list(self):
        d = self.b.get("/api/xlsx-sheets" + q(path=self.book)).json()
        self.assertEqual([(s["name"], s["hidden"], s["filled"]) for s in d["sheets"]],
                         [("Vide", False, False), ("Données", False, True),
                          ("Cachée", True, True), ("Autre", False, True)])

    def test_sheet_list_errors(self):
        self.assertEqual(self.b.get("/api/xlsx-sheets" + q(path=self.b.tmp("none.xlsx"))).status, 404)
        p = self.b.tmp("notzip.xlsx")
        with open(p, "w") as f:
            f.write("a;b\n")
        self.assertEqual(self.b.get("/api/xlsx-sheets" + q(path=p)).status, 400)

    def test_pick_asks_when_several_sheets_have_data(self):
        r = self.b.post("/api/xlsx2csv" + q(src=self.book, delim=";", pick=1))
        self.assertEqual([s["name"] for s in r.json()["choose"]], ["Vide", "Données", "Cachée", "Autre"])

    def test_pick_converts_when_only_one_sheet_has_data(self):
        r = self.b.post("/api/xlsx2csv" + q(src=self.single, delim=";", pick=1))
        self.assertEqual(r.header("Content-Type"), "text/csv; charset=utf-8")
        self.assertEqual(r.text.replace("\r\n", "\n"), self.EXPECTED)
        self.assertEqual(json.loads(r.header("X-Xlsx-Info"))["sheet"], "Feuil1")

    def test_default_sheet_is_the_first_visible_with_data(self):
        r = self.b.post("/api/xlsx2csv" + q(src=self.book, delim=";"))
        self.assertEqual(r.text.replace("\r\n", "\n"), self.EXPECTED)
        self.assertEqual(json.loads(r.header("X-Xlsx-Info")),
                         {"sheet": "Données", "others": 1, "rows": 4, "cols": 6})

    def test_named_sheet_and_comma_delimiter(self):
        r = self.b.post("/api/xlsx2csv" + q(src=self.book, delim=",", sheet="Autre"))
        self.assertEqual(r.text.replace("\r\n", "\n"), "a\n2\n")
        r = self.b.post("/api/xlsx2csv" + q(src=self.book, delim=",", sheet="Données"))
        self.assertIn("Dupont,Lyon,2023-06-15,1234.5,TRUE", r.text)       # point décimal hors « ; »

    def test_uploaded_body(self):
        with open(self.book, "rb") as f:
            body = f.read()
        r = self.b.post("/api/xlsx2csv" + q(delim=";", pick=1), body)
        self.assertIn("choose", r.json())
        r = self.b.post("/api/xlsx2csv" + q(delim=";", sheet="Données"), body)
        self.assertEqual(r.text.replace("\r\n", "\n"), self.EXPECTED)
        self.assertEqual(sorted(f for f in os.listdir(self.b.state) if f.startswith(".csvfab")), [])

    def test_written_beside_with_backup(self):
        dest = self.b.tmp("book - Données.csv")
        with open(dest, "w", encoding="utf-8") as f:
            f.write("old\n")
        r = self.b.post("/api/xlsx2csv" + q(src=self.book, dest=dest, delim=";", sheet="Données", backup=1))
        d = r.json()
        self.assertEqual((d["ok"], d["path"], d["sheet"]), (True, os.path.realpath(dest), "Données"))
        with open(dest, encoding="utf-8", newline="") as f:
            self.assertEqual(f.read().replace("\r\n", "\n"), self.EXPECTED)
        self.assertEqual(d["size"], os.path.getsize(dest))
        baks = [f for f in os.listdir(os.path.dirname(dest)) if f.startswith("book - Données.csv.") and f.endswith(".bak")]
        self.assertEqual(len(baks), 1)
        with open(os.path.join(os.path.dirname(dest), baks[0]), encoding="utf-8") as f:
            self.assertEqual(f.read(), "old\n")

    def test_date1904(self):
        p = self.b.tmp("mac.xlsx")
        make_workbook(p, [("S", "", '<row r="1"><c r="A1" s="1"><v>0</v></c></row>')], date1904=True)
        self.assertEqual(self.b.post("/api/xlsx2csv" + q(src=p, delim=";")).text.strip(), "1904-01-01")

    def test_errors(self):
        p = self.b.tmp("bad.xlsx")
        with open(p, "w") as f:
            f.write("a;b\n")
        r = self.b.post("/api/xlsx2csv" + q(src=p, delim=";"))
        self.assertEqual(r.status, 400)
        self.assertIn("pas un classeur xlsx lisible", r.json()["error"])
        self.assertEqual(self.b.post("/api/xlsx2csv" + q(src=self.b.tmp("none.xlsx"), delim=";")).status, 404)
        self.assertEqual(self.b.post("/api/xlsx2csv" + q(src=self.book, delim="ab")).status, 400)
        r = self.b.post("/api/xlsx2csv" + q(src=self.book, delim=";", dest=self.b.tmp("nodir", "x.csv")))
        self.assertEqual(r.status, 400)


if __name__ == "__main__":
    unittest.main()
