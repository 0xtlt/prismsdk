#!/usr/bin/env bash
# Compiles a 1024×1024 PNG into an app icon (asset catalog) for a bundle.
#
#   app-icon.sh <icon.png> <sdk> <min-os> <resources-dir> <work-dir>
#
# <sdk>: iphoneos, iphonesimulator, macosx, appletvos, appletvsimulator,
# watchos, watchsimulator, xros, xrsimulator.
#
# Writes Assets.car (+ fallback PNGs) into <resources-dir> and the Info.plist
# keys to add into <work-dir>/icon-info.xml.
#
# - iOS, watchOS: one 1024 px image (Xcode's single size).
# - macOS: every size of the classic icon set.
# - visionOS: a solid image stack (front and back layers, 1024 px).
# - tvOS: the brand assets the App Store requires: layered app icons
#   (400×240 @1x/@2x, 1280×768) and top shelf images (1920×720, 2320×720),
#   the icon filling each (centered, cropped).
set -euo pipefail
ICON="$1" SDK="$2" MIN="$3" RES="$4" WORK="$5"
CAT="$WORK/Icon.xcassets"
INFO='"info":{"author":"prism","version":1}'
rm -rf "$CAT" && mkdir -p "$CAT"
echo "{$INFO}" > "$CAT/Contents.json"
APP_ICON=AppIcon

# The icon filling w×h: scaled to cover, centered, cropped.
cover() { # out w h
  local big=$(( $2 > $3 ? $2 : $3 ))
  sips -z "$big" "$big" "$ICON" --out "$1" >/dev/null
  [[ $2 == "$3" ]] || sips --cropToHeightWidth "$3" "$2" "$1" >/dev/null
}

case "$SDK" in
  macosx)
    SET="$CAT/AppIcon.appiconset" && mkdir -p "$SET"
    images=""
    for s in 16 32 128 256 512; do
      for scale in 1 2; do
        px=$((s * scale)) name="icon_${s}x${s}@${scale}x.png"
        sips -z "$px" "$px" "$ICON" --out "$SET/$name" >/dev/null
        images+="{\"idiom\":\"mac\",\"size\":\"${s}x${s}\",\"scale\":\"${scale}x\",\"filename\":\"$name\"},"
      done
    done
    echo "{\"images\":[${images%,}],$INFO}" > "$SET/Contents.json"
    DEVICES=() ;;
  watchos | watchsimulator)
    SET="$CAT/AppIcon.appiconset" && mkdir -p "$SET"
    sips -z 1024 1024 "$ICON" --out "$SET/icon-1024.png" >/dev/null
    echo "{\"images\":[{\"idiom\":\"universal\",\"platform\":\"watchos\",\"size\":\"1024x1024\",\"filename\":\"icon-1024.png\"}],$INFO}" > "$SET/Contents.json"
    DEVICES=(--target-device watch) ;;
  xros | xrsimulator)
    STACK="$CAT/AppIcon.solidimagestack"
    mkdir -p "$STACK"
    echo "{\"layers\":[{\"filename\":\"Front.solidimagestacklayer\"},{\"filename\":\"Back.solidimagestacklayer\"}],$INFO}" > "$STACK/Contents.json"
    for layer in Front Back; do
      SET="$STACK/$layer.solidimagestacklayer/Content.imageset"
      mkdir -p "$SET"
      echo "{$INFO}" > "$STACK/$layer.solidimagestacklayer/Contents.json"
      sips -z 1024 1024 "$ICON" --out "$SET/icon.png" >/dev/null
      echo "{\"images\":[{\"idiom\":\"vision\",\"scale\":\"2x\",\"filename\":\"icon.png\"}],$INFO}" > "$SET/Contents.json"
    done
    DEVICES=(--target-device vision) ;;
  appletvos | appletvsimulator)
    APP_ICON="App Icon & Top Shelf Image"
    BRAND="$CAT/$APP_ICON.brandassets"
    mkdir -p "$BRAND"
    stack() { # name w h scales…
      local dir="$BRAND/$1.imagestack" w=$2 h=$3
      shift 3
      mkdir -p "$dir"
      echo "{\"layers\":[{\"filename\":\"Front.imagestacklayer\"},{\"filename\":\"Back.imagestacklayer\"}],$INFO}" > "$dir/Contents.json"
      for layer in Front Back; do
        local set="$dir/$layer.imagestacklayer/Content.imageset" images=""
        mkdir -p "$set"
        echo "{$INFO}" > "$dir/$layer.imagestacklayer/Contents.json"
        for s in "$@"; do
          cover "$set/icon@${s}x.png" $((w * s)) $((h * s))
          images+="{\"idiom\":\"tv\",\"scale\":\"${s}x\",\"filename\":\"icon@${s}x.png\"},"
        done
        echo "{\"images\":[${images%,}],$INFO}" > "$set/Contents.json"
      done
    }
    shelf() { # name w h
      local set="$BRAND/$1.imageset" images=""
      mkdir -p "$set"
      for s in 1 2; do
        cover "$set/shelf@${s}x.png" $(($2 * s)) $(($3 * s))
        images+="{\"idiom\":\"tv\",\"scale\":\"${s}x\",\"filename\":\"shelf@${s}x.png\"},"
      done
      echo "{\"images\":[${images%,}],$INFO}" > "$set/Contents.json"
    }
    stack "App Icon - App Store" 1280 768 1
    stack "App Icon" 400 240 1 2
    shelf "Top Shelf Image" 1920 720
    shelf "Top Shelf Image Wide" 2320 720
    cat > "$BRAND/Contents.json" <<JSON
{"assets":[
  {"filename":"App Icon - App Store.imagestack","idiom":"tv","role":"primary-app-icon","size":"1280x768"},
  {"filename":"App Icon.imagestack","idiom":"tv","role":"primary-app-icon","size":"400x240"},
  {"filename":"Top Shelf Image Wide.imageset","idiom":"tv","role":"top-shelf-image-wide","size":"2320x720"},
  {"filename":"Top Shelf Image.imageset","idiom":"tv","role":"top-shelf-image","size":"1920x720"}
],$INFO}
JSON
    DEVICES=(--target-device tv) ;;
  *)
    SET="$CAT/AppIcon.appiconset" && mkdir -p "$SET"
    sips -z 1024 1024 "$ICON" --out "$SET/icon-1024.png" >/dev/null
    echo "{\"images\":[{\"idiom\":\"universal\",\"platform\":\"ios\",\"size\":\"1024x1024\",\"filename\":\"icon-1024.png\"}],$INFO}" > "$SET/Contents.json"
    DEVICES=(--target-device iphone --target-device ipad) ;;
esac
mkdir -p "$RES"
xcrun actool --compile "$RES" --platform "$SDK" --minimum-deployment-target "$MIN" \
  --app-icon "$APP_ICON" --output-partial-info-plist "$WORK/icon-partial.plist" \
  ${DEVICES[@]+"${DEVICES[@]}"} --output-format human-readable-text --notices --warnings \
  "$CAT" > "$WORK/actool.log"
# The keys actool asks for (CFBundleIcons, TVTopShelfImage…), as plist XML to
# paste into a <dict>.
python3 - "$WORK/icon-partial.plist" > "$WORK/icon-info.xml" <<'PY'
import plistlib, sys, re
xml = plistlib.dumps(plistlib.load(open(sys.argv[1], "rb"))).decode()
print(re.search(r"<dict>(.*)</dict>", xml, re.S).group(1))
PY
