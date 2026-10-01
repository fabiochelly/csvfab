"""Le serveur vu de l'extérieur : jeton, Origin, page, script, fichiers statiques, routes."""

import os
import unittest

from tests.support import ROOT, Bridge, q, read_version


class HttpTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    # --- ping, sans jeton ------------------------------------------------
    def test_ping_needs_no_token(self):
        r = self.b.get("/api/ping", token=False)
        self.assertEqual(r.status, 200)
        d = r.json()
        self.assertTrue(d["ok"])
        self.assertEqual(d["version"], read_version())
        self.assertEqual(d["pid"], self.b.proc.pid)
        self.assertIn("client", d)           # son sens : voir test_session

    def test_token_file_holds_port_then_token(self):
        with open(os.path.join(self.b.state, "token"), encoding="utf-8") as f:
            port, token = f.read().splitlines()[:2]
        self.assertEqual(int(port), self.b.port)
        self.assertEqual(token, self.b.token)
        if os.name != "nt":
            self.assertEqual(os.stat(os.path.join(self.b.state, "token")).st_mode & 0o777, 0o600)

    # --- jeton et Origin -------------------------------------------------
    def test_api_without_token_is_forbidden(self):
        p = self.b.tmp("a.txt")
        open(p, "w").close()
        for method, route in (("GET", "/api/stat" + q(path=p)), ("GET", "/api/file" + q(path=p)),
                              ("GET", "/api/pending"), ("GET", "/api/fonts"),
                              ("POST", "/api/open"), ("POST", "/api/xlsx"),
                              ("PUT", "/api/file" + q(path=p))):
            with self.subTest(route=route):
                r = self.b.request(method, route, body=b"x" if method != "GET" else None, token=False)
                self.assertEqual(r.status, 403)
        self.assertEqual(os.path.getsize(p), 0)   # le PUT refusé n'a rien écrit

    def test_wrong_token_is_forbidden(self):
        r = self.b.get("/api/pending", headers={"X-Csv-Token": "x" * len(self.b.token)})
        self.assertEqual(r.status, 403)

    def test_token_in_query_string(self):
        r = self.b.get("/api/pending" + q(wait=0, token=self.b.token), token=False)
        self.assertEqual(r.status, 200)

    def test_foreign_origin_is_forbidden_even_with_token(self):
        r = self.b.get("/api/pending", headers={"Origin": "http://evil.example"})
        self.assertEqual(r.status, 403)
        r = self.b.get("/api/pending", headers={"Origin": f"http://127.0.0.1:{self.b.port + 1}"})
        self.assertEqual(r.status, 403)

    def test_own_origins_are_allowed(self):
        for origin in (self.b.origin, f"http://localhost:{self.b.port}"):
            with self.subTest(origin=origin):
                r = self.b.get("/api/pending?wait=0", headers={"Origin": origin})
                self.assertEqual(r.status, 200)

    def test_no_cors_headers(self):
        r = self.b.get("/api/pending?wait=0", headers={"Origin": self.b.origin})
        self.assertFalse(any(k.startswith("access-control-") for k in r.headers))

    # --- routes inconnues ------------------------------------------------
    def test_unknown_routes(self):
        # Le jeton est vérifié avant la route, quelle que soit la méthode : sans
        # lui, on n'apprend même pas quelles routes existent.
        for method in ("GET", "POST", "PUT"):
            with self.subTest(method=method):
                self.assertEqual(self.b.request(method, "/api/nope").status, 404)
                self.assertEqual(self.b.request(method, "/api/nope", token=False).status, 403)
        self.assertEqual(self.b.post("/", token=False).status, 404)
        self.assertEqual(self.b.request("DELETE", "/api/file").status, 501)

    # --- la page ---------------------------------------------------------
    def test_page_carries_the_token(self):
        for route in ("/", "/viewer.htm", "/index.html"):
            with self.subTest(route=route):
                r = self.b.get(route, token=False)
                self.assertEqual(r.status, 200)
                self.assertTrue(r.header("Content-Type").startswith("text/html"))
                self.assertEqual(r.header("Cache-Control"), "no-store")
                self.assertIn(f"window.CSVFAB_TOKEN = '{self.b.token}'", r.text)
                self.assertNotIn("__CSVE_TOKEN__", r.text)
                csp = r.header("Content-Security-Policy")
                for d in ("connect-src 'self'", "form-action 'none'", "frame-ancestors 'none'", "object-src 'none'"):
                    self.assertIn(d, csp)

    # --- /app.js ---------------------------------------------------------
    def test_app_js_joins_ui_js_in_name_order(self):
        r = self.b.get("/app.js", token=False)
        self.assertEqual(r.status, 200)
        self.assertTrue(r.header("Content-Type").startswith("text/javascript"))
        self.assertEqual(r.header("Cache-Control"), "no-cache")
        self.assertTrue(r.text.startswith("//# allFunctionsCalledOnLoad\n"))
        names = sorted(f for f in os.listdir(os.path.join(ROOT, "ui", "js")) if f.endswith(".js"))
        marks = [r.text.index(f"/* ---- ui/js/{n} ---- */") for n in names]
        self.assertEqual(marks, sorted(marks))
        first = os.path.join(ROOT, "ui", "js", names[0])
        with open(first, encoding="utf-8") as f:
            self.assertIn(f.read(), r.text)
        self.assertNotIn(self.b.token, r.text)   # le jeton ne sort que par la page

    def test_app_js_etag_revalidation(self):
        r = self.b.get("/app.js", token=False)
        tag = r.header("ETag")
        self.assertTrue(tag and tag.startswith('"'))
        r2 = self.b.get("/app.js", token=False, headers={"If-None-Match": tag})
        self.assertEqual(r2.status, 304)
        self.assertEqual(r2.body, b"")
        self.assertEqual(r2.header("ETag"), tag)

    # --- fichiers statiques ----------------------------------------------
    def test_static_assets_are_served(self):
        for route, ctype in (("/papaparse.min.js", "text/javascript"), ("/ui/app.css", "text/css"),
                             ("/ui/themes.css", "text/css"), ("/icons/csvfab.svg", "image/svg+xml"),
                             ("/icons/csvfab.ico", "image/x-icon")):
            with self.subTest(route=route):
                r = self.b.get(route, token=False)
                self.assertEqual(r.status, 200)
                self.assertEqual(r.header("Content-Type"), ctype)
                with open(os.path.join(ROOT, *route.lstrip("/").split("/")), "rb") as f:
                    self.assertEqual(r.body, f.read())
                tag = r.header("ETag")
                self.assertEqual(self.b.get(route, token=False, headers={"If-None-Match": tag}).status, 304)

    def test_static_refuses_everything_else(self):
        # Le dossier de l'app contient le code du serveur et, souvent, des CSV.
        for route in ("/server.py", "/csvfab.py", "/LICENSE", "/csvfab.desktop", "/install.sh",
                      "/ui", "/ui/js", "/../server.py", "/ui/../server.py", "/ui/../../etc/passwd",
                      "/%2e%2e/server.py", "/nope.js", "/.git/config"):
            with self.subTest(route=route):
                self.assertEqual(self.b.get(route, token=False).status, 404)

    def test_static_ignores_files_outside_the_app_dir(self):
        # Une copie de l'app dans un dossier temporaire, un .js posé à côté
        # d'elle : rien n'est écrit hors des dossiers de test.
        import shutil
        import tempfile
        outer = tempfile.mkdtemp(prefix="csvfab-test-")
        try:
            app = os.path.join(outer, "app")
            os.makedirs(app)
            for name in ("server.py", "viewer.htm"):
                shutil.copy2(os.path.join(ROOT, name), app)
            shutil.copytree(os.path.join(ROOT, "bridge"), os.path.join(app, "bridge"),
                            ignore=shutil.ignore_patterns("__pycache__"))
            with open(os.path.join(outer, "secret.js"), "w") as f:
                f.write("secret")
            with Bridge(app) as b:
                for route in ("/../secret.js", "/./../secret.js", "/%2e%2e/secret.js"):
                    with self.subTest(route=route):
                        self.assertEqual(b.get(route, token=False).status, 404)
        finally:
            shutil.rmtree(outer, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
