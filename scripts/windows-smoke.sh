#!/bin/sh
# Smoke test of the native Windows backend, from macOS or Linux: builds the
# counter example on the Windows machine of scripts/windows-remote.sh, runs
# it on each tab, fetches a PNG of each and checks them here (blank
# windows, crashes, controls that could not be pressed, a "+1" press that
# changed nothing). Works over plain SSH: the backend draws its snapshots
# offscreen, no desktop needed.
#
#   PRISM_WINDOWS_HOST=… scripts/windows-smoke.sh [out-dir]   (default target/windows-smoke)
#   scripts/windows-smoke.sh --check target/windows-smoke     (only the PNG checks)
#
# Environment:
#   PRISM_WINDOWS_REMOTE   the remote helper (default scripts/windows-remote.sh;
#                          see it for PRISM_WINDOWS_HOST and the others)
#   PRISM_APPEARANCE       light or dark: the system appearance the app sees
#   PRISM_SMOKE_NO_SYNC=1  run what the remote copy already has
# The PNG checks need python3.
set -eu

check() {
  out=$1
  n=0
  for png in "$out"/*.png; do
    python3 scripts/smoke/check.py png "$png" --min-size 300 400
    n=$((n + 1))
  done
  [ "$n" -ge 6 ] || { echo "windows-smoke: FAIL: $n snapshots, expected 6" >&2; exit 1; }
  # The click redrew the counter.
  if cmp -s "$out/home.png" "$out/home-clicked.png"; then
    echo "windows-smoke: FAIL: pressing +1 changed nothing" >&2
    exit 1
  fi
  echo "windows-smoke: $n snapshots OK"
}

if [ "${1:-}" = "--check" ]; then
  check "$2"
  exit 0
fi

remote=${PRISM_WINDOWS_REMOTE:-scripts/windows-remote.sh}
out=${1:-target/windows-smoke}
appearance=${PRISM_APPEARANCE:-light}
mkdir -p "$out"
rm -f "$out"/*.png "$out"/*.log

[ "${PRISM_SMOKE_NO_SYNC:-}" = 1 ] || "$remote" sync
"$remote" run "cargo build -p counter"
"$remote" run "(if exist target\\windows-smoke rmdir /s /q target\\windows-smoke) & mkdir target\\windows-smoke"

run() {
  name=$1 path=$2 click=$3
  # The app runs in examples\counter (its assets): paths are relative to
  # it. Fresh storage each run (LOCALAPPDATA): the same state, so
  # screenshots compare. GUI apps start from cmd (`start /wait /b`), not
  # PowerShell.
  line="set \"LOCALAPPDATA=%TEMP%\\prism-smoke-$name\"&& (if exist \"%TEMP%\\prism-smoke-$name\" rmdir /s /q \"%TEMP%\\prism-smoke-$name\")&& set \"PRISM_PATH=$path\"&& set \"PRISM_CLICK=$click\"&& set \"PRISM_APPEARANCE=$appearance\"&& set \"PRISM_SNAPSHOT=..\\..\\target\\windows-smoke\\$name.png\"&& set PRISM_DELAY=1.5&& cd examples\\counter&& start \"\" /wait /b ..\\..\\target\\debug\\counter.exe > ..\\..\\target\\windows-smoke\\$name.log 2>&1"
  status=0
  "$remote" run "$line" || status=$?
  "$remote" get "target/windows-smoke/$name.log" "$out" >/dev/null 2>&1 || true
  if [ "$status" -ne 0 ]; then
    cat "$out/$name.log" >&2 || true
    echo "windows-smoke: FAIL: $name ($path) exited with $status" >&2
    exit 1
  fi
  if grep -E "nothing labelled|panicked|snapshot failed" "$out/$name.log" >&2; then
    echo "windows-smoke: FAIL: $name ($path): see above" >&2
    exit 1
  fi
  "$remote" get "target/windows-smoke/$name.png" "$out" >/dev/null 2>&1 || {
    cat "$out/$name.log" >&2 || true
    echo "windows-smoke: FAIL: $name ($path) wrote no snapshot" >&2
    exit 1
  }
  echo "windows-smoke: $name ($path) OK"
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
