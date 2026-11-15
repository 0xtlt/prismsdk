#!/usr/bin/env bash
# Bundle a Prism app as a real macOS .app — with its WidgetKit extension,
# notifications and deep links — then open it.
#
#   scripts/macos-app.sh <package> [--no-open]
#
# Environment: PRISM_BUNDLE_ID, PRISM_APP_NAME, PRISM_ICON (1024×1024 PNG),
# PRISM_IDENTITY (default: ad hoc), PRISM_RELEASE=1, PRISM_PROJECT, PRISM_PLATFORM,
# PRISM_BUILD (CFBundleVersion), PRISM_CATEGORY (LSApplicationCategoryType),
# PRISM_ENCRYPTION=0 (ITSAppUsesNonExemptEncryption = NO).
#
# Distribution (PRISM_DIST):
#   appstore      sandboxed, signed with "Apple Distribution" (or "3rd Party Mac
#                 Developer Application") and Mac App Store profiles
#                 (PRISM_MAC_PROFILE, PRISM_MAC_WIDGETS_PROFILE: default, the best
#                 match), packaged by productbuild into a .pkg signed with
#                 PRISM_INSTALLER_IDENTITY ("3rd Party Mac Developer Installer";
#                 "-" leaves it unsigned, for tests)
#   developer-id  signed with "Developer ID Application", hardened runtime and a
#                 secure timestamp, then a signed .dmg (notarize it next)
#   PRISM_KEYCHAIN    keychain holding the identities (default: the search list)
#   PRISM_TIMESTAMP=none  no secure timestamp (offline tests only)
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
DIST="${PRISM_DIST:-}"
KEYCHAIN_ARGS=()
[[ -n "${PRISM_KEYCHAIN:-}" ]] && KEYCHAIN_ARGS=(--keychain "$PRISM_KEYCHAIN")
# The first valid identity whose name starts with one of the arguments.
identity() {
  local out
  out="$(security find-identity -v -p codesigning ${PRISM_KEYCHAIN:+"$PRISM_KEYCHAIN"} 2>/dev/null; \
    security find-identity -v ${PRISM_KEYCHAIN:+"$PRISM_KEYCHAIN"} 2>/dev/null)"
  for prefix in "$@"; do
    name="$(sed -n "s/.*\"\($prefix[^\"]*\)\".*/\1/p" <<< "$out" | head -1)"
    [[ -n "$name" ]] && { echo "$name"; return; }
  done
}
# The best Mac provisioning profile for a bundle id (App Store: no devices).
find_profile() { # $1 bundle id
  BUNDLE_ID="$1" TEAM="${PRISM_TEAM:-}" python3 -c '
import glob, os, plistlib, subprocess, datetime
bid, team = os.environ["BUNDLE_ID"], os.environ["TEAM"]
home = os.path.expanduser("~")
best = None
for f in glob.glob(home + "/Library/Developer/Xcode/UserData/Provisioning Profiles/*.provisionprofile") + \
         glob.glob(home + "/Library/MobileDevice/Provisioning Profiles/*.provisionprofile"):
    raw = subprocess.run(["security", "cms", "-D", "-i", f], capture_output=True).stdout
    try: p = plistlib.loads(raw)
    except Exception: continue
    if p["ExpirationDate"] < datetime.datetime.now() or p.get("ProvisionedDevices") or p.get("ProvisionsAllDevices"): continue
    if team and p["TeamIdentifier"][0] != team: continue
    app_id = p["Entitlements"].get("com.apple.application-identifier", "").split(".", 1)[-1]
    score = 2 if app_id == bid else 1 if app_id == "*" or (app_id.endswith("*") and bid.startswith(app_id[:-1])) else 0
    if score and (best is None or score > best[0]): best = (score, f)
print(best[1] if best else "")
'
}
TEAM="${PRISM_TEAM:-}"
case "$DIST" in
  "") ;;
  appstore)
    SIGN="${PRISM_IDENTITY:-$(identity "Apple Distribution" "3rd Party Mac Developer Application")}"
    INSTALLER="${PRISM_INSTALLER_IDENTITY:-$(identity "3rd Party Mac Developer Installer" "Mac Installer Distribution")}"
    [[ -n "$SIGN" ]] || { echo "No \"Apple Distribution\" identity in the keychain (or set PRISM_IDENTITY)" >&2; exit 1; }
    [[ -n "$INSTALLER" ]] || { echo "No \"3rd Party Mac Developer Installer\" identity in the keychain (or set PRISM_INSTALLER_IDENTITY)" >&2; exit 1; }
    PROFILE="${PRISM_MAC_PROFILE:-$(find_profile "$BUNDLE_ID")}"
    WIDGETS_PROFILE="${PRISM_MAC_WIDGETS_PROFILE:-$(find_profile "$BUNDLE_ID.widgets")}"
    [[ -n "$PROFILE" ]] || { echo "No Mac App Store provisioning profile for $BUNDLE_ID (set PRISM_MAC_PROFILE)" >&2; exit 1; }
    [[ -n "$WIDGETS_PROFILE" ]] || { echo "No Mac App Store provisioning profile for $BUNDLE_ID.widgets (set PRISM_MAC_WIDGETS_PROFILE)" >&2; exit 1; }
    TEAM="$(security cms -D -i "$PROFILE" | plutil -extract TeamIdentifier.0 raw -)" ;;
  developer-id)
    SIGN="${PRISM_IDENTITY:-$(identity "Developer ID Application")}"
    [[ -n "$SIGN" ]] || { echo "No \"Developer ID Application\" identity in the keychain (or set PRISM_IDENTITY)" >&2; exit 1; }
    # "Developer ID Application: Name (TEAMID)"
    [[ -z "$TEAM" && "$SIGN" =~ \(([A-Z0-9]+)\)$ ]] && TEAM="${BASH_REMATCH[1]}"
    [[ -n "$TEAM" ]] || { echo "Set PRISM_TEAM (the Apple team id): $SIGN does not name one" >&2; exit 1; } ;;
  *) echo "PRISM_DIST=$DIST: use appstore or developer-id" >&2; exit 1 ;;
