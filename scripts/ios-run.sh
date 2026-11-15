#!/usr/bin/env bash
# Build a Prism app for iOS — app + WidgetKit extension (widgets, Live
# Activities, Dynamic Island) — or tvOS (PRISM_OS=tvos: the app alone), then
# install and launch it on a simulator or a real device. Used by
# `prism run ios|tvos` / `prism build ios|tvos` and `mise run ios`.
#
#   scripts/ios-run.sh <package> [--no-launch]
#
# Environment:
#   PRISM_OS         ios (default), tvos, watchos or visionos
#   PRISM_SIM        simulator name or UDID (default: first available iPhone / Apple TV)
#   PRISM_DEVICE     real device name or UDID (signs and installs with devicectl)
#   PRISM_DIST=appstore  a store build: device SDK, "Apple Distribution"
#                    identity, App Store profiles (app and widgets), then an .ipa
#   PRISM_WIDGETS_PROFILE  the widget extension's profile (default: best match
#                    for <bundle id>.widgets, else the app's)
#   PRISM_KEYCHAIN   keychain holding the identity (default: the search list)
#   PRISM_BUILD      CFBundleVersion (default 1)
#   PRISM_CATEGORY   LSApplicationCategoryType (public.app-category.…)
#   PRISM_ENCRYPTION=0  ITSAppUsesNonExemptEncryption = NO (export compliance)
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
DIST="${PRISM_DIST:-}"
case "$DIST" in
  "") ;;
  appstore) export PRISM_NO_INSTALL=1 ;;
  *) echo "PRISM_DIST=$DIST: use appstore" >&2; exit 1 ;;
esac
# Device SDK builds: a real device or a store build.
DEVICE_BUILD="$DEVICE$DIST"
OS="${PRISM_OS:-ios}"
if [[ "$OS" == watchos ]]; then
  # watchOS: a watch-only SwiftUI app (WKApplication, WKWatchOnly), no widgets.
  RUNTIME_NAME=watchOS DEVICE_PREFIX="Apple Watch" DEVICE_FAMILY="<integer>4</integer>" WIDGETS=0
  MIN_IOS="${WATCHOS_DEPLOYMENT_TARGET:-10.0}" SWIFT_OS=watchos
  if [[ -n "$DEVICE_BUILD" ]]; then
    TARGET=aarch64-apple-watchos SDK=watchos PLATFORM_NAME=WatchOS SWIFT_TRIPLE_SUFFIX=""
    # arm64 watches (Rust's aarch64-apple-watchos) run watchOS 26+.
    if [[ "${MIN_IOS%%.*}" -lt 26 ]]; then MIN_IOS=26.0; fi
  else
    TARGET=aarch64-apple-watchos-sim SDK=watchsimulator PLATFORM_NAME=WatchSimulator SWIFT_TRIPLE_SUFFIX="-simulator"
  fi
elif [[ "$OS" == visionos ]]; then
  # visionOS: the UIKit app in a window (iPad-like); widgets need visionOS 26.
  RUNTIME_NAME=xrOS DEVICE_PREFIX="Apple Vision Pro" DEVICE_FAMILY="<integer>7</integer>" WIDGETS=1
  MIN_IOS="${XROS_DEPLOYMENT_TARGET:-26.0}" SWIFT_OS=xros
  if [[ -n "$DEVICE_BUILD" ]]; then
    TARGET=aarch64-apple-visionos SDK=xros PLATFORM_NAME=XROS SWIFT_TRIPLE_SUFFIX=""
  else
    TARGET=aarch64-apple-visionos-sim SDK=xrsimulator PLATFORM_NAME=XRSimulator SWIFT_TRIPLE_SUFFIX="-simulator"
  fi
