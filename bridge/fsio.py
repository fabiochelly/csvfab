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


def fsync_dir(path):
    """Rend durable un renommage dans ce dossier (POSIX) : fsync du fichier protège son
    contenu, mais l'entrée du dossier qui le désigne n'est écrite qu'au fsync du dossier —
    une coupure juste après os.replace() pourrait sinon rendre l'ancienne version."""
    if os.name == "nt":
        return                                    # NTFS journalise le renommage ; pas de fsync de dossier
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    except OSError:
        pass                                      # certains systèmes de fichiers (réseau, FUSE) refusent
    finally:
        os.close(fd)


_locks = {}
_locks_guard = threading.Lock()


def path_lock(path):
    """Le verrou d'un fichier de destination. Le serveur sert chaque requête dans son fil :
    deux écritures du même fichier (deux onglets, un export par-dessus une sauvegarde) se
    suivent au lieu de se croiser. Réentrant : la route qui vérifie puis écrit le tient à
    travers replacing(), qui le prend aussi."""
    key = os.path.normcase(os.path.abspath(path))   # pas realpath : les routes passent un chemin déjà résolu (path_arg), et realpath relit le disque à chaque écriture
    with _locks_guard:
        lock = _locks.get(key)
        if lock is None:
            lock = _locks[key] = threading.RLock()
        return lock


@contextmanager
def replacing(dest, durable=False):
    """Donne un chemin temporaire voisin de dest ; à la sortie sans erreur, il
    remplace dest (en gardant ses permissions : un CSV en 600 le reste), sinon
    il est effacé et dest reste tel quel. durable : le dossier est synchronisé
    après le remplacement (l'appelant a synchronisé le fichier lui-même)."""
    with path_lock(dest):
        tmp = temp_name(dest)
        try:
            yield tmp
            try:
                os.chmod(tmp, os.stat(dest).st_mode & 0o7777)
            except FileNotFoundError:
                pass                              # nouveau fichier : le umask décide
            os.replace(tmp, dest)
        except BaseException:
            remove(tmp)
            raise
        if durable:
            fsync_dir(os.path.dirname(dest) or ".")


@contextmanager
def scratch(directory, suffix):
    """Un fichier de travail dans directory, effacé à la sortie quoi qu'il arrive."""
    path = os.path.join(directory, f".csvfab.{os.getpid()}.{threading.get_ident()}{suffix}")
    try:
        yield path
    finally:
        remove(path)


def backup(path):
    """Copie datée <path>.<stamp>.bak à côté de path ; son chemin, ou None sans fichier à copier.
    Jamais par-dessus une copie existante : deux sauvegardes dans la même seconde (l'horodatage
    est à la seconde, lisible) donnent <stamp>-2, <stamp>-3… — la seconde écrasait la première."""
    if not os.path.isfile(path):
        return None
    base, k = f"{path}.{stamp()}", 1
    while True:
        made = f"{base}.bak" if k == 1 else f"{base}-{k}.bak"
        try:
            fd = os.open(made, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)   # réservé, sans course possible
            break
        except FileExistsError:
            k += 1
    os.close(fd)
    shutil.copy2(path, made)                     # contenu, dates et permissions de l'original
    return made
