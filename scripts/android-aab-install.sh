#!/bin/bash
# Security review S36: `prism run android --aab` installs the bundle's split
# APKs signed with the SDK's debug key (whose password is public); the store
# key's passwords only reach jarsigner through the environment, never a
# command line or a file.
#
# Makes a throwaway store key with random passwords (never printed), builds
# and installs examples/counter as a debug App Bundle on the device while
# sampling every command line (ps) for the passwords, then searches the files
# the build wrote for them, and checks which key signed the bundle and the
# installed APKs.
#
#   scripts/android-aab-install.sh [emulator-5554]
#
# Needs ANDROID_HOME (mise.toml sets it) and target/debug/prism.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
serial=${1:-emulator-5554}
export ANDROID_HOME=${ANDROID_HOME:-$HOME/Library/Android/sdk}
tools=$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)
work=$(mktemp -d)
watch=
trap 'kill $watch 2>/dev/null || true; rm -rf "$work"' EXIT

# A JKS store: its key can have a password of its own (PKCS12 would reuse the store's).
PRISM_ANDROID_KEYSTORE_PASS=$(openssl rand -hex 16)
PRISM_ANDROID_KEY_PASS=$(openssl rand -hex 16)
export PRISM_ANDROID_KEYSTORE_PASS PRISM_ANDROID_KEY_PASS
export PRISM_ANDROID_KEY_ALIAS=s36-upload PRISM_ANDROID_KEYSTORE="$work/s36-upload.jks"
keytool -genkeypair -keystore "$PRISM_ANDROID_KEYSTORE" -storetype JKS -alias s36-upload \
  -storepass:env PRISM_ANDROID_KEYSTORE_PASS -keypass:env PRISM_ANDROID_KEY_PASS \
  -keyalg RSA -keysize 2048 -validity 2 -dname "CN=S36 throwaway upload key" 2>/dev/null
echo "store key: $PRISM_ANDROID_KEYSTORE (JKS, alias s36-upload, random store and key passwords)"

touch "$work/start"
sleep 1
# Every command line, every 0.1 s, while the build runs.
(while :; do ps -axww -o command= >>"$work/ps.txt"; sleep 0.1; done) &
watch=$!
status=0
(cd "$repo/examples/counter" && exec "$repo/target/debug/prism" run android --aab --device "$serial") \
  >"$work/build.log" 2>&1 || status=$?
kill $watch
wait $watch 2>/dev/null || true
watch=
grep -E "^Built|Installed|error" "$work/build.log" || true
[ "$status" -eq 0 ] || { tail -20 "$work/build.log"; echo "android-aab-install: FAIL: the build exited with $status"; exit 1; }

failed=0
samples=$(grep -c . "$work/ps.txt")
seen() { grep -F -e "$PRISM_ANDROID_KEYSTORE_PASS" -e "$PRISM_ANDROID_KEY_PASS" "$@"; }
if seen -q "$work/ps.txt"; then
  echo "FAIL a command line held a store password"
  failed=1
else
  echo "ok   no command line held a store password ($samples command lines sampled)"
fi
echo "     the bundle was signed with: $(grep -o "jarsigner -keystore [^ ]* -storepass[^ ]* [^ ]* -keypass[^ ]* [^ ]*" "$work/ps.txt" | sort -u | sed "s|$work|<tmp>|" | head -1)"
echo "     the device's APKs were signed with: $(grep -o -- "--ks=[^ ]*debug.keystore --ks-pass=[^ ]* --key-pass=[^ ]*" "$work/ps.txt" | sort -u | sed "s|$HOME|~|" | head -1)"
# The files the build wrote (its output, the extracted scripts, temporary
# folders, the SDK's settings).
written=$(find "$repo/target/" "${TMPDIR:-/tmp}" /tmp "$HOME/.android" -newer "$work/start" -type f \
  -not -path "$work/*" 2>/dev/null | wc -l | tr -d ' ')
leaks=$(find "$repo/target/" "${TMPDIR:-/tmp}" /tmp "$HOME/.android" -newer "$work/start" -type f \
  -not -path "$work/*" -print0 2>/dev/null | xargs -0 grep -l -F -e "$PRISM_ANDROID_KEYSTORE_PASS" -e "$PRISM_ANDROID_KEY_PASS" 2>/dev/null || true)
if [ -n "$leaks" ]; then
  echo "FAIL files hold a store password:"
  echo "$leaks"
  failed=1
else
  echo "ok   none of the $written files the build wrote holds a store password"
fi

aab=$(sed -n 's/^Built \(.*\.aab\) .*/\1/p' "$work/build.log")
signer=$(keytool -J-Duser.language=en -printcert -jarfile "$aab" | sed -n 's/^Owner: //p' | head -1)
echo "     the bundle ($(basename "$aab")) is signed by: $signer"
[ "$signer" = "CN=S36 throwaway upload key" ] || { echo "FAIL the bundle is not signed with the store key"; failed=1; }
adb -s "$serial" shell pm path dev.prism.counter | sed 's|^package:.*/|     installed: |'
mkdir "$work/apks"
for p in $(adb -s "$serial" shell pm path dev.prism.counter | sed 's/^package://' | tr -d '\r'); do
  adb -s "$serial" pull "$p" "$work/apks/" >/dev/null
done
for apk in "$work/apks"/*.apk; do
  dn=$("$tools/apksigner" verify --print-certs "$apk" | sed -n 's/^.*Signer.*certificate DN: //p' | head -1)
  echo "     $(basename "$apk"): $dn"
  [ "$dn" = "CN=Android Debug, O=Android, C=US" ] || { echo "FAIL $(basename "$apk") is not signed with the debug key"; failed=1; }
done
[ "$(ls "$work/apks" | wc -l)" -gt 1 ] || { echo "FAIL not split APKs: the bundle was not installed"; failed=1; }
[ "$failed" -eq 0 ] || exit 1
echo "android-aab-install: OK"
