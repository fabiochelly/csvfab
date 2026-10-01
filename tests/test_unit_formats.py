"""bridge.formats sans serveur : typage des colonnes, écriture et lecture xlsx, SQLite."""

import csv
import io
import os
import shutil
import sqlite3
import tempfile
import unittest
import zipfile

from bridge.formats import sqlite_io, xlsx_reader, xlsx_writer
from bridge.formats.common import number_text
from bridge.formats.xlsx_columns import ColumnStat, excel_value, number_xml


def decide(values):
    st = ColumnStat()
    for v in values:
        st.see(v)
    st.decide()
    return st.kind, st.fmt


class ColumnStatTest(unittest.TestCase):
    def test_numbers(self):
        self.assertEqual(decide(["1 234,50", "-5,25", "7"]), ("fr", "#,##0.00"))
        self.assertEqual(decide(["1,234.5", "2.25", "3"]), ("en", "#,##0.00"))
        # Des entiers nus : les deux conventions les lisent, l'anglaise l'emporte à égalité ;
        # format Général (pas de séparateur de milliers sur un identifiant).
        self.assertEqual(decide(["1", "2", "30"]), ("en", None))

    def test_codes_stay_text(self):
        self.assertEqual(decide(["01000", "75001", "13002"]), ("text", None))
        self.assertEqual(decide(["1234567890123456", "1"]), ("text", None))
        self.assertEqual(decide(["06 12 34 56 78", "07 00 00 00 00"]), ("text", None))

    def test_threshold(self):
        # 9 nombres sur 10 : numérique ; 8 sur 10 : texte.
        self.assertEqual(decide(["1,5"] * 9 + ["x"])[0], "fr")
        self.assertEqual(decide(["1,5"] * 8 + ["x", "y"])[0], "text")
        self.assertEqual(decide(["", " ", "1,5"])[0], "fr")            # les vides ne comptent pas

    def test_dates(self):
        self.assertEqual(decide(["15/06/2023", "01/01/2024"]), ("dmy", "dd/mm/yyyy"))
        self.assertEqual(decide(["06/15/2023", "01/01/2024"]), ("mdy", "mm/dd/yyyy"))
        self.assertEqual(decide(["2023-06-15", "2024-01-01"]), ("ymd", "yyyy-mm-dd"))
        self.assertEqual(decide(["2023-06-15 10:30", "2024-01-01"]), ("ymd", "yyyy-mm-dd hh:mm"))
        self.assertEqual(decide(["2023-06-15T10:30:15Z"]), ("ymd", "yyyy-mm-dd hh:mm:ss"))

    def test_percent(self):
        self.assertEqual(decide(["12%", "7,5 %"]), ("pct", "0.0%"))
        self.assertEqual(decide(["12%", "100%"]), ("pct", "0%"))

    def test_excel_value(self):
        self.assertEqual(excel_value("fr", "1 234,5"), 1234.5)
        self.assertEqual(excel_value("fr", "1 234,5"), 1234.5)
        self.assertEqual(excel_value("en", "1,234.5"), 1234.5)
        self.assertIsNone(excel_value("fr", "abc"))
        self.assertEqual(excel_value("pct", "12,5%"), 0.125)
        self.assertEqual(excel_value("dmy", "15/06/2023"), 45092)
        self.assertEqual(excel_value("mdy", "06/15/2023"), 45092)
        self.assertEqual(excel_value("dmy", "15/06/23"), 45092)         # années à deux chiffres : 20xx
        self.assertEqual(excel_value("ymd", "2023-06-15T12:00:00"), 45092.5)
        self.assertIsNone(excel_value("dmy", "31/02/2023"))             # date impossible : laissée en texte

    def test_number_xml(self):
        self.assertEqual(number_xml(3.0), "3")
        self.assertEqual(number_xml(-0.5), "-0.5")
        self.assertEqual(number_xml(1e16), "1e+16")


class NumberTextTest(unittest.TestCase):
    def test_number_text(self):
        self.assertEqual(number_text("1234.0", False), "1234")
        self.assertEqual(number_text("1234.5", True), "1234,5")
        self.assertEqual(number_text("0.1", False), "0.1")
        self.assertEqual(number_text("abc", True), "abc")
        self.assertEqual(number_text("1e20", False), "1e+20")


class XlsxTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="csvfab-unit-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def roundtrip(self, rows, delim=";", header=True, sheet="Feuille"):
        path = os.path.join(self.dir, "t.xlsx")
        info = xlsx_writer.write(lambda: iter(rows), path, sheet, header)
        out = io.StringIO()
        back = xlsx_reader.to_csv(path, out, delim)
        return info, back, list(csv.reader(io.StringIO(out.getvalue()), delimiter=delim)), path

    def test_text_survives(self):
        rows = [["Nom", "Note <b>&"], ["a\tb", "x\x01y"], ["ligne\nsuivante", "é ü 漢字 🎉"], ["", "seule"]]
        info, back, got, _ = self.roundtrip(rows)
        self.assertEqual(info["rows"], 4)
        self.assertEqual(got, [["Nom", "Note <b>&"], ["a\tb", "xy"],          # contrôles illégaux en XML : retirés
                               ["ligne\nsuivante", "é ü 漢字 🎉"], ["", "seule"]])

    def test_sheet_name_is_sanitised(self):
        _, back, _, path = self.roundtrip([["a"], ["1"]], sheet="a/b:c*?[x]" + "z" * 40)
        self.assertEqual(len(back["sheet"]), 31)
        self.assertNotRegex(back["sheet"], r"[\[\]:*?/\\]")
        _, back, _, path = self.roundtrip([["a"], ["1"]], sheet="l'été")
        self.assertEqual(back["sheet"], "l'été")
        with zipfile.ZipFile(path) as z:
            self.assertIn("'l''été'!$A$1:$A$2", z.read("xl/workbook.xml").decode())

    def test_wide_sheet(self):
        row = [str(i) for i in range(30)]
        _, back, got, path = self.roundtrip([row, row])
        self.assertEqual((back["cols"], got[1]), (30, row))
        with zipfile.ZipFile(path) as z:
            self.assertIn('<autoFilter ref="A1:AD2"/>', z.read("xl/worksheets/sheet1.xml").decode())

    def test_empty(self):
        info, back, got, _ = self.roundtrip([])
        self.assertEqual((info["rows"], back["rows"], got), (0, 0, []))

    def test_col_index_and_dates(self):
        self.assertEqual([xlsx_reader.col_index(r) for r in ("A1", "Z9", "AA1", "BC12", "XFD1")], [0, 25, 26, 54, 16383])
        self.assertEqual(xlsx_reader.date_text(45092, False), "2023-06-15")
        self.assertEqual(xlsx_reader.date_text(45092.4375, False), "2023-06-15 10:30")
        self.assertEqual(xlsx_reader.date_text(45092 + 37815 / 86400, False), "2023-06-15 10:30:15")
        self.assertEqual(xlsx_reader.date_text(0, True), "1904-01-01")
        self.assertIsNone(xlsx_reader.date_text(1e12, False))


class SqliteTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="csvfab-unit-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_sql_real(self):
        for text, want in (("1 234,50", 1234.5), ("1,234.50", 1234.5), ("1.234,5", 1234.5), ("12,5", 12.5),
                           ("1,234", 1234.0), ("-3", -3.0), ("1 000,5", 1000.5)):
            with self.subTest(text=text):
                self.assertEqual(sqlite_io.sql_real(text), want)
        for text in ("abc", "1,2,3", "1.2.3,4,5", "", "12%"):
            with self.subTest(text=text):
                self.assertIsNone(sqlite_io.sql_real(text))

    def test_write_names_and_ragged_rows(self):
        path = os.path.join(self.dir, "t.db")
        rows = [["a", "A", "", "b"], ["1", "2"], ["3", "4", "5", "6", "7"]]
        info = sqlite_io.write(lambda: iter(rows), path, "t", True)
        self.assertEqual(list(info["types"]), ["a", "A_2", "col3", "b", "col5"])
        con = sqlite3.connect(path)
        try:
            self.assertEqual(con.execute("select * from t").fetchall(),
                             [(1, 2, None, None, None), (3, 4, 5, 6, 7)])
        finally:
            con.close()

    def test_write_replaces_an_existing_file(self):
        path = os.path.join(self.dir, "t.db")
        sqlite_io.write(lambda: iter([["a"], ["1"]]), path, "t", True)
        sqlite_io.write(lambda: iter([["b"], ["x"]]), path, "u", True)
        self.assertEqual([t["name"] for t in sqlite_io.table_list(path)], ["u"])

    def test_identifiers_are_quoted(self):
        path = os.path.join(self.dir, "t.db")
        sqlite_io.write(lambda: iter([['x"; drop table t; --'], ["1"]]), path, 'ta"ble', True)
        out = io.StringIO()
        info = sqlite_io.to_csv(path, out, ",")
        self.assertEqual((info["table"], out.getvalue()), ('ta"ble', '"x""; drop table t; --"\n1\n'))


if __name__ == "__main__":
    unittest.main()
