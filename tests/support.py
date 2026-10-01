"""Outillage commun des tests : un vrai server.py lancé en sous-processus.

Les tests de ce dossier sont des tests de caractérisation en boîte noire : ils
ne passent que par HTTP (et par la ligne de commande du lanceur), jamais par
les fonctions internes. Ils décrivent donc le comportement, pas le découpage
du code, et doivent rester verts tels quels pendant qu'on réorganise celui-ci.

Chaque serveur a son port libre et son propre dossier d'état temporaire
(XDG_STATE_HOME, LOCALAPPDATA et HOME y pointent, selon l'OS que lit
state_dir()), donc rien ne touche à l'app réellement installée.
"""

import http.client
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SERVER = os.path.join(ROOT, "server.py")
LAUNCHER = os.path.join(ROOT, "csvfab.py")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def isolated_env(home, port, **extra):
    """Environnement d'un processus du projet confiné à `home`."""
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("CSVFAB_", "CSV_EDITOR_"))}
    env.update(HOME=home, USERPROFILE=home,
               XDG_STATE_HOME=os.path.join(home, "state"),
               LOCALAPPDATA=os.path.join(home, "state"),
               CSVFAB_PORT=str(port))
    env.update(extra)
    return env


def state_of(home):
    """Le dossier d'état que server.py choisira pour ce HOME."""
    if os.name == "nt" or sys.platform != "darwin":
        return os.path.join(home, "state", "csvfab")
    return os.path.join(home, "Library", "Application Support", "csvfab")


def read_version():
    """La version, lue dans le source (là où release.sh l'écrit), pas importée."""
    path = os.path.join(ROOT, "bridge", "config.py")
    with open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("VERSION = "):
                return line.split('"')[1]
    raise AssertionError(f"VERSION introuvable dans {path}")


class Response:
    def __init__(self, status, headers, body):
        self.status, self.headers, self.body = status, headers, body

    def header(self, name):
        return self.headers.get(name.lower())

    @property
    def text(self):
        return self.body.decode("utf-8")

    def json(self):
        return json.loads(self.body)

    def __repr__(self):
        return f"<Response {self.status} {self.body[:200]!r}>"


