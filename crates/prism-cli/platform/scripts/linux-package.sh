#!/usr/bin/env bash
# Build a Prism app for Linux (GTK 4, release) and package it as a .deb:
# the binary and its assets in /usr/lib/<name>, a launcher in /usr/bin, a
# desktop entry, an icon and AppStream metadata. Dependencies come from
# dpkg-shlibdeps. Runs on Debian/Ubuntu (or `prism build linux` runs it in
# the docker/linux-dev.Dockerfile image).
#
#   scripts/linux-package.sh <package>
#
# Environment: PRISM_PROJECT, PRISM_BUNDLE_ID, PRISM_APP_NAME, PRISM_VERSION,
# PRISM_BUILD (Debian revision, default 1), PRISM_ICON, PRISM_DESCRIPTION,
# PRISM_MAINTAINER, PRISM_LINUX_OUT (default <target>/linux).
#
# The package and bundle ID become file names and launcher source, so they
# are checked against narrow grammars before anything runs; names,
# descriptions and the maintainer are one line each, escaped per format.
set -euo pipefail
# Character classes and ranges in the checks below mean ASCII.
export LC_ALL=C

fail() { echo "linux-package.sh: $*" >&2; exit 1; }
matches() { [[ "$1" =~ $2 ]]; }
# `$2` as one line of text: no line breaks or other control characters.
one_line() { [[ "$2" != *[[:cntrl:]]* ]] || fail "$1 must be one line, without control characters"; }
# `$1` as a single-quoted sh word.
sq() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
# `$1` as a desktop entry string value (`\` escapes).
desktop() { printf '%s' "$1" | sed 's/\\/\\\\/g'; }

PKG="${1:?usage: linux-package.sh <package>}"
matches "$PKG" '^[A-Za-z0-9_][A-Za-z0-9_-]*$' ||
  fail "package $PKG: letters, digits, _ and - only"
ROOT="${PRISM_PROJECT:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$ROOT"
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
OUT="${PRISM_LINUX_OUT:-$TARGET_DIR/linux}"

SLUG="$(tr '[:upper:]_' '[:lower:]-' <<< "$PKG")"
APP_ID="${PRISM_BUNDLE_ID:-dev.prism.$SLUG}"
APP_NAME="${PRISM_APP_NAME:-$(tr '[:lower:]' '[:upper:]' <<< "${PKG:0:1}")${PKG:1}}"
DESCRIPTION="${PRISM_DESCRIPTION:-$APP_NAME, made with Prism}"
MAINTAINER="${PRISM_MAINTAINER:-$APP_NAME maintainers <noreply@example.com>}"
# A reverse-DNS ID (desktop entries, D-Bus): dot-separated words.
if (( ${#APP_ID} > 255 )) || ! matches "$APP_ID" '^[A-Za-z0-9_-]+([.][A-Za-z0-9_-]+)+$'; then
  fail "bundle ID $APP_ID: dot-separated words of letters, digits, _ and - (com.example.app)"
fi
one_line "app name" "$APP_NAME"
one_line "description" "$DESCRIPTION"
one_line "maintainer" "$MAINTAINER"
# Debian versions: <upstream>-<revision>, upstream starting with a digit.
UPSTREAM="${PRISM_VERSION:-0.1.0}"
UPSTREAM="${UPSTREAM//_/\~}"
matches "$UPSTREAM" '^[0-9][A-Za-z0-9.+~]*$' || fail "version $UPSTREAM: Debian versions start with a digit"
REVISION="${PRISM_BUILD:-1}"
matches "$REVISION" '^[A-Za-z0-9.+~]+$' || fail "build $REVISION: letters, digits, ., + and ~ only"
VERSION="$UPSTREAM-$REVISION"
ARCH="$(dpkg --print-architecture)"

# 1. The app (release) and what it declares (assets).
cargo build --release -p "$PKG"
BIN="$TARGET_DIR/release/$PKG"
ASSETS="$(PRISM_DESCRIBE=assets "$BIN" 2>/dev/null || true)"

# 2. The file tree.
STAGE="$OUT/deb/${SLUG}_${VERSION}_$ARCH"
rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/lib/$SLUG" "$STAGE/usr/share/applications" \
  "$STAGE/usr/share/metainfo" "$STAGE/usr/share/pixmaps"
install -m 755 "$BIN" "$STAGE/usr/lib/$SLUG/$PKG"
[[ -n "$ASSETS" && -d "$ASSETS" ]] && cp -R "$ASSETS" "$STAGE/usr/lib/$SLUG/assets"
# The launcher holds identifiers only, each a quoted literal.
cat > "$STAGE/usr/bin/$SLUG" <<SH
#!/bin/sh
# Starts the app: it finds its pictures in its assets folder, and desktops
# match its window to its desktop entry.
if [ -z "\${PRISM_ASSETS_DIR:-}" ]; then PRISM_ASSETS_DIR=$(sq "/usr/lib/$SLUG/assets"); fi
export PRISM_ASSETS_DIR
export PRISM_APP_ID=$(sq "$APP_ID")
exec $(sq "/usr/lib/$SLUG/$PKG") "\$@"
SH
chmod 755 "$STAGE/usr/bin/$SLUG"
ICON_LINE=""
if [[ -n "${PRISM_ICON:-}" && -f "$PRISM_ICON" ]]; then
  install -m 644 "$PRISM_ICON" "$STAGE/usr/share/pixmaps/$APP_ID.png"
  ICON_LINE="Icon=$APP_ID"
fi
cat > "$STAGE/usr/share/applications/$APP_ID.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$(desktop "$APP_NAME")
Comment=$(desktop "$DESCRIPTION")
Exec=$SLUG
$ICON_LINE
Terminal=false
Categories=Utility;
StartupWMClass=$APP_ID
DESKTOP
xml() { python3 -c 'import sys; from xml.sax.saxutils import escape; print(escape(sys.argv[1]))' "$1"; }
cat > "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>$APP_ID</id>
  <name>$(xml "$APP_NAME")</name>
  <summary>$(xml "$DESCRIPTION")</summary>
  <metadata_license>CC0-1.0</metadata_license>
  <launchable type="desktop-id">$APP_ID.desktop</launchable>
  <provides><binary>$SLUG</binary></provides>
  <releases><release version="$UPSTREAM"/></releases>
</component>
XML
rmdir "$STAGE/usr/share/pixmaps" 2>/dev/null || true

# 3. Control file: dependencies from the libraries the binary links.
SHLIBS="$OUT/deb/shlibs"
rm -rf "$SHLIBS" && mkdir -p "$SHLIBS/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$SLUG" "$SLUG" > "$SHLIBS/debian/control"
DEPENDS="$(cd "$SHLIBS" && dpkg-shlibdeps -O -e "$STAGE/usr/lib/$SLUG/$PKG" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
rm -rf "$SHLIBS"
[[ -n "$DEPENDS" ]] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: $SLUG
Version: $VERSION
Architecture: $ARCH
Maintainer: $MAINTAINER
Installed-Size: $(du -sk "$STAGE/usr" | cut -f1)
Depends: $DEPENDS
Section: misc
Priority: optional
Description: $DESCRIPTION
 $APP_NAME ($APP_ID), a native GTK 4 app built with Prism.
CONTROL

DEB="$OUT/${SLUG}_${VERSION}_$ARCH.deb"
dpkg-deb --root-owner-group --build "$STAGE" "$DEB" >/dev/null
rm -rf "$OUT/deb"
echo "Built $DEB"
dpkg-deb --field "$DEB" Package Version Architecture Depends
