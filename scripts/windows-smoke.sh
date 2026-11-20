#!/bin/sh
# Smoke test of the native Windows backend, from macOS or Linux: builds the
# counter example on the Windows machine of scripts/windows-remote.sh, runs
# it on each tab, fetches a PNG of each and checks them here (blank
# windows, crashes, controls that could not be pressed, a "+1" press that
# changed nothing).
#
# WinUI renders only in a desktop session: when the build has the WinUI
# runtime next to it, each run goes through scripts/windows/run-in-session.ps1
# into the signed-in session of the SSH user (who must be signed in, on
# screen or switched away from). A backend that draws offscreen (Direct2D)
# runs over plain SSH, in session 0.
#
#   PRISM_WINDOWS_HOST=… scripts/windows-smoke.sh [out-dir]   (default target/windows-smoke)
#   scripts/windows-smoke.sh --check target/windows-smoke     (only the PNG checks)
#
# Environment:
#   PRISM_WINDOWS_REMOTE   the remote helper (default scripts/windows-remote.sh;
#                          see it for PRISM_WINDOWS_HOST and the others)
#   PRISM_APPEARANCE       light or dark: the system appearance the app sees
#   PRISM_WINDOWS_SESSION  desktop or ssh: where the app runs (default: desktop
#                          when the WinUI runtime is next to the build)
#   PRISM_SMOKE_PROFILE    cargo profile (default dev; release builds with --release)
#   PRISM_SMOKE_NO_SYNC=1  run what the remote copy already has
#   PRISM_SMOKE_A11Y=1     then check the UI Automation trees too
#                          (scripts/windows-a11y.sh, into <out-dir>/a11y)
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
profile=${PRISM_SMOKE_PROFILE:-dev}
case $profile in
  dev) build="cargo build -p counter" exe=debug ;;
  release) build="cargo build -p counter --release" exe=release ;;
  *) build="cargo build -p counter --profile $profile" exe=$profile ;;
esac
mkdir -p "$out"
rm -f "$out"/*.png "$out"/*.log

[ "${PRISM_SMOKE_NO_SYNC:-}" = 1 ] || "$remote" sync
"$remote" run "$build"
"$remote" run "(if exist target\\windows-smoke rmdir /s /q target\\windows-smoke) & mkdir target\\windows-smoke"

session=${PRISM_WINDOWS_SESSION:-}
if [ -z "$session" ]; then
  if "$remote" run "if exist target\\$exe\\Microsoft.UI.Xaml.Controls.dll (exit 0) else (exit 1)" 2>/dev/null; then
    session=desktop
  else
    session=ssh
  fi
fi
echo "windows-smoke: running in the $session session"

script=$(mktemp "${TMPDIR:-/tmp}/windows-smoke-XXXXXX")
trap 'rm -f "$script"' EXIT

run() {
  name=$1 path=$2 click=$3
  # The app runs in examples\counter (its assets): paths are relative to
  # it. Fresh storage each run (LOCALAPPDATA): the same state, so
  # screenshots compare. GUI apps start from cmd (`start /wait /b`), not
  # PowerShell. The batch file keeps the values out of every shell's
  # quoting (ssh, cmd, PowerShell, the task scheduler).
  {
    printf '%s\r\n' "\$bat = Join-Path (Get-Location).ProviderPath 'target\\windows-smoke\\$name.cmd'"
    printf '%s\r\n' "[IO.File]::WriteAllText(\$bat, @'"
    printf '%s\r\n' "@echo off"
    printf '%s\r\n' "set \"LOCALAPPDATA=%TEMP%\\prism-smoke-$name\""
    printf '%s\r\n' "if exist \"%TEMP%\\prism-smoke-$name\" rmdir /s /q \"%TEMP%\\prism-smoke-$name\""
    printf '%s\r\n' "set \"PRISM_PATH=$path\""
    printf '%s\r\n' "set \"PRISM_CLICK=$click\""
    printf '%s\r\n' "set \"PRISM_APPEARANCE=$appearance\""
    printf '%s\r\n' "set \"PRISM_SNAPSHOT=%~dp0$name.png\""
    printf '%s\r\n' "set PRISM_DELAY=1.5"
    printf '%s\r\n' "cd /d \"%~dp0..\\..\\examples\\counter\""
    printf '%s\r\n' "start \"\" /wait /b ..\\..\\target\\$exe\\counter.exe > \"%~dp0$name.log\" 2>&1"
    printf '%s\r\n' "exit /b %errorlevel%"
    printf '%s\r\n' "'@, [Text.Encoding]::ASCII)"
    if [ "$session" = desktop ]; then
      printf '%s\r\n' "& powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\run-in-session.ps1 -TimeoutSeconds 120 -Command \"call \`\"\$bat\`\"\""
    else
      printf '%s\r\n' "& \$env:ComSpec /d /c \"call \`\"\$bat\`\"\""
    fi
    printf '%s\r\n' "exit \$LASTEXITCODE"
  } >"$script"
  status=0
  "$remote" ps "$script" || status=$?
  "$remote" get "target/windows-smoke/$name.log" "$out" >/dev/null 2>&1 || true
  if [ "$status" -eq 125 ] && [ "$session" = desktop ]; then
    echo "windows-smoke: FAIL: no desktop session to run WinUI in: sign the SSH user in on the machine (fast user switching keeps the session)" >&2
    exit 1
  fi
  if [ "$status" -ne 0 ]; then
    cat "$out/$name.log" >&2 || true
    echo "windows-smoke: FAIL: $name ($path) exited with $status" >&2
    exit 1
  fi
  if grep -E "nothing labelled|panicked|snapshot failed|cannot start" "$out/$name.log" >&2; then
    echo "windows-smoke: FAIL: $name ($path): see above" >&2
    exit 1
  fi
  "$remote" get "target/windows-smoke/$name.png" "$out" >/dev/null 2>&1 || {
    cat "$out/$name.log" >&2 || true
    echo "windows-smoke: FAIL: $name ($path) wrote no snapshot" >&2
    exit 1
  }
  # What DWM showed of the window (WinUI in a desktop session).
  "$remote" get "target/windows-smoke/$name-window.png" "$out" >/dev/null 2>&1 || true
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

# Roles, names and states for screen readers, on the same build.
if [ "${PRISM_SMOKE_A11Y:-}" = 1 ]; then
  PRISM_SMOKE_NO_SYNC=1 PRISM_SMOKE_PROFILE=$profile scripts/windows-a11y.sh "$out/a11y"
fi
