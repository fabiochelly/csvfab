"""Réglages partagés par le lanceur (csvfab.py) et le serveur.

N'importe que os et sys : le lanceur le charge avant de démarrer Chromium, et
chaque milliseconde passée avant compte dans l'ouverture de la fenêtre.
"""

import os
import sys

VERSION = "1.14.0"
HOST = "127.0.0.1"
WINDOWS = os.name == "nt"
MACOS = sys.platform == "darwin"
# Le dossier de l'app (viewer.htm, ui/, csvfab.py), parent de ce paquet.
APP_DIR = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))


def env(name, default=None):
    """CSVFAB_<name> ; l'ancien nom CSV_EDITOR_<name> reste lu (avant le renommage en 1.0.0)."""
    return os.environ.get("CSVFAB_" + name) or os.environ.get("CSV_EDITOR_" + name) or default


# Fixe : Chromium mémorise les permissions de fichiers par origine (donc par port).
PORT = int(env("PORT") or "8787")
DEBUG = bool(env("DEBUG"))


def state_base():
    if WINDOWS:
        return os.environ.get("LOCALAPPDATA") or os.path.expanduser("~\\AppData\\Local")
    if MACOS:
        return os.path.expanduser("~/Library/Application Support")
    return os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")


def state_dir():
    """Dossier d'état (jeton, journal, profil du navigateur) : « csvfab », ou l'ancien
    « csv-editor » tant que le lanceur ne l'a pas migré (il le renomme, profil
    Chromium et permissions compris)."""
    base = state_base()
    new, old = os.path.join(base, "csvfab"), os.path.join(base, "csv-editor")
    return old if not os.path.exists(new) and os.path.isdir(old) else new
