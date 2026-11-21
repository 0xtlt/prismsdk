#!/bin/sh
# Security review S33 on Linux (GTK): the browser sign-in ends only with its
# own callback. Runs the counter under Xvfb against scripts/mock-oidc.py and
# presses « Sign in with Prism ID » on /settings. The default browser is
# scripts/s33-browser.py (through a desktop entry and mimeapps.list): before
# approving, it sends the app's loopback listener callbacks that are not the
# sign-in's (another path, another state, two states, no state). The app must
# still end signed in with the real one. Secrets go to a fresh gnome-keyring
# on a private session bus.
#
# In the dev image (see docker/linux-dev.Dockerfile), from the repository:
#   docker run --rm -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/linux \
#     prism-linux-dev scripts/linux-sign-in.sh target/linux-sign-in
# Writes sign-in.png, app.log, browser.log and mock-oidc.log to the folder.
set -eu

out=${1:-target/linux-sign-in}
mkdir -p "$out"
abs_out=$(cd "$out" && pwd)
rm -f "$abs_out"/*.png "$abs_out"/*.log
target=${CARGO_TARGET_DIR:-target}
cargo build -p counter
bin="$target/debug/counter"
case $bin in /*) ;; *) bin="$PWD/$bin" ;; esac

home=$(mktemp -d)
oidc=
trap 'kill "$oidc" 2>/dev/null || true; rm -rf "$home"' EXIT
python3 scripts/mock-oidc.py --port 8765 >"$abs_out/mock-oidc.log" 2>&1 &
oidc=$!

# The fake browser, the default for http(s) links.
mkdir -p "$home/data/applications" "$home/data/keyrings" "$home/config"
cat >"$home/data/applications/s33-browser.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=S33 fake browser
Exec=python3 $PWD/scripts/s33-browser.py %u
MimeType=x-scheme-handler/http;x-scheme-handler/https;
NoDisplay=true
EOF
printf '[Default Applications]\nx-scheme-handler/http=s33-browser.desktop\nx-scheme-handler/https=s33-browser.desktop\n' \
  >"$home/config/mimeapps.list"
# The login keyring (made by --unlock below) is the default collection.
printf login >"$home/data/keyrings/default"

export XDG_DATA_HOME="$home/data" XDG_CONFIG_HOME="$home/config" S33_LOG="$abs_out/browser.log"
echo "default browser: $(gio mime x-scheme-handler/http | head -1)"
status=0
(cd examples/counter &&
  PRISM_PATH=/settings PRISM_LOCALE=en PRISM_CLICK="Sign in with Prism ID" \
    PRISM_SNAPSHOT="$abs_out/sign-in.png" PRISM_DELAY=2 PRISM_AFTER=15 \
    timeout 120 dbus-run-session -- sh -c '
      printf %s s33-test | gnome-keyring-daemon --unlock --components=secrets >/dev/null
      exec xvfb-run -a -s "-screen 0 1280x1024x24" "$0"' "$bin") >"$abs_out/app.log" 2>&1 || status=$?

sed 's/^/  app | /' "$abs_out/app.log"
sed 's/^/  browser | /' "$abs_out/browser.log" 2>/dev/null || true
grep -E "register|approve|token|picture" "$abs_out/mock-oidc.log" | sed 's/^/  provider | /' || true
failed=0
[ "$status" -eq 0 ] || { echo "linux-sign-in: FAIL: the app exited with $status"; failed=1; }
test -s "$abs_out/sign-in.png" || { echo "linux-sign-in: FAIL: no snapshot"; failed=1; }
if grep -q "the real callback -> 200" "$abs_out/browser.log" 2>/dev/null &&
  ! grep -E "not the sign-in's: .* -> 200" "$abs_out/browser.log"; then
  echo "ok   the wrong callbacks were refused, the real one was still awaited"
else
  echo "linux-sign-in: FAIL: see the browser's log"
  failed=1
fi
if grep -q "tokens issued" "$abs_out/mock-oidc.log"; then
  echo "ok   the app redeemed the code (PKCE checked by the provider)"
else
  echo "linux-sign-in: FAIL: no tokens issued"
  failed=1
fi
[ "$failed" -eq 0 ] || exit 1
echo "linux-sign-in: OK (look at $out/sign-in.png: signed in)"
