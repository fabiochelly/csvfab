#!/bin/sh
# Cuts a release: sets the version in Cargo.toml (and Cargo.lock), commits, tags vX.Y.Z and pushes.
# GitHub Actions then builds the programs (Linux, macOS, Windows), the Windows installer and the release.
# Afterwards: packaging/update-packages.sh X.Y.Z
set -eu
V="${1:?usage: packaging/release.sh X.Y.Z}"
cd "$(dirname "$0")/.."
[ -z "$(git status --porcelain)" ] || { echo "commit or stash your changes first" >&2; exit 1; }
sed -i.orig "s/^version = \".*\"/version = \"$V\"/" Cargo.toml && rm -f Cargo.toml.orig
cargo update --workspace --offline -q   # the lock file's own record of the version
git commit -qam "csvfab $V"
git tag -a "v$V" -m "csvfab $V"
git push && git push origin "v$V"
echo "Tag v$V pushed. Once the release workflow is green: packaging/update-packages.sh $V"