esac
# Signed outside the App Store, an app group must start with the team id.
[[ -n "$DIST" ]] && GROUP="$TEAM.$BUNDLE_ID"
TIMESTAMP=(--timestamp=none)
[[ "$DIST" == developer-id && "${PRISM_TIMESTAMP:-}" != none ]] && TIMESTAMP=(--timestamp)
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
  <key>CFBundleSupportedPlatforms</key><array><string>MacOSX</string></array>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
$BUILD_PLIST
  <key>PrismAppGroup</key><string>$GROUP</string>
  <key>PrismURLScheme</key><string>$SCHEME</string>
$4
</dict>
</plist>
PLIST
}

RESOURCES="$APP/Contents/Resources"
# What App Store Connect reads: the SDK and Xcode that built the app.
BUILD_PLIST=""
if [[ -n "$DIST" ]]; then
  SDK_VERSION="$(xcrun --sdk macosx --show-sdk-version)"
  SDK_BUILD="$(xcrun --sdk macosx --show-sdk-build-version)"
  BUILD_PLIST="  <key>DTSDKName</key><string>macosx$SDK_VERSION</string>
  <key>DTSDKBuild</key><string>$SDK_BUILD</string>
  <key>DTPlatformName</key><string>macosx</string>
  <key>DTPlatformVersion</key><string>$SDK_VERSION</string>
  <key>DTPlatformBuild</key><string>$SDK_BUILD</string>
  <key>DTXcode</key><string>$(xcodebuild -version | awk 'NR==1 {split($2, v, "."); printf "%d%d%d", v[1], v[2], v[3]}')</string>
  <key>DTXcodeBuild</key><string>$(xcodebuild -version | awk '/Build version/ {print $3}')</string>
  <key>DTCompiler</key><string>com.apple.compilers.llvm.clang.1_0</string>
  <key>BuildMachineOSBuild</key><string>$(sw_vers -buildVersion)</string>"
fi
STORE_PLIST=""
if [[ -n "${PRISM_CATEGORY:-}" ]]; then
  STORE_PLIST+="  <key>LSApplicationCategoryType</key><string>$PRISM_CATEGORY</string>"
elif [[ "$DIST" == appstore ]]; then
  echo "note: no category (Prism.toml [app] category): using public.app-category.utilities" >&2
  STORE_PLIST+="  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>"
fi
[[ "${PRISM_ENCRYPTION:-}" == 0 ]] && STORE_PLIST+="
  <key>ITSAppUsesNonExemptEncryption</key><false/>"
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
$STORE_PLIST
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

# Entitlements. Widget extensions must be sandboxed; app and extension share
# the App Group. Store builds are sandboxed too, with what Prism.toml's
# [permissions] ask for (also needed under the hardened runtime), and carry
# the app id their profile grants.
[[ "$DIST" == appstore ]] && security cms -D -i "$PROFILE" > "$GEN/profile.plist" \
  && security cms -D -i "$WIDGETS_PROFILE" > "$GEN/widgets.profile.plist"
