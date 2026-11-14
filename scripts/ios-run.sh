#!/usr/bin/env bash
# Build a Prism app for iOS — app + WidgetKit extension (widgets, Live
# Activities, Dynamic Island) — then install and launch it on a simulator or
# a real iPhone. Used by `prism run ios` / `prism build ios` and `mise run ios`.
#
#   scripts/ios-run.sh <package> [--no-launch]
#
# Environment:
#   PRISM_SIM        simulator name or UDID (default: first available iPhone)
#   PRISM_DEVICE     real device name or UDID (signs and installs with devicectl)
#   PRISM_TEAM       Apple team id (default: the team of a matching profile)
#   PRISM_PROFILE    .mobileprovision to use (default: best match for the bundle id)
#   PRISM_IDENTITY   code signing identity (default: first "Apple Development")
#   PRISM_BUNDLE_ID  default dev.prism.<package>
#   PRISM_APP_NAME   display name (default: package name, capitalized)
#   PRISM_ICON       1024×1024 PNG app icon
#   PRISM_RELEASE=1  release build
#   PRISM_PROJECT    project root (default: this repository)
#   PRISM_PLATFORM   Prism platform files (default: <project>/platform)
set -euo pipefail

PKG="${1:?usage: ios-run.sh <package> [--no-launch]}"
LAUNCH=1
[[ "${2:-}" == "--no-launch" ]] && LAUNCH=0

ROOT="${PRISM_PROJECT:-$(cd "$(dirname "$0")/.." && pwd)}"
PLATFORM="${PRISM_PLATFORM:-$ROOT/platform}"
cd "$ROOT"
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
DEVICE="${PRISM_DEVICE:-}"
if [[ -n "$DEVICE" ]]; then
  TARGET=aarch64-apple-ios SDK=iphoneos PLATFORM_NAME=iPhoneOS SWIFT_TRIPLE_SUFFIX=""
else
  TARGET=aarch64-apple-ios-sim SDK=iphonesimulator PLATFORM_NAME=iPhoneSimulator SWIFT_TRIPLE_SUFFIX="-simulator"
fi
PROFILE_DIR=debug CARGO_FLAGS=()
if [[ "${PRISM_RELEASE:-}" == 1 ]]; then PROFILE_DIR=release CARGO_FLAGS=(--release); fi
MIN_IOS="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
SLUG="${PKG//_/-}"
BUNDLE_ID="${PRISM_BUNDLE_ID:-dev.prism.$SLUG}"
GROUP="group.$BUNDLE_ID"
SCHEME="prism-$SLUG"
APP_NAME="${PRISM_APP_NAME:-$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}}"
OUT="$TARGET_DIR/ios/$SDK"
APP="$OUT/$APP_NAME.app"
APPEX="$APP/PlugIns/PrismWidgets.appex"
GEN="$OUT/generated"
mkdir -p "$GEN"

# 1. Pick the destination: a real device, or a booted (slimmed) simulator.
#    Build-only runs (PRISM_NO_INSTALL=1) need neither.
if [[ "${PRISM_NO_INSTALL:-}" == 1 ]]; then
  UDID=""
elif [[ -n "$DEVICE" ]]; then
  xcrun devicectl list devices --json-output "$GEN/devices.json" >/dev/null
  UDID="$(PRISM_DEVICE="$DEVICE" GEN="$GEN" python3 -c '
import json, os
want = os.environ["PRISM_DEVICE"]
devs = json.load(open(os.environ["GEN"] + "/devices.json"))["result"]["devices"]
for d in devs:
    p, h = d.get("deviceProperties", {}), d.get("hardwareProperties", {})
    if h.get("platform") == "iOS" and want in (d.get("identifier"), h.get("udid"), p.get("name")):
        print(d["identifier"]); break
')"
  [[ -n "$UDID" ]] || { echo "No paired iOS device named or with id $DEVICE (xcrun devicectl list devices)" >&2; exit 1; }
else
  UDID="$(xcrun simctl list devices available -j | python3 -c '
