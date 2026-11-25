#!/usr/bin/env python3
"""What App Store Connect checks about an Apple app's privacy, for the
bundlers (ios-run.sh, macos-app.sh).

  apple-privacy.py usage
      The Info.plist usage descriptions (NSCameraUsageDescription…) of
      Prism.toml's [permissions], from PRISM_PERMISSIONS (`name<TAB>text`
      lines), as <key>/<string> lines.

  apple-privacy.py manifest <os> <strict> <dir> <binary> [<dir> <binary>]…
      Writes PrivacyInfo.xcprivacy into each bundle <dir>: the app first,
      then its extensions. <os>: ios, tvos, watchos, visionos or macos.
      <strict> 1 (App Store builds for iOS, tvOS, watchOS, visionOS): what
      App Store Connect rejects fails the build instead of warning.

The manifest of each bundle declares the required-reason APIs its binary
references (ITMS-91053): Prism finds them in the binary (the symbols and
selectors Apple lists) and gives the reasons of its own uses:

  FileTimestamp   C617.1  metadata of files in the app's containers
  SystemBootTime  35F9.1  time elapsed between the app's events
  DiskSpace       E174.1  SQLite checks the volume before it writes

Other categories (UserDefaults, ActiveKeyboards) are the app's own: it
gives their reasons under [privacy.apis]. The app's manifest also carries
[privacy] (tracking, domains, collected data: PRISM_PRIVACY, JSON) and the
project's PrivacyInfo.xcprivacy, merged in.

The app's binary is also checked for protected APIs (CLLocationManager,
AVCaptureDevice…) without the usage description App Store Connect requires
for each (ITMS-90683): code from a crate that asks for a resource the app's
[permissions] doesn't declare.
"""

import json
import os
import plistlib
import re
import sys
from xml.sax.saxutils import escape

# [permissions] name → the Info.plist keys of its text.
USAGE = {
    "location": ["NSLocationWhenInUseUsageDescription", "NSLocationUsageDescription"],
    "location_always": ["NSLocationAlwaysAndWhenInUseUsageDescription", "NSLocationAlwaysUsageDescription"],
    "camera": ["NSCameraUsageDescription"],
    "microphone": ["NSMicrophoneUsageDescription"],
    "photos": ["NSPhotoLibraryUsageDescription", "NSPhotoLibraryAddUsageDescription"],
    "motion": ["NSMotionUsageDescription"],
    "face_id": ["NSFaceIDUsageDescription"],
    "contacts": ["NSContactsUsageDescription"],
    "calendars": ["NSCalendarsFullAccessUsageDescription", "NSCalendarsUsageDescription"],
    "reminders": ["NSRemindersFullAccessUsageDescription", "NSRemindersUsageDescription"],
    "bluetooth": ["NSBluetoothAlwaysUsageDescription", "NSBluetoothPeripheralUsageDescription"],
    "speech": ["NSSpeechRecognitionUsageDescription"],
    "tracking": ["NSUserTrackingUsageDescription"],
    "health": ["NSHealthShareUsageDescription", "NSHealthUpdateUsageDescription"],
    "nfc": ["NFCReaderUsageDescription"],
    "media_library": ["NSAppleMusicUsageDescription"],
    "homekit": ["NSHomeKitUsageDescription"],
    "siri": ["NSSiriUsageDescription"],
    "local_network": ["NSLocalNetworkUsageDescription"],
}

# Protected APIs → the [permissions] names whose text covers them (any of).
# Swift and Objective-C code reference these classes by their symbol.
PROTECTED = [
    (["location", "location_always"], ["CLLocationManager"]),
    (["camera", "microphone"], ["AVCaptureDevice"]),
    (["microphone"], ["AVAudioRecorder", "AVAudioApplication"]),
    (["photos"], ["PHPhotoLibrary"]),
    (["contacts"], ["CNContactStore"]),
    (["calendars", "reminders"], ["EKEventStore"]),
    (["bluetooth"], ["CBCentralManager", "CBPeripheralManager"]),
    (["speech"], ["SFSpeechRecognizer"]),
    (["tracking"], ["ATTrackingManager"]),
    (["health"], ["HKHealthStore"]),
    (["nfc"], ["NFCNDEFReaderSession", "NFCTagReaderSession", "NFCVASReaderSession"]),
    (["media_library"], ["MPMediaLibrary", "MPMediaQuery"]),
    (["homekit"], ["HMHomeManager"]),
    (["siri"], ["INPreferences"]),
    (["motion"], ["CMMotionActivityManager", "CMPedometer"]),
    # Face ID: iOS and visionOS only (Touch ID asks no text).
    (["face_id"], ["LAContext"]),
]

