"""Le pont local de csvfab : le serveur que lance server.py, et ce qu'il partage avec le lanceur.

    config      version, port, dossier d'état (partagés avec csvfab.py)
    session     file des chemins à ouvrir, fenêtres vivantes, arrêt à vide
    fsio        écritures atomiques, copies .bak, réception des corps de requête
    httpd       le serveur HTTP : plomberie, table des routes, jeton
    app         démarrage du serveur (main)
    routes/     une route = une fonction, par domaine (page, fichiers, fenêtres, conversions, polices)
    formats/    lecture et écriture xlsx / SQLite, sans rien savoir de HTTP
    fonts       polices à chasse fixe installées, catalogue et installation

Rien n'est importé ici : chaque module se charge à la demande, les conversions
(zipfile, ElementTree, csv, sqlite3) seulement à la première qui en a besoin.
"""
