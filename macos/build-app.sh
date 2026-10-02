#!/bin/sh
# Builds csvfab.app, the macOS application bundle, from a checkout or an unpacked
# release:  sh macos/build-app.sh <source folder> <destination folder>
# → <destination>/csvfab.app, self-contained (the app's files in Contents/Resources/csvfab).
#
# Why a bundle at all: "Open with", Launchpad, Spotlight and the default handler
# for CSV files only exist for application bundles, and the Finder hands files to
# an app as Apple Events, not as arguments — a shell script in Contents/MacOS
# never sees them. The bundle is therefore an AppleScript applet (osacompile,
# stock on every Mac) whose "on open" handler runs csvfab.py with the files.
# Built on the user's Mac (install.sh, the Homebrew cask): nothing is downloaded
# as an app, so Gatekeeper's quarantine never applies; signed ad hoc all the same.
set -eu
SRC="${1:?usage: build-app.sh <source folder> <destination folder>}"
OUT="${2:?usage: build-app.sh <source folder> <destination folder>}"
[ -f "$SRC/csvfab.py" ] || { echo "build-app.sh: $SRC is not a csvfab folder" >&2; exit 1; }
[ "$(uname -s)" = Darwin ] || { echo "build-app.sh: macOS only (osacompile, iconutil, PlistBuddy)" >&2; exit 1; }
V=$(sed -n 's/^VERSION = "\(.*\)"/\1/p' "$SRC/bridge/config.py")
HERE=$(cd "$(dirname "$0")" && pwd)
APP="$OUT/csvfab.app"
mkdir -p "$OUT"; rm -rf "$APP"

osacompile -o "$APP" "$HERE/csvfab.applescript"

# The app inside the bundle.
RES="$APP/Contents/Resources"; mkdir -p "$RES/csvfab/icons"
for f in csvfab.py server.py viewer.htm papaparse.min.js LICENSE; do cp "$SRC/$f" "$RES/csvfab/"; done
cp -R "$SRC/ui" "$RES/csvfab/ui"
cp -R "$SRC/bridge" "$RES/csvfab/bridge"; rm -rf "$RES/csvfab/bridge"/__pycache__ "$RES/csvfab/bridge"/*/__pycache__
cp "$SRC"/icons/csvfab.svg "$SRC"/icons/csvfab-*.png "$RES/csvfab/icons/"
chmod 755 "$RES/csvfab/csvfab.py"

# The icon: an iconset from the PNGs (16…512, each also the @2x of the size below).
set_=$(mktemp -d)/csvfab.iconset; mkdir -p "$set_"
for s in 16 32 128 256 512; do
    cp "$SRC/icons/csvfab-$s.png" "$set_/icon_${s}x${s}.png"
    d2=$((s * 2)); [ -f "$SRC/icons/csvfab-$d2.png" ] && cp "$SRC/icons/csvfab-$d2.png" "$set_/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$set_" -o "$RES/csvfab.icns"
# osacompile (macOS 11+) also puts the Script Editor icon in an asset catalog, Assets.car,
# named by CFBundleIconName: that key wins over CFBundleIconFile, so with either left in
# place the Finder, the Dock and Launchpad keep showing the generic applet icon.
rm -f "$RES/applet.icns" "$RES/droplet.icns" "$RES/Assets.car"; rm -rf "$(dirname "$set_")"

# Identity, version and document types: the applet's own keys replaced, the rest merged in.
PL="$APP/Contents/Info.plist"; PB=/usr/libexec/PlistBuddy
for k in CFBundleIdentifier CFBundleName CFBundleDisplayName CFBundleIconFile CFBundleIconName CFBundleDocumentTypes; do "$PB" -c "Delete :$k" "$PL" 2>/dev/null || true; done
"$PB" -c "Merge $HERE/Info-extra.plist" "$PL"
for k in CFBundleShortVersionString CFBundleVersion; do "$PB" -c "Set :$k $V" "$PL" 2>/dev/null || "$PB" -c "Add :$k string $V" "$PL"; done

# The Python modules compiled now, inside the bundle, before it is signed: the
# launcher and the server never write __pycache__ there (it would break the
# signature), so without this every launch would compile them anew.
python3 -m compileall -q "$RES/csvfab/bridge" >/dev/null 2>&1 || true

# Ad hoc signature (no developer certificate needed), and no quarantine flag on
# what was just built here. Then Launch Services learns the bundle at once.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
xattr -cr "$APP" 2>/dev/null || true
# The Finder caches an app's icon by path: a rebuild over an older bundle would keep showing
# the old one. A fresh date on the bundle makes it read the icon again.
touch "$APP"
LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$LSREG" ] && "$LSREG" -f "$APP" >/dev/null 2>&1 || true
echo "$APP (csvfab $V)"
