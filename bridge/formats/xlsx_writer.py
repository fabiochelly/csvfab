"""Ecriture d'un CSV en classeur .xlsx.

Un .xlsx n'est qu'un zip de quelques parties XML : le format est assez simple
pour etre ecrit a la main, ce qui evite d'imposer openpyxl a un projet qui n'a
aucune dependance. La feuille est ecrite en flux dans le zip, donc un gros CSV
ne passe jamais entierement par la memoire.
"""

import io
import zipfile

from .xlsx_columns import ColumnStat, excel_value, number_xml

XLSX_MAX_ROWS = 1048576          # limites dures d'Excel
XLSX_MAX_COLS = 16384

_CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">\
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>\
<Default Extension="xml" ContentType="application/xml"/>\
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>\
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>\
<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>\
</Types>"""

_ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>\
</Relationships>"""

_WB_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>\
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>\
</Relationships>"""


def _col_letters(i):
    """0 -> A, 25 -> Z, 26 -> AA."""
    out = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        out = chr(65 + r) + out
    return out


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


MEMO_TOTAL = 500000      # valeurs gardees en tout, toutes colonnes confondues
MEMO_PROBE = 20000       # valeurs nouvelles vues dans une colonne avant de la juger


def value_cache(convert, ncols, rows):
    """num(i, val) = convert(i, val), memorise par colonne et par texte de cellule : les
    scores, departements, dates et montants se repetent d'une ligne a l'autre, et la
    conversion d'une valeur deja vue ne coute plus qu'une recherche.

    Borne, pour qu'une optimisation ne devienne pas une fuite : une colonne dont plus
    d'une valeur sur deux est nouvelle au bout de MEMO_PROBE valeurs (identifiants,
    montants tous differents) cesse d'etre memorisee — la garder coutait de la memoire
    sans rien faire gagner ; et l'ensemble ne depasse pas MEMO_TOTAL valeurs (l'ancien
    plafond, 200 000 par colonne, laissait 100 colonnes atteindre 20 millions d'entrees).
    rows() : le nombre de lignes lues jusque-la. Renvoie num et les memos (pour les tests)."""
    memo = [{} for _ in range(ncols)]
    misses = [0] * ncols
    room = [MEMO_TOTAL]

    def num(i, val):
        m = memo[i]
        if m is not None:
            x = m.get(val, m)
            if x is not m:
                return x
        x = convert(i, val)
        if m is not None:
            k = misses[i] = misses[i] + 1
            if k == MEMO_PROBE and k * 2 > rows():
                room[0] += len(m)
                memo[i] = None
            elif room[0] > 0:
                m[val] = x
                room[0] -= 1
        return x
    return num, memo


def write(open_rows, out_path, sheet="Sheet1", header=True):
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
            stats.append(ColumnStat())
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

    # Les lettres de colonne, autant qu'il en faut : pas les 16 384 d'Excel a chaque demarrage.
    letters = [_col_letters(i) for i in range(max(len(stats), 1))]
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
            esc, fmt_num = _xml_escape, number_xml

            def convert(i, val):
                y = excel_value(kinds[i], val)
                return fmt_num(y) if y is not None else None
            num, _ = value_cache(convert, len(stats), lambda: written)
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
                        buf.append('<c r="%s1" s="1" t="inlineStr"><is><t xml:space="preserve">%s</t></is></c>' % (letters[i], esc(val)))
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
                        buf.append('<c r="' + letters[i] + r + '"' + s_attrs[i] + '><v>' + x + '</v></c>')
                    else:
                        buf.append('<c r="' + letters[i] + r + '" t="inlineStr"><is><t xml:space="preserve">' + esc(val) + '</t></is></c>')
                buf.append("</row>")
                out.write("".join(buf).encode("utf-8"))
            out.write(b"</sheetData>")
            last = "%s%d" % (letters[max(cols_max, 1) - 1], max(written, 1))
            if header and written:
                out.write(('<autoFilter ref="A1:%s"/>' % last).encode("utf-8"))
            out.write(b"</worksheet>")
            out.flush()

        # Ecrit apres la feuille : la plage du filtre n'est connue qu'a la fin.
        defined = ""
        if header and written:
            defined = ('<definedNames><definedName name="_xlnm._FilterDatabase" localSheetId="0" hidden="1">'
                       "'%s'!$A$1:$%s$%d</definedName></definedNames>"
                       % (_xml_escape(name).replace("'", "''"), letters[max(cols_max, 1) - 1], written))
        zf.writestr("xl/workbook.xml",
                    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
                    '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
                    ' xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
                    '<sheets><sheet name="%s" sheetId="1" r:id="rId1"/></sheets>%s</workbook>'
                    % (_xml_escape(name), defined))

    return {"rows": written, "cols": cols_max,
            "truncated_rows": truncated_rows, "truncated_cols": truncated_cols}
