#!/usr/bin/env bash
# Build a Prism app for Android (an APK, without Gradle: the SDK's own tools)
# and install it on the running emulator or a connected device. Used by
# `prism run android` / `prism build android`.
#
#   scripts/android-run.sh <package> [--no-launch]
#
# The app crate's code lives in its library (`prism::android_main!(run)`):
# Android loads it as a shared library.
#
# Environment:
#   ANDROID_HOME       the SDK (mise.toml sets ~/Library/Android/sdk)
#   PRISM_SERIAL       adb serial of the device (default: the only one connected)
#   PRISM_BUNDLE_ID    application id (default dev.prism.<package>)
#   PRISM_APP_NAME     label (default: package name, capitalized)
#   PRISM_VERSION      version name (default 0.1.0)
#   PRISM_ICON         1024×1024 PNG app icon
#   PRISM_PERMISSIONS  Prism.toml [permissions] (name<TAB>text per line)
#   PRISM_HTTP_HOSTS   plain-HTTP hosts, comma-separated (cleartext traffic)
#   PRISM_PATH         first screen (deep link)
#   PRISM_RELEASE=1    release build
#   PRISM_NO_INSTALL=1 build only
#   PRISM_ANDROID_ABI  arm64-v8a (default) or x86_64 (x86 emulators, e.g. CI)
#   PRISM_ANDROID_AUTOMOTIVE=1  a build for cars running Android (default:
#                      when the device is one); car screens need cx.register_car
#   PRISM_PUSH         remote push (Prism.toml [push]): the UnifiedPush receiver
#   PRISM_GOOGLE_SERVICES  Firebase's google-services.json: FCM, with the Firebase
#                      SDK fetched from Google's Maven and bundled (opt-in)
#   PRISM_VAPID_PUBLIC_KEY  given to UnifiedPush distributors that require VAPID
#   PRISM_ANDROID_KEYSTORE, PRISM_ANDROID_KEYSTORE_PASS, PRISM_ANDROID_KEY_ALIAS
#                      signing key for store builds (default: the debug key);
#                      PRISM_ANDROID_KEY_PASS if the key's password differs
#   PRISM_BUILD        versionCode (default 1)
#   PRISM_ANDROID_AAB=1  an Android App Bundle for Google Play (bundletool,
#                      signed with jarsigner) instead of an APK; installing it
#                      goes through `bundletool build-apks --local-testing`,
#                      the device's APKs signed with the debug key
set -euo pipefail

PKG="${1:?usage: android-run.sh <package> [--no-launch]}"
LAUNCH=1
[[ "${2:-}" == "--no-launch" ]] && LAUNCH=0

ROOT="${PRISM_PROJECT:-$(cd "$(dirname "$0")/.." && pwd)}"
PLATFORM="${PRISM_PLATFORM:-$ROOT/platform}"
cd "$ROOT"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK (mise.toml does)}"
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

MIN_SDK=29
PLATFORM_DIR="$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -1)"
TARGET_SDK="${PLATFORM_DIR##*-}"
ANDROID_JAR="$PLATFORM_DIR/android.jar"
BUILD_TOOLS="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
ADB="$ANDROID_HOME/platform-tools/adb"

SLUG="${PKG//_/-}"
LIB="${PKG//-/_}"
APP_ID="${PRISM_BUNDLE_ID:-dev.prism.${LIB}}"
APP_NAME="${PRISM_APP_NAME:-$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}}"
SCHEME="prism-$SLUG"
AAB="${PRISM_ANDROID_AAB:-}"
VERSION_CODE="${PRISM_BUILD:-1}"
[[ "$VERSION_CODE" =~ ^[1-9][0-9]{0,9}$ ]] && (( VERSION_CODE <= 2100000000 )) \
  || { echo "versionCode must be a whole number from 1 to 2100000000 (Prism.toml [app] build), not $VERSION_CODE" >&2; exit 1; }
PROFILE_DIR=debug CARGO_FLAGS=()
if [[ "${PRISM_RELEASE:-}" == 1 ]]; then PROFILE_DIR=release CARGO_FLAGS=(--release); fi
OUT="$TARGET_DIR/android/$PROFILE_DIR"
STAGE="$OUT/stage"
ABI="${PRISM_ANDROID_ABI:-arm64-v8a}"
case "$ABI" in
  arm64-v8a) TRIPLE=aarch64-linux-android ;;
  x86_64) TRIPLE=x86_64-linux-android ;;
  *) echo "PRISM_ANDROID_ABI: arm64-v8a or x86_64, not $ABI" >&2; exit 1 ;;
