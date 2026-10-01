#!/usr/bin/env python3
"""Pont fichier local de l'éditeur CSV — point d'entrée ; le code est dans bridge/.

Pourquoi ce serveur existe : une page web ne peut pas ouvrir un chemin reçu en
ligne de commande — l'API File System Access n'accorde de handle qu'au travers
d'un sélecteur ou d'un drag & drop. Pour qu'un « Ouvrir avec » venu de yazi ou
d'un .desktop fonctionne, le contenu doit donc transiter par ici : le serveur
lit et réécrit le fichier à la place du navigateur.

Sécurité (jeton, Origin) : voir bridge/httpd.py. Arrêt : le serveur s'éteint
seul dès que plus aucune fenêtre ne l'interroge (bridge/session.py).
"""

import os
import socket
import sys

# Le port est ouvert avant tout le reste (imports, définitions : des dizaines
# de ms) : le lanceur démarre Chromium dès qu'il accepte des connexions, et
# une requête arrivée entre-temps attend dans la file d'écoute au lieu d'être
# refusée. Un port déjà pris signale, comme avant, un serveur déjà lancé.
# Le port est lu ici à la main plutôt que par bridge.config : rien ne doit
# précéder l'ouverture (même règle que config.PORT).
_EARLY = None
if __name__ == "__main__":
    _port = int(os.environ.get("CSVFAB_PORT") or os.environ.get("CSV_EDITOR_PORT") or "8787")
    _EARLY = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    if os.name != "nt":            # voir bridge.httpd.Server.allow_reuse_address
        _EARLY.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        _EARLY.bind(("127.0.0.1", _port))
        _EARLY.listen(64)
    except OSError as e:
        print(f"port {_port} indisponible : {e}", file=sys.stderr)
        sys.exit(1)

HERE = os.path.dirname(os.path.realpath(__file__))
if ".app/Contents/" in HERE:
    # Dans le paquet macOS (signé) : ne jamais y écrire de __pycache__, ce qui
    # en invaliderait la signature ; build-app.sh l'a précompilé.
    sys.dont_write_bytecode = True
if sys.path[:1] != [HERE]:
    sys.path.insert(0, HERE)

from bridge.app import main  # noqa: E402

if __name__ == "__main__":
    sys.exit(main(_EARLY))
