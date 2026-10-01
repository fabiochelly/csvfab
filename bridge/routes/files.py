"""Lire, décrire et réécrire un fichier par son chemin (les onglets ouverts depuis la ligne de commande)."""

import os

from .. import fsio
from ..httpd import HttpError


def stat(req):
    p = req.path_arg()
    if not p or not os.path.isfile(p):
        raise HttpError(404, "not a file")
    st = os.stat(p)
    return {"name": os.path.basename(p), "path": p, "size": st.st_size, "mtime": st.st_mtime,
            "writable": os.access(p, os.W_OK)}


def read(req):
    """Les octets du fichier tels quels ; ?from=N seulement ceux qui suivent l'octet N
    (le suivi d'un journal, tail -f : la page ne relit que ce qui a été ajouté)."""
    p = req.path_arg()
    if not p or not os.path.isfile(p):
        raise HttpError(404, "not a file", text=True)
    try:
        start = max(0, int(req.arg("from", "0")))
    except ValueError:
        start = 0
    try:
        with open(p, "rb") as f:
            size = os.fstat(f.fileno()).st_size
            start = min(start, size)
            req.send_response(200)
            req.send_header("Content-Type", "text/csv; charset=utf-8")
            req.send_header("Content-Length", str(size - start))
            req.send_header("X-File-Size", str(size))
            req.send_header("Cache-Control", "no-store")
            req.end_headers()
            if req.command != "HEAD" and size > start:
                # sendfile : du fichier au socket sans passer par Python (os.sendfile
                # sous Linux et macOS, une boucle d'envois ailleurs). Et jamais plus
                # que Content-Length, même si le fichier grandit entre-temps (un
                # journal suivi) : la connexion gardée ouverte resterait sinon
                # désynchronisée.
                req.connection.sendfile(f, start, size - start)
    except OSError as e:
        try:
            req.send(500, f"lecture impossible : {e}")
        except Exception:
            pass


def write(req):
    """Écriture atomique (bridge.fsio) ; ?backup=1 copie d'abord le fichier en .bak
    (une fois par onglet et par session : c'est la page qui décide)."""
    p = req.path_arg()
    if not p:
        raise HttpError(400, "chemin manquant")
    d = os.path.dirname(p)
    if not os.path.isdir(d):
        raise HttpError(400, f"dossier inexistant : {d}")
    n = req.content_length()
    made = None
    try:
        if req.arg("backup") == "1":
            made = fsio.backup(p)
        with fsio.replacing(p, durable=True) as tmp:
            with open(tmp, "wb") as f:
                fsio.receive(req.rfile, n, f)
                fsio.fsync(f)
    except Exception as e:
        raise HttpError(500, str(e))
    return {"ok": True, "bytes": n, "backup": os.path.basename(made) if made else None,
            "size": os.path.getsize(p)}
