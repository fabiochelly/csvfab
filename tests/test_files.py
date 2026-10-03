"""Lecture, stat et écriture atomique des fichiers par chemin (les onglets t.path)."""

import os
import re
import socket
import time
import unittest

from tests.support import Bridge, q


class FilesTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    def write(self, name, data):
        p = self.b.tmp(name)
        with open(p, "wb") as f:
            f.write(data)
        return p

    def leftovers(self, path):
        d = os.path.dirname(path)
        return sorted(f for f in os.listdir(d) if f.endswith(".part"))

    # --- GET /api/file ---------------------------------------------------
    def test_read_returns_bytes_unchanged(self):
        data = b"\xef\xbb\xbfa;b\r\n\xe9t\xe9;\x00\xff\r\n"   # BOM, CRLF, octets non UTF-8
        p = self.write("raw.csv", data)
        r = self.b.get("/api/file" + q(path=p))
        self.assertEqual(r.status, 200)
        self.assertEqual(r.body, data)
        self.assertEqual(r.header("X-File-Size"), str(len(data)))
        self.assertEqual(r.header("Content-Length"), str(len(data)))
        self.assertEqual(r.header("Cache-Control"), "no-store")

    def test_read_from_offset(self):
        p = self.write("log.txt", b"line1\nline2\n")
        r = self.b.get("/api/file" + q(path=p, **{"from": 6}))
        self.assertEqual(r.body, b"line2\n")
        self.assertEqual(r.header("X-File-Size"), "12")
        r = self.b.get("/api/file" + q(path=p, **{"from": 99}))     # au-delà : vide, taille donnée
        self.assertEqual((r.status, r.body, r.header("X-File-Size")), (200, b"", "12"))
        r = self.b.get("/api/file" + q(path=p, **{"from": "x"}))    # illisible : depuis le début
        self.assertEqual(r.body, b"line1\nline2\n")
        r = self.b.get("/api/file" + q(path=p, **{"from": -5}))
        self.assertEqual(r.body, b"line1\nline2\n")

    def test_head_sends_headers_only(self):
        p = self.write("h.csv", b"abc")
        r = self.b.request("HEAD", "/api/file" + q(path=p))
        self.assertEqual((r.status, r.body, r.header("X-File-Size")), (200, b"", "3"))

    def test_read_missing_or_directory(self):
        self.assertEqual(self.b.get("/api/file" + q(path=self.b.tmp("none.csv"))).status, 404)
        self.assertEqual(self.b.get("/api/file" + q(path=self.b.tmp())).status, 404)
        self.assertEqual(self.b.get("/api/file").status, 404)

    def test_read_expands_user_and_resolves_links(self):
        p = self.write("tilde.csv", b"x")
        rel = "~/" + os.path.relpath(p, self.b.home).replace(os.sep, "/")
        self.assertEqual(self.b.get("/api/file" + q(path=rel)).body, b"x")

    # --- GET /api/stat ---------------------------------------------------
    def test_stat(self):
        p = self.write("s.csv", b"12345")
        os.utime(p, (1_700_000_000, 1_700_000_000.5))
        d = self.b.get("/api/stat" + q(path=p)).json()
        self.assertEqual(d, {"name": "s.csv", "path": os.path.realpath(p), "size": 5,
                             "mtime": 1_700_000_000.5, "mtime_ns": "1700000000500000000", "writable": True})
        self.assertEqual(self.b.get("/api/stat" + q(path=self.b.tmp("none"))).status, 404)

    # --- PUT /api/file ---------------------------------------------------
    def test_write_creates_and_overwrites(self):
        p = self.b.tmp("new.csv")
        r = self.b.put("/api/file" + q(path=p), b"a;b\n")
        self.assertEqual(r.status, 200)
        self.assertEqual(r.json(), {"ok": True, "bytes": 4, "backup": None, "size": 4})
        r = self.b.put("/api/file" + q(path=p), b"x")
        self.assertEqual(r.json()["size"], 1)
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"x")
        self.assertEqual(self.leftovers(p), [])

    def test_write_empty_body(self):
        p = self.write("empty.csv", b"old")
        r = self.b.put("/api/file" + q(path=p), b"")
        self.assertEqual(r.json()["size"], 0)
        self.assertEqual(os.path.getsize(p), 0)

    def test_write_with_backup(self):
        p = self.write("b.csv", b"old content")
        r = self.b.put("/api/file" + q(path=p, backup=1), b"new")
        bak = r.json()["backup"]
        self.assertRegex(bak, r"^b\.csv\.\d{8}-\d{6}\.bak$")
        with open(os.path.join(os.path.dirname(p), bak), "rb") as f:
            self.assertEqual(f.read(), b"old content")
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"new")

    def test_backup_of_a_missing_file_is_nothing(self):
        p = self.b.tmp("fresh.csv")
        self.assertIsNone(self.b.put("/api/file" + q(path=p, backup=1), b"x").json()["backup"])

    def test_write_errors(self):
        r = self.b.put("/api/file", b"x")
        self.assertEqual(r.status, 400)
        r = self.b.put("/api/file" + q(path=self.b.tmp("nodir", "x.csv")), b"x")
        self.assertEqual(r.status, 400)
        self.assertIn("dossier inexistant", r.json()["error"])

    def test_interrupted_write_keeps_the_original(self):
        # Le cœur de la garantie : un envoi coupé ne tronque jamais le fichier.
        p = self.write("keep.csv", b"precious data")
        path = "/api/file" + q(path=p)
        s = socket.create_connection(("127.0.0.1", self.b.port))
        s.sendall((f"PUT {path} HTTP/1.1\r\nHost: x\r\nX-Csv-Token: {self.b.token}\r\n"
                   "Content-Length: 100000\r\n\r\n").encode() + b"partial")
        time.sleep(0.2)
        s.close()
        deadline = time.time() + 5
        while self.leftovers(p) and time.time() < deadline:
            time.sleep(0.05)
        time.sleep(0.1)
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"precious data")
        self.assertEqual(self.leftovers(p), [])

    def test_write_preserves_bytes(self):
        data = bytes(range(256)) * 4 + b"\r\n"
        p = self.b.tmp("bin.csv")
        self.b.put("/api/file" + q(path=p), data)
        self.assertEqual(self.b.get("/api/file" + q(path=p)).body, data)

    def test_large_write(self):
        data = (b"x" * 1023 + b"\n") * 5 * 1024     # 5 Mo : plusieurs tranches de 1 Mo
        p = self.b.tmp("big.csv")
        r = self.b.put("/api/file" + q(path=p), data)
        self.assertEqual(r.json()["size"], len(data))
        self.assertEqual(self.b.get("/api/file" + q(path=p, **{"from": len(data) - 4})).body, b"xxx\n")


if __name__ == "__main__":
    unittest.main()
