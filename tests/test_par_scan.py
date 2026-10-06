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
// chunk: the bytes handed over as they would arrive from the server (scanParallel's feed), in pieces
// of 1 to chunk bytes with pauses between them — pseudo-random from a fixed seed, so a failure replays.
// tight: a slice copied with few bytes past its own (4 slices), the first record looked for in 300 —
// or, the files being small, every slice of an arriving file would wait for its last byte.
window.__scan = async (path, slice, lines, chunk, tight) => {
  const ab = await (await srvFetch(srvFileUrl(path))).arrayBuffer();
  const u8 = new Uint8Array(ab), sn = sniffEncoding(u8.subarray(0, 4096)), enc = sn.enc;
  const bom = sn.bom ? (enc === 'utf-8' ? 3 : 2) : 0;
  const f = lines ? textEol(u8, bom) : sniffFormat(u8, enc, bom, '');
  const msg = buf => ({ buf, enc, validate: enc === 'utf-8' && !sn.bom, delim: lines ? -1 : f.delim.charCodeAt(0), nl: f.eol === '\r' ? 13 : 10, bom, lines });
  const keep = { ...SCAN_PAR };
  Object.assign(SCAN_PAR, { min: 0, slice });   // tiny slices, the rest as in use
  if (tight) Object.assign(SCAN_PAR, { over: 4 * slice, scanOver: 4 * slice, head: 300 });
  let p;
  try {
    if (!chunk) p = await scanParallel(msg(ab), null);
    else {
      const dst = new Uint8Array(ab.byteLength), feed = byteFeed();
      const junk = new TextEncoder().encode('"\n;x"\r\n""a;\n');   // what is not there yet must never be read: quotes and breaks, not zeros
      for (let i = 0; i < dst.length; i++) dst[i] = junk[i % junk.length];
      let seed = ab.byteLength * 31 + slice + chunk;
      const rnd = () => (seed = (seed * 1103515245 + 12345) >>> 0) / 4294967296;
      const pump = (async () => {
        feed.start(dst, null);
        for (let i = 0, k = 0; i < u8.length; k++) {
          const n = 1 + Math.floor(rnd() * chunk);
          dst.set(u8.subarray(i, i + n), i); i = Math.min(u8.length, i + n); feed.got(i);
          if (k % 50 === 49) await new Promise(r => setTimeout(r, 0)); else await null;
        }
        feed.end(true);
      })();
      p = await scanParallel(msg(dst.buffer), null, feed);
      await pump;
    }
  } finally { Object.assign(SCAN_PAR, keep); }
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

    def check(self, paths, slices, lines=False, chunk=0, tight=False):
        done = fell = 0
        for p in paths:
            size = os.path.getsize(p)
            for sl in slices:
                if size // sl > 3000 or size < 2 * sl:
                    continue
                r = self.chrome.eval(f"__scan({p!r}, {sl}, {str(lines).lower()}, {chunk}, {str(tight).lower()})", timeout=120)
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

    def test_bytes_still_arriving(self):
        # Le scan lancé pendant la lecture (scanStream) : les tranches partent à mesure que leurs
        # octets arrivent, par morceaux de taille et de rythme quelconques — même résultat.
        # Les mêmes tranches qu'à octets tous là (tight des deux côtés) : autant de coutures réussies,
        # et chacune donne le résultat du scan entier. Le tampon tient d'abord des guillemets et des
        # sauts de ligne là où rien n'est encore arrivé : une tranche partie trop tôt les lirait.
        whole = self.check(self.random[:6], [257, 1024, 4096], tight=True)
        for chunk in (7, 300, 5000):
            self.assertEqual(self.check(self.random[:6], [257, 1024, 4096], chunk=chunk, tight=True), whole, f"morceaux de {chunk} o")
        for chunk in (1000, 70000):
            done, fell = self.check([self.crm, self.one_bad], [4096, 65536], chunk=chunk, tight=True)
            self.assertEqual(fell, 0, self.why[-3:])
        self.assertEqual(self.check(self.corpus, [16, 64, 257, 1024, 4096], chunk=50, tight=True),
                         self.check(self.corpus, [16, 64, 257, 1024, 4096], tight=True))

    def test_open_streamed(self):
        # Une vraie ouverture d'un fichier de plus de SCAN_PAR.min par le pont : scanné pendant sa
        # lecture, et la base obtenue est celle du scan entier des mêmes octets.
        big = os.path.join(self.dir, "big.csv")
        make_crm(big, 40000)
        self.assertGreater(os.path.getsize(big), 8 << 20)
        r = self.chrome.eval(f"""(async () => {{
  const calls = [], sp = window.scanParallel;
  window.scanParallel = async (...a) => {{ const m = await sp(...a); calls.push([!!a[2], m === null ? 'null' : m === false ? 'false' : 'ok']); return m; }};
  try {{ await addPathTabs([{big!r}]); const t = tabs.find(x => x.path === {big!r}); await tabRows(t); }}
  finally {{ window.scanParallel = sp; }}
  const B = T().base, ab = fileBytes(B).slice().buffer;
  const s = await runScan({{ buf: ab, enc: 'utf-8', validate: true, delim: 59, nl: 10, bom: 0, lines: false }}, null);
  const same = k => JSON.stringify(Array.from(B[k] || [])) === JSON.stringify(Array.from(s[k] || []));
  return {{ calls, starts: same('starts'), chars: same('chars'), n: B.n === s.n, width: B.width === s.width, qerr: B.qerr === s.qerr, enc: B.enc === s.enc, rows: T().allData.length }};
}})()""", timeout=120)
        self.assertEqual(r["calls"][0], [True, "ok"], r)
        for k in ("starts", "chars", "n", "width", "qerr", "enc"):
            self.assertTrue(r[k], (k, r))
        self.assertEqual(r["rows"], 40000)


if __name__ == "__main__":
    unittest.main()
