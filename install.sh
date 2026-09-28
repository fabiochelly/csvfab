#!/bin/sh
# csvfab installer for Linux and macOS — per user, no root needed.
#
#   curl -fsSL https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.sh | sh
#   ./install.sh                 from a checkout or an unpacked release
#   sh install.sh --uninstall    removes the app (your settings are kept)
#
# Environment: CSVFAB_VERSION=1.2.3 installs that release instead of the latest.
set -eu

REPO="${CSVFAB_REPO:-fabiochelly/csvfab}"
VERSION="${CSVFAB_VERSION:-latest}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
DEST="$DATA/csvfab"
BIN="$HOME/.local/bin"
OS=$(uname -s)

say()  { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

uninstall() {
    say "Removing csvfab"
    rm -rf "$DEST" "$BIN/csvfab" "$DATA/applications/csvfab.desktop" "$HOME/Applications/csvfab.app"
    for s in 16 24 32 48 64 128 256 512; do rm -f "$DATA/icons/hicolor/${s}x${s}/apps/csvfab.png"; done
    rm -f "$DATA/icons/hicolor/scalable/apps/csvfab.svg"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" || true
    say "Done. Settings and the browser profile stay in your state folder."
    exit 0
}
[ "${1:-}" = "--uninstall" ] && uninstall

# --- requirements -----------------------------------------------------------
command -v python3 >/dev/null 2>&1 || die "Python 3 is required (3.8 or newer)."
python3 -c 'import sys; sys.exit(sys.version_info < (3, 8))' || die "Python 3.8 or newer is required."

browser=""
if [ "$OS" = Darwin ]; then
    for a in "Google Chrome" Chromium "Brave Browser" "Microsoft Edge"; do
        [ -d "/Applications/$a.app" ] || [ -d "$HOME/Applications/$a.app" ] && { browser="$a"; break; }
    done
else
    for b in chromium brave brave-browser google-chrome-stable google-chrome chrome microsoft-edge; do
        command -v "$b" >/dev/null 2>&1 && { browser="$b"; break; }
    done
fi
[ -n "$browser" ] || warn "No Chromium-based browser found (Chrome, Chromium, Brave or Edge): csvfab needs one to open its window."

# --- sources: this folder, or the release archive ---------------------------
SRC=""
case "$0" in
    */*) d=$(cd "$(dirname "$0")" && pwd); [ -f "$d/csvfab.py" ] && [ -f "$d/viewer.htm" ] && SRC="$d" ;;
esac
if [ -z "$SRC" ]; then
    command -v curl >/dev/null 2>&1 || die "curl is required to download csvfab."
    TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
    if [ "$VERSION" = latest ]; then URL="https://github.com/$REPO/releases/latest/download/csvfab.tar.gz"
    else URL="https://github.com/$REPO/releases/download/v$VERSION/csvfab-$VERSION.tar.gz"; fi
    say "Downloading $URL"
    curl -fsSL "$URL" | tar -xz -C "$TMP" || die "download failed"
    SRC="$TMP/csvfab"
fi

# --- install ----------------------------------------------------------------
say "Installing into $DEST"
rm -rf "$DEST"
mkdir -p "$DEST/icons" "$BIN"
for f in csvfab.py server.py viewer.htm papaparse.min.js LICENSE; do cp "$SRC/$f" "$DEST/"; done
cp "$SRC"/icons/csvfab.svg "$SRC"/icons/csvfab-*.png "$DEST/icons/"
chmod 755 "$DEST/csvfab.py"
ln -sf "$DEST/csvfab.py" "$BIN/csvfab"          # the command is "csvfab"; the file keeps its .py

if [ "$OS" = Darwin ]; then
    # A minimal app bundle, so csvfab shows in Launchpad and Spotlight.
    APP="$HOME/Applications/csvfab.app"
    rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    printf '#!/bin/sh\nexec /usr/bin/env python3 "%s/csvfab.py" "$@"\n' "$DEST" > "$APP/Contents/MacOS/csvfab"
    chmod 755 "$APP/Contents/MacOS/csvfab"
    if command -v iconutil >/dev/null 2>&1; then
        set_=$(mktemp -d)/csvfab.iconset; mkdir -p "$set_"
        for s in 16 32 128 256 512; do
            cp "$SRC/icons/csvfab-$s.png" "$set_/icon_${s}x${s}.png"
            d2=$((s * 2)); [ -f "$SRC/icons/csvfab-$d2.png" ] && cp "$SRC/icons/csvfab-$d2.png" "$set_/icon_${s}x${s}@2x.png"
        done
        iconutil -c icns "$set_" -o "$APP/Contents/Resources/csvfab.icns" 2>/dev/null || true
    fi
    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>csvfab</string>
  <key>CFBundleIdentifier</key><string>io.github.csvfab</string>
  <key>CFBundleExecutable</key><string>csvfab</string>
  <key>CFBundleIconFile</key><string>csvfab</string>
  <key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST
else
    mkdir -p "$DATA/applications"
    # Absolute Exec: a session started outside a login shell may not have ~/.local/bin on its PATH.
    sed "s|^Exec=csvfab|Exec=$DEST/csvfab.py|" "$SRC/csvfab.desktop" > "$DATA/applications/csvfab.desktop"
    for s in 16 24 32 48 64 128 256 512; do
        mkdir -p "$DATA/icons/hicolor/${s}x${s}/apps"
        cp "$SRC/icons/csvfab-$s.png" "$DATA/icons/hicolor/${s}x${s}/apps/csvfab.png"
    done
    mkdir -p "$DATA/icons/hicolor/scalable/apps"
    cp "$SRC/icons/csvfab.svg" "$DATA/icons/hicolor/scalable/apps/csvfab.svg"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
fi

case ":$PATH:" in *":$BIN:"*) ;; *) warn "$BIN is not on your PATH: add it to run 'csvfab' from a terminal." ;; esac
say "csvfab $(python3 "$DEST/csvfab.py" --version | cut -d' ' -f2) installed. Run: csvfab [file.csv]"
