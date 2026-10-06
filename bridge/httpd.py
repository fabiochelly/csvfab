"""Le serveur HTTP : plomberie des réponses, jeton, et la table des routes.

Sécurité : écoute uniquement sur 127.0.0.1 et toute route /api/ (sauf ping)
exige le jeton tiré au démarrage — vérifié avant même de chercher la route,
donc une requête sans jeton n'apprend rien, pas même quelles routes existent.
Ce jeton est injecté dans la page servie (il n'apparaît donc jamais dans
l'URL, ni dans l'historique) et déposé dans le dossier d'état pour le
lanceur. Aucun en-tête CORS n'est émis et toute requête portant un Origin
étranger est refusée : une page tierce ne peut ni lire nos réponses ni écrire
un fichier.

Une route est une fonction f(req) d'un module de bridge.routes, importé à sa
première requête : les conversions (zipfile, ElementTree, csv, sqlite3) ne
coûtent rien au démarrage. Elle répond elle-même (req.send, req.json), ou
renvoie un dict (réponse JSON 200), ou lève HttpError.
"""

import http.server
import importlib
import json
import os
import secrets
import sys
import urllib.parse

from . import config

# (méthode, chemin) -> (module de bridge.routes, fonction). HEAD suit GET.
ROUTES = {
    ("GET", "/"): ("assets", "page"),
    ("GET", "/viewer.htm"): ("assets", "page"),
    ("GET", "/index.html"): ("assets", "page"),
    ("GET", "/app.js"): ("assets", "app_js"),

    ("GET", "/api/ping"): ("windows", "ping"),
    ("GET", "/api/pending"): ("windows", "pending"),
    ("POST", "/api/open"): ("windows", "open_paths"),
    ("POST", "/api/relaunch"): ("windows", "relaunch"),

    ("GET", "/api/stat"): ("files", "stat"),
    ("GET", "/api/file"): ("files", "read"),
    ("PUT", "/api/file"): ("files", "write"),
    ("POST", "/api/mkdir"): ("files", "mkdir"),

    ("POST", "/api/xlsx"): ("convert", "csv_to_xlsx"),
    ("POST", "/api/sqlite"): ("convert", "csv_to_sqlite"),
    ("POST", "/api/xlsx2csv"): ("convert", "xlsx_to_csv"),
    ("POST", "/api/sqlite2csv"): ("convert", "sqlite_to_csv"),
    ("GET", "/api/xlsx-sheets"): ("convert", "xlsx_sheets"),
    ("GET", "/api/sqlite-tables"): ("convert", "sqlite_tables"),

    ("GET", "/api/fonts"): ("fonts", "listing"),
    ("POST", "/api/font-install"): ("fonts", "install"),
}
# Toute autre requête GET hors de /api/ : les fichiers de l'app (styles, scripts, icônes).
STATIC = ("assets", "static")
PUBLIC = frozenset({"/api/ping"})       # sans jeton : le lanceur s'en sert pour savoir si un serveur tourne
_resolved = {}


def _resolve(spec):
    fn = _resolved.get(spec)
    if fn is None:
        fn = _resolved[spec] = getattr(importlib.import_module("bridge.routes." + spec[0]), spec[1])
    return fn


