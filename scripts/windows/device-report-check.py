#!/usr/bin/env python3
"""Checks a PRISM_DEVICE_REPORT file (scripts/windows-device-apis.sh).

Fails when a capability says something other than the checks behind it,
when a part that needs no prompt did not work (the photo's file token, the
window's DataTransferManager), when a passkey error maps to the wrong
Prism error, or when the report says where the PC is. Prints one line per
API, then "device-report: OK".

    python3 scripts/windows/device-report-check.py <device-report.json>
"""

import json
import sys

# What WebAuthNGetErrorName calls each HRESULT (webauthn.h; E_INVALIDARG is
# one it does not name) and the Prism error passkey.rs `error` makes of it.
# passkey.rs ERROR_NAMES is the same table: a test there keeps them equal.
KNOWN = {
    "0x8009000F": ("InvalidStateError", "Platform"),  # NTE_EXISTS
    "0x80070032": ("ConstraintError", "Platform"),  # ERROR_NOT_SUPPORTED
    "0x80090029": ("ConstraintError", "Platform"),  # NTE_NOT_SUPPORTED
    "0x80090023": ("ConstraintError", "Platform"),  # NTE_TOKEN_KEYSET_STORAGE_FULL
    "0x80090027": ("NotSupportedError", "Unavailable"),  # NTE_INVALID_PARAMETER
    "0x80090035": ("NotAllowedError", "Cancelled"),  # NTE_DEVICE_NOT_FOUND
    "0x80090011": ("NotAllowedError", "Cancelled"),  # NTE_NOT_FOUND
    "0x800704C7": ("NotAllowedError", "Cancelled"),  # ERROR_CANCELLED
    "0x80090036": ("NotAllowedError", "Cancelled"),  # NTE_USER_CANCELLED
    "0x800705B4": ("NotAllowedError", "Cancelled"),  # ERROR_TIMEOUT
    "0x80070057": ("UnknownError", "Failed"),  # E_INVALIDARG
}
STATUSES = {"Ready", "Initializing", "NoData", "Disabled", "NotInitialized"}
PERMISSIONS = {"NotDetermined", "Granted", "Limited", "Denied", "Unavailable"}
PLACE = {"latitude", "longitude", "lat", "lon", "lng", "coordinate", "point"}

fails = []


def expect(ok, what):
    if not ok:
        fails.append(what)


def places(v, path="report"):
    """Keys that would say where the PC is."""
    if isinstance(v, dict):
        for k, x in v.items():
            if k.lower() in PLACE:
                yield f"{path}.{k}"
            yield from places(x, f"{path}.{k}")
    elif isinstance(v, list):
        for i, x in enumerate(v):
            yield from places(x, f"{path}[{i}]")


def kind(error):
    """`Platform { name: … }` → Platform, `Failed("…")` → Failed."""
    return error.split("(")[0].split("{")[0].strip()


def fix(v):
    """A position or watch result, in a few words."""
    if isinstance(v, str):
        return v
    if v.get("ok"):
        if "first_after_s" in v:
            return f"first event after {v['first_after_s']} s, ±{v['accuracy_m']:g} m"
        return f"±{v['accuracy_m']:g} m, {v['age_s']:g} s old"
    return f"error {v.get('error')}"


def location(r, caps):
    loc = r["location"]
    status = loc["locator_status"]
    present = (
        loc["policy_disable_location"] is False
        and loc["service_lfsvc"] is True
        and status in STATUSES
    )
    expect(caps["location"] == present, f"location: capability {caps['location']}, checks say {present}")
    perm = loc["permission"]
    expect(perm in PERMISSIONS, f"location: permission {perm!r}")
    if loc["access"] == "Allowed":
        expect(perm == "Granted", f"location: access Allowed, permission {perm}")
        expect(isinstance(loc["position"], dict), "location: access Allowed, no position asked")
        expect(isinstance(loc["watch"], dict), "location: access Allowed, no watch started")
    print(
        f"location: capability {caps['location']} (lfsvc {loc['service_lfsvc']}, "
        f"policy off {loc['policy_disable_location']}, status {status}); "
        f"access {loc['access']}, settings {loc['settings']}, permission {perm}; "
        f"position: {fix(loc['position'])}; watch: {fix(loc['watch'])}"
    )


