#!/bin/sh
# Accessibility check of the native Windows backend, from macOS or Linux:
# builds the counter example on the Windows machine of
# scripts/windows-remote.sh, opens each case of
# scripts/windows/a11y-expect.json (the six routes, a dialog, a sheet, a
# popover, an alert, a contrast theme) in the desktop session, dumps its UI
# Automation tree (scripts/windows/uia-dump.ps1 -Json, through
# scripts/windows/a11y-dump.ps1) and checks the roles, names and states a
# screen reader gets there (scripts/windows/a11y-check.py). Narrator is
# never started: it would speak aloud.
#
#   PRISM_WINDOWS_HOST=… scripts/windows-a11y.sh [out-dir] [case…]   (default target/windows-a11y)
#   scripts/windows-a11y.sh --check target/windows-a11y [case…]      (only the checks)
#
# The app runs in English (PRISM_LOCALE=en-US), so the names do not depend
# on the machine's language. The user of the remote helper must be signed
# in on the machine (see scripts/windows/run-in-session.ps1).
#
# Environment:
#   PRISM_WINDOWS_REMOTE   the remote helper (default scripts/windows-remote.sh;
#                          see it for PRISM_WINDOWS_HOST and the others)
#   PRISM_SMOKE_PROFILE    cargo profile (default dev; release builds with --release)
#   PRISM_SMOKE_NO_SYNC=1  run what the remote copy already has (and its build)
# The checks need python3.
set -eu

expect=scripts/windows/a11y-expect.json

if [ "${1:-}" = "--check" ]; then
  out=$2
  shift 2
  exec python3 scripts/windows/a11y-check.py "$expect" "$out" "$@"
fi

remote=${PRISM_WINDOWS_REMOTE:-scripts/windows-remote.sh}
out=${1:-target/windows-a11y}
[ $# -gt 0 ] && shift
profile=${PRISM_SMOKE_PROFILE:-dev}
case $profile in
  dev) build="cargo build -p counter" exe=debug ;;
  release) build="cargo build -p counter --release" exe=release ;;
  *) build="cargo build -p counter --profile $profile" exe=$profile ;;
esac
mkdir -p "$out"

if [ "${PRISM_SMOKE_NO_SYNC:-}" != 1 ]; then
  "$remote" sync
  "$remote" run "$build"
fi
"$remote" run "if not exist target\\windows-a11y mkdir target\\windows-a11y"

cases=$(mktemp "${TMPDIR:-/tmp}/windows-a11y-XXXXXX")
script=$(mktemp "${TMPDIR:-/tmp}/windows-a11y-XXXXXX")
trap 'rm -f "$cases" "$script"' EXIT
# One line per case: name, route, then the clicks and the environment in
# base64 (UTF-8), which no shell on the way changes.
python3 - "$expect" "$@" >"$cases" <<'EOF'
import base64, json, sys
only = set(sys.argv[2:])
b64 = lambda s: base64.b64encode(s.encode("utf-8")).decode("ascii")
for c in json.load(open(sys.argv[1], encoding="utf-8"))["cases"]:
    if only and c["name"] not in only:
        continue
    # The clicks wait for the first frame (a debug build's comes after 3 s):
    # a popover opens on its laid-out anchor.
    env = {"PRISM_LOCALE": "en-US", "PRISM_APPEARANCE": "light", "PRISM_DELAY": "4", **c.get("env", {})}
    lines = "".join(f"{k}={v}\n" for k, v in env.items())
    press = "".join(f"{p}\n" for p in c.get("press", []))
    print("\t".join([c["name"], c.get("path", "/"), b64(c.get("click", "")) or "-", b64(lines), b64(press) or "-"]))
EOF

tab=$(printf '\t')
while IFS="$tab" read -r name path click env press; do
  [ "$click" = - ] && click=
  [ "$press" = - ] && press=
  rm -f "$out/$name.json" "$out/$name.err"
  {
    printf '%s\r\n' "\$line = 'powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\a11y-dump.ps1 -Exe target\\$exe\\counter.exe -WorkingDirectory examples\\counter -Path $path -Out target\\windows-a11y\\$name.json -EnvBase64 $env'"
    [ -n "$click" ] && printf '%s\r\n' "\$line += ' -ClickBase64 $click'"
    [ -n "$press" ] && printf '%s\r\n' "\$line += ' -PressBase64 $press'"
    printf '%s\r\n' "& powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\run-in-session.ps1 -TimeoutSeconds 150 -Command \$line"
    printf '%s\r\n' "exit \$LASTEXITCODE"
  } >"$script"
  status=0
  "$remote" ps "$script" </dev/null || status=$?
  if [ "$status" -eq 125 ]; then
    echo "windows-a11y: FAIL: no desktop session to run WinUI in: sign the SSH user in on the machine" >&2
    exit 1
  fi
  "$remote" get "target/windows-a11y/$name.err" "$out" </dev/null >/dev/null 2>&1 || true
  if [ "$status" -ne 0 ] || ! "$remote" get "target/windows-a11y/$name.json" "$out" </dev/null >/dev/null 2>&1; then
    cat "$out/$name.err" >&2 2>/dev/null || true
    echo "windows-a11y: FAIL: $name ($path): no UI Automation tree (exit $status)" >&2
    exit 1
  fi
  echo "windows-a11y: $name ($path) dumped"
done <"$cases"

python3 scripts/windows/a11y-check.py "$expect" "$out" "$@"
