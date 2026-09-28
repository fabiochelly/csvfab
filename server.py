#!/usr/bin/env python3
"""Pont fichier local de l'éditeur CSV.

Pourquoi ce serveur existe : une page web ne peut pas ouvrir un chemin reçu en
ligne de commande — l'API File System Access n'accorde de handle qu'au travers
d'un sélecteur ou d'un drag & drop. Pour qu'un « Ouvrir avec » venu de yazi ou
d'un .desktop fonctionne, le contenu doit donc transiter par ici : le serveur
lit et réécrit le fichier à la place du navigateur.

Sécurité : écoute uniquement sur 127.0.0.1 et toute route /api/ exige le jeton
tiré au démarrage. Ce jeton est injecté dans la page servie (il n'apparaît donc
jamais dans l'URL, ni dans l'historique) et déposé dans
le dossier d'état (voir state_dir()) pour le lanceur. Aucun en-tête CORS n'est émis
et toute requête portant un Origin étranger est refusée : une page tierce ne
peut donc ni lire nos réponses ni écrire un fichier.

Arrêt : le serveur s'éteint seul dès que plus aucune fenêtre ne l'interroge,
pour ne rien laisser tourner derrière l'application fermée.
"""

import csv
import http.server
import io
import json
import os
import secrets
import shutil
import socketserver
import sys
import threading
import time
import urllib.parse
import zipfile
from datetime import datetime

VERSION = "1.0.2"
HERE = os.path.dirname(os.path.abspath(__file__))
# CSVFAB_* ; les anciens noms CSV_EDITOR_* restent lus.
PORT = int(os.environ.get("CSVFAB_PORT") or os.environ.get("CSV_EDITOR_PORT") or "8787")
HOST = "127.0.0.1"

# Marge avant extinction : le temps qu'une fenêtre naisse et se signale, puis,
# une fois qu'au moins une l'a fait, le temps de tolérer un rechargement.
IDLE_BOOT = 120.0
IDLE_LIVE = 30.0
POLL_ALIVE = 6.0        # au-delà, plus aucun client n'est considéré vivant

def state_dir():
    """Dossier d'état (jeton, journal, profil du navigateur), selon l'OS.

    Doit rester identique au choix du lanceur csvfab : c'est là qu'il lit le
    jeton. « csvfab », ou l'ancien « csv-editor » tant que le lanceur ne l'a
    pas migré (il le renomme, profil Chromium et permissions compris).
    """
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~\\AppData\\Local")
    elif sys.platform == "darwin":
        base = os.path.expanduser("~/Library/Application Support")
    else:
        base = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    new, old = os.path.join(base, "csvfab"), os.path.join(base, "csv-editor")
    return old if not os.path.exists(new) and os.path.isdir(old) else new


STATE = state_dir()
TOKEN = secrets.token_urlsafe(24)

_lock = threading.Lock()
_queue = []             # chemins en attente d'ouverture, déposés par le lanceur
_last_seen = 0.0        # dernier /api/pending d'une fenêtre
_seen_once = False


def client_alive():
    with _lock:
        return _seen_once and (time.time() - _last_seen) < POLL_ALIVE


def stamp():
    return datetime.now().strftime("%Y%m%d-%H%M%S")


# ---------------------------------------------------------------------------
# ECRITURE XLSX
#
# Un .xlsx n'est qu'un zip de quelques parties XML : le format est assez simple
# pour etre ecrit a la main, ce qui evite d'imposer openpyxl a un projet qui n'a
# aucune dependance. La feuille est ecrite en flux dans le zip, donc un gros CSV
# ne passe jamais entierement par la memoire.
# ---------------------------------------------------------------------------
XLSX_MAX_ROWS = 1048576          # limites dures d'Excel
XLSX_MAX_COLS = 16384

_CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">\
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>\
<Default Extension="xml" ContentType="application/xml"/>\
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>\
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>\
</Types>"""

_ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>\
</Relationships>"""

_WB_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>\
</Relationships>"""

def _col_letters(i):
    """0 -> A, 25 -> Z, 26 -> AA."""
    out = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        out = chr(65 + r) + out
    return out


