"""Schémas Table Schema (ui/js/41-schema.js), éprouvés dans la vraie page.

1. Déduction : ce que chaque colonne d'un fichier respecte, en règles prudentes
   (types, obligatoire, unique, liste de valeurs, motif des codes à zéro initial,
   jamais négatif ; pas de bornes observées).
2. Validation : un autre fichier contre ce schéma — structure (colonne manquante,
   en trop), lignes irrégulières, chaque règle comptée avec ses exemples, et le
   fichier d'origine sans aucune erreur.
3. Le schéma écrit à côté du fichier, et retrouvé pour un export daté de la même
   famille (clients_2026-10-04.csv → clients.schema.json).
4. Un motif à retour arrière catastrophique est refusé par la garde des regex.

Sauté sans Chromium.
"""

import json
import os
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER

GOOD = ("id;code;montant;date;actif;email;statut;note\r\n"
        + "".join(f"{i};{i % 7:05d};{i * 3},{i % 10}0;{(i % 28) + 1:02d}/{(i % 12) + 1:02d}/2024;{'oui' if i % 3 else 'non'};"
                  f"client{i}@exemple.fr;{['actif', 'inactif', 'vip'][i % 3]};{'texte libre ' + str(i) if i % 4 else ''}\r\n"
                  for i in range(1, 121)))

