"""L'écriture refuse un fichier changé depuis la vérification, et deux écritures du même
fichier se suivent sans se croiser (bridge/routes/files.py, bridge/fsio.py).

1. Par HTTP : PUT /api/file?expect_size=&expect_mtime_ns= compare le fichier, juste avant
   de le remplacer, à ce que la page a vérifié ; s'il a changé, 409, rien n'est écrit,
   aucune copie .bak, aucun fichier temporaire laissé, et la connexion reste utilisable.
2. Dans la page : un fichier modifié entre la vérification et l'écriture ramène la
   question (écraser ou recharger) au lieu d'être écrasé en silence.

La partie page est sautée sans Chromium.
"""

import hashlib
import http.client
import json
import os
import threading
import time
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge, q
from tests.test_forensic import HELPER


class WriteGuardHttpTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()

    @classmethod
    def tearDownClass(cls):
        cls.b.stop()

    def make(self, name, data):
        p = self.b.tmp(name)
        with open(p, "wb") as f:
            f.write(data)
        return p

    def leftovers(self, p):
        d = os.path.dirname(p)
        return sorted(f for f in os.listdir(d) if f.endswith((".part", ".bak")) and os.path.basename(p) in f)

    def stamp(self, p):
        st = self.b.get("/api/stat" + q(path=p)).json()
        self.assertIsInstance(st["mtime_ns"], str)
        return st["size"], st["mtime_ns"]

    def test_unchanged_file_is_written(self):
        p = self.make("same.csv", b"a;b\r\n1;2\r\n")
        size, ns = self.stamp(p)
        r = self.b.put("/api/file" + q(path=p, expect_size=size, expect_mtime_ns=ns), b"a;b\r\n1;3\r\n")
        self.assertEqual(r.status, 200, r)
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"a;b\r\n1;3\r\n")

    def test_changed_file_is_refused_and_untouched(self):
        p = self.make("changed.csv", b"a;b\r\n1;2\r\n")
        size, ns = self.stamp(p)
        time.sleep(0.02)
        with open(p, "wb") as f:                     # un autre programme écrit entre-temps
            f.write(b"a;b\r\n9;9\r\n")
        r = self.b.put("/api/file" + q(path=p, expect_size=size, expect_mtime_ns=ns, backup=1), b"a;b\r\n1;3\r\n")
        self.assertEqual(r.status, 409, r)
        self.assertIn("changed", r.json()["error"])
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"a;b\r\n9;9\r\n")
        self.assertEqual(self.leftovers(p), [], "ni temporaire ni .bak")

    def test_deleted_file_is_refused(self):
        p = self.make("gone.csv", b"x\r\n")
        size, ns = self.stamp(p)
        os.remove(p)
        r = self.b.put("/api/file" + q(path=p, expect_size=size, expect_mtime_ns=ns), b"y\r\n")
        self.assertEqual(r.status, 409)
        self.assertFalse(os.path.exists(p))

    def test_without_expectation_writes_as_before(self):
        p = self.make("plain.csv", b"x\r\n")
        self.assertEqual(self.b.put("/api/file" + q(path=p), b"y\r\n").status, 200)
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"y\r\n")

    def test_connection_stays_aligned_after_409(self):
        p = self.make("keepalive.csv", b"x\r\n")
        size, ns = self.stamp(p)
        with open(p, "wb") as f:
            f.write(b"changed!\r\n")
        conn = http.client.HTTPConnection("127.0.0.1", self.b.port, timeout=10)
        try:
            h = {"X-Csv-Token": self.b.token}
            conn.request("PUT", "/api/file" + q(path=p, expect_size=size, expect_mtime_ns=ns), body=b"z" * 200000, headers=h)
            r = conn.getresponse(); r.read()
            self.assertEqual(r.status, 409)
            conn.request("GET", "/api/stat" + q(path=p), headers=h)   # même connexion
            r = conn.getresponse()
            self.assertEqual(r.status, 200)
            self.assertEqual(json.loads(r.read())["size"], 10)
        finally:
            conn.close()

    def test_simultaneous_writes_do_not_mix(self):
        p = self.make("race.csv", b"start\r\n")
        bodies = [bytes([65 + k]) * 3_000_000 for k in range(4)]
        results = []

        def put(body):
            results.append(self.b.put("/api/file" + q(path=p), body, timeout=60).status)

        threads = [threading.Thread(target=put, args=(b,)) for b in bodies]
        for th in threads:
            th.start()
        for th in threads:
            th.join()
        self.assertEqual(results, [200] * 4)
        with open(p, "rb") as f:
            self.assertIn(f.read(), bodies, "le fichier est l'une des écritures, entière")
        self.assertEqual(self.leftovers(p), [])


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class WriteGuardPageTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def test_change_during_save_asks_again(self):
        p = self.b.tmp("racing.csv")
        with open(p, "wb") as f:
            f.write(b"nom;ville\r\nDupont;Paris\r\n")
        tab = self.chrome.eval(f"__fx.open({p!r})")
        res = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({tab});
            __fx.edit({tab}, 'Durand');
            /* La vérification voit le fichier tel qu'il était ; un autre programme l'écrit juste après. */
            const real = diskStamp, before = await real(t);
            let once = true;
            diskStamp = async (...a) => {{ if (once) {{ once = false; return before; }} return real(...a); }};
            return true;
        }})()""")
        self.assertTrue(res)
        time.sleep(0.02)
        with open(p, "wb") as f:
            f.write(b"nom;ville\r\nMartin;Lyon\r\n")
        out = self.chrome.eval("""(async () => {
            const saving = saveInPlace();
            // La seconde vérification voit le changement : la question revient ; on choisit d'écraser.
            for (let i = 0; i < 100 && !document.querySelector('#dlg button'); i++) await new Promise(r => setTimeout(r, 50));
            const asked = [...document.querySelectorAll('#dlg button')].map(b => b.textContent);
            [...document.querySelectorAll('#dlg button')].find(b => b.textContent === 'Overwrite').click();
            const ok = await saving;
            return { ok, asked };
        })()""", timeout=30)
        self.assertTrue(out["ok"])
        self.assertIn("Overwrite", out["asked"])
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"nom;ville\r\nDurand;Paris\r\n")
        baks = [f for f in os.listdir(os.path.dirname(p)) if f.startswith("racing.csv.") and f.endswith(".bak")]
        self.assertEqual(len(baks), 1, "la version de l'autre programme est gardée en .bak")
        with open(os.path.join(os.path.dirname(p), baks[0]), "rb") as f:
            self.assertEqual(f.read(), b"nom;ville\r\nMartin;Lyon\r\n")

    def test_fingerprint_taken_after_showing_is_the_files(self):
        # Le SHA-256 n'est plus pris avant le scan (crypto.subtle.digest bloque le fil principal)
        # mais une fois l'onglet affiché, sur les octets gardés (03-…, afterShown) : ceux du
        # fichier, BOM compris, et ses octets d'origine pour l'UTF-16, transcodé pour la lecture.
        # La date vient de la réponse de lecture (X-File-Mtime*), plus d'un /api/stat.
        files = {"fp-bom.csv": "\ufeffnom;ville\r\nÉlodie;Nîmes\r\n".encode("utf-8"),
                 "fp-u16.csv": "nom;ville\r\nÉlodie;Nîmes\r\n".encode("utf-16"),
                 "fp-plain.csv": b"a;b\n1;2\n"}
        for name, data in files.items():
            p = self.b.tmp(name)
            with open(p, "wb") as f:
                f.write(data)
            tab = self.chrome.eval(f"__fx.open({p!r})")
            out = self.chrome.eval(f"""(async () => {{
                const t = __fx.tab({tab});
                for (let i = 0; i < 100 && !t.stamp.fp; i++) await new Promise(r => setTimeout(r, 20));
                const st = await srvStat(t.path);
                return [t.stamp.fp, t.stamp.size, t.stamp.mtime, t.stamp.mtime_ns, st.size, st.mtime, st.mtime_ns, t.name];
            }})()""")
            self.assertEqual(out[0], hashlib.sha256(data).hexdigest(), name)
            self.assertEqual(out[1:4], out[4:7], name)
            self.assertEqual(out[7], name)

    def test_save_right_after_opening_knows_its_own_file(self):
        # Enregistré avant que l'empreinte différée ait tourné, un fichier dont seule la date a
        # changé (un client de synchro la réécrit) est toujours reconnu : checkDisk la prend alors.
        # Sans image (requestAnimationFrame neutralisé, comme dans une fenêtre cachée), seul le
        # minuteur de 250 ms la prendrait — après la vérification.
        p = self.b.tmp("quick.csv")
        with open(p, "wb") as f:
            f.write(b"nom;ville\r\nDupont;Paris\r\n")
        out = self.chrome.eval(f"""(async () => {{
            const old = tabs.find(x => x.path === {p!r}); if (old) {{ tabs.splice(tabs.indexOf(old), 1); activeTabId = null; }}
            const raf = window.requestAnimationFrame;
            window.requestAnimationFrame = () => 0;
            await addPathTabs([{p!r}]);
            const t = tabs.find(x => x.path === {p!r});
            await tabRows(t);
            const early = !t.stamp.fp;        // l'empreinte n'est pas encore prise
            t.stamp.mtime -= 10;              // la date a changé, pas le contenu
            setCells(t, [[t.allData[0], 0, 'Durand']], 'test');
            let asked = false;
            const watch = (async () => {{
                for (let i = 0; i < 60; i++) {{
                    await new Promise(r => setTimeout(r, 50));
                    const no = [...document.querySelectorAll('#dlg button')].find(b => /cancel/i.test(b.textContent));
                    if (no) {{ asked = true; no.click(); return; }}
                }}
            }})();
            const ok = await saveInPlace();
            window.requestAnimationFrame = raf;
            return {{ early, ok, asked }};
        }})()""", timeout=30)
        self.assertTrue(out["early"], "l'empreinte était déjà prise : le cas n'est pas exercé")
        self.assertFalse(out["asked"])
        self.assertTrue(out["ok"])
        with open(p, "rb") as f:
            self.assertEqual(f.read(), b"nom;ville\r\nDurand;Paris\r\n")