_COLS = [_col_letters(i) for i in range(XLSX_MAX_COLS)]
_ILLEGAL = {c: None for c in range(0x20) if c not in (0x09, 0x0A, 0x0D)}


def _xml_escape(v):
    return (v.translate(_ILLEGAL)
             .replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))


def _sheet_name(base):
    """31 caracteres max, et aucun de []:*?/\\ — sinon Excel refuse d'ouvrir."""
    name = "".join(" " if c in "[]:*?/\\" else c for c in base).strip() or "Sheet1"
    return name[:31]


# --- Typage des colonnes ----------------------------------------------------
# Chaque colonne est analysee en entier avant l'ecriture (premiere passe) :
# elle devient numerique, pourcentage ou date quand au moins 90 % de ses
# cellules non vides le sont, et seules les cellules conformes sont converties
# — les autres restent du texte, rendu tel quel. Garde-fous : un entier a zero
# initial (code postal, telephone, identifiant) ou de plus de 15 chiffres
# (au-dela, Excel arrondit) laisse toute la colonne en texte.
import re as _re

_SPACES = _re.compile(r"[   ]")
_FR_NUM = _re.compile(r"^-?(?:\d{1,3}(?:[   ]\d{3})+|\d+)(?:,\d+)?$")    # 1 234,5
_EN_NUM = _re.compile(r"^-?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?$")                 # 1,234.5
_PCT = _re.compile(r"^(-?\d+(?:[.,]\d+)?)\s?%$")
_YMD = _re.compile(r"^(\d{4})-(\d{1,2})-(\d{1,2})(?:[T ](\d{1,2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$")
_DMY = _re.compile(r"^(\d{1,2})[/.-](\d{1,2})[/.-](\d{4}|\d{2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?$")
_EPOCH = datetime(1899, 12, 30)
_THRESHOLD = 0.9


