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

import os
import socket
import sys

# Le port est ouvert avant tout le reste (imports, définitions : ~70 ms de
# plus) : le lanceur démarre Chromium dès qu'il accepte des connexions, et
# une requête arrivée entre-temps attend dans la file d'écoute au lieu d'être
# refusée. Un port déjà pris signale, comme avant, un serveur déjà lancé.
_EARLY = None
if __name__ == "__main__":
    _port = int(os.environ.get("CSVFAB_PORT") or os.environ.get("CSV_EDITOR_PORT") or "8787")
    _EARLY = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    if os.name != "nt":            # voir Server.allow_reuse_address
        _EARLY.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        _EARLY.bind(("127.0.0.1", _port))
        _EARLY.listen(64)
    except OSError as e:
        print(f"port {_port} indisponible : {e}", file=sys.stderr)
        sys.exit(1)

import csv
import hashlib
import http.server
import io
import json
import secrets
import select
import shutil
import socketserver
import threading
import time
import urllib.parse
import xml.etree.ElementTree as ET
import zipfile
from datetime import datetime, timedelta

VERSION = "1.11.1"
HERE = os.path.dirname(os.path.abspath(__file__))
# CSVFAB_* ; les anciens noms CSV_EDITOR_* restent lus.
PORT = int(os.environ.get("CSVFAB_PORT") or os.environ.get("CSV_EDITOR_PORT") or "8787")
HOST = "127.0.0.1"

# Marge avant extinction : le temps qu'une fenêtre naisse et se signale, puis,
# une fois qu'au moins une l'a fait, le temps de tolérer un rechargement.
IDLE_BOOT = 120.0
IDLE_LIVE = 30.0
POLL_ALIVE = 6.0        # au-delà, plus aucun client n'est considéré vivant
POLL_MAX = 10.0         # durée maximale d'un /api/pending tenu (?wait=)

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
_cond = threading.Condition(_lock)   # réveille les /api/pending en attente
_queue = []             # chemins en attente d'ouverture, déposés par le lanceur
_polling = 0            # /api/pending tenus en ce moment : autant de fenêtres vivantes
_last_seen = 0.0        # dernier /api/pending d'une fenêtre
_seen_once = False


def client_alive():
    with _lock:
        return _polling > 0 or (_seen_once and (time.time() - _last_seen) < POLL_ALIVE)


def stamp():
    return datetime.now().strftime("%Y%m%d-%H%M%S")


def relaunch_after_close(paths):
    """Rouvre l'app une fois sa fenetre fermee (une police installee n'est vue par Chromium
    qu'a son demarrage) : attend que plus aucune fenetre n'interroge, oublie la derniere vue
    (sinon le lanceur la croirait encore la, POLL_ALIVE durant, et n'ouvrirait rien), remet
    les fichiers ouverts en file, puis lance le lanceur, qui ouvre une fenetre qui les reprend."""
    def run():
        global _seen_once, _last_seen
        deadline = time.time() + 20
        while time.time() < deadline:
            with _lock:
                if _polling == 0:
                    break
            time.sleep(0.1)
        time.sleep(0.3)
        with _cond:
            _seen_once, _last_seen = False, 0.0
            for p in paths:
                if p not in _queue:
                    _queue.append(p)
            _cond.notify_all()
        import subprocess
        kw = {"creationflags": 0x00000008} if os.name == "nt" else {"start_new_session": True}   # DETACHED_PROCESS / setsid
        subprocess.Popen([sys.executable, os.path.join(HERE, "csvfab.py")], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kw)
    threading.Thread(target=run, daemon=True).start()


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
    # Les tests « in » et isprintable() sont en C : la plupart des cellules n'ont
    # rien a echapper et s'en sortent sans les quatre passes de remplacement.
    if not v.isprintable():
        v = v.translate(_ILLEGAL)
    if "&" in v:
        v = v.replace("&", "&amp;")
    if "<" in v:
        v = v.replace("<", "&lt;")
    if ">" in v:
        v = v.replace(">", "&gt;")
    return v


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
        # Un texte qui ne commence ni par un chiffre, ni par un signe, ni par une
        # devise n'est ni nombre, ni pourcentage, ni date : les quatre motifs sont
        # inutiles (noms, villes, e-mails — la plupart des cellules).
        if v[0] not in "0123456789-+€$£.":
            return
        if v.isdigit():                           # un entier nu, le cas courant : aucun motif a essayer
            self.fr += 1
            self.en += 1
            if v[0] == "0" and len(v) > 1:
                self.lead0 = True
            if len(v) > 15:
                self.long = True
            return
        digits = v.lstrip("-")
        if digits[:1] == "0" and len(digits) > 1 and digits[1:2].isdigit():
            self.lead0 = True                     # 007, 06 12 34 56 78, 01000
        if "/" in v or "-" in digits or "T" in v:  # une date, ou rien : aucun nombre ne contient cela
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
            return
        if v[-1] == "%":
            m = _PCT.match(v)
            if m:
                self.pct += 1
                num = m.group(1).replace(",", ".")
                if "." in num:
                    self.pct_dec = max(self.pct_dec, len(num) - num.index(".") - 1)
            return
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

    # compresslevel=1 : la feuille d'un CSV de 130 Mo pese 780 Mo de XML ; au niveau
    # 6 la compression seule prenait 6 s, au niveau 1 2,6 s pour un classeur 25 % plus
    # gros (92 Mo au lieu de 74) — le temps d'attente compte plus que les octets.
    with zipfile.ZipFile(out_path, "w", zipfile.ZIP_DEFLATED, compresslevel=1) as zf:
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

            # Par colonne, ce qui ne change pas d'une ligne a l'autre : la reference,
            # le type, l'attribut de style — 12 millions de cellules passent ici.
            kinds = [st.kind for st in stats]
            s_attrs = [' s="%d"' % s if s else "" for s in style_of]
            esc, fmt_num = _xml_escape, _fmt_num
            # Valeur Excel deja formatee, par colonne et par texte de cellule : les
            # scores, departements, dates et montants se repetent d'une ligne a l'autre.
            memo = [{} for _ in stats]

            def num(i, val):
                m = memo[i]
                x = m.get(val, m)
                if x is m:
                    y = _num(kinds[i], val)
                    x = fmt_num(y) if y is not None else None
                    if len(m) < 200000:
                        m[val] = x
                return x
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
                if is_head:
                    buf = ['<row r="1" ht="22" customHeight="1">']
                    for i, val in enumerate(row):
                        buf.append('<c r="%s1" s="1" t="inlineStr"><is><t xml:space="preserve">%s</t></is></c>' % (_COLS[i], esc(val)))
                    buf.append("</row>")
                    out.write("".join(buf).encode("utf-8"))
                    continue
                r = str(written)
                buf = ['<row r="', r, '">']
                for i, val in enumerate(row):
                    if not val:
                        continue
                    x = num(i, val) if kinds[i] != "text" else None
                    if x is not None:
                        buf.append('<c r="' + _COLS[i] + r + '"' + s_attrs[i] + '><v>' + x + '</v></c>')
                    else:
                        buf.append('<c r="' + _COLS[i] + r + '" t="inlineStr"><is><t xml:space="preserve">' + esc(val) + '</t></is></c>')
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


