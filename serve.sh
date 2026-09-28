#!/usr/bin/env bash
# Ouvre l'editeur dans le navigateur courant, en onglet normal.
#
# Pour l'application de bureau (fenetre sans barre d'adresse, integration
# "Ouvrir avec"), utiliser ./csvfab a la place. Ce script reste le mode
# navigateur : meme serveur, mais une simple page.
#
# Le serveur est indispensable : l'API File System Access -- seul moyen pour une
# page de reecrire le fichier ouvert -- refuse de tourner sur une page file://
# (origine opaque). Tout http://localhost est digne de confiance, cela suffit.
# Le port est fixe car Chromium memorise les permissions accordees par origine.
set -euo pipefail
cd "$(dirname "$0")"
PORT="${CSVFAB_PORT:-${CSV_EDITOR_PORT:-${PORT:-8787}}}"
URL="http://127.0.0.1:${PORT}/"

if curl -sf --max-time 1 "${URL}api/ping" >/dev/null 2>&1; then
    echo "Serveur deja actif — ouverture de ${URL}"
    for b in chromium brave xdg-open open; do
        command -v "$b" >/dev/null && { "$b" "${URL}" >/dev/null 2>&1 & break; }
    done
    exit 0
fi

CSVFAB_PORT="$PORT" python3 server.py &
SRV=$!
trap 'kill "${SRV}" 2>/dev/null || true' EXIT INT TERM
for _ in $(seq 40); do curl -sf --max-time 1 "${URL}api/ping" >/dev/null 2>&1 && break; sleep 0.05; done
echo "Sert $(pwd) sur ${URL}  (Ctrl+C pour arreter)"

for b in chromium brave xdg-open open; do
    command -v "$b" >/dev/null && { "$b" "${URL}" >/dev/null 2>&1 & break; }
done
wait "${SRV}"
