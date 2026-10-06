"""La file des chemins à ouvrir, le sondage long et le signal « une fenêtre vit »."""

import json
import os
import socket
import threading
import time
import unittest

from tests.support import Bridge


class SessionTest(unittest.TestCase):
    def setUp(self):
        # Un serveur par test : l'état « une fenêtre a été vue » ne se remet pas à zéro.
        self.b = Bridge().start()

    def tearDown(self):
        self.b.stop()

    def open_paths(self, paths):
        import json
        return self.b.post("/api/open", json.dumps({"paths": paths}).encode(),
                           headers={"Content-Type": "application/json"})

    def test_open_queues_absolute_resolved_paths(self):
        p = self.b.tmp("a.csv")
        open(p, "w").close()
        rel = "~/" + os.path.relpath(p, self.b.home).replace(os.sep, "/")
        r = self.open_paths([rel, p, "", self.b.tmp("missing.csv")])
        self.assertEqual(r.json(), {"queued": 3, "client": False})
        # Doublons écartés, chemins inexistants gardés (la page les signale).
        self.assertEqual(self.b.drain(), [os.path.realpath(p), os.path.realpath(self.b.tmp("missing.csv"))])
        self.assertEqual(self.b.drain(), [])

    def test_open_rejects_bad_json(self):
        r = self.b.post("/api/open", b"{nope")
        self.assertEqual(r.status, 400)

    def test_pending_waits_then_answers_empty(self):
        t0 = time.time()
        r = self.b.get("/api/pending?wait=0.5")
        self.assertEqual(r.json(), {"paths": [], "stats": []})
        self.assertGreaterEqual(time.time() - t0, 0.45)

    def test_pending_says_name_and_size(self):
        # Ce que la page demandait aussitôt par /api/stat : nom et taille, None pour ce qui n'est pas un fichier.
        f = self.b.tmp("a b.csv")
        with open(f, "wb") as h:
            h.write(b"x;y\n1;2\n")
        gone = self.b.tmp("absent.csv")
        self.b.post("/api/open", json.dumps({"paths": [f, gone]}).encode())
        j = self.b.get("/api/pending?wait=0").json()
        self.assertEqual(j["paths"], [os.path.realpath(f), os.path.realpath(gone)])
        self.assertEqual(j["stats"], [{"name": "a b.csv", "size": 8}, None])

    def test_pending_wait_is_capped(self):
        # ?wait= est borné à POLL_MAX (10 s) ; une valeur illisible vaut 0.
        t0 = time.time()
        self.b.get("/api/pending?wait=abc")
        self.assertLess(time.time() - t0, 1)

    def test_long_poll_wakes_up_when_a_path_arrives(self):
        got = {}

        def poll():
            t0 = time.time()
            got["paths"] = self.b.get("/api/pending?wait=8").json()["paths"]
            got["took"] = time.time() - t0

        th = threading.Thread(target=poll)
        th.start()
        time.sleep(0.3)
        p = os.path.realpath(self.b.tmp("late.csv"))
        self.open_paths([p])
        th.join(10)
        self.assertEqual(got["paths"], [p])
        self.assertLess(got["took"], 2)

    def test_client_flag_follows_polling(self):
        self.assertFalse(self.b.get("/api/ping").json()["client"])
        th = threading.Thread(target=lambda: self.b.get("/api/pending?wait=2"))
        th.start()
        time.sleep(0.3)
        self.assertTrue(self.b.get("/api/ping").json()["client"])   # requête tenue = fenêtre vivante
        self.assertTrue(self.open_paths([]).json()["client"])
        th.join(5)
        self.assertTrue(self.b.get("/api/ping").json()["client"])   # vue il y a moins de POLL_ALIVE

    def test_window_closed_mid_poll_releases_the_queue(self):
        # Une fenêtre fermée pendant son sondage ne doit pas avaler les chemins
        # déposés ensuite : ils restent pour la fenêtre suivante.
        s = socket.create_connection(("127.0.0.1", self.b.port))
        s.sendall(f"GET /api/pending?wait=8 HTTP/1.1\r\nHost: x\r\nX-Csv-Token: {self.b.token}\r\n\r\n".encode())
        time.sleep(0.3)
        s.close()
        time.sleep(0.6)                      # le serveur s'en aperçoit en 0,25 s
        p = os.path.realpath(self.b.tmp("after-close.csv"))
        self.open_paths([p])
        self.assertEqual(self.b.drain(), [p])


if __name__ == "__main__":
    unittest.main()
