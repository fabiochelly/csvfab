"""Le moteur de formules ne laisse passer que son langage (ui/js/38-formula-parser.js).

Une formule tourne dans la page, qui détient le jeton d'accès à tous les
fichiers : du JavaScript quelconque pourrait les lire et les envoyer ailleurs.
Vérifié dans la vraie page :
- chaque exemple de la documentation (FX_DOC) est accepté et donne, ligne par
  ligne, exactement ce que donnait l'ancien moteur (new Function sur le texte tapé) ;
- les tentatives d'évasion sont refusées à la compilation, ou neutralisées à
  l'exécution pour l'index calculé (x["constructor"]), sans que rien ne s'exécute ;
- le code compilé est aussi rapide qu'avant.
"""

import json
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

CSV = ("id;nom;ville;montant;date;email\n"
       + "".join(f"{i};Nom{i} Dupont;{'Lyon' if i % 3 else 'Paris'};{i * 13 % 997},{i % 100:02d};"
                 f"{1 + i % 28:02d}/{1 + i % 12:02d}/20{10 + i % 15};user{i}@exemple.fr\n" for i in range(1, 61)))

# Chacune doit être refusée à la compilation.
REFUSED = [
    'fetch("/api/file")', 'window.CSVFAB_TOKEN', 'CSVFAB_TOKEN', 'this', 'globalThis', 'self', 'top', 'parent',
    'document.cookie', 'location', 'eval("1")', '(0, eval)("1")', 'Function("return 1")()', 'new Date()',
    '"".constructor.constructor("return 1")()', '{nom}.constructor', '{nom}.__proto__', '{nom}.prototype',
    '[].constructor', '/a/.constructor', 'Math.constructor', 'Math', '{nom}.toString.call', '{nom}.at.bind({nom})',
    'upper.call(null, "x")', 'upper', 'today', '{nom}.upper', 'x = 1', '{nom} = "x"', '$[0]', '$', 'row = 1',
    'a => a', '(() => 1)()', '`${1}`', '1; 2', '1, 2', '{nom}?.length', 'import("x")', 'require("fs")',
    '__xss()', 'fxAt({nom}, 0)', 'srvFetch("/api/file")', '{nom}["constructor"]["constructor"]("return 1")()',
    '{nom}.split(" ")["constructor"]("x")', 'typeof {nom}', 'void 0', 'delete {nom}', '{nom} in {nom}',
    'upper({nom})()', '({nom}).upper()()', '"x"("y")', '{inconnue}', '{nom', '"non fermé', '/x',
]
# Acceptées, mais l'index calculé n'atteint jamais une propriété : la valeur est vide.
NEUTRALISED = ['{nom}["constructor"]', '{nom}["__proto__"]', '{nom}["length"]', '{nom}.split(" ")["constructor"]',
               '[1, 2]["constructor"]', '{nom}[{nom}]']

