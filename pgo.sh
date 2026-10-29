#!/bin/sh
# A release build optimised with the profile of real work (PGO): ./pgo.sh [files…]
#
# 1. builds an instrumented engine benchmark, 2. runs it on the files (default: the bench
# files open / filter / sort / save), 3. builds the app with that profile.
# Measured on 5M rows (A/B, medians of 7): filters −10…18 %, sorts −10…13 %, open and
# save unchanged. Twice the build time (~10 min), so not the default `cargo build`.
# Needs llvm-profdata of rustc's LLVM major version (`rustc -vV`; the `llvm-tools`
# rustup component has one: ~/.rustup/toolchains/*/lib/rustlib/*/bin/llvm-profdata).
set -e
cd "$(dirname "$0")"
T=${CARGO_TARGET_DIR:-target}
D=$PWD/$T/pgo
rm -rf "$D" && mkdir -p "$D"
[ $# -gt 0 ] || set -- "$HOME/.cache/csvfab-bench/customers-5m.csv" "$HOME/.cache/csvfab-bench/customers-1m.csv"
PROFDATA=$(command -v llvm-profdata || ls ~/.rustup/toolchains/*/lib/rustlib/*/bin/llvm-profdata 2>/dev/null | head -1)
[ -x "$PROFDATA" ] || { echo "llvm-profdata not found (rustup component add llvm-tools)" >&2; exit 1; }

CARGO_TARGET_DIR=$T/pgo-gen RUSTFLAGS="-Cprofile-generate=$D/raw" cargo build --release -p csvfab-engine --example bench
for f in "$@"; do "$T/pgo-gen/release/examples/bench" "$f" >/dev/null; done
"$PROFDATA" merge -o "$D/merged.profdata" "$D/raw"
RUSTFLAGS="-Cprofile-use=$D/merged.profdata" cargo build --release -p csvfab
echo "PGO build: $T/release/csvfab"
