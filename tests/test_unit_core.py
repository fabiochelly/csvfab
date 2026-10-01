"""bridge.session, bridge.fsio, bridge.config et la plomberie de bridge.httpd, sans sous-processus."""

import http.client
import io
import json
import os
import shutil
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

from bridge import config, fsio, httpd, session as session_mod
from bridge.app import make_server
from bridge.session import Session


class FakeClock:
    def __init__(self, t=1000.0):
        self.t = t

    def __call__(self):
        return self.t


class SessionTest(unittest.TestCase):
    def test_boot_without_any_window(self):
        clock = FakeClock()
        s = Session(clock)
        self.assertFalse(s.alive())
        clock.t += session_mod.IDLE_BOOT - 1
        self.assertFalse(s.should_stop())
        clock.t += 2
        self.assertTrue(s.should_stop())

    def test_after_a_window(self):
        clock = FakeClock()
        s = Session(clock)
        self.assertEqual(s.wait(0, lambda: False), [])
        self.assertTrue(s.alive())
        clock.t += session_mod.POLL_ALIVE + 0.1
        self.assertFalse(s.alive())
        self.assertFalse(s.should_stop())
        clock.t += session_mod.IDLE_LIVE
        self.assertTrue(s.should_stop())

    def test_a_held_poll_keeps_the_server(self):
        clock = FakeClock()
        s = Session(clock)
        got = {}
        th = threading.Thread(target=lambda: got.update(paths=s.wait(60, lambda: False)))
        th.start()
        while not s.polling():
            time.sleep(0.01)
        clock.t += 10_000
        self.assertTrue(s.alive())
        self.assertFalse(s.should_stop())
        s.push(["/a", "/b", "/a"])
        th.join(5)
        self.assertEqual(got["paths"], ["/a", "/b"])
        self.assertEqual(s.polling(), 0)

    def test_closed_window_leaves_the_queue(self):
        s = Session()
        stop = threading.Event()
        got = {}
        th = threading.Thread(target=lambda: got.update(paths=s.wait(5, stop.is_set)))
        th.start()
        time.sleep(0.05)
        stop.set()
        th.join(2)
        self.assertIsNone(got["paths"])
        s.push(["/x"])
        self.assertEqual(s.wait(0, lambda: False), ["/x"])

    def test_hand_over(self):
        clock = FakeClock()
        s = Session(clock)
        s.wait(0, lambda: False)
        self.assertTrue(s.alive())
        s.hand_over(["/f"])
        self.assertFalse(s.alive())             # le lanceur ouvrira une fenêtre
        self.assertEqual(s.wait(0, lambda: False), ["/f"])


class FsioTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="csvfab-unit-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def path(self, name, data=None):
        p = os.path.join(self.dir, name)
        if data is not None:
            with open(p, "wb") as f:
                f.write(data)
        return p

    def test_receive(self):
        out = io.BytesIO()
        fsio.receive(io.BytesIO(b"x" * 3_000_000 + b"tail"), 3_000_000, out)
        self.assertEqual(len(out.getvalue()), 3_000_000)
        fsio.receive(io.BytesIO(b""), 0, out)
        with self.assertRaises(IOError):
            fsio.receive(io.BytesIO(b"short"), 10, io.BytesIO())

    def test_replacing_success_and_failure(self):
        p = self.path("a.csv", b"old")
        with fsio.replacing(p) as tmp:
            with open(tmp, "wb") as f:
                f.write(b"new")
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"new")
        with self.assertRaises(RuntimeError):
            with fsio.replacing(p) as tmp:
                with open(tmp, "wb") as f:
                    f.write(b"half")
                raise RuntimeError("coupure")
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"new")
        self.assertEqual(os.listdir(self.dir), ["a.csv"])

    @unittest.skipUnless(os.path.isdir("/proc/self/fd"), "fsync de dossier : POSIX ; le test lit /proc (Linux)")
    def test_durable_replace_syncs_the_directory(self):
        p = self.path("d.csv", b"old")
        synced = []
        real = os.fsync
        with mock.patch("os.fsync", side_effect=lambda fd: synced.append(os.path.realpath(f"/proc/self/fd/{fd}")) or real(fd)):
            with fsio.replacing(p, durable=True) as tmp:
                with open(tmp, "wb") as f:
                    f.write(b"new")
            with fsio.replacing(p) as tmp:
                with open(tmp, "wb") as f:
                    f.write(b"newer")
        self.assertEqual(synced, [os.path.realpath(self.dir)])            # une fois, le dossier, et pas sans durable

    @unittest.skipIf(os.name == "nt", "permissions POSIX")
    def test_replacing_keeps_permissions(self):
        p = self.path("secret.csv", b"x")
        os.chmod(p, 0o600)
        with fsio.replacing(p) as tmp:
            open(tmp, "wb").close()
        self.assertEqual(os.stat(p).st_mode & 0o777, 0o600)

    def test_temp_names_differ_between_threads(self):
        names = set()
        ths = [threading.Thread(target=lambda: names.add(fsio.temp_name("/d/f.csv"))) for _ in range(4)]
        for t in ths:
            t.start()
        for t in ths:
            t.join()
        names.add(fsio.temp_name("/d/f.csv"))
        self.assertGreaterEqual(len(names), 2)
        self.assertTrue(all(os.path.basename(n).startswith(".f.csv.") for n in names))

    def test_backups_never_overwrite_each_other(self):
        p = self.path("c.csv", b"v1")
        a = fsio.backup(p)
        with open(p, "wb") as f:
            f.write(b"v2")
        b = fsio.backup(p)                                   # même seconde, très probablement
        self.assertNotEqual(a, b)
        with open(a, "rb") as f1, open(b, "rb") as f2:
            self.assertEqual((f1.read(), f2.read()), (b"v1", b"v2"))
        if os.path.basename(a)[:-4] == os.path.basename(b)[:-6]:
            self.assertTrue(b.endswith("-2.bak"))

    def test_backup_and_scratch(self):
        self.assertIsNone(fsio.backup(self.path("none.csv")))
        p = self.path("b.csv", b"v1")
        made = fsio.backup(p)
        self.assertRegex(os.path.basename(made), r"^b\.csv\.\d{8}-\d{6}\.bak$")
        with open(made, "rb") as f:
            self.assertEqual(f.read(), b"v1")
        with fsio.scratch(self.dir, ".tmp") as s:
            open(s, "w").close()
        self.assertFalse(os.path.exists(s))


class ConfigTest(unittest.TestCase):
    def test_env_new_name_first(self):
        with mock.patch.dict(os.environ, {"CSVFAB_X": "new", "CSV_EDITOR_X": "old"}):
            self.assertEqual(config.env("X"), "new")
        with mock.patch.dict(os.environ, {"CSV_EDITOR_X": "old"}):
            os.environ.pop("CSVFAB_X", None)
            self.assertEqual(config.env("X"), "old")
        self.assertEqual(config.env("NO_SUCH_SETTING_XYZ", "d"), "d")

    def test_state_dir_per_os(self):
        env = {"XDG_STATE_HOME": "/x/state", "LOCALAPPDATA": "C:\\Users\\u\\AppData\\Local"}
        with mock.patch.dict(os.environ, env):
            with mock.patch.multiple(config, WINDOWS=False, MACOS=False):
                self.assertEqual(config.state_base(), "/x/state")
            with mock.patch.multiple(config, WINDOWS=True, MACOS=False):
                self.assertEqual(config.state_base(), env["LOCALAPPDATA"])
            with mock.patch.multiple(config, WINDOWS=False, MACOS=True):
                self.assertTrue(config.state_base().endswith(os.path.join("Library", "Application Support")))

    def test_old_state_folder_until_migrated(self):
        base = tempfile.mkdtemp(prefix="csvfab-unit-")
        try:
            with mock.patch.object(config, "state_base", lambda: base):
                self.assertEqual(config.state_dir(), os.path.join(base, "csvfab"))
                os.mkdir(os.path.join(base, "csv-editor"))
                self.assertEqual(config.state_dir(), os.path.join(base, "csv-editor"))
                os.mkdir(os.path.join(base, "csvfab"))
                self.assertEqual(config.state_dir(), os.path.join(base, "csvfab"))
        finally:
            shutil.rmtree(base)


