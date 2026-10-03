#!/usr/bin/env python3
"""Fichiers de test du banc : un export clients typique, généré, déterministe.

    python3 benchmark/gen.py 1000000 customers-1m.csv

Quinze colonnes d'un export de CRM ou de boutique en ligne (identifiant, nom,
e-mail, téléphone, société, adresse, dates, montants, statut), six pays,
accents, quelques champs entre guillemets (une virgule dedans) — ~200 octets
par ligne, donc ~200 Mo pour un million de lignes. Même graine, mêmes octets :
deux machines mesurent le même fichier.

À côté du CSV, <fichier>.json donne ce que chaque outil doit retrouver (lignes,
lignes dont la ville contient « lyon », plus gros montant) : un temps n'est
rapporté que si le résultat est juste.
"""

import json
import os
import random
import sys

SEED = 20261003
HEADER = ["id", "first_name", "last_name", "email", "phone", "company", "street", "postal_code", "city",
          "country", "signup_date", "last_order", "orders", "total_spent", "status"]

# pays : (code, poids, prénoms, noms, villes, rues, format du téléphone, du code postal)
COUNTRIES = [
    ("France", 30,
     "Jean Marie Pierre Michel Nathalie Isabelle Sophie Nicolas Julien Camille Léa Chloé Hugo Lucas Inès Théo Émilie Céline Hélène Benoît François Gaëlle Loïc Zoé Maël".split(),
     "Martin Bernard Dubois Thomas Robert Richard Petit Durand Leroy Moreau Simon Laurent Lefèvre Michel Garcia David Bertrand Roux Vincent Fournier Morel Girard André Mercier Dupont Lambert Bonnet François Martinez Legrand Gauthier Rousseau Blanc Guérin Muller Henry Faure Chevalier Perrin Roussel".split(),
     "Paris Lyon Marseille Toulouse Nice Nantes Strasbourg Montpellier Bordeaux Lille Rennes Reims Le Havre Saint-Étienne Toulon Grenoble Dijon Angers Nîmes Villeurbanne".split(" "),
     ["rue de la République", "avenue Jean Jaurès", "rue Victor Hugo", "boulevard Gambetta", "rue de la Paix", "place de l'Église", "chemin des Vignes", "allée des Tilleuls"],
     "+33 {a} {b:02d} {c:02d} {d:02d} {e:02d}", "{p:05d}"),
    ("United States", 25,
     "James Mary Robert Patricia John Jennifer Michael Linda David Elizabeth William Barbara Richard Susan Joseph Jessica Thomas Sarah Charles Karen".split(),
     "Smith Johnson Williams Brown Jones Miller Davis Wilson Anderson Taylor Moore Jackson White Harris Clark Lewis Walker Hall Young King Wright Lopez Hill Scott Green Adams Baker Nelson Carter".split(),
     "New York|Los Angeles|Chicago|Houston|Phoenix|Philadelphia|San Antonio|San Diego|Dallas|Austin|Seattle|Denver|Boston|Portland|Atlanta|Miami".split("|"),
     ["Main Street", "Oak Avenue", "Maple Drive", "Cedar Lane", "Park Road", "Washington Boulevard", "Lake Street", "Hillside Avenue"],
     "+1 ({a}{b:02d}) {c:03d}-{d:02d}{e:02d}", "{p:05d}"),
    ("United Kingdom", 12,
     "Oliver George Harry Jack Amelia Olivia Isla Emily Charlotte Ava Sophie Thomas William Grace Lily".split(),
     "Smith Jones Taylor Brown Williams Wilson Johnson Davies Robinson Wright Thompson Evans Walker White Roberts Green Hall Wood Jackson Clarke O'Brien".split(),
     "London|Birmingham|Manchester|Leeds|Glasgow|Liverpool|Bristol|Sheffield|Edinburgh|Cardiff|Leicester|Nottingham".split("|"),
     ["High Street", "Station Road", "Church Lane", "Victoria Road", "Green Lane", "Manor Road", "Kings Road"],
     "+44 {a}{b:03d} {c:03d}{d:02d}{e}", "{pc}"),
    ("Germany", 13,
     "Lukas Leon Finn Jonas Paul Mia Emma Hannah Sophia Lena Jürgen Günter Käthe Björn Jörg".split(),
     "Müller Schmidt Schneider Fischer Weber Meyer Wagner Becker Schulz Hoffmann Schäfer Koch Bauer Richter Klein Wolf Schröder Neumann Schwarz Zimmermann".split(),
     "Berlin|Hamburg|München|Köln|Frankfurt am Main|Stuttgart|Düsseldorf|Leipzig|Dortmund|Essen|Bremen|Dresden|Nürnberg".split("|"),
     ["Hauptstraße", "Schulstraße", "Gartenstraße", "Bahnhofstraße", "Dorfstraße", "Bergstraße", "Lindenstraße"],
     "+49 {a}{b:02d} {c:03d}{d:02d}{e}", "{p:05d}"),
    ("Spain", 10,
     "José Antonio Manuel Francisco María Carmen Ana Laura Javier Lucía Sofía Martín Álvaro Íñigo Begoña".split(),
     "García Rodríguez González Fernández López Martínez Sánchez Pérez Gómez Martín Jiménez Ruiz Hernández Díaz Moreno Muñoz Álvarez Romero Alonso Gutiérrez".split(),
     "Madrid|Barcelona|Valencia|Sevilla|Zaragoza|Málaga|Murcia|Palma|Bilbao|Alicante|Córdoba|Valladolid".split("|"),
     ["Calle Mayor", "Calle Real", "Avenida de la Constitución", "Plaza de España", "Calle del Sol", "Gran Vía"],
     "+34 {a}{b:02d} {c:03d} {d:03d}", "{p:05d}"),
    ("Italy", 10,
     "Giuseppe Giovanni Antonio Mario Luigi Francesca Giulia Chiara Sara Alessandro Lorenzo Matteo Niccolò Federica".split(),
     "Rossi Russo Ferrari Esposito Bianchi Romano Colombo Ricci Marino Greco Bruno Gallo Conti De Luca Mancini Costa Giordano Rizzo Lombardi Moretti".split(),
     "Roma|Milano|Napoli|Torino|Palermo|Genova|Bologna|Firenze|Bari|Catania|Venezia|Verona".split("|"),
     ["Via Roma", "Via Garibaldi", "Via Mazzini", "Corso Italia", "Via Dante", "Piazza del Duomo", "Via Verdi"],
     "+39 {a}{b:02d} {c:03d} {d:04d}", "{p:05d}"),
]

