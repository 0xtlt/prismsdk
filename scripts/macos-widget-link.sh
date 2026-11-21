#!/bin/sh
# Security review S19 on macOS: a widget button's link runs its action only
# with a token the app's own widget snapshot gave it, and only once. Any app
# or web page can open the app's links (`open <scheme>:/__prism/widget-action?…`);
# only the app's widgets know the tokens, a link opened once (a log, another
# app that saw it) does nothing the second time, and the links of a drawing
# the app replaced over an hour ago have expired.
#
# Needs the counter's bundle (`prism run macos --bundle` in examples/counter,
# which builds target/macos/Counter.app): its todo widget snapshot (the App
# Group container) holds the « toggle » button of a todo with its token. Links
# go through Launch Services to that bundle, as a widget's Link does; the
# todo's state is read from the app's storage (`todos`: id, done, title). The
# script also quits the app and opens a link with it closed (a tap that
# launches it). It toggles the todo an even number of times, so the todo ends
# as it was.
#
# Start the app from a shell (`target/macos/Counter.app/Contents/MacOS/counter &`):
# macOS refuses an ad-hoc signed app (no team ID) launched by Launch Services
# its App Group container, so it could not write the widget snapshot. The
# script restarts it that way after the tap that launches it.
#
#   scripts/macos-widget-link.sh [path/to/Counter.app]
set -eu
app=${1:-target/macos/Counter.app}
app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")"
scheme=$(/usr/libexec/PlistBuddy -c "Print :CFBundleURLTypes:0:CFBundleURLSchemes:0" "$app/Contents/Info.plist")
group=$(/usr/libexec/PlistBuddy -c "Print :PrismAppGroup" "$app/Contents/Info.plist")
id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$app/Contents/Info.plist")
snap="$HOME/Library/Group Containers/$group/prism-widgets/todo.json"
data="$HOME/Library/Application Support/$id"
store="$data/todos"
key="$data/widget-links/widget-link-key"
ledger="$data/widget-links/widget-link-ledger"
exe=$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$app/Contents/Info.plist")
running() { pgrep -f "$(basename "$app")/Contents/MacOS/" >/dev/null; }
quit() {
  pkill -TERM -f "$(basename "$app")/Contents/MacOS/" || true
  i=0
  while running && [ $i -lt 50 ]; do sleep 0.2; i=$((i + 1)); done
  running && { echo "FAIL the app did not quit"; exit 1; }
  return 0
}
running || { echo "macos-widget-link: start the app first"; exit 1; }
echo "key: $(stat -f '%Sp %z bytes' "$key") ($key)"

# The first button the widget draws: kind, action, payload, token.
button() {
  python3 -c '
import json, sys
found = []
def walk(o):
    if isinstance(o, dict):
        if "wa" in o: found.append(o["wa"])
        for v in o.values(): walk(v)
    elif isinstance(o, list):
        for v in o: walk(v)
walk(json.load(open(sys.argv[1])))
w = found[0]
print("kind=%s action=%s payload=%s token=%s" % (w["k"], w["a"], w["p"], w.get("t", "")))
' "$snap"
}
token_now() { button | sed 's/.*token=//'; }
# Waits (5 s at most) for the widget to be drawn with a token other than $1.
redrawn() {
  i=0
  while [ "$(token_now)" = "$1" ] && [ $i -lt 25 ]; do sleep 0.2; i=$((i + 1)); done
  token_now
}
short() { printf '%s…%s' "$(printf %s "$1" | cut -c1-6)" "$(printf %s "$1" | cut -c17-22)"; }
ledger_summary() {
  python3 -c '
import json, sys
d = json.load(open(sys.argv[1]))
live = [x for x in d["drawings"] if x[1] is None]
print("%d drawings (%d current), %d used links" % (len(d["drawings"]), len(live), len(d["used"])))
' "$ledger"
}

eval "$(button)"
[ -n "$token" ] || { echo "FAIL the widget snapshot has no token"; exit 1; }
echo "widget button: kind=$kind action=$action payload=$payload token=$(short "$token") (${#token} hex digits: drawing id, then MAC)"

