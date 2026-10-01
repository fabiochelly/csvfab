"""Le lanceur csvfab.py face à un vrai serveur, avec un faux navigateur.

Le navigateur est un script qui note ses arguments (CSVFAB_BROWSER), et le PATH
est réduit à un dossier vide : ni uwsm-app, ni hyprctl, ni notify-send du
poste ne sont appelés. Ce qui est vérifié surtout : lanceur et serveur
s'accordent sur le dossier d'état (le jeton), et une fenêtre n'est ouverte
que s'il n'y en a pas déjà une.
"""

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import unittest.mock
import urllib.request

from tests.support import LAUNCHER, Bridge, free_port, isolated_env, read_version, state_of, wait_token


def run_launcher(home, port, *args, timeout=20):
    bin_dir = os.path.join(home, "bin")
    os.makedirs(bin_dir, exist_ok=True)
    stub = os.path.join(bin_dir, "fake-browser")
    if not os.path.exists(stub):
        with open(stub, "w") as f:
            # Rien que des commandes internes du shell : le PATH est vide.
            f.write('#!/bin/sh\nfor a in "$@"; do printf "%s\\n" "$a"; done >> "${0%/*}/browser-calls"\n'
                    'echo --- >> "${0%/*}/browser-calls"\n')
        os.chmod(stub, 0o755)
    env = isolated_env(home, port, CSVFAB_BROWSER=stub, PATH=bin_dir)
    return subprocess.run([sys.executable, LAUNCHER, *args], env=env, cwd=home,
                          capture_output=True, text=True, timeout=timeout)


def browser_calls(home, expect=0, timeout=5):
    """Les appels du faux navigateur ; il est lancé détaché, on attend `expect` appels."""
    deadline = time.time() + timeout
    while True:
        calls = _calls(home)
        if len(calls) >= expect or time.time() > deadline:
            return calls
        time.sleep(0.02)


def _calls(home):
    try:
        with open(os.path.join(home, "bin", "browser-calls")) as f:
            text = f.read()
    except OSError:
        return []
    return [c.strip().split("\n") for c in text.split("---") if c.strip()]


def ping(port):
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(
            f"http://127.0.0.1:{port}/api/ping", timeout=2) as r:
        return json.load(r)


def pending(port, token, wait=0):
    req = urllib.request.Request(f"http://127.0.0.1:{port}/api/pending?wait={wait}",
                                 headers={"X-Csv-Token": token})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=wait + 5) as r:
        return json.load(r)["paths"]