import json, os, sys
want = os.environ.get("PRISM_SIM", "")
devices = [d for runtime, ds in json.load(sys.stdin)["devices"].items() if "iOS" in runtime for d in ds]
pick = [d for d in devices if want in (d["udid"], d["name"])] if want else [d for d in devices if d["name"].startswith("iPhone")]
booted = [d for d in pick if d["state"] == "Booted"]
print((booted or pick)[0]["udid"] if pick else "")
')"
  [[ -n "$UDID" ]] || { echo "No matching iOS simulator (PRISM_SIM=${PRISM_SIM:-})" >&2; exit 1; }
  if command -v simslim >/dev/null && ! simslim list --booted 2>/dev/null | grep -q "$UDID"; then
    # Prism apps ship widgets and Live Activities: keep their daemons running.
    simslim on "$UDID" --except widgets
  fi
  xcrun simctl bootstatus "$UDID" -b >/dev/null
fi

# 2. Describe the app's widgets (host build) and generate the extension entry point.
cargo build -q -p "$PKG"
HOST_BIN="$TARGET_DIR/debug/$PKG"
PRISM_DESCRIBE=widgets "$HOST_BIN" > "$GEN/widgets.json"
python3 "$PLATFORM/apple/generate_widgets.py" "$GEN/widgets.json" ios > "$GEN/Generated.swift"

# 3. Signing (device) or simulator entitlements: an App Group shared by the
#    app and its widget extension.
if [[ -n "$DEVICE" ]]; then
  PROFILE="${PRISM_PROFILE:-}"
  if [[ -z "$PROFILE" ]]; then
    PROFILE="$(BUNDLE_ID="$BUNDLE_ID" TEAM="${PRISM_TEAM:-}" python3 -c '
import glob, os, plistlib, subprocess, datetime
bid, team = os.environ["BUNDLE_ID"], os.environ["TEAM"]
home = os.path.expanduser("~")
best = None
for f in glob.glob(home + "/Library/Developer/Xcode/UserData/Provisioning Profiles/*.mobileprovision") + \
         glob.glob(home + "/Library/MobileDevice/Provisioning Profiles/*.mobileprovision"):
    raw = subprocess.run(["security", "cms", "-D", "-i", f], capture_output=True).stdout
    try: p = plistlib.loads(raw)
    except Exception: continue
    if p["ExpirationDate"] < datetime.datetime.now() or not p.get("ProvisionedDevices"): continue
    t = p["TeamIdentifier"][0]
    if team and t != team: continue
    app_id = p["Entitlements"]["application-identifier"].split(".", 1)[1]
    score = 2 if app_id == bid else 1 if app_id == "*" or (app_id.endswith("*") and bid.startswith(app_id[:-1])) else 0
    if score and (best is None or score > best[0]): best = (score, f)
print(best[1] if best else "")
')"
  fi
  [[ -n "$PROFILE" ]] || { echo "No provisioning profile for $BUNDLE_ID: open the project once in Xcode with your team, or set PRISM_PROFILE" >&2; exit 1; }
  security cms -D -i "$PROFILE" > "$GEN/profile.plist"
  # Entitlements granted by the profile (app id, team, debugging, groups).
  python3 - "$GEN/profile.plist" "$GEN/entitlements.plist" "$BUNDLE_ID" <<'PY'
import plistlib, sys
p = plistlib.load(open(sys.argv[1], "rb"))
e = dict(p["Entitlements"])
team = p["TeamIdentifier"][0]
e["application-identifier"] = f"{team}.{sys.argv[3]}"
e.pop("keychain-access-groups", None)
plistlib.dump(e, open(sys.argv[2], "wb"))
PY
  cp "$GEN/entitlements.plist" "$GEN/appex.entitlements.plist"
  /usr/libexec/PlistBuddy -c "Set :application-identifier $(/usr/libexec/PlistBuddy -c 'Print :TeamIdentifier:0' "$GEN/profile.plist").$BUNDLE_ID.widgets" "$GEN/appex.entitlements.plist"
  if ! /usr/libexec/PlistBuddy -c "Print :com.apple.security.application-groups" "$GEN/entitlements.plist" >/dev/null 2>&1; then
    echo "note: the profile has no App Group: data stays in the app (widgets show placeholders)" >&2
  fi
  IDENTITY="${PRISM_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)}"
  [[ -n "$IDENTITY" ]] || { echo "No \"Apple Development\" signing identity in the keychain" >&2; exit 1; }
