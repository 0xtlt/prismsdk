#!/bin/sh
# Proof of the Windows device APIs (location, motion, camera, share sheet,
# passkeys, haptics), from macOS or Linux, on the Windows machine of
# scripts/windows-remote.sh. It shows no prompt and no system UI, so it
# runs with the session locked too:
#
# 1. builds the counter and runs it in the desktop session (through
#    scripts/windows/run-in-session.ps1) on /components, the device card,
#    with PRISM_DEVICE_REPORT; fetches the report, the log and a PNG;
# 2. checks the report here (scripts/windows/device-report-check.py): each
#    capability says what the checks behind it saw; the motion sensors
#    give a sample when the PC has them; the Camera app's
#    capture UI answers for its URI and a photo's file token goes in and
#    out of SharedStorageAccessManager; the window has a DataTransferManager
#    with the app's handler; webauthn.dll loads, says its API version and
#    whether Windows Hello can make passkeys, and its errors map to Prism's;
#    the vibration devices are counted; nothing says where the PC is;
# 3. when location access is already refused (no prompt then), presses
#    « Locate »: request_permission and current_location answer Denied
#    through the app (locate.png). When access is allowed, the report has
#    a position and a watch's first event (their accuracy and age only):
#    the button would put the PC's coordinates on screen, so it is not
#    pressed.
#
# What only a person at the unlocked PC can prove (each shows Windows' UI):
# - share: the Share dialog opens over the window; a target (Mail, Nearby
#   sharing, an app) gets the text, the link and the files;
# - camera: « Camera » opens the Camera app's capture UI; the photo comes
#   back as a JPEG; closing it gives Cancelled;
# - passkeys: create and get with Windows Hello, a security key and a
#   phone; closing the dialog gives Cancelled; an excluded credential gives
#   InvalidStateError;
# - location: Windows 11's prompt (24H2 and later, once per app) when
#   access is not decided yet; positions and watch events when it was not
#   allowed during the run;
# - haptics: a PC with a vibration device feels the patterns;
# - motion: on a tablet or 2-in-1, the device card's gyroscope switch
#   follows the PC as it moves (axes, units).
#
#   PRISM_WINDOWS_HOST=… scripts/windows-device-apis.sh [out-dir]   (default target/windows-device-apis)
#   scripts/windows-device-apis.sh --check <out-dir>               (only the checks)
#
# Environment: PRISM_WINDOWS_REMOTE (default scripts/windows-remote.sh),
# PRISM_SMOKE_PROFILE and PRISM_SMOKE_NO_SYNC=1 as in scripts/windows-smoke.sh.
# The checks need python3. The fetched files name no account: a path under
# C:\Users\<name> reads C:\Users\<account> before it is printed or kept.
set -eu