elif [[ "$OS" == tvos ]]; then
  # tvOS: no WidgetKit, Live Activities or iPhone-only keys.
  RUNTIME_NAME=tvOS DEVICE_PREFIX="Apple TV" DEVICE_FAMILY="<integer>3</integer>" WIDGETS=0
  MIN_IOS="${TVOS_DEPLOYMENT_TARGET:-17.0}" SWIFT_OS=tvos
  if [[ -n "$DEVICE_BUILD" ]]; then
    TARGET=aarch64-apple-tvos SDK=appletvos PLATFORM_NAME=AppleTVOS SWIFT_TRIPLE_SUFFIX=""
  else
    TARGET=aarch64-apple-tvos-sim SDK=appletvsimulator PLATFORM_NAME=AppleTVSimulator SWIFT_TRIPLE_SUFFIX="-simulator"
  fi
else
  RUNTIME_NAME=iOS DEVICE_PREFIX=iPhone DEVICE_FAMILY="<integer>1</integer><integer>2</integer>" WIDGETS=1
  MIN_IOS="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}" SWIFT_OS=ios
  if [[ -n "$DEVICE_BUILD" ]]; then
    TARGET=aarch64-apple-ios SDK=iphoneos PLATFORM_NAME=iPhoneOS SWIFT_TRIPLE_SUFFIX=""
  else
    TARGET=aarch64-apple-ios-sim SDK=iphonesimulator PLATFORM_NAME=iPhoneSimulator SWIFT_TRIPLE_SUFFIX="-simulator"
  fi
fi
PROFILE_DIR=debug CARGO_FLAGS=()
if [[ "${PRISM_RELEASE:-}" == 1 ]]; then PROFILE_DIR=release CARGO_FLAGS=(--release); fi
SLUG="${PKG//_/-}"
BUNDLE_ID="${PRISM_BUNDLE_ID:-dev.prism.$SLUG}"
GROUP="group.$BUNDLE_ID"
SCHEME="prism-$SLUG"
APP_NAME="${PRISM_APP_NAME:-$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}}"
OUT="$TARGET_DIR/$OS/$SDK"
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
  UDID="$(PRISM_DEVICE="$DEVICE" GEN="$GEN" RUNTIME_NAME="$RUNTIME_NAME" python3 -c '
import json, os
want = os.environ["PRISM_DEVICE"]
devs = json.load(open(os.environ["GEN"] + "/devices.json"))["result"]["devices"]
for d in devs:
    p, h = d.get("deviceProperties", {}), d.get("hardwareProperties", {})
    if h.get("platform") == os.environ["RUNTIME_NAME"] and want in (d.get("identifier"), h.get("udid"), p.get("name")):
        print(d["identifier"]); break
')"
  [[ -n "$UDID" ]] || { echo "No paired $RUNTIME_NAME device named or with id $DEVICE (xcrun devicectl list devices)" >&2; exit 1; }
else
  UDID="$(xcrun simctl list devices available -j | RUNTIME_NAME="$RUNTIME_NAME" DEVICE_PREFIX="$DEVICE_PREFIX" python3 -c '
import json, os, sys
want = os.environ.get("PRISM_SIM", "")
runtime = "SimRuntime." + os.environ["RUNTIME_NAME"] + "-"
devices = [d for r, ds in json.load(sys.stdin)["devices"].items() if runtime in r for d in ds]
pick = [d for d in devices if want in (d["udid"], d["name"])] if want else [d for d in devices if d["name"].startswith(os.environ["DEVICE_PREFIX"])]
booted = [d for d in pick if d["state"] == "Booted"]
print((booted or pick)[0]["udid"] if pick else "")
')"
  [[ -n "$UDID" ]] || { echo "No matching $RUNTIME_NAME simulator (PRISM_SIM=${PRISM_SIM:-}; create one with xcrun simctl create)" >&2; exit 1; }
  if command -v simslim >/dev/null && ! simslim list --booted 2>/dev/null | grep -q "$UDID"; then
    # Prism apps ship widgets and Live Activities: keep their daemons running.
    if [[ $WIDGETS == 1 ]]; then simslim on "$UDID" --except widgets; else simslim on "$UDID"; fi
  fi
  xcrun simctl bootstatus "$UDID" -b >/dev/null
fi