CATEGORY = "NSPrivacyAccessedAPICategory"
# Required-reason API categories → what references them in a binary:
# imported functions and constants (Mach-O symbols, `_` first) and
# Objective-C selectors (Swift calls Foundation through them).
REASON_APIS = {
    "FileTimestamp": {
        "symbols": ["stat", "fstat", "fstatat", "lstat", "getattrlist", "fgetattrlist",
                    "getattrlistat", "getattrlistbulk", "NSFileCreationDate",
                    "NSFileModificationDate", "NSURLContentModificationDateKey",
                    "NSURLCreationDateKey"],
        "selectors": ["fileModificationDate"],
    },
    "SystemBootTime": {
        "symbols": ["mach_absolute_time"],
        "selectors": ["systemUptime"],
    },
    "DiskSpace": {
        "symbols": ["statfs", "statvfs", "fstatfs", "fstatvfs", "getattrlist", "fgetattrlist",
                    "getattrlistat", "NSFileSystemFreeSize", "NSFileSystemSize",
                    "NSURLVolumeAvailableCapacityKey",
                    "NSURLVolumeAvailableCapacityForImportantUsageKey",
                    "NSURLVolumeAvailableCapacityForOpportunisticUsageKey",
                    "NSURLVolumeTotalCapacityKey"],
        "selectors": [],
    },
    "ActiveKeyboards": {
        "symbols": [],
        "selectors": ["activeInputModes"],
    },
    "UserDefaults": {
        "symbols": ["OBJC_CLASS_$_NSUserDefaults"],
        "selectors": [],
    },
}
# The reasons of Prism's own uses (the module docstring).
PRISM_REASONS = {
    "FileTimestamp": ["C617.1"],
    "SystemBootTime": ["35F9.1"],
    "DiskSpace": ["E174.1"],
}
# [privacy.apis]'s names, for the messages.
TOML_NAMES = {
    "FileTimestamp": "file_timestamp",
    "SystemBootTime": "system_boot_time",
    "DiskSpace": "disk_space",
    "ActiveKeyboards": "active_keyboards",
    "UserDefaults": "user_defaults",
}
APPROVED = {
    "FileTimestamp": ["DDA9.1", "C617.1", "3B52.1", "0A2A.1"],
    "SystemBootTime": ["35F9.1", "8FFB.1", "3D61.1"],
    "DiskSpace": ["85F4.1", "E174.1", "7D9E.1", "B728.1"],
    "ActiveKeyboards": ["3EC4.1", "54BD.1"],
    "UserDefaults": ["CA92.1", "1C8F.1", "C56D.1", "AC6B.1"],
}


def permissions():
    """[permissions] as (name, text) pairs, from PRISM_PERMISSIONS."""
    out = []
    for line in os.environ.get("PRISM_PERMISSIONS", "").splitlines():
        name, _, text = line.partition("\t")
        if name:
            out.append((name, text))
    return out


def usage():
    for name, text in permissions():
        if name not in USAGE:
            print("prism: unknown permission " + name, file=sys.stderr)
            continue
        for key in USAGE[name]:
            print("  <key>" + key + "</key><string>" + escape(text) + "</string>")


def names(data, words):
    """The words of `words` found in `data` as whole NUL-ended strings."""
    if not words:
        return set()
    pattern = re.compile(rb"\x00(" + b"|".join(re.escape(w.encode()) for w in words) + rb")(?:\$INODE64)?\x00")
    return {m.group(1).decode() for m in pattern.finditer(data)}


def scan(binary):
    """The required-reason categories `binary` references, each with what
    references it."""
    data = open(binary, "rb").read()
    found = {}
    for category, apis in REASON_APIS.items():
        hits = names(data, ["_" + s for s in apis["symbols"]]) | names(data, apis["selectors"])
        if hits:
            found[category] = sorted(h.lstrip("_") for h in hits)
    return data, found


def protected(data, os_name):
    """The [permissions] lists whose text the binary's protected APIs need,
    each with the class that needs it."""
    out = []
    for perms, classes in PROTECTED:
        if perms == ["face_id"] and os_name not in ("ios", "visionos"):
            continue
        hits = names(data, ["_OBJC_CLASS_$_" + c for c in classes])
        if hits:
            out.append((perms, sorted(h.replace("_OBJC_CLASS_$_", "") for h in hits)))
    return out


def merge_apis(into, entries):
    """Adds NSPrivacyAccessedAPITypes entries to `into` (category → reasons)."""
    for e in entries:
        category = e.get("NSPrivacyAccessedAPIType")
        if not isinstance(category, str):
            continue
        reasons = into.setdefault(category, [])
        for r in e.get("NSPrivacyAccessedAPITypeReasons", []):
            if r not in reasons:
                reasons.append(r)


def merge_collected(into, entries):
    """Adds NSPrivacyCollectedDataTypes entries to `into` (one per type)."""
    for e in entries:
        kind = e.get("NSPrivacyCollectedDataType")
        same = next((c for c in into if c["NSPrivacyCollectedDataType"] == kind), None)
        if same is None:
            into.append(dict(e))
            continue
        for flag in ("NSPrivacyCollectedDataTypeLinked", "NSPrivacyCollectedDataTypeTracking"):
            same[flag] = bool(same.get(flag)) or bool(e.get(flag))
        purposes = same.setdefault("NSPrivacyCollectedDataTypePurposes", [])
        for p in e.get("NSPrivacyCollectedDataTypePurposes", []):
            if p not in purposes:
                purposes.append(p)


