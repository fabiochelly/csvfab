#!/bin/sh
# Cuts a release: sets VERSION in the sources, commits, tags vX.Y.Z and pushes.
# GitHub Actions then builds the archives and the Windows installer.
# Afterwards: packaging/update-packages.sh X.Y.Z
set -eu
V="${1:?usage: packaging/release.sh X.Y.Z}"
cd "$(dirname "$0")/.."
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first" >&2; exit 1; }
sed -i.orig "s/^VERSION = \".*\"/VERSION = \"$V\"/" csvfab.py server.py && rm -f csvfab.py.orig server.py.orig
git commit -qam "csvfab $V"
git tag -a "v$V" -m "csvfab $V"
git push && git push origin "v$V"
echo "Tag v$V pushed. Once the release workflow is green: packaging/update-packages.sh $V"