# 2. Describe the app's widgets (host build) and generate the extension entry point.
cargo build -q -p "$PKG"
HOST_BIN="$TARGET_DIR/debug/$PKG"
# CarPlay (cx.register_car): a template scene. CarPlay only shows apps with an
# entitlement Apple grants on request (com.apple.developer.carplay-driving-task):
# devices get it from the provisioning profile; the simulator refuses to launch
# an app claiming it without one.
CAR=0
[[ "$OS" == ios ]] && CAR="$(PRISM_DESCRIBE=car "$HOST_BIN" 2>/dev/null || echo 0)"
if [[ $WIDGETS == 1 ]]; then
  PRISM_DESCRIBE=widgets "$HOST_BIN" > "$GEN/widgets.json"
  python3 "$PLATFORM/apple/generate_widgets.py" "$GEN/widgets.json" "$OS" > "$GEN/Generated.swift"
fi

# 3. Signing (device) or simulator entitlements: an App Group shared by the
#    app and its widget extension.
KEYCHAIN_ARGS=()
[[ -n "${PRISM_KEYCHAIN:-}" ]] && KEYCHAIN_ARGS=(--keychain "$PRISM_KEYCHAIN")
# The best provisioning profile for a bundle id: development profiles list
# devices; App Store ones list none and do not allow debugging.
find_profile() { # $1 bundle id
  BUNDLE_ID="$1" TEAM="${PRISM_TEAM:-}" DIST="$DIST" python3 -c '
import glob, os, plistlib, subprocess, datetime
bid, team, dist = os.environ["BUNDLE_ID"], os.environ["TEAM"], os.environ["DIST"]
home = os.path.expanduser("~")
best = None
for f in glob.glob(home + "/Library/Developer/Xcode/UserData/Provisioning Profiles/*.mobileprovision") + \
         glob.glob(home + "/Library/MobileDevice/Provisioning Profiles/*.mobileprovision"):
    raw = subprocess.run(["security", "cms", "-D", "-i", f], capture_output=True).stdout
    try: p = plistlib.loads(raw)
    except Exception: continue
    if p["ExpirationDate"] < datetime.datetime.now(): continue
    if dist:
        if p.get("ProvisionedDevices") or p.get("ProvisionsAllDevices") or p["Entitlements"].get("get-task-allow"): continue
    elif not p.get("ProvisionedDevices"):
        continue
    t = p["TeamIdentifier"][0]
    if team and t != team: continue
    app_id = p["Entitlements"]["application-identifier"].split(".", 1)[1]
    score = 2 if app_id == bid else 1 if app_id == "*" or (app_id.endswith("*") and bid.startswith(app_id[:-1])) else 0
    if score and (best is None or score > best[0]): best = (score, f)
print(best[1] if best else "")
'
}
# Entitlements granted by a profile (app id, team, debugging, groups) for a bundle.
profile_entitlements() { # $1 profile plist, $2 output, $3 bundle id
  ICLOUD="${PRISM_ICLOUD:-}" python3 - "$1" "$2" "$3" <<'PY'
import os, plistlib, sys
p = plistlib.load(open(sys.argv[1], "rb"))
e = dict(p["Entitlements"])
team = p["TeamIdentifier"][0]
e["application-identifier"] = f"{team}.{sys.argv[3]}"
e.pop("keychain-access-groups", None)
kv = "com.apple.developer.ubiquity-kvstore-identifier"
if os.environ.get("ICLOUD") == "1":
    if kv in e:
        e[kv] = f"{team}.{sys.argv[3]}"
    else:
        print("note: the provisioning profile has no iCloud: cloud_storage stays local (enable iCloud for the App ID in Xcode)", file=sys.stderr)
else:
    e.pop(kv, None)
plistlib.dump(e, open(sys.argv[2], "wb"))
PY
}
if [[ -n "$DEVICE_BUILD" ]]; then
  KIND=development
  [[ -n "$DIST" ]] && KIND="App Store"
  PROFILE="${PRISM_PROFILE:-$(find_profile "$BUNDLE_ID")}"
  [[ -n "$PROFILE" ]] || { echo "No $KIND provisioning profile for $BUNDLE_ID: create one in the developer portal (or open the project once in Xcode with your team), or set PRISM_PROFILE" >&2; exit 1; }
  security cms -D -i "$PROFILE" > "$GEN/profile.plist"
  profile_entitlements "$GEN/profile.plist" "$GEN/entitlements.plist" "$BUNDLE_ID"
  if [[ "${PRISM_ICLOUD:-}" == 1 ]] && ! /usr/libexec/PlistBuddy -c "Print :com.apple.developer.ubiquity-kvstore-identifier" "$GEN/entitlements.plist" >/dev/null 2>&1; then
    PRISM_ICLOUD=0
  fi
  # The widget extension: its own profile when there is one (App Store
  # builds need it), else the app's (a wildcard development profile).
  APPEX_PROFILE="$PROFILE"
  if [[ $WIDGETS == 1 ]]; then
    APPEX_PROFILE="${PRISM_WIDGETS_PROFILE:-$(find_profile "$BUNDLE_ID.widgets")}"
    APPEX_PROFILE="${APPEX_PROFILE:-$PROFILE}"
    security cms -D -i "$APPEX_PROFILE" > "$GEN/appex.profile.plist"
    profile_entitlements "$GEN/appex.profile.plist" "$GEN/appex.entitlements.plist" "$BUNDLE_ID.widgets"
    APPEX_ID="$(/usr/libexec/PlistBuddy -c 'Print :Entitlements:application-identifier' "$GEN/appex.profile.plist")"
    if [[ -n "$DIST" && "$APPEX_ID" != *".$BUNDLE_ID.widgets" ]]; then
      echo "No App Store provisioning profile for $BUNDLE_ID.widgets (the widget extension): create one, or set PRISM_WIDGETS_PROFILE" >&2
      exit 1
    fi
  fi
  if ! /usr/libexec/PlistBuddy -c "Print :com.apple.security.application-groups" "$GEN/entitlements.plist" >/dev/null 2>&1; then
    echo "note: the profile has no App Group: data stays in the app (widgets show placeholders)" >&2
  fi
  if [[ -n "$DIST" ]]; then
    IDENTITY="${PRISM_IDENTITY:-$(security find-identity -v -p codesigning ${PRISM_KEYCHAIN:+"$PRISM_KEYCHAIN"} | sed -n 's/.*"\(Apple Distribution[^"]*\)".*/\1/p;s/.*"\(iPhone Distribution[^"]*\)".*/\1/p' | head -1)}"
    [[ -n "$IDENTITY" ]] || { echo "No \"Apple Distribution\" signing identity in the keychain (or set PRISM_IDENTITY)" >&2; exit 1; }
  else
    IDENTITY="${PRISM_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Apple Development[^"]*\)".*/\1/p' | head -1)}"
    [[ -n "$IDENTITY" ]] || { echo "No \"Apple Development\" signing identity in the keychain" >&2; exit 1; }
  fi
