#!/usr/bin/env bash
# Compiles a 1024×1024 PNG into an app icon (asset catalog) for a bundle.
#
#   app-icon.sh <icon.png> <iphoneos|iphonesimulator|macosx> <min-os> <resources-dir> <work-dir>
#
# Writes Assets.car (+ fallback PNGs) into <resources-dir> and the Info.plist
# keys to add into <work-dir>/icon-info.xml.
set -euo pipefail
ICON="$1" SDK="$2" MIN="$3" RES="$4" WORK="$5"
SET="$WORK/Icon.xcassets/AppIcon.appiconset"
rm -rf "$WORK/Icon.xcassets" && mkdir -p "$SET"
echo '{"info":{"author":"prism","version":1}}' > "$WORK/Icon.xcassets/Contents.json"
if [[ "$SDK" == macosx ]]; then
  images=""
  for s in 16 32 128 256 512; do
    for scale in 1 2; do
      px=$((s * scale)) name="icon_${s}x${s}@${scale}x.png"
      sips -z "$px" "$px" "$ICON" --out "$SET/$name" >/dev/null
      images+="{\"idiom\":\"mac\",\"size\":\"${s}x${s}\",\"scale\":\"${scale}x\",\"filename\":\"$name\"},"
    done
  done
  echo "{\"images\":[${images%,}],\"info\":{\"author\":\"prism\",\"version\":1}}" > "$SET/Contents.json"
  DEVICES=()
else
  sips -z 1024 1024 "$ICON" --out "$SET/icon-1024.png" >/dev/null
  echo '{"images":[{"idiom":"universal","platform":"ios","size":"1024x1024","filename":"icon-1024.png"}],"info":{"author":"prism","version":1}}' > "$SET/Contents.json"
  DEVICES=(--target-device iphone --target-device ipad)
fi
mkdir -p "$RES"
xcrun actool --compile "$RES" --platform "$SDK" --minimum-deployment-target "$MIN" \
  --app-icon AppIcon --output-partial-info-plist "$WORK/icon-partial.plist" \
  ${DEVICES[@]+"${DEVICES[@]}"} --output-format human-readable-text --notices --warnings \
  "$WORK/Icon.xcassets" > "$WORK/actool.log"
# The keys actool asks for (CFBundleIcons…), as plist XML to paste into a <dict>.
python3 - "$WORK/icon-partial.plist" > "$WORK/icon-info.xml" <<'PY'
import plistlib, sys, re
xml = plistlib.dumps(plistlib.load(open(sys.argv[1], "rb"))).decode()
print(re.search(r"<dict>(.*)</dict>", xml, re.S).group(1))
PY
