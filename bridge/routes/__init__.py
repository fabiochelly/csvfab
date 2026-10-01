"""Les routes du serveur, par domaine ; la table qui les relie aux URL est dans bridge.httpd.

Chaque route est une fonction f(req), req étant le gestionnaire de la requête
(bridge.httpd.Handler : req.arg(), req.path_arg(), req.send(), req.json(),
req.server.session…). Un module n'est importé qu'à la première requête qui
en a besoin.
"""
