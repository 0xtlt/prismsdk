#!/bin/sh
# Tray smoke test (StatusNotifierItem and com.canonical.dbusmenu): runs the
# counter example on a session bus of its own, with a stand-in for the
# panel's watcher (scripts/mock-sni-watcher.py), and checks with gdbus that
#  - the tray registers with the watcher and serves its properties and menu;
#  - a menu click reaches the app: the todo it names is done, so the tray's
#    count drops (NewTitle, LayoutUpdated, the menu without it);
#  - a click on the icon (Activate) brings the window forward: it takes the
#    X input focus (no window manager on Xvfb moves it otherwise);
#  - the watcher sees the item leave when the app quits.
#
# In the dev image (see docker/linux-dev.Dockerfile), from the repository:
#   docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/linux \
#     prism-linux-dev scripts/linux-tray-smoke.sh target/linux-tray
# Logs (app, watcher, gdbus calls, signals) land in the output directory.
set -eu

out=${1:-target/linux-tray}

if [ -z "${PRISM_TRAY_SMOKE_INNER:-}" ]; then
  mkdir -p "$out"
  rm -f "$out"/*.log
  cargo build -p counter
  # A private session bus and display for the rest.
  PRISM_TRAY_SMOKE_INNER=1 exec dbus-run-session -- \
    xvfb-run -a -s "-screen 0 1280x1024x24" "$0" "$@"
fi

target=${CARGO_TARGET_DIR:-target}
bin="$target/debug/counter"
case $bin in /*) ;; *) bin="$PWD/$bin" ;; esac
out=$(cd "$out" && pwd)
calls="$out/gdbus.log"
: >"$calls"

fail() {
  echo "linux-tray-smoke: FAIL: $*" >&2
  for log in "$out"/*.log; do
    echo "--- $log" >&2
    tail -n 40 "$log" >&2
  done
  exit 1
}

# Runs a gdbus command, logging it and its answer.
bus() {
  echo "\$ gdbus $*" >>"$calls"
  answer=$(LC_ALL=C.UTF-8 gdbus "$@" 2>&1) || {
    echo "$answer" >>"$calls"
    fail "gdbus $*"
  }
  echo "$answer" >>"$calls"
  echo "$answer"
}

# Waits up to 10 s for a command to succeed.
until_ok() {
  i=0
  until "$@" >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -lt 100 ] || return 1
    sleep 0.1
  done
}

pids=""
cleanup() {
  for pid in $pids; do kill "$pid" 2>/dev/null || true; done
  rm -rf "${data:-}"
}
trap cleanup EXIT

python3 scripts/mock-sni-watcher.py >"$out/watcher.log" 2>&1 &
pids="$pids $!"
until_ok grep -q running "$out/watcher.log" || fail "the mock watcher did not start"

# Two open todos, so the tray menu lists them.
data=$(mktemp -d)
mkdir -p "$data/Prism"
printf '1\t0\tBuy milk\n2\t0\tWrite the tray proof' >"$data/Prism/todos"

(cd examples/counter && XDG_DATA_HOME="$data" exec "$bin") >"$out/app.log" 2>&1 &
app=$!
pids="$pids $app"

until_ok grep -q "registered org.kde.StatusNotifierItem-" "$out/watcher.log" ||
  fail "the tray did not register with the watcher"
item=$(sed -n 's/.*registered \(org\.kde\.StatusNotifierItem-[0-9-]*\).*/\1/p' "$out/watcher.log" | head -n 1)
echo "linux-tray-smoke: $item registered"

# Signals from the item, for the checks below.
gdbus monitor --session --dest "$item" >"$out/signals.log" 2>&1 &
pids="$pids $!"

sni="--session --dest $item --object-path /StatusNotifierItem"
menu="--session --dest $item --object-path /MenuBar"

# Properties.
bus introspect $sni >/dev/null
props=$(bus call $sni --method org.freedesktop.DBus.Properties.GetAll org.kde.StatusNotifierItem)
for want in "'Id': <'prism-todos'>" "'Title': <'2'>" "'Status': <'Active'>" \
  "'Menu': <objectpath '/MenuBar'>" "'ItemIsMenu': <false>" "'2 open todos'"; do
  case $props in *"$want"*) ;; *) fail "properties lack $want" ;; esac
