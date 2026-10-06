"""Les fenêtres de l'app vues du serveur : la file des chemins à ouvrir, le
sondage long qui la vide, et ce qu'il dit des fenêtres encore vivantes.

Le lanceur dépose les chemins reçus en ligne de commande (/api/open) ; chaque
fenêtre tient une requête /api/pending ouverte, à laquelle le serveur répond
dès qu'un chemin arrive. Une requête tenue vaut donc fenêtre vivante, et
quand plus aucune n'interroge, le serveur s'arrête (should_stop) : fermer
l'app ne laisse rien tourner.

L'horloge est injectable : les tests font passer le temps sans l'attendre.
"""

import threading
import time

POLL_ALIVE = 6.0        # au-delà, plus aucune fenêtre n'est considérée vivante
POLL_MAX = 10.0         # durée maximale d'un /api/pending tenu (?wait=)
TICK = 0.25             # une fenêtre fermée en plein sondage est remarquée en autant
# Marge avant extinction : le temps qu'une fenêtre naisse et se signale, puis,
# une fois qu'au moins une l'a fait, le temps de tolérer un rechargement.
IDLE_BOOT = 120.0
IDLE_LIVE = 30.0


class Session:
    def __init__(self, clock=time.time):
        self.clock = clock
        self._cond = threading.Condition()
        self._queue = []        # chemins en attente d'ouverture
        self._polling = 0       # /api/pending tenus en ce moment : autant de fenêtres vivantes
        self._last_seen = 0.0   # fin du dernier /api/pending
        self._seen_once = False
        self.started = clock()

    def alive(self):
        """Une fenêtre interroge, ou l'a fait il y a moins de POLL_ALIVE."""
        with self._cond:
            return self._polling > 0 or (self._seen_once and self.clock() - self._last_seen < POLL_ALIVE)

    def queued(self):
        """Les chemins en attente, sans les retirer de la file (la page les inscrit dans son en-tête)."""
        with self._cond:
            return list(self._queue)

    def polling(self):
        with self._cond:
            return self._polling

    def push(self, paths):
        """Met des chemins en file (sans doublon) et réveille les sondages en attente."""
        with self._cond:
            for p in paths:
                if p not in self._queue:
                    self._queue.append(p)
            self._cond.notify_all()

    def wait(self, timeout, gone):
        """Le sondage long : rend la file (vidée) dès qu'elle n'est plus vide, sinon
        une liste vide au bout de timeout secondes ; None si gone() devient vrai
        entre-temps — la fenêtre s'est fermée, et les chemins restent en file pour
        la suivante au lieu d'être perdus dans une réponse que personne ne lira."""
        deadline = self.clock() + timeout
        closed = False
        with self._cond:
            self._last_seen = self.clock()
            self._seen_once = True
            self._polling += 1
        try:
            while True:
                with self._cond:
                    left = deadline - self.clock()
                    if self._queue or left <= 0:
                        paths, self._queue = self._queue, []
                        break
                    self._cond.wait(min(left, TICK))
                if gone():
                    closed = True
                    break
        finally:
            with self._cond:
                self._polling -= 1
                if not closed:
                    self._last_seen = self.clock()
        return None if closed else paths

    def should_stop(self):
        """Plus aucune fenêtre depuis IDLE_LIVE (ou, sans qu'aucune se soit jamais
        signalée, depuis IDLE_BOOT après le démarrage)."""
        with self._cond:
            seen, last, held = self._seen_once, self._last_seen, self._polling
        if held:
            return False
        idle = self.clock() - (last if seen else self.started)
        return idle > (IDLE_LIVE if seen else IDLE_BOOT)

    def hand_over(self, paths):
        """Oublie les fenêtres vues et remet des chemins en file, pour la fenêtre
        qu'un relancement va ouvrir : sinon le lanceur, croyant la dernière encore
        là pendant POLL_ALIVE, n'en ouvrirait aucune."""
        with self._cond:                         # verrou réentrant (RLock) : push() le reprend
            self._seen_once, self._last_seen = False, 0.0
            self.push(paths)
