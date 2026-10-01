"""Lecture d'un classeur .xlsx en CSV.

L'editeur n'ouvre que des CSV : un classeur est d'abord ecrit en CSV a cote
de lui, et c'est ce CSV qui devient l'onglet. Une feuille : celle que la page
a fait choisir, sinon la premiere visible qui porte une valeur ; lue en flux
(iterparse) : les valeurs comme texte, les nombres au format
le plus court, les cellules dont le style est un format de date en
yyyy-mm-dd[ hh:mm:ss], les booleens en TRUE/FALSE. Virgule decimale quand
le CSV est en « ; » (la convention des tableurs francais, comme a l'export).
"""

import csv
import re
from array import array
import xml.etree.ElementTree as ET
import zipfile
from datetime import datetime, timedelta

from .common import number_text

_NS = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
_RNS = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
_DATE_FMT_IDS = set(range(14, 23)) | set(range(27, 37)) | set(range(45, 48)) | set(range(50, 59))


def _sheets(zf):
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


def _probe(zf, path):
    """(remplie ?, taille annoncee par <dimension>). Lu en flux et arrete a la premiere
    cellule qui porte une valeur : une feuille vide (le cas courant : Feuil2, Feuil3,
    des cellules seulement mises en forme) n'en a aucune, une pleine en a une tout de suite."""
    dim = None
    with zf.open(path) as f:
        for ev, el in ET.iterparse(f, events=("start", "end")):
            if ev == "start":
                if el.tag == f"{_NS}dimension":
                    m = re.match(r"([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?$", el.get("ref", ""))
                    if m and m.group(3):
                        dim = [int(m.group(4)) - int(m.group(2)) + 1, col_index(m.group(3)) - col_index(m.group(1)) + 1]
                continue
            if el.tag == f"{_NS}v" and (el.text or "").strip():
                return True, dim
            if el.tag == f"{_NS}t" and (el.text or "").strip():      # une chaine en ligne (inlineStr)
                return True, dim
            if el.tag == f"{_NS}row":
                el.clear()
    return False, dim


# Une archive dont la taille decompressee annoncee depasse ceci, ou dont un gros membre se
# compresse plus que du XML ne le fait jamais (5 a 30 fois ; deflate plafonne vers 1 000),
# est une bombe, pas un classeur. Les tailles annoncees bornent ce que zipfile lit vraiment.
XLSX_MAX_TOTAL = 16 << 30
XLSX_MAX_RATIO = 500
XLSX_RATIO_FROM = 256 << 20


def _guard(zf):
    total = 0
    for i in zf.infolist():
        total += i.file_size
        if i.file_size > XLSX_RATIO_FROM and i.file_size > XLSX_MAX_RATIO * max(i.compress_size, 1):
            raise ValueError(f"{i.filename} se decompresse {i.file_size // max(i.compress_size, 1)} fois : archive piegee")
    if total > XLSX_MAX_TOTAL:
        raise ValueError(f"{total >> 30} Gio une fois decompresse : archive refusee")


def sheet_list(src):
    """Toutes les feuilles avec leur etat, pour que la page propose un choix."""
    with zipfile.ZipFile(src) as zf:
        _guard(zf)
        sheets, _ = _sheets(zf)
        for sh in sheets:
            sh["filled"], sh["dim"] = _probe(zf, sh["path"])
            del sh["path"]
    return sheets


def _pick(zf, sheets, wanted):
    """La feuille demandee par son nom ; a defaut, la premiere visible et remplie."""
    if wanted:
        for sh in sheets:
            if sh["name"] == wanted:
                return sh
        raise ValueError(f"pas de feuille « {wanted} »")
    for sh in sheets:
        if not sh["hidden"] and _probe(zf, sh["path"])[0]:
            return sh
    return sheets[0]


SHARED_COMPACT = 32 << 20     # chaines partagees (XML decompresse) au-dela desquelles on les tasse


class _Packed:
    """Les chaines partagees d'un gros classeur : un seul bloc d'octets UTF-8 et la fin de
    chacune, au lieu d'un objet str Python par chaine (~50 octets de surcout chacune : dix
    millions de chaines courtes pesaient pres d'un Go). Lu comme une liste."""

    def __init__(self):
        self.buf, self.ends = bytearray(), array("Q")

    def append(self, s):
        self.buf += s.encode("utf-8")
        self.ends.append(len(self.buf))

    def __getitem__(self, i):
        if i < 0:
            raise IndexError(i)
        return self.buf[self.ends[i - 1] if i else 0:self.ends[i]].decode("utf-8")

    def __len__(self):
        return len(self.ends)