DIST="$DIST" GROUP="$GROUP" GEN="$GEN" ICLOUD="${PRISM_ICLOUD:-}" python3 <<'PY'
import os, plistlib
dist, group, gen = os.environ["DIST"], os.environ["GROUP"], os.environ["GEN"]
by_permission = {
    "camera": ["com.apple.security.device.camera"],
    "microphone": ["com.apple.security.device.audio-input"],
    "location": ["com.apple.security.personal-information.location"],
    "location_always": ["com.apple.security.personal-information.location"],
    "photos": ["com.apple.security.personal-information.photos-library"],
}
asked = [l.partition("\t")[0] for l in os.environ.get("PRISM_PERMISSIONS", "").splitlines()]
def profile(name):
    path = os.path.join(gen, name)
    return plistlib.load(open(path, "rb"))["Entitlements"] if dist == "appstore" else {}
def granted(p, e):
    for key in ("com.apple.application-identifier", "com.apple.developer.team-identifier"):
        if key in p:
            e[key] = p[key]
app = {"com.apple.security.application-groups": [group]}
appex = {"com.apple.security.app-sandbox": True, "com.apple.security.application-groups": [group]}
if dist:
    for name in asked:
        for key in by_permission.get(name, []):
            app[key] = True
if dist == "appstore":
    app.update({
        "com.apple.security.app-sandbox": True,
        "com.apple.security.network.client": True,
        "com.apple.security.files.user-selected.read-write": True,
    })
    p = profile("profile.plist")
    granted(p, app)
    kv = "com.apple.developer.ubiquity-kvstore-identifier"
    if os.environ.get("ICLOUD") == "1" and kv in p:
        app[kv] = p[kv]
    granted(profile("widgets.profile.plist"), appex)
plistlib.dump(app, open(os.path.join(gen, "app.entitlements"), "wb"))
plistlib.dump(appex, open(os.path.join(gen, "appex.entitlements"), "wb"))
PY
SIGN_ARGS=(--force --sign "$SIGN" ${KEYCHAIN_ARGS[@]+"${KEYCHAIN_ARGS[@]}"})
# Distribution builds run under the hardened runtime (notarization needs it).
[[ -n "$DIST" ]] && SIGN_ARGS+=(--options runtime "${TIMESTAMP[@]}")
if [[ "$DIST" == appstore ]]; then
  cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
  cp "$WIDGETS_PROFILE" "$APPEX/Contents/embedded.provisionprofile"
fi
codesign "${SIGN_ARGS[@]}" --entitlements "$GEN/appex.entitlements" "$APPEX"
codesign "${SIGN_ARGS[@]}" --entitlements "$GEN/app.entitlements" "$APP"
[[ -n "$DIST" ]] && codesign --verify --deep --strict "$APP"
echo "Built $APP (widgets: $(python3 -c "import json;print(len(json.load(open('$GEN/widgets.json'))))"))"
if [[ "$DIST" == appstore ]]; then
  # The Mac App Store takes a signed product archive installing into /Applications.
  PKG_OUT="$OUT/$APP_NAME.pkg"
  rm -f "$PKG_OUT"
  SIGN_PKG=(--sign "$INSTALLER" ${KEYCHAIN_ARGS[@]+"${KEYCHAIN_ARGS[@]}"} "${TIMESTAMP[@]}")
  # "-": unsigned (productbuild only signs with an identity the system trusts:
  # scripts/test-signing.sh has none).
  if [[ "$INSTALLER" == - ]]; then
    SIGN_PKG=()
    echo "note: the package is not signed (PRISM_INSTALLER_IDENTITY=-)" >&2
  fi
  productbuild --component "$APP" /Applications ${SIGN_PKG[@]+"${SIGN_PKG[@]}"} --quiet "$PKG_OUT"
  echo "Built $PKG_OUT"
  exit 0
elif [[ "$DIST" == developer-id ]]; then
  # A disk image to drag the app into Applications, signed like the app.
  DMG="$OUT/$APP_NAME.dmg"
  rm -rf "$OUT/dmg" "$DMG" && mkdir -p "$OUT/dmg"
  cp -R "$APP" "$OUT/dmg/"
  ln -s /Applications "$OUT/dmg/Applications"
  hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$OUT/dmg" -fs HFS+ -format UDZO -ov "$DMG"
  rm -rf "$OUT/dmg"
  codesign --force --sign "$SIGN" ${KEYCHAIN_ARGS[@]+"${KEYCHAIN_ARGS[@]}"} "${TIMESTAMP[@]}" "$DMG"
  echo "Built $DMG"
  exit 0
fi
[[ "${2:-}" == "--no-open" ]] || open "$APP"
