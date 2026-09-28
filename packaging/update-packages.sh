#!/bin/sh
# After the release workflow of vX.Y.Z has published: fills the checksums into
#   - packaging/aur/PKGBUILD + .SRCINFO   (push that folder to the AUR git repo)
#   - packaging/homebrew/csvfab.rb        (and into the tap checkout, if CSVFAB_TAP points at it)
#   - packaging/winget/manifests/…/X.Y.Z  (submit with: wingetcreate submit <folder>)
set -eu
V="${1:?usage: packaging/update-packages.sh X.Y.Z}"
cd "$(dirname "$0")/.."
OWNER=$(git remote get-url origin | sed -E 's#.*github.com[:/]([^/]+)/.*#\1#')
TAR="https://github.com/$OWNER/csvfab/archive/refs/tags/v$V.tar.gz"
EXE="https://github.com/$OWNER/csvfab/releases/download/v$V/csvfab-setup-$V.exe"
sum() { curl -fsSL "$1" | sha256sum | cut -d' ' -f1; }
TAR_SUM=$(sum "$TAR"); EXE_SUM=$(sum "$EXE")
echo "source $TAR_SUM"; echo "setup  $EXE_SUM"

sed -i -e "s/^pkgver=.*/pkgver=$V/" -e "s/^pkgrel=.*/pkgrel=1/" -e "s/^sha256sums=.*/sha256sums=('$TAR_SUM')/" packaging/aur/PKGBUILD
(cd packaging/aur && makepkg --printsrcinfo > .SRCINFO)

sed -i -e "s#archive/refs/tags/v[^\"]*\.tar\.gz#archive/refs/tags/v$V.tar.gz#" -e "s/sha256 \".*\"/sha256 \"$TAR_SUM\"/" packaging/homebrew/csvfab.rb
if [ -n "${CSVFAB_TAP:-}" ]; then mkdir -p "$CSVFAB_TAP/Formula" && cp packaging/homebrew/csvfab.rb "$CSVFAB_TAP/Formula/csvfab.rb"; fi

W="packaging/winget/manifests/f/FabioChelly/csvfab/$V"; mkdir -p "$W"
for f in packaging/winget/template/*.yaml; do
    sed -e "s/__VERSION__/$V/g" -e "s/__SHA256_SETUP__/$(echo "$EXE_SUM" | tr a-f A-F)/g" "$f" > "$W/$(basename "$f")"
done
echo "Updated: packaging/aur, packaging/homebrew/csvfab.rb, $W"
