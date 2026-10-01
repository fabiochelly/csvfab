"""SQLite : ouvrir une table comme CSV, ecrire un CSV comme base.

sqlite3 est dans la bibliotheque standard : rien a installer. Importe a la demande, comme
le reste des conversions, pour ne pas ralentir le demarrage du serveur.
"""

import csv
import os
import re
import urllib.parse

from .common import number_text

SQLITE_MAGIC = b"SQLite format 3\x00"


def _sqlite_ro(path):
    """Connexion en lecture seule : ouvrir une base pour la lire ne doit jamais l'ecrire
    (ni creer un journal a cote)."""
    import sqlite3
    with open(path, "rb") as f:
        if f.read(16) != SQLITE_MAGIC:
            raise ValueError("ce n'est pas une base SQLite")
    return sqlite3.connect("file:" + urllib.parse.quote(os.path.abspath(path)) + "?mode=ro&immutable=1", uri=True)


def _sql_ident(name):
    return '"' + str(name).replace('"', '""') + '"'


def table_list(path):
    """Tables et vues d'une base : {name, view, hidden, filled, dim: [lignes, colonnes]},
    au format des feuilles d'un classeur, pour que la page propose le meme choix."""
    con = _sqlite_ro(path)
    try:
        out = []
        for name, typ in con.execute("SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') "
                                     "AND name NOT LIKE 'sqlite_%' ORDER BY type = 'view', name"):
            q = _sql_ident(name)
            try:
                cols = len(con.execute(f"SELECT * FROM {q} LIMIT 0").description or [])
                n = con.execute(f"SELECT COUNT(*) FROM {q}").fetchone()[0]
            except Exception:                    # une vue cassee (table disparue) : listee, vide
                cols, n = 0, 0
            out.append({"name": name, "view": typ == "view", "hidden": False, "filled": n > 0, "dim": [n, cols]})
        return out
    finally:
        con.close()


def to_csv(src, out, delim, table=None):
    """Une table (ou vue) de src -> CSV dans out, la ligne d'en-tete d'abord ; sans table nommee,
    la premiere table non vide. Nombres comme dans un classeur (virgule decimale dans un
    fichier en ;), NULL vide, BLOB en hexadecimal (0x…)."""
    tables = table_list(src)
    if not tables:
        raise ValueError("la base ne contient aucune table")
    pick = next((t for t in tables if t["name"] == table), None) if table else \
        next((t for t in tables if t["filled"] and not t["view"]), None) or tables[0]
    if not pick:
        raise ValueError(f"table introuvable : {table}")
    comma = delim == ";"

    def cell(v):
        if v is None:
            return ""
        if isinstance(v, float):
            return number_text(repr(v), comma)
        if isinstance(v, (bytes, memoryview)):
            return "0x" + bytes(v).hex()
        return str(v)
    con = _sqlite_ro(src)
    try:
        cur = con.execute(f"SELECT * FROM {_sql_ident(pick['name'])}")
        names = [d[0] for d in cur.description]
        w = csv.writer(out, delimiter=delim, lineterminator="\n")
        w.writerow(names)
        rows = 0
        while True:
            batch = cur.fetchmany(10000)
            if not batch:
                break
            w.writerows([cell(v) for v in r] for r in batch)
            rows += len(batch)
    finally:
        con.close()
    others = sum(1 for t in tables if t is not pick and t["filled"])
    return {"table": pick["name"], "others": others, "rows": rows, "cols": len(names)}


_SQL_INT = re.compile(r"^-?(0|[1-9]\d{0,14})$")
_SQL_CODE = re.compile(r"^(0\d+|\d{16,})$")       # un zero en tete ou plus de 15 chiffres : un code, pas un nombre
_SQL_NUM = (re.compile(r"^-?\d+([.,]\d+)?$"), re.compile(r"^-?\d{1,3}(,\d{3})+(\.\d+)?$"), re.compile(r"^-?\d{1,3}(\.\d{3})+(,\d+)?$"))


