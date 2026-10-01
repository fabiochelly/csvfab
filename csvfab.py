#!/usr/bin/env python3
"""csvfab — lanceur de l'éditeur CSV en application de bureau (Linux, macOS, Windows).

Rôle : garantir qu'un serveur local tourne, lui confier les chemins reçus en
argument, et n'ouvrir une fenêtre que s'il n'y en a pas déjà une. Une page web
ne peut pas ouvrir un chemin venu de la ligne de commande — l'API File System
Access n'accorde de handle qu'au travers d'un sélecteur — donc le fichier
transite par server.py, qui le lit et le réécrit pour elle.

Aucune console : le navigateur est lancé en mode --app (ni barre d'adresse, ni
onglets), détaché du processus appelant, et le serveur écrit dans un journal.
Les erreurs passent donc par une notification (notify-send, osascript, ou une
boîte de message sous Windows), seul canal visible sans terminal.

Python plutôt que bash : c'est déjà une dépendance du serveur, et c'est la
seule façon d'avoir un même lanceur sur les trois systèmes. Sous Windows,
csvfab.cmd le lance avec pythonw (pas de fenêtre de console). Installé, ce fichier
csvfab.py est exposé sous le nom de commande « csvfab » (lien ou raccourci).

    csvfab [fichier.csv …]     ouvre les fichiers (dans la fenêtre déjà ouverte s'il y en a une)
    csvfab --version
"""

import os
import shutil
import socket
import subprocess
import sys
import time

VERSION = "1.9.0"
HERE = os.path.dirname(os.path.realpath(__file__))   # suit le lien ~/.local/bin ou /usr/bin
# CSVFAB_* ; les anciens noms CSV_EDITOR_* restent lus.
PORT = int(os.environ.get("CSVFAB_PORT") or os.environ.get("CSV_EDITOR_PORT") or "8787")
URL = f"http://127.0.0.1:{PORT}/"
WINDOWS = os.name == "nt"
MACOS = sys.platform == "darwin"


def state_base():
    if WINDOWS:
        return os.environ.get("LOCALAPPDATA") or os.path.expanduser("~\\AppData\\Local")
    if MACOS:
        return os.path.expanduser("~/Library/Application Support")
    return os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")


def state_dir():
    """Même choix que state_dir() de server.py : c'est là qu'il dépose le jeton.
    « csvfab », ou l'ancien « csv-editor » tant qu'il n'a pas été migré."""
    new, old = os.path.join(state_base(), "csvfab"), os.path.join(state_base(), "csv-editor")
    return old if not os.path.exists(new) and os.path.isdir(old) else new


def migrate_state():
    """Renomme l'ancien dossier d'état csv-editor en csvfab — profil Chromium et
    permissions de fichiers compris. Seulement quand aucun serveur ne tourne :
    un profil en cours d'usage ne se déplace pas sous les pieds du navigateur."""
    global STATE, PROFILE
    new, old = os.path.join(state_base(), "csvfab"), os.path.join(state_base(), "csv-editor")
    if os.path.exists(new) or not os.path.isdir(old) or ping():
        return
    try:
        os.rename(old, new)
    except OSError:
        return
    STATE, PROFILE = new, os.path.join(new, "profile")


STATE = state_dir()
# Profil dédié : les permissions de fichiers accordées au navigateur sont
# mémorisées par origine ET par profil, donc l'app garde les siennes sans
# polluer le navigateur principal (c'est aussi pourquoi le port est fixe).
PROFILE = os.path.join(STATE, "profile")

_http = None


def http():
    """urllib et json sont importés à la demande : ~20 ms d'imports qui, au
    démarrage à froid, retardaient d'autant le lancement de Chromium."""
    global _http
    if _http is None:
        import urllib.request
        # Pas de proxy pour 127.0.0.1, même si l'environnement en déclare un.
        _http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return _http


def notify(msg):
    try:
        if WINDOWS:
            import ctypes
            ctypes.windll.user32.MessageBoxW(None, msg, "csvfab", 0x10)
        elif MACOS:
            import json
            text = json.dumps(msg, ensure_ascii=False)   # guillemets et \\ échappés
            subprocess.run(["osascript", "-e", f'display notification {text} with title "csvfab"'],
                           timeout=5, capture_output=True)
        elif shutil.which("notify-send"):
            subprocess.run(["notify-send", "-u", "critical", "csvfab", msg],
                           timeout=5, capture_output=True)
    except Exception:
        pass