def _shared_strings(zf):
    if "xl/sharedStrings.xml" not in zf.namelist():
        return []
    out = _Packed() if zf.getinfo("xl/sharedStrings.xml").file_size > SHARED_COMPACT else []
    root = None
    with zf.open("xl/sharedStrings.xml") as f:
        for ev, el in ET.iterparse(f, events=("start", "end")):
            if root is None:
                root = el
            if ev == "end" and el.tag == f"{_NS}si":
                out.append("".join(t.text or "" for t in el.iter(f"{_NS}t")))
                root.clear()                  # sinon la racine garde la coquille de chaque <si> lu
    return out


def _date_styles(zf):
    """Indices des styles (cellXfs) dont le format de nombre est une date ou une heure."""
    if "xl/styles.xml" not in zf.namelist():
        return set()
    root = ET.fromstring(zf.read("xl/styles.xml"))
    custom = set()
    for nf in root.iter(f"{_NS}numFmt"):
        code = nf.get("formatCode", "")
        bare = re.sub(r'"[^"]*"|\[[^\]]*\]|\\.', "", code).lower()   # sans les textes cites, [$-40C], [h]
        if re.search(r"[ymdhs]", bare) and not re.search(r"[0#?]", bare):
            custom.add(int(nf.get("numFmtId", "-1")))
    xfs = root.find(f"{_NS}cellXfs")
    out = set()
    if xfs is not None:
        for i, xf in enumerate(xfs.findall(f"{_NS}xf")):
            fid = int(xf.get("numFmtId", "0"))
            if fid in _DATE_FMT_IDS or fid in custom:
                out.add(i)
    return out


def col_index(ref):
    """'BC12' -> 54 (0 = A)."""
    n = 0
    for ch in ref:
        if not ch.isalpha():
            break
        n = n * 26 + ord(ch.upper()) - 64
    return n - 1


def date_text(x, d1904):
    base = datetime(1904, 1, 1) if d1904 else datetime(1899, 12, 30)
    try:
        t = base + timedelta(seconds=round(x * 86400))
    except (OverflowError, ValueError):
        return None
    if t.hour or t.minute or t.second:
        return t.strftime("%Y-%m-%d %H:%M:%S" if t.second else "%Y-%m-%d %H:%M")
    return t.strftime("%Y-%m-%d")


def to_csv(src, out, delim, sheet=None):
    """Une feuille de src (chemin ou fichier ouvert) -> lignes CSV dans out (texte). Renvoie un bilan.
    sheet : son nom ; sans, la premiere feuille visible qui porte une valeur."""
    with zipfile.ZipFile(src) as zf:
        _guard(zf)
        sheets, d1904 = _sheets(zf)
        sh = _pick(zf, sheets, sheet)
        name, sheet_path = sh["name"], sh["path"]
        # Les autres feuilles qui portent des donnees, pour le bilan : les vides et les masquees ne comptent pas.
        others = sum(1 for o in sheets if o is not sh and not o["hidden"] and _probe(zf, o["path"])[0])
        shared = _shared_strings(zf)
        dates = _date_styles(zf)
        comma = delim == ";"
        w = csv.writer(out, delimiter=delim, lineterminator="\n")
        width = rows = maxw = 0
        sd = None                             # <sheetData> : vide de ses lignes au fur et a mesure
        with zf.open(sheet_path) as f:
            for ev, el in ET.iterparse(f, events=("start", "end")):
                if ev == "start":
                    if el.tag == f"{_NS}sheetData":
                        sd = el
                    elif el.tag == f"{_NS}dimension":
                        m = re.match(r"[A-Z]+\d*(?::([A-Z]+)\d*)?$", el.get("ref", ""))
                        if m and m.group(1):
                            width = col_index(m.group(1)) + 1
                    continue
                if el.tag != f"{_NS}row":
                    continue
                cells = []
                for c in el.findall(f"{_NS}c"):
                    idx = col_index(c.get("r")) if c.get("r") else len(cells)
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
                            val = date_text(float(raw), d1904) or raw
                        except ValueError:
                            val = raw
                    else:
                        val = number_text(raw, comma)
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
                if sd is not None:
                    sd.clear()                # sinon <sheetData> garde la coquille de chaque ligne lue (mesure : 24 Mo de pic pour 300 000 lignes, 1 Mo ainsi)
    return {"sheet": name, "others": others, "rows": rows, "cols": max(width, maxw)}
