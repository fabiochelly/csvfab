#!/bin/sh
# Rebuilds csvfab (release, optimised) then launches it: ./run-dev.sh [files…]
set -e
cd "$(dirname "$0")"
cargo build --release -p csvfab
exec ./target/release/csvfab "$@"