esac
rm -rf "$STAGE" && mkdir -p "$STAGE/lib/$ABI" "$OUT/classes" "$STAGE/assets"

# 1. The app as a shared library (arm64: phones and Apple Silicon emulators).
cargo ndk -t "$ABI" -P "$MIN_SDK" rustc -p "$PKG" --lib --crate-type cdylib ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
cp "$TARGET_DIR/$TRIPLE/$PROFILE_DIR/lib$LIB.so" "$STAGE/lib/$ABI/"

# 2. What the app declares, asked once of a build for this computer
#    (PRISM_DESCRIBE=all): assets (picture("logo.png")), widgets, car screen.
#    Prism's native backend can stay off there (docs: « What the bundlers ask
#    the app »).
cargo build -q -p "$PKG" || {
  echo "prism: building $PKG for this computer failed. The bundler runs that build to ask the app what it declares (PRISM_DESCRIBE): it must compile here, with Prism's native backend off if the app enables it only for Android (see « What the bundlers ask the app » in the docs)." >&2
  exit 1
}
HOST_BIN="$TARGET_DIR/debug/$PKG"
GEN="$OUT/gen-describe"
mkdir -p "$GEN"
PRISM_DESCRIBE=all "$HOST_BIN" > "$GEN/describe.json"
describe() { # $1: a key of describe.json (lists as JSON, flags as 1/0)
  python3 -c '
import json, sys
v = json.load(open(sys.argv[1]))[sys.argv[2]]
print(int(v) if isinstance(v, bool) else v if isinstance(v, str) else json.dumps(v))
' "$GEN/describe.json" "$1"
}
ASSETS="$(describe assets)"
[[ -n "$ASSETS" && -d "$ASSETS" ]] && cp -R "$ASSETS"/. "$STAGE/assets/"
rm -rf "$OUT/res" "$OUT/gen" && mkdir -p "$OUT/res/layout" "$OUT/gen"
# `[network] allow_http` (PRISM_HTTP_HOSTS): cleartext traffic to these hosts
# and their subdomains only (Android blocks it since API 28).
NET_ATTR=""
if [[ -n "${PRISM_HTTP_HOSTS:-}" ]]; then
  mkdir -p "$OUT/res/xml"
  {
    echo '<network-security-config><domain-config cleartextTrafficPermitted="true">'
    IFS=, read -ra HTTP_HOSTS <<< "$PRISM_HTTP_HOSTS"
    for h in "${HTTP_HOSTS[@]}"; do echo "  <domain includeSubdomains=\"true\">$h</domain>"; done
    echo '</domain-config></network-security-config>'
  } > "$OUT/res/xml/prism_network_security.xml"
  NET_ATTR='android:networkSecurityConfig="@xml/prism_network_security"'
fi
# Each widget kind: a provider subclass, its size (appwidget-provider XML) and
# a manifest receiver.
RECEIVERS="$(describe widgets | OUT="$OUT" python3 -c '
import json, os, re, sys
from xml.sax.saxutils import escape
out = os.environ["OUT"]
try:
    widgets = json.load(sys.stdin)
except ValueError:
    widgets = []
# Smallest family first: its size in dp and launcher cells.
sizes = [("systemSmall", 110, 110, 2, 2), ("systemMedium", 250, 110, 4, 2),
         ("systemLarge", 250, 250, 4, 4), ("systemExtraLarge", 500, 250, 6, 4)]
strings = []
for w in widgets:
    fams = [s for s in sizes if s[0] in w.get("families", [])]
    if not fams:
        continue
    _, mw, mh, cw, ch = fams[0]
    name = re.sub(r"[^A-Za-z0-9_]", "_", w["kind"])
    pkg_dir = os.path.join(out, "gen/dev/prism/widgets")
    os.makedirs(pkg_dir, exist_ok=True)
    with open(os.path.join(pkg_dir, f"Widget_{name}.java"), "w") as f:
        kind = w["kind"]
        f.write(f"package dev.prism.widgets;\n\n/** The {kind} widget. */\npublic final class Widget_{name} extends dev.prism.PrismWidget {{}}\n")
    os.makedirs(os.path.join(out, "res/xml"), exist_ok=True)
    with open(os.path.join(out, f"res/xml/prism_widget_{name.lower()}.xml"), "w") as f:
        f.write(f"""<appwidget-provider xmlns:android="http://schemas.android.com/apk/res/android"
    android:minWidth="{mw}dp" android:minHeight="{mh}dp"
    android:targetCellWidth="{cw}" android:targetCellHeight="{ch}"
    android:resizeMode="horizontal|vertical" android:widgetCategory="home_screen"
    android:description="@string/prism_widget_{name.lower()}" android:updatePeriodMillis="0"/>
""")
    strings.append(f"""  <string name="prism_widget_{name.lower()}">{escape(w.get("description", ""))}</string>""")
    print(f"""    <receiver android:name="dev.prism.widgets.Widget_{name}" android:exported="false"
        android:label="{escape(w.get("name", w["kind"]), {chr(34): "&quot;"})}">
      <intent-filter><action android:name="android.appwidget.action.APPWIDGET_UPDATE"/></intent-filter>
      <meta-data android:name="android.appwidget.provider" android:resource="@xml/prism_widget_{name.lower()}"/>
    </receiver>""")