def die(msg):
    notify(msg)
    print(f"csvfab: {msg}", file=sys.stderr)
    sys.exit(1)


def detached():
    """Le processus lancé survit à la fermeture de yazi, du terminal ou du Finder."""
    if WINDOWS:
        return {"creationflags": subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}   # l'équivalent de setsid


def ping():
    if not port_open():              # le cas du démarrage à froid, sans urllib
        return None
    import json
    try:
        with http().open(URL + "api/ping", timeout=1) as r:
            return json.load(r)
    except Exception:
        return None


# --- 1. le serveur ---------------------------------------------------------
def port_open():
    try:
        with socket.create_connection(("127.0.0.1", PORT), timeout=0.2):
            return True
    except OSError:
        return False


def start_server():
    """Lance le serveur et rend la main dès que son port accepte des connexions
    (il l'ouvre avant ses imports) : Chromium peut démarrer en parallèle de la
    fin de son initialisation, ses requêtes attendront dans la file d'écoute."""
    os.makedirs(STATE, exist_ok=True)
    log = open(os.path.join(STATE, "server.log"), "ab")
    # sys.executable : le même Python que celui du lanceur (pythonw sous
    # Windows, donc pas de console non plus pour le serveur).
    subprocess.Popen([sys.executable or "python3", os.path.join(HERE, "server.py")],
                     env={**os.environ, "CSVFAB_PORT": str(PORT)},
                     stdin=subprocess.DEVNULL, stdout=log, stderr=log, **detached())


def wait_server():
    for _ in range(1000):            # 5 s ; Python démarre plus lentement sous Windows
        if port_open():
            return
        time.sleep(0.005)
    die(f"le serveur local n'a pas démarré — voir {os.path.join(STATE, 'server.log')}")


def wait_ping():
    for _ in range(100):
        status = ping()
        if status:
            return status
        time.sleep(0.02)
    die(f"le serveur local ne répond pas — voir {os.path.join(STATE, 'server.log')}")


def read_token():
    try:
        with open(os.path.join(STATE, "token"), encoding="utf-8") as f:
            lines = f.read().splitlines()
        return lines[1].strip() if len(lines) > 1 else ""
    except OSError:
        return ""


# --- 2. les fichiers à ouvrir ----------------------------------------------
# Ils sont déposés dans la file du serveur ; la fenêtre (celle qui existe déjà,
# ou celle qu'on ouvre juste après) la vide et en fait des onglets.
def queue_paths(paths, token):
    import json, urllib.request
    body = json.dumps({"paths": [os.path.abspath(p) for p in paths]}).encode()
    req = urllib.request.Request(URL + "api/open", data=body, method="POST", headers={
        "X-Csv-Token": token, "Content-Type": "application/json"})
    try:
        with http().open(req, timeout=3) as r:
            r.read()
    except Exception:
        die("le serveur a refusé les fichiers")


# --- 3. la fenêtre ---------------------------------------------------------
def focus_existing():
    # Sous Wayland, Chromium ignore --class en mode --app et dérive l'app_id
    # de l'URL ; on vise donc les deux formes possibles. Ailleurs, le fichier
    # arrive en onglet dans la fenêtre existante, sans la mettre au premier plan.
    # Hyprland ≥ 0.56 n'accepte plus que des dispatchers Lua (hl.dsp.*) et
    # rejette l'ancienne forme « focuswindow class:… » : sans ce passage, le
    # fichier arrivait en onglet dans une fenêtre restée sur un autre bureau,
    # et rien ne semblait se passer. L'ancienne forme reste en repli.
    if shutil.which("hyprctl"):
        sel = r"class:^(chrome-127\.0\.0\.1.*|csvfab)$"
        lua = 'hl.dsp.focus({ window = "%s" })' % sel.replace("\\", "\\\\")
        r = subprocess.run(["hyprctl", "dispatch", lua], capture_output=True, text=True)
        if r.returncode != 0 or r.stdout.strip() != "ok":
            subprocess.run(["hyprctl", "dispatch", "focuswindow", sel], capture_output=True)


