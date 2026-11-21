#!/bin/sh
# Security review S19 on macOS: a widget button's link runs its action only
# with the token the app's own widget snapshot gave it. Any app or web page
# can open the app's links (`open <scheme>:/__prism/widget-action?…`); only
# the app's widgets know the tokens.
#
# Needs the counter's bundle running (`prism run macos --bundle` in
# examples/counter, or target/macos/Counter.app/Contents/MacOS/counter):
# its todo widget snapshot (the App Group container) holds the « toggle »
# button of todo 1 with its token. Links go through Launch Services to that
# bundle, as a widget's Link does; the todo's state is read from the app's
# storage (`todos`: id, done, title).
#
#   scripts/macos-widget-link.sh [path/to/Counter.app]
set -eu
app=${1:-target/macos/Counter.app}
app="$(cd "$(dirname "$app")" && pwd)/$(basename "$app")"
scheme=$(/usr/libexec/PlistBuddy -c "Print :CFBundleURLTypes:0:CFBundleURLSchemes:0" "$app/Contents/Info.plist")
group=$(/usr/libexec/PlistBuddy -c "Print :PrismAppGroup" "$app/Contents/Info.plist")
id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$app/Contents/Info.plist")
snap="$HOME/Library/Group Containers/$group/prism-widgets/todo.json"
store="$HOME/Library/Application Support/$id/todos"
key="$HOME/Library/Application Support/$id/widget-link-key"
pgrep -f "$(basename "$app")/Contents/MacOS/" >/dev/null || { echo "macos-widget-link: start the app first"; exit 1; }
echo "key: $(stat -f '%Sp %z bytes' "$key") ($key)"

# The button the widget draws: kind, action, payload, token.
eval "$(python3 -c '
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
' "$snap")"
[ -n "$token" ] || { echo "FAIL the widget snapshot has no token"; exit 1; }
echo "widget button: kind=$kind action=$action payload=$payload token=${token%"${token#??????}"}… (${#token} hex digits)"

done_of() { awk -F '\t' -v id="$1" '$1 == id { print $2 }' "$store"; }
failed=0
send() { # what, query, expected change (yes/no)
  before=$(done_of "$payload")
  open -a "$app" "$scheme:/__prism/widget-action?$2"
  sleep 2
  after=$(done_of "$payload")
  if [ "$before" != "$after" ]; then changed=yes; else changed=no; fi
  if [ "$changed" = "$3" ]; then ok="ok  "; else ok=FAIL; failed=1; fi
  echo "$ok $1: todo $payload done $before -> $after"
}
base="kind=$kind&action=$action&payload=$payload"
send "no token" "$base" no
send "a made-up token" "$base&token=00000000000000000000000000000000" no
# The counter toggles any kind's todo whose payload parses to its id: these
# would toggle todo 1 if the token did not bind the kind and the payload.
send "the token, on another kind" "kind=todos&action=$action&payload=$payload&token=$token" no
send "the token, payload 0$payload (the same id)" "kind=$kind&action=$action&payload=0$payload&token=$token" no
send "the token, cut short" "$base&token=${token%??}" no
send "the widget's own link" "$base&token=$token" yes
send "the widget's own link again (toggles back)" "$base&token=$token" yes
[ "$failed" -eq 0 ] || exit 1
echo "macos-widget-link: OK"
