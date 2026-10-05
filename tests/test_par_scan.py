"""Scan en parallèle (ui/js/45-par-scan.js) : le même résultat que le scan d'un seul tenant.

Chaque fichier est découpé en tranches minuscules (16 octets à 4 Ko, réglages de SCAN_PAR
forcés), scanné par les workers puis recousu, et comparé au scan entier du même fichier :
débuts d'enregistrement, offsets en caractères, lignes irrégulières, largeur, erreurs de
guillemets, encodage retenu. Sur le corpus des fichiers difficiles (tests/corpus.py) et sur
des fichiers tirés au hasard qui accumulent ce qui trompe un scan commencé au milieu d'un
fichier : champs entre guillemets sur plusieurs lignes, guillemets mal fermés ou doublés,
lignes vides, CRLF et LF mêlés, CR seuls, accents, octets UTF-8 invalides. Une tranche qui ne
peut pas être recousue fait scanner le fichier entier : permis, mais compté — la plupart
des découpes doivent réussir, sinon le test ne prouverait rien.

Sauté sans Chromium.
"""

import os
import random
import shutil
import tempfile
import unittest

from tests import corpus
from tests.bench_page import make_crm
from tests.cdp import Chrome, find_chromium
from tests.support import Bridge

BOTH = r"""
window.__scan = async (path, slice, lines) => {
  const ab = await (await srvFetch(srvFileUrl(path))).arrayBuffer();
  const u8 = new Uint8Array(ab), sn = sniffEncoding(u8.subarray(0, 4096)), enc = sn.enc;
  const bom = sn.bom ? (enc === 'utf-8' ? 3 : 2) : 0;
  const f = lines ? textEol(u8, bom) : sniffFormat(u8, enc, bom, '');
  const msg = buf => ({ buf, enc, validate: enc === 'utf-8' && !sn.bom, delim: lines ? -1 : f.delim.charCodeAt(0), nl: f.eol === '\r' ? 13 : 10, bom, lines });
  const keep = { ...SCAN_PAR };
  Object.assign(SCAN_PAR, { min: 0, slice });   // tiny slices, the rest as in use
  let p;
  try { p = await scanParallel(msg(ab), null); } finally { Object.assign(SCAN_PAR, keep); }
  const s = await runScan(msg(ab.slice(0)), null);
  if (!p) return { fallback: true, why: scanParWhy };
  const arr = x => x ? Array.from(x) : null, out = {};
  for (const k of ['n', 'width', 'qerr', 'enc']) if (p[k] !== s[k]) out[k] = [p[k], s[k]];
  for (const k of ['starts', 'chars', 'odd']) {
    const a = arr(p[k]), b = arr(s[k]);
    if (JSON.stringify(a) !== JSON.stringify(b)) {
      const i = a && b ? a.findIndex((v, i) => v !== b[i]) : -1;
      out[k] = { at: i, par: a && a.slice(Math.max(0, i - 2), i + 3), whole: b && b.slice(Math.max(0, i - 2), i + 3), lens: [a && a.length, b && b.length] };
    }
  }
  if (p.off !== s.off || p.len !== s.len) out.span = [p.off, p.len, s.off, s.len];
  return { fallback: false, diff: out };
};
true
"""


def hostile(seed, rows=400):
    """Un CSV tiré au hasard, fait de tout ce qui trompe un scan commencé en pleine ligne."""
    rng = random.Random(seed)
    words = ["a", "bé", "c;d", "x\"y", "é€😀", "lyon", "", "  ", "z\r", "q\nq", "\"\"", "fin\""]
    out = bytearray()
    for _ in range(rows):
        fields = []
        for _ in range(rng.choice([3, 3, 3, 2, 4])):
            w = "".join(rng.choice(words) for _ in range(rng.randrange(0, 4)))
            r = rng.random()
            if r < 0.35:
                fields.append('"' + w.replace('"', '""') + ('\n' + w if rng.random() < 0.3 else "") + '"')
            elif r < 0.4:
                fields.append('"' + w)                          # jamais fermé… jusqu'au prochain guillemet
            elif r < 0.45:
                fields.append('"' + w + '"x')                   # fermé trop tôt : guillemet égaré
            else:
                fields.append(w.replace("\n", " ").replace('"', "'").replace(";", ","))
        out += ";".join(fields).encode("utf-8")
        if rng.random() < 0.02:
            out += b"\xc3\x28"                                   # UTF-8 invalide
        out += rng.choice([b"\n", b"\n", b"\r\n", b"\n\n", b"\r\n\r\n"])
    return bytes(out)


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class ParScanTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix="csvfab-pscan-")
        cls.corpus = corpus.write(os.path.join(cls.dir, "corpus"))
        cls.random = []
        for seed in range(12):
            p = os.path.join(cls.dir, f"hostile-{seed}.csv")
            with open(p, "wb") as f:
                f.write(hostile(seed))
            cls.random.append(p)
        cls.crm = os.path.join(cls.dir, "crm.csv")
        make_crm(cls.crm, 3000)
        # Un seul octet Windows-1252 (é = E9), vers la fin : le fichier n'est pas de l'UTF-8, et c'est
        # une tranche tardive qui le voit — son compte doit survivre à la couture.
        with open(cls.crm, "rb") as f:
            data = f.read()
        cut = data.index(b"\n", len(data) * 9 // 10) + 1
        cls.one_bad = os.path.join(cls.dir, "one-1252.csv")
        with open(cls.one_bad, "wb") as f:
            f.write(data[:cut] + b"Ren\xe9;" + data[cut:])
        cls.b = Bridge().start()
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof scanParallel === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(BOTH)

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()
        shutil.rmtree(cls.dir, ignore_errors=True)

    why = []

    def check(self, paths, slices, lines=False):
        done = fell = 0
        for p in paths:
            size = os.path.getsize(p)
            for sl in slices:
                if size // sl > 3000 or size < 2 * sl:
                    continue
                r = self.chrome.eval(f"__scan({p!r}, {sl}, {str(lines).lower()})", timeout=120)
                if r["fallback"]:
                    fell += 1
                    self.why.append((os.path.basename(p), sl, r["why"]))
                    continue
                done += 1
                self.assertEqual(r["diff"], {}, f"{os.path.basename(p)}, tranches de {sl} o")
        return done, fell

    def test_corpus(self):
        done, fell = self.check(self.corpus, [16, 64, 257, 1024, 4096])
        self.assertGreaterEqual(done, 14, f"trop peu de découpes recousues ({done}, {fell} scannés entiers)")

    def test_hostile(self):
        # Des tranches d'une ligne ou moins se recousent mal (le point de jonction doit tomber dans
        # la tranche suivante) : elles ne servent qu'à multiplier les coutures ; dès 4 Ko, toutes passent.
        done, fell = self.check(self.random, [16, 64, 257, 1024])
        self.assertGreaterEqual(done, 18, f"recousus {done}, scannés entiers {fell}")
        done, fell = self.check(self.random, [4096])
        self.assertEqual(fell, 0, self.why[-3:])

    def test_text_files_and_plain(self):
        done, _ = self.check(self.random[:4] + [self.crm], [64, 1024, 4096], lines=True)
        self.assertGreater(done, 5)
        done, fell = self.check([self.crm], [256, 4096, 65536])
        self.assertEqual(fell, 0)

    def test_one_invalid_byte(self):
        done, fell = self.check([self.one_bad], [256, 4096, 65536])
        self.assertEqual((done, fell), (3, 0))


if __name__ == "__main__":
    unittest.main()
