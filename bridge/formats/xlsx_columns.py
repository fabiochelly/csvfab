"""Typage des colonnes d'un CSV avant son ecriture en classeur (xlsx_writer).

Chaque colonne est analysee en entier avant l'ecriture (premiere passe) :
elle devient numerique, pourcentage ou date quand au moins 90 % de ses
cellules non vides le sont, et seules les cellules conformes sont converties
— les autres restent du texte, rendu tel quel. Garde-fous : un entier a zero
initial (code postal, telephone, identifiant) ou de plus de 15 chiffres
(au-dela, Excel arrondit) laisse toute la colonne en texte.
"""

import re
from datetime import datetime

_SPACES = re.compile(r"[   ]")
_FR_NUM = re.compile(r"^-?(?:\d{1,3}(?:[   ]\d{3})+|\d+)(?:,\d+)?$")    # 1 234,5
_EN_NUM = re.compile(r"^-?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?$")                 # 1,234.5
_PCT = re.compile(r"^(-?\d+(?:[.,]\d+)?)\s?%$")
_YMD = re.compile(r"^(\d{4})-(\d{1,2})-(\d{1,2})(?:[T ](\d{1,2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?$")
_DMY = re.compile(r"^(\d{1,2})[/.-](\d{1,2})[/.-](\d{4}|\d{2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2}))?)?$")
_EPOCH = datetime(1899, 12, 30)
_THRESHOLD = 0.9


class ColumnStat:
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


def excel_value(kind, v):
    """Valeur Excel (float) d'une cellule deja retenue par ColumnStat, ou None."""
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


def number_xml(x):
    """Le texte d'une valeur dans le XML de la feuille : un entier sans .0."""
    return repr(int(x)) if x == int(x) and abs(x) < 1e15 else repr(x)
