"""SQLite dans les deux sens : base → CSV (/api/sqlite-tables, /api/sqlite2csv), CSV → base (/api/sqlite)."""

import json
import os
import sqlite3
import unittest

from tests.support import Bridge, q


class SqliteReadTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.db = cls.b.tmp("base.sqlite")
        c = sqlite3.connect(cls.db)
        c.executescript("""
            create table clients(id integer, nom text, montant real, photo blob, note text);
            insert into clients values (1, 'Dupont', 1234.5, x'00ff', null),
                                       (2, 'Lefèvre;x', -2.0, null, 'ok');
            create table vide(a);
            create view v as select id, nom from clients;
        """)
        c.commit()
        c.close()
        cls.one = cls.b.tmp("one.db")
        c = sqlite3.connect(cls.one)
        c.executescript("create table t(a, b); insert into t values ('x', 1.0); create table e(z);")
        c.commit()
        c.close()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    def test_table_list(self):
        d = self.b.get("/api/sqlite-tables" + q(path=self.db)).json()
        # Sans COUNT(*) : le compte d'une table vient de max(rowid) (approx), une vue n'en a pas.
        self.assertEqual(d["sheets"], [
            {"name": "clients", "view": False, "hidden": False, "filled": True, "dim": [2, 5], "approx": True},
            {"name": "vide", "view": False, "hidden": False, "filled": False, "dim": [0, 1], "approx": False},
            {"name": "v", "view": True, "hidden": False, "filled": True, "dim": None, "approx": False}])

    def test_pick_asks_when_several_tables_have_rows(self):
        r = self.b.post("/api/sqlite2csv" + q(src=self.db, delim=";", pick=1))
        self.assertEqual([s["name"] for s in r.json()["choose"]], ["clients", "vide", "v"])

    def test_pick_converts_when_only_one_table_has_rows(self):
        r = self.b.post("/api/sqlite2csv" + q(src=self.one, delim=";", pick=1))
        self.assertEqual(r.text.replace("\r\n", "\n"), "a;b\nx;1\n")

    def test_table_to_csv(self):
        r = self.b.post("/api/sqlite2csv" + q(src=self.db, delim=";", sheet="clients"))
        self.assertEqual(r.header("Content-Type"), "text/csv; charset=utf-8")
        # NULL vide, BLOB en hexadécimal, virgule décimale dans un fichier « ; ».
        self.assertEqual(r.text.replace("\r\n", "\n"),
                         "id;nom;montant;photo;note\n1;Dupont;1234,5;0x00ff;\n2;\"Lefèvre;x\";-2;;ok\n")
        self.assertEqual(json.loads(r.header("X-Xlsx-Info")),
                         {"table": "clients", "others": 1, "rows": 2, "cols": 5})

    def test_view_with_comma(self):
        r = self.b.post("/api/sqlite2csv" + q(src=self.db, delim=",", sheet="v"))
        self.assertEqual(r.text.replace("\r\n", "\n"), "id,nom\n1,Dupont\n2,Lefèvre;x\n")

    def test_written_beside(self):
        dest = self.b.tmp("base - clients.csv")
        d = self.b.post("/api/sqlite2csv" + q(src=self.db, dest=dest, delim=";", sheet="clients")).json()
        self.assertEqual((d["ok"], d["path"], d["table"]), (True, os.path.realpath(dest), "clients"))
        with open(dest, encoding="utf-8") as f:
            self.assertTrue(f.read().startswith("id;nom;"))

    def test_uploaded_body(self):
        with open(self.db, "rb") as f:
            body = f.read()
        r = self.b.post("/api/sqlite2csv" + q(delim=";", sheet="clients"), body)
        self.assertTrue(r.text.startswith("id;nom;montant"))

    def test_database_is_not_modified(self):
        before = os.stat(self.db).st_mtime_ns, os.path.getsize(self.db)
        self.b.post("/api/sqlite2csv" + q(src=self.db, delim=";", sheet="clients"))
        self.assertEqual(before, (os.stat(self.db).st_mtime_ns, os.path.getsize(self.db)))
        self.assertFalse(os.path.exists(self.db + "-journal") or os.path.exists(self.db + "-wal"))

    def test_not_a_database(self):
        p = self.b.tmp("x.db")
        with open(p, "w") as f:
            f.write("a;b\n")
        r = self.b.post("/api/sqlite2csv" + q(src=p, delim=";"))
        self.assertEqual(r.status, 400)
        self.assertIn("SQLite", r.json()["error"])
        self.assertEqual(self.b.get("/api/sqlite-tables" + q(path=p)).status, 400)
        self.assertEqual(self.b.get("/api/sqlite-tables" + q(path=self.b.tmp("none.db"))).status, 404)


class SqliteWriteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    def test_column_types(self):
        body = ("Code;Qte;Prix;Nom;Vide\n"
                "01000;3;1 234,5;Dupont;\n"
                "02000;-4;2,25;Lefèvre;\n"
                "12345678901234567;5;3;x;\n").encode()
        dest = self.b.tmp("out.sqlite")
        d = self.b.post("/api/sqlite" + q(delim=";", header=1, path=dest, sheet="ventes"), body).json()
        self.assertEqual(d, {"rows": 3, "cols": 5, "table": "ventes", "ok": True, "path": os.path.realpath(dest),
                             "types": {"Code": "TEXT", "Qte": "INTEGER", "Prix": "REAL", "Nom": "TEXT", "Vide": "TEXT"}})
        c = sqlite3.connect(dest)
        try:
            rows = c.execute("select Code, Qte, Prix, Nom, Vide, typeof(Code), typeof(Prix), typeof(Vide) from ventes").fetchall()
        finally:
            c.close()
        # Un zéro en tête ou plus de 15 chiffres : un code, gardé en texte (01000 était devenu 1000.0).
        self.assertEqual(rows, [("01000", 3, 1234.5, "Dupont", None, "text", "real", "null"),
                                ("02000", -4, 2.25, "Lefèvre", None, "text", "real", "null"),
                                ("12345678901234567", 5, 3.0, "x", None, "text", "real", "null")])

    def test_english_numbers(self):
        body = b'n,m\n"1,234.5",1.5\n2,2\n'
        dest = self.b.tmp("en.sqlite")
        self.b.post("/api/sqlite" + q(delim=",", header=1, path=dest, sheet="t"), body)
        c = sqlite3.connect(dest)
        try:
            self.assertEqual(c.execute("select n, m from t").fetchall(), [(1234.5, 1.5), (2.0, 2.0)])
        finally:
            c.close()

    def test_returned_as_bytes(self):
        r = self.b.post("/api/sqlite" + q(delim=";", header=1, sheet="t"), b"a\n1\n")
        self.assertEqual(r.header("Content-Type"), "application/vnd.sqlite3")
        self.assertTrue(r.body.startswith(b"SQLite format 3\x00"))
        self.assertEqual(json.loads(r.header("X-Xlsx-Info"))["types"], {"a": "INTEGER"})

    def test_round_trip(self):
        body = "Code;Montant;Nom\n007;1,5;Zoé\n008;-2;\"a;b\"\n".encode()
        dest = self.b.tmp("rt.sqlite")
        self.b.post("/api/sqlite" + q(delim=";", header=1, path=dest, sheet="rt"), body)
        r = self.b.post("/api/sqlite2csv" + q(src=dest, delim=";"))
        self.assertEqual(r.text.replace("\r\n", "\n"), "Code;Montant;Nom\n007;1,5;Zoé\n008;-2;\"a;b\"\n")


if __name__ == "__main__":
    unittest.main()