if strings:
    # Widget buttons (widget_action): their broadcasts run the app in place.
    print("""    <receiver android:name="dev.prism.PrismWidget$Action" android:exported="false"/>""")
    os.makedirs(os.path.join(out, "res/values"), exist_ok=True)
    with open(os.path.join(out, "res/values/prism_widgets.xml"), "w") as f:
        f.write("<resources>\n" + "\n".join(strings) + "\n</resources>\n")
')"

# 3. The car (cx.register_car): Google's Car App Library, fetched from Maven
#    (android-deps.py) for Android Auto; for cars running Android, its
#    Automotive part too (PRISM_ANDROID_AUTOMOTIVE=1, or such a device).
CAR="$(describe car)"
SERIAL="${PRISM_SERIAL:-$("$ADB" devices 2>/dev/null | awk 'NR>1 && $2=="device" {print $1; exit}')}"
AUTOMOTIVE="${PRISM_ANDROID_AUTOMOTIVE:-}"
if [[ -z "$AUTOMOTIVE" && -n "$SERIAL" && "${PRISM_NO_INSTALL:-}" != 1 ]]; then
  "$ADB" -s "$SERIAL" shell pm has-feature android.hardware.type.automotive 2>/dev/null | grep -q true && AUTOMOTIVE=1
fi
# Remote push (PRISM_PUSH): UnifiedPush needs no library. FCM
# (PRISM_GOOGLE_SERVICES) needs Google's Firebase SDK: firebase-messaging and
# its dependencies (Play services client libraries, Kotlin…), fetched like the
# car library. Their manifests are merged below (android-manifest-merge.py).
GSERVICES="${PRISM_GOOGLE_SERVICES:-}"
[[ -n "$GSERVICES" && ! -f "$GSERVICES" ]] && { echo "PRISM_GOOGLE_SERVICES: $GSERVICES not found" >&2; exit 1; }
[[ -n "$GSERVICES" ]] && PRISM_PUSH="${PRISM_PUSH:-1}"
LIB_JARS="" LIB_RES=() LIB_PKGS="" ROOTS=() FCM_DIRS=()
if [[ "$CAR" == 1 ]]; then
  ROOTS+=(androidx.car.app:app:1.7.0)
  [[ "$AUTOMOTIVE" == 1 ]] && ROOTS+=(androidx.car.app:app-automotive:1.7.0)
  mkdir -p "$OUT/res/xml"
  printf '<automotiveApp><uses name="template"/></automotiveApp>\n' > "$OUT/res/xml/automotive_app_desc.xml"