else
  cat > "$GEN/entitlements.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.application-groups</key><array><string>$GROUP</string></array>
</dict>
</plist>
PLIST
  cp "$GEN/entitlements.plist" "$GEN/appex.entitlements.plist"
  # The simulator reads them from a section of each binary (XML and DER).
  if xcrun -f derq >/dev/null 2>&1; then
    xcrun derq query -f xml -i "$GEN/entitlements.plist" -o "$GEN/entitlements.plist.der" --raw
  fi
  export PRISM_ENTITLEMENTS="$GEN/entitlements.plist"
fi

# 4. Build the app (links the Swift bridge, see crates/prism-apple-build).
cargo build -p "$PKG" --target "$TARGET" ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}

# 5. Bundle: binary, assets, icon.
rm -rf "$APP" && mkdir -p "$APPEX"
cp "$TARGET_DIR/$TARGET/$PROFILE_DIR/$PKG" "$APP/$PKG"
ASSETS="$(PRISM_DESCRIBE=assets "$HOST_BIN")"
[[ -n "$ASSETS" && -d "$ASSETS" ]] && cp -R "$ASSETS" "$APP/assets"
RESOURCES="$APP"
# Background tasks (cx.register_background_task) are declared to iOS.
BG_PLIST="$(PRISM_DESCRIBE=background "$HOST_BIN" | BUNDLE_ID="$BUNDLE_ID" python3 -c '
import json, os, sys
tasks = json.loads(sys.stdin.read() or "[]")
if tasks:
    bid = os.environ["BUNDLE_ID"]
    ids = "".join("<string>%s.%s</string>" % (bid, t["id"]) for t in tasks)
    modes = sorted({"fetch" if t["kind"] == "refresh" else "processing" for t in tasks})
    print("  <key>BGTaskSchedulerPermittedIdentifiers</key><array>" + ids + "</array>")
    print("  <key>UIBackgroundModes</key><array>" + "".join("<string>%s</string>" % m for m in modes) + "</array>")