# ---------------------------------------------------------------------------
# LECTURE XLSX -> CSV
#
# L'editeur n'ouvre que des CSV : un classeur est d'abord ecrit en CSV a cote
# de lui, et c'est ce CSV qui devient l'onglet. Premiere feuille seulement,
# lue en flux (iterparse) : les valeurs comme texte, les nombres au format
# le plus court, les cellules dont le style est un format de date en
# yyyy-mm-dd[ hh:mm:ss], les booleens en TRUE/FALSE. Virgule decimale quand
# le CSV est en « ; » (la convention des tableurs francais, comme a l'export).
# ---------------------------------------------------------------------------
_NS = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
_RNS = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
_DATE_FMT_IDS = set(range(14, 23)) | set(range(27, 37)) | set(range(45, 48)) | set(range(50, 59))


def _xlsx_sheets(zf):
    """Les feuilles du classeur, dans l'ordre des onglets : [{name, path, hidden}], plus dates 1904 ?"""
    wb = ET.fromstring(zf.read("xl/workbook.xml"))
    sheets = wb.findall(f"{_NS}sheets/{_NS}sheet")
    if not sheets:
        raise ValueError("classeur sans feuille")
    targets = {rel.get("Id"): rel.get("Target") for rel in ET.fromstring(zf.read("xl/_rels/workbook.xml.rels"))}
    out = []
    for i, sh in enumerate(sheets):
        target = targets.get(sh.get(f"{_RNS}id"))
        if not target:
            continue
        path = target.lstrip("/") if target.startswith("/") else "xl/" + target
        out.append({"name": sh.get("name") or f"Sheet{i + 1}", "path": path,
                    "hidden": sh.get("state") in ("hidden", "veryHidden")})
    if not out:
        raise ValueError("feuille introuvable dans le classeur")
    pr = wb.find(f"{_NS}workbookPr")
    return out, pr is not None and pr.get("date1904") in ("1", "true")


def _xlsx_sheet_probe(zf, path):
    """(remplie ?, taille annoncee par <dimension>). Lu en flux et arrete a la premiere
    cellule qui porte une valeur : une feuille vide (le cas courant : Feuil2, Feuil3,
    des cellules seulement mises en forme) n'en a aucune, une pleine en a une tout de suite."""
    dim = None
    with zf.open(path) as f:
        for ev, el in ET.iterparse(f, events=("start", "end")):
            if ev == "start":
                if el.tag == f"{_NS}dimension":
                    m = _re.match(r"([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?$", el.get("ref", ""))
                    if m and m.group(3):
                        dim = [int(m.group(4)) - int(m.group(2)) + 1, _xlsx_col(m.group(3)) - _xlsx_col(m.group(1)) + 1]
                continue
            if el.tag == f"{_NS}v" and (el.text or "").strip():
                return True, dim
            if el.tag == f"{_NS}t" and (el.text or "").strip():      # une chaine en ligne (inlineStr)
                return True, dim
            if el.tag == f"{_NS}row":
                el.clear()
    return False, dim


def xlsx_sheet_list(src):
    """Toutes les feuilles avec leur etat, pour que la page propose un choix."""
    with zipfile.ZipFile(src) as zf:
        sheets, _ = _xlsx_sheets(zf)
        for sh in sheets:
            sh["filled"], sh["dim"] = _xlsx_sheet_probe(zf, sh["path"])
            del sh["path"]
    return sheets


def _xlsx_pick(zf, sheets, wanted):
    """La feuille demandee par son nom ; a defaut, la premiere visible et remplie."""
    if wanted:
        for sh in sheets:
            if sh["name"] == wanted:
                return sh
        raise ValueError(f"pas de feuille « {wanted} »")
    for sh in sheets:
        if not sh["hidden"] and _xlsx_sheet_probe(zf, sh["path"])[0]:
            return sh
    return sheets[0]


def _xlsx_shared_strings(zf):
    if "xl/sharedStrings.xml" not in zf.namelist():
        return []
    out = []
    with zf.open("xl/sharedStrings.xml") as f:
        for _, el in ET.iterparse(f):
            if el.tag == f"{_NS}si":
                out.append("".join(t.text or "" for t in el.iter(f"{_NS}t")))
                el.clear()
    return out


def _xlsx_date_styles(zf):
    """Indices des styles (cellXfs) dont le format de nombre est une date ou une heure."""
    if "xl/styles.xml" not in zf.namelist():
        return set()
    root = ET.fromstring(zf.read("xl/styles.xml"))
    custom = set()
    for nf in root.iter(f"{_NS}numFmt"):
        code = nf.get("formatCode", "")
        bare = _re.sub(r'"[^"]*"|\[[^\]]*\]|\\.', "", code).lower()   # sans les textes cites, [$-40C], [h]
        if _re.search(r"[ymdhs]", bare) and not _re.search(r"[0#?]", bare):
            custom.add(int(nf.get("numFmtId", "-1")))
    xfs = root.find(f"{_NS}cellXfs")
    out = set()
    if xfs is not None:
        for i, xf in enumerate(xfs.findall(f"{_NS}xf")):
            fid = int(xf.get("numFmtId", "0"))
            if fid in _DATE_FMT_IDS or fid in custom:
                out.add(i)
    return out