COMPANY_SUFFIX = ["SA", "SARL", "SAS", "Inc.", "LLC", "Ltd", "GmbH", "S.L.", "S.p.A.", "& Co", "Group", "Holding"]
COMPANY_WORD = ["Global", "Tech", "Data", "Green", "Blue", "North", "Prime", "Alpha", "Nova", "Smart", "Bright",
                "Solutions", "Systems", "Logistics", "Consulting", "Industries", "Services", "Digital", "Partners", "Labs"]
DOMAINS = ["gmail.com", "yahoo.com", "outlook.com", "hotmail.com", "orange.fr", "free.fr", "gmx.de", "web.de",
           "icloud.com", "proton.me", "libero.it", "aol.com"]
STATUS = ["active"] * 6 + ["inactive"] * 2 + ["pending", "churned", "vip"]
UK_PC = ["SW1A 1AA", "EC1A 1BB", "W1A 0AX", "M1 1AE", "B33 8TH", "CR2 6XH", "DN55 1PT", "LS1 4AP", "G1 1XQ", "EH1 1YZ"]

ASCII = str.maketrans("àâäáãåçéèêëíìîïñóòôöõúùûüýÿßÀÂÄÁÉÈÊËÍÎÏÑÓÔÖÚÙÛÜÇ'",
                      "aaaaaaceeeeiiiinooooouuuuyysAAAAEEEEIIINOOOUUUUC-")


