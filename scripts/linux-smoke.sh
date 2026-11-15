#!/bin/sh
# Smoke test of the GTK backend: runs the counter example under Xvfb on each
# tab, saves a PNG of each, and checks them (blank windows, crashes, controls
# that could not be pressed, a "+1" press that changed nothing).
#
# In the dev image (see docker/linux-dev.Dockerfile), from the repository:
#   docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/linux \
#     prism-linux-dev scripts/linux-smoke.sh target/linux-smoke
# The PNG checks need python3; without it (the image has none) run them on
# the host afterwards: scripts/linux-smoke.sh --check target/linux-smoke
set -eu

check() {
  out=$1
  n=0
  for png in "$out"/*.png; do
    python3 scripts/smoke/check.py png "$png" --min-size 300 400
    n=$((n + 1))
  done
  [ "$n" -ge 6 ] || { echo "linux-smoke: FAIL: $n snapshots, expected 6" >&2; exit 1; }
  # The click redrew the counter.
  if cmp -s "$out/home.png" "$out/home-clicked.png"; then
    echo "linux-smoke: FAIL: pressing +1 changed nothing" >&2
    exit 1
  fi
  echo "linux-smoke: $n snapshots OK"
}

if [ "${1:-}" = "--check" ]; then
  check "$2"
  exit 0
fi

out=${1:-target/linux-smoke}
mkdir -p "$out"
rm -f "$out"/*.png "$out"/*.log
target=${CARGO_TARGET_DIR:-target}
cargo build -p counter
bin="$target/debug/counter"
case $bin in /*) ;; *) bin="$PWD/$bin" ;; esac
abs_out=$(cd "$out" && pwd)

run() {
  name=$1 path=$2 click=$3
  log="$abs_out/$name.log"
  status=0
  # Fresh storage each run: the same state, so screenshots compare.
  data=$(mktemp -d)
  (cd examples/counter &&
    XDG_DATA_HOME="$data" PRISM_PATH="$path" PRISM_CLICK="$click" \
      PRISM_SNAPSHOT="$abs_out/$name.png" PRISM_DELAY=1.5 \
      timeout 60 xvfb-run -a -s "-screen 0 1280x1024x24" "$bin") >"$log" 2>&1 || status=$?
  rm -rf "$data"
  if [ "$status" -ne 0 ]; then
    cat "$log" >&2
    echo "linux-smoke: FAIL: $name ($path) exited with $status" >&2
    exit 1
  fi
  if grep -E "nothing labelled|panicked|CRITICAL" "$log" >&2; then
    echo "linux-smoke: FAIL: $name ($path): see above" >&2
    exit 1
  fi
  test -s "$abs_out/$name.png" || {
    echo "linux-smoke: FAIL: $name ($path) wrote no snapshot" >&2
    exit 1
  }
  echo "linux-smoke: $name ($path) OK"
}

run home / ""
run home-clicked / "+1"
run focus /focus ""
run components /components ""
run settings /settings ""
run detail /todos/new ""

if command -v python3 >/dev/null; then
  check "$out"
fi