else
  # The simulator refuses to launch ad hoc builds with the iCloud
  # entitlement: cloud_storage stays local there.
  if [[ "${PRISM_ICLOUD:-}" == 1 ]]; then
    echo "note: iCloud needs a signed build (a device): on the simulator cloud_storage stays local" >&2
    PRISM_ICLOUD=0
  fi
  cat > "$GEN/entitlements.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.application-groups</key><array><string>$GROUP</string></array>
$( [[ "${PRISM_ICLOUD:-}" == 1 ]] && echo "  <key>com.apple.developer.ubiquity-kvstore-identifier</key><string>${PRISM_TEAM:-SIMULATOR}.$BUNDLE_ID</string>" )
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
rm -rf "$APP" && mkdir -p "$APP"
[[ $WIDGETS == 1 ]] && mkdir -p "$APPEX"
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
# Permissions (Prism.toml [permissions]): the texts iOS/macOS show when asking.
USAGE_PLIST="$(PLATFORM_KIND=ios python3 -c '
import os
from xml.sax.saxutils import escape
keys = {
    "location": ["NSLocationWhenInUseUsageDescription", "NSLocationUsageDescription"],
    "location_always": ["NSLocationAlwaysAndWhenInUseUsageDescription", "NSLocationAlwaysUsageDescription"],
    "camera": ["NSCameraUsageDescription"],
    "photos": ["NSPhotoLibraryUsageDescription", "NSPhotoLibraryAddUsageDescription"],
    "microphone": ["NSMicrophoneUsageDescription"],
    "motion": ["NSMotionUsageDescription"],
}
for line in os.environ.get("PRISM_PERMISSIONS", "").splitlines():
    name, _, text = line.partition("\t")
    if name not in keys:
        print("<!-- unknown permission " + escape(name) + " -->")
        continue
    for key in keys[name]:
        print("  <key>" + key + "</key><string>" + escape(text) + "</string>")
