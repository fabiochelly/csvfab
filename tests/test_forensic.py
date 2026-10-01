"""L'invariant du débogueur de CSV, éprouvé dans la vraie page sur le corpus (tests/corpus.py).

1. Ouvrir puis réécrire sans rien modifier redonne les octets d'origine, au
   bit près — que le fichier soit valide ou non.
2. Modifier une cellule ne change que les octets de son enregistrement :
   tout ce qui précède et tout ce qui suit reste identique.
3. Annuler la modification fait de nouveau réécrire les octets d'origine.

La page tourne dans un Chromium sans fenêtre (tests/cdp.py), servie par un
vrai serveur ; l'écriture passe par srvWrite(), le chemin de toute
sauvegarde d'un fichier ouvert par son chemin. Sauté sans Chromium.
"""

import os
import unittest

from tests import corpus
from tests.cdp import Chrome, find_chromium
from tests.support import Bridge

HELPER = r"""
window.__fx = {
  tab(id) { return tabs.find(t => t.id === id); },
  async open(path) {
    // Un fichier rouvert l'est à neuf : l'onglet d'un test précédent (modifié ou non) est écarté.
    const old = tabs.find(x => x.path === path);
    if (old) { tabs.splice(tabs.indexOf(old), 1); if (activeTabId === old.id) activeTabId = null; }
    await addPathTabs([path]);
    const t = tabs.find(x => x.path === path);
    if (!t) throw new Error('no tab for ' + path);
    await tabRows(t);
    if (t.error) throw new Error(String(t.error.message || t.error));
    activateTab(t.id);
    await new Promise(r => setTimeout(r, 0));
    return t.id;
  },
  info(id) {
    const t = this.tab(id);
    return { rows: t.allData.length, headers: t.headers, synthetic: !!t.syntheticHeader, delim: t.detectedDelim,
             enc: t.detectedEnc, eol: t.detectedEol, bom: !!t.bom, qerr: t.quoteErrors,
             transcoded: !!(t.base && t.base.transcoded) };
  },
  async write(id, out) {
    const j = await srvWrite(this.tab(id), out, false);
    if (!j) throw new Error('write failed: ' + document.getElementById('stats').textContent);
    return j.size;
  },
  edit(id, value) {
    const t = this.tab(id), r = t.allData[0], B = t.base;
    const span = [B.starts[r.b], B.starts[r.b + 1]];
    setCells(t, [[r, 0, value]], 'test');
    return { span, record: r.b };
  },
  undo(id) { activateTab(id); undo(); return this.tab(id).modificationsLog.length; },
};
true
"""


def first_diff(a, b):
    n = min(len(a), len(b))
    i = next((k for k in range(n) if a[k] != b[k]), n)
    return i, a[max(0, i - 12):i + 12], b[max(0, i - 12):i + 12]


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class ForensicTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.paths = dict(zip(corpus.NAMES, corpus.write(cls.b.tmp("corpus"))))
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def open(self, name):
        return self.chrome.eval(f"__fx.open({self.paths[name]!r})")

    def write(self, tab, name):
        out = self.b.tmp(f"out-{name}")
        self.chrome.eval(f"__fx.write({tab}, {out!r})")
        with open(out, "rb") as f:
            return f.read()

    def assertSameBytes(self, got, want, what):
        if got != want:
            i, w, g = first_diff(want, got)
            self.fail(f"{what} : {len(want)} octets attendus, {len(got)} écrits ; premier écart à l'octet {i}\n"
                      f"  attendu {w!r}\n  écrit   {g!r}")

    def test_rewrite_unmodified_is_byte_identical(self):
        for name, data, why in corpus.CASES:
            with self.subTest(name, why=why):
                tab = self.open(name)
                self.assertSameBytes(self.write(tab, name), data, "réécriture sans modification")

    def test_edit_touches_only_its_record(self):
        for name, data, why in corpus.CASES:
            with self.subTest(name, why=why):
                tab = self.open(name)
                info = self.chrome.eval(f"__fx.info({tab})")
                if not info["rows"]:
                    continue
                e = self.chrome.eval(f"__fx.edit({tab}, 'EDITED')")
                got = self.write(tab, name + ".edit")
                if info["transcoded"]:
                    # UTF-16 : comparé en texte, ligne à ligne (les décalages sont ceux de la copie UTF-8).
                    want_lines = data.decode("utf-16", "surrogatepass").splitlines(True)
                    got_lines = got.decode("utf-16", "surrogatepass").splitlines(True)
                    k = e["record"]
                    self.assertEqual(len(got_lines), len(want_lines))
                    self.assertEqual(got_lines[:k] + got_lines[k + 1:], want_lines[:k] + want_lines[k + 1:])
                else:
                    s, end = e["span"]
                    i, _, _ = first_diff(data, got)
                    tail = len(data) - end
                    self.assertGreaterEqual(i, s, "un octet avant l'enregistrement modifié a changé")
                    self.assertEqual(got[len(got) - tail:] if tail else b"", data[end:],
                                     "un octet après l'enregistrement modifié a changé")
                    self.assertIn(b"EDITED", got[s:len(got) - tail])
                self.chrome.eval(f"__fx.undo({tab})")
                self.assertSameBytes(self.write(tab, name + ".undo"), data, "réécriture après annulation")


if __name__ == "__main__":
    unittest.main()
