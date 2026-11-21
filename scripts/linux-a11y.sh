#!/bin/sh
# Accessibility check of the GTK backend: runs the counter example under
# Xvfb with the AT-SPI bus, dumps the tree Orca and other screen readers get
# on each case of scripts/linux/a11y-expect.json (scripts/linux/atspi-dump.py:
# roles, names, descriptions, attributes, relations, the Table and TableCell
# interfaces) and checks it (scripts/linux/a11y-check.py). Orca itself is
# never started.
#
# In an a11y image (the AT-SPI bus and its typelib: GTK 4.8 on top of the
# dev image, or the newest GTK, see docker/linux-a11y.Dockerfile), from the
# repository:
#   docker build -f docker/linux-a11y.Dockerfile --target gtk48 -t prism-linux-a11y:gtk4.8 docker
#   docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/linux \
#     prism-linux-a11y:gtk4.8 scripts/linux-a11y.sh target/linux-a11y [case…]
#   scripts/linux-a11y.sh --check target/linux-a11y [case…]   (only the checks)
# The newest GTK: `--target newest -t prism-linux-a11y:newest` (Debian
# unstable), with a target folder of its own.
#
# Each case writes <case>.json (the tree) and <case>.txt (one line per
# element) to the output folder; a case's "press" keys are pressed on its
# elements first (a drawn button pressed with Enter, then Space). The app runs in English
# (PRISM_LOCALE=en-US) with fresh storage.
#
# Environment:
#   PRISM_A11Y_BIN   dump this build of the counter instead of building one
set -eu

expect=scripts/linux/a11y-expect.json

if [ "${1:-}" = "--check" ]; then
  out=$2
  shift 2
  exec python3 scripts/linux/a11y-check.py "$expect" "$out" "$@"
fi

# A session bus of its own, for the AT-SPI bus launcher.
if [ -z "${PRISM_A11Y_SESSION:-}" ]; then
  PRISM_A11Y_SESSION=1 exec dbus-run-session -- "$0" "$@"
fi

out=${1:-target/linux-a11y}
[ $# -gt 0 ] && shift
mkdir -p "$out"
abs_out=$(cd "$out" && pwd)
rm -f "$abs_out"/*.json "$abs_out"/*.txt "$abs_out"/*.log

bin=${PRISM_A11Y_BIN:-}
if [ -z "$bin" ]; then
  cargo build -p counter
  bin="${CARGO_TARGET_DIR:-target}/debug/counter"
fi
case $bin in /*) ;; *) bin="$PWD/$bin" ;; esac
app=$(basename "$bin")

/usr/libexec/at-spi-bus-launcher --launch-immediately >"$abs_out/atspi.log" 2>&1 &
bus=$!
display=:$((90 + $$ % 9))
Xvfb "$display" -screen 0 1280x1024x24 >"$abs_out/xvfb.log" 2>&1 &
xvfb=$!
trap 'kill $bus $xvfb 2>/dev/null || true' EXIT
sleep 1
export DISPLAY="$display"

# One line per case: name and route.
cases=$(python3 - "$expect" "$@" <<'EOF'
import json, sys
cases = json.load(open(sys.argv[1]))["cases"]
only = sys.argv[2:]
for c in cases:
    if not only or c["name"] in only:
        print(c["name"], c["path"])
EOF
)

# A case's key presses, as atspi-dump.py arguments, one per line.
presses() {
  python3 - "$expect" "$1" <<'PY'
import json, sys
for c in json.load(open(sys.argv[1]))["cases"]:
    if c["name"] == sys.argv[2]:
        for p in c.get("press", []):
            print("--press", p["role"], p["name"], p["key"], sep="\n")
        if c.get("back"):
            print("--back")
PY
}

echo "$cases" | while read -r name path; do
  [ -n "$name" ] || continue
  # The presses as positional arguments (names have spaces).
  set --
  while IFS= read -r arg; do
    if [ -n "$arg" ]; then set -- "$@" "$arg"; fi
  done <<ARGS
$(presses "$name")
ARGS
  data=$(mktemp -d)
  (cd examples/counter &&
    exec env XDG_DATA_HOME="$data" PRISM_LOCALE=en-US PRISM_PATH="$path" \
      "$bin") >"$abs_out/$name.log" 2>&1 &
  pid=$!
  status=0
  python3 scripts/linux/atspi-dump.py "$app" "$abs_out/$name.json" \
    --text "$abs_out/$name.txt" --wait 60 "$@" || status=$?
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  rm -rf "$data"
  if [ "$status" -ne 0 ]; then
    cat "$abs_out/$name.log" >&2
    echo "linux-a11y: FAIL: $name ($path): no tree" >&2
    exit 1
  fi
  if grep -E "panicked|CRITICAL" "$abs_out/$name.log" >&2; then
    echo "linux-a11y: FAIL: $name ($path): see above" >&2
    exit 1
  fi
  echo "linux-a11y: $name ($path) dumped"
done

python3 scripts/linux/a11y-check.py "$expect" "$abs_out" "$@"
