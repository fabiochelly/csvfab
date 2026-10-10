"""« What's new » (ui/js/57-whats-new.js, notes dans ui/releases.js) : la liste des versions.

Les notes ne sont chargées qu'à la première ouverture (app.js ne les porte pas) ; ouvert depuis
la fenêtre About, le dialogue montre chaque version, la plus récente d'abord, la version en cours
marquée. Les notes elles-mêmes : versions décroissantes, dates ISO croissant avec elles, aucune
version ultérieure à celle de bridge/config.py.

Sauté sans Chromium.
"""

import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge, read_version


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class WhatsNewTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/", args=("--window-size=1100,800",))
        cls.chrome.wait_for("typeof openWhatsNew === 'function' && document.readyState === 'complete'")

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def test_from_about(self):
        self.assertFalse(self.chrome.eval("'CSVFAB_RELEASES' in window"), "notes chargées dès le démarrage")
        self.chrome.eval("openAbout(); document.querySelector('#modal-about .ab-news').click(); true")
        self.chrome.wait_for("!!document.querySelector('#nw-list .nw-rel')")
        r = self.chrome.eval("""(() => ({
            about: getComputedStyle(document.getElementById('modal-about')).display,
            news: getComputedStyle(document.getElementById('modal-news')).display,
            vers: [...document.querySelectorAll('#nw-list .nw-when b')].map(b => b.textContent),
            cur: [...document.querySelectorAll('#nw-list .nw-cur .nw-when b')].map(b => b.textContent),
            items: document.querySelectorAll('#nw-list .nw-items li').length,
            n: CSVFAB_RELEASES.length }))()""")
        self.assertEqual(r["about"], "none")
        self.assertEqual(r["news"], "flex")
        self.assertEqual(len(r["vers"]), r["n"])
        self.assertGreater(r["items"], r["n"])
        version = read_version()
        if version in r["vers"]:
            self.assertEqual(r["cur"], [version])
        # Le bouton About revient à la fenêtre About ; une seconde ouverture ne recharge rien.
        self.chrome.eval("document.querySelector('#modal-news .btn-outline').click(); true")
        self.assertEqual(self.chrome.eval("getComputedStyle(document.getElementById('modal-about')).display"), "block")
        self.chrome.eval("openWhatsNew(); true")
        self.assertEqual(self.chrome.eval("document.querySelectorAll('script[src=\"ui/releases.js\"]').length"), 1)
        self.chrome.eval("closeAllModals(); true")

    def test_notes_in_order(self):
        rel = self.chrome.eval("releasesLoad().then(r => r.map(x => [x.v, x.date, x.items.length]))")
        key = lambda v: tuple(int(p) for p in v.split("."))
        vers = [key(v) for v, _, _ in rel]
        self.assertEqual(vers, sorted(vers, reverse=True), "versions hors d'ordre")
        self.assertEqual(len(set(vers)), len(vers))
        dates = [d for _, d, _ in rel]
        self.assertEqual(dates, sorted(dates, reverse=True), "dates hors d'ordre")
        for v, d, n in rel:
            self.assertRegex(d, r"^\d{4}-\d{2}-\d{2}$", v)
            self.assertGreater(n, 0, v)
        self.assertLessEqual(vers[0], key(read_version()), "une version annoncée avant sa sortie")


if __name__ == "__main__":
    unittest.main()