def motion(r, caps):
    m = r["motion"]
    present = m["accelerometer"] is True or m["gyrometer"] is True
    expect(caps["motion"] == present, f"motion: capability {caps['motion']}, sensors say {present}")
    if present:
        expect(m["first_sample"] is True, "motion: the sensors gave no sample in 2 s")
    sensors = [k for k in ("accelerometer", "gravity_sensor", "linear_acceleration_sensor", "gyrometer", "inclinometer") if m[k]]
    print(f"motion: capability {caps['motion']} (sensors: {', '.join(sensors) or 'none'}; first sample {m['first_sample']})")


def camera(r, caps):
    cam = r["camera"]
    present = isinstance(cam["cameras"], int) and cam["cameras"] > 0 and cam["capture_ui"] == "Available"
    expect(caps["camera"] == present, f"camera: capability {caps['camera']}, checks say {present}")
    token = cam["photo_file_token"]
    expect(token.get("ok") is True and token.get("removed") is True, f"camera: photo file token {token}")
    print(
        f"camera: capability {caps['camera']} ({cam['cameras']} camera(s), Camera app capture UI "
        f"{cam['capture_ui']}); photo file token {'ok' if token.get('ok') else token}; "
        f"permission {cam['permission']} (app's webcam access {cam['app_webcam_access']}); "
        f"ApplicationData.Current: {cam['application_data_current']}"
    )


def share(r, caps):
    s = r["share"]
    expect(caps["share"] == (s["supported"] is True), f"share: capability {caps['share']}, IsSupported {s['supported']}")
    if s["supported"] is True:
        expect(s["window_manager"] == "ok", f"share: GetForWindow: {s['window_manager']}")
    print(f"share: capability {caps['share']} (IsSupported {s['supported']}); window's DataTransferManager {s['window_manager']}")


def passkeys(r, caps):
    p = r["passkeys"]
    present = p.get("dll") is True and p.get("api_version", 0) >= 1
    expect(caps["passkeys"] == present, f"passkeys: capability {caps['passkeys']}, webauthn.dll says {present}")
    if not p.get("dll"):
        print(f"passkeys: capability {caps['passkeys']} (no webauthn.dll)")
        return
    errors = p["errors"]
    for hr, (name, mapped) in errors.items():
        if hr not in KNOWN:
            fails.append(f"passkeys: {hr} ({name}) is not in the checker's table")
            continue
        known_name, want = KNOWN[hr]
        expect(name == known_name, f"passkeys: {hr} is {name}, expected {known_name}")
        expect(kind(mapped) == want, f"passkeys: {hr} ({name}) → {mapped}, expected {want}")
    missing = set(KNOWN) - set(errors)
    expect(not missing, f"passkeys: no mapping for {sorted(missing)}")
    print(
        f"passkeys: capability {caps['passkeys']} (webauthn.dll API {p['api_version']}, options "
        f"v{p['make_credential_options_version']}/v{p['get_assertion_options_version']}); "
        f"platform authenticator {p['platform_authenticator']}; {len(errors)} error mappings checked"
    )


def haptics(r, caps):
    h = r["haptics"]
    expect(caps["haptics"] == (h["default_device"] is True), f"haptics: capability {caps['haptics']}, default device {h['default_device']}")
    print(
        f"haptics: capability {caps['haptics']} (vibration access {h['access']}, "
        f"{h['devices']} device(s), waveforms {h['waveforms']})"
    )


def main(path):
    with open(path, encoding="utf-8") as f:
        r = json.load(f)
    caps = r["capabilities"]
    for check in (location, motion, camera, share, passkeys, haptics):
        try:
            check(r, caps)
        except (KeyError, TypeError, AttributeError) as e:
            fails.append(f"{check.__name__}: the report lacks {e}")
    for where in places(r):
        fails.append(f"privacy: {where} says where the PC is")
    if fails:
        for f in fails:
            print(f"device-report: FAIL: {f}", file=sys.stderr)
        return 1
    print("device-report: OK")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sys.exit(main(sys.argv[1]))
