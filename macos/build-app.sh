#!/bin/sh
# Builds csvfab.app, the macOS application bundle, from an unpacked release or a
# checkout:  sh macos/build-app.sh <source folder> <destination folder>
# → <destination>/csvfab.app, the program in Contents/Resources/csvfab.
# The program is <source>/csvfab (a release), $CSVFAB_EXE, or <source>/target/release/csvfab.
#
# Why a bundle at all: "Open with", Launchpad, Spotlight and the default handler
# for CSV files only exist for application bundles, and the Finder hands files to
# an app as Apple Events, not as arguments — which the program's window toolkit
# does not read. The bundle is therefore an AppleScript applet (osacompile, stock
# on every Mac) whose "on open" handler runs the program with the files; a csvfab
# window already open takes them as new tabs.
# Built on the user's Mac (install.sh, the Homebrew cask), then cleared of the
# download's quarantine flag and signed ad hoc.
set -eu
SRC="${1:?usage: build-app.sh <source folder> <destination folder>}"
OUT="${2:?usage: build-app.sh <source folder> <destination folder>}"
[ "$(uname -s)" = Darwin ] || { echo "build-app.sh: macOS only (osacompile, iconutil, PlistBuddy)" >&2; exit 1; }
EXE="${CSVFAB_EXE:-}"
[ -n "$EXE" ] || { [ -f "$SRC/csvfab" ] && EXE="$SRC/csvfab"; } || EXE="$SRC/target/release/csvfab"
[ -f "$EXE" ] || { echo "build-app.sh: no csvfab program in $SRC" >&2; exit 1; }
# The version: the release's VERSION file, or the checkout's Cargo.toml (the program
# itself is not run before its quarantine flag is gone).
if [ -f "$SRC/VERSION" ]; then V=$(cat "$SRC/VERSION"); else V=$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1); fi
HERE=$(cd "$(dirname "$0")" && pwd)
APP="$OUT/csvfab.app"
mkdir -p "$OUT"; rm -rf "$APP"

osacompile -o "$APP" "$HERE/csvfab.applescript"

# The program inside the bundle.
RES="$APP/Contents/Resources"
cp "$EXE" "$RES/csvfab"; chmod 755 "$RES/csvfab"

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

# No quarantine flag on what was just assembled here (the program may come from a
# download), then an ad hoc signature (no developer certificate needed). Then Launch
# Services learns the bundle at once.
xattr -cr "$APP" 2>/dev/null || true
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
# The Finder caches an app's icon by path: a rebuild over an older bundle would keep showing
# the old one. A fresh date on the bundle makes it read the icon again.
touch "$APP"
LSREG=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ -x "$LSREG" ] && "$LSREG" -f "$APP" >/dev/null 2>&1 || true
echo "$APP (csvfab $V)"
