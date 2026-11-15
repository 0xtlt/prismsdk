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
#   PRISM_PATH         first screen (deep link)
#   PRISM_RELEASE=1    release build
#   PRISM_NO_INSTALL=1 build only
#   PRISM_ANDROID_AUTOMOTIVE=1  a build for cars running Android (default:
#                      when the device is one); car screens need cx.register_car
#   PRISM_ANDROID_KEYSTORE, PRISM_ANDROID_KEYSTORE_PASS, PRISM_ANDROID_KEY_ALIAS
#                      signing key for store builds (default: the debug key)
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
PROFILE_DIR=debug CARGO_FLAGS=()
if [[ "${PRISM_RELEASE:-}" == 1 ]]; then PROFILE_DIR=release CARGO_FLAGS=(--release); fi
OUT="$TARGET_DIR/android/$PROFILE_DIR"
STAGE="$OUT/stage"
rm -rf "$STAGE" && mkdir -p "$STAGE/lib/arm64-v8a" "$OUT/classes" "$STAGE/assets"

# 1. The app as a shared library (arm64: phones and Apple Silicon emulators).
cargo ndk -t arm64-v8a -P "$MIN_SDK" rustc -p "$PKG" --lib --crate-type cdylib ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
cp "$TARGET_DIR/aarch64-linux-android/$PROFILE_DIR/lib$LIB.so" "$STAGE/lib/arm64-v8a/"

# 2. What the app declares (a host build): assets (picture("logo.png")) and widgets.
cargo build -q -p "$PKG"
HOST_BIN="$TARGET_DIR/debug/$PKG"
ASSETS="$(PRISM_DESCRIBE=assets "$HOST_BIN" 2>/dev/null || true)"
[[ -n "$ASSETS" && -d "$ASSETS" ]] && cp -R "$ASSETS"/. "$STAGE/assets/"
rm -rf "$OUT/res" "$OUT/gen" && mkdir -p "$OUT/res/layout" "$OUT/gen"
# Each widget kind: a provider subclass, its size (appwidget-provider XML) and
# a manifest receiver.
RECEIVERS="$(PRISM_DESCRIBE=widgets "$HOST_BIN" 2>/dev/null | OUT="$OUT" python3 -c '
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
    os.makedirs(os.path.join(out, "res/values"), exist_ok=True)
    with open(os.path.join(out, "res/values/prism_widgets.xml"), "w") as f:
        f.write("<resources>\n" + "\n".join(strings) + "\n</resources>\n")
')"

# 3. The car (cx.register_car): Google's Car App Library, fetched from Maven
#    (android-deps.py) for Android Auto; for cars running Android, its
#    Automotive part too (PRISM_ANDROID_AUTOMOTIVE=1, or such a device).
CAR="$(PRISM_DESCRIBE=car "$HOST_BIN" 2>/dev/null || echo 0)"
SERIAL="${PRISM_SERIAL:-$("$ADB" devices 2>/dev/null | awk 'NR>1 && $2=="device" {print $1; exit}')}"
AUTOMOTIVE="${PRISM_ANDROID_AUTOMOTIVE:-}"
if [[ -z "$AUTOMOTIVE" && -n "$SERIAL" && "${PRISM_NO_INSTALL:-}" != 1 ]]; then
  "$ADB" -s "$SERIAL" shell pm has-feature android.hardware.type.automotive 2>/dev/null | grep -q true && AUTOMOTIVE=1
