#!/bin/sh
# csvfab installer for Linux and macOS — per user, no root needed.
#
#   curl -fsSL https://raw.githubusercontent.com/fabiochelly/csvfab/main/install.sh | sh
#   ./install.sh                 from an unpacked release, or a checkout (built with cargo)
#   sh install.sh --uninstall    removes the app (your settings are kept)
#
# Environment: CSVFAB_VERSION=2.0.0 installs that release instead of the latest.
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
    say "Done. Your settings stay in your state folder."
    exit 0
}
[ "${1:-}" = "--uninstall" ] && uninstall

# --- the program: this folder's, or the release's -----------------------------
# SRC holds icons/, csvfab.desktop (Linux) and macos/ (macOS); EXE is the program.
SRC=""; EXE=""
# $0 is the script's path from a folder ("./install.sh", "sh install.sh"), "sh" under curl | sh.
if [ -f "$0" ]; then
    d=$(cd "$(dirname "$0")" && pwd)
    if [ -x "$d/csvfab" ] && [ -d "$d/icons" ]; then
        SRC="$d"; EXE="$d/csvfab"                        # an unpacked release
    elif [ -f "$d/Cargo.toml" ] && [ -d "$d/app" ]; then
        command -v cargo >/dev/null 2>&1 || die "Building from the sources needs Rust (https://rustup.rs). Or run: curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh"
        say "Building csvfab (cargo build --release): a few minutes the first time"
        (cd "$d" && cargo build --release --locked -p csvfab) || die "the build failed"
        SRC="$d"; EXE="$d/target/release/csvfab"         # a checkout
    fi
fi
if [ -z "$SRC" ]; then
    case "$OS-$(uname -m)" in
        Linux-x86_64|Linux-amd64) ASSET=linux-x86_64 ;;
        Darwin-*) ASSET=macos ;;                          # one binary for Apple silicon and Intel
        *) die "No ready-made csvfab for $OS $(uname -m): build it from the sources (git clone https://github.com/$REPO && ./csvfab/install.sh, with Rust installed)." ;;
    esac
    command -v curl >/dev/null 2>&1 || die "curl is required to download csvfab."
    TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
    if [ "$VERSION" = latest ]; then URL="https://github.com/$REPO/releases/latest/download/csvfab-$ASSET.tar.gz"
    else URL="https://github.com/$REPO/releases/download/v$VERSION/csvfab-$VERSION-$ASSET.tar.gz"; fi
    say "Downloading $URL"
    curl -fsSL "$URL" | tar -xz -C "$TMP" || die "download failed"
    SRC="$TMP/csvfab"; EXE="$SRC/csvfab"
fi

# --- install ----------------------------------------------------------------
mkdir -p "$BIN"
if [ "$OS" = Darwin ]; then
    # csvfab.app in ~/Applications: a real bundle, listed in "Open with" for CSV files;
    # macos/build-app.sh puts the program inside it and registers it with Launch Services.
    rm -rf "$DEST"                                        # what csvfab 1.x installed there
    mkdir -p "$HOME/Applications"
    CSVFAB_EXE="$EXE" sh "$SRC/macos/build-app.sh" "$SRC" "$HOME/Applications" >/dev/null || die "the app bundle could not be built"
    say "csvfab.app in ~/Applications"
    INSTALLED="$HOME/Applications/csvfab.app/Contents/Resources/csvfab"
    ln -sfn "$INSTALLED" "$BIN/csvfab"
    # The default app for CSV files is the user's choice: asked here (on the terminal,
    # even under curl | sh), done with duti when it is there, shown otherwise.
    if [ -r /dev/tty ] && { : < /dev/tty; } 2>/dev/null; then
        printf 'Make csvfab the default app for CSV files? [y/N] '; read -r ans < /dev/tty || ans=
        case "$ans" in
            [yY]*)
                if command -v duti >/dev/null 2>&1; then
                    for u in public.comma-separated-values-text public.tab-separated-values-text; do duti -s io.github.fabiochelly.csvfab "$u" all; done
                    say "Done: CSV and TSV files open with csvfab."
                else
                    warn "duti is not installed (brew install duti). By hand: select a .csv, File > Get Info > Open with > csvfab > Change All."
                fi ;;
        esac
    fi
else
    say "Installing into $DEST"
    rm -rf "$DEST"                                        # also what csvfab 1.x installed there
    mkdir -p "$DEST"
    cp "$EXE" "$DEST/csvfab"; chmod 755 "$DEST/csvfab"
    INSTALLED="$DEST/csvfab"
    ln -sfn "$INSTALLED" "$BIN/csvfab"
    mkdir -p "$DATA/applications"
    # Absolute Exec: a session started outside a login shell may not have ~/.local/bin on its PATH.
    sed -e "s|^Exec=csvfab|Exec=$INSTALLED|" -e "/^Exec=/a TryExec=$INSTALLED" "$SRC/csvfab.desktop" > "$DATA/applications/csvfab.desktop"
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
say "$("$INSTALLED" --version) installed. Run: csvfab [file.csv]"