def _xlsx_col(ref):
    """'BC12' -> 54 (0 = A)."""
    n = 0
    for ch in ref:
        if not ch.isalpha():
            break
        n = n * 26 + ord(ch.upper()) - 64
    return n - 1


def _xlsx_date_text(x, d1904):
    base = datetime(1904, 1, 1) if d1904 else datetime(1899, 12, 30)
    try:
        t = base + timedelta(seconds=round(x * 86400))
    except (OverflowError, ValueError):
        return None
    if t.hour or t.minute or t.second:
        return t.strftime("%Y-%m-%d %H:%M:%S" if t.second else "%Y-%m-%d %H:%M")
    return t.strftime("%Y-%m-%d")


def _xlsx_num_text(raw, comma):
    try:
        x = float(raw)
    except ValueError:
        return raw
    s = str(int(x)) if x == int(x) and abs(x) < 1e15 else repr(x)
    return s.replace(".", ",") if comma else s


def xlsx_to_csv(src, out, delim, sheet=None):
    """Une feuille de src (chemin ou fichier ouvert) -> lignes CSV dans out (texte). Renvoie un bilan.
    sheet : son nom ; sans, la premiere feuille visible qui porte une valeur."""
    with zipfile.ZipFile(src) as zf:
        sheets, d1904 = _xlsx_sheets(zf)
        sh = _xlsx_pick(zf, sheets, sheet)
        name, sheet_path = sh["name"], sh["path"]
        # Les autres feuilles qui portent des donnees, pour le bilan : les vides et les masquees ne comptent pas.
        others = sum(1 for o in sheets if o is not sh and not o["hidden"] and _xlsx_sheet_probe(zf, o["path"])[0])
        shared = _xlsx_shared_strings(zf)
        dates = _xlsx_date_styles(zf)
        comma = delim == ";"
        w = csv.writer(out, delimiter=delim, lineterminator="\n")
        width = rows = maxw = 0
        with zf.open(sheet_path) as f:
            for ev, el in ET.iterparse(f, events=("start", "end")):
                if ev == "start":
                    if el.tag == f"{_NS}dimension":
                        m = _re.match(r"[A-Z]+\d*(?::([A-Z]+)\d*)?$", el.get("ref", ""))
                        if m and m.group(1):
                            width = _xlsx_col(m.group(1)) + 1
                    continue
                if el.tag != f"{_NS}row":
                    continue
                cells = []
                for c in el.findall(f"{_NS}c"):
                    idx = _xlsx_col(c.get("r")) if c.get("r") else len(cells)
                    while len(cells) < idx:
                        cells.append("")
                    t, s, v = c.get("t"), c.get("s"), c.find(f"{_NS}v")
                    raw = v.text if v is not None and v.text is not None else None
                    if t == "s":
                        val = shared[int(raw)] if raw is not None else ""
                    elif t == "inlineStr":
                        is_ = c.find(f"{_NS}is")
                        val = "".join(x.text or "" for x in is_.iter(f"{_NS}t")) if is_ is not None else ""
                    elif t == "b":
                        val = "TRUE" if raw == "1" else "FALSE"
                    elif t in ("str", "e"):
                        val = raw or ""
                    elif raw is None:
                        val = ""
                    elif s is not None and int(s) in dates:
                        try:
                            val = _xlsx_date_text(float(raw), d1904) or raw
                        except ValueError:
                            val = raw
                    else:
                        val = _xlsx_num_text(raw, comma)
                    cells.append(val)
                if not width and any(cells):
                    width = len(cells)            # pas de <dimension> : la premiere ligne (les titres) fixe la largeur
                while len(cells) < width:
                    cells.append("")
                if any(cells):                    # les lignes vides de mise en forme n'ont rien a faire dans le CSV
                    w.writerow(cells)
                    rows += 1
                    maxw = max(maxw, len(cells))
                el.clear()
    return {"sheet": name, "others": others, "rows": rows, "cols": max(width, maxw)}


# Les familles de polices a chasse fixe installees, pour le choix de police de la page :
# fc-list les connait (Linux, et macOS quand fontconfig y est) ; sans lui, une liste vide
# et la page passe par l'API du navigateur. Les familles nommees d'apres un style
# (« Aptos Mono Bold ») sont ramenees a leur famille de base quand elle existe.
# --- SQLite : ouvrir une table comme CSV, ecrire un CSV comme base -----------------------
# sqlite3 est dans la bibliotheque standard : rien a installer. Importe a la demande, comme
# le reste des conversions, pour ne pas ralentir le demarrage du serveur.
SQLITE_MAGIC = b"SQLite format 3\x00"


def _sqlite_ro(path):
    """Connexion en lecture seule : ouvrir une base pour la lire ne doit jamais l'ecrire
    (ni creer un journal a cote)."""
    import sqlite3
    with open(path, "rb") as f:
        if f.read(16) != SQLITE_MAGIC:
            raise ValueError("ce n'est pas une base SQLite")
    return sqlite3.connect("file:" + urllib.parse.quote(os.path.abspath(path)) + "?mode=ro&immutable=1", uri=True)


def _sql_ident(name):
    return '"' + str(name).replace('"', '""') + '"'


