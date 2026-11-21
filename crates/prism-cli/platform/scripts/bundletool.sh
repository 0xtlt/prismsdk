#!/usr/bin/env bash
# Runs Google's bundletool (Android App Bundles), fetched once from its GitHub
# release and checked against a pinned SHA-256: after the download and again
# before each run (the cache is shared by every project).
#
#   scripts/bundletool.sh build-bundle --modules=base.zip --output=app.aab
#   scripts/bundletool.sh --path      # prints the jar's path
#
# Environment:
#   PRISM_BUNDLETOOL  a bundletool-all jar to use instead (not checked)
set -euo pipefail

VERSION=1.18.3
# From the release's asset digest (gh api repos/google/bundletool/releases/latest).
SHA256=a099cfa1543f55593bc2ed16a70a7c67fe54b1747bb7301f37fdfd6d91028e29
URL="https://github.com/google/bundletool/releases/download/$VERSION/bundletool-all-$VERSION.jar"

JAR="${PRISM_BUNDLETOOL:-}"
if [[ -z "$JAR" ]]; then
  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/prism/bundletool"
  JAR="$CACHE/bundletool-all-$VERSION.jar"
  sha() { (sha256sum "$1" 2>/dev/null || shasum -a 256 "$1") | cut -d' ' -f1; }
  if [[ -f "$JAR" && "$(sha "$JAR")" != "$SHA256" ]]; then
    echo "bundletool: $JAR changed (checksum mismatch): fetching it again" >&2
    rm -f "$JAR"
  fi
  if [[ ! -f "$JAR" ]]; then
    mkdir -p "$CACHE"
    echo "Fetching bundletool $VERSION" >&2
    # A private file beside the jar, moved in place once checked.
    PART="$(mktemp "$CACHE/.part.XXXXXX")"
    trap 'rm -f "$PART"' EXIT
    curl -fsSL --retry 3 -o "$PART" "$URL"
    got="$(sha "$PART")"
    if [[ "$got" != "$SHA256" ]]; then
      echo "bundletool: checksum mismatch (got $got, want $SHA256)" >&2
      exit 1
    fi
    mv "$PART" "$JAR"
  fi
fi
if [[ "${1:-}" == --path ]]; then
  echo "$JAR"
  exit 0
fi
exec java -jar "$JAR" "$@"