# Replaces the account in the paths of the files fetched into $1.
redact() {
  for f in "$1"/*.log "$1"/*.json; do
    [ -f "$f" ] || continue
    sed -E 's/([A-Za-z]:[\\/]+[Uu][Ss][Ee][Rr][Ss][\\/]+)[^\\/"]+/\1<account>/g' "$f" >"$f.tmp" && mv "$f.tmp" "$f"
  done
}

check() {
  python3 scripts/windows/device-report-check.py "$1/device-report.json"
  if grep -E "panicked|cannot start|no answer at startup" "$1/device.log" >&2; then
    echo "windows-device-apis: FAIL: see the log above" >&2
    exit 1
  fi
}

if [ "${1:-}" = "--check" ]; then
  check "$2"
  exit 0
fi

remote=${PRISM_WINDOWS_REMOTE:-scripts/windows-remote.sh}
out=${1:-target/windows-device-apis}
profile=${PRISM_SMOKE_PROFILE:-dev}
case $profile in
  dev) build="cargo build -p counter" exe=debug ;;
  release) build="cargo build -p counter --release" exe=release ;;
  *) build="cargo build -p counter --profile $profile" exe=$profile ;;
esac
mkdir -p "$out"
rm -f "$out"/*.png "$out"/*.log "$out"/*.json

[ "${PRISM_SMOKE_NO_SYNC:-}" = 1 ] || "$remote" sync
"$remote" run "$build"
"$remote" run "(if exist target\\windows-device-apis rmdir /s /q target\\windows-device-apis) & mkdir target\\windows-device-apis"

script=$(mktemp "${TMPDIR:-/tmp}/windows-device-apis-XXXXXX")
trap 'rm -f "$script"' EXIT

# Runs the counter on /components in the desktop session: fresh storage
# (removed after), the snapshot as <name>.png, then any `set` lines given.
run() {
  name=$1
  shift
  {
    printf '%s\r\n' "\$bat = Join-Path (Get-Location).ProviderPath 'target\\windows-device-apis\\$name.cmd'"
    printf '%s\r\n' "[IO.File]::WriteAllText(\$bat, @'"
    printf '%s\r\n' "@echo off"
    printf '%s\r\n' "set \"LOCALAPPDATA=%TEMP%\\prism-device-$name\""
    printf '%s\r\n' "if exist \"%TEMP%\\prism-device-$name\" rmdir /s /q \"%TEMP%\\prism-device-$name\""
    printf '%s\r\n' "set \"PRISM_PATH=/components\""
    printf '%s\r\n' "set \"PRISM_SNAPSHOT=%~dp0$name.png\""
    printf '%s\r\n' "set PRISM_DELAY=1.5"
    for line in "$@"; do
      printf '%s\r\n' "set \"$line\""
    done
    printf '%s\r\n' "cd /d \"%~dp0..\\..\\examples\\counter\""
    printf '%s\r\n' "start \"\" /wait /b ..\\..\\target\\$exe\\counter.exe > \"%~dp0$name.log\" 2>&1"
    printf '%s\r\n' "set \"code=%errorlevel%\""
    printf '%s\r\n' "rmdir /s /q \"%TEMP%\\prism-device-$name\" 2>nul"
    printf '%s\r\n' "exit /b %code%"
    printf '%s\r\n' "'@, [Text.Encoding]::ASCII)"
    printf '%s\r\n' "& powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\run-in-session.ps1 -TimeoutSeconds 150 -Command \"call \`\"\$bat\`\"\""
    printf '%s\r\n' "exit \$LASTEXITCODE"
  } >"$script"
  status=0
  "$remote" ps "$script" || status=$?
  "$remote" get "target/windows-device-apis/$name.log" "$out" >/dev/null 2>&1 || true
  redact "$out"
  if [ "$status" -eq 125 ]; then
    echo "windows-device-apis: FAIL: no desktop session to run WinUI in: the SSH user must be signed in on the machine" >&2
    exit 1
  fi
  if [ "$status" -ne 0 ]; then
    cat "$out/$name.log" >&2 || true
    echo "windows-device-apis: FAIL: $name exited with $status" >&2
    exit 1
  fi
  "$remote" get "target/windows-device-apis/$name.png" "$out" >/dev/null 2>&1 || {
    cat "$out/$name.log" >&2 || true
    echo "windows-device-apis: FAIL: $name wrote no snapshot" >&2
    exit 1
  }
  echo "windows-device-apis: $name OK"
}

run device "PRISM_DEVICE_REPORT=%~dp0device-report.json"
"$remote" get "target/windows-device-apis/device-report.json" "$out" >/dev/null 2>&1 || {
  cat "$out/device.log" >&2 || true
  echo "windows-device-apis: FAIL: no device report" >&2
  exit 1
}
redact "$out"
check "$out"

# Location refused: the app's own calls answer Denied, without a prompt.
if grep -q '"permission": "Denied"' "$out/device-report.json" &&
  grep -q '"location": true' "$out/device-report.json"; then
  run locate "PRISM_CLICK=Locate"
  echo "windows-device-apis: locate.png shows what « Locate » answered (Denied expected)"
else
  echo "windows-device-apis: « Locate » not pressed: location access is not refused (see the report)"
fi
echo "windows-device-apis: OK ($out)"
