#!/usr/bin/env bash
# iOS simulator smoke test: builds the counter example for the simulator
# (scripts/ios-run.sh), installs it, then launches it on each tab with the dev
# hooks (PRISM_SNAPSHOT, PRISM_A11Y_DUMP, PRISM_CLICK) and checks that it
# draws a non-blank window and exposes the expected accessibility labels.
#
#   PRISM_SIM=<udid|name> scripts/ios-smoke.sh   # default: first available iPhone
#   PRISM_SMOKE_SKIP_BUILD=1                     # reuse the installed app
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
check="python3 $root/scripts/smoke/check.py"
out="${PRISM_SMOKE_OUT:-$(mktemp -d)}"
mkdir -p "$out"
bundle=dev.prism.counter

udid="$(xcrun simctl list devices available -j | PRISM_SIM="${PRISM_SIM:-}" python3 -c '
import json, os, sys
want = os.environ["PRISM_SIM"]
devices = [d for r, ds in json.load(sys.stdin)["devices"].items() if "SimRuntime.iOS-" in r for d in ds]
pick = [d for d in devices if want in (d["udid"], d["name"])] if want else [d for d in devices if d["name"].startswith("iPhone")]
booted = [d for d in pick if d["state"] == "Booted"]
print((booted or pick)[0]["udid"] if pick else "")
')"
if [ -z "$udid" ]; then
  echo "smoke: FAIL: no iOS simulator (PRISM_SIM=${PRISM_SIM:-})" >&2
  exit 1
fi
echo "smoke: simulator $udid"
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null

if [ "${PRISM_SMOKE_SKIP_BUILD:-}" != 1 ]; then
  PRISM_SIM="$udid" "$root/scripts/ios-run.sh" counter --no-launch
fi

# Launches the app and waits (90 s at most) for it to quit after its snapshot.
run() {
  local name="$1"
  shift
  local log="$out/$name.log"
  rm -f "$out/$name.png" "$out/$name.json"
  local vars=(
    "SIMCTL_CHILD_PRISM_LOCALE=en"
    "SIMCTL_CHILD_PRISM_DELAY=2"
    "SIMCTL_CHILD_PRISM_SNAPSHOT=$out/$name.png"
    "SIMCTL_CHILD_PRISM_A11Y_DUMP=$out/$name.json"
  )
  for v in "$@"; do vars+=("SIMCTL_CHILD_$v"); done
  env "${vars[@]}" xcrun simctl launch --console-pty --terminate-running-process \
    "$udid" "$bundle" >"$log" 2>&1 &
  local pid=$! waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge 900 ]; then
      kill "$pid" 2>/dev/null || true
      xcrun simctl terminate "$udid" "$bundle" 2>/dev/null || true
      cat "$log"
      echo "smoke: FAIL: $name timed out" >&2
      exit 1
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  wait "$pid" || true
  if ! grep -q "snapshot written" "$log"; then
    cat "$log"
    echo "smoke: FAIL: $name: no snapshot (crash?)" >&2
    exit 1
  fi
  if grep -q "nothing labelled" "$log"; then
    cat "$log"
    echo "smoke: FAIL: $name: PRISM_CLICK found nothing" >&2
    exit 1
  fi
  $check png "$out/$name.png" --min-size 300 600
  echo "smoke: $name ok"
}

tabs=(--segments "Todos,Focus,Components,Settings")

run home PRISM_PATH=/ "PRISM_CLICK=+1|+1"
$check a11y "$out/home.json" "${tabs[@]}" --selected 0 --button "+1" --label "Count: 2"

run focus PRISM_PATH=/focus
$check a11y "$out/focus.json" "${tabs[@]}" --selected 1 --label "Start focus"

run components PRISM_PATH=/components
$check a11y "$out/components.json" "${tabs[@]}" --selected 2

run settings PRISM_PATH=/settings
$check a11y "$out/settings.json" "${tabs[@]}" --selected 3 --label "Appearance"

echo "smoke: iOS ok ($out)"
