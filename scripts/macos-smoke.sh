#!/usr/bin/env bash
# macOS smoke test: runs the counter example (AppKit) on each tab with the dev
# hooks, and checks that it exits cleanly, draws a non-blank window and exposes
# the expected accessibility tree.
#
#   scripts/macos-smoke.sh            # builds with cargo first
#   PRISM_SMOKE_BIN=path/to/counter scripts/macos-smoke.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
check="python3 $root/scripts/smoke/check.py"
out="${PRISM_SMOKE_OUT:-$(mktemp -d)}"
mkdir -p "$out"

bin="${PRISM_SMOKE_BIN:-}"
if [ -z "$bin" ]; then
  (cd "$root" && cargo build -p counter)
  bin="${CARGO_TARGET_DIR:-$root/target}/debug/counter"
fi

# Runs the app with a 60 s limit; fails on a crash, a timeout or a click that
# found nothing.
run() {
  local name="$1"
  shift
  local log="$out/$name.log"
  (
    cd "$root/examples/counter"
    env PRISM_LOCALE=en PRISM_DATA_DIR="$out/data-$name" PRISM_DELAY=1 \
      PRISM_SNAPSHOT="$out/$name.png" PRISM_A11Y_DUMP="$out/$name.json" "$@" "$bin"
  ) >"$log" 2>&1 &
  local pid=$! waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge 600 ]; then
      kill -9 "$pid" 2>/dev/null || true
      cat "$log"
      echo "smoke: FAIL: $name timed out" >&2
      exit 1
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  local status=0
  wait "$pid" || status=$?
  if [ "$status" -ne 0 ]; then
    cat "$log"
    echo "smoke: FAIL: $name exited with $status" >&2
    exit 1
  fi
  if grep -q "nothing labelled" "$log"; then
    cat "$log"
    echo "smoke: FAIL: $name: PRISM_CLICK found nothing" >&2
    exit 1
  fi
  $check png "$out/$name.png" --min-size 400 600
  echo "smoke: $name ok"
}

tabs="Todos,Focus,Components,Settings"

# Home: +1 is pressed twice, so the counter reads 2.
run home PRISM_PATH=/ PRISM_CLICK="+1|+1"
$check a11y "$out/home.json" --segments "$tabs" --selected 0 \
  --button "+1" --button "−1" --label "Count: 2" --label "New todo" \
  --text "Background refresh: not yet" --text "Rust UI · running on macos · 0 open todos"

run focus PRISM_PATH=/focus
$check a11y "$out/focus.json" --segments "$tabs" --selected 1 --label "Start focus"

run components PRISM_PATH=/components
$check a11y "$out/components.json" --segments "$tabs" --selected 2 --label "Components"

run settings PRISM_PATH=/settings
$check a11y "$out/settings.json" --segments "$tabs" --selected 3 \
  --label "Appearance" --label "Language"

# A pushed screen (deep link into the stack).
run detail PRISM_PATH=/todos/new
$check a11y "$out/detail.json" --label "New todo" --button "Add todo"

echo "smoke: macOS ok ($out)"
