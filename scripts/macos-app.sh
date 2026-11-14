#!/usr/bin/env bash
# Bundle a Prism app as a real macOS .app — with its WidgetKit extension,
# notifications and deep links — then open it.
#
#   scripts/macos-app.sh <package> [--no-open]
#
# Environment: PRISM_BUNDLE_ID, PRISM_APP_NAME, PRISM_ICON (1024×1024 PNG),
# PRISM_IDENTITY (default: ad hoc), PRISM_RELEASE=1, PRISM_PROJECT, PRISM_PLATFORM.
set -euo pipefail

PKG="${1:?usage: macos-app.sh <package> [--no-open]}"
ROOT="${PRISM_PROJECT:-$(cd "$(dirname "$0")/.." && pwd)}"
PLATFORM="${PRISM_PLATFORM:-$ROOT/platform}"
cd "$ROOT"
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
PROFILE_DIR=debug CARGO_FLAGS=()
if [[ "${PRISM_RELEASE:-}" == 1 ]]; then PROFILE_DIR=release CARGO_FLAGS=(--release); fi
SLUG="${PKG//_/-}"
BUNDLE_ID="${PRISM_BUNDLE_ID:-dev.prism.$SLUG}"
GROUP="group.$BUNDLE_ID"
SCHEME="prism-$SLUG"
APP_NAME="${PRISM_APP_NAME:-$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}}"
SIGN="${PRISM_IDENTITY:--}"
OUT="$TARGET_DIR/macos"
APP="$OUT/$APP_NAME.app"
APPEX="$APP/Contents/PlugIns/PrismWidgets.appex"
GEN="$OUT/generated"
mkdir -p "$GEN"

cargo build -p "$PKG" ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}
BIN="$TARGET_DIR/$PROFILE_DIR/$PKG"
PRISM_DESCRIBE=widgets "$BIN" > "$GEN/widgets.json"
python3 "$PLATFORM/apple/generate_widgets.py" "$GEN/widgets.json" macos > "$GEN/Generated.swift"

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APPEX/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/$PKG"
ASSETS="$(PRISM_DESCRIBE=assets "$BIN")"
[[ -n "$ASSETS" && -d "$ASSETS" ]] && mkdir -p "$APP/Contents/Resources" && cp -R "$ASSETS" "$APP/Contents/Resources/assets"

plist() { # $1 executable, $2 bundle id, $3 package type, $4 extra
  cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>$1</string>
  <key>CFBundleIdentifier</key><string>$2</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundlePackageType</key><string>$3</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>${PRISM_VERSION:-0.1.0}</string>
  <key>CFBundleVersion</key><string>${PRISM_BUILD:-1}</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>PrismAppGroup</key><string>$GROUP</string>
  <key>PrismURLScheme</key><string>$SCHEME</string>
$4
</dict>
</plist>
PLIST
}

RESOURCES="$APP/Contents/Resources"
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
USAGE_PLIST="$(PLATFORM_KIND=macos python3 -c '
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
  bash "$PLATFORM/apple/app-icon.sh" "$PRISM_ICON" macosx 14.0 "$APP/Contents/Resources" "$GEN"
  ICON_PLIST="$(cat "$GEN/icon-info.xml")"
fi

# iCloud on macOS needs a provisioning profile (ad hoc signed apps with the
# entitlement do not launch): `cloud_storage` stays local otherwise.
if [[ "${PRISM_ICLOUD:-}" == 1 && -z "${PRISM_MAC_PROFILE:-}" ]]; then
  echo "note: iCloud on macOS needs PRISM_MAC_PROFILE (a Mac provisioning profile); cloud_storage stays local" >&2
fi
plist "$PKG" "$BUNDLE_ID" APPL "$ICON_PLIST
$LOC_PLIST
$USAGE_PLIST
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
    <key>CFBundleURLSchemes</key><array><string>$SCHEME</string></array>
  </dict></array>" > "$APP/Contents/Info.plist"

xcrun --sdk macosx swiftc -target arm64-apple-macos14.0 -swift-version 5 -O \
  -parse-as-library -application-extension -module-name PrismWidgets \
  -Xlinker -e -Xlinker _NSExtensionMain \
  -o "$APPEX/Contents/MacOS/PrismWidgets" \
  "$PLATFORM"/apple/Shared/*.swift "$PLATFORM"/apple/Extension/*.swift "$GEN/Generated.swift"
plist PrismWidgets "$BUNDLE_ID.widgets" XPC! "  <key>NSExtension</key>
  <dict><key>NSExtensionPointIdentifier</key><string>com.apple.widgetkit-extension</string></dict>" \
  > "$APPEX/Contents/Info.plist"

# Widget extensions must be sandboxed; app and extension share the App Group.
cat > "$GEN/app.entitlements" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.application-groups</key><array><string>$GROUP</string></array>
</dict></plist>
PLIST
cat > "$GEN/appex.entitlements" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.app-sandbox</key><true/>
  <key>com.apple.security.application-groups</key><array><string>$GROUP</string></array>
</dict></plist>
PLIST
codesign --force --sign "$SIGN" --entitlements "$GEN/appex.entitlements" "$APPEX"
codesign --force --sign "$SIGN" --entitlements "$GEN/app.entitlements" "$APP"
echo "Built $APP (widgets: $(python3 -c "import json;print(len(json.load(open('$GEN/widgets.json'))))"))"
[[ "${2:-}" == "--no-open" ]] || open "$APP"