class HttpError(Exception):
    """Réponse d'erreur levée par une route : {"error": message} en JSON, ou texte brut."""

    def __init__(self, code, message, text=False):
        super().__init__(message)
        self.code, self.message, self.text = code, message, text


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "csvfab"
    protocol_version = "HTTP/1.1"
    # TCP_NODELAY : en-têtes et corps partent en deux écritures sur une
    # connexion gardée ouverte ; avec Nagle + l'ACK différé du client, la
    # seconde attendait ~40 ms (mesuré : 46 ms par /api/stat, 3 à chaque
    # ouverture de fichier).
    disable_nagle_algorithm = True
    _parsed_for = None      # l'instance sert toutes les requêtes d'une connexion gardée ouverte

    # --- plomberie -------------------------------------------------------
    def log_message(self, fmt, *a):
        # Silencieux par défaut (aucune console) ; CSVFAB_DEBUG=1 pour tracer.
        if config.DEBUG:
            sys.stderr.write("%s %s\n" % (self.command, self.path))
            hdr = self.headers.get("Range")
            if hdr:
                sys.stderr.write("    Range: %s\n" % hdr)

    def send(self, code, body=b"", ctype="text/plain; charset=utf-8", extra=None):
        if isinstance(body, str):
            body = body.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def json(self, code, obj):
        self.send(code, json.dumps(obj), "application/json; charset=utf-8")

    def send_cached(self, body, ctype, etag):
        """Réponse revalidée à chaque chargement (no-cache + ETag, 304 si inchangée)
        au lieu de no-store : Chromium ne garde le code compilé d'un script (son
        cache de code V8) que si le script lui-même est dans son cache HTTP. Réservé
        aux fichiers de l'app, sans jeton ; la page elle-même reste en no-store.

        Demandée sous sa version courante (?v=etag, l'adresse que la page sert, cf.
        assets.page) : gardable un an, immuable — Chromium la reprend de son cache sans
        rien demander. La revalidation coûtait un aller-retour par fichier à chaque
        chargement, sur le chemin du lancement (~5 à 13 ms, au repos)."""
        tag = f'"{etag}"'
        cache = "public, max-age=31536000, immutable" if self.arg("v") == etag else "no-cache"
        if self.headers.get("If-None-Match") == tag:
            self.send_response(304)
            self.send_header("ETag", tag)
            self.send_header("Cache-Control", cache)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", cache)
        self.send_header("ETag", tag)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    # --- lecture de la requête -------------------------------------------
    def _parse(self):
        if self._parsed_for is not self.path:
            u = urllib.parse.urlsplit(self.path)
            self.route, self._query = u.path, urllib.parse.parse_qs(u.query)
            self._parsed_for = self.path

    def arg(self, name, default=""):
        v = self._query.get(name)
        return v[0] if v else default

    def path_arg(self, name="path"):
        """Un chemin passé en paramètre, absolu, ~ développé et liens résolus ; None sans."""
        raw = self.arg(name)
        return os.path.realpath(os.path.expanduser(raw)) if raw else None

    def content_length(self):
        try:
            return int(self.headers.get("Content-Length") or 0)
        except ValueError:
            raise HttpError(400, "Content-Length invalide")

    def authorized(self):
        """Jeton valide et requête non initiée par une page étrangère."""
        origin = self.headers.get("Origin")
        if origin and origin not in self.server.origins:
            return False
        given = self.headers.get("X-Csv-Token") or self.arg("token")
        return secrets.compare_digest(given.encode("utf-8", "replace"), self.server.token_bytes)

    # --- aiguillage ------------------------------------------------------
    def _dispatch(self):
        self._parse()
        route = self.route
        api = route.startswith("/api/")
        if api and route not in PUBLIC and not self.authorized():
            return self.send(403, "forbidden")
        method = "GET" if self.command == "HEAD" else self.command
        try:
            spec = ROUTES.get((method, route))
            if spec is None:
                if method != "GET" or api:
                    return self.send(404, "no such route")
                spec = STATIC
            result = _resolve(spec)(self)
            if result is not None:
                self.json(200, result)
        except HttpError as e:
            if e.text:
                self.send(e.code, e.message)
            else:
                self.json(e.code, {"error": e.message})
        except Exception as e:
            # Une erreur imprévue : tracée dans le journal, une réponse 500 si
            # rien n'est encore parti, et la connexion fermée (si une réponse
            # était commencée, le flux ne serait plus lisible).
            import traceback
            traceback.print_exc()
            self.close_connection = True
            try:
                self.send(500, json.dumps({"error": str(e)}), "application/json; charset=utf-8",
                          {"Connection": "close"})
            except OSError:
                pass

    do_GET = do_HEAD = do_POST = do_PUT = _dispatch



class Server(http.server.ThreadingHTTPServer):
    # Sous Windows, SO_REUSEADDR laisse un second processus se lier au port déjà
    # pris — deux serveurs se voleraient les requêtes ; l'échec franc du bind y
    # est justement ce qui signale « un serveur tourne déjà ».
    allow_reuse_address = os.name != "nt"

    def setup_app(self, session, state, token=None):
        """L'état que les routes lisent (req.server.*), une fois le port connu."""
        port = self.server_address[1]
        self.session = session
        self.state = state
        self.token = token or secrets.token_urlsafe(24)
        self.token_bytes = self.token.encode("ascii")
        self.origins = (f"http://{config.HOST}:{port}", f"http://localhost:{port}")
        return self
