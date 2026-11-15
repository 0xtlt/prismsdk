#!/usr/bin/env bash
# Throwaway signing material to exercise the store builds without an Apple
# account: a temporary keychain with self-signed identities, and provisioning
# profiles signed by them (CMS, like Apple's). Nothing here is trusted by
# Apple: the outputs are for checking the packaging, never for uploading.
#
#   eval "$(scripts/test-signing.sh <dir> <bundle id>)"
#   prism build ios --store && prism publish ios --dry-run
#
# Prints the PRISM_* variables to export. The keychain is not added to the
# search list (codesign and productbuild get --keychain).
set -euo pipefail
DIR="${1:?usage: test-signing.sh <dir> <bundle id>}"
BUNDLE_ID="${2:?usage: test-signing.sh <dir> <bundle id>}"
TEAM=PRISMTEST1
mkdir -p "$DIR"
DIR="$(cd "$DIR" && pwd)"
KEYCHAIN="$DIR/prism-test.keychain-db"
PASS=prism-test

# One self-signed certificate per role, with its key usage.
identity() { # $1 name, $2 file stem, $3 extended key usage
  cat > "$DIR/$2.cnf" <<CNF
[req]
distinguished_name = dn
prompt = no
[dn]
CN = $1
OU = $TEAM
O = Prism Test
[ext]
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, $3
basicConstraints = critical, CA:false
CNF
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$DIR/$2.key" -out "$DIR/$2.pem" -days 7 \
    -config "$DIR/$2.cnf" -extensions ext 2>/dev/null
  # -legacy: the PKCS#12 encryption `security import` reads (OpenSSL 3).
  openssl pkcs12 -export -legacy -inkey "$DIR/$2.key" -in "$DIR/$2.pem" -out "$DIR/$2.p12" \
    -passout "pass:$PASS" 2>/dev/null \
    || openssl pkcs12 -export -inkey "$DIR/$2.key" -in "$DIR/$2.pem" -out "$DIR/$2.p12" -passout "pass:$PASS"
  security import "$DIR/$2.p12" -k "$KEYCHAIN" -P "$PASS" -T /usr/bin/codesign -T /usr/bin/productbuild >/dev/null
}

# A profile: the plist Apple would sign, signed by the test identity.
profile() { # $1 output, $2 app id key, $3 bundle id, $4 extra entitlements XML
  cat > "$DIR/profile.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Name</key><string>Prism Test $3</string>
  <key>TeamIdentifier</key><array><string>$TEAM</string></array>
  <key>TeamName</key><string>Prism Test</string>
  <key>ExpirationDate</key><date>2099-01-01T00:00:00Z</date>
  <key>Entitlements</key><dict>
    <key>$2</key><string>$TEAM.$3</string>
    <key>com.apple.developer.team-identifier</key><string>$TEAM</string>
    $4
  </dict>
</dict></plist>
PLIST
  openssl cms -sign -in "$DIR/profile.plist" -signer "$DIR/distribution.pem" -inkey "$DIST_KEY" \
    -outform DER -nodetach -binary -out "$1"
}

rm -f "$KEYCHAIN"
security create-keychain -p "$PASS" "$KEYCHAIN"
security set-keychain-settings "$KEYCHAIN"
security unlock-keychain -p "$PASS" "$KEYCHAIN"
identity "Prism Test Distribution" distribution codeSigning
# Lets codesign use the keys without a password prompt.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PASS" "$KEYCHAIN" >/dev/null
DIST_KEY="$DIR/distribution.key"

GROUPS_XML="<key>com.apple.security.application-groups</key><array><string>group.$BUNDLE_ID</string></array>
    <key>beta-reports-active</key><true/>"
profile "$DIR/app.mobileprovision" application-identifier "$BUNDLE_ID" "$GROUPS_XML"
profile "$DIR/widgets.mobileprovision" application-identifier "$BUNDLE_ID.widgets" "$GROUPS_XML"
profile "$DIR/app.provisionprofile" com.apple.application-identifier "$BUNDLE_ID" ""
profile "$DIR/widgets.provisionprofile" com.apple.application-identifier "$BUNDLE_ID.widgets" ""
rm -f "$DIR"/*.key "$DIR/profile.plist" "$DIR"/*.cnf

cat <<ENV
export PRISM_KEYCHAIN="$KEYCHAIN"
export PRISM_TEAM=$TEAM
export PRISM_IDENTITY="Prism Test Distribution"
# productbuild only signs with identities the system trusts: the test
# package stays unsigned (prism publish --dry-run warns about it).
export PRISM_INSTALLER_IDENTITY=-
export PRISM_PROFILE="$DIR/app.mobileprovision"
export PRISM_WIDGETS_PROFILE="$DIR/widgets.mobileprovision"
export PRISM_MAC_PROFILE="$DIR/app.provisionprofile"
export PRISM_MAC_WIDGETS_PROFILE="$DIR/widgets.provisionprofile"
export PRISM_TIMESTAMP=none
ENV
