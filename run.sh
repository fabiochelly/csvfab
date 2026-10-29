#!/bin/sh
# Launches the last build as it is, without compiling: ./run.sh [files…]
BIN="$(dirname "$0")/target/release/csvfab"
[ -x "$BIN" ] || { echo "No build yet: run ./run-dev.sh first." >&2; exit 1; }
exec "$BIN" "$@"
