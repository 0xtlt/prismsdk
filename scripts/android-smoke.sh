#!/usr/bin/env bash
# Smoke test on a running Android emulator: builds and installs the counter
# APK, opens each tab (deep links), and checks the activity is resumed, the
# process is alive, nothing crashed in logcat, and the screen is not blank.
# No instrumentation: adb only.
#
#   scripts/android-smoke.sh [out dir]      (default target/android-smoke)
#
# Environment: ANDROID_HOME, PRISM_SERIAL (default: the only device),
# PRISM_ANDROID_ABI (x86_64 on x86 emulators), PRISM_SMOKE_APK (a prebuilt
# APK instead of building), PRISM_SMOKE_WAIT (seconds per screen, default 8).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/target/android-smoke}"
mkdir -p "$OUT"
rm -f "$OUT"/*.png "$OUT"/*.txt
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"
ADB="$ANDROID_HOME/platform-tools/adb"
APP_ID=dev.prism.counter
ACTIVITY="$APP_ID/dev.prism.PrismActivity"
SCHEME=prism-counter
WAIT="${PRISM_SMOKE_WAIT:-8}"

SERIAL="${PRISM_SERIAL:-$("$ADB" devices | awk 'NR>1 && $2=="device" {print $1; exit}')}"
[[ -n "$SERIAL" ]] || { echo "android-smoke: no emulator running" >&2; exit 1; }
adb() { "$ADB" -s "$SERIAL" "$@"; }
fail() {
  echo "android-smoke: FAIL: $*" >&2
  adb logcat -d -t 400 > "$OUT/logcat-failure.txt" 2>&1 || true
  tail -80 "$OUT/logcat-failure.txt" >&2 || true
  exit 1
}

adb wait-for-device
# Boot finished and the launcher is up (emulator runners return early).
for _ in $(seq 1 120); do
  [[ "$(adb shell getprop sys.boot_completed | tr -d '\r')" == 1 ]] && break
  sleep 1
done
adb shell input keyevent 82 >/dev/null 2>&1 || true # dismiss the lock screen

# Build and install (no launch: the smoke test launches each screen), or
# install a prebuilt APK (PRISM_SMOKE_APK).
if [[ -n "${PRISM_SMOKE_APK:-}" ]]; then
  adb install -r "$PRISM_SMOKE_APK" >/dev/null
else
  # Through the prism CLI, which writes out the build script and Java host.
  (cd "$ROOT/examples/counter" && cargo run -q -p prism-cli -- run android --device "$SERIAL")
fi

# The resumed activity, from the activity manager.
resumed() {
  adb shell dumpsys activity activities | tr -d '\r' |
    grep -E 'mResumedActivity|topResumedActivity|ResumedActivity:' | head -1
}

check_screen() {
  local name=$1 path=$2
  adb logcat -c
  if [[ "$path" == / ]]; then
    adb shell am start -W -n "$ACTIVITY" >/dev/null
  else
    adb shell am start -W -a android.intent.action.VIEW -d "$SCHEME:$path" "$APP_ID" >/dev/null
  fi
  sleep "$WAIT"
  local top
  top="$(resumed)"
  [[ "$top" == *"$ACTIVITY"* || "$top" == *"$APP_ID/.PrismActivity"* ]] ||
    fail "$name: $ACTIVITY is not resumed (${top:-nothing resumed})"
  [[ -n "$(adb shell pidof "$APP_ID" | tr -d '\r')" ]] || fail "$name: $APP_ID is not running"
  adb logcat -d > "$OUT/logcat-$name.txt"
  if grep -E "FATAL EXCEPTION|Fatal signal|panicked at|ANR in $APP_ID|UnsatisfiedLinkError" \
    "$OUT/logcat-$name.txt" >&2; then
    fail "$name: crash in logcat"
  fi
  adb exec-out screencap -p > "$OUT/$name.png"
  if command -v python3 >/dev/null; then
    python3 "$ROOT/scripts/smoke/check.py" png "$OUT/$name.png" --min-size 300 400 ||
      fail "$name: blank screen"
  fi
  echo "android-smoke: $name ($path) OK"
}

check_screen home /
check_screen focus /focus
check_screen components /components
check_screen settings /settings
# Back to the first screen: the process survived every switch.
check_screen home-again /
echo "android-smoke: all screens OK"