def sqlite_table_list(path):
    """Tables et vues d'une base : {name, view, hidden, filled, dim: [lignes, colonnes]},
    au format des feuilles d'un classeur, pour que la page propose le meme choix."""
    con = _sqlite_ro(path)
    try:
        out = []
        for name, typ in con.execute("SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') "
                                     "AND name NOT LIKE 'sqlite_%' ORDER BY type = 'view', name"):
            q = _sql_ident(name)
            try:
                cols = len(con.execute(f"SELECT * FROM {q} LIMIT 0").description or [])
                n = con.execute(f"SELECT COUNT(*) FROM {q}").fetchone()[0]
            except Exception:                    # une vue cassee (table disparue) : listee, vide
                cols, n = 0, 0
            out.append({"name": name, "view": typ == "view", "hidden": False, "filled": n > 0, "dim": [n, cols]})
        return out
    finally:
        con.close()


def sqlite_to_csv(src, out, delim, table=None):
    """Une table (ou vue) de src -> CSV dans out, la ligne d'en-tete d'abord ; sans table nommee,
    la premiere table non vide. Nombres comme dans un classeur (virgule decimale dans un
    fichier en ;), NULL vide, BLOB en hexadecimal (0x…)."""
    tables = sqlite_table_list(src)
    if not tables:
        raise ValueError("la base ne contient aucune table")
    pick = next((t for t in tables if t["name"] == table), None) if table else \
        next((t for t in tables if t["filled"] and not t["view"]), None) or tables[0]
    if not pick:
        raise ValueError(f"table introuvable : {table}")
    comma = delim == ";"

    def cell(v):
        if v is None:
            return ""
        if isinstance(v, float):
            return _xlsx_num_text(repr(v), comma)
        if isinstance(v, (bytes, memoryview)):
            return "0x" + bytes(v).hex()
        return str(v)
    con = _sqlite_ro(src)
    try:
        cur = con.execute(f"SELECT * FROM {_sql_ident(pick['name'])}")
        names = [d[0] for d in cur.description]
        w = csv.writer(out, delimiter=delim, lineterminator="\n")
        w.writerow(names)
        rows = 0
        while True:
            batch = cur.fetchmany(10000)
            if not batch:
                break
            w.writerows([cell(v) for v in r] for r in batch)
            rows += len(batch)
    finally:
        con.close()
    others = sum(1 for t in tables if t is not pick and t["filled"])
    return {"table": pick["name"], "others": others, "rows": rows, "cols": len(names)}


_SQL_INT = _re.compile(r"^-?(0|[1-9]\d{0,14})$")
_SQL_CODE = _re.compile(r"^(0\d+|\d{16,})$")       # un zero en tete ou plus de 15 chiffres : un code, pas un nombre
_SQL_NUM = (_re.compile(r"^-?\d+([.,]\d+)?$"), _re.compile(r"^-?\d{1,3}(,\d{3})+(\.\d+)?$"), _re.compile(r"^-?\d{1,3}(\.\d{3})+(,\d+)?$"))


def _sql_real(v):
    """Un nombre ecrit a la francaise ou a l'anglaise (1 234,50 · 1,234.50 · 12,5) -> float, sinon None."""
    x = v.replace(" ", "").replace(" ", "").replace(" ", "")
    if not any(r.match(x) for r in _SQL_NUM):
        return None
    if "," in x and "." in x:
        x = x.replace(",", "") if x.rfind(".") > x.rfind(",") else x.replace(".", "").replace(",", ".")
    elif _SQL_NUM[1].match(x):
        x = x.replace(",", "")
    elif _SQL_NUM[2].match(x):
        x = x.replace(".", "")
    else:
        x = x.replace(",", ".")
    try:
        return float(x)
    except ValueError:
        return None


def write_sqlite(open_rows, out_path, table, header=True):
    """Le CSV relu deux fois (open_rows : un csv.reader neuf a chaque appel) : d'abord le type de
    chaque colonne — INTEGER quand toutes ses valeurs sont des entiers (sans zero en tete : un code
    postal reste du texte), REAL quand toutes sont des nombres, TEXT sinon —, puis l'ecriture,
    les cellules vides en NULL. Une seule table, nommee d'apres le fichier."""
    import sqlite3
    names, kinds, width = None, None, 0
    for k, row in enumerate(open_rows()):
        if k == 0 and header:
            names = row
            continue
        if kinds is None:
            kinds = []
        width = max(width, len(row))
        while len(kinds) < width:
            kinds.append([0, 0, 0, 0])           # valeurs, entiers, nombres, codes
        for c, v in enumerate(row):
            v = v.strip()
            if not v:
                continue
            kc = kinds[c]
            kc[0] += 1
            if _SQL_CODE.match(v):               # 01000, 0612345678, un identifiant de 16 chiffres : du texte
                kc[3] += 1
            elif _SQL_INT.match(v):
                kc[1] += 1
                kc[2] += 1
            elif _sql_real(v) is not None:
                kc[2] += 1
    kinds = kinds or []
    width = max(width, len(names or []))
    while len(kinds) < width:
        kinds.append([0, 0, 0, 0])
    types = ["TEXT" if kc[3] else "INTEGER" if kc[0] and kc[1] == kc[0] else "REAL" if kc[0] and kc[2] == kc[0] else "TEXT" for kc in kinds]
    cols, seen = [], {}
    for c in range(width):
        n = ((names[c] if names and c < len(names) else "") or "").strip() or f"col{c + 1}"
        base, i = n, 2
        while n.lower() in seen:
            n, i = f"{base}_{i}", i + 1
        seen[n.lower()] = 1
        cols.append(n)
    if os.path.exists(out_path):
        os.unlink(out_path)
    con = sqlite3.connect(out_path)
    rows = 0
    try:
        con.execute(f"CREATE TABLE {_sql_ident(table)} (" + ", ".join(f"{_sql_ident(n)} {t}" for n, t in zip(cols, types)) + ")")
        ins = f"INSERT INTO {_sql_ident(table)} VALUES (" + ", ".join("?" * width) + ")"

        def conv(row):
            out = []
            for c in range(width):
                v = row[c].strip() if c < len(row) else ""
                if not v:
                    out.append(None)
                elif types[c] == "INTEGER":
                    out.append(int(v))
                elif types[c] == "REAL":
                    out.append(_sql_real(v))
                else:
                    out.append(row[c])           # le texte tel qu'ecrit, espaces compris
            return out
        batch = []
        for k, row in enumerate(open_rows()):
            if k == 0 and header:
                continue
            batch.append(conv(row))
            if len(batch) >= 10000:
                con.executemany(ins, batch)
                rows += len(batch)
                batch = []
        if batch:
            con.executemany(ins, batch)
            rows += len(batch)
        con.commit()
    finally:
        con.close()
    return {"rows": rows, "cols": width, "table": table, "types": dict(zip(cols, types))}


