#!/usr/bin/env bash
# Build a Prism app for the iOS simulator — app + WidgetKit extension (widgets,
# Live Activities, Dynamic Island) — then install and launch it.
#
#   scripts/ios-run.sh <package> [--no-launch]
#   PRISM_SIM="iPhone 17" (name or UDID, default: first available iPhone)
set -euo pipefail

PKG="${1:?usage: ios-run.sh <package> [--no-launch]}"
LAUNCH=1
[[ "${2:-}" == "--no-launch" ]] && LAUNCH=0

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET=aarch64-apple-ios-sim
MIN_IOS="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
SLUG="${PKG//_/-}"
BUNDLE_ID="dev.prism.$SLUG"
GROUP="group.$BUNDLE_ID"
SCHEME="prism-$SLUG"
APP_NAME="$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}"
OUT="$ROOT/target/ios"
APP="$OUT/$APP_NAME.app"
APPEX="$APP/PlugIns/PrismWidgets.appex"
GEN="$OUT/generated"
mkdir -p "$GEN"

# 1. Pick and boot a simulator (slimmed with simslim when available).
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

# 2. Describe the app's widgets (host build) and generate the extension entry point.
cargo build -q -p "$PKG"
PRISM_DESCRIBE=widgets "$ROOT/target/debug/$PKG" > "$GEN/widgets.json"
python3 "$ROOT/platform/apple/generate_widgets.py" "$GEN/widgets.json" ios > "$GEN/Generated.swift"

# Entitlements: App Group shared by the app and its widget extension.
cat > "$GEN/entitlements.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.application-groups</key><array><string>$GROUP</string></array>
</dict>
</plist>
PLIST
# The simulator reads them from a section of each binary (XML and DER).
if xcrun -f derq >/dev/null 2>&1; then
  xcrun derq query -f xml -i "$GEN/entitlements.plist" -o "$GEN/entitlements.plist.der" --raw
fi
export PRISM_ENTITLEMENTS="$GEN/entitlements.plist"

# 3. Build the app (links the Swift bridge, see crates/prism-apple-build).
cargo build -p "$PKG" --target "$TARGET"

# 4. Bundle.
rm -rf "$APP" && mkdir -p "$APPEX"
cp "$ROOT/target/$TARGET/debug/$PKG" "$APP/$PKG"
ASSETS="$(PRISM_DESCRIBE=assets "$ROOT/target/debug/$PKG")"
[[ -n "$ASSETS" && -d "$ASSETS" ]] && cp -R "$ASSETS" "$APP/assets"

plist_common() { # $1 executable, $2 bundle id, $3 package type
  cat <<PLIST
  <key>CFBundleExecutable</key><string>$1</string>
  <key>CFBundleIdentifier</key><string>$2</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>$3</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>DTPlatformName</key><string>iphonesimulator</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneSimulator</string></array>
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

# 5. Widget extension: Prism's generic SwiftUI renderer + the generated entry point.
xcrun --sdk iphonesimulator swiftc -target "arm64-apple-ios$MIN_IOS-simulator" -swift-version 5 -O \
  -parse-as-library -application-extension -module-name PrismWidgets \
  -o "$APPEX/PrismWidgets" \
  -Xlinker -e -Xlinker _NSExtensionMain \
  -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __entitlements -Xlinker "$GEN/entitlements.plist" \
  $( [[ -f "$GEN/entitlements.plist.der" ]] && echo "-Xlinker -sectcreate -Xlinker __TEXT -Xlinker __ents_der -Xlinker $GEN/entitlements.plist.der" ) \
  "$ROOT"/platform/apple/Shared/*.swift "$ROOT"/platform/apple/Extension/*.swift "$GEN/Generated.swift"
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

# 6. Sign (ad-hoc) with the App Group shared by app and extension.
codesign --force --sign - --entitlements "$GEN/entitlements.plist" "$APPEX" >/dev/null
codesign --force --sign - --entitlements "$GEN/entitlements.plist" "$APP" >/dev/null

# 7. Install and launch.
xcrun simctl install "$UDID" "$APP"
echo "Installed $BUNDLE_ID (+ widgets, Live Activities) on $UDID"
if [[ $LAUNCH == 1 ]]; then
  open -a Simulator --args -CurrentDeviceUDID "$UDID"
  xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE_ID"
fi
