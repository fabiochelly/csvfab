"""Le corpus de fichiers difficiles : ce qu'un CSV réel peut contenir de pire.

Chaque cas est un nom, ses octets exacts et ce qu'il éprouve. Le corpus est
généré plutôt que versionné (le .gitignore exclut *.csv, et un octet piégé
s'explique mieux dans le code qui le fabrique que dans un fichier binaire) :

    python3 tests/corpus.py <dossier>     écrit les fichiers, pour les ouvrir à la main

L'invariant éprouvé sur chacun (tests/test_forensic.py) : ouvrir puis
réécrire sans modification redonne les octets d'origine, et une modification
ne change que les octets de l'enregistrement modifié.
"""

import os
import sys

E = "é".encode("utf-8")


def _utf16le(text):
    return b"\xff\xfe" + text.encode("utf-16-le")


def _huge():
    # Un champ de 5 Mo cité, qui contient délimiteurs, guillemets doublés et sauts de ligne.
    blob = ('abc,"def"\n' * 512).replace('"', '""') * 1000
    return ('id,blob,after\n1,"' + blob + '",x\n2,small,y\n').encode("utf-8")


CASES = [
    # (nom, octets, ce qui est éprouvé)
    ("empty.csv", b"", "un fichier vide"),
    ("header_only.csv", b"a;b;c\n", "un en-tête et aucune ligne"),
    ("one_column.csv", b"name\nAlice\nBob\n", "une seule colonne : aucun délimiteur à deviner"),
    ("quoted_newline.csv", b'id,text\n1,"line one\nline two"\n2,plain\n', "un saut de ligne dans un champ cité"),
    ("embedded_crlf.csv", b'id,text\n1,"line one\r\nline two"\n2,plain\n', "un CRLF cité dans un fichier en LF"),
    ("mixed_line_endings.csv", b"a;b\r\n1;2\n3;4\r\n5;6\n", "CRLF et LF mêlés"),
    ("cr_only.csv", b"a;b\r1;2\r3;4\r", "des fins de ligne CR seules (vieux Mac)"),
    ("no_final_newline.csv", b"a;b\n1;2\n3;4", "pas de saut de ligne final"),
    ("blank_lines.csv", b"a;b\n1;2\n\n\n3;4\n\n", "des lignes vides au milieu et à la fin"),
    ("utf8_bom.csv", b"\xef\xbb\xbf" + "nom;ville\nZoé;Lyon\n".encode("utf-8"), "un BOM UTF-8"),
    ("utf16le.csv", _utf16le("nom;ville\r\nZoé;\"Lyon; Rhône\"\r\nŁukasz;Kraków\r\n"), "UTF-16 LE avec BOM, champ cité"),
    ("utf16le_all_quoted.csv", _utf16le('"nom";"ville"\r\n"Zoé";"Lyon"\r\n'), "UTF-16, chaque champ cité sans nécessité"),
    ("utf16be.csv", b"\xfe\xff" + 'a;b\n"x";Zoé\n'.encode("utf-16-be"), "UTF-16 BE avec BOM"),
    ("cp1252.csv", "nom;ville\r\nZoé;Orléans\r\n€uro;Œuvre « ici »\r\n".encode("cp1252"), "Windows-1252, € et guillemets français"),
    ("malformed_quote.csv", b'a,b\n1,"unclosed\n2,3\n', "un guillemet jamais refermé"),
    ("stray_quote.csv", b'a,b\n0,"ok"z "w"\n1,x "quoted" y\n2,"ok"z\n3,4\n',
     "des guillemets au milieu d'un champ ; du texte après un guillemet fermant, sur sa ligne puis jusqu'à la fin du fichier"),
    ("ragged_rows.csv", b"a;b;c\n1;2;3\n4;5\n6;7;8;9\n10\n", "des lignes trop courtes et trop longues"),
    ("huge_field.csv", _huge(), "un champ de 5 Mo"),
    ("nul_character.csv", b"a,b\n1,x\x00y\n2,\x00\n", "des caractères NUL"),
    ("duplicate_headers.csv", b"id;nom;nom;id\n1;a;b;2\n", "des titres de colonne en double"),
    ("empty_headers.csv", b";nom;;\n1;a;;x\n", "des titres vides"),
    ("quoted_header.csv", b'"Name";"City; region";"Say ""hi"""\n"Ann";"Lyon; RA";"x"\n', "un en-tête cité, délimiteur et guillemets dedans"),
    ("semicolon_decimal_comma.csv", "produit;prix;qte\nA;1 234,50;3\nB;-0,5;1\n".encode("utf-8"), "décimales à virgule, espace fine en séparateur de milliers"),
    ("formula_injection.csv", b"a,b,c,d\n=1+1,+cmd|' /C calc'!A0,-2+3,@SUM(A1)\n", "des cellules qui commencent comme des formules"),
    ("emoji.csv", "id;txt\n1;👍🏽\n2;👨‍👩‍👧‍👦\n3;🇫🇷\n4;é\n".encode("utf-8"), "émojis composés (ZWJ, drapeau, teinte) et accent combinant"),
    ("surrogate_edgecases.csv",
     b"id;txt\n1;" + b"\xed\xa0\x80" + b"\n2;" + b"\x80lone" + b"\n3;" + b"\xc0\xaf" + b"\n4;" + b"\xf4\x8f\xbf\xbf" + b"\n5;\xe2\x82",
     "UTF-8 invalide : surrogate encodé, octet de continuation isolé, forme trop longue, fin tronquée"),
    ("mixed_encodings.csv", "nom;ville\nZoé;Lyon\n".encode("utf-8") + "Zoé;Orléans\n".encode("cp1252"),
     "une ligne UTF-8 et une ligne Windows-1252 dans le même fichier"),
    ("utf16le_no_bom.csv", "a;b\r\n1;Zoé\r\n".encode("utf-16-le"), "UTF-16 LE sans BOM"),
    ("utf16le_lone_surrogate.csv", b"\xff\xfe" + "a;b\r\n1;".encode("utf-16-le") + b"\x00\xd8" + "x\r\n".encode("utf-16-le"),
     "UTF-16 invalide : un surrogate haut orphelin"),
    ("random_bytes.csv", bytes((i * 167 + 13) % 256 for i in range(4096)), "4 Ko d'octets quelconques : rien d'un CSV"),
    ("cr_in_unquoted_field.csv", b"a;b\n1;x\ry\n2;z\n", "un CR seul dans un champ non cité d'un fichier en LF"),
    ("long_line.csv", b"a,b\n" + b"x" * (1 << 20) + b",y\n", "une ligne d'un mégaoctet sans guillemets"),
    ("only_blank_lines.csv", b"\n\n\r\n\n", "rien que des lignes vides"),
    ("bom_in_middle.csv", "a;b\n1;\ufeffx\n".encode("utf-8"), "un U+FEFF au milieu d'un champ"),
    ("unquoted_delimiter.csv",
     "id;nom;adresse;ville;cp\n1;Ann;12 rue Vert;Lyon;69001\n2;Bob;5 av. Foch;Paris;75008\n"
     "3;Cy;7; rue Haute;Lyon;69002\n4;Dee;9 bd Sud;Nice;06000\n".encode("utf-8"),
     "un délimiteur non cité dans une adresse (« 7; rue Haute »)"),
    ("unquoted_newline.csv", b"id;adresse;ville\n1;12 rue Vert;Lyon\n2;5 av. Foch\nbat. B;Paris\n3;9 bd Sud;Nice\n",
     "un saut de ligne non cité coupe une ligne en deux"),
    ("tsv.tsv", b"a\tb\tc\n1\t2,5\t\"x\"\n", "des tabulations"),
    ("spaces_around_quotes.csv", b'a, b ,c\n1, "two" ,3\n', "des espaces autour des guillemets"),
    ("trailing_delimiter.csv", b"a;b;\n1;2;\n", "un délimiteur en fin de ligne"),
    ("bom_quoted_header.csv", b'\xef\xbb\xbf"id","label"\r\n"1","un, deux"\r\n', "BOM et en-tête cité, CRLF"),
    ("utf8_no_bom_accents.csv", "Prénom,Âge\nÉlodie,30\nJosé,41\n".encode("utf-8"), "UTF-8 sans BOM, accents dans l'en-tête"),
]

NAMES = [c[0] for c in CASES]


def write(directory):
    os.makedirs(directory, exist_ok=True)
    out = []
    for name, data, _ in CASES:
        path = os.path.join(directory, name)
        with open(path, "wb") as f:
            f.write(data)
        out.append(path)
    return out


if __name__ == "__main__":
    d = sys.argv[1] if len(sys.argv) > 1 else "corpus"
    for p in write(d):
        print(p)
