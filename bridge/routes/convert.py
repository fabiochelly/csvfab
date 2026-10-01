"""Conversions : CSV -> classeur xlsx ou base SQLite (export), xlsx ou SQLite -> CSV (ouverture).

L'éditeur n'édite que des CSV. Un classeur ou une base ouverts sont d'abord
écrits en CSV à côté d'eux ; à l'export, la page envoie le même CSV que pour
une sauvegarde, que le serveur relit pour le convertir. Les corps de requête
passent par un fichier de travail : un CSV de 500 Mo n'a pas à tenir en RAM.
"""

import io
import json
import os

from .. import fsio
from ..httpd import HttpError


def _formats(kind):
    """(lister, convertir en CSV, écrire depuis un CSV, erreurs de lecture, libellé), importés à la demande."""
    if kind == "xlsx":
        import xml.etree.ElementTree as ET
        import zipfile
        from ..formats import xlsx_reader, xlsx_writer
        return (xlsx_reader.sheet_list, xlsx_reader.to_csv, xlsx_writer.write,
                (zipfile.BadZipFile, KeyError, ET.ParseError, ValueError, IndexError), "un classeur xlsx")
    import sqlite3
    from ..formats import sqlite_io
    return sqlite_io.table_list, sqlite_io.to_csv, sqlite_io.write, (sqlite3.Error, ValueError), "une base SQLite"


MIME = {"xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "sqlite": "application/vnd.sqlite3"}


def _delim(req, default):
    delim = req.arg("delim", default)
    if len(delim) != 1:
        raise HttpError(400, "delimiteur invalide")
    return delim


# --- listes de feuilles / tables -------------------------------------------
def _listing(req, kind):
    p = req.path_arg()
    if not p or not os.path.isfile(p):
        raise HttpError(404, "not a file")
    lister, _, _, errors, label = _formats(kind)
    try:
        return {"sheets": lister(p)}
    except errors as e:
        raise HttpError(400, f"ce n'est pas {label} lisible ({e})")


def xlsx_sheets(req):
    """Les feuilles d'un classeur (?path=), vides et masquées comprises : la page
    ne propose un choix que s'il y en a plusieurs à garder."""
    return _listing(req, "xlsx")


def sqlite_tables(req):
    """Les tables et vues d'une base (?path=), au format des feuilles d'un classeur."""
    return _listing(req, "sqlite")


# --- CSV -> classeur / base --------------------------------------------------
def _from_csv(req, kind):
    """Avec ?path=, le fichier est écrit là, atomiquement ; sinon ses octets sont
    renvoyés, pour que la page les dépose elle-même via un handle."""
    delim = _delim(req, ",")
    dest = req.path_arg()
    sheet = req.arg("sheet") or (os.path.splitext(os.path.basename(dest))[0] if dest else "Sheet1")
    n = req.content_length()
    d = os.path.dirname(dest) if dest else req.server.state
    if dest and not os.path.isdir(d):
        raise HttpError(400, f"dossier inexistant : {d}")
    header = req.arg("header", "1") != "0"
    writer = _formats(kind)[2]
    import csv
    try:
        with fsio.scratch(d, ".csv") as src:
            with open(src, "wb") as f:
                fsio.receive(req.rfile, n, f)

            def open_rows():                       # relu deux fois : analyse, puis écriture
                with open(src, "r", encoding="utf-8", newline="") as f:
                    yield from csv.reader(f, delimiter=delim)

            if dest:
                with fsio.replacing(dest) as tmp:
                    info = writer(open_rows, tmp, sheet, header)
                info["path"] = dest
                info["ok"] = True
                return info
            with fsio.scratch(d, "." + kind) as tmp:
                info = writer(open_rows, tmp, sheet, header)
                with open(tmp, "rb") as f:
                    body = f.read()
    except HttpError:
        raise
    except Exception as e:
        raise HttpError(500, str(e))
    req.send(200, body, MIME[kind], {"X-Xlsx-Info": json.dumps(info)})


def csv_to_xlsx(req):
    return _from_csv(req, "xlsx")


def csv_to_sqlite(req):
    return _from_csv(req, "sqlite")


# --- classeur / base -> CSV --------------------------------------------------
def _to_csv(req, kind):
    """Le fichier par ?src= (un chemin), sinon dans le corps. ?sheet= nomme la
    feuille ou la table. Avec ?dest=, le CSV est écrit là, atomiquement (et
    ?backup=1 garde une copie .bak d'un fichier déjà présent) ; sinon ses octets
    sont renvoyés, avec le bilan dans X-Xlsx-Info, pour que la page les dépose.

    ?pick=1 sans ?sheet= : si plusieurs feuilles visibles portent des valeurs, rien
    n'est converti, la liste revient pour que la page demande laquelle. Un seul envoi
    du classeur dans le cas courant (une feuille), deux seulement s'il faut choisir."""
    delim = _delim(req, ";")
    src, dest = req.path_arg("src"), req.path_arg("dest")
    backup = req.arg("backup") == "1"
    sheet = req.arg("sheet") or None
    pick = req.arg("pick") == "1" and not sheet
    if src and not os.path.isfile(src):
        raise HttpError(404, "fichier introuvable")
    d = os.path.dirname(dest) if dest else req.server.state
    if dest and not os.path.isdir(d):
        raise HttpError(400, f"dossier inexistant : {d}")
    lister, convert, _, errors, label = _formats(kind)
    try:
        with fsio.scratch(d, "." + kind) as tmp_in:
            if not src:
                n = req.content_length()
                with open(tmp_in, "wb") as f:
                    fsio.receive(req.rfile, n, f)
            source = src or tmp_in
            try:
                if pick:
                    sheets = lister(source)
                    if sum(1 for s in sheets if s["filled"] and not s["hidden"]) > 1:
                        return {"choose": sheets}
                if dest:
                    with fsio.replacing(dest) as tmp_out:
                        with open(tmp_out, "w", encoding="utf-8", newline="") as out:
                            info = convert(source, out, delim, sheet)
                            fsio.fsync(out)
                        if backup:
                            fsio.backup(dest)
                    info.update(ok=True, path=dest, size=os.path.getsize(dest))
                    return info
                out = io.StringIO()
                info = convert(source, out, delim, sheet)
            except errors as e:
                raise HttpError(400, f"ce n'est pas {label} lisible ({e})")
    except HttpError:
        raise
    except Exception as e:
        raise HttpError(500, str(e))
    req.send(200, out.getvalue().encode("utf-8"), "text/csv; charset=utf-8", {"X-Xlsx-Info": json.dumps(info)})


def xlsx_to_csv(req):
    return _to_csv(req, "xlsx")


def sqlite_to_csv(req):
    return _to_csv(req, "sqlite")