')"
# Languages (locales/*.json): declared to the system, so iOS Settings
# offers a per-app language and the App Store lists them.
LOC_PLIST=""
if [[ -n "${PRISM_LOCALES:-}" ]]; then
  LOC_PLIST="  <key>CFBundleLocalizations</key><array>"
  for l in ${PRISM_LOCALES//,/ }; do
    LOC_PLIST+="<string>$l</string>"
    mkdir -p "$RESOURCES/$l.lproj"
    printf '/* Prism: %s */\n' "$l" > "$RESOURCES/$l.lproj/InfoPlist.strings"
  done
  LOC_PLIST+="</array>"
fi
ICON_PLIST=""
if [[ -n "${PRISM_ICON:-}" ]]; then
  bash "$PLATFORM/apple/app-icon.sh" "$PRISM_ICON" "$SDK" "$MIN_IOS" "$APP" "$GEN"
  ICON_PLIST="$(cat "$GEN/icon-info.xml")"
fi

plist_common() { # $1 executable, $2 bundle id, $3 package type
  cat <<PLIST
  <key>CFBundleExecutable</key><string>$1</string>
  <key>CFBundleIdentifier</key><string>$2</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>$3</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>DTPlatformName</key><string>$SDK</string>
  <key>CFBundleShortVersionString</key><string>${PRISM_VERSION:-0.1.0}</string>
  <key>CFBundleVersion</key><string>${PRISM_BUILD:-1}</string>
  <key>CFBundleSupportedPlatforms</key><array><string>$PLATFORM_NAME</string></array>
  <key>MinimumOSVersion</key><string>$MIN_IOS</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>PrismAppGroup</key><string>$GROUP</string>
  <key>PrismURLScheme</key><string>$SCHEME</string>
PLIST
}

cat > "$APP/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
$(plist_common "$PKG" "$BUNDLE_ID" APPL)
$ICON_PLIST
$LOC_PLIST
$BG_PLIST
  <key>LSRequiresIPhoneOS</key><true/>
  <key>UILaunchScreen</key><dict/>
  <key>NSSupportsLiveActivities</key><true/>
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
    <key>CFBundleURLSchemes</key><array><string>$SCHEME</string></array>
  </dict></array>
  <key>UIApplicationSceneManifest</key>
  <dict>
    <key>UIApplicationSupportsMultipleScenes</key><false/>
    <key>UISceneConfigurations</key>
    <dict>
      <key>UIWindowSceneSessionRoleApplication</key>
      <array><dict>
        <key>UISceneConfigurationName</key><string>Default</string>
        <key>UISceneDelegateClassName</key><string>PrismSceneDelegate</string>
      </dict></array>
    </dict>
  </dict>
</dict>
</plist>
PLIST

# 6. Widget extension: Prism's generic SwiftUI renderer + the generated entry point.
SIM_SECTIONS=()
if [[ -z "$DEVICE" ]]; then
  SIM_SECTIONS=(-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __entitlements -Xlinker "$GEN/entitlements.plist")
  [[ -f "$GEN/entitlements.plist.der" ]] && SIM_SECTIONS+=(-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __ents_der -Xlinker "$GEN/entitlements.plist.der")
fi
xcrun --sdk "$SDK" swiftc -target "arm64-apple-ios$MIN_IOS$SWIFT_TRIPLE_SUFFIX" -swift-version 5 -O \
  -parse-as-library -application-extension -module-name PrismWidgets \
  -o "$APPEX/PrismWidgets" \
  -Xlinker -e -Xlinker _NSExtensionMain \
  ${SIM_SECTIONS[@]+"${SIM_SECTIONS[@]}"} \
  "$PLATFORM"/apple/Shared/*.swift "$PLATFORM"/apple/Extension/*.swift "$GEN/Generated.swift"
cat > "$APPEX/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
$(plist_common PrismWidgets "$BUNDLE_ID.widgets" XPC!)
  <key>NSExtension</key>
  <dict><key>NSExtensionPointIdentifier</key><string>com.apple.widgetkit-extension</string></dict>
</dict>
</plist>
PLIST

# 7. Sign: ad hoc on the simulator, with the profile on a device.
if [[ -n "$DEVICE" ]]; then
  cp "$PROFILE" "$APP/embedded.mobileprovision"
  cp "$PROFILE" "$APPEX/embedded.mobileprovision"
  codesign --force --sign "$IDENTITY" --timestamp=none --entitlements "$GEN/appex.entitlements.plist" "$APPEX" >/dev/null
  codesign --force --sign "$IDENTITY" --timestamp=none --entitlements "$GEN/entitlements.plist" "$APP" >/dev/null
  codesign --verify --deep --strict "$APP"
else
  codesign --force --sign - --entitlements "$GEN/appex.entitlements.plist" "$APPEX" >/dev/null
  codesign --force --sign - --entitlements "$GEN/entitlements.plist" "$APP" >/dev/null
fi
echo "Built $APP"
[[ "${PRISM_NO_INSTALL:-}" == 1 ]] && exit 0

# 8. Install and launch.
if [[ -n "$DEVICE" ]]; then
  xcrun devicectl device install app --device "$UDID" "$APP" >/dev/null
  echo "Installed $BUNDLE_ID on $DEVICE"
  if [[ $LAUNCH == 1 ]]; then
    xcrun devicectl device process launch --terminate-existing --device "$UDID" "$BUNDLE_ID" >/dev/null
  fi
else
  xcrun simctl install "$UDID" "$APP"
  echo "Installed $BUNDLE_ID (+ widgets, Live Activities) on $UDID"
  if [[ $LAUNCH == 1 ]]; then
    open -a Simulator --args -CurrentDeviceUDID "$UDID" 2>/dev/null || true
    # Prism variables (PRISM_PATH, PRISM_DEV_STATE…) reach the app.
    for v in $(env | sed -n 's/^\(PRISM_[A-Z_]*\)=.*/\1/p'); do export "SIMCTL_CHILD_$v=${!v}"; done
    xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE_ID"
  fi
fi
