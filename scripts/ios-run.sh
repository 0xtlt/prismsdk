#!/usr/bin/env bash
# Build a Prism example for the iOS simulator, wrap it in a .app, install, launch.
#
#   scripts/ios-run.sh <package> [--no-launch]
#   PRISM_SIM="iPhone 17" (name or UDID, default: first available iPhone)
set -euo pipefail

PKG="${1:?usage: ios-run.sh <package> [--no-launch]}"
LAUNCH=1
[[ "${2:-}" == "--no-launch" ]] && LAUNCH=0

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET=aarch64-apple-ios-sim
BUNDLE_ID="dev.prism.${PKG//_/-}"
APP_NAME="$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}"
APP="$ROOT/target/ios/$APP_NAME.app"

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
  simslim on "$UDID"
fi
xcrun simctl bootstatus "$UDID" -b >/dev/null

# 2. Build.
cargo build -p "$PKG" --target "$TARGET"

# 3. Bundle.
rm -rf "$APP" && mkdir -p "$APP"
cp "$ROOT/target/$TARGET/debug/$PKG" "$APP/$PKG"
cat > "$APP/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>$PKG</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneSimulator</string></array>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>${IPHONEOS_DEPLOYMENT_TARGET:-17.0}</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UILaunchScreen</key><dict/>
  <key>UIApplicationSceneManifest</key>
  <dict>
    <key>UIApplicationSupportsMultipleScenes</key><false/>
    <key>UISceneConfigurations</key>
    <dict>
      <key>UIWindowSceneSessionRoleApplication</key>
      <array>
        <dict>
          <key>UISceneConfigurationName</key><string>Default</string>
          <key>UISceneDelegateClassName</key><string>PrismSceneDelegate</string>
        </dict>
      </array>
    </dict>
  </dict>
</dict>
</plist>
PLIST
codesign --force --sign - "$APP" >/dev/null

# 4. Install and launch.
xcrun simctl install "$UDID" "$APP"
echo "Installed $BUNDLE_ID on $UDID"
if [[ $LAUNCH == 1 ]]; then
  open -a Simulator --args -CurrentDeviceUDID "$UDID"
  xcrun simctl launch --terminate-running-process "$UDID" "$BUNDLE_ID"
fi