class HttpdTest(unittest.TestCase):
    """Le serveur dans ce processus, sur un port libre, avec des routes de test."""

    @classmethod
    def setUpClass(cls):
        cls.state = tempfile.mkdtemp(prefix="csvfab-unit-")
        cls.srv = make_server(port=0, state=cls.state)
        cls.port = cls.srv.server_address[1]
        cls.th = threading.Thread(target=cls.srv.serve_forever, daemon=True)
        cls.th.start()

        def boom(req):
            raise KeyError("imprévu")

        def text_error(req):
            raise httpd.HttpError(418, "théière", text=True)

        def echo(req):
            return {"a": req.arg("a"), "missing": req.arg("zz", "dflt"), "path": req.path_arg()}
        cls.extra = {("GET", "/api/boom"): boom, ("GET", "/api/tea"): text_error, ("GET", "/api/echo"): echo}
        for key, fn in cls.extra.items():
            spec = ("test", key[1])
            httpd.ROUTES[key] = spec
            httpd._resolved[spec] = fn

    @classmethod
    def tearDownClass(cls):
        for key in cls.extra:
            httpd._resolved.pop(httpd.ROUTES.pop(key), None)
        cls.srv.shutdown()
        cls.srv.server_close()
        shutil.rmtree(cls.state, ignore_errors=True)

    def get(self, path, token=True, headers=None):
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        h = dict(headers or {})
        if token:
            h["X-Csv-Token"] = self.srv.token
        c.request("GET", path, headers=h)
        r = c.getresponse()
        out = r.status, r.getheader("Content-Type"), r.read(), r.getheader("Connection")
        c.close()
        return out

    def test_unexpected_error_is_a_500_and_closes(self):
        with mock.patch("sys.stderr", io.StringIO()):          # la trace va au journal
            status, ctype, body, conn = self.get("/api/boom")
        self.assertEqual(status, 500)
        self.assertEqual(json.loads(body), {"error": "'imprévu'"})
        self.assertEqual(conn, "close")

    def test_text_error(self):
        status, ctype, body, _ = self.get("/api/tea")
        self.assertEqual((status, ctype, body.decode()), (418, "text/plain; charset=utf-8", "théière"))

    def test_args(self):
        status, _, body, _ = self.get("/api/echo?a=1&a=2&path=~/x")
        d = json.loads(body)
        self.assertEqual((d["a"], d["missing"]), ("1", "dflt"))
        self.assertEqual(d["path"], os.path.realpath(os.path.expanduser("~/x")))

    def test_non_ascii_token_is_refused_not_crashed(self):
        status, _, _, _ = self.get("/api/echo?token=%C3%A9t%C3%A9", token=False)
        self.assertEqual(status, 403)

    def test_ping_lazy_routes(self):
        status, _, body, _ = self.get("/api/ping", token=False)
        self.assertEqual(status, 200)
        self.assertIn(b'"version": "%s"' % config.VERSION.encode(), body)

    def test_conversion_modules_load_on_demand(self):
        # Le démarrage n'importe aucune conversion : elles ne coûtent qu'à leur premier usage.
        import subprocess
        code = ("import sys; sys.path.insert(0, %r); import bridge.app, bridge.routes.windows, bridge.routes.files; "
                "print(sorted(m for m in ('zipfile', 'csv', 'sqlite3', 'xml.etree.ElementTree', 'hashlib') "
                "if m in sys.modules))") % config.APP_DIR
        out = subprocess.run([sys.executable, "-S", "-c", code], capture_output=True, text=True).stdout.strip()
        self.assertEqual(out, "[]")


if __name__ == "__main__":
    unittest.main()
