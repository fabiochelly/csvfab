"""La page, le script de l'app et ses fichiers statiques (sans jeton)."""

import os

from .. import config
from ..httpd import HttpError

JS_DIR = os.path.join(config.APP_DIR, "ui", "js")

# Seuls les fichiers de l'app sont servis : son dossier contient aussi le code
# du serveur et, souvent, les CSV de l'utilisateur — rien de tout cela n'a à
# sortir par une route sans jeton.
ASSETS = {".js": "text/javascript", ".css": "text/css", ".png": "image/png",
          ".svg": "image/svg+xml", ".ico": "image/x-icon",
          ".woff2": "font/woff2", ".woff": "font/woff"}

VIEWER = os.path.join(config.APP_DIR, "viewer.htm")

_page = None            # (signature de viewer.htm et jeton, octets prêts à l'envoi)

# Politique de sécurité de la page. Elle ne peut pas empêcher le code de s'exécuter
# (gestionnaires onclick en ligne : unsafe-inline ; formules compilées : unsafe-eval) ;
# elle empêche ce code, quel qu'il soit, d'envoyer quoi que ce soit ailleurs qu'au
# serveur local : connect-src (fetch), img-src (une image-balise), form-action, et
# aucun cadre. La page détient le jeton qui lit tous les fichiers : rien ne doit sortir.
# blob: pour le worker d'analyse (21-…), data: pour les icônes SVG du CSS, cdnjs pour
# le repli de PapaParse si papaparse.min.js manque.
CSP = "; ".join([
    "default-src 'self'",
    "script-src 'self' 'unsafe-inline' 'unsafe-eval' https://cdnjs.cloudflare.com",
    "worker-src 'self' blob:",
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    "connect-src 'self'",
    "media-src 'none'",
    "object-src 'none'",
    "frame-src 'none'",
    "frame-ancestors 'none'",
    "base-uri 'none'",
    "form-action 'none'",
])
_app_js = None          # (signature des sources, corps, etag)


def page(req):
    """viewer.htm, jeton injecté dans le marqueur du script. Gardée prête à
    l'envoi tant que le fichier ne change pas (date, taille) : ni relecture, ni
    décodage, ni remplacement, ni encodage de ses ~45 Ko à chaque chargement."""
    global _page
    try:
        st = os.stat(VIEWER)
        sig = (st.st_mtime_ns, st.st_size, req.server.token)
        cached = _page
        if cached is None or cached[0] != sig:
            with open(VIEWER, "r", encoding="utf-8") as f:
                html = f.read()
            cached = _page = (sig, html.replace("__CSVE_TOKEN__", req.server.token).encode("utf-8"))
    except OSError as e:
        raise HttpError(500, f"viewer.htm illisible : {e}", text=True)
    req.send(200, cached[1], "text/html; charset=utf-8", {"Content-Security-Policy": CSP})


def app_js(req):
    """/app.js : les sources ui/js/*.js mises bout à bout, dans l'ordre de leur nom.
    Elles se comportent ainsi exactement comme le script unique dont elles sont
    issues : une fonction déclarée dans un fichier suivant reste appelable depuis
    un fichier précédent (hoisting), ce que des <script> séparés ne permettraient
    pas. Le jeton n'y figure pas : il reste dans viewer.htm (cf. page).

    Assemblé une fois puis gardé tant que les sources ne changent pas (noms,
    dates, tailles) : relire et hacher ~450 Ko à chaque chargement coûtait ~2 ms,
    un stat par fichier coûte quelques µs — et modifier un fichier reste visible
    au rechargement suivant."""
    global _app_js
    try:
        names = sorted(f for f in os.listdir(JS_DIR) if f.endswith(".js"))
        sig = tuple((n, st.st_mtime_ns, st.st_size) for n in names for st in (os.stat(os.path.join(JS_DIR, n)),))
        cached = _app_js
        if cached is None or cached[0] != sig:
            import hashlib
            parts = []
            for name in names:
                with open(os.path.join(JS_DIR, name), "r", encoding="utf-8") as f:
                    parts.append(f"/* ---- ui/js/{name} ---- */\n" + f.read())
            # allFunctionsCalledOnLoad : V8 compile toutes les fonctions d'emblée (hors
            # du fil principal) au lieu de les découvrir une à une, et le cache de code
            # de Chromium les contient toutes.
            body = ("//# allFunctionsCalledOnLoad\n" + "\n".join(parts)).encode("utf-8")
            cached = _app_js = (sig, body, hashlib.sha1(body).hexdigest()[:20])
    except OSError as e:
        raise HttpError(500, f"ui/js illisible : {e}", text=True)
    req.send_cached(cached[1], "text/javascript; charset=utf-8", cached[2])


def static(req):
    name = os.path.normpath(req.route.lstrip("/"))
    full = os.path.realpath(os.path.join(config.APP_DIR, name))
    ctype = ASSETS.get(os.path.splitext(full)[1])
    if not ctype or not full.startswith(config.APP_DIR + os.sep) or not os.path.isfile(full):
        raise HttpError(404, "not found", text=True)
    st = os.stat(full)
    with open(full, "rb") as f:
        body = f.read()                      # quelques centaines de Ko au plus (fichiers de l'app)
    req.send_cached(body, ctype, f"{st.st_mtime_ns:x}-{st.st_size:x}")