done
case $props in *"'IconName': <''>"*) fail "no icon name" ;; esac
echo "linux-tray-smoke: properties OK"

# The menu.
layout=$(bus call $menu --method com.canonical.dbusmenu.GetLayout -- 0 -1 '[]')
for want in "'label': <'2 open todos'>" "'enabled': <false>" "'label': <'Buy milk'>" \
  "'label': <'Write the tray proof'>" "'type': <'separator'>"; do
  case $layout in *"$want"*) ;; *) fail "menu lacks $want" ;; esac
done
id=$(echo "$layout" | grep -o "<([0-9]*, {'label': <'Buy milk'>" | grep -o '[0-9][0-9]*' | head -n 1)
[ -n "$id" ] || fail "no id for 'Buy milk'"
echo "linux-tray-smoke: menu OK ('Buy milk' is item $id)"

# A click on "Buy milk": the app marks it done and updates the tray.
bus call $menu --method com.canonical.dbusmenu.Event -- "$id" clicked '<int32 0>' 0 >/dev/null
title_is_1() {
  gdbus call $sni --method org.freedesktop.DBus.Properties.Get org.kde.StatusNotifierItem Title |
    grep -q "<'1'>"
}
until_ok title_is_1 || fail "the click on 'Buy milk' did not reach the app (title still 2)"
bus call $sni --method org.freedesktop.DBus.Properties.Get org.kde.StatusNotifierItem Title >/dev/null
layout=$(bus call $menu --method com.canonical.dbusmenu.GetLayout -- 0 -1 '[]')
case $layout in *"'Buy milk'"*) fail "'Buy milk' still in the menu" ;; esac
case $layout in *"'1 open todo'"*) ;; *) fail "the menu does not say 1 open todo" ;; esac
until_ok grep -q "LayoutUpdated" "$out/signals.log" || fail "no LayoutUpdated signal"
grep -q "NewTitle" "$out/signals.log" || fail "no NewTitle signal"
grep -q "NewToolTip" "$out/signals.log" || fail "no NewToolTip signal"
echo "linux-tray-smoke: menu click OK (todo done, tray says 1, signals sent)"

# A click on the icon brings the window forward.
python3 scripts/smoke/x11-focus.py clear
before=$(python3 scripts/smoke/x11-focus.py get)
echo "X focus before Activate: $before" >>"$calls"
[ "${before%% *}" = 0x0 ] || fail "could not clear the X focus ($before)"
# Control: nothing else takes the focus back.
sleep 1
idle=$(python3 scripts/smoke/x11-focus.py get)
echo "X focus 1 s later, no Activate: $idle" >>"$calls"
[ "${idle%% *}" = 0x0 ] || fail "the focus moved without Activate ($idle)"
# The counter logs its lifecycle: inactive now, active after the click.
lifecycle() { grep "app is now" "$out/app.log" | tail -n 1 | grep -q "now $1\$"; }
until_ok lifecycle Inactive || fail "the app did not become inactive"
bus call $sni --method org.kde.StatusNotifierItem.Activate 0 0 >/dev/null
focused() { [ "$(python3 scripts/smoke/x11-focus.py get | cut -d' ' -f1)" != 0x0 ]; }
until_ok focused || fail "Activate did not bring the window forward"
until_ok lifecycle Active || fail "the app did not become active"
after=$(python3 scripts/smoke/x11-focus.py get)
echo "X focus after Activate: $after" >>"$calls"
echo "linux-tray-smoke: Activate OK (X focus: $after)"

# The panel shows the menu itself.
if gdbus call $sni --method org.kde.StatusNotifierItem.ContextMenu 0 0 >>"$calls" 2>&1; then
  fail "ContextMenu should be refused"
fi

# Quitting takes the item off the bus.
kill "$app"
wait "$app" 2>/dev/null || true
until_ok grep -q "unregistered $item" "$out/watcher.log" || fail "the item did not leave the watcher"
echo "linux-tray-smoke: unregistered on quit OK"

if grep -E "panicked|CRITICAL" "$out/app.log" >&2; then
  fail "the app logged errors (above)"
fi
echo "linux-tray-smoke: OK"
