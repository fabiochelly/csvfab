"""Démarrage du serveur : port, jeton, surveillance des fenêtres."""

import os
import sys
import threading
import time

from . import config
from .httpd import Handler, Server
from .session import Session


def watchdog(srv, period=2.0):
    """Éteint le serveur quand plus aucune fenêtre ne se manifeste."""
    while True:
        time.sleep(period)
        if srv.session.should_stop():
            srv.shutdown()
            return


def make_server(early=None, port=None, state=None, session=None):
    """Le serveur prêt à servir. early : le socket déjà ouvert par server.py, dont
    server_bind() — et son socket.getfqdn(), une résolution de nom inutile ici —
    est alors sauté. Lève OSError si le port est pris."""
    state = state or config.state_dir()
    if early is not None:
        srv = Server((config.HOST, config.PORT), Handler, bind_and_activate=False)
        srv.socket.close()
        srv.socket = early
        srv.server_address = early.getsockname()
        srv.server_name, srv.server_port = config.HOST, srv.server_address[1]
    else:
        srv = Server((config.HOST, config.PORT if port is None else port), Handler)
    return srv.setup_app(session or Session(), state)


def main(early=None):
    state = config.state_dir()
    os.makedirs(state, exist_ok=True)
    try:
        srv = make_server(early, state=state)
    except OSError as e:
        # Déjà occupé : un serveur tourne probablement, le lanceur s'en sert.
        print(f"port {config.PORT} indisponible : {e}", file=sys.stderr)
        return 1

    tok = os.path.join(state, "token")
    fd = os.open(tok, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(f"{srv.server_address[1]}\n{srv.token}\n")

    threading.Thread(target=watchdog, args=(srv,), daemon=True).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        srv.server_close()
        try:
            os.unlink(tok)
        except OSError:
            pass
    return 0