fi
[[ -n "$GSERVICES" ]] && ROOTS+=(com.google.firebase:firebase-messaging:26.0.2)
if [[ ${#ROOTS[@]} -gt 0 ]]; then
  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/prism/maven"
  mkdir -p "$OUT/libres"
  while IFS=$'\t' read -r kind a b; do
    case $kind in
      jar) LIB_JARS="$LIB_JARS:$a" ;;
      res)
        zip="$OUT/libres/${b//./_}.zip"
        "$BUILD_TOOLS/aapt2" compile --dir "$a" -o "$zip"
        LIB_RES+=(-R "$zip") LIB_PKGS="${LIB_PKGS:+$LIB_PKGS:}$b" ;;
      aar) FCM_DIRS+=("$a") ;;
    esac
  done < <(python3 "$(dirname "$0")/android-deps.py" "$CACHE" "${ROOTS[@]}" | python3 -c '
import json, os, sys
for e in json.load(sys.stdin):
    for j in e["jars"]:
        print(f"jar\t{j}\t")
    res, package = e["res"], e["package"]
    if res and package:
        print(f"res\t{res}\t{package}")
    # Firebase / Play services manifests (components, permissions) to merge.
    if package and e["id"].startswith(("com.google.firebase:", "com.google.android.gms:", "com.google.android.datatransport:")) and e["jars"]:
        first = e["jars"][0]
        print("aar\t" + os.path.dirname(first) + "\t")
')
fi
# google-services.json → the resources Firebase reads at startup (what the
# google-services Gradle plugin generates).
if [[ -n "$GSERVICES" ]]; then
  mkdir -p "$OUT/res/values"
  python3 - "$GSERVICES" "$APP_ID" > "$OUT/res/values/google_services.xml" <<'PY'
import json, sys
from xml.sax.saxutils import escape
g, app_id = json.load(open(sys.argv[1])), sys.argv[2]
clients = [c for c in g.get("client", []) if c["client_info"]["android_client_info"]["package_name"] == app_id]
if not clients:
    sys.exit(f"google-services.json has no Android app for {app_id}")
c, p = clients[0], g["project_info"]
values = {
    "google_app_id": c["client_info"]["mobilesdk_app_id"],
    "gcm_defaultSenderId": p["project_number"],
    "google_api_key": c["api_key"][0]["current_key"],
    "google_crash_reporting_api_key": c["api_key"][0]["current_key"],
    "project_id": p["project_id"],
}
if p.get("storage_bucket"):
    values["google_storage_bucket"] = p["storage_bucket"]
print("<resources>")
for k, v in values.items():
    print(f'  <string name="{k}" translatable="false">{escape(str(v))}</string>')
print("</resources>")
PY
fi

# 4. Manifest: permissions from Prism.toml, deep links, the file provider.
PERMS="$(python3 -c '
import os
keys = {
    "location": ["ACCESS_FINE_LOCATION", "ACCESS_COARSE_LOCATION"],
    "location_always": ["ACCESS_BACKGROUND_LOCATION"],
    "camera": ["CAMERA"],
    "microphone": ["RECORD_AUDIO"],
}
# Background jobs survive reboots (JobInfo.setPersisted).
names = ["INTERNET", "VIBRATE", "POST_NOTIFICATIONS", "RECEIVE_BOOT_COMPLETED"]
for line in os.environ.get("PRISM_PERMISSIONS", "").splitlines():
    names += keys.get(line.partition("\t")[0], [])
for n in dict.fromkeys(names):
    print(f"""  <uses-permission android:name="android.permission.{n}"/>""")
')"
ICON_ATTR=""
if [[ -n "${PRISM_ICON:-}" && -f "$PRISM_ICON" ]]; then
  mkdir -p "$OUT/res/mipmap-xxxhdpi" "$OUT/res/drawable"
  cp "$PRISM_ICON" "$OUT/res/mipmap-xxxhdpi/ic_launcher.png"
  # Android TV's launcher shows a 16:9 banner: the icon on a neutral card.
  cat > "$OUT/res/drawable/prism_banner.xml" <<'BANNER'
<layer-list xmlns:android="http://schemas.android.com/apk/res/android">
  <item><shape><size android:width="320dp" android:height="180dp"/><solid android:color="#FF1C1C1E"/></shape></item>
  <item android:width="120dp" android:height="120dp" android:gravity="center" android:drawable="@mipmap/ic_launcher"/>
</layer-list>
BANNER
  ICON_ATTR='android:icon="@mipmap/ic_launcher" android:banner="@drawable/prism_banner"'
fi
# Layouts for widgets and Live Activities (RemoteViews only inflate resources;
# PrismSurface.java nests them): prism_<kind>_<size>.xml.
for kind in col row text chrono progress image space; do
  case $kind in
    col) tag=LinearLayout extra='android:orientation="vertical"' ;;
    row) tag=LinearLayout extra='android:orientation="horizontal"' ;;
    text) tag=TextView extra='' ;;
    chrono) tag=Chronometer extra='' ;;
    progress) tag=ProgressBar extra='style="@android:style/Widget.Material.ProgressBar.Horizontal"' ;;
    image) tag=ImageView extra='android:scaleType="fitXY"' ;;
    space) tag=FrameLayout extra='' ;;
  esac
  for size in wrap fill growh growv both; do
    case $size in
      wrap) dims='android:layout_width="wrap_content" android:layout_height="wrap_content"' ;;
      fill) dims='android:layout_width="match_parent" android:layout_height="wrap_content"' ;;
      growh) dims='android:layout_width="0dp" android:layout_height="wrap_content" android:layout_weight="1"' ;;
      growv) dims='android:layout_width="match_parent" android:layout_height="0dp" android:layout_weight="1"' ;;
      both) dims='android:layout_width="match_parent" android:layout_height="match_parent"' ;;
    esac
    printf '<%s xmlns:android="http://schemas.android.com/apk/res/android" android:id="@+id/prism_node" %s %s/>\n' \
      "$tag" "$dims" "$extra" > "$OUT/res/layout/prism_${kind}_${size}.xml"
  done
