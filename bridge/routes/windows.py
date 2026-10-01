"""Les fenêtres : ping du lanceur, file des chemins à ouvrir, sondage long, relancement."""

import json
import os
import select
import sys
import threading
import time

from .. import config
from ..httpd import HttpError
from ..session import POLL_MAX


def ping(req):
    """Sans jeton : le lanceur s'en sert pour savoir si un serveur tourne, et si une
    fenêtre l'interroge encore ("client") — auquel cas il n'en ouvre pas d'autre."""
    return {"ok": True, "pid": os.getpid(), "version": config.VERSION, "client": req.server.session.alive()}


def pending(req):
    """Sondage long : la réponse part dès qu'un chemin est déposé, sinon après
    ?wait= secondes (la page relance aussitôt). Sans cela un fichier ouvert alors
    que l'app tournait déjà attendait le sondage suivant, jusqu'à 1,5 s."""
    try:
        wait = float(req.arg("wait", "0"))
    except ValueError:
        wait = 0.0
    wait = min(wait, POLL_MAX) if wait > 0 else 0.0           # nan et négatifs compris
    # Fenêtre fermée pendant l'attente ? Son socket devient lisible (fin de flux).
    paths = req.server.session.wait(wait, lambda: select.select([req.connection], [], [], 0)[0])
    if paths is None:
        return
    try:
        req.json(200, {"paths": paths})
    except (BrokenPipeError, ConnectionResetError):
        pass


def _paths(req):
    try:
        payload = json.loads(req.rfile.read(req.content_length()) or b"{}")
    except (ValueError, UnicodeDecodeError):
        raise HttpError(400, "bad json")
    if not isinstance(payload, dict):
        raise HttpError(400, "bad json")
    return [os.path.realpath(os.path.expanduser(p))
            for p in payload.get("paths") or [] if isinstance(p, str) and p]


def open_paths(req):
    """Le lanceur dépose ici les fichiers reçus en ligne de commande."""
    paths = _paths(req)
    session = req.server.session
    session.push(paths)
    return {"queued": len(paths), "client": session.alive()}


def relaunch(req):
    relaunch_after_close(req.server.session, [p for p in _paths(req) if os.path.isfile(p)])
    return {"ok": True}


def relaunch_after_close(session, paths):
    """Rouvre l'app une fois sa fenêtre fermée (une police installée n'est vue par
    Chromium qu'à son démarrage) : attend que plus aucune fenêtre n'interroge,
    remet les fichiers ouverts en file, puis lance le lanceur, qui ouvre une
    fenêtre qui les reprend."""
    def run():
        deadline = time.time() + 20
        while time.time() < deadline and session.polling():
            time.sleep(0.1)
        time.sleep(0.3)
        session.hand_over(paths)
        import subprocess
        kw = {"creationflags": 0x00000008} if os.name == "nt" else {"start_new_session": True}   # DETACHED_PROCESS / setsid
        subprocess.Popen([sys.executable, os.path.join(config.APP_DIR, "csvfab.py")], stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kw)
    threading.Thread(target=run, daemon=True).start()