')"
ICON_PLIST=""
if [[ -n "${PRISM_ICON:-}" ]]; then
  bash "$PLATFORM/apple/app-icon.sh" "$PRISM_ICON" "$SDK" "$MIN_IOS" "$APP" "$GEN"
  ICON_PLIST="$(cat "$GEN/icon-info.xml")"
fi

# Device builds record the SDK and Xcode that built them, like Xcode does
# (App Store Connect reads these keys).
BUILD_PLIST=""
if [[ -n "$DEVICE_BUILD" ]]; then
  SDK_VERSION="$(xcrun --sdk "$SDK" --show-sdk-version)"
  SDK_BUILD="$(xcrun --sdk "$SDK" --show-sdk-build-version)"
  XCODE_BUILD="$(xcodebuild -version | awk '/Build version/ {print $3}')"
  # Xcode 26.1.1 → 2611, 27.2 → 2720.
  XCODE_VERSION="$(xcodebuild -version | awk 'NR==1 {split($2, v, "."); printf "%d%d%d", v[1], v[2], v[3]}')"
  BUILD_PLIST="  <key>DTSDKName</key><string>$SDK$SDK_VERSION</string>
  <key>DTSDKBuild</key><string>$SDK_BUILD</string>
  <key>DTPlatformVersion</key><string>$SDK_VERSION</string>
  <key>DTPlatformBuild</key><string>$SDK_BUILD</string>
  <key>DTXcode</key><string>$XCODE_VERSION</string>
  <key>DTXcodeBuild</key><string>$XCODE_BUILD</string>
  <key>DTCompiler</key><string>com.apple.compilers.llvm.clang.1_0</string>
  <key>BuildMachineOSBuild</key><string>$(sw_vers -buildVersion)</string>"
fi
STORE_PLIST=""
[[ -n "${PRISM_CATEGORY:-}" ]] && STORE_PLIST+="  <key>LSApplicationCategoryType</key><string>$PRISM_CATEGORY</string>
"
[[ "${PRISM_ENCRYPTION:-}" == 0 ]] && STORE_PLIST+="  <key>ITSAppUsesNonExemptEncryption</key><false/>
"
if [[ "$OS" == ios ]]; then
  # iPad multitasking needs every orientation (App Store validation).
  STORE_PLIST+="  <key>UISupportedInterfaceOrientations</key><array><string>UIInterfaceOrientationPortrait</string><string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string></array>
  <key>UISupportedInterfaceOrientations~ipad</key><array><string>UIInterfaceOrientationPortrait</string><string>UIInterfaceOrientationPortraitUpsideDown</string><string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string></array>"
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
  <key>UIDeviceFamily</key><array>$DEVICE_FAMILY</array>
  <key>PrismAppGroup</key><string>$GROUP</string>
  <key>PrismURLScheme</key><string>$SCHEME</string>
$BUILD_PLIST
PLIST
}

cat > "$APP/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
$(plist_common "$PKG" "$BUNDLE_ID" APPL)
$STORE_PLIST
$ICON_PLIST
$LOC_PLIST
$BG_PLIST
$USAGE_PLIST
$( [[ "${PRISM_ICLOUD:-}" == 1 ]] && echo "  <key>PrismICloud</key><true/>" )
$( [[ "$OS" == ios ]] && printf '%s\n' "  <key>LSRequiresIPhoneOS</key><true/>" "  <key>UILaunchScreen</key><dict/>" "  <key>NSSupportsLiveActivities</key><true/>" )
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
    <key>CFBundleURLSchemes</key><array><string>$SCHEME</string></array>
  </dict></array>