def find_browser():
    """Un navigateur Chromium : Firefox et Safari n'implémentent pas l'API File
    System Access, dont dépend l'ouverture manuelle de fichiers réinscriptibles."""
    forced = os.environ.get("CSVFAB_BROWSER") or os.environ.get("CSV_EDITOR_BROWSER")
    if forced:
        return forced
    if WINDOWS:
        # Edge est livré avec Windows : il y a donc toujours au moins lui.
        bases = [os.environ.get(v) for v in ("PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA")]
        for rel in (("Google", "Chrome", "Application", "chrome.exe"),
                    ("BraveSoftware", "Brave-Browser", "Application", "brave.exe"),
                    ("Chromium", "Application", "chrome.exe"),
                    ("Microsoft", "Edge", "Application", "msedge.exe")):
            for base in filter(None, bases):
                p = os.path.join(base, *rel)
                if os.path.isfile(p):
                    return p
        return shutil.which("chrome") or shutil.which("msedge")
    if MACOS:
        # Le binaire lui-même, pas « open -a » : c'est lui qui accepte nos options.
        for app in ("Google Chrome", "Chromium", "Brave Browser", "Microsoft Edge"):
            for root in ("/Applications", os.path.expanduser("~/Applications")):
                p = os.path.join(root, f"{app}.app", "Contents", "MacOS", app)
                if os.path.isfile(p):
                    return p
        return None
    for b in ("chromium", "brave", "google-chrome-stable", "chrome", "microsoft-edge"):
        if shutil.which(b):
            return b
    return None


def open_window():
    browser = find_browser()
    if not browser:
        die("aucun navigateur Chromium trouvé (Chrome, Chromium, Brave ou Edge)")
    args = [browser, f"--app={URL}", f"--user-data-dir={PROFILE}",
            "--no-first-run", "--no-default-browser-check",
            # Une fenêtre cachée (autre bureau) garde son rythme : Chromium y
            # ralentit sinon les minuteurs jusqu'à un par minute, et le serveur,
            # sans nouvelles pendant 30 s, s'arrêterait sous l'app ouverte.
            "--disable-background-timer-throttling", "--disable-renderer-backgrounding",
            "--disable-backgrounding-occluded-windows",
            # Rien d'utile à une page locale : ni composants téléchargés et tenus
            # à jour (130 Mo sur 146 dans le profil), ni extensions (celles que
            # chromium-flags.conf charge partout, ici dans notre page).
            "--disable-component-update", "--disable-background-networking", "--disable-extensions"]
    if not (WINDOWS or MACOS):
        args += ["--class=csvfab", "--name=csvfab",
                 # Sans trousseau : --password-store=gnome-libsecret (celui de
                 # chromium-flags.conf sous Omarchy) interroge le trousseau par
                 # D-Bus au démarrage, ~40 ms. Il ne chiffre que les cookies et
                 # mots de passe, dont une page locale n'a pas ; localStorage et
                 # IndexedDB (thème, dossier de sauvegarde) ne sont pas chiffrés.
                 "--password-store=basic"]
        # uwsm-app place l'appli dans son propre scope systemd, comme toute
        # appli lancée sous Omarchy ; sans lui on se contente du détachement.
        if shutil.which("uwsm-app"):
            args = ["uwsm-app", "--"] + args
    subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, **detached())


def main(paths):
    if paths[:1] in (["--version"], ["-V"]):
        print(f"csvfab {VERSION}")
        return
    if paths[:1] in (["--help"], ["-h"]):
        print(__doc__)
        return
    migrate_state()
    status = ping()
    if not status:
        # Pas de serveur, donc pas de fenêtre. Chromium est lancé sans attendre
        # que le serveur ouvre son port (~15 ms) : il lui faut ~250 ms avant sa
        # première requête. Sauf sous Windows, où Python démarre parfois plus
        # lentement que Chromium — une page « connexion refusée » ne réessaie pas.
        start_server()
        if WINDOWS:
            wait_server()
        open_window()
        wait_server()
        wait_ping()
    token = read_token()
    if not token:
        die(f"jeton introuvable dans {os.path.join(STATE, 'token')}")
    if paths:
        queue_paths(paths, token)
    # "client": true <=> une fenêtre interroge encore le serveur : inutile d'en
    # ouvrir une seconde, le fichier y apparaîtra en onglet.
    if status and status.get("client"):
        focus_existing()
    elif status:
        open_window()


if __name__ == "__main__":
    main(sys.argv[1:])