done
"$BUILD_TOOLS/aapt2" compile --dir "$OUT/res" -o "$OUT/res.zip"
RES_ARGS=("$OUT/res.zip")
# The phone app's launcher entry; a car's launcher opens the car screens instead.
LAUNCHER='      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
        <category android:name="android.intent.category.LEANBACK_LAUNCHER"/>
      </intent-filter>'
CAR_FEATURES="" CAR_APP=""
if [[ "$CAR" == 1 ]]; then
  # The libraries' own manifest entries (Gradle would merge them).
  CAR_FEATURES='  <queries>
    <provider android:name="androidx.car.app.connection.provider" android:authorities="androidx.car.app.connection"/>
    <intent><action android:name="android.car.template.host.RendererService"/></intent>
  </queries>'
  CAR_APP='    <service android:name="dev.prism.PrismCarService" android:exported="true">
      <intent-filter>
        <action android:name="androidx.car.app.CarAppService"/>
        <category android:name="androidx.car.app.category.IOT"/>
      </intent-filter>
    </service>
    <meta-data android:name="androidx.car.app.minCarApiLevel" android:value="1"/>
    <activity android:name="androidx.car.app.CarAppPermissionActivity" android:exported="false"
        android:theme="@android:style/Theme.Translucent.NoTitleBar"/>
    <receiver android:name="androidx.car.app.notification.CarAppNotificationBroadcastReceiver" android:exported="false"/>'
  if [[ "$AUTOMOTIVE" == 1 ]]; then
    CAR_FEATURES="$CAR_FEATURES
  <uses-feature android:name=\"android.hardware.type.automotive\" android:required=\"true\"/>
  <uses-feature android:name=\"android.software.car.templates_host\" android:required=\"true\"/>"
    LAUNCHER=""
    CAR_APP="$CAR_APP
    <meta-data android:name=\"com.android.automotive\" android:resource=\"@xml/automotive_app_desc\"/>
    <activity android:name=\"androidx.car.app.activity.CarAppActivity\" android:exported=\"true\"
        android:theme=\"@android:style/Theme.DeviceDefault.NoActionBar\" android:launchMode=\"singleTask\" android:label=\"$APP_NAME\">
      <intent-filter>
        <action android:name=\"android.intent.action.MAIN\"/>
        <category android:name=\"android.intent.category.LAUNCHER\"/>
      </intent-filter>
      <meta-data android:name=\"distractionOptimized\" android:value=\"true\"/>
    </activity>
    <service android:name=\"androidx.car.app.CarAppMetadataHolderService\" android:enabled=\"false\" android:exported=\"false\">
      <meta-data android:name=\"androidx.car.app.CarAppMetadataHolderService.CAR_HARDWARE_MANAGER\"
          android:value=\"androidx.car.app.hardware.AutomotiveCarHardwareManager\"/>
      <meta-data android:name=\"androidx.car.app.CarAppMetadataHolderService.RESULT_MANAGER\"
          android:value=\"androidx.car.app.activity.ResultManagerAutomotive\"/>
    </service>"
  else
    CAR_APP="$CAR_APP
    <meta-data android:name=\"com.google.android.gms.car.application\" android:resource=\"@xml/automotive_app_desc\"/>"
  fi
fi
# Remote push: the UnifiedPush connector (PrismPush.Receiver), FCM's service
# and the Firebase libraries' own entries.
PUSH_PERMS="" PUSH_QUERIES="" PUSH_APP=""
if [[ -n "${PRISM_PUSH:-}" ]]; then
  # Finds distributors (Android 11+ package visibility).
  PUSH_QUERIES='  <queries><intent><action android:name="org.unifiedpush.android.distributor.REGISTER"/></intent></queries>'
  PUSH_APP='    <receiver android:name="dev.prism.PrismPush$Receiver" android:exported="true">
      <intent-filter>
        <action android:name="org.unifiedpush.android.connector.MESSAGE"/>
        <action android:name="org.unifiedpush.android.connector.NEW_ENDPOINT"/>
        <action android:name="org.unifiedpush.android.connector.REGISTRATION_FAILED"/>
        <action android:name="org.unifiedpush.android.connector.UNREGISTERED"/>
        <action android:name="org.unifiedpush.android.connector.TEMP_UNAVAILABLE"/>
      </intent-filter>
    </receiver>'
  [[ -n "${PRISM_VAPID_PUBLIC_KEY:-}" ]] && PUSH_APP="$PUSH_APP
    <meta-data android:name=\"prism.vapid\" android:value=\"$PRISM_VAPID_PUBLIC_KEY\"/>"
