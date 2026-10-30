#!/bin/sh
# Installs the native csvfab for the current user (Linux): the desktop entry
# csvfab.desktop (menus, "Open with", the file types csvfab opens), pointing at this
# checkout's release build, so each `cargo build --release` is what the entry opens.
# Also links ~/.local/bin/csvfab to that build. The icons are the ones the web app's install.sh put in ~/.local/share/icons.
#   ./install.sh            install (or update)
#   ./install.sh --remove   remove the entry
set -e
cd "$(dirname "$0")"
DATA=${XDG_DATA_HOME:-$HOME/.local/share}
APP="$DATA/applications/csvfab.desktop"
BIN="$(pwd)/target/release/csvfab"
if [ "$1" = --remove ]; then
    rm -f "$APP"
    [ "$(readlink "$HOME/.local/bin/csvfab")" = "$BIN" ] && rm -f "$HOME/.local/bin/csvfab"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" || true
    echo "Removed $APP"
    exit 0
fi
[ -x "$BIN" ] || cargo build --release -p csvfab
mkdir -p "$DATA/applications"
cat > "$APP" <<EOF
[Desktop Entry]
Type=Application
Name=csvfab
GenericName=CSV Editor
GenericName[fr]=Éditeur CSV
Comment=Open, filter, clean and rewrite large CSV files in place
Comment[fr]=Ouvrir, filtrer, nettoyer et réécrire de gros fichiers CSV
Exec=$BIN %F
TryExec=$BIN
Icon=csvfab
Terminal=false
Categories=Office;Spreadsheet;
Keywords=csv;tsv;xlsx;json;parquet;spreadsheet;table;
MimeType=text/csv;text/tab-separated-values;text/comma-separated-values;application/csv;application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;application/json;application/vnd.sqlite3;application/x-sqlite3;application/vnd.apache.parquet;
StartupWMClass=csvfab
StartupNotify=true
EOF
# The icons, when the web app's installer did not put them there.
SRC="$(cd .. && pwd)/icons"
if [ ! -f "$DATA/icons/hicolor/scalable/apps/csvfab.svg" ] && [ -f "$SRC/csvfab.svg" ]; then
    for s in 16 24 32 48 64 128 256 512; do
        [ -f "$SRC/csvfab-$s.png" ] && mkdir -p "$DATA/icons/hicolor/${s}x${s}/apps" && cp "$SRC/csvfab-$s.png" "$DATA/icons/hicolor/${s}x${s}/apps/csvfab.png"
    done
    mkdir -p "$DATA/icons/hicolor/scalable/apps" && cp "$SRC/csvfab.svg" "$DATA/icons/hicolor/scalable/apps/csvfab.svg"
fi
# The `csvfab` command: this build too (a link, so each build is what it runs).
mkdir -p "$HOME/.local/bin"
ln -sfn "$BIN" "$HOME/.local/bin/csvfab"
command -v desktop-file-validate >/dev/null 2>&1 && desktop-file-validate "$APP" || true
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA/applications" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
echo "Installed $APP -> $BIN"