class _ColStat:
    __slots__ = ("filled", "fr", "en", "comma_dec", "dot_dec", "decimals", "pct", "pct_dec", "ymd", "dmy",
                 "first_gt12", "second_gt12", "time", "secs", "lead0", "long", "width", "kind", "fmt")

    def __init__(self):
        self.filled = self.fr = self.en = self.comma_dec = self.dot_dec = self.decimals = self.pct = self.pct_dec = 0
        self.ymd = self.dmy = 0
        self.first_gt12 = self.second_gt12 = self.time = self.secs = self.lead0 = self.long = False
        self.width = 0
        self.kind = "text"
        self.fmt = None

    def see(self, v):
        self.width = max(self.width, len(v))
        v = v.strip()
        if not v:
            return
        self.filled += 1
        digits = v.lstrip("-")
        if digits[:1] == "0" and len(digits) > 1 and digits[1:2].isdigit():
            self.lead0 = True                     # 007, 06 12 34 56 78, 01000
        if _FR_NUM.match(v):
            self.fr += 1
            if "," in v:
                self.comma_dec += 1
                self.decimals = max(self.decimals, len(v) - v.index(",") - 1)
        if _EN_NUM.match(v):
            self.en += 1
            if "." in v:
                self.dot_dec += 1
                self.decimals = max(self.decimals, len(v) - v.index(".") - 1)
        if len(_SPACES.sub("", digits).split(",")[0].split(".")[0]) > 15:
            self.long = True
        m = _PCT.match(v)
        if m:
            self.pct += 1
            num = m.group(1).replace(",", ".")
            if "." in num:
                self.pct_dec = max(self.pct_dec, len(num) - num.index(".") - 1)
        m = _YMD.match(v)
        if m:
            self.ymd += 1
            self._t(m.group(4), m.group(6))
            return
        m = _DMY.match(v)
        if m:
            self.dmy += 1
            if int(m.group(1)) > 12:
                self.first_gt12 = True
            if int(m.group(2)) > 12:
                self.second_gt12 = True
            self._t(m.group(4), m.group(6))

    def _t(self, h, s):
        if h is not None:
            self.time = True
            if s is not None and s != "00":
                self.secs = True

    def decide(self):
        n = self.filled
        if not n:
            return
        need = n * _THRESHOLD
        if self.ymd >= need or self.dmy >= need:
            iso = self.ymd >= self.dmy
            self.kind = "ymd" if iso else ("mdy" if self.second_gt12 and not self.first_gt12 else "dmy")
            date = "yyyy-mm-dd" if iso else "dd/mm/yyyy" if self.kind == "dmy" else "mm/dd/yyyy"
            self.fmt = date + (" hh:mm:ss" if self.secs else " hh:mm" if self.time else "")
            self.width = max(self.width, len(self.fmt))
            return
        if self.lead0 or self.long:
            return
        if self.pct >= need:
            self.kind, self.fmt = "pct", "0." + "0" * min(self.pct_dec, 4) + "%" if self.pct_dec else "0%"
            return
        # Virgule ou point decimal : la convention qui explique le plus de cellules.
        if self.comma_dec and not self.dot_dec and self.fr >= need:
            self.kind = "fr"
        elif self.en >= need and self.en >= self.fr:
            self.kind = "en"
        elif self.fr >= need:
            self.kind = "fr"
        else:
            return
        d = min(self.decimals, 6)
        self.fmt = "#,##0." + "0" * d if d else None      # entiers : format General, sans separateur de milliers
        self.width += 1 + (self.width // 3 if d else 0)


def _num(kind, v):
    """Valeur Excel (float) d'une cellule deja retenue par _ColStat, ou None."""
    v = v.strip()
    try:
        if kind == "fr":
            if not _FR_NUM.match(v):
                return None
            return float(_SPACES.sub("", v).replace(",", "."))
        if kind == "en":
            if not _EN_NUM.match(v):
                return None
            return float(v.replace(",", ""))
        if kind == "pct":
            m = _PCT.match(v)
            return float(m.group(1).replace(",", ".")) / 100 if m else None
        if kind in ("ymd", "dmy", "mdy"):
            m = (_YMD if kind == "ymd" else _DMY).match(v)
            if not m:
                return None
            a, b, c = m.group(1), m.group(2), m.group(3)
            y, mo, d = (int(a), int(b), int(c)) if kind == "ymd" else \
                (int(c), int(b), int(a)) if kind == "dmy" else (int(c), int(a), int(b))
            if y < 100:
                y += 2000
            t = datetime(y, mo, d, int(m.group(4) or 0), int(m.group(5) or 0), int(m.group(6) or 0))
            delta = t - _EPOCH
            return delta.days + delta.seconds / 86400
    except (ValueError, OverflowError):
        return None
    return None


def _fmt_num(x):
    return repr(int(x)) if x == int(x) and abs(x) < 1e15 else repr(x)


# --- Styles -----------------------------------------------------------------
# Police Aptos partout ; ligne de titres en gras, blanc sur bleu nuit, figee et
# munie d'un filtre automatique. Un style par format de nombre ou de date.
_HEADER_FILL = "FF1E3A8A"
_HEADER_LINE = "FF3B82F6"
_BUILTIN_FMT = {"0%": 9, "0.00%": 10}


def _styles_xml(formats):
    """formats : liste de codes de format (index -> style 2 + index)."""
    custom, ids = [], []
    for k, code in enumerate(formats):
        fid = _BUILTIN_FMT.get(code)
        if fid is None:
            fid = 164 + len(custom)
            custom.append('<numFmt numFmtId="%d" formatCode="%s"/>' % (fid, _xml_escape(code).replace('"', "&quot;")))
        ids.append(fid)
    xfs = ('<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
           '<xf numFmtId="0" fontId="1" fillId="2" borderId="1" xfId="0" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1">'
           '<alignment vertical="center"/></xf>'
           + "".join('<xf numFmtId="%d" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>' % f for f in ids))
    # Concatenation et non « % » : un code de format peut lui-meme contenir un %.
    return ('<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
            + ('<numFmts count="' + str(len(custom)) + '">' + "".join(custom) + '</numFmts>' if custom else "")
            + '<fonts count="2">'
              '<font><sz val="11"/><name val="Aptos"/><family val="2"/></font>'
              '<font><b/><sz val="11"/><color rgb="FFFFFFFF"/><name val="Aptos"/><family val="2"/></font></fonts>'
              '<fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>'
              '<fill><patternFill patternType="solid"><fgColor rgb="' + _HEADER_FILL + '"/><bgColor indexed="64"/></patternFill></fill></fills>'
              '<borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border>'
              '<border><left/><right/><top/><bottom style="medium"><color rgb="' + _HEADER_LINE + '"/></bottom><diagonal/></border></borders>'
              '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
              '<cellXfs count="' + str(2 + len(ids)) + '">' + xfs + '</cellXfs>'
              '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>'
              '</styleSheet>')


_CONTENT_TYPES = _CONTENT_TYPES.replace(
    "</Types>",
    '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>')
_WB_RELS = _WB_RELS.replace(
    "</Relationships>",
    '<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>')


def write_xlsx(open_rows, out_path, sheet="Sheet1", header=True):
    """Ecrit un .xlsx a partir de open_rows(), qui renvoie a chaque appel un
    nouvel iterateur de lignes (deux passes : analyse, puis ecriture).
    header : la premiere ligne est une ligne de titres. Renvoie un bilan."""
    # 1re passe : largeurs et types, colonne par colonne.
    stats = []
    for k, row in enumerate(open_rows()):
        if k >= XLSX_MAX_ROWS:
            break
        row = row[:XLSX_MAX_COLS]
        while len(stats) < len(row):
            stats.append(_ColStat())
        if header and k == 0:
            for c, v in enumerate(row):
                stats[c].width = max(stats[c].width, int(len(v) * 1.15) + 3)   # gras + bouton de filtre
            continue
        for c, v in enumerate(row):
            stats[c].see(v)
    formats = []
    style_of = []
    for st in stats:
        st.decide()
        if st.fmt:
            if st.fmt not in formats:
                formats.append(st.fmt)
            style_of.append(2 + formats.index(st.fmt))
        else:
            style_of.append(0)

    written = cols_max = 0
    truncated_rows = truncated_cols = False
    name = _sheet_name(sheet)

    with zipfile.ZipFile(out_path, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("[Content_Types].xml", _CONTENT_TYPES)
        zf.writestr("_rels/.rels", _ROOT_RELS)
        zf.writestr("xl/_rels/workbook.xml.rels", _WB_RELS)
        zf.writestr("xl/styles.xml", _styles_xml(formats))

        # Pas de force_zip64 : l'entete ZIP64 qu'il ajoute fait deraper les
        # lecteurs xlsx (SheetJS lit une taille aberrante et abandonne). Sans
        # lui, zipfile leve proprement si la feuille depasse 4 Gio, ce qui est
        # un meilleur echec qu'un classeur illisible.
        with zf.open("xl/worksheets/sheet1.xml", "w") as fh:
            out = io.BufferedWriter(fh, 1 << 16)
            head = ['<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                    '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">']
            if header:
                head.append('<sheetViews><sheetView workbookViewId="0">'
                            '<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>'
                            '<selection pane="bottomLeft" activeCell="A2" sqref="A2"/></sheetView></sheetViews>')
            head.append('<sheetFormatPr defaultRowHeight="15"/>')
            if stats:
                head.append("<cols>" + "".join(
                    '<col min="%d" max="%d" width="%d" customWidth="1"/>' % (c + 1, c + 1, min(60, max(8, st.width + 2)))
                    for c, st in enumerate(stats)) + "</cols>")
            head.append("<sheetData>")
            out.write("".join(head).encode("utf-8"))

            for row in open_rows():
                if written >= XLSX_MAX_ROWS:
                    truncated_rows = True
                    break
                if len(row) > XLSX_MAX_COLS:
                    row = row[:XLSX_MAX_COLS]
                    truncated_cols = True
                written += 1
                cols_max = max(cols_max, len(row))
                is_head = header and written == 1
                buf = ['<row r="1" ht="22" customHeight="1">' if is_head else '<row r="%d">' % written]
                for i, val in enumerate(row):
                    ref = _COLS[i]
                    if is_head:
                        buf.append('<c r="%s1" s="1" t="inlineStr"><is><t xml:space="preserve">%s</t></is></c>'
                                   % (ref, _xml_escape(val)))
                        continue
                    if val is None or val == "":
                        continue
                    st = stats[i]
                    x = _num(st.kind, val) if st.kind != "text" else None
                    if x is not None:
                        s_attr = ' s="%d"' % style_of[i] if style_of[i] else ""
                        buf.append('<c r="%s%d"%s><v>%s</v></c>' % (ref, written, s_attr, _fmt_num(x)))
                    else:
                        buf.append('<c r="%s%d" t="inlineStr"><is><t xml:space="preserve">%s</t></is></c>'
                                   % (ref, written, _xml_escape(val)))
                buf.append("</row>")
                out.write("".join(buf).encode("utf-8"))
            out.write(b"</sheetData>")
            last = "%s%d" % (_COLS[max(cols_max, 1) - 1], max(written, 1))
            if header and written:
                out.write(('<autoFilter ref="A1:%s"/>' % last).encode("utf-8"))
            out.write(b"</worksheet>")
            out.flush()

        # Ecrit apres la feuille : la plage du filtre n'est connue qu'a la fin.
        defined = ""
        if header and written:
            defined = ('<definedNames><definedName name="_xlnm._FilterDatabase" localSheetId="0" hidden="1">'
                       "'%s'!$A$1:$%s$%d</definedName></definedNames>"
                       % (_xml_escape(name).replace("'", "''"), _COLS[max(cols_max, 1) - 1], written))
        zf.writestr("xl/workbook.xml",
                    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                    '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
                    ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
                    '<sheets><sheet name="%s" sheetId="1" r:id="rId1"/></sheets>%s</workbook>'
                    % (_xml_escape(name), defined))

    return {"rows": written, "cols": cols_max,
            "truncated_rows": truncated_rows, "truncated_cols": truncated_cols}


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "csvfab"
    protocol_version = "HTTP/1.1"

    # --- plomberie -------------------------------------------------------
    def log_message(self, fmt, *a):
        # Silencieux par defaut (aucune console) ; CSVFAB_DEBUG=1 pour tracer.
        if os.environ.get("CSVFAB_DEBUG") or os.environ.get("CSV_EDITOR_DEBUG"):
            sys.stderr.write("%s %s\n" % (self.command, self.path))
            hdr = self.headers.get("Range")
            if hdr:
                sys.stderr.write("    Range: %s\n" % hdr)

    def _send(self, code, body=b"", ctype="text/plain; charset=utf-8", extra=None):
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

    def _json(self, code, obj):
        self._send(code, json.dumps(obj), "application/json; charset=utf-8")

    def _query(self):
        return urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)

    def _authorized(self):
        """Jeton valide et requête non initiée par une page étrangère."""
        origin = self.headers.get("Origin")
        if origin and origin not in (f"http://{HOST}:{PORT}", f"http://localhost:{PORT}"):
            return False
        given = self.headers.get("X-Csv-Token") or (self._query().get("token") or [""])[0]
        return secrets.compare_digest(given, TOKEN)

    def _path_arg(self):
        raw = (self._query().get("path") or [""])[0]
        if not raw:
            return None
        return os.path.realpath(os.path.expanduser(raw))

    # --- routes ----------------------------------------------------------
    def do_GET(self):
        route = urllib.parse.urlparse(self.path).path

        if route in ("/", "/viewer.htm", "/index.html"):
            return self._page()
        if route == "/api/ping":
            return self._json(200, {"ok": True, "pid": os.getpid(), "version": VERSION,
                                    "client": client_alive()})
        if not route.startswith("/api/"):
            return self._static(route)

        if not self._authorized():
            return self._send(403, "forbidden")

        if route == "/api/pending":
            global _last_seen, _seen_once
            with _lock:
                _last_seen = time.time()
                _seen_once = True
                paths, _queue[:] = list(_queue), []
            return self._json(200, {"paths": paths})

        if route == "/api/stat":
            p = self._path_arg()
            if not p or not os.path.isfile(p):
                return self._json(404, {"error": "not a file"})
            st = os.stat(p)
            return self._json(200, {"name": os.path.basename(p), "path": p,
                                    "size": st.st_size, "mtime": st.st_mtime,
                                    "writable": os.access(p, os.W_OK)})

        if route == "/api/file":
            return self._read_file()

        return self._send(404, "no such route")

    do_HEAD = do_GET

    def do_POST(self):
        route = urllib.parse.urlparse(self.path).path
        if route not in ("/api/open", "/api/xlsx"):
            return self._send(404, "no such route")
        if not self._authorized():
            return self._send(403, "forbidden")
        if route == "/api/xlsx":
            return self._make_xlsx()
        try:
            n = int(self.headers.get("Content-Length") or 0)
            payload = json.loads(self.rfile.read(n) or b"{}")
        except (ValueError, json.JSONDecodeError):
            return self._json(400, {"error": "bad json"})
        paths = [os.path.realpath(os.path.expanduser(p))
                 for p in payload.get("paths", []) if p]
        with _lock:
            for p in paths:
                if p not in _queue:
                    _queue.append(p)
        return self._json(200, {"queued": len(paths), "client": client_alive()})

    def _make_xlsx(self):
        """CSV en entree, classeur en sortie.

        L'editeur envoie le meme CSV que pour une sauvegarde ordinaire, et le
        delimiteur avec lequel il l'a serialise ; le serveur le relit et le
        convertit. Avec ?path=, le classeur est ecrit la (atomiquement, comme
        toute ecriture) ; sinon ses octets sont renvoyes, pour que la page les
        depose elle-meme via un handle.
        """
        q = self._query()
        delim = (q.get("delim") or [","])[0] or ","
        if len(delim) != 1:
            return self._json(400, {"error": "delimiteur invalide"})
        dest = self._path_arg()
        sheet = (q.get("sheet") or [""])[0] or (
            os.path.splitext(os.path.basename(dest))[0] if dest else "Sheet1")

        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            return self._json(400, {"error": "Content-Length invalide"})

        # Le corps transite par un fichier temporaire : un CSV de 500 Mo n'a pas
        # a tenir en RAM pour etre relu ligne a ligne.
        d = os.path.dirname(dest) if dest else STATE
        if dest and not os.path.isdir(d):
            return self._json(400, {"error": f"dossier inexistant : {d}"})
        src = os.path.join(d, f".csvfab.{os.getpid()}.csv")
        tmp = os.path.join(d, f".csvfab.{os.getpid()}.xlsx")
        try:
            remaining = n
            with open(src, "wb") as f:
                while remaining > 0:
                    chunk = self.rfile.read(min(1 << 20, remaining))
                    if not chunk:
                        raise IOError("flux interrompu avant la fin du corps")
                    f.write(chunk)
                    remaining -= len(chunk)

            def open_rows():                       # relu deux fois : analyse, puis ecriture
                with open(src, "r", encoding="utf-8", newline="") as f:
                    yield from csv.reader(f, delimiter=delim)
            header = (q.get("header") or ["1"])[0] != "0"
            info = write_xlsx(open_rows, tmp, sheet, header)

            if dest:
                os.replace(tmp, dest)
                info["path"] = dest
                info["ok"] = True
                return self._json(200, info)

            with open(tmp, "rb") as f:
                body = f.read()
            return self._send(200, body,
                              "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                              {"X-Xlsx-Info": json.dumps(info)})
        except Exception as e:
            return self._json(500, {"error": str(e)})
        finally:
            for f in (src, tmp):
                try:
                    os.unlink(f)
                except OSError:
                    pass

    def do_PUT(self):
        if urllib.parse.urlparse(self.path).path != "/api/file":
            return self._send(404, "no such route")
        if not self._authorized():
            return self._send(403, "forbidden")
        return self._write_file()

    # --- implémentations -------------------------------------------------
    def _page(self):
        """viewer.htm, jeton injecté à la volée dans le marqueur du script."""
        try:
            with open(os.path.join(HERE, "viewer.htm"), "r", encoding="utf-8") as f:
                html = f.read()
        except OSError as e:
            return self._send(500, f"viewer.htm illisible : {e}")
        html = html.replace("__CSVE_TOKEN__", TOKEN)
        return self._send(200, html, "text/html; charset=utf-8")

    # Seuls les assets de l'app sont servis : le dossier contient aussi le code
    # du serveur et, souvent, les CSV de l'utilisateur — rien de tout cela n'a
    # à sortir par une route sans jeton.
    ASSETS = {".js": "text/javascript", ".css": "text/css", ".png": "image/png",
              ".svg": "image/svg+xml", ".ico": "image/x-icon",
              ".woff2": "font/woff2", ".woff": "font/woff"}

    def _static(self, route):
        name = os.path.normpath(route.lstrip("/"))
        full = os.path.realpath(os.path.join(HERE, name))
        ctype = self.ASSETS.get(os.path.splitext(full)[1])
        if not ctype or not full.startswith(HERE + os.sep) or not os.path.isfile(full):
            return self._send(404, "not found")
        size = os.path.getsize(full)
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(size))
        self.end_headers()
        with open(full, "rb") as f:
            shutil.copyfileobj(f, self.wfile)

    def _read_file(self):
        p = self._path_arg()
        if not p or not os.path.isfile(p):
            return self._send(404, "not a file")
        try:
            size = os.path.getsize(p)
            self.send_response(200)
            self.send_header("Content-Type", "text/csv; charset=utf-8")
            self.send_header("Content-Length", str(size))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            if self.command == "HEAD":
                return
            with open(p, "rb") as f:
                shutil.copyfileobj(f, self.wfile, 1 << 20)
        except OSError as e:
            try:
                self._send(500, f"lecture impossible : {e}")
            except Exception:
                pass

    def _write_file(self):
        """Écriture atomique : fichier temporaire voisin puis os.replace().

        Le fichier d'origine n'est donc jamais tronqué en cas d'échec ou de
        coupure — c'est le pendant serveur du swap file de createWritable().
        """
        p = self._path_arg()
        if not p:
            return self._json(400, {"error": "chemin manquant"})
        d = os.path.dirname(p)
        if not os.path.isdir(d):
            return self._json(400, {"error": f"dossier inexistant : {d}"})

        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            return self._json(400, {"error": "Content-Length invalide"})

        backup = (self._query().get("backup") or ["0"])[0] == "1"
        made = tmp = None
        try:
            if backup and os.path.isfile(p):
                made = f"{p}.{stamp()}.bak"
                shutil.copy2(p, made)

            tmp = os.path.join(d, f".{os.path.basename(p)}.{os.getpid()}.part")
            remaining = n
            with open(tmp, "wb") as f:
                while remaining > 0:
                    chunk = self.rfile.read(min(1 << 20, remaining))
                    if not chunk:
                        raise IOError("flux interrompu avant la fin du corps")
                    f.write(chunk)
                    remaining -= len(chunk)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, p)
        except Exception as e:
            if tmp:
                try:
                    os.unlink(tmp)
                except OSError:
                    pass
            return self._json(500, {"error": str(e)})

        return self._json(200, {"ok": True, "bytes": n,
                                "backup": os.path.basename(made) if made else None,
                                "size": os.path.getsize(p)})


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    # Sous Windows, SO_REUSEADDR laisse un second processus se lier au port déjà
    # pris — deux serveurs se voleraient les requêtes ; l'échec franc du bind y
    # est justement ce qui signale « un serveur tourne déjà ».
    allow_reuse_address = os.name != "nt"


def watchdog(srv):
    """Éteint le serveur quand plus aucune fenêtre ne se manifeste."""
    while True:
        time.sleep(2)
        with _lock:
            seen, last = _seen_once, _last_seen
        idle = time.time() - (last if seen else _started)
        if idle > (IDLE_LIVE if seen else IDLE_BOOT):
            srv.shutdown()
            return


def main():
    global _started
    _started = time.time()
    os.makedirs(STATE, exist_ok=True)

    try:
        srv = Server((HOST, PORT), Handler)
    except OSError as e:
        # Déjà occupé : un serveur tourne probablement, le lanceur s'en sert.
        print(f"port {PORT} indisponible : {e}", file=sys.stderr)
        return 1

    tok = os.path.join(STATE, "token")
    fd = os.open(tok, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(f"{PORT}\n{TOKEN}\n")

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


if __name__ == "__main__":
    sys.exit(main())