fi
if [[ -n "$GSERVICES" ]]; then
  MERGED="$(python3 "$(dirname "$0")/android-manifest-merge.py" "$APP_ID" "${FCM_DIRS[@]}")"
  PUSH_PERMS="$(sed '/^--$/,$d' <<< "$MERGED")"
  PUSH_APP="$PUSH_APP
    <service android:name=\"dev.prism.PrismFcm\" android:exported=\"false\">
      <intent-filter><action android:name=\"com.google.firebase.MESSAGING_EVENT\"/></intent-filter>
    </service>
    <!-- A token only when the app calls register_for_push. -->
    <meta-data android:name=\"firebase_messaging_auto_init_enabled\" android:value=\"false\"/>
$(sed '1,/^--$/d' <<< "$MERGED")"
fi
cat > "$OUT/AndroidManifest.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$APP_ID">
$PERMS
$PUSH_PERMS
$CAR_FEATURES
$PUSH_QUERIES
  <!-- Also for TVs (no touch screen) and watches. -->
  <uses-feature android:name="android.hardware.touchscreen" android:required="false"/>
  <uses-feature android:name="android.software.leanback" android:required="false"/>
  <uses-feature android:name="android.hardware.type.watch" android:required="false"/>
  <!-- BiometricPrompt (normal permission, granted at install). -->
  <uses-permission android:name="android.permission.USE_BIOMETRIC"/>
  <!-- Package visibility (Android 11+): browsers with Custom Tabs, for sign-in. -->
  <queries>
    <intent><action android:name="android.support.customtabs.action.CustomTabsService"/></intent>
    <intent>
      <action android:name="android.intent.action.VIEW"/>
      <category android:name="android.intent.category.BROWSABLE"/>
      <data android:scheme="https"/>
    </intent>
  </queries>
  <!-- cloud_storage: key/value backup (PrismBackup); secrets are not backed up. -->
  <application android:label="$APP_NAME" $ICON_ATTR $NET_ATTR android:extractNativeLibs="false" android:supportsRtl="true"
      android:theme="@android:style/Theme.DeviceDefault.DayNight"
      android:allowBackup="true" android:backupAgent="dev.prism.PrismBackup" android:restoreAnyVersion="true">
    <activity android:name="dev.prism.PrismActivity" android:exported="true" android:launchMode="singleTop"
        android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density|fontScale|locale|layoutDirection">
      <meta-data android:name="prism.lib" android:value="$LIB"/>
      <meta-data android:name="prism.scheme" android:value="$SCHEME"/>
$LAUNCHER
    </activity>
    <!-- Deep links and sign-in redirects, handed to the running PrismActivity. -->
    <activity android:name="dev.prism.PrismLinkActivity" android:exported="true" android:noHistory="true"
        android:excludeFromRecents="true" android:theme="@android:style/Theme.Translucent.NoTitleBar">
      <intent-filter>
        <action android:name="android.intent.action.VIEW"/>
        <category android:name="android.intent.category.DEFAULT"/>
        <category android:name="android.intent.category.BROWSABLE"/>
        <data android:scheme="$SCHEME"/>
      </intent-filter>
    </activity>
    <!-- Wear OS: runs without a phone. -->
    <meta-data android:name="com.google.android.wearable.standalone" android:value="true"/>
$RECEIVERS
$CAR_APP
$PUSH_APP
    <service android:name="dev.prism.PrismJob" android:exported="false"
        android:permission="android.permission.BIND_JOB_SERVICE"/>
    <provider android:name="dev.prism.PrismFiles" android:authorities="$APP_ID.prism.files"
        android:exported="false" android:grantUriPermissions="true"/>
  </application>
</manifest>
XML

# 5. Link resources (the libraries' too, with their R classes), then the Java
#    host, widget providers and libraries → dex. App bundles keep the manifest
#    and resources in protobuf format (bundletool's input).
rm -rf "$OUT/rjava" && mkdir -p "$OUT/rjava"
LINK_FORMAT=()
[[ "$AAB" == 1 ]] && LINK_FORMAT=(--proto-format)
"$BUILD_TOOLS/aapt2" link ${LINK_FORMAT[@]+"${LINK_FORMAT[@]}"} -o "$OUT/base.apk" -I "$ANDROID_JAR" --manifest "$OUT/AndroidManifest.xml" \
  -A "$STAGE/assets" --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
  --version-code "$VERSION_CODE" --version-name "${PRISM_VERSION:-0.1.0}" ${RES_ARGS[@]+"${RES_ARGS[@]}"} \
  ${LIB_RES[@]+"${LIB_RES[@]}"} --auto-add-overlay --java "$OUT/rjava" ${LIB_PKGS:+--extra-packages "$LIB_PKGS"}