_STYLE_RE = _re.compile(r"\s+(Thin|ExtraLight|Light|Regular|Medium|SemiBold|Bold|ExtraBold|Black|Italic|Oblique|Condensed|Wide|SemiWide)(\s+\w+)*$", _re.I)


def mono_families():
    exe = shutil.which("fc-list")
    if not exe:
        return []
    import subprocess
    try:
        out = subprocess.run([exe, ":spacing=100", "family"], capture_output=True, text=True, timeout=5).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    names = {line.split(",")[0].strip() for line in out.splitlines() if line.strip()}
    base = {n for n in names if not _STYLE_RE.search(n)}
    return sorted({n if n in base or _STYLE_RE.sub("", n) not in base else _STYLE_RE.sub("", n) for n in names}, key=str.lower)


# ---------------------------------------------------------------------------
# Polices libres a installer depuis la page (licence SIL OFL, sauf mention).
# Un catalogue fixe : la page ne donne qu'un identifiant, jamais une adresse.
# La derniere version de chaque projet est prise sur GitHub (API des releases),
# et l'archive n'est pas telechargee en entier : un zip garde son repertoire a
# la fin, on le lit par requetes Range (quelques Ko), puis seulement les fichiers
# de police retenus — 24 TTF de Cascadia Code dans une archive de 147 Mo.
# Installees pour l'utilisateur seul : ~/.local/share/fonts/csvfab/<id> + fc-cache
# (Linux), ~/Library/Fonts (macOS), %LOCALAPPDATA%\Microsoft\Windows\Fonts + la
# cle HKCU des polices (Windows). macOS et Windows : non essayes sur une vraie machine.
# ---------------------------------------------------------------------------
_STATIC = r"(Thin|ExtraLight|Light|Regular|Medium|SemiBold|Semibold|Bold|ExtraBold|Black)?(Italic|It)?"
FONT_CATALOG = [
    # id, nom affiche, famille, depot, archive (regex sur les fichiers de la derniere release, ou
    # une adresse fixe quand le projet n'en publie pas), fichiers retenus (regex), licence, note
    ("cascadia-code", "Cascadia Code", "Cascadia Code", "microsoft/cascadia-code", r"^CascadiaCode-.*\.zip$", r"^ttf/static/CascadiaCode-[A-Za-z]+\.ttf$", "OFL", "Microsoft, with ligatures"),
    ("cascadia-mono", "Cascadia Mono", "Cascadia Mono", "microsoft/cascadia-code", r"^CascadiaCode-.*\.zip$", r"^ttf/static/CascadiaMono-[A-Za-z]+\.ttf$", "OFL", "Cascadia without ligatures"),
] + [
    (f"monaspace-{n.lower()}", f"Monaspace {n}", f"Monaspace {n}", "githubnext/monaspace", r"^monaspace-static-.*\.zip$",
     rf"^Static Fonts/Monaspace {n}/Monaspace{n}-{_STATIC}\.otf$", "OFL", note) for n, note in (
        ("Neon", "GitHub, neo-grotesque"), ("Argon", "GitHub, humanist"), ("Xenon", "GitHub, slab serif"), ("Radon", "GitHub, handwriting"), ("Krypton", "GitHub, mechanical"))
] + [
    ("jetbrains-mono", "JetBrains Mono", "JetBrains Mono", "JetBrains/JetBrainsMono", r"\.zip$", r"^fonts/ttf/JetBrainsMono-[A-Za-z]+\.ttf$", "OFL", "JetBrains"),
    ("fira-code", "Fira Code", "Fira Code", "tonsky/FiraCode", r"\.zip$", r"^ttf/FiraCode-(Light|Regular|Medium|SemiBold|Bold)\.ttf$", "OFL", "Mozilla's Fira, with ligatures"),
    ("geist-mono", "Geist Mono", "Geist Mono", "vercel/geist-font", r"\.zip$", r"^geist-font/GeistMono/ttf/GeistMono-[A-Za-z]+\.ttf$", "OFL", "Vercel"),
    ("commit-mono", "Commit Mono", "CommitMono", "eigilnikolajsen/commit-mono", r"\.zip$", r"/ttfautohint/CommitMono-[0-9]+-[A-Za-z]+\.ttf$", "OFL", "neutral, Swiss-like"),
    ("maple-mono", "Maple Mono", "Maple Mono", "subframe7536/maple-font", r"^MapleMono-TTF\.zip$", r"^MapleMono-[A-Za-z]+\.ttf$", "OFL", "rounded"),
    ("intel-one-mono", "Intel One Mono", "Intel One Mono", "intel/intel-one-mono", r"^ttf\.zip$", r"^ttf/IntelOneMono-[A-Za-z]+\.ttf$", "OFL", "Intel, made for legibility"),
    ("0xproto", "0xProto", "0xProto", "0xType/0xProto", r"\.zip$", r"^fonts/0xProto-[A-Za-z]+\.ttf$", "OFL", "clear 0 / O, l / 1"),
    ("source-code-pro", "Source Code Pro", "Source Code Pro", "adobe-fonts/source-code-pro", r"^TTF-.*\.zip$", r"^TTF/SourceCodePro-[A-Za-z]+\.ttf$", "OFL", "Adobe"),
    ("hack", "Hack", "Hack", "source-foundry/Hack", r"-ttf\.zip$", r"^ttf/Hack-[A-Za-z]+\.ttf$", "MIT", "from Bitstream Vera / DejaVu"),
    # Monaco est une police d'Apple, non redistribuable : Meslo LG est la version libre de
    # Menlo, qui lui a succede sur Mac. Pas de release sur GitHub : l'archive du depot.
    ("meslo-lg", "Meslo LG", "Meslo LG M", "andreberg/Meslo-Font",
     "https://raw.githubusercontent.com/andreberg/Meslo-Font/master/dist/v1.2.1/Meslo%20LG%20v1.2.1.zip", r"/MesloLGM-[A-Za-z]+\.ttf$", "Apache 2.0", "free Menlo, the closest to Monaco"),
]
_UA = {"User-Agent": "csvfab"}