BAD = ("id;code;montant;date;actif;email;statut;extra\r\n"
       "1;00001;10,50;01/01/2024;oui;a@b.fr;actif;x\r\n"
       "1;00002;-3,00;31/02/2024;peut-être;pas-un-mail;inconnu;x\r\n"     # doublon, négatif, date, booléen, e-mail, liste
       "3;123;abc;01/01/2024;non;c@d.fr;vip;x\r\n"                         # motif, nombre
       ";00004;1,00;01/01/2024;oui;e@f.fr;actif\r\n")                      # obligatoire vide, ligne courte


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class SchemaTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.good = cls.b.tmp("clients.csv")
        cls.dated = cls.b.tmp("clients_2026-10-04.csv")
        with open(cls.good, "w", encoding="utf-8", newline="") as f:
            f.write(GOOD)
        with open(cls.dated, "w", encoding="utf-8", newline="") as f:
            f.write(BAD)
        cls.chrome = Chrome(cls.b.origin + "/")
        cls.chrome.wait_for("typeof schemaInfer === 'function' && document.readyState === 'complete'")
        cls.chrome.eval(HELPER)
        cls.schema = cls.chrome.eval(f"__fx.open({cls.good!r}).then(id => schemaInfer(__fx.tab(id)))")

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def open(self, path):
        """Un onglet neuf : rouvrir un chemin remplace l'onglet d'un test précédent."""
        return self.chrome.eval(f"__fx.open({path!r})")

    def field(self, name):
        return next(f for f in self.schema["fields"] if f["name"] == name)

    def test_inference(self):
        self.assertEqual(self.field("id"), {"name": "id", "type": "integer", "constraints": {"minimum": 0, "required": True, "unique": True}})
        self.assertEqual(self.field("code"), {"name": "code", "type": "string", "constraints": {"pattern": "\\d{5}", "required": True}})
        self.assertEqual(self.field("montant"), {"name": "montant", "type": "number", "decimalChar": ",", "constraints": {"minimum": 0, "required": True}})
        self.assertEqual(self.field("date"), {"name": "date", "type": "date", "format": "%d/%m/%Y", "constraints": {"required": True}})
        self.assertEqual(self.field("actif"), {"name": "actif", "type": "boolean", "trueValues": ["oui"], "falseValues": ["non"], "constraints": {"required": True}})
        self.assertEqual(self.field("email"), {"name": "email", "type": "string", "format": "email", "constraints": {"required": True, "unique": True}})
        self.assertEqual(self.field("statut")["constraints"], {"required": True, "enum": ["actif", "inactif", "vip"]})
        self.assertEqual(self.field("note"), {"name": "note", "type": "string"})     # vide par endroits, libre : aucune règle
        self.assertEqual(self.schema["primaryKey"], "id")

    def test_own_file_is_valid(self):
        r = self.chrome.eval(f"schemaValidate(__fx.tab({self.open(self.good)}), {json.dumps(self.schema)}).then(v => ({{ rows: v.rows.size, structure: v.structure, checked: v.checked }}))")
        self.assertEqual(r, {"rows": 0, "structure": [], "checked": 120})

    def test_bad_file(self):
        tab = self.open(self.dated)
        r = self.chrome.eval(f"""schemaValidate(__fx.tab({tab}), {json.dumps(self.schema)}).then(v => ({{
            rows: v.rows.size, structure: v.structure,
            items: Object.fromEntries(v.items.map(i => [(i.field || '-') + ' ' + i.rule, [i.count, i.examples]])) }}))""")
        self.assertEqual(r["structure"], ["missing column “note”", "unexpected column “extra”"])
        self.assertEqual(r["items"], {
            "id unique": [1, ["1"]], "id required": [1, [""]],
            "montant minimum": [1, ["-3,00"]], "montant type": [1, ["abc"]],
            "date type": [1, ["31/02/2024"]], "actif type": [1, ["peut-être"]], "email type": [1, ["pas-un-mail"]],
            "statut enum": [1, ["inconnu"]], "code pattern": [1, ["123"]], "- fields": [1, ["row 4"]]})
        self.assertEqual(r["rows"], 3)          # la première ligne est juste

    def test_written_beside_and_found_for_a_dated_export(self):
        r = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab(await __fx.open({self.good!r}));
            const where = await schemaWrite(t, JSON.stringify({json.dumps(self.schema)}, null, 2));
            const tab2 = await __fx.open({self.dated!r});
            const found = await schemaFind(__fx.tab(tab2));
            return {{ where, found: found && found.name, same: found && JSON.parse(found.text).primaryKey }};
        }})()""")
        self.assertTrue(r["where"].endswith("clients.schema.json"))
        self.assertEqual(r["found"], "clients.schema.json")
        self.assertEqual(r["same"], "id")
        with open(os.path.join(os.path.dirname(self.good), "clients.schema.json"), encoding="utf-8") as f:
            self.assertEqual(json.load(f), self.schema)

    def test_stems_of_dated_exports(self):
        r = self.chrome.eval("""['clients_2026-10-04.csv', 'clients-20261004.csv', 'clients v3.csv', 'clients (2).csv', 'export_2026-10-04T08-30.csv', 'clients.csv']
            .map(n => schemaStems(n))""")
        self.assertEqual(r, [["clients_2026-10-04", "clients"], ["clients-20261004", "clients"], ["clients v3", "clients"],
                             ["clients (2)", "clients"], ["export_2026-10-04T08-30", "export"], ["clients"]])

    def test_catastrophic_pattern_is_refused(self):
        p = self.b.tmp("motifs.csv")
        with open(p, "w", encoding="utf-8", newline="") as f:
            f.write("mot\r\n" + "".join("a" * 30 + "b\r\n" for _ in range(50)))
        tab = self.open(p)
        schema = {"fields": [{"name": "mot", "type": "string", "constraints": {"pattern": "(a+)+"}}]}
        r = self.chrome.eval(f"schemaValidate(__fx.tab({tab}), {json.dumps(schema)}).then(v => v.error || 'ok')", timeout=30)
        self.assertIn("too long", r)

    def test_report_marks_rows(self):
        tab = self.open(self.dated)
        r = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({tab});
            document.getElementById('modal-schema').style.display = 'block';     // as openSchemaValidate leaves it: closed means cancelled
            await schemaRun(t, 'clients.schema.json', JSON.stringify({json.dumps(self.schema)}));
            schemaShow();
            const m = t.rowMark;
            return {{ rows: m.rows.size, only: m.only, shown: t.filteredData.length,
                      tip: [...m.cells.values()].map(c => [...c.entries()]).flat().map(e => e.join(': ')).sort().slice(0, 3) }};
        }})()""")
        self.assertEqual(r["rows"], 3)
        self.assertTrue(r["only"])
        self.assertEqual(r["shown"], 3)
        self.assertIn("actif: not a boolean", r["tip"])

    def test_rules_added_by_hand(self):
        """« + rule » : suggestions tirées des valeurs, valeur modifiable, décompte des lignes qui la violent."""
        tab = self.open(self.good)
        r = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({tab});
            await openSchemaCreate();
            const D = schemaDraft, col = n => t.headers.indexOf(n), out = {{}};
            const h = D.hints;
            out.code = h[col('code')].patterns.map(p => p[0]);
            out.notePattern = h[col('note')].patterns[0];
            out.montant = [h[col('montant')].min, h[col('montant')].max];
            out.date = [h[col('date')].min, h[col('date')].max];
            out.note = [h[col('note')].lmin, h[col('note')].lmax];
            schemaAddRule(col('montant'), 'maximum');                        // le plus grand vu : personne ne la viole
            schemaSetValue(col('id'), 'minimum', '100');                       // 99 identifiants en dessous
            schemaAddRule(col('note'), 'pattern', 'texte libre \\\\d+');
            schemaSetValue(col('statut'), 'enum', 'actif, "vip, or"');           // inactif (40) et vip (40) n'y sont plus
            schemaSetValue(col('montant'), 'minimum', 'abc');                  // ne se lit pas : refusée, l'ancienne valeur reste
            await new Promise(r => setTimeout(r, 1500));                      // le motif passe par la garde des regex
            const c = k => D.counts.get(col(k.split('.')[0]) + '|' + k.split('.')[1]);
            out.counts = {{ max: c('montant.maximum'), id: c('id.minimum'), note: c('note.pattern'), statut: c('statut.enum'), bad: c('montant.minimum') }};
            const s = schemaFromDraft(), f = n => s.fields.find(x => x.name === n).constraints;
            out.written = {{ montant: f('montant'), id: f('id'), note: f('note'), statut: f('statut') }};
            out.list = scParseList(scRuleText(s.fields.find(x => x.name === 'statut'), 'enum'));
            closeAllModals();
            return out;
        }})()""", timeout=60)
        self.assertEqual(r["code"], ["\\d{5}", "\\d+"])            # la forme exacte, puis toute longueur
        self.assertEqual(r["notePattern"], ["[a-zß-öø-ÿ]{5} [a-zß-öø-ÿ]{5} \\d{1,3}", "the shape of the values, with the lengths seen"])
        self.assertEqual(r["montant"], ["3,10", "360,00"])
        self.assertEqual(r["date"], ["01/01/2024", "28/12/2024"])
        self.assertEqual(r["note"], [13, 15])
        self.assertEqual(r["counts"], {"max": 0, "id": 99, "note": 0, "statut": 80, "bad": "not valid"})
        self.assertEqual(r["written"], {
            "montant": {"minimum": 0, "required": True, "maximum": 360},
            "id": {"minimum": 100, "required": True, "unique": True},
            "note": {"pattern": "texte libre \\d+"},
            "statut": {"required": True, "enum": ["actif", "vip, or"]}})
        self.assertEqual(r["list"], ["actif", "vip, or"])

    def test_shapes(self):
        r = self.chrome.eval("""['AB-1234', '+33 6 12 34 56 78', 'a{b}+c', 'Élodie', 'x.y/z'].map(v => {
            const [e, g] = scShape(v), re = scShapeRegex(e), rg = scShapeRegex(g);
            return [re, new RegExp(`^(?:${re})$`, 'u').test(v), new RegExp(`^(?:${rg})$`, 'u').test(v)];
        })""")
        self.assertEqual([x[0] for x in r], ["[A-ZÀ-ÖØ-Þ]{2}-\\d{4}", "\\+\\d{2} \\d \\d{2} \\d{2} \\d{2} \\d{2}",
                                             "[a-zß-öø-ÿ]\\{[a-zß-öø-ÿ]\\}\\+[a-zß-öø-ÿ]", "[A-ZÀ-ÖØ-Þ][a-zß-öø-ÿ]{5}", "[a-zß-öø-ÿ]\\.[a-zß-öø-ÿ]\\/[a-zß-öø-ÿ]"])
        self.assertTrue(all(x[1] and x[2] for x in r))

    def test_text_checks(self):
        """Contrôles d'un texte (caractères invisibles, doubles espaces, accents abîmés, casse) :
        ajoutés un à un, comptés à part, écrits dans le seul motif standard (aucune clé hors
        Table Schema), relus contrôle par contrôle — et le motif dit la même chose à Python."""
        p = self.b.tmp("noms.csv")
        noms = ["Dupont", "DUPONT", "dupont", "Le  Havre", "Ã©cole", "Zoé​", "Boulogne-sur-Mer", "Jean-Pierre"]
        with open(p, "w", encoding="utf-8", newline="") as f:
            f.write("id;nom\r\n" + "".join(f"{i};{n}\r\n" for i, n in enumerate(noms, 1)))
        tab = self.open(p)
        r = self.chrome.eval(f"""(async () => {{
            const t = __fx.tab({tab});
            await openSchemaCreate();
            const D = schemaDraft, c = t.headers.indexOf('nom'), out = {{}};
            const menu = () => {{ document.querySelectorAll('.sc-add')[c].click(); const s = document.getElementById('sc-menu').textContent; closeDDs(); return s; }};
            out.before = ['Upper case', 'Capitalised', 'No garbled accents'].map(l => menu().includes(l));
            for (const k of ['noInvisible', 'noDoubleSpace', 'noMojibake', 'capitalised']) schemaAddRule(c, k);
            out.after = ['Upper case', 'Capitalised'].map(l => menu().includes(l));     // l'un exclut l'autre
            schemaAddRule(c, 'pattern', '[\\\\s\\\\S]{{1,20}}');
            await new Promise(r => setTimeout(r, 1500));
            out.counts = Object.fromEntries(['noInvisible', 'noDoubleSpace', 'noMojibake', 'capitalised', 'pattern'].map(k => [k, D.counts.get(c + '|' + k)]));
            const s = schemaFromDraft();
            out.field = s.fields[c];
            out.split = scSplitPattern(out.field.constraints.pattern);
            out.plain = scSplitPattern('(?![\\\\s\\\\S]*(?: {{2}}))x|y');      // pas écrit ainsi par csvfab : un motif tel quel
            const v = await schemaValidate(t, s);
            out.items = Object.fromEntries(v.items.map(i => [i.rule, [i.count, i.label, i.examples]]));
            closeAllModals();
            return out;
        }})()""", timeout=60)
        self.assertEqual(r["before"], [True, True, True])
        self.assertEqual(r["after"], [False, False])
        self.assertEqual(r["counts"], {"noInvisible": 1, "noDoubleSpace": 1, "noMojibake": 1, "capitalised": 2, "pattern": 0})
        f = r["field"]
        self.assertEqual(set(f), {"name", "type", "constraints"})          # rien d'autre que du Table Schema
        self.assertEqual(r["split"], {"base": "[\\s\\S]{1,20}", "checks": ["noInvisible", "noDoubleSpace", "noMojibake", "capitalised"]})
        self.assertEqual(r["plain"], {"base": "(?![\\s\\S]*(?: {2}))x|y", "checks": []})
        self.assertEqual({k: v[0] for k, v in r["items"].items()}, {"noInvisible": 1, "noDoubleSpace": 1, "noMojibake": 1, "capitalised": 2})
        self.assertEqual(sorted(r["items"]["capitalised"][2]), ["DUPONT", "dupont"])
        self.assertEqual(r["items"]["noMojibake"][1], "holds garbled accents (mojibake)")
        import re
        motif = re.compile(f["constraints"]["pattern"])
        self.assertEqual([n for n in noms if motif.fullmatch(n)], ["Dupont", "Boulogne-sur-Mer", "Jean-Pierre"])
