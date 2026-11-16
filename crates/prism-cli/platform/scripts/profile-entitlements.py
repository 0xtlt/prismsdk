"""The entitlements to sign a bundle with, from its provisioning profile.

    profile-entitlements.py <profile plist> <output plist> <bundle id> app|extension

The profile's entitlements, for this bundle id: its application-identifier,
no wildcard keychain-access-groups, iCloud key-value storage and Sign in
with Apple only when the app asks for them (PRISM_ICLOUD=1,
PRISM_APPLE_SIGNIN=1).

The capability notes (iCloud, Sign in with Apple, Push Notifications with
PRISM_PUSH) are about the app's own profile: an extension's profile (the
widgets, <bundle id>.widgets) needs none of them, so it prints none.
"""

import os
import plistlib
import sys


def main():
    profile, output, bundle_id, role = sys.argv[1:5]
    if role not in ("app", "extension"):
        sys.exit(f"profile-entitlements.py: role {role!r}: use app or extension")
    app = role == "app"
    with open(profile, "rb") as f:
        p = plistlib.load(f)
    e = dict(p["Entitlements"])
    team = p["TeamIdentifier"][0]
    e["application-identifier"] = f"{team}.{bundle_id}"
    e.pop("keychain-access-groups", None)

    def note(text):
        if app:
            print(f"note: the provisioning profile of {bundle_id} {text}", file=sys.stderr)

    kv = "com.apple.developer.ubiquity-kvstore-identifier"
    if os.environ.get("PRISM_ICLOUD") == "1":
        if kv in e:
            e[kv] = f"{team}.{bundle_id}"
        else:
            note("has no iCloud: cloud_storage stays local (enable iCloud for the App ID in Xcode)")
    else:
        e.pop(kv, None)
    siwa = "com.apple.developer.applesignin"
    if os.environ.get("PRISM_APPLE_SIGNIN") != "1":
        e.pop(siwa, None)
    elif siwa not in e:
        note("has no Sign in with Apple: enable the capability for the App ID")
    if os.environ.get("PRISM_PUSH") and "aps-environment" not in e:
        note("has no Push Notifications capability: register_for_push will fail (enable it for the App ID)")
    with open(output, "wb") as f:
        plistlib.dump(e, f)


main()
