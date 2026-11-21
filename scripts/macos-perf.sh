#!/bin/sh
# Startup timings of the counter on macOS (AppKit), to compare with
# scripts/windows-perf.sh: runs it several times on one route with
# PRISM_TRACE=1 and prints the medians of each phase (the app's tree, the
# views and their layout, the first frame, the memory then), through
# scripts/perf/report.py.
#
#   scripts/macos-perf.sh [out-dir]          (default target/macos-perf)
#
# Environment:
#   PRISM_PERF_PROFILES  cargo profiles (default "release dev")
#   PRISM_PERF_RUNS      runs per profile (default 5; the first, with no app
#                        data yet, is left out)
#   PRISM_PERF_PATH      the route (default /components)
#   PRISM_PERF_WINDOW    the window's size (default 460x720, as on Windows)
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/macos-perf}
profiles=${PRISM_PERF_PROFILES:-release dev}
runs=${PRISM_PERF_RUNS:-5}
path=${PRISM_PERF_PATH:-/components}
size=${PRISM_PERF_WINDOW:-460x720}
mkdir -p "$out"
rm -f "$out"/*.log "$out"/*.png

for profile in $profiles; do
  case $profile in
    dev) (cd "$root" && cargo build -p counter) && exe=debug ;;
    release) (cd "$root" && cargo build -p counter --release) && exe=release ;;
    *) (cd "$root" && cargo build -p counter --profile "$profile") && exe=$profile ;;
  esac
  bin="${CARGO_TARGET_DIR:-$root/target}/$exe/counter"
  # The app's data, kept from the second run on (as on Windows).
  data=$(mktemp -d)
  i=0
  while [ "$i" -lt "$runs" ]; do
    i=$((i + 1))
    name="$profile-$i"
    (
      cd "$root/examples/counter"
      env PRISM_TRACE=1 PRISM_LOCALE=en PRISM_APPEARANCE=light PRISM_DATA_DIR="$data" \
        PRISM_PATH="$path" PRISM_WINDOW="$size" PRISM_DELAY=1.5 \
        PRISM_SNAPSHOT="$out/$name.png" "$bin"
    ) >"$out/$name.log" 2>&1 || true
    echo "macos-perf: $name done" >&2
  done
  rm -rf "$data"
done

python3 "$root/scripts/perf/report.py" "$out"/*.log | tee "$out/report.md"
