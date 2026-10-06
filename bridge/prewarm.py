"""Le navigateur relu d'avance quand il n'est pas en cache (lanceur seulement).

Premier lancement après le démarrage de la machine : les ~400 Mo du navigateur
ne sont pas en cache, et ses défauts de page, épars, lisent le disque morceau
par morceau — +130 ms mesurés jusqu'à la première image. Un enfant relit alors
ses fichiers d'un trait pendant qu'il démarre : de csvfab à la page qui
interroge le serveur, 471 → 350 ms. Seulement quand le cache est froid : relire
400 Mo déjà en cache ralentissait le lancement ordinaire (~15 ms) au lieu de
l'accélérer.

Un module plutôt que du code dans csvfab.py : un script est recompilé à chaque
lancement (pas de .pyc), et ces lignes y coûtaient ~0,5 ms à chacun, même à
« csvfab --version ». Importé seulement quand une fenêtre s'ouvre.
"""

import os


def browser_dir(exe):
    """Le dossier d'installation du navigateur (celui de resources.pak) :
    /usr/bin/chromium n'est souvent qu'un lanceur ou un script qui exécute
    /usr/lib/chromium/chromium. None si on ne le trouve pas."""
    if not exe:
        return None
    exe = os.path.realpath(exe)
    name = os.path.basename(exe)
    for d in (os.path.dirname(exe), f"/usr/lib/{name}", f"/usr/lib64/{name}",
              f"/usr/lib/{name}-browser", f"/usr/lib64/{name}-browser"):
        if os.path.isfile(os.path.join(d, "resources.pak")):
            return d
    return None


def warm(exe):
    """exe : le chemin du navigateur. La sonde lit quelques pages du binaire
    avec RWF_NOWAIT, qui échoue sans toucher au disque si la page n'est pas en
    cache (~0,05 ms en tout). Linux seulement : ni RWF_NOWAIT ni fork ailleurs."""
    if not hasattr(os, "RWF_NOWAIT") or not hasattr(os, "fork"):
        return
    d = browser_dir(exe)
    if not d:
        return
    try:
        files = sorted((e for e in os.scandir(d) if e.is_file()), key=lambda e: e.stat().st_size, reverse=True)
        main = files[0].path                     # le binaire : le plus gros fichier
        fd = os.open(main, os.O_RDONLY)
        try:
            size, page, cold = os.fstat(fd).st_size, bytearray(4096), False
            for i in range(16):
                try:
                    os.preadv(fd, [page], (size * i // 16) & ~4095, os.RWF_NOWAIT)
                except BlockingIOError:
                    cold = True
                    break
        finally:
            os.close(fd)
        if not cold or os.fork():
            return
    except OSError:
        return
    # L'enfant : ses sorties sur /dev/null, pour qu'un appelant qui attend la
    # fin de la sortie du lanceur (yazi, un test) n'attende pas aussi la lecture.
    try:
        null = os.open(os.devnull, os.O_RDWR)
        for n in (0, 1, 2):
            os.dup2(null, n)
        # Dans l'ordre où il s'en sert : le binaire, ICU, l'instantané V8, les
        # ressources, la langue ; puis les bibliothèques du processus GPU.
        lang = (os.environ.get("LANG") or "en_US").split(".")[0].replace("_", "-")
        names = ["icudtl.dat", "v8_context_snapshot.bin", "snapshot_blob.bin", "resources.pak",
                 "chrome_100_percent.pak", "chrome_200_percent.pak",
                 f"locales/{lang}.pak", f"locales/{lang.split('-')[0]}.pak", "locales/en-US.pak"]
        paths = [main] + [os.path.join(d, n) for n in names] + sorted(e.path for e in files if e.name.endswith(".so"))
        buf = bytearray(1 << 21)
        for p in paths:
            try:
                with open(p, "rb", buffering=0) as f:
                    while f.readinto(buf):
                        pass
            except OSError:
                pass
    finally:
        os._exit(0)
