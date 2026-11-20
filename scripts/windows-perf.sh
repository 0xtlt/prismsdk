#!/bin/sh
# Startup and tab-switch timings of the counter on Windows (WinUI), from
# macOS or Linux, on the machine of scripts/windows-remote.sh: builds the
# counter (release and debug), runs it several times on one route in the
# signed-in desktop session with PRISM_TRACE=1, switches tabs, and prints
# the medians of each phase (scripts/perf/report.py):
#
# - WinUI's start, the app's tree, the XAML elements (create, style,
#   children), the layout (measure, place), XAML's own layout, the first
#   frame, and the memory then;
# - each tab switch, from the click to the frame showing it.
#
#   PRISM_WINDOWS_HOST=… scripts/windows-perf.sh [out-dir]   (default target/windows-perf)
#   scripts/windows-perf.sh --report target/windows-perf      (only the table)
#
# Environment:
#   PRISM_WINDOWS_REMOTE  the remote helper (default scripts/windows-remote.sh)
#   PRISM_PERF_PROFILES   cargo profiles, in order (default "release dev")
#   PRISM_PERF_RUNS       runs per profile (default 5; the first warms the
#                         disk cache and is left out of the medians)
#   PRISM_PERF_PATH       the route (default /components)
#   PRISM_PERF_CLICKS     PRISM_CLICK steps after the first frame (default:
#                         to Todos, back, to Settings, back)
#   PRISM_PERF_ENV        more variables for the app, `A=1 B=2`
#   PRISM_PERF_NO_SYNC=1  run what the remote copy already has
#   PRISM_PERF_NO_BUILD=1 run what it already built
# Needs python3 for the table.
set -eu

out=${1:-target/windows-perf}
if [ "$out" = "--report" ]; then
  python3 scripts/perf/report.py "$2"/*.log
  exit 0
fi

remote=${PRISM_WINDOWS_REMOTE:-scripts/windows-remote.sh}
profiles=${PRISM_PERF_PROFILES:-release dev}
runs=${PRISM_PERF_RUNS:-5}
path=${PRISM_PERF_PATH:-/components}
clicks=${PRISM_PERF_CLICKS:-wait:800|Todos|wait:800|Components|wait:800|Settings|wait:800|Components|wait:800}
extra=${PRISM_PERF_ENV:-}
mkdir -p "$out"
rm -f "$out"/*.log "$out"/*.png

[ "${PRISM_PERF_NO_SYNC:-}" = 1 ] || "$remote" sync
"$remote" run "(if exist target\\windows-perf rmdir /s /q target\\windows-perf) & mkdir target\\windows-perf"

script=$(mktemp "${TMPDIR:-/tmp}/windows-perf-XXXXXX")
trap 'rm -f "$script"' EXIT

for profile in $profiles; do
  case $profile in
    dev) build="cargo build -p counter" exe=debug ;;
    release) build="cargo build -p counter --release" exe=release ;;
    *) build="cargo build -p counter --profile $profile" exe=$profile ;;
  esac
  [ "${PRISM_PERF_NO_BUILD:-}" = 1 ] || "$remote" run "$build"
  i=0
  while [ "$i" -lt "$runs" ]; do
    i=$((i + 1))
    name="$profile-$i"
    # Like windows-smoke.sh: a batch file in the session (run-in-session),
    # fresh storage each run, English labels for the clicks.
    {
      printf '%s\r\n' "\$bat = Join-Path (Get-Location).ProviderPath 'target\\windows-perf\\$name.cmd'"
      printf '%s\r\n' "[IO.File]::WriteAllText(\$bat, @'"
      printf '%s\r\n' "@echo off"
      printf '%s\r\n' "set \"LOCALAPPDATA=%TEMP%\\prism-perf\""
      printf '%s\r\n' "if exist \"%TEMP%\\prism-perf\" rmdir /s /q \"%TEMP%\\prism-perf\""
      printf '%s\r\n' "set PRISM_TRACE=1"
      printf '%s\r\n' "set PRISM_LOCALE=en"
      printf '%s\r\n' "set PRISM_APPEARANCE=light"
      printf '%s\r\n' "set \"PRISM_PATH=$path\""
      printf '%s\r\n' "set \"PRISM_CLICK=$clicks\""
      printf '%s\r\n' "set \"PRISM_SNAPSHOT=%~dp0$name.png\""
      printf '%s\r\n' "set PRISM_DELAY=1.5"
      printf '%s\r\n' "set PRISM_AFTER=0.5"
      for kv in $extra; do printf '%s\r\n' "set \"$kv\""; done
      printf '%s\r\n' "cd /d \"%~dp0..\\..\\examples\\counter\""
      printf '%s\r\n' "start \"\" /wait /b ..\\..\\target\\$exe\\counter.exe > \"%~dp0$name.log\" 2>&1"
      printf '%s\r\n' "exit /b %errorlevel%"
      printf '%s\r\n' "'@, [Text.Encoding]::ASCII)"
      printf '%s\r\n' "& powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\run-in-session.ps1 -TimeoutSeconds 120 -Command \"call \`\"\$bat\`\"\""
      printf '%s\r\n' "exit \$LASTEXITCODE"
    } >"$script"
    status=0
    "$remote" ps "$script" >/dev/null 2>&1 || status=$?
    if [ "$status" -eq 125 ]; then
      echo "windows-perf: no desktop session to run WinUI in: sign the SSH user in on the machine" >&2
      exit 1
    fi
    "$remote" get "target/windows-perf/$name.log" "$out" >/dev/null 2>&1 || {
      echo "windows-perf: $name wrote no log (exit $status)" >&2
      exit 1
    }
    "$remote" get "target/windows-perf/$name.png" "$out" >/dev/null 2>&1 || true
    echo "windows-perf: $name done" >&2
  done
done

python3 scripts/perf/report.py "$out"/*.log | tee "$out/report.md"
