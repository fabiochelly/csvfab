#!/usr/bin/env python3
"""Banc A/B du côté Python : démarrage, latence des routes, débits, conversions.

Garde-fou de la réorganisation de server.py : rien ne doit devenir plus lent.
Deux mesures séparées du même code s'écartent de ±50 % sur les temps sous la
milliseconde (mesuré), donc on ne compare pas deux exécutions : la version de
référence (extraite de git) et la copie de travail tournent ici en
alternance, essai par essai, et chaque mesure est jugée sur ses paires.

    python3 tests/bench.py                  # copie de travail contre HEAD
    python3 tests/bench.py --ref v1.12.0    # … contre une étiquette ou un commit
    python3 tests/bench.py --quick          # moins d'essais, fichiers plus petits

Une mesure est dite plus lente quand sa médiane dépasse celle de la référence
de plus de --tolerance % ET que la nouvelle version perd assez de ses duels
pour que le hasard ne l'explique pas (test de signe, p < 0,05 : un écart dû
au bruit se répartit à peu près moitié-moitié ; en --quick, trop peu de duels
pour conclure sur le démarrage et les conversions). Code de sortie 1 s'il y en a une. Ce n'est pas un test
unitaire (le nom ne commence pas par test_) : unittest ne le lance pas.
"""

import argparse
import http.client
import io
import json
import math
import os
import shutil
import socket
import statistics
import subprocess
import sys
import tarfile
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tests.support import ROOT, Bridge, free_port, isolated_env, q  # noqa: E402


def extract(rev, dest):
    """La version `rev` du dépôt, telle que git archive la livrerait."""
    data = subprocess.run(["git", "-C", ROOT, "archive", "--format=tar", rev],
                          check=True, capture_output=True).stdout
    with tarfile.open(fileobj=io.BytesIO(data)) as t:
        t.extractall(dest, filter="data")
    return dest