class Bridge:
    """Un server.py isolé. `with Bridge() as b:` ou start()/stop()."""

    def __init__(self, root=ROOT):
        self.server = os.path.join(root, "server.py")
        self.home = tempfile.mkdtemp(prefix="csvfab-test-")
        self.port = free_port()
        self.state = state_of(self.home)
        self.token = None
        self.proc = None

    # --- cycle de vie ----------------------------------------------------
    def start(self):
        self.proc = subprocess.Popen(
            [sys.executable, self.server], cwd=self.home,
            env=isolated_env(self.home, self.port),
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.token = wait_token(self.state, self.proc)
        return self

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self.proc and self.proc.stderr:
            self.proc.stderr.close()
        shutil.rmtree(self.home, ignore_errors=True)

    def __enter__(self):
        return self if self.proc else self.start()

    def __exit__(self, *exc):
        self.stop()

    # --- requêtes --------------------------------------------------------
    @property
    def origin(self):
        return f"http://127.0.0.1:{self.port}"

    def request(self, method, path, body=None, headers=None, token=True, timeout=30):
        h = dict(headers or {})
        if token and "X-Csv-Token" not in h:
            h["X-Csv-Token"] = self.token
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
        try:
            conn.request(method, path, body=body, headers=h)
            r = conn.getresponse()
            return Response(r.status, {k.lower(): v for k, v in r.getheaders()}, r.read())
        finally:
            conn.close()

    def get(self, path, **kw):
        return self.request("GET", path, **kw)

    def post(self, path, body=b"", **kw):
        return self.request("POST", path, body=body, **kw)

    def put(self, path, body=b"", **kw):
        return self.request("PUT", path, body=body, **kw)

    def drain(self):
        """Vide la file des chemins à ouvrir (elle est partagée entre les tests d'une classe)."""
        return self.get("/api/pending?wait=0").json()["paths"]

    def tmp(self, *parts):
        """Un chemin de travail dans le dossier du test (hors du dossier d'état)."""
        d = os.path.join(self.home, "work")
        os.makedirs(d, exist_ok=True)
        return os.path.join(d, *parts)


def wait_token(state, proc=None, timeout=10):
    """Le jeton que le serveur dépose dans <état>/token une fois prêt."""
    path = os.path.join(state, "token")
    deadline = time.time() + timeout
    while time.time() < deadline:
        if proc is not None and proc.poll() is not None:
            err = proc.stderr.read().decode(errors="replace") if proc.stderr else ""
            raise AssertionError(f"server.py s'est arrêté ({proc.returncode}) : {err}")
        try:
            with open(path, encoding="utf-8") as f:
                lines = f.read().splitlines()
            if len(lines) > 1 and lines[1]:
                return lines[1]
        except OSError:
            pass
        time.sleep(0.02)
    raise AssertionError(f"pas de jeton dans {path} après {timeout} s")


def q(**params):
    """Chaîne de requête encodée (les chemins contiennent /, espaces, accents)."""
    import urllib.parse
    return "?" + urllib.parse.urlencode(params)


# --- fichiers de test fabriqués à la volée ----------------------------------
# Le .gitignore exclut *.csv / *.xlsx du dépôt : les fixtures sont générées ici.

_NS = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
_R = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'


def make_workbook(path, sheets, shared=(), date1904=False):
    """Un classeur minimal écrit à la main, sans passer par write_xlsx.

    sheets : liste de (nom, état, xml de <sheetData>) — état '' ou 'hidden'.
    shared : les chaînes partagées (t="s" renvoie à leur rang).
    Style 0 = Général, 1 = date (numFmtId 14), 2 = format perso date+heure.
    """
    ct = ['<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>',
          '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>',
          '<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>']
    wb_sheets, rels = [], []
    for i, (name, state, _) in enumerate(sheets, 1):
        st = f' state="{state}"' if state else ""
        wb_sheets.append(f'<sheet name="{name}" sheetId="{i}"{st} r:id="rId{i}"/>')
        rels.append(f'<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>')
        ct.append(f'<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>')
    n = len(sheets)
    rels.append(f'<Relationship Id="rId{n + 1}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>')
    rels.append(f'<Relationship Id="rId{n + 2}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>')
    pr = '<workbookPr date1904="1"/>' if date1904 else ""
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml",
                   '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
                   '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
                   '<Default Extension="xml" ContentType="application/xml"/>' + "".join(ct) + "</Types>")
        z.writestr("_rels/.rels",
                   '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                   '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>')
        z.writestr("xl/workbook.xml",
                   f'<?xml version="1.0" encoding="UTF-8"?><workbook {_NS} {_R}>{pr}<sheets>{"".join(wb_sheets)}</sheets></workbook>')
        z.writestr("xl/_rels/workbook.xml.rels",
                   '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                   + "".join(rels) + "</Relationships>")
        z.writestr("xl/styles.xml",
                   f'<?xml version="1.0" encoding="UTF-8"?><styleSheet {_NS}>'
                   '<numFmts count="1"><numFmt numFmtId="170" formatCode="dd/mm/yyyy hh:mm"/></numFmts>'
                   '<cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/>'
                   '<xf numFmtId="170" applyNumberFormat="1"/></cellXfs></styleSheet>')
        z.writestr("xl/sharedStrings.xml",
                   f'<?xml version="1.0" encoding="UTF-8"?><sst {_NS} count="{len(shared)}" uniqueCount="{len(shared)}">'
                   + "".join(f"<si><t>{s}</t></si>" for s in shared) + "</sst>")
        for i, (_, _, data) in enumerate(sheets, 1):
            z.writestr(f"xl/worksheets/sheet{i}.xml",
                       f'<?xml version="1.0" encoding="UTF-8"?><worksheet {_NS}><sheetData>{data}</sheetData></worksheet>')