def sql_real(v):
    """Un nombre ecrit a la francaise ou a l'anglaise (1 234,50 · 1,234.50 · 12,5) -> float, sinon None."""
    x = v.replace(" ", "").replace(" ", "").replace(" ", "")
    if not any(r.match(x) for r in _SQL_NUM):
        return None
    if "," in x and "." in x:
        x = x.replace(",", "") if x.rfind(".") > x.rfind(",") else x.replace(".", "").replace(",", ".")
    elif _SQL_NUM[1].match(x):
        x = x.replace(",", "")
    elif _SQL_NUM[2].match(x):
        x = x.replace(".", "")
    else:
        x = x.replace(",", ".")
    try:
        return float(x)
    except ValueError:
        return None


def write(open_rows, out_path, table, header=True):
    """Le CSV relu deux fois (open_rows : un csv.reader neuf a chaque appel) : d'abord le type de
    chaque colonne — INTEGER quand toutes ses valeurs sont des entiers (sans zero en tete : un code
    postal reste du texte), REAL quand toutes sont des nombres, TEXT sinon —, puis l'ecriture,
    les cellules vides en NULL. Une seule table, nommee d'apres le fichier."""
    import sqlite3
    names, kinds, width = None, None, 0
    for k, row in enumerate(open_rows()):
        if k == 0 and header:
            names = row
            continue
        if kinds is None:
            kinds = []
        width = max(width, len(row))
        while len(kinds) < width:
            kinds.append([0, 0, 0, 0])           # valeurs, entiers, nombres, codes
        for c, v in enumerate(row):
            v = v.strip()
            if not v:
                continue
            kc = kinds[c]
            kc[0] += 1
            if _SQL_CODE.match(v):               # 01000, 0612345678, un identifiant de 16 chiffres : du texte
                kc[3] += 1
            elif _SQL_INT.match(v):
                kc[1] += 1
                kc[2] += 1
            elif sql_real(v) is not None:
                kc[2] += 1
    kinds = kinds or []
    width = max(width, len(names or []))
    while len(kinds) < width:
        kinds.append([0, 0, 0, 0])
    types = ["TEXT" if kc[3] else "INTEGER" if kc[0] and kc[1] == kc[0] else "REAL" if kc[0] and kc[2] == kc[0] else "TEXT" for kc in kinds]
    cols, seen = [], {}
    for c in range(width):
        n = ((names[c] if names and c < len(names) else "") or "").strip() or f"col{c + 1}"
        base, i = n, 2
        while n.lower() in seen:
            n, i = f"{base}_{i}", i + 1
        seen[n.lower()] = 1
        cols.append(n)
    if os.path.exists(out_path):
        os.unlink(out_path)
    con = sqlite3.connect(out_path)
    rows = 0
    try:
        con.execute(f"CREATE TABLE {_sql_ident(table)} (" + ", ".join(f"{_sql_ident(n)} {t}" for n, t in zip(cols, types)) + ")")
        ins = f"INSERT INTO {_sql_ident(table)} VALUES (" + ", ".join("?" * width) + ")"

        def conv(row):
            out = []
            for c in range(width):
                v = row[c].strip() if c < len(row) else ""
                if not v:
                    out.append(None)
                elif types[c] == "INTEGER":
                    out.append(int(v))
                elif types[c] == "REAL":
                    out.append(sql_real(v))
                else:
                    out.append(row[c])           # le texte tel qu'ecrit, espaces compris
            return out
        batch = []
        for k, row in enumerate(open_rows()):
            if k == 0 and header:
                continue
            batch.append(conv(row))
            if len(batch) >= 10000:
                con.executemany(ins, batch)
                rows += len(batch)
                batch = []
        if batch:
            con.executemany(ins, batch)
            rows += len(batch)
        con.commit()
    finally:
        con.close()
    return {"rows": rows, "cols": width, "table": table, "types": dict(zip(cols, types))}