def generate(rows, path, seed=SEED, chunk=20000):
    """Écrit `rows` lignes dans `path` et rend ce que les outils doivent retrouver."""
    rnd = random.Random(seed)
    weights = [c[1] for c in COUNTRIES]
    # Les sociétés : un pool fixe, comme dans un vrai export (beaucoup de clients par société).
    companies = []
    for _ in range(5000):
        a, b = rnd.sample(COMPANY_WORD, 2)
        name = f"{a} {b} {rnd.choice(COMPANY_SUFFIX)}"
        if rnd.random() < 0.15:
            name = f"{rnd.choice(COUNTRIES[0][3])}, {rnd.choice(COUNTRIES[0][3])} & Associés"
        companies.append('"' + name + '"' if "," in name else name)
    expect = {"rows": rows, "city_lyon": 0, "max_total_spent": 0.0}
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(",".join(HEADER) + "\r\n")
        rid = 0
        while rid < rows:
            n = min(chunk, rows - rid)
            # Tirages groupés par colonne : random.choices sur n est ~20× plus rapide qu'un tirage par ligne.
            cs = rnd.choices(COUNTRIES, weights, k=n)
            r = [rnd.random() for _ in range(n)]
            nums = [rnd.getrandbits(32) for _ in range(n)]
            out = []
            for i in range(n):
                rid += 1
                cname, _, firsts, lasts, cities, streets, phone, pcode = cs[i]
                x = nums[i]
                first, last = firsts[x % len(firsts)], lasts[(x >> 5) % len(lasts)]
                city = cities[(x >> 11) % len(cities)]
                email = f"{first.translate(ASCII).lower()}.{last.translate(ASCII).lower().replace(' ', '')}{(x >> 3) % 100}@{DOMAINS[(x >> 15) % len(DOMAINS)]}"
                tel = phone.format(a=1 + x % 9, b=(x >> 4) % 100, c=(x >> 8) % 1000, d=(x >> 12) % 100, e=(x >> 16) % 10)
                if "{pc}" in pcode:
                    pc = UK_PC[(x >> 20) % len(UK_PC)]
                else:
                    pc = pcode.format(p=1000 + (x >> 7) % 98000)
                street = f"{1 + (x >> 18) % 199} {streets[(x >> 22) % len(streets)]}"
                y, m, d = 2015 + (x >> 9) % 10, 1 + (x >> 13) % 12, 1 + (x >> 17) % 28
                orders = int(r[i] * r[i] * 80)
                spent = round(orders * (12 + (x >> 19) % 400) + r[i] * 99, 2) if orders else 0.0
                lo = min(y * 12 + m + (x >> 21) % 36, 2026 * 12 + 8)    # 1 à 36 mois après l'inscription, au plus août 2026
                last_order = f"{(lo - 1) // 12}-{(lo - 1) % 12 + 1:02d}-{1 + (x >> 25) % 28:02d}" if orders else ""
                company = companies[(x >> 6) % len(companies)] if r[i] < 0.7 else ""
                status = STATUS[(x >> 27) % len(STATUS)]
                if "Lyon" in city:
                    expect["city_lyon"] += 1
                if spent > expect["max_total_spent"]:
                    expect["max_total_spent"] = spent
                out.append(f"{rid},{first},{last},{email},{tel},{company},{street},{pc},{city},{cname},"
                           f"{y}-{m:02d}-{d:02d},{last_order},{orders},{spent:.2f},{status}\r\n")
            f.write("".join(out))
    expect["bytes"] = os.path.getsize(path)
    with open(path + ".json", "w", encoding="utf-8") as f:
        json.dump(expect, f)
    return expect


def ensure(rows, path):
    """Le fichier s'il est déjà là, de la bonne taille et de la même graine ; sinon généré."""
    try:
        with open(path + ".json", encoding="utf-8") as f:
            expect = json.load(f)
        if expect.get("rows") == rows and expect.get("bytes") == os.path.getsize(path):
            return expect
    except (OSError, ValueError):
        pass
    return generate(rows, path)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    print(json.dumps(generate(int(sys.argv[1]), sys.argv[2])))
