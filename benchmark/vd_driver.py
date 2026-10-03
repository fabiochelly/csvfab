"""Les mêmes opérations dans VisiData, mesurées dans son propre interpréteur.

Lancé par run.py avec le Python où VisiData est installé (ou `uv run --with
visidata`) : python vd_driver.py <fichier.csv>. Rend une ligne JSON.

Chaque opération passe par ce que fait la commande correspondante de VisiData,
et attend la fin de ses fils (vd.sync()) avant d'arrêter le chrono :
  ouverture    vd.openSource(…) puis chargement complet (la barre de progression à 100 %)
  filtre       `|` (select-col-regex) sur city, « lyon » (casse ignorée par défaut)
  tri nombre   `#` (type-float) puis `]` (sort-desc) sur total_spent
  tri texte    `[` (sort-asc) sur last_name
Ce qu'il faut dessiner à l'écran n'est pas compté (pas de curses ici).
"""

import json
import sys
import time

import visidata
from visidata import vd

# Sans interface, pas de commande où accrocher l'annulation : coupée. Elle ne copie
# que la liste des lignes avant un tri (quelques ms) — VisiData y gagne, s'il y gagne.
vd.options.undo = False


def timed(fn):
    t0 = time.perf_counter()
    out = fn()
    vd.sync()
    return time.perf_counter() - t0, out


def main(path):
    res = {"version": visidata.__version__}
    sheet = {}

    def load():
        vs = vd.openSource(visidata.Path(path), filetype="csv")
        vs.ensureLoaded()
        sheet["vs"] = vs

    res["open"], _ = timed(load)
    vs = sheet["vs"]
    res["rows"] = len(vs.rows)

    city = vs.column("city")

    def select():
        vs.selectByIdx(list(vd.searchRegex(vs, regex="lyon", columns=city)))

    res["filter"], _ = timed(select)
    res["filter_n"] = vs.nSelectedRows

    spent = vs.column("total_spent")

    def sort_num():
        spent.type = float
        vd.clearCaches()     # `ordering` est mis en cache ; la boucle de l'interface le vide après chaque commande
        vs.orderBy(None, spent, reverse=True, save_cmd_input=False)

    res["sort_num"], _ = timed(sort_num)
    res["sort_num_first"] = spent.getValue(vs.rows[0])

    last = vs.column("last_name")

    def sort_text():
        vd.clearCaches()
        vs.orderBy(None, last, reverse=False, save_cmd_input=False)

    res["sort_text"], _ = timed(sort_text)
    res["sort_text_first"] = last.getValue(vs.rows[0])
    print(json.dumps(res))


if __name__ == "__main__":
    main(sys.argv[1])
