"""/api/fonts : la liste des polices à chasse fixe et le catalogue (sans téléchargement)."""

import unittest

from tests.support import Bridge, q


class FontsTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    def test_shape(self):
        d = self.b.get("/api/fonts").json()
        self.assertEqual(sorted(d), ["catalog", "mono"])
        self.assertIsInstance(d["mono"], list)          # vide sans fontconfig
        self.assertTrue(all(isinstance(f, str) for f in d["mono"]))
        ids = [f["id"] for f in d["catalog"]]
        self.assertEqual(len(ids), len(set(ids)))
        for want in ("cascadia-code", "jetbrains-mono"):
            self.assertIn(want, ids)
        for f in d["catalog"]:
            self.assertTrue({"id", "name", "family", "license", "installed"} <= set(f), f)
            self.assertIsInstance(f["installed"], bool)

    def test_install_unknown_id_fails_without_network(self):
        r = self.b.post("/api/font-install" + q(id="no-such-font"))
        self.assertEqual(r.status, 502)
        self.assertIn("error", r.json())


if __name__ == "__main__":
    unittest.main()