def snapshot(dest):
    """La copie de travail (fichiers suivis et nouveaux, hors ignorés) copiée à
    côté de la référence : les deux lisent alors leurs fichiers sur le même
    disque (/app.js relit ui/js à chaque requête ; kDrive contre tmpfs, c'était
    26 % d'écart pour un code identique)."""
    names = subprocess.run(["git", "-C", ROOT, "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                           check=True, capture_output=True).stdout.decode().split("\0")
    for n in filter(None, names):
        src = os.path.join(ROOT, n)
        if os.path.isfile(src):
            os.makedirs(os.path.dirname(os.path.join(dest, n)), exist_ok=True)
            shutil.copy2(src, os.path.join(dest, n))
    return dest


def sign_p(k, n):
    """Probabilité de perdre au moins k duels sur n par pur hasard (test de signe
    unilatéral) : sous 0,05, la nouvelle version est réellement plus lente."""
    return sum(math.comb(n, i) for i in range(k, n + 1)) / 2 ** n


class Results:
    """Paires d'échantillons (référence, nouvelle) par mesure, en secondes."""

    def __init__(self):
        self.pairs = {}

    def add(self, name, a, b):
        self.pairs.setdefault(name, []).append((a, b))

    def ab(self, name, fa, fb, runs):
        """runs duels, l'ordre alterné à chaque fois (pas de prime au second)."""
        for i in range(runs):
            if i % 2:
                b = fb()
                a = fa()
            else:
                a = fa()
                b = fb()
            self.add(name, a, b)

    def table(self, tolerance):
        rows, worse = [], []
        for name, pairs in self.pairs.items():
            a = statistics.median(p[0] for p in pairs) * 1000
            b = statistics.median(p[1] for p in pairs) * 1000
            n = len(pairs)
            k = sum(1 for x, y in pairs if y > x)
            lost = k / n
            d = (b - a) / a * 100 if a else 0.0
            slow = d > tolerance and sign_p(k, n) < 0.05
            if slow:
                worse.append(name)
            rows.append((name, a, b, d, lost, len(pairs), slow))
        return rows, worse

    def as_json(self):
        return {k: [[round(x * 1000, 4), round(y * 1000, 4)] for x, y in v] for k, v in self.pairs.items()}


# --- placement sur les cœurs --------------------------------------------------
def fast_cpus():
    """Les cœurs les plus rapides (Linux). Sur un processeur hybride, un processus
    posé sur un cœur efficace le reste longtemps : mesuré, deux serveurs au code
    identique s'écartaient ainsi de 10 à 27 %. Les deux serveurs comparés sont
    donc épinglés au même cœur, le client à un autre."""
    if not hasattr(os, "sched_setaffinity"):
        return []
    freq = {}
    for cpu in os.sched_getaffinity(0):
        try:
            with open(f"/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq") as f:
                freq[cpu] = int(f.read())
        except OSError:
            freq[cpu] = 0
    top = max(freq.values())
    # Les « turbo max » n'ont pas tous la même fréquence nominale : ±5 % font un groupe.
    return sorted(c for c, f in freq.items() if f >= top * 0.95) or sorted(freq)


def pin(cpus):
    if cpus and hasattr(os, "sched_setaffinity"):
        os.sched_setaffinity(0, set(cpus))


# --- démarrage ---------------------------------------------------------------
def server_start_once(root):
    """(port qui accepte, premier /api/ping), depuis le lancement de server.py."""
    home = tempfile.mkdtemp(prefix="csvfab-bench-")
    port = free_port()
    t0 = time.perf_counter()
    p = subprocess.Popen([sys.executable, os.path.join(root, "server.py")], env=isolated_env(home, port),
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        while True:
            try:
                socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                break
            except OSError:
                time.sleep(0.0005)
        t_port = time.perf_counter() - t0
        c = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        c.request("GET", "/api/ping")
        c.getresponse().read()
        t_ping = time.perf_counter() - t0
        c.close()
        return t_port, t_ping
    finally:
        p.terminate()
        p.wait()
        shutil.rmtree(home, ignore_errors=True)


def launcher_version_once(root):
    t0 = time.perf_counter()
    subprocess.run([sys.executable, os.path.join(root, "csvfab.py"), "--version"], capture_output=True)
    return time.perf_counter() - t0


def launcher_cold_once(root):
    """Démarrage à froid du lanceur jusqu'à l'appel du navigateur — le chemin
    critique : Chromium démarre là, tout ce qui précède retarde la fenêtre."""
    home = tempfile.mkdtemp(prefix="csvfab-bench-")
    port = free_port()
    bin_dir = os.path.join(home, "bin")
    os.makedirs(bin_dir)
    stub, mark = os.path.join(bin_dir, "fake-browser"), os.path.join(bin_dir, "called")
    with open(stub, "w") as f:
        f.write(f'#!/bin/sh\n: > "{mark}"\n')
    os.chmod(stub, 0o755)
    env = isolated_env(home, port, CSVFAB_BROWSER=stub, PATH=bin_dir)
    t0 = time.perf_counter()
    lp = subprocess.Popen([sys.executable, os.path.join(root, "csvfab.py")], env=env,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    while not os.path.exists(mark):
        time.sleep(0.0002)
    took = time.perf_counter() - t0
    lp.wait()
    try:
        c = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        c.request("GET", "/api/ping")
        os.kill(json.loads(c.getresponse().read())["pid"], 15)
    except Exception:
        pass
    shutil.rmtree(home, ignore_errors=True)
    return took


def startup(res, ref, new, runs):
    for i in range(runs):
        order = [(ref, 0), (new, 1)] if i % 2 == 0 else [(new, 1), (ref, 0)]
        got = {}
        for root, k in order:
            got[k] = server_start_once(root)
        res.add("server_port_open", got[0][0], got[1][0])
        res.add("server_first_ping", got[0][1], got[1][1])
    res.ab("launcher_version", lambda: launcher_version_once(ref), lambda: launcher_version_once(new), runs)
    if os.name != "nt":
        res.ab("launcher_cold_to_browser", lambda: launcher_cold_once(ref), lambda: launcher_cold_once(new), runs)


# --- serveurs en régime ------------------------------------------------------
class Window:
    """Une fenêtre qui sonde /api/pending en continu, comme la page : sans elle,
    un serveur qui a vu une fenêtre (les mesures de /api/pending) s'éteint 30 s
    après son dernier sondage, au milieu des conversions."""

    def __init__(self, b):
        self.b, self.stop = b, threading.Event()
        self.th = threading.Thread(target=self.run, daemon=True)

    def run(self):
        while not self.stop.is_set():
            try:
                self.b.get("/api/pending?wait=1")
            except OSError:
                time.sleep(0.1)

    def __enter__(self):
        self.th.start()
        return self

    def __exit__(self, *exc):
        self.stop.set()
        self.th.join(3)


def latency(res, ba, bb, n, rounds):
    """Requêtes successives sur une connexion gardée ouverte, comme la page,
    par blocs alternés entre les deux serveurs."""
    def setup(b):
        small = b.tmp("small.csv")
        with open(small, "wb") as f:
            f.write(b"a;b\n1;2\n")
        return small, http.client.HTTPConnection("127.0.0.1", b.port, timeout=10), {"X-Csv-Token": b.token}

    sa, sb = setup(ba), setup(bb)
    routes = [("ping", "GET", lambda s: "/api/ping", None),
              ("stat", "GET", lambda s: "/api/stat" + q(path=s), None),
              ("read_small", "GET", lambda s: "/api/file" + q(path=s), None),
              ("write_small", "PUT", lambda s: "/api/file" + q(path=s), b"a;b\n1;2\n"),
              ("pending_nowait", "GET", lambda s: "/api/pending?wait=0", None),
              ("page", "GET", lambda s: "/", None),
              ("app_js", "GET", lambda s: "/app.js", None)]

    def one(state, method, path, body):
        small, conn, h = state
        t0 = time.perf_counter()
        conn.request(method, path(small), body=body, headers=h)
        conn.getresponse().read()
        return time.perf_counter() - t0

    for name, method, path, body in routes:
        for r in range(rounds):
            for _ in range(n):
                if r % 2:
                    y = one(sb, method, path, body)
                    x = one(sa, method, path, body)
                else:
                    x = one(sa, method, path, body)
                    y = one(sb, method, path, body)
                res.add(name, x, y)
    sa[1].close()
    sb[1].close()


def make_csv(path, rows):
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write("Id;Code;Nom;Ville;Montant;Date;Pct;Note\n")
        for i in range(rows):
            f.write(f"{i};{i % 97:05d};Nom {i};Ville {i % 300};{i % 10000} {i % 1000:03d},{i % 100:02d};"
                    f"{1 + i % 28:02d}/{1 + i % 12:02d}/20{10 + i % 15};{i % 100},5%;texte libre {i}\n")


def timer(fn):
    def run():
        t0 = time.perf_counter()
        r = fn()
        took = time.perf_counter() - t0
        if getattr(r, "status", 200) != 200:
            raise RuntimeError(r)
        return took
    return run


def throughput(res, ba, bb, rows, big_mb, runs):
    data = (b"x" * 1023 + b"\n") * (big_mb * 1024)
    src = os.path.join(ba.home, "conv.csv")
    make_csv(src, rows)
    with open(src, "rb") as f:
        body = f.read()

    def prep(b):
        big = b.tmp("big.bin")
        with open(big, "wb") as f:
            f.write(data)
        p = {"big": big, "xlsx": b.tmp("conv.xlsx"), "sqlite": b.tmp("conv.sqlite")}
        b.post("/api/xlsx" + q(path=p["xlsx"], delim=";", header=1), body)
        b.post("/api/sqlite" + q(path=p["sqlite"], sheet="t", delim=";", header=1), body)
        return p

    pa, pb = prep(ba), prep(bb)
    jobs = [
        (f"read_{big_mb}mb", lambda b, p: b.get("/api/file" + q(path=p["big"]))),
        (f"write_{big_mb}mb", lambda b, p: b.put("/api/file" + q(path=p["big"]), data)),
        (f"csv_to_xlsx_{rows}", lambda b, p: b.post("/api/xlsx" + q(path=p["xlsx"], delim=";", header=1), body)),
        (f"xlsx_to_csv_{rows}", lambda b, p: b.post("/api/xlsx2csv" + q(src=p["xlsx"], dest=b.tmp("back.csv"), delim=";"))),
        ("xlsx_sheet_list", lambda b, p: b.get("/api/xlsx-sheets" + q(path=p["xlsx"]))),
        (f"csv_to_sqlite_{rows}", lambda b, p: b.post("/api/sqlite" + q(path=p["sqlite"], sheet="t", delim=";", header=1), body)),
        (f"sqlite_to_csv_{rows}", lambda b, p: b.post("/api/sqlite2csv" + q(src=p["sqlite"], dest=b.tmp("back2.csv"), delim=";"))),
        ("fonts", lambda b, p: b.get("/api/fonts")),
    ]
    for name, job in jobs:
        res.ab(name, timer(lambda: job(ba, pa)), timer(lambda: job(bb, pb)), runs)


# --- sortie ------------------------------------------------------------------
def report(res, tolerance, ref):
    rows, worse = res.table(tolerance)
    w = max(len(r[0]) for r in rows)
    print(f"\nréférence = {ref}, nouvelle = copie de travail ; médianes en ms\n")
    print(f"{'mesure':<{w}}  {'réf.':>10}  {'nouv.':>10}  {'écart':>8}  {'perdus':>7}  {'n':>4}")
    for name, a, b, d, lost, n, slow in rows:
        print(f"{name:<{w}}  {a:>10.3f}  {b:>10.3f}  {d:>+7.1f}%  {lost * 100:>6.0f}%  {n:>4}"
              + ("  ← plus lent" if slow else ""))
    return worse


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--ref", default="HEAD", help="révision de référence (défaut : HEAD)")
    ap.add_argument("--tolerance", type=float, default=3.0,
                    help="écart de médiane en %% au-delà duquel une mesure peut être dite plus lente (défaut : 3)")
    ap.add_argument("--quick", action="store_true", help="moins d'essais, fichiers plus petits")
    ap.add_argument("--save", help="écrire tous les échantillons dans ce fichier JSON")
    a = ap.parse_args()

    q_ = a.quick
    tmp = tempfile.mkdtemp(prefix="csvfab-bench-ref-")
    try:
        ref = extract(a.ref, os.path.join(tmp, "ref"))
        new = snapshot(os.path.join(tmp, "new"))
        res = Results()
        cpus = fast_cpus()
        if len(cpus) < 2:
            print("pas d'épinglage possible : les écarts entre processus resteront bruités", file=sys.stderr)
            cpus = []
        server_cpu, client_cpus = cpus[:1], cpus[1:]
        print("démarrage…", file=sys.stderr)
        pin(cpus)                      # le lanceur et le serveur héritent du masque
        startup(res, ref, new, 6 if q_ else 30)
        # Les serveurs héritent du masque au lancement : un seul cœur, le même pour
        # les deux (ils ne travaillent jamais en même temps), puis le client ailleurs.
        pin(server_cpu)
        ba, bb = Bridge(ref).start(), Bridge(new).start()
        pin(client_cpus)
        with ba, bb, Window(ba), Window(bb):
            print("latence…", file=sys.stderr)
            latency(res, ba, bb, 20 if q_ else 100, 2 if q_ else 6)
            print("débits et conversions…", file=sys.stderr)
            throughput(res, ba, bb, 20_000 if q_ else 200_000, 20 if q_ else 200, 4 if q_ else 10)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    worse = report(res, a.tolerance, a.ref)
    if a.save:
        with open(a.save, "w") as f:
            json.dump({"ref": a.ref, "python": sys.version.split()[0], "platform": sys.platform,
                       "samples_ms": res.as_json()}, f)
    if worse:
        print(f"\nplus lent : {', '.join(worse)}")
        return 1
    print("\nrien de plus lent.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