$( if [[ "$OS" == watchos ]]; then cat <<'WATCH'
  <key>WKApplication</key><true/>
  <key>WKWatchOnly</key><true/>
WATCH
else cat <<'SCENE'
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
SCENE
if [[ "$CAR" == 1 ]]; then cat <<'CARPLAY'
      <key>CPTemplateApplicationSceneSessionRoleApplication</key>
      <array><dict>
        <key>UISceneConfigurationName</key><string>CarPlay</string>
        <key>UISceneClassName</key><string>CPTemplateApplicationScene</string>
        <key>UISceneDelegateClassName</key><string>PrismCarPlaySceneDelegate</string>
      </dict></array>
CARPLAY
fi
cat <<'SCENE_END'
    </dict>
  </dict>
SCENE_END
[[ "$CAR" == 1 ]] && echo "  <key>PrismCarPlay</key><true/>"
fi )
</dict>
</plist>
PLIST

# 6. Widget extension (iOS): Prism's generic SwiftUI renderer + the generated entry point.
SIM_SECTIONS=()
if [[ -z "$DEVICE_BUILD" ]]; then
  SIM_SECTIONS=(-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __entitlements -Xlinker "$GEN/entitlements.plist")
  [[ -f "$GEN/entitlements.plist.der" ]] && SIM_SECTIONS+=(-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __ents_der -Xlinker "$GEN/entitlements.plist.der")
fi
if [[ $WIDGETS == 1 ]]; then
xcrun --sdk "$SDK" swiftc -target "arm64-apple-$SWIFT_OS$MIN_IOS$SWIFT_TRIPLE_SUFFIX" -swift-version 5 -O \
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
fi

# 7. Sign: ad hoc on the simulator, with the profile on a device.
if [[ -n "$DEVICE_BUILD" ]]; then
  cp "$PROFILE" "$APP/embedded.mobileprovision"
  if [[ $WIDGETS == 1 ]]; then
    cp "$APPEX_PROFILE" "$APPEX/embedded.mobileprovision"
    codesign --force --sign "$IDENTITY" ${KEYCHAIN_ARGS[@]+"${KEYCHAIN_ARGS[@]}"} --timestamp=none \
      --entitlements "$GEN/appex.entitlements.plist" "$APPEX" >/dev/null
  fi
  codesign --force --sign "$IDENTITY" ${KEYCHAIN_ARGS[@]+"${KEYCHAIN_ARGS[@]}"} --timestamp=none \
    --entitlements "$GEN/entitlements.plist" "$APP" >/dev/null
  codesign --verify --deep --strict "$APP"
else
  [[ $WIDGETS == 1 ]] && codesign --force --sign - --entitlements "$GEN/appex.entitlements.plist" "$APPEX" >/dev/null
  codesign --force --sign - --entitlements "$GEN/entitlements.plist" "$APP" >/dev/null
fi
echo "Built $APP"
if [[ -n "$DIST" ]]; then
  # The .ipa App Store Connect takes: Payload/<App>.app, zipped.
  IPA="$OUT/$APP_NAME.ipa"
  rm -rf "$OUT/ipa" "$IPA" && mkdir -p "$OUT/ipa/Payload"
  cp -R "$APP" "$OUT/ipa/Payload/"
  (cd "$OUT/ipa" && zip -qry -X "$IPA" Payload)
  rm -rf "$OUT/ipa"
  echo "Built $IPA"
fi
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
  echo "Installed $BUNDLE_ID$( [[ $WIDGETS == 1 ]] && echo " (+ widgets, Live Activities)") on $UDID"
  if [[ $LAUNCH == 1 ]]; then
    open -a Simulator --args -CurrentDeviceUDID "$UDID" 2>/dev/null || true
    # Prism variables (PRISM_PATH, PRISM_DEV_STATE…) reach the app.
    for v in $(env | sed -n 's/^\(PRISM_[A-Z_]*\)=.*/\1/p'); do export "SIMCTL_CHILD_$v=${!v}"; done
    xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE_ID"
  fi
fi
