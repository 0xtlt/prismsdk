#!/usr/bin/env bash
# Bundle a Prism app as a real macOS .app — with its WidgetKit extension,
# notifications and deep links — then open it.
#
#   scripts/macos-app.sh <package> [--no-open]
set -euo pipefail

PKG="${1:?usage: macos-app.sh <package> [--no-open]}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SLUG="${PKG//_/-}"
BUNDLE_ID="dev.prism.$SLUG"
GROUP="group.$BUNDLE_ID"
SCHEME="prism-$SLUG"
APP_NAME="$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}"
OUT="$ROOT/target/macos"
APP="$OUT/$APP_NAME.app"
APPEX="$APP/Contents/PlugIns/PrismWidgets.appex"
GEN="$OUT/generated"
mkdir -p "$GEN"

cargo build -p "$PKG"
PRISM_DESCRIBE=widgets "$ROOT/target/debug/$PKG" > "$GEN/widgets.json"
python3 "$ROOT/platform/apple/generate_widgets.py" "$GEN/widgets.json" macos > "$GEN/Generated.swift"

rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APPEX/Contents/MacOS"
cp "$ROOT/target/debug/$PKG" "$APP/Contents/MacOS/$PKG"

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
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>PrismAppGroup</key><string>$GROUP</string>
  <key>PrismURLScheme</key><string>$SCHEME</string>
$4
</dict>
</plist>
PLIST
}

plist "$PKG" "$BUNDLE_ID" APPL "  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleURLTypes</key>
  <array><dict>
    <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
    <key>CFBundleURLSchemes</key><array><string>$SCHEME</string></array>
  </dict></array>" > "$APP/Contents/Info.plist"

xcrun --sdk macosx swiftc -target arm64-apple-macos14.0 -swift-version 5 -O \
  -parse-as-library -application-extension -module-name PrismWidgets \
  -Xlinker -e -Xlinker _NSExtensionMain \
  -o "$APPEX/Contents/MacOS/PrismWidgets" \
  "$ROOT"/platform/apple/Shared/*.swift "$ROOT"/platform/apple/Extension/*.swift "$GEN/Generated.swift"
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
codesign --force --sign - --entitlements "$GEN/appex.entitlements" "$APPEX"
codesign --force --sign - --entitlements "$GEN/app.entitlements" "$APP"
echo "Built $APP (widgets: $(python3 -c "import json;print(len(json.load(open('$GEN/widgets.json'))))"))"
[[ "${2:-}" == "--no-open" ]] || open "$APP"
