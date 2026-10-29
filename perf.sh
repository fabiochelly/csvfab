#!/bin/sh
# The performance guard: every number csvfab is judged by, measured the same way
# each time. Run it before and after a change; nothing may get slower.
#   ./perf.sh            everything (release build first)
#   ./perf.sh engine     only the engine benchmark
# Files: ~/.cache/csvfab-bench/customers-{1m,5m}.csv (on disk, not tmpfs, so
# "cold" means read from the disk: the file is evicted from the page cache first).
set -e
# One measurement at a time on this machine: concurrent runs would skew each other.
mkdir -p $HOME/.cache/csvfab-bench; exec 9>$HOME/.cache/csvfab-bench/lock; flock 9
cd "$(dirname "$0")"
B=$HOME/.cache/csvfab-bench
BIN=./target/release/csvfab
evict() { python3 -I -c "import os,sys; f=os.open(sys.argv[1], os.O_RDONLY); os.posix_fadvise(f,0,0,os.POSIX_FADV_DONTNEED); os.close(f)" "$1"; }
cargo build --release -p csvfab -p csvfab-engine --examples --bins -q 2>/dev/null || cargo build --release -p csvfab -p csvfab-engine --examples --bins

echo "## Engine (5M rows, 809 MB)"
evict $B/customers-5m.csv
echo "cold:"; ./target/release/examples/bench $B/customers-5m.csv 2>/dev/null | head -1 || true
echo "warm:"; ./target/release/examples/bench $B/customers-5m.csv
[ "$1" = engine ] && exit 0

echo; echo "## Start-up (median of 3), warm"
for f in customers-1m customers-5m; do
  for i in 1 2 3; do CSVFAB_PROFILE=1 CSVFAB_SHOT=none timeout 5 $BIN $B/$f.csv 2>&1 >/dev/null | grep "first frame with the grid" ; done \
    | sed -E 's/.*\[ *([0-9.]+) ms\].*/\1/' | sort -n | sed -n 2p | xargs printf "$f: grid shown at %s ms\n"
done
evict $B/customers-5m.csv
CSVFAB_PROFILE=1 CSVFAB_SHOT=none timeout 5 $BIN $B/customers-5m.csv 2>&1 >/dev/null | grep "first frame with the grid" | sed 's/^/customers-5m cold: /'

echo; echo "## Grid frame time, scrolling 9000 px a frame on 5M rows"
CSVFAB_GRID_TIMING=1 CSVFAB_SHOT=autoscroll:9000 timeout 6 $BIN $B/customers-5m.csv 2>&1 >/dev/null | grep -i "grid" | tail -2

echo; echo "## Memory and idle"
CSVFAB_FPS=1 CSVFAB_SHOT=none $BIN $B/customers-5m.csv 2>$B/fps.log >/dev/null &
P=$!; sleep 4
grep -E "^(Rss|Anonymous|Pss_File)" /proc/$P/smaps_rollup | tr -s ' ' | tr '\n' ' '; echo
: > $B/fps.log; sleep 3; kill $P
echo "frames drawn in 3 s idle: $(grep -c fps $B/fps.log) seconds with frames"