done_of() { awk -F '\t' -v id="$1" '$1 == id { print $2 }' "$store"; }
failed=0
send() { # what, query, expected change (yes/no)
  before=$(done_of "$payload")
  open -a "$app" "$scheme:/__prism/widget-action?$2"
  if [ "$3" = yes ]; then
    i=0
    while [ "$(done_of "$payload")" = "$before" ] && [ $i -lt 50 ]; do sleep 0.2; i=$((i + 1)); done
  else
    sleep 2
  fi
  after=$(done_of "$payload")
  if [ "$before" != "$after" ]; then changed=yes; else changed=no; fi
  if [ "$changed" = "$3" ]; then ok="ok  "; else ok=FAIL; failed=1; fi
  echo "$ok $1: todo $payload done $before -> $after"
}
base="kind=$kind&action=$action&payload=$payload"
t0=$token

echo "--- forged links"
send "no token" "$base" no
send "a made-up token" "$base&token=000000000000000000000000000000000000000000000000" no
# The counter toggles any kind's todo whose payload parses to its id: these
# would toggle the todo if the token did not bind the kind and the payload.
send "the token, on another kind" "kind=todos&action=$action&payload=$payload&token=$t0" no
send "the token, payload 0$payload (the same id)" "kind=$kind&action=$action&payload=0$payload&token=$t0" no
send "the token, cut short" "$base&token=${t0%??}" no
other=$(printf '%016x' $(( 0x$(printf %s "$t0" | cut -c1-16) - 1 )))$(printf %s "$t0" | cut -c17-48)
send "the MAC with another drawing's id" "$base&token=$other" no

echo "--- replays"
send "the widget's own link" "$base&token=$t0" yes
t1=$(redrawn "$t0")
[ "$t1" != "$t0" ] && echo "ok   the job's reload drew the widget anew: token $(short "$t1")" || { echo "FAIL no new token"; failed=1; }
send "the same link again (a replay)" "$base&token=$t0" no
t2=$(redrawn "$t1")
send "the same link a third time" "$base&token=$t0" no
t3=$(redrawn "$t2")
echo "     ledger: $(ledger_summary)"
send "the redrawn widget's link (drawn before two redraws: may still be on screen)" "$base&token=$t1" yes
t4=$(redrawn "$t3")
send "that link again (a replay)" "$base&token=$t1" no
t5=$(redrawn "$t4")

echo "--- expiry"
# An hour later: the ledger's replacement times moved back two hours (the
# app merges the file with what it knows, keeping the earliest time).
python3 -c '
import json, os, sys, time
p = sys.argv[1]
d = json.load(open(p))
for x in d["drawings"]:
    if x[1] is not None: x[1] = int(time.time()) - 7200
open(p + ".proof~", "w").write(json.dumps(d))
os.replace(p + ".proof~", p)
' "$ledger"
echo "     ledger, replaced drawings moved two hours back: $(ledger_summary)"
send "an unused link of a drawing replaced over an hour ago" "$base&token=$t2" no
t6=$(redrawn "$t5")
send "another one" "$base&token=$t3" no
t7=$(redrawn "$t6")
echo "     ledger: $(ledger_summary)"

echo "--- a tap that launches the app"
quit
echo "     the app quit; the widget shows token $(short "$t7")"
send "the widget's link, the app closed (launches it)" "$base&token=$t7" yes
running && echo "ok   the app is running" || { echo "FAIL the app is not running"; failed=1; }
send "that link again (a replay after the restart)" "$base&token=$t7" no
send "the first drawing's link, replayed after the restart" "$base&token=$t0" no

echo "--- the app restarted from the shell (it draws its widgets again)"
quit
"$app/Contents/MacOS/$exe" </dev/null >/dev/null 2>&1 &
t8=$(redrawn "$t7")
[ "$t8" != "$t7" ] && echo "ok   the widget was drawn anew: token $(short "$t8")" || { echo "FAIL no new token"; failed=1; }
send "the link that launched the app, replayed after another restart" "$base&token=$t7" no
t9=$(redrawn "$t8")
send "the widget's current link (a fresh link)" "$base&token=$t9" yes
send "that link again" "$base&token=$t9" no
echo "     ledger: $(ledger_summary)"
[ "$failed" -eq 0 ] || exit 1
echo "macos-widget-link: OK"