rm -rf "$OUT/classes" && mkdir -p "$OUT/classes"
SOURCES=("$PLATFORM/android/java" "$OUT/gen")
[[ "$CAR" == 1 ]] && SOURCES+=("$PLATFORM/android/car" "$OUT/rjava")
[[ -n "$GSERVICES" ]] && SOURCES+=("$PLATFORM/android/fcm")
javac --release 17 -nowarn -classpath "$ANDROID_JAR$LIB_JARS" -d "$OUT/classes" $(find "${SOURCES[@]}" -name '*.java')
"$BUILD_TOOLS/d8" --min-api "$MIN_SDK" --lib "$ANDROID_JAR" --output "$STAGE" \
  $(find "$OUT/classes" -name '*.class') ${LIB_JARS//:/ }

# 6. Signing key: the store key, else the SDK's debug key.
# The SDK's conventional debug key (what Android Studio uses too), made once.
DEBUG_KEYSTORE="$HOME/.android/debug.keystore"
debug_keystore() {
  if [[ ! -f "$DEBUG_KEYSTORE" ]]; then
    mkdir -p "$(dirname "$DEBUG_KEYSTORE")"
    keytool -genkeypair -keystore "$DEBUG_KEYSTORE" -storepass android -keypass android -alias androiddebugkey \
      -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >/dev/null
  fi
}
if [[ -n "${PRISM_ANDROID_KEYSTORE:-}" ]]; then
  KEYSTORE="$PRISM_ANDROID_KEYSTORE" PASS="${PRISM_ANDROID_KEYSTORE_PASS:?set PRISM_ANDROID_KEYSTORE_PASS}"
  KEY_PASS="${PRISM_ANDROID_KEY_PASS:-$PASS}"
  ALIAS="${PRISM_ANDROID_KEY_ALIAS:?set PRISM_ANDROID_KEY_ALIAS}"
else
  KEYSTORE="$DEBUG_KEYSTORE" PASS=android KEY_PASS=android ALIAS=androiddebugkey
  debug_keystore
  [[ "${PRISM_RELEASE:-}" == 1 ]] && echo "note: signed with the debug key; set PRISM_ANDROID_KEYSTORE for a store build" >&2
fi

# The libraries' Java resources (car-app-api.level…), as Gradle packages them.
# Unzipped entries can be read-only: make them removable first.
[ -d "$OUT/javares" ] && chmod -R u+rwX "$OUT/javares"
rm -rf "$OUT/javares"
if [[ -n "$LIB_JARS" ]]; then
  mkdir -p "$OUT/javares"
  for jar in ${LIB_JARS//:/ }; do
    unzip -qo "$jar" -x '*.class' 'META-INF/MANIFEST.MF' 'META-INF/*.SF' 'META-INF/*.RSA' 'META-INF/*.DSA' \
      'META-INF/versions/*' 'module-info.class' -d "$OUT/javares" 2>/dev/null || true
  done
  # Some jars store directories without the search bit: fix them, or their
  # files would be left out.
  chmod -R u+rwX "$OUT/javares"
fi

if [[ "$AAB" == 1 ]]; then
  # 7. App bundle: the base module's zip (manifest/, dex/, res/, resources.pb,
  #    assets/, lib/, root/), bundletool build-bundle, then jarsigner (Play
  #    rejects bundles signed with apksigner or a debug key).
  BUNDLETOOL=(bash "$(dirname "$0")/bundletool.sh")
  MOD="$OUT/module"
  rm -rf "$MOD" "$OUT/base.zip" && mkdir -p "$MOD/manifest" "$MOD/dex"
  (cd "$MOD" && unzip -q "$OUT/base.apk")
  mv "$MOD/AndroidManifest.xml" "$MOD/manifest/AndroidManifest.xml"
  mv "$STAGE"/classes*.dex "$MOD/dex/"
  cp -R "$STAGE/lib" "$MOD/lib"
  if [[ -d "$OUT/javares" ]]; then mkdir -p "$MOD/root" && cp -R "$OUT/javares/." "$MOD/root/"; fi
  (cd "$MOD" && zip -q -r -D "$OUT/base.zip" .)
  # Native libraries stay uncompressed and 16 KB aligned in the APKs Play
  # generates (Android 15+ devices with 16 KB pages).
  cat > "$OUT/BundleConfig.json" <<'JSON'
{
  "optimizations": {
    "uncompressNativeLibraries": { "enabled": true, "alignment": "PAGE_ALIGNMENT_16K" }
  }
}
JSON
  BUNDLE="$OUT/$APP_NAME.aab"
  rm -f "$OUT/unsigned.aab"
  "${BUNDLETOOL[@]}" build-bundle --modules="$OUT/base.zip" --config="$OUT/BundleConfig.json" \
    --output="$OUT/unsigned.aab"
  rm -f "$BUNDLE"
  PRISM_KS_PASS="$PASS" PRISM_KEY_PASS="$KEY_PASS" jarsigner -keystore "$KEYSTORE" \
    -storepass:env PRISM_KS_PASS -keypass:env PRISM_KEY_PASS \
    -sigalg SHA256withRSA -digestalg SHA-256 -signedjar "$BUNDLE" "$OUT/unsigned.aab" "$ALIAS" >/dev/null
  # Every entry signed: the strict check, whose 4 (a self-signed upload key's
  # chain, not validated) is the only failure taken.
  verified=0
  jarsigner -verify -strict "$BUNDLE" >/dev/null 2>&1 || verified=$?
  if (( verified & ~4 )); then
    echo "error: $BUNDLE does not verify (jarsigner -verify -strict: exit $verified)" >&2
    exit 1
  fi
  echo "Built $BUNDLE (versionCode $VERSION_CODE)"
  [[ "${PRISM_NO_INSTALL:-}" == 1 ]] && exit 0

  # 8. Install like Play would: the device's split APKs from the bundle,
  #    signed with the debug key, never the store key: bundletool reads
  #    passwords only from its command line (shown by ps) or from files (left
  #    on disk if the build is killed). Play signs what it delivers with its
  #    own key anyway. A copy signed with another key must be uninstalled.
  [[ -n "$SERIAL" ]] || { echo "No Android device or emulator running (start one: emulator -avd <name>)" >&2; exit 1; }
  rm -f "$OUT/local.apks"
  debug_keystore
  "${BUNDLETOOL[@]}" build-apks --bundle="$BUNDLE" \
    --output="$OUT/local.apks" --local-testing --connected-device --device-id="$SERIAL" \
    --adb="$ADB" --aapt2="$BUILD_TOOLS/aapt2" \
    --ks="$DEBUG_KEYSTORE" --ks-pass=pass:android --key-pass=pass:android --ks-key-alias=androiddebugkey
  "${BUNDLETOOL[@]}" install-apks --apks="$OUT/local.apks" --device-id="$SERIAL" --adb="$ADB"
else
# 7. APK: add code and native libraries, align (16 KB pages), sign.
(cd "$STAGE" && zip -q "$OUT/base.apk" classes*.dex && zip -q -0 -r "$OUT/base.apk" lib)
if [[ -d "$OUT/javares" ]]; then
  (cd "$OUT/javares" && find . -type f | sed 's|^\./||' | zip -q "$OUT/base.apk" -@)
fi
"$BUILD_TOOLS/zipalign" -f -P 16 4 "$OUT/base.apk" "$OUT/aligned.apk"
APK="$OUT/$APP_NAME.apk"
PRISM_KS_PASS="$PASS" PRISM_KEY_PASS="$KEY_PASS" "$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" \
  --ks-pass env:PRISM_KS_PASS --key-pass env:PRISM_KEY_PASS --ks-key-alias "$ALIAS" --out "$APK" "$OUT/aligned.apk"
echo "Built $APK (versionCode $VERSION_CODE)"
[[ "${PRISM_NO_INSTALL:-}" == 1 ]] && exit 0

# 8. Install and launch.
[[ -n "$SERIAL" ]] || { echo "No Android device or emulator running (start one: emulator -avd <name>)" >&2; exit 1; }
"$ADB" -s "$SERIAL" install -r "$APK" >/dev/null
fi
echo "Installed $APP_ID on $SERIAL"
if [[ $LAUNCH == 1 ]]; then
  if [[ -n "${PRISM_PATH:-}" ]]; then
    "$ADB" -s "$SERIAL" shell am start -W -a android.intent.action.VIEW -d "$SCHEME:${PRISM_PATH}" "$APP_ID" >/dev/null
  elif [[ "$AUTOMOTIVE" == 1 && "$CAR" == 1 ]]; then
    "$ADB" -s "$SERIAL" shell am start -W -n "$APP_ID/androidx.car.app.activity.CarAppActivity" >/dev/null
  else
    "$ADB" -s "$SERIAL" shell am start -W -n "$APP_ID/dev.prism.PrismActivity" >/dev/null
  fi
fi