SETUP = r"""
window.__pwn = 0;
window.__oldCompile = (t, src) => {                 // l'ancien moteur, pour comparer
  const slugs = t.headers.map(h => slugify(h)), used = [];
  const body = src.replace(/\{([^{}\n]+)\}/g, (m, name) => {
    let c = t.headers.indexOf(name); if (c < 0) c = t.headers.indexOf(name.trim()); if (c < 0) c = slugs.indexOf(slugify(name));
    if (!used.includes(c)) used.push(c); return `$[${c}]`; });
  return { fn: new Function('$', 'row', ...Object.keys(FX), `"use strict"; return (${body}\n);`), used };
};
window.__run = (t, c) => t.allData.map(r => { try { return evalRow(c.fn, t, r, c.used); } catch (e) { return 'ERR:' + e.message; } });
true
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class FormulaSafetyTest(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.path = cls.b.tmp("formules.csv")
        with open(cls.path, "w", encoding="utf-8") as f:
            f.write(CSV)
        cls.chrome = Chrome(cls.b.origin + "/")
        try:
            cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
            cls.chrome.eval(HELPER)
            cls.chrome.eval(SETUP)
            cls.tab = cls.chrome.eval(f"__fx.open({cls.path!r})")
        except Exception:
            cls.chrome.close()
            cls.b.stop()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def test_every_documented_example_compiles_and_gives_the_same_values(self):
        res = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({self.tab}), out = [];
            setSel(t, 0, 1, 0, 1);
            /* Les regex d'une formule sont d'abord essayées à part (40-…) : une passe pour les faire vérifier. */
            for (const x of FX_DOC) for (const [ex] of x.ex || []) compileFormula(t, fxExample(t, ex, x.pick));
            while (regexWaiting.size) await new Promise(r => setTimeout(r, 20));
            for (const x of FX_DOC) for (const [ex] of x.ex || []) {{
                const src = fxExample(t, ex, x.pick);
                if (/uuid/.test(src)) continue;                       // aléatoires : jamais deux fois les mêmes
                const c = compileFormula(t, src);
                if (c.error) {{ out.push({{ src, error: c.error }}); continue; }}
                const a = JSON.stringify(__run(t, c)), b = JSON.stringify(__run(t, __oldCompile(t, src)));
                if (a !== b) out.push({{ src, error: 'différent', now: a.slice(0, 200), before: b.slice(0, 200) }});
            }}
            return {{ bad: out, count: FX_DOC.reduce((n, x) => n + (x.ex || []).length, 0) }};
        }})()""")
        self.assertGreater(res["count"], 80)
        self.assertEqual(res["bad"], [], json.dumps(res["bad"], indent=1, ensure_ascii=False))

    def test_escapes_are_refused(self):
        res = self.chrome.eval(f"""(() => {{
            const t = __fx.tab({self.tab}), out = {{}};
            for (const src of {json.dumps(REFUSED)}) {{ const c = compileFormula(t, src); out[src] = c.error || null; }}
            return out;
        }})()""")
        accepted = [src for src, err in res.items() if not err]
        self.assertEqual(accepted, [], "acceptées alors qu'elles devaient être refusées")
        self.assertEqual(self.chrome.eval("window.__pwn"), 0)

    def test_computed_index_is_neutralised(self):
        res = self.chrome.eval(f"""(() => {{
            const t = __fx.tab({self.tab}), out = {{}};
            for (const src of {json.dumps(NEUTRALISED)}) {{
                const c = compileFormula(t, src);
                out[src] = c.error ? 'refusée : ' + c.error : [...new Set(__run(t, c))];
            }}
            out['{{nom}}.split(" ")[0]'] = __run(t, compileFormula(t, '{{nom}}.split(" ")[0]')).slice(0, 2);
            out['{{nom}}[0]'] = __run(t, compileFormula(t, '{{nom}}[0]')).slice(0, 1);
            return out;
        }})()""")
        for src in NEUTRALISED:
            self.assertEqual(res[src], [""], f"{src} : {res[src]}")
        self.assertEqual(res['{nom}.split(" ")[0]'], ["Nom1", "Nom2"])          # l'usage documenté marche
        self.assertEqual(res['{nom}[0]'], ["N"])

    def test_useful_syntax_still_works(self):
        res = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({self.tab}), r = t.allData[0], out = {{}};
            fxComma = true;                                         // un fichier en « ; » : virgule décimale
            const srcs = ['row % 2 === 0', '{{montant}}.num() > 100 ? "big" : "small"', 'Math.round({{montant}}.num())',
                               'Math.max(1, 2) ** 2', '-{{montant}}.num()', 'matches({{email}}, /^user\\\\d+@/)', '[{{ville}}, "x"].join("-")',
                               '{{ville}}.length', 'Math.PI > 3', '{{nom}}.replace(/\\\\d+/g, "#")', '!empty({{nom}}) && {{ville}} !== "Paris"',
                               '{{date}}.date().year() >= 2010', 'first("", {{ville}})', '"a\\\\"b" + \\'c\\'', '0x1F + .5', '1e3'];
            for (const src of srcs) compileFormula(t, src);          // les regex sont d'abord essayées à part (40-…)
            while (regexWaiting.size) await new Promise(r => setTimeout(r, 20));
            for (const src of srcs) {{
                const c = compileFormula(t, src);
                out[src] = c.error ? 'ERREUR ' + c.error : evalRow(c.fn, t, r, c.used);
            }}
            return out;
        }})()""")
        self.assertEqual(res, {
            'row % 2 === 0': "false", '{montant}.num() > 100 ? "big" : "small"': "small", 'Math.round({montant}.num())': "13",
            'Math.max(1, 2) ** 2': "4", '-{montant}.num()': "-13,01", 'matches({email}, /^user\\d+@/)': "true",
            '[{ville}, "x"].join("-")': "Lyon-x", '{ville}.length': "4", 'Math.PI > 3': "true",
            '{nom}.replace(/\\d+/g, "#")': "Nom# Dupont", '!empty({nom}) && {ville} !== "Paris"': "true",
            '{date}.date().year() >= 2010': "true", 'first("", {ville})': "Lyon", '"a\\"b" + \'c\'': 'a"bc', '0x1F + .5': "31,5", '1e3': "1000"})

    def test_duplicates_formula_is_checked_too(self):
        res = self.chrome.eval("""(() => ({ ok: dupFormula('left(slug(v), 3)').error || null,
            bad: dupFormula('fetch("/x")').error || null, col: dupFormula('{nom}').error || null,
            value: dupFormula('left(slug(v), 3)').fn('Éléonore') }))()""")
        self.assertIsNone(res["ok"])
        self.assertEqual(res["value"], "ele")
        self.assertIn("Unknown name: fetch", res["bad"])
        self.assertIn("Write v", res["col"])

    def test_as_fast_as_before(self):
        # Le code compilé tourne sur chaque ligne : même vitesse que l'ancien (V8 le compile pareil).
        res = self.chrome.eval("""(() => {
            const t = T(), rows = [], N = 300000;
            for (let i = 0; i < N; i++) rows.push([String(i), 'Nom' + i, i % 3 ? 'Lyon' : 'Paris', (i * 13 % 997) + ',5', '15/03/2020', 'u@x.fr']);
            const srcs = ['{montant}.num() > 100 && {ville} === "Lyon"', 'contains({nom}, "12") || {ville}.startsWith("P")', '{nom}.split("m")[1]'];
            const time = c => { let best = 1e9; for (let k = 0; k < 5; k++) { const t0 = performance.now(); let n = 0;
                for (let i = 0; i < N; i++) if (c.fn(rows[i], i, ...FX_VALUES)) n++; best = Math.min(best, performance.now() - t0); } return best; };
            return srcs.map(s => ({ src: s, before: time(__oldCompile(t, s)), now: time(compileFormula(t, s)),
                                    compile_us: (() => { const t0 = performance.now(); for (let k = 0; k < 200; k++) compileFormula(t, s); return (performance.now() - t0) / 200 * 1000; })() }));
        })()""", timeout=120)
        for r in res:
            print(f"\n  {r['src']:<52} avant {r['before']:6.1f} ms  après {r['now']:6.1f} ms  compilation {r['compile_us']:.0f} µs", end="")
            self.assertLess(r["now"], r["before"] * 1.25 + 2, r)
        print()


if __name__ == "__main__":
    unittest.main()