def load_launcher():
    """csvfab.py importé comme un module (son main() ne tourne pas)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("csvfab_launcher", LAUNCHER)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class LauncherUnitTest(unittest.TestCase):
    def setUp(self):
        self.l = load_launcher()
        self.dir = tempfile.mkdtemp(prefix="csvfab-test-")

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)

    def test_shares_the_server_config(self):
        from bridge import config
        self.assertEqual((self.l.VERSION, self.l.PORT), (config.VERSION, config.PORT))
        self.assertIs(self.l.state_dir, config.state_dir)

    @unittest.skipIf(os.name == "nt", "PATHEXT")
    def test_which(self):
        exe = os.path.join(self.dir, "tool")
        with open(exe, "w") as f:
            f.write("#!/bin/sh\n")
        plain = os.path.join(self.dir, "data")
        open(plain, "w").close()
        os.chmod(exe, 0o755)
        with unittest.mock.patch.dict(os.environ, {"PATH": os.pathsep.join(["", "/nonexistent", self.dir])}):
            self.assertEqual(self.l.which("tool"), exe)
            self.assertEqual(self.l.which("tool"), shutil.which("tool"))
            self.assertIsNone(self.l.which("data"))              # pas exécutable
            self.assertIsNone(self.l.which("absent"))

    @unittest.skipIf(os.name == "nt", "posix_spawn")
    def test_spawn_detaches_and_redirects(self):
        out = os.path.join(self.dir, "out.log")
        done = os.path.join(self.dir, "done")
        code = ("import os, sys; print(os.getsid(0) == os.getpid(), sys.stdin.read() == ''); "
                "sys.stdout.flush(); open(%r, 'w').close()") % done
        with open(out, "ab") as log:
            self.l.spawn([sys.executable, "-c", code], out=log.fileno())
        deadline = time.time() + 10
        while not os.path.exists(done) and time.time() < deadline:
            time.sleep(0.02)
        with open(out) as f:
            self.assertEqual(f.read(), "True True\n")      # sa propre session, entrée vide

    @unittest.skipIf(os.name == "nt", "posix_spawn")
    def test_spawn_missing_program(self):
        with self.assertRaises(FileNotFoundError):
            self.l.spawn(["csvfab-no-such-program-xyz"])


class LauncherCliTest(unittest.TestCase):
    def test_version(self):
        home = tempfile.mkdtemp(prefix="csvfab-test-")
        try:
            r = run_launcher(home, free_port(), "--version")
            self.assertEqual((r.returncode, r.stdout), (0, f"csvfab {read_version()}\n"))
            self.assertEqual(run_launcher(home, free_port(), "-V").stdout, r.stdout)
            self.assertFalse(os.path.exists(os.path.join(home, "state")))   # rien de lancé ni créé
        finally:
            shutil.rmtree(home, ignore_errors=True)

    def test_help(self):
        home = tempfile.mkdtemp(prefix="csvfab-test-")
        try:
            r = run_launcher(home, free_port(), "--help")
            self.assertEqual(r.returncode, 0)
            self.assertIn("csvfab --version", r.stdout)
        finally:
            shutil.rmtree(home, ignore_errors=True)


@unittest.skipIf(os.name == "nt", "faux navigateur en script shell")
class LauncherTest(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.mkdtemp(prefix="csvfab-test-")
        self.port = free_port()
        self.state = state_of(self.home)
        self.pid = None

    def tearDown(self):
        if self.pid:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except OSError:
                pass
        shutil.rmtree(self.home, ignore_errors=True)

    def file(self, name):
        p = os.path.join(self.home, name)
        open(p, "w").close()
        return os.path.realpath(p)

    def test_cold_start(self):
        p = self.file("a.csv")
        r = run_launcher(self.home, self.port, p)
        self.assertEqual(r.returncode, 0, r.stderr)
        status = ping(self.port)
        self.pid = status["pid"]
        self.assertEqual(status["version"], read_version())
        # Une seule fenêtre, en mode app, sur notre port et notre profil.
        calls = browser_calls(self.home, 1)
        self.assertEqual(len(calls), 1, calls)
        args = calls[0]
        self.assertIn(f"--app=http://127.0.0.1:{self.port}/", args)
        self.assertIn(f"--user-data-dir={os.path.join(self.state, 'profile')}", args)
        self.assertIn("--disable-background-timer-throttling", args)
        if sys.platform.startswith("linux"):
            self.assertIn("--class=csvfab", args)
        # Le fichier attend dans la file du serveur que lance le lanceur.
        token = wait_token(self.state)
        self.assertEqual(pending(self.port, token), [p])
        self.assertTrue(os.path.isfile(os.path.join(self.state, "server.log")))

    def test_server_without_window_opens_one(self):
        with Bridge() as b:
            p = os.path.realpath(b.tmp("b.csv"))
            open(p, "w").close()
            r = run_launcher(b.home, b.port, p)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(len(browser_calls(b.home, 1)), 1)
            self.assertEqual(b.drain(), [p])

    def test_running_window_gets_the_file_as_a_tab(self):
        with Bridge() as b:
            got = {}
            th = threading.Thread(target=lambda: got.update(paths=b.get("/api/pending?wait=8").json()["paths"]))
            th.start()
            time.sleep(0.3)
            p = os.path.realpath(b.tmp("c.csv"))
            open(p, "w").close()
            r = run_launcher(b.home, b.port, p)
            th.join(10)
            self.assertEqual(r.returncode, 0, r.stderr)
            time.sleep(0.3)
            self.assertEqual(browser_calls(b.home), [])      # pas de seconde fenêtre
            self.assertEqual(got["paths"], [p])

    @unittest.skipIf(sys.platform == "darwin", "le dossier d'état macOS n'a pas d'ancien nom à part")
    def test_old_state_folder_is_migrated(self):
        old = os.path.join(os.path.dirname(self.state), "csv-editor")
        os.makedirs(os.path.join(old, "profile"))
        with open(os.path.join(old, "profile", "keep"), "w") as f:
            f.write("granted permissions")
        r = run_launcher(self.home, self.port)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.pid = ping(self.port)["pid"]
        self.assertFalse(os.path.exists(old))
        with open(os.path.join(self.state, "profile", "keep")) as f:
            self.assertEqual(f.read(), "granted permissions")
        wait_token(self.state)
        self.assertIn(f"--user-data-dir={os.path.join(self.state, 'profile')}", browser_calls(self.home, 1)[0])


if __name__ == "__main__":
    unittest.main()