class _RangeFile(io.RawIOBase):
    """Un fichier distant lu par plages HTTP, que zipfile parcourt comme un fichier local."""

    def __init__(self, url):
        import urllib.request
        self._u = urllib.request
        with urllib.request.urlopen(urllib.request.Request(url, method="HEAD", headers=_UA), timeout=30) as r:
            self.url, self.size = r.url, int(r.headers["Content-Length"])   # l'adresse apres redirection (stockage GitHub)
        self.pos = 0

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = off if whence == 0 else self.pos + off if whence == 1 else self.size + off
        return self.pos

    def read(self, n=-1):
        if n is None or n < 0:
            n = self.size - self.pos
        if n <= 0 or self.pos >= self.size:
            return b""
        end = min(self.size, self.pos + n) - 1
        req = self._u.Request(self.url, headers={**_UA, "Range": f"bytes={self.pos}-{end}"})
        with self._u.urlopen(req, timeout=60) as r:
            data = r.read()
        self.pos += len(data)
        return data

    def readinto(self, b):
        d = self.read(len(b))
        b[:len(d)] = d
        return len(d)


def _font_dir():
    if os.name == "nt":
        return os.path.join(os.environ.get("LOCALAPPDATA") or os.path.expanduser("~\\AppData\\Local"), "Microsoft", "Windows", "Fonts")
    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/Fonts")
    return os.path.join(os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share"), "fonts", "csvfab")


def font_catalog():
    have = {f.lower() for f in mono_families()}
    return [{"id": i, "name": n, "family": fam, "repo": repo, "license": lic, "note": note, "installed": fam.lower() in have}
            for i, n, fam, repo, _a, _f, lic, note in FONT_CATALOG]


def install_font(fid):
    """Telecharge et installe une police du catalogue ; renvoie un bilan."""
    import urllib.request
    entry = next((e for e in FONT_CATALOG if e[0] == fid), None)
    if not entry:
        raise ValueError("police inconnue")
    _i, name, family, repo, asset_re, file_re, _lic, _note = entry
    if asset_re.startswith("https://"):
        url, version = asset_re, None
    else:
        with urllib.request.urlopen(urllib.request.Request(f"https://api.github.com/repos/{repo}/releases/latest", headers=_UA), timeout=30) as r:
            rel = json.loads(r.read().decode("utf-8"), strict=False)
        asset = next((a for a in rel.get("assets", []) if _re.search(asset_re, a["name"])), None)
        if not asset:
            raise ValueError(f"pas d'archive dans la derniere version de {repo}")
        url, version = asset["browser_download_url"], rel.get("tag_name")
    zf = zipfile.ZipFile(_RangeFile(url))
    members = [m for m in zf.namelist() if _re.search(file_re, m)]
    if not members:
        raise ValueError("aucun fichier de police reconnu dans l'archive")
    dest = _font_dir() if os.name == "nt" or sys.platform == "darwin" else os.path.join(_font_dir(), fid)
    os.makedirs(dest, exist_ok=True)
    written = []
    for m in members:
        base = os.path.basename(m)
        if not base or not _re.fullmatch(r"[\w.\-\[\] ]+\.(ttf|otf)", base, _re.I):   # jamais de chemin venu de l'archive
            continue
        path = os.path.join(dest, base)
        tmp = path + ".part"
        with zf.open(m) as src, open(tmp, "wb") as out:
            shutil.copyfileobj(src, out)
        os.replace(tmp, path)
        written.append(path)
    if os.name == "nt":
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Microsoft\Windows NT\CurrentVersion\Fonts", 0, winreg.KEY_SET_VALUE) as k:
            for p in written:
                kind = "OpenType" if p.lower().endswith(".otf") else "TrueType"
                winreg.SetValueEx(k, f"{os.path.splitext(os.path.basename(p))[0]} ({kind})", 0, winreg.REG_SZ, p)
    elif shutil.which("fc-cache"):
        import subprocess
        subprocess.run(["fc-cache", "-f", dest], capture_output=True, timeout=120)
    return {"ok": True, "id": fid, "name": name, "family": family, "version": version, "files": len(written), "dir": dest}


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "csvfab"
    protocol_version = "HTTP/1.1"
    # TCP_NODELAY : en-têtes et corps partent en deux écritures sur une
    # connexion gardée ouverte ; avec Nagle + l'ACK différé du client, la
    # seconde attendait ~40 ms (mesuré : 46 ms par /api/stat, 3 à chaque
    # ouverture de fichier).
    disable_nagle_algorithm = True

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
        if route == "/app.js":
            return self._app_js()
        if not route.startswith("/api/"):
            return self._static(route)

        if not self._authorized():
            return self._send(403, "forbidden")

        if route == "/api/pending":
            # Sondage long : la réponse part dès qu'un chemin est déposé, sinon
            # après ?wait= secondes (la page relance aussitôt). Sans cela un
            # fichier ouvert alors que l'app tournait déjà attendait le sondage
            # suivant, jusqu'à 1,5 s. Une requête tenue vaut fenêtre vivante.
            global _last_seen, _seen_once, _polling
            try:
                wait = min(float((self._query().get("wait") or ["0"])[0]), POLL_MAX)
            except ValueError:
                wait = 0.0
            deadline, closed = time.time() + wait, False
            with _cond:
                _last_seen = time.time()
                _seen_once = True
                _polling += 1
            try:
                while True:
                    with _cond:
                        left = deadline - time.time()
                        if _queue or left <= 0:
                            paths, _queue[:] = list(_queue), []
                            break
                        _cond.wait(min(left, 0.25))
                    # Fenêtre fermée pendant l'attente ? Son socket devient
                    # lisible (fin de flux) : sans ce test, elle passerait
                    # pour vivante jusqu'à l'échéance, et un fichier ouvert
                    # entre-temps irait dans une file que personne ne viderait.
                    if select.select([self.connection], [], [], 0)[0]:
                        closed = True
                        break
            finally:
                with _cond:
                    _polling -= 1
                    if not closed:
                        _last_seen = time.time()
            if closed:
                return
            try:
                return self._json(200, {"paths": paths})
            except (BrokenPipeError, ConnectionResetError):
                return

        if route == "/api/fonts":
            return self._json(200, {"mono": mono_families(), "catalog": font_catalog()})
        if route == "/api/xlsx-sheets":
            # Les feuilles d'un classeur (?path=), vides et masquees comprises : la page
            # ne propose un choix que s'il y en a plusieurs a garder.
            p = self._path_arg()
            if not p or not os.path.isfile(p):
                return self._json(404, {"error": "not a file"})
            try:
                return self._json(200, {"sheets": xlsx_sheet_list(p)})
            except (zipfile.BadZipFile, KeyError, ET.ParseError, ValueError) as e:
                return self._json(400, {"error": f"ce n'est pas un classeur xlsx lisible ({e})"})
        if route == "/api/sqlite-tables":
            # Les tables et vues d'une base (?path=), au format des feuilles d'un classeur.
            p = self._path_arg()
            if not p or not os.path.isfile(p):
                return self._json(404, {"error": "not a file"})
            try:
                return self._json(200, {"sheets": sqlite_table_list(p)})
            except Exception as e:
                return self._json(400, {"error": f"ce n'est pas une base SQLite lisible ({e})"})
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
        if route not in ("/api/open", "/api/xlsx", "/api/xlsx2csv", "/api/sqlite", "/api/sqlite2csv", "/api/font-install", "/api/relaunch"):
            return self._send(404, "no such route")
        if not self._authorized():
            return self._send(403, "forbidden")
        if route == "/api/xlsx":
            return self._make_xlsx()
        if route == "/api/xlsx2csv":
            return self._xlsx_to_csv()
        if route == "/api/sqlite":
            return self._make_xlsx(sqlite=True)
        if route == "/api/sqlite2csv":
            return self._xlsx_to_csv(sqlite=True)
        if route == "/api/font-install":
            # Le corps est ignore : seul l'identifiant du catalogue compte.
            try:
                return self._json(200, install_font((self._query().get("id") or [""])[0]))
            except Exception as e:
                return self._json(502, {"error": str(e)})
        try:
            n = int(self.headers.get("Content-Length") or 0)
            payload = json.loads(self.rfile.read(n) or b"{}")
        except (ValueError, json.JSONDecodeError):
            return self._json(400, {"error": "bad json"})
        paths = [os.path.realpath(os.path.expanduser(p))
                 for p in payload.get("paths", []) if p]
        if route == "/api/relaunch":
            relaunch_after_close([p for p in paths if os.path.isfile(p)])
            return self._json(200, {"ok": True})
        with _cond:
            for p in paths:
                if p not in _queue:
                    _queue.append(p)
            _cond.notify_all()
        return self._json(200, {"queued": len(paths), "client": client_alive()})

    def _make_xlsx(self, sqlite=False):
        """CSV en entree, classeur en sortie (ou base SQLite d'une table, avec sqlite=True).

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
        tmp = os.path.join(d, f".csvfab.{os.getpid()}.{'sqlite' if sqlite else 'xlsx'}")
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
            info = write_sqlite(open_rows, tmp, sheet, header) if sqlite else write_xlsx(open_rows, tmp, sheet, header)

            if dest:
                os.replace(tmp, dest)
                info["path"] = dest
                info["ok"] = True
                return self._json(200, info)

            with open(tmp, "rb") as f:
                body = f.read()
            return self._send(200, body,
                              "application/vnd.sqlite3" if sqlite else "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                              {"X-Xlsx-Info": json.dumps(info)})
        except Exception as e:
            return self._json(500, {"error": str(e)})
        finally:
            for f in (src, tmp):
                try:
                    os.unlink(f)
                except OSError:
                    pass

    def _xlsx_to_csv(self, sqlite=False):
        """Classeur en entree (?src= un chemin, sinon le corps), CSV en sortie — ou base SQLite
        avec sqlite=True, ?sheet= nommant alors la table.

        Avec ?dest=, le CSV est ecrit la, atomiquement (et ?backup=1 garde une
        copie .bak d'un fichier deja present) ; sinon ses octets sont renvoyes,
        avec le bilan dans X-Xlsx-Info, pour que la page les depose elle-meme.
        """
        q = self._query()
        delim = (q.get("delim") or [";"])[0] or ";"
        if len(delim) != 1:
            return self._json(400, {"error": "delimiteur invalide"})
        src = (q.get("src") or [""])[0]
        src = os.path.realpath(os.path.expanduser(src)) if src else None
        dest = (q.get("dest") or [""])[0]
        dest = os.path.realpath(os.path.expanduser(dest)) if dest else None
        backup = (q.get("backup") or ["0"])[0] == "1"
        sheet = (q.get("sheet") or [""])[0] or None
        # ?pick=1 sans ?sheet= : si plusieurs feuilles visibles portent des valeurs, rien
        # n'est converti, la liste revient pour que la page demande laquelle. Un seul envoi
        # du classeur dans le cas courant (une feuille), deux seulement s'il faut choisir.
        pick = (q.get("pick") or ["0"])[0] == "1" and not sheet
        if src and not os.path.isfile(src):
            return self._json(404, {"error": "fichier introuvable"})
        d = os.path.dirname(dest) if dest else STATE
        if dest and not os.path.isdir(d):
            return self._json(400, {"error": f"dossier inexistant : {d}"})
        tmp_in = None if src else os.path.join(d, f".csvfab.{os.getpid()}.{'sqlite' if sqlite else 'xlsx'}")
        tmp_out = os.path.join(d, f".{os.path.basename(dest)}.{os.getpid()}.part") if dest else None
        try:
            if tmp_in:
                try:
                    n = int(self.headers.get("Content-Length") or 0)
                except ValueError:
                    return self._json(400, {"error": "Content-Length invalide"})
                remaining = n
                with open(tmp_in, "wb") as f:
                    while remaining > 0:
                        chunk = self.rfile.read(min(1 << 20, remaining))
                        if not chunk:
                            raise IOError("flux interrompu avant la fin du corps")
                        f.write(chunk)
                        remaining -= len(chunk)
            try:
                if pick:
                    sheets = sqlite_table_list(src or tmp_in) if sqlite else xlsx_sheet_list(src or tmp_in)
                    if sum(1 for s in sheets if s["filled"] and not s["hidden"]) > 1:
                        return self._json(200, {"choose": sheets})
                if dest:
                    with open(tmp_out, "w", encoding="utf-8", newline="") as out:
                        info = (sqlite_to_csv if sqlite else xlsx_to_csv)(src or tmp_in, out, delim, sheet)
                        out.flush()
                        os.fsync(out.fileno())
                    if backup and os.path.isfile(dest):
                        shutil.copy2(dest, f"{dest}.{stamp()}.bak")
                    os.replace(tmp_out, dest)
                    info.update(ok=True, path=dest, size=os.path.getsize(dest))
                    return self._json(200, info)
                out = io.StringIO()
                info = (sqlite_to_csv if sqlite else xlsx_to_csv)(src or tmp_in, out, delim, sheet)
                return self._send(200, out.getvalue().encode("utf-8"), "text/csv; charset=utf-8",
                                  {"X-Xlsx-Info": json.dumps(info)})
            except (zipfile.BadZipFile, KeyError, ET.ParseError, ValueError, IndexError) as e:
                return self._json(400, {"error": f"ce n'est pas {'une base SQLite' if sqlite else 'un classeur xlsx'} lisible ({e})"})
            except Exception as e:
                if sqlite and type(e).__module__ == "sqlite3":
                    return self._json(400, {"error": f"base SQLite illisible ({e})"})
                raise
        except Exception as e:
            return self._json(500, {"error": str(e)})
        finally:
            for f in (tmp_in, tmp_out):
                if f:
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

    def _app_js(self):
        """/app.js : les sources ui/js/*.js mises bout à bout, dans l'ordre de leur nom.
        Elles se comportent ainsi exactement comme le script unique dont elles sont
        issues : une fonction déclarée dans un fichier suivant reste appelable depuis
        un fichier précédent (hoisting), ce que des <script> séparés ne permettraient
        pas. Le jeton n'y figure pas : il reste dans viewer.htm (cf. _page)."""
        d = os.path.join(HERE, "ui", "js")
        try:
            parts = []
            for name in sorted(f for f in os.listdir(d) if f.endswith(".js")):
                with open(os.path.join(d, name), "r", encoding="utf-8") as f:
                    parts.append(f"/* ---- ui/js/{name} ---- */\n" + f.read())
        except OSError as e:
            return self._send(500, f"ui/js illisible : {e}")
        # allFunctionsCalledOnLoad : V8 compile toutes les fonctions d'emblée (hors
        # du fil principal) au lieu de les découvrir une à une, et le cache de code
        # de Chromium les contient toutes.
        body = ("//# allFunctionsCalledOnLoad\n" + "\n".join(parts)).encode("utf-8")
        return self._send_cached(body, "text/javascript; charset=utf-8",
                                 hashlib.sha1(body).hexdigest()[:20])

    def _send_cached(self, body, ctype, etag):
        """Réponse revalidée à chaque chargement (no-cache + ETag, 304 si inchangée)
        au lieu de no-store : Chromium ne garde le code compilé d'un script (son
        cache de code V8) que si le script lui-même est dans son cache HTTP. Réservé
        aux fichiers de l'app, sans jeton ; la page elle-même reste en no-store."""
        tag = f'"{etag}"'
        if self.headers.get("If-None-Match") == tag:
            self.send_response(304)
            self.send_header("ETag", tag)
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-cache")
        self.send_header("ETag", tag)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

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
        st = os.stat(full)
        with open(full, "rb") as f:
            body = f.read()                      # quelques centaines de Ko au plus (assets de l'app)
        return self._send_cached(body, ctype, f"{st.st_mtime_ns:x}-{st.st_size:x}")

    def _read_file(self):
        p = self._path_arg()
        if not p or not os.path.isfile(p):
            return self._send(404, "not a file")
        try:
            # ?from=N : seulement les octets a partir de N (le suivi d'un journal, tail -f :
            # la page ne relit que ce qui a ete ajoute depuis sa derniere lecture).
            try:
                start = max(0, int((self._query().get("from") or ["0"])[0]))
            except ValueError:
                start = 0
            size = os.path.getsize(p)
            start = min(start, size)
            self.send_response(200)
            self.send_header("Content-Type", "text/csv; charset=utf-8")
            self.send_header("Content-Length", str(size - start))
            self.send_header("X-File-Size", str(size))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            if self.command == "HEAD":
                return
            with open(p, "rb") as f:
                f.seek(start)
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
            seen, last, held = _seen_once, _last_seen, _polling
        if held:
            continue
        idle = time.time() - (last if seen else _started)
        if idle > (IDLE_LIVE if seen else IDLE_BOOT):
            srv.shutdown()
            return


def main():
    global _started
    _started = time.time()
    os.makedirs(STATE, exist_ok=True)

    if _EARLY is not None:
        # Le socket ouvert en tête de fichier ; server_bind() est sauté, et avec
        # lui son socket.getfqdn(), une résolution de nom inutile ici.
        srv = Server((HOST, PORT), Handler, bind_and_activate=False)
        srv.socket.close()
        srv.socket = _EARLY
        srv.server_address = _EARLY.getsockname()
        srv.server_name, srv.server_port = HOST, PORT
    else:
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