def manifest(argv):
    os_name, strict, pairs = argv[0], argv[1] == "1", argv[2:]
    if len(pairs) < 2 or len(pairs) % 2:
        sys.exit("usage: apple-privacy.py manifest <os> <strict> <dir> <binary> [<dir> <binary>]…")
    problems, warnings = [], []
    config = json.loads(os.environ.get("PRISM_PRIVACY") or "{}")
    granted = {name for name, _ in permissions()}

    for i in range(0, len(pairs), 2):
        bundle, binary = pairs[i], pairs[i + 1]
        is_app = i == 0
        what = "the app" if is_app else next(
            (part for part in bundle.split(os.sep) if part.endswith(".appex")), "an extension")
        data, found = scan(binary)

        doc = {}
        apis = {}
        if is_app and config.get("manifest"):
            path = config["manifest"]
            try:
                with open(path, "rb") as f:
                    doc = plistlib.load(f)
            except Exception as e:  # noqa: BLE001 — any parse error is the file's
                sys.exit("prism: [privacy] manifest " + path + ": not a property list (" + str(e) + ")")
            if not isinstance(doc, dict):
                sys.exit("prism: [privacy] manifest " + path + ": the root must be a dictionary")
            merge_apis(apis, doc.get("NSPrivacyAccessedAPITypes", []))
        if is_app:
            merge_apis(apis, [
                {"NSPrivacyAccessedAPIType": e["category"], "NSPrivacyAccessedAPITypeReasons": e["reasons"]}
                for e in config.get("apis", [])
            ])
        for category in sorted(found):
            key = CATEGORY + category
            if category in PRISM_REASONS:
                merge_apis(apis, [{"NSPrivacyAccessedAPIType": key,
                                   "NSPrivacyAccessedAPITypeReasons": PRISM_REASONS[category]}])
            elif key not in apis:
                via = ", ".join(found[category])
                if is_app:
                    msg = ("%s references %s (%s), a required-reason API: say why under [privacy.apis] in Prism.toml "
                           "(%s = [\"…\"], Apple's reasons: %s) or in PrivacyInfo.xcprivacy"
                           % (what, category, via, TOML_NAMES[category], ", ".join(APPROVED[category])))
                else:
                    msg = "%s references %s (%s), which Prism declares no reason for" % (what, category, via)
                (problems if strict else warnings).append(msg)

        tracking = bool(doc.get("NSPrivacyTracking")) or (is_app and bool(config.get("tracking")))
        domains = list(doc.get("NSPrivacyTrackingDomains", [])) if is_app else []
        for d in config.get("tracking_domains", []) if is_app else []:
            if d not in domains:
                domains.append(d)
        collected = []
        if is_app:
            merge_collected(collected, doc.get("NSPrivacyCollectedDataTypes", []))
            merge_collected(collected, [{
                "NSPrivacyCollectedDataType": c["type"],
                "NSPrivacyCollectedDataTypeLinked": bool(c["linked"]),
                "NSPrivacyCollectedDataTypeTracking": bool(c["tracking"]),
                "NSPrivacyCollectedDataTypePurposes": list(c["purposes"]),
            } for c in config.get("collected", [])])
        if tracking and not domains:
            (problems if strict else warnings).append(
                "NSPrivacyTracking is true without NSPrivacyTrackingDomains: list the domains that track "
                "([privacy] tracking_domains in Prism.toml)")
        if tracking and "tracking" not in granted:
            (problems if strict else warnings).append(
                "NSPrivacyTracking is true: add `tracking = \"why\"` under [permissions] in Prism.toml "
                "(NSUserTrackingUsageDescription, the App Tracking Transparency prompt)")

        doc.update({
            "NSPrivacyTracking": tracking,
            "NSPrivacyTrackingDomains": domains,
            "NSPrivacyCollectedDataTypes": collected,
            "NSPrivacyAccessedAPITypes": [
                {"NSPrivacyAccessedAPIType": k, "NSPrivacyAccessedAPITypeReasons": v}
                for k, v in sorted(apis.items())
            ],
        })
        os.makedirs(bundle, exist_ok=True)
        with open(os.path.join(bundle, "PrivacyInfo.xcprivacy"), "wb") as f:
            plistlib.dump(doc, f)

        # Purpose strings: the app's binary holds every crate's code.
        if is_app:
            for perms, classes in protected(data, os_name):
                if granted.isdisjoint(perms):
                    msg = ("the app's code references %s, which App Store Connect accepts only with a purpose string "
                           "(ITMS-90683): add `%s = \"why the app needs it\"` under [permissions] in Prism.toml, "
                           "or drop the crate that asks for it" % (", ".join(classes), perms[0]))
                    (problems if strict else warnings).append(msg)

    for w in warnings:
        print("warning: " + w, file=sys.stderr)
    if problems:
        for p in problems:
            print("prism: " + p, file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "usage":
        usage()
    elif len(sys.argv) >= 2 and sys.argv[1] == "manifest":
        manifest(sys.argv[2:])
    else:
        sys.exit(__doc__)
