#!/bin/sh
# Security review S31 on Android: the UnifiedPush receiver keys are made
# once, private (0600, in the app's files, outside the backed-up
# files/cloud), and a key file that cannot be used is reported, not
# replaced. Needs the emulator as root (`adb root`) and the counter
# installed (`prism run android` in examples/counter: [push] is on, so the
# connector's receiver is in the manifest).
#
#   scripts/android-push-keys.sh setup      # a fake distributor (a manifest
#                                           # alone: nothing answers REGISTER)
#   (tap « Turn on push » in the counter's Components > Push card)
#   scripts/android-push-keys.sh endpoint https://push.example/ep1
#   scripts/android-push-keys.sh keys       # the key file: mode, owner, hash
#   scripts/android-push-keys.sh corrupt    # garbage in the key file
#   scripts/android-push-keys.sh cleanup    # the distributor and keys removed
#
# `endpoint` answers the app's registration as a distributor would
# (NEW_ENDPOINT, with the app's registration token): the counter then shows
# the subscription, or the error.
set -eu
pkg=dev.prism.counter
dist=dev.prism.fakedistributor
files=/data/data/$pkg/files
sdk=${ANDROID_HOME:-$HOME/Library/Android/sdk}
tools=$(ls -d "$sdk"/build-tools/* | sort -V | tail -1)
platform=$(ls -d "$sdk"/platforms/android-* | sort -V | tail -1)

case ${1:-} in
setup)
  dir=$(mktemp -d)
  trap 'rm -rf "$dir"' EXIT
  cat >"$dir/AndroidManifest.xml" <<EOF
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$dist">
  <application android:label="Fake distributor" android:hasCode="false">
    <receiver android:name=".Register" android:exported="true">
      <intent-filter><action android:name="org.unifiedpush.android.distributor.REGISTER"/></intent-filter>
    </receiver>
  </application>
</manifest>
EOF
  "$tools/aapt2" link -o "$dir/unsigned.apk" -I "$platform/android.jar" --manifest "$dir/AndroidManifest.xml" \
    --min-sdk-version 26 --target-sdk-version 34
  "$tools/zipalign" -f 4 "$dir/unsigned.apk" "$dir/aligned.apk"
  # The SDK's debug key, whose password is public.
  "$tools/apksigner" sign --ks "$HOME/.android/debug.keystore" --ks-pass pass:android \
    --out "$dir/dist.apk" "$dir/aligned.apk"
  adb install -r "$dir/dist.apk"
  adb shell am force-stop $pkg
  adb shell rm -f "$files/prism-push-keys" "/data/data/$pkg/shared_prefs/prism_push.xml"
  echo "setup: $dist installed; the counter's push keys and registration removed"
  ;;
endpoint)
  token=$(adb shell cat "/data/data/$pkg/shared_prefs/prism_push.xml" | sed -n 's/.*name="token">\([^<]*\)<.*/\1/p')
  [ -n "$token" ] || { echo "endpoint: no registration token (tap « Turn on push » first)"; exit 1; }
  adb shell am broadcast -a org.unifiedpush.android.connector.NEW_ENDPOINT \
    -n "$pkg/dev.prism.PrismPush\\\$Receiver" --es token "$token" --es endpoint "$2"
  ;;
keys)
  adb shell "ls -lnZ $files/prism-push-keys; stat -c '%a %u:%g %s bytes' $files/prism-push-keys; sha256sum $files/prism-push-keys | cut -c1-16; ls -ldn $files; dumpsys package $pkg | grep -m1 userId="
  ;;
corrupt)
  owner=$(adb shell stat -c %u:%g "$files")
  adb shell "printf 'not a key' > $files/prism-push-keys && chown $owner $files/prism-push-keys && chmod 600 $files/prism-push-keys && restorecon $files/prism-push-keys"
  echo "corrupt: $(adb shell cat "$files/prism-push-keys")"
  ;;
cleanup)
  adb uninstall $dist || true
  adb shell am force-stop $pkg
  adb shell rm -f "$files/prism-push-keys" "/data/data/$pkg/shared_prefs/prism_push.xml"
  echo "cleanup: done"
  ;;
*)
  sed -n '2,19p' "$0"
  exit 2
  ;;
esac
