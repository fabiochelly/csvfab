"""Écritures sûres : un fichier temporaire voisin, fsync, puis os.replace().

Le fichier d'origine n'est jamais tronqué, quoi qu'il arrive en cours
d'écriture — le pendant, côté serveur, du swap file de createWritable().
"""

import os
import shutil
import threading
import time
from contextlib import contextmanager

CHUNK = 1 << 20


def stamp():
    return time.strftime("%Y%m%d-%H%M%S")


def remove(path):
    try:
        os.unlink(path)
    except OSError:
        pass


def temp_name(dest, suffix=".part"):
    """Un nom caché voisin de dest, propre à ce processus et à ce fil : deux
    requêtes simultanées vers le même fichier ne se marchent pas dessus."""
    d, base = os.path.split(dest)
    return os.path.join(d, f".{base}.{os.getpid()}.{threading.get_ident()}{suffix}")


def receive(rfile, n, out):
    """Recopie exactement n octets du corps d'une requête dans out (fichier binaire).
    Un flux coupé avant la fin lève : la cible ne doit pas être remplacée."""
    buf = memoryview(bytearray(min(CHUNK, n) or 1))
    remaining = n
    while remaining > 0:
        got = rfile.readinto(buf[:min(len(buf), remaining)])
        if not got:
            raise IOError("flux interrompu avant la fin du corps")
        out.write(buf[:got])
        remaining -= got


def fsync(f):
    f.flush()
    os.fsync(f.fileno())


@contextmanager
def replacing(dest):
    """Donne un chemin temporaire voisin de dest ; à la sortie sans erreur, il
    remplace dest (en gardant ses permissions : un CSV en 600 le reste), sinon
    il est effacé et dest reste tel quel."""
    tmp = temp_name(dest)
    try:
        yield tmp
        try:
            os.chmod(tmp, os.stat(dest).st_mode & 0o7777)
        except FileNotFoundError:
            pass                                  # nouveau fichier : le umask décide
        os.replace(tmp, dest)
    except BaseException:
        remove(tmp)
        raise


@contextmanager
def scratch(directory, suffix):
    """Un fichier de travail dans directory, effacé à la sortie quoi qu'il arrive."""
    path = os.path.join(directory, f".csvfab.{os.getpid()}.{threading.get_ident()}{suffix}")
    try:
        yield path
    finally:
        remove(path)


def backup(path):
    """Copie datée <path>.<stamp>.bak à côté de path ; son chemin, ou None sans fichier à copier."""
    if not os.path.isfile(path):
        return None
    made = f"{path}.{stamp()}.bak"
    shutil.copy2(path, made)
    return made