fi
LIB_JARS="" LIB_RES=() LIB_PKGS=""
if [[ "$CAR" == 1 ]]; then
  ROOTS=(androidx.car.app:app:1.7.0)
  [[ "$AUTOMOTIVE" == 1 ]] && ROOTS+=(androidx.car.app:app-automotive:1.7.0)
  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/prism/maven"
  mkdir -p "$OUT/libres"
  while IFS=$'\t' read -r kind a b; do
    case $kind in
      jar) LIB_JARS="$LIB_JARS:$a" ;;
      res)
        zip="$OUT/libres/${b//./_}.zip"
        "$BUILD_TOOLS/aapt2" compile --dir "$a" -o "$zip"
        LIB_RES+=(-R "$zip") LIB_PKGS="${LIB_PKGS:+$LIB_PKGS:}$b" ;;
    esac
  done < <(python3 "$(dirname "$0")/android-deps.py" "$CACHE" "${ROOTS[@]}" | python3 -c '
import json, sys
for e in json.load(sys.stdin):
    for j in e["jars"]:
        print(f"jar\t{j}\t")
    res, package = e["res"], e["package"]
    if res and package:
        print(f"res\t{res}\t{package}")
')
  mkdir -p "$OUT/res/xml"
  printf '<automotiveApp><uses name="template"/></automotiveApp>\n' > "$OUT/res/xml/automotive_app_desc.xml"
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
cat > "$OUT/AndroidManifest.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="$APP_ID">
$PERMS
$CAR_FEATURES
  <!-- Also for TVs (no touch screen) and watches. -->
  <uses-feature android:name="android.hardware.touchscreen" android:required="false"/>
  <uses-feature android:name="android.software.leanback" android:required="false"/>
  <uses-feature android:name="android.hardware.type.watch" android:required="false"/>
  <application android:label="$APP_NAME" $ICON_ATTR android:extractNativeLibs="false" android:supportsRtl="true"
      android:theme="@android:style/Theme.DeviceDefault.DayNight">
    <activity android:name="dev.prism.PrismActivity" android:exported="true" android:launchMode="singleTop"
        android:configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|uiMode|density|fontScale|locale|layoutDirection">
      <meta-data android:name="prism.lib" android:value="$LIB"/>
$LAUNCHER
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
    <service android:name="dev.prism.PrismJob" android:exported="false"
        android:permission="android.permission.BIND_JOB_SERVICE"/>
    <provider android:name="dev.prism.PrismFiles" android:authorities="$APP_ID.prism.files"
        android:exported="false" android:grantUriPermissions="true"/>
  </application>
</manifest>
XML

# 5. Link resources (the libraries' too, with their R classes), then the Java
#    host, widget providers and libraries → dex.
rm -rf "$OUT/rjava" && mkdir -p "$OUT/rjava"
"$BUILD_TOOLS/aapt2" link -o "$OUT/base.apk" -I "$ANDROID_JAR" --manifest "$OUT/AndroidManifest.xml" \
  -A "$STAGE/assets" --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
  --version-code "${PRISM_BUILD:-1}" --version-name "${PRISM_VERSION:-0.1.0}" ${RES_ARGS[@]+"${RES_ARGS[@]}"} \
  ${LIB_RES[@]+"${LIB_RES[@]}"} --auto-add-overlay --java "$OUT/rjava" ${LIB_PKGS:+--extra-packages "$LIB_PKGS"}
rm -rf "$OUT/classes" && mkdir -p "$OUT/classes"
SOURCES=("$PLATFORM/android/java" "$OUT/gen")
[[ "$CAR" == 1 ]] && SOURCES+=("$PLATFORM/android/car" "$OUT/rjava")
javac --release 17 -nowarn -classpath "$ANDROID_JAR$LIB_JARS" -d "$OUT/classes" $(find "${SOURCES[@]}" -name '*.java')
"$BUILD_TOOLS/d8" --min-api "$MIN_SDK" --lib "$ANDROID_JAR" --output "$STAGE" \
  $(find "$OUT/classes" -name '*.class') ${LIB_JARS//:/ }

# 6. Add code and native libraries, align (16 KB pages), sign.
(cd "$STAGE" && zip -q "$OUT/base.apk" classes.dex && zip -q -0 -r "$OUT/base.apk" lib)
# The libraries' Java resources (car-app-api.level…), as Gradle packages them.
if [[ -n "$LIB_JARS" ]]; then
  # Unzipped entries can be read-only: make them removable first.
  [ -d "$OUT/javares" ] && chmod -R u+rwX "$OUT/javares"
  rm -rf "$OUT/javares" && mkdir -p "$OUT/javares"
  for jar in ${LIB_JARS//:/ }; do
    unzip -qo "$jar" -x '*.class' 'META-INF/MANIFEST.MF' 'META-INF/*.SF' 'META-INF/*.RSA' 'META-INF/*.DSA' \
      'META-INF/versions/*' 'module-info.class' -d "$OUT/javares" 2>/dev/null || true
  done
  # Some jars store directories without the search bit: fix them, or their
  # files would be left out.
  chmod -R u+rwX "$OUT/javares"
  (cd "$OUT/javares" && find . -type f | sed 's|^\./||' | zip -q "$OUT/base.apk" -@)
fi
"$BUILD_TOOLS/zipalign" -f -P 16 4 "$OUT/base.apk" "$OUT/aligned.apk"
if [[ -n "${PRISM_ANDROID_KEYSTORE:-}" ]]; then
  KEYSTORE="$PRISM_ANDROID_KEYSTORE" PASS="${PRISM_ANDROID_KEYSTORE_PASS:?set PRISM_ANDROID_KEYSTORE_PASS}"
  ALIAS="${PRISM_ANDROID_KEY_ALIAS:?set PRISM_ANDROID_KEY_ALIAS}"
else
  # The SDK's conventional debug key (what Android Studio uses too).
  KEYSTORE="$HOME/.android/debug.keystore" PASS=android ALIAS=androiddebugkey
  if [[ ! -f "$KEYSTORE" ]]; then
    mkdir -p "$HOME/.android"
    keytool -genkeypair -keystore "$KEYSTORE" -storepass android -keypass android -alias androiddebugkey \
      -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >/dev/null
  fi
  [[ "${PRISM_RELEASE:-}" == 1 ]] && echo "note: signed with the debug key; set PRISM_ANDROID_KEYSTORE for a store build" >&2
fi
APK="$OUT/$APP_NAME.apk"
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-pass "pass:$PASS" --key-pass "pass:$PASS" \
  --ks-key-alias "$ALIAS" --out "$APK" "$OUT/aligned.apk"
echo "Built $APK"
[[ "${PRISM_NO_INSTALL:-}" == 1 ]] && exit 0

# 7. Install and launch.
[[ -n "$SERIAL" ]] || { echo "No Android device or emulator running (start one: emulator -avd <name>)" >&2; exit 1; }
"$ADB" -s "$SERIAL" install -r "$APK" >/dev/null
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
